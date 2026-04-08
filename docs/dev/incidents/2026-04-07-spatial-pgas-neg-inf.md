# Incident: Spatial PGAS -inf complete-data log-likelihood

**Date:** 2026-04-07
**Status:** Fixed (three bugs). Gamma density disabled pending separate fix.
**Severity:** Blocking — spatial PGAS inference produced -inf on every sweep, making Bayesian inference impossible for multi-patch models.
**Fix commits:**
- `f64668f` — snapshot counts_before in SubstepRecord (simulate_reference)
- `b15cb39` — reference particle counts_before mismatch in CSMC-AS traceback
- `faffe8f` — near-zero rate/flow mismatch: soft handling + iota warning

## Fundamental vs. implementation

This incident has two layers.

**The implementation bug** was: the density used post-clamp counts while
`step_one` used pre-clamp counts. This is purely a coding error — the density
and simulator must agree on what state generated the draws. Fixed by storing
both snapshots in `SubstepRecord`.

**The fundamental issue** is: the Euler-multinomial approximation assumes
per-step exit probabilities are small ($p_{\text{total}} \ll 1$). When R0 is
large and dt = 1 day, $p_{\text{total}} = 1 - e^{-R_0 \cdot \gamma \cdot
\Delta t}$ can approach 1, meaning almost everyone exits S in one step. With
spatial models, multiple source groups draw from overlapping compartments via
the deferred-delta mechanism, and total withdrawals can exceed the population —
the overdraft that triggers clamping.

This is a known limitation of the Euler scheme. pomp has the same issue and
handles it the same way (clamp negative counts). The correct fix for the
approximation is smaller dt (subdivide each day into 4–10 substeps), which
keeps $p_{\text{total}} < 0.3$ and makes overdrafts vanishingly rare. But
smaller dt means more substeps, which means slower inference.

The `counts_before`/`counts_after` fix addresses the implementation bug. The
Euler approximation breakdown requires either smaller dt or the
merge-same-destination optimization (see Remaining Issues).

## Summary

The complete-data log-likelihood (`complete_data_loglik`) returned `-inf` for
spatial (multi-patch) models during PGAS inference. The root cause was a
mismatch between the compartment counts used by `step_one` to draw transitions
and the counts stored in the trajectory for density evaluation. Negative
clamping in `step_one` reduced compartment counts after draws, so the density
evaluated `Binom(k; n, p)` with `k > n`, which is mathematically impossible
and returns `-inf`.

## Background

PGAS (Particle Gibbs with Ancestor Sampling) requires evaluating the
complete-data log-likelihood over a stored trajectory. This decomposes into:

```
log p(y, X | θ) = log p(x₀|θ) + Σ_s log p(x_s | x_{s-1}, θ) + Σ_k log p(y_k | x, θ)
```

The transition density `log p(x_s | x_{s-1}, θ)` uses the Euler-multinomial
decomposition: total exits from source compartment `j` are
`Binom(n_exit; N_j, 1 - exp(-R·dt))`, then split proportionally among
competing transitions. The constraint `n_exit ≤ N_j` is fundamental —
binomial with `k > n` is undefined.

Spatial models have inter-patch coupling via importation terms (e.g.,
`c_ij * I_j / N_j`). These coupling terms can create transient negative
compartments when outflows exceed the current count in a single substep.
The simulator handles this with "negative clamping" in `step_one`: after
all draws, any compartment that went negative is clamped to 0.

## The bug

The trajectory's `SubstepRecord` stored only the post-step compartment counts
(after clamping). When the density evaluator needed the counts *before* a
substep, it read `trajectory.substeps[s-1].counts` — the *previous substep's
post-clamp* state. But `step_one` had evaluated propensities and drawn exits
from the *pre-clamp* state of the *current* substep (which is the post-step
state before clamping was applied).

The mismatch:

1. At substep `s`, `step_one` sees compartment `I[p5] = 264` (pre-clamp)
2. It draws `n_exit = 264` exits (all individuals leave)
3. Negative clamping reduces `I[p5]` to 249 (because inflows from other
   patches partially compensated, but the net was still an overcount)
