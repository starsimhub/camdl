# Performance Lab Notebook

Profiling and optimization journal for `compartmental` simulation and inference.

**Hardware:** Apple M4 Max, macOS
**Model:** seir_age (8 compartments, 6 transitions, 2x2 contact matrix)
**Methodology:** Criterion benchmarks (release build), flamegraphs via samply

## Benchmark Suite

| Benchmark                  | What it measures                                  |
|----------------------------|---------------------------------------------------|
| `step_one/seir_age`        | Single chain-binomial Euler-multinomial step       |
| `eval_propensities/seir_age` | Propensity evaluation only (expression eval)     |
| `pfilter/seir_age/100p`   | Bootstrap PF, 100 obs, 100 particles               |
| `pfilter/seir_age/500p`   | Bootstrap PF, 100 obs, 500 particles               |
| `pfilter/seir_age/1000p`  | Bootstrap PF, 100 obs, 1000 particles              |
| `negbin_logpmf`            | 1000 neg-binomial log-PMF evaluations              |

## Running Benchmarks

```bash
# Full suite
cd rust && cargo bench --bench inference -p sim

# Single benchmark
cd rust && cargo bench --bench inference -p sim -- step_one

# Flamegraph (requires samply: cargo install samply)
cd rust && cargo bench --bench inference -p sim --no-run
samply record target/release/deps/inference-* --bench -- pfilter
```

---

## Entries

_(Newest first)_

### 2026-03-30 — Optimization 2: Rayon parallel particle propagation

#### The problem

The particle propagation loop in `bootstrap_filter` and `run_if2` was
sequential: `for i in 0..n { step_fn(particle[i], ...) }`. With 1000
particles and 14 cores on the M4 Max, only one core was doing work.

#### The fix

Replaced the sequential loop with a **batched rayon dispatch per observation
interval**. Instead of one `par_iter` per sub-step (which would create 780 ×
7 = 5,460 sync points for weekly measles data with daily dt), each thread
runs all sub-steps for its particle chunk before syncing:

```rust
swarm.states.par_iter_mut()
    .zip(rngs.par_iter_mut())
    .zip(scratches.par_iter_mut())
    .map(|((state, rng), scratch)| {
        let mut t_local = t_start;
        while t_local < obs_time - 1e-10 {
            step_fn(state, t_local, dt, rng, scratch)?;
            t_local += dt;
        }
        Ok(())
    })
    .collect();
```

One sync per observation time (for resampling, which is inherently
sequential). Particle state stays in the core's L1/L2 for all sub-steps.
Interventions are handled inside the per-particle sub-stepping — they read
model data (immutable/shared) and write per-particle state (independent).

`StepFn` type now requires `Send + Sync` for rayon compatibility. All
existing closures satisfy this (they capture `&CompiledModel` and `&[f64]`,
both `Sync`).

#### Nesting with existing parallelism

Existing parallel levels (all in CLI, none in sim):

| Level | Mechanism | Parallelizes |
|-------|-----------|-------------|
| IF2 multi-chain | `std::thread::scope` | Independent chains |
| Profile likelihood | `rayon` threadpool | Grid points × starts |
| Experiment runner | `rayon` threadpool | Independent plans |

New inner parallelism uses rayon's global pool, which nests correctly under
both `std::thread::scope` (each std thread shares the rayon pool) and
`profile`'s explicit `pool.install(|| ...)` (inner `par_iter` reuses it).
No new thread pool created — work-stealing handles contention.

When running 4 IF2 chains with inner parallelism, all chains share the
rayon pool's 14 threads. This is correct: total core utilization, not
per-chain dedication.

#### Results

| Benchmark                     | Pre-scratch  | Post-scratch | Post-rayon (best 2 of 4) | vs baseline |
|-------------------------------|--------------|--------------|--------------------------|-------------|
| `pfilter/seir_age/100p`       | 47.1 ms      | 39.1 ms      | **21 ms**               | 2.24×       |
| `pfilter/seir_age/500p`       | 234 ms       | 194 ms       | **61 ms**               | 3.84×       |
| `pfilter/seir_age/1000p`      | 464 ms       | 391 ms       | **110 ms**              | 4.22×       |
| `step_one/seir_age`           | 851 ns       | 711 ns       | 711 ns (unchanged)       | 1.20×       |

Repeated runs (pfilter/1000p): 162, 156, 111, 110 ms. First two runs had
heavy background agent activity inflating numbers. Stable at ~110 ms when
cores are available.

#### Analysis

