---
status: closed
date: 2026-03-28
note: All issues from this review have been addressed and incorporated.
---

# Code Review #9 — Inference Implementation

**Codebase:** ~2100 new lines. `inference/` module (obs_loglik, resampling,
particle_filter, if2, types), CLI commands (pfilter, if2, profile, eval), tests.

---

## What's correct

### obs_loglik.rs — mathematically solid

Hand-rolled lgamma (Lanczos g=7, ~15 digits). NegBinomial, Normal, Poisson,
discretized Normal all check out against scipy reference values. Tolerance
parameter matches pomp's `tol`. No external deps.

### Resampling — correct

Systematic resampling with log-weight normalization. Edge cases tested (uniform,
degenerate, proportional).

### Particle filter — correct algorithm

Per-particle RNG streams derived from seed. Weighted prediction diagnostics. ESS
tracking. Sub-stepping between observations.

### IF2 — correctly structured

Cooling schedule matches pomp's `cooling.fraction.50` (per-step, not
per-iteration). Delta method `transformed_sd` conversion is correct. Multi-chain
with Rhat. Regime presets wired.

### DrawMethod enum — clean refactor

Replaces the old `overdispersion: Option<Expr>` with a proper enum:
`Poisson | Overdispersed(Expr) | Deterministic`. Single dispatch point in
step_one. Good.

### camdl eval — clean and useful

Compartment reference detection prevents confusing errors. Resolves forcing
functions and parameters. Time-grid evaluation works.

---

## Issues

### 1. BUG (Critical): step_one does NOT handle interventions

`step_one` (chain_binomial.rs:287-398) does pure dynamics — no intervention
checking. Interventions are only applied in `run_chain_binomial`'s main loop
(line 242). The particle filter calls `step_one` in its inner loop but never
calls `apply_interventions_at`.

**Impact:** For the He et al. model, the `school_entry` transfer (B_hold → S on
day 251 each year) never fires during pfilter or IF2. The birth cohort
accumulates in B_hold forever and never enters the susceptible pool. This
produces a systematic downward bias in susceptible supply and could cause
significant loglik discrepancy vs pomp.

**Fix options:**

(A) Add intervention handling to the PF sub-stepping loop:

```rust
while t < obs.time - 1e-10 {
    let step_dt = dt.min(obs.time - t);
    for (i, state) in swarm.states.iter_mut().enumerate() {
        step_fn(state, t, step_dt, &mut rngs[i])?;
    }
    t += step_dt;
    // Apply interventions to ALL particles at time t
    for state in &mut swarm.states {
        apply_interventions_to_particle(state, model, t, tolerance);
    }
}
```

(B) Make `step_one` accept an intervention list and handle them internally. This
is better because callers don't need to know about interventions:

```rust
pub fn step_one(
    model: &CompiledModel,
    counts: &mut [i64],
    flows: &mut [u64],
    params: &[f64],
    t: f64,
    dt: f64,
    rng: &mut StatefulRng,
    intervention_times: &[f64],  // NEW
) -> Result<(), SimError>
```

Option (B) is cleaner. The intervention check is one scan of the sorted time
list — negligible cost.

**Priority: Fix before any inference validation.** Without this, pfilter and IF2
results on models with interventions are wrong.

### 2. BUG (Medium): step_one allocates on every call

Line 303: `let int_s = IntState { counts: counts.to_vec() };` allocates a new
Vec every step. The PF calls step_one ~27M times for one measles pfilter (5000
particles × 780 obs × 7 substeps). That's 27M allocations of ~80 bytes each =
2.2 GB allocation churn.

Line 306: `let mut propensities = Vec::with_capacity(n_transitions);` is another
allocation per call.

**Fix:** Pre-allocate scratch buffers and pass them in:

```rust
pub fn step_one(
    model: &CompiledModel,
    counts: &mut [i64],
    flows: &mut [u64],
    params: &[f64],
    t: f64,
    dt: f64,
    rng: &mut StatefulRng,
    scratch: &mut StepScratch,  // pre-allocated buffers
) -> Result<(), SimError>
```

Where `StepScratch` holds the IntState, propensities Vec, and draw method
resolutions. One per particle (or per thread with rayon).

**Impact:** Not a correctness bug, but 27M allocations will dominate runtime.
Fixing this should give 3-5× speedup on pfilter.

### 3. DESIGN: pfilter dmeasure signature differs from IF2

pfilter: `Fn(f64, f64) -> f64` (projected, observed) IF2:
`Fn(f64, f64, &[f64]) -> f64` (projected, observed, params)