4. The trajectory stores `counts[s] = [..., 249, ...]`
5. At substep `s+1`, the density evaluator reads
   `counts_before = trajectory.substeps[s].counts = [..., 249, ...]`
6. It evaluates `Binom(264, 249, p)` — but `k=264 > n=249`, so the result
   is `-inf`

This never affected single-patch models because clamping only triggers with
spatial coupling (importation terms create negative transients). It also didn't
affect the particle filter, which evaluates likelihood at observation times
only, not per-substep.

## Investigation timeline

The investigation involved collaboration between the engine developer and a
downstream vignettes agent testing a 5-patch SEIR polio model.

### Phase 1: Reproduction and isolation (commits `4a6c2e3`–`d3a5c82`)

The downstream agent reported that PGAS produced `-inf` on their spatial model.
Initial diagnostics were added to `complete_data_loglik` to identify which
substep first produced `-inf`. A standalone spatial density test suite was
created (`tests/spatial_density.rs`) with SIR, SIR+demography, two-patch, and
5-patch models.

**Key finding:** All density tests passed at true parameter values. The `-inf`
only occurred during actual PGAS inference, not in the standalone tests. This
led to an incorrect hypothesis that the issue was parameter-specific.

### Phase 2: Model-specific vs. code-level (commits `d223f02`–`234d120`)

The downstream agent provided their compiled IR and exact parameter values.
Testing with their model at their parameters *also passed* the round-trip
density test (100/100 seeds). This was confusing — the same model failed during
PGAS but passed in isolation.

**Key insight:** The difference was that the standalone test evaluated density
immediately after `simulate_reference` (using the same trajectory), while PGAS
evaluated density after a CSMC sweep had modified the trajectory. The CSMC
traceback reconstructed counts from history, and the history stored post-clamp
counts.

### Phase 3: Hypotheses explored and rejected

Several hypotheses were investigated:

1. **Gamma density indexing mismatch.** The gamma multiplier index tracking
   between `step_one` and `log_transition_density_substep` was found to be
   inconsistent. The gamma density was disabled (`if false {}`) but `-inf`
   persisted, ruling out gamma as the sole cause.

2. **Floating-point threshold divergence.** The zero-rate threshold in
   `step_one` (originally `0.0`) differed from the density evaluator (also
   `0.0` but with different floating-point behavior). Both were aligned to
   `1e-15`, but this was not the root cause.

3. **Merge-same-destination optimization.** A proposal from the upstream
   colleague suggested merging transitions with identical source AND
   destination stoichiometry to eliminate the multinomial split for spatial
   importation groups. This would be a correct architectural fix but was
   deferred — the snapshot fix was simpler and more direct.

### Phase 4: Root cause identified (commit `f2f614a`)

Enhanced diagnostics printed the exact `Binom(k, n, p)` arguments at the
failing substep. The output showed `Binom(264, 249, p)` — `k > n`. Once this
was visible, the cause was obvious: the density was using post-clamp counts as
`n`, but the flows were drawn from pre-clamp counts.

### Phase 5: Fix attempts

**Attempt 1 (commit `c57ffe6`):** Added `n_exit > n_src` guards and capping
logic. This was a bandaid — it prevented `-inf` but changed the statistical
model (capping `n` to `n_exit` gives `Binom(k; k, p) = p^k`, which is wrong).

**Attempt 2 (commit `f64668f`, final fix):** Store a pre-step snapshot
(`counts_before`) in `SubstepRecord` alongside the post-step state
(`counts_after`). The density evaluator reads `counts_before` directly —
the exact counts that `step_one` used when drawing exits. This is
mathematically correct: the density evaluates the probability of the observed
flows given the state that actually generated them.

### Phase 6: Second bug — CSMC reference counts_before (commit `b15cb39`)

After deploying the snapshot fix, the downstream agent rebuilt and still
got `-inf`: `Binom(677, 670, p)` at `src_comp_idx=3`. This was NOT a
clamping issue — it was a separate bug in `csmc_as`.