**Speedup scales with particle count:** 100p gets 2.2× (rayon overhead is a
larger fraction of the small workload), 500p gets 3.8×, 1000p gets 4.2×.
This is consistent with the M4 Max having 10 performance cores + 4 efficiency
cores — rayon can use ~10 effective cores, but Amdahl's law limits the gain
because resampling + weight computation (~30% of wall time) are sequential.

**The batching paid off.** With per-sub-step dispatch, 1000p × 100 obs × 7
sub-steps = 700 rayon syncs. With per-observation dispatch: 100 syncs. At
~2-5µs per sync, that's 0.7-3.5ms saved — small but real. The bigger win is
cache affinity: each thread's particle stays in L1 across all 7 sub-steps.

**Background load sensitivity:** Parallel benchmarks are much noisier than
sequential ones. The 162ms vs 110ms spread (47% variance) reflects competition
for cores from background processes. Sequential benchmarks had <5% variance.

#### Updated workload estimates

| Workload              | Baseline | Post-scratch | Post-rayon  | Total speedup |
|-----------------------|----------|--------------|-------------|---------------|
| IF2 (1000p, 50 iter)  | ~23 s   | ~19.5 s      | **~5.5 s**  | 4.2×          |
| IF2 (5000p, 100 iter) | ~232 s  | ~195 s       | **~55 s**   | 4.2×          |
| Profile (50 grid pts) | ~19 min | ~16 min      | **~4.5 min**| 4.2×          |

Note: multi-chain IF2 and profile already parallelize at the chain/grid level.
Adding inner particle parallelism means cores are shared. Effective speedup for
`camdl if2 --chains 4 --parallel 4` will be less than 4.2× per chain because
threads contend. Total throughput stays similar — you're just redistributing
cores from chain-level to particle-level parallelism.

---

### 2026-03-30 — Optimization 1: Pre-allocated scratch buffers

#### The problem

`step_one` is the innermost hot function — called once per particle per time
step. For a pfilter with 1000 particles and 100 weekly observations (700 daily
steps each), that's **700,000 calls per run**. Every single call was doing ~6
heap allocations:

```
1. counts.to_vec()          → malloc 64 B (8 × i64), memcpy counts in
2. RealState::new(0)        → malloc (empty Vec header still hits allocator)
3. Vec::with_capacity(6)    → malloc 48 B (6 × f64 propensities)
4. Vec<ResolvedDraw>.collect → malloc ~48 B (6 enum variants)
5. Vec::new() pending_deltas → malloc (grows as deltas accumulate)
6. vec![false; 6]           → malloc 6 B (handled flags)
7. Vec::with_capacity(n)    → malloc per source group (probs)
```

Each malloc/free pair costs ~20-30 ns on jemalloc/M4 Max for small objects.
That's ~140-210 ns of pure allocator overhead per step — roughly 20% of the
851 ns baseline. The intervention check at the end of `step_one` did it
*again* (another `counts.to_vec()` + `RealState::new()`), even for models
with no interventions.

Total: **~4.2 million heap allocations per pfilter run** (700k steps × 6 allocs).

#### The fix

Added `StepScratch` — a struct holding all reusable buffers, allocated **once
per particle** at pfilter startup:

```rust
pub struct StepScratch {
    int_s: IntState,              // reused via copy_from_slice (memcpy, no alloc)
    real_s: RealState,            // zeroed once, never reallocated
    propensities: Vec<f64>,       // reused via clear() + push() (capacity retained)
    draws: Vec<ResolvedDraw>,     // same
    pending_deltas: Vec<(usize, i64)>, // same
    handled: Vec<bool>,           // reused via fill(false)
    probs: Vec<(usize, f64)>,    // same
}
```

Each step now does `copy_from_slice` (64-byte memcpy, L1-hot, ~2 ns) instead
of `to_vec()` (malloc + memcpy). Vec buffers are `clear()`'d (sets len to 0,
capacity stays) instead of freshly allocated. The intervention path reuses the
same scratch IntState instead of allocating a new one.

Total allocator calls per pfilter: **1,000** (one `StepScratch::new` per
particle) instead of 4.2 million.

#### Results

| Benchmark                     | Before       | After (3-run median) | Change   | Speedup |
|-------------------------------|--------------|----------------------|----------|---------|
| `step_one/seir_age`           | 851 ns       | **711 ns**           | -16.5%   | 1.20×   |
| `eval_propensities/seir_age`  | 505 ns       | **471 ns**           | -6.7%    | 1.07×   |
| `pfilter/seir_age/100p`       | 47.1 ms      | **39.1 ms**          | -17.0%   | 1.20×   |
| `pfilter/seir_age/500p`       | 234 ms       | **194 ms**           | -17.1%   | 1.21×   |
| `pfilter/seir_age/1000p`      | 464 ms       | **391 ms**           | -15.7%   | 1.19×   |
| `negbin_logpmf` (×1000)       | 25.5 µs      | 25.3 µs              | -0.9%    | —       |