For fixed-parameter pfilter, capturing rho/k in the closure is fine. But it
means you can't share the dmeasure between pfilter and IF2 without wrapping.
Minor inconsistency — consider unifying to the 3-arg signature and having
pfilter just pass `&params`.

### 4. DESIGN: Regime defaults use sentinel detection

```rust
if n_particles == 2000 { n_particles = 200; }  // scout
```

If the user explicitly passes `--particles 2000 --regime scout`, scout won't
override because 2000 is the default. Should use `Option<usize>` for
user-specified values and only apply regime defaults when `None`.

### 5. DESIGN: No IVP parameter support

IF2Param has no `ivp: bool` flag. Initial conditions (S₀, E₀, I₀) are perturbed
at every observation time, same as rate parameters. For He et al., perturbing S₀
at week 400 makes no sense — it's a fixed initial condition.

**Fix:** Add `ivp: bool` to IF2Param. In the perturbation loop, skip IVP params
when `obs_index > 0`:

```rust
for spec in if2_params {
    if spec.ivp && obs_index > 0 { continue; }
    // ... perturb
}
```

CLI: `--rw-sd "S0=5000(ivp),I0=10(ivp)"`

### 6. MINOR: Rhat naming

The Rhat computation uses IF2 iteration parameter means across chains, not
posterior samples. This is the right diagnostic for IF2 (do chains agree on the
MLE?) but should be documented as "IF2 chain convergence diagnostic" not "Rhat"
— to avoid confusion with the Gelman-Rubin Rhat used for MCMC convergence.

### 7. MINOR: particle_filter.rs clone during resampling

Line 127: `let old_states: Vec<ParticleState> = swarm.states.clone();` clones
ALL particle states before resampling. For 5000 particles × 10 compartments,
this is 50K i64 values cloned per observation (780 times). Total: 390M values
cloned.

**Fix:** Double-buffer pattern — maintain two state arrays and swap between them
during resampling. No cloning needed:

```rust
let mut states_a: Vec<ParticleState> = ...;
let mut states_b: Vec<ParticleState> = ...;
// After resampling:
for (i, &src) in indices.iter().enumerate() {
    states_b[i].counts.copy_from_slice(&states_a[src].counts);
    states_b[i].flow_accumulators.copy_from_slice(&states_a[src].flow_accumulators);
}
std::mem::swap(&mut states_a, &mut states_b);
```

### 8. MINOR: normal_cdf accuracy

The Abramowitz & Stegun 7.1.26 approximation (line 73-87) has max error 1.5e-7.
For the discretized Normal logpmf at extreme tails (e.g., y=0 when mean=500),
the CDF difference is ~1e-100, and the approximation error of 1e-7 means the
result is dominated by approximation noise. The `tol` floor (1e-18) saves this
for the logpmf, but the raw `normal_cdf` function shouldn't be trusted below
~1e-6.

For inference this is fine — particles that predict 500 when data is 0 are
effectively dead regardless of whether their log-weight is -41 or -45. But
document the accuracy bound.

---

## What's working well

**The inference architecture is clean.** ProcessSimulator trait separates the
simulation step from inference algorithms. step_one extracted from
chain_binomial. Types module with flat ParticleState. All the right
abstractions.

**The CLI is well-designed.** Regime presets reduce cognitive load.
Multi-chain + Rhat gives immediate convergence feedback. The parameter trace
output is directly plottable.

**Testing is solid.** Determinism, variance scaling, ESS bounds, known-value
lgamma, scipy-matched discretized normal. The PF test on the pure death model is
a good analytical benchmark.

**The profile command** is immediately useful for identifiability analysis —
parallel grid search with multi-start IF2.

---

## Priority

| # | Issue                     | Impact                                                          | Fix effort |
| - | ------------------------- | --------------------------------------------------------------- | ---------- |
| 1 | Interventions in step_one | **Correctness** — wrong results on any model with interventions | ~30 lines  |
| 2 | Allocation in step_one    | **Performance** — 3-5× slower than necessary                    | ~50 lines  |
| 5 | IVP parameters            | **Correctness** for IF2 — wrong perturbation schedule           | ~15 lines  |
| 4 | Regime sentinel detection | **UX** — surprising override behavior                           | ~20 lines  |
| 7 | Resampling clone          | **Performance** — unnecessary allocation per obs                | ~20 lines  |
| 3 | dmeasure signature        | **Consistency** — minor                                         | ~10 lines  |
| 6 | Rhat naming               | **Documentation** — minor                                       | ~5 lines   |
| 8 | normal_cdf accuracy       | **Documentation** — minor                                       | ~3 lines   |

**Fix #1 first.** It's a correctness bug that affects every model with
interventions. Without it, He et al. pfilter/IF2 validation is meaningless.