In CSMC-AS, each substep: (1) resamples particles, (2) saves
`prev_counts[j] = counts[j]`, (3) propagates free particles, (4) clamps
the reference particle to its stored trajectory. Step 2 saves
`prev_counts[j_ref]` from the post-resample state — which after
resampling could be *any* particle's state, not the reference's actual
pre-step state. But the reference's flows (`ref_rec.flows`) at step 4
were drawn from `ref_rec.counts_before`.

The traceback paired `counts_before = prev_counts[j_ref]` (wrong) with
`flows = ref_rec.flows` (drawn from a different state), producing
`k > n → -inf`. Fix: after step 4, overwrite
`prev_counts[j_ref] = ref_rec.counts_before`.

## The fixes

### Fix 1: Snapshot in SubstepRecord (simulate_reference)

`SubstepRecord` was changed from:

```rust
pub struct SubstepRecord {
    pub counts: Vec<i64>,    // post-clamp
    pub flows: Vec<u64>,
    pub gammas: Vec<f64>,
}
```

to:

```rust
pub struct SubstepRecord {
    pub counts_before: Vec<i64>,  // pre-step snapshot (what step_one saw)
    pub counts_after: Vec<i64>,   // post-step, post-clamp
    pub flows: Vec<u64>,
    pub gammas: Vec<f64>,
}
```

All density evaluation paths (`complete_data_loglik`, `complete_data_loglik_grad`,
`log_transition_density_substep`) now use `rec.counts_before` instead of
deriving counts from the previous substep.

All trajectory construction paths (`simulate_reference`, CSMC-AS traceback)
now store both fields. The CSMC history tracks `history_counts_before` (the
pre-step particle states) and `history_counts_after` (post-step states).

**Files changed:** `pgas.rs`, `pgas_grad.rs`, `cli/fit/pgas.rs`,
`tests/pgas_resume.rs`, `tests/spatial_density.rs`

**Memory overhead:** One extra `Vec<i64>` per substep (n_compartments integers).
For a 5-patch SEIR model (20 compartments) with 1000 substeps, this adds
~160 KB per trajectory — negligible compared to the particle array.

### Fix 2: Reference particle counts_before in CSMC-AS

After clamping the reference particle (step 3 in the CSMC loop), overwrite
`prev_counts[j_ref]` with the reference's actual pre-step state:

```rust
prev_counts[j_ref].copy_from_slice(&ref_rec.counts_before);
```

This ensures the history correctly pairs the reference's pre-step state
with its flows, regardless of what resampling did to the `j_ref` slot.

### Fix 3: Near-zero rate/flow mismatch (iota warning)

When `step_one` evaluates a spatial importation expression like
`c_ij * I_j / N_j * S_i`, floating-point arithmetic can produce a
near-zero but nonzero rate (e.g., `1e-16`) even when `I_j = 0`. If
step_one draws `flow=1` from this near-zero rate, the density evaluator
may recompute the rate as exactly zero (or below `RATE_EPSILON`) and
reject the trajectory.

Before this fix, any `flow > 0` with `rate ≤ RATE_EPSILON` returned
`-inf`. After:

- **rate == 0.0 exactly, flow > 0:** Still `-inf`, but emits a one-time
  warning: "transition X has rate=0 but flow=N — consider adding a
  seeding term (iota)." This catches the model specification issue where
  infection rates vanish when a compartment empties.
- **0 < rate ≤ RATE_EPSILON, flow > 0:** Include the transition in the
  multinomial with its tiny rate. The Binomial density gives a very
  negative but finite score, correctly penalizing the unlikely event
  without hard-rejecting the trajectory.

User-facing documentation added to `docs/inference.md` under "Spatial
models and seeding (iota)".

## Remaining issues

### Gamma density disabled

The gamma multiplier density (`log Gamma(g; dt/σ², σ²/dt)`) is disabled with
`if false {}` in `complete_data_loglik`. The gamma index tracking between
`step_one` and the density evaluator is fragile: `step_one` only pushes to
`gamma_used` when an overdispersed transition has positive rate, but the
density evaluator's index advancement logic didn't perfectly mirror this.