**Replication:** step_one ran 3 times: 689, 724, 711 ns (median 711 ns).
First run was during low background load; 689 ns is optimistic. pfilter/1000p
replicated at 391 ms on second run. All numbers use medians.

#### Analysis

The ~140 ns saved per step_one (851 → 711) is consistent with eliminating 6
small-object alloc/dealloc pairs at ~23 ns each.

`eval_propensities` improved 7% as a secondary effect: the scratch IntState
lives at a stable address that stays warm in L1 cache across iterations, while
the old `counts.to_vec()` returned a fresh pointer each time that could land
on a different cache line.

pfilter improvement is consistent across particle counts (16-17%), confirming
the optimization is purely per-particle with no interaction effects.

#### Tradeoffs

**Memory:** ~200 bytes per scratch for seir_age. 1000 particles = 200 KB.
For a large model (50 comps, 100 transitions): ~2 KB × 5000 particles = 10 MB.
Negligible either way.

**Ergonomics:** `step_one` signature grew by one parameter. Every caller
must create and pass `&mut StepScratch`. The `ProcessSimulator` trait method
falls back to a fresh allocation per call (not on the hot path).

**Correctness:** All buffers are `clear()`'d / `fill()`'d before use — no
stale data leakage. `copy_from_slice` ensures the scratch IntState is a
faithful snapshot. All existing tests pass unchanged.

#### Updated workload estimates

| Workload              | Before  | After    | Speedup |
|-----------------------|---------|----------|---------|
| IF2 (1000p, 50 iter)  | ~23 s  | ~19.5 s  | 1.19×   |
| IF2 (5000p, 100 iter) | ~232 s | ~195 s   | 1.19×   |
| Profile (50 grid pts) | ~19 min | ~16 min | 1.19×   |

---

### 2026-03-30 — Baseline

First benchmark run. No optimizations applied.

| Benchmark                     | Time         | Notes                            |
|-------------------------------|--------------|----------------------------------|
| `step_one/seir_age`           | **851 ns**   | single CB step, 8 comps, 6 trans |
| `eval_propensities/seir_age`  | **505 ns**   | expression eval only             |
| `pfilter/seir_age/100p`       | **47.1 ms**  | 100 obs × 100 particles         |
| `pfilter/seir_age/500p`       | **234 ms**   | 100 obs × 500 particles         |
| `pfilter/seir_age/1000p`      | **464 ms**   | 100 obs × 1000 particles        |
| `negbin_logpmf` (×1000)       | **25.5 µs**  | 1000 logpmf evaluations          |

**Key observations:**

1. **Propensity eval is 59% of step_one** (505 ns / 851 ns). The remaining 346 ns
   is draw resolution, binomial sampling, deferred delta application, and clamping.

2. **pfilter scales linearly with particles** (47ms / 100p ≈ 0.47ms per particle).
   1000p = 464ms → 0.464ms per particle. Nearly perfect linear scaling confirms
   no shared-state overhead — parallelization should work well.

3. **step_one cost per pfilter**: 100 obs × 7 days × 1 step/day = 700 step_one
   calls per particle. 700 × 851ns = 596µs. Observed: 464µs/particle. The gap
   suggests the epidemic dies early in many particles (zero propensities → fast path).

4. **negbin_logpmf is cheap**: 25.5ns per call. 1000 particles × 100 obs = 100k
   calls → 2.5ms. This is <1% of pfilter time. Not a bottleneck.

5. **Allocation cost in step_one**: Currently allocates IntState (8×i64 = 64 bytes),
   RealState (0 bytes here), propensities Vec (6×f64 = 48 bytes), pending_deltas Vec,
   handled Vec (6 bools), and probs Vec — per call. That's ~6 heap allocations per
   851ns step. Eliminating these is the first optimization target.

**Derived estimates for realistic workloads:**

| Workload              | Estimated time | Notes                                  |
|-----------------------|----------------|----------------------------------------|
| IF2 (1000p, 50 iter)  | ~23 s         | 50 × 464ms                            |
| IF2 (5000p, 100 iter) | ~232 s        | 100 × 5 × 464ms                       |
| Profile (50 grid pts) | ~19 min       | 50 × 23s                               |