Disabling the gamma density is statistically valid for now — the transition
density already constrains `σ²` through `p_total = 1 - exp(-R·g·dt)`, so the
parameter is still identified. The gamma density adds a prior-like constraint
that improves mixing but is not essential. Re-enabling requires careful
alignment of gamma indexing between simulation and density evaluation.

### Merge-same-destination (architectural alternative)

An upstream colleague proposed merging transitions that share both source AND
destination compartments. In spatial models, each patch's infection transition
has the same stoichiometry (`S_i → I_i`) but different rate expressions (local
vs. importation). The Euler-multinomial split assigns flows to each sub-rate,
but only the *total* flow matters for state evolution.

Merging these would eliminate the multinomial split density entirely for
importation groups, removing the source of floating-point fragility. This is
the "right" architectural fix but requires changes to the compiler's
transition grouping and to `step_one`'s source-group logic. Deferred to a
future iteration.

## Lessons learned

1. **Clamping creates a simulation/density gap.** Any post-hoc state
   modification (clamping, balancing, events) that isn't reflected in the
   density creates a mismatch. The density must evaluate against the state
   that *generated* the draws, not the state that *resulted* from them.

2. **Round-trip tests are necessary but not sufficient.** The standalone
   density test (`simulate_reference` → `complete_data_loglik`) passed 100/100
   seeds because it used the same trajectory object. The bug only manifested
   when the trajectory was reconstructed from CSMC history, which stored
   post-clamp counts. Testing density round-trips after CSMC reconstruction
   would have caught this earlier.

3. **Spatial models are qualitatively different.** Single-patch models never
   trigger negative clamping, so single-patch tests provide no coverage for
   this class of bugs. Spatial density tests should be part of the standard
   regression suite.

4. **Store the generating state, not the resulting state.** When a record
   needs to support both "what happened next" (simulation) and "how likely was
   what happened" (density), store the input state explicitly rather than
   deriving it from adjacent records. The derivation breaks when there are
   non-invertible transformations (like clamping) between steps.

## Hardening recommendations

These follow-up changes would prevent recurrence and catch related issues
earlier.

### 1. Euler approximation warning

Add a runtime check for $p_{\text{total}} > 0.5$ during the particle filter
validation step. When the exit probability is high, the Euler-multinomial
approximation is breaking down and the user should reduce dt.

```rust
// In step_one or the pfilter validation pass:
if p_total > 0.5 {
    n_high_p += 1;
}
// After simulation:
if n_high_p > 0 {
    log::warn!("{} substeps had exit probability > 0.5. \
        Consider reducing dt (e.g., dt = 0.25) for numerical stability.", n_high_p);
}
```

### 2. Centralize the rate epsilon — DONE (`19ac52c`)

`RATE_EPSILON = 1e-15` defined once in `chain_binomial.rs`, imported by
`pgas.rs`.

### 3. Debug assertions in step_one — DONE (`19ac52c`, `44b28d7`)

`debug_assert!(n_exit <= n_src)` in `step_one`, `simulate_reference`,
and `csmc_as` traceback.

### 4. Trace-gated -inf logging in distribution functions

Add checked wrappers on `binom_logpmf`, `poisson_logpmf`, etc. that trace-log
when returning `-inf` (gated behind `CAMDL_TRACE_STEPS`). Currently, `-inf`
propagates silently through summation and is only caught by the cumulative
check in `complete_data_loglik`. Per-term logging would have identified the
`k > n` binomial immediately.

### 5. Clean up existing debug diagnostics — DONE (`19ac52c`)

All density diagnostics gated behind `trace_enabled()`. `CAMDL_VERIFY_DENSITY`
removed. Gamma density `if false {}` block replaced with TODO comment.

### 6. Near-zero rate soft handling + iota warning — DONE (`faffe8f`)

Near-zero rates with nonzero flow are included in the multinomial rather than
hard-rejected. Truly zero rates emit a one-time warning about adding iota.
User-facing guidance added to `docs/inference.md`.
