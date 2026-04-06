---
status: closed
date: 2026-04-06
note: All issues from this review have been addressed and incorporated.
---

# Code Review #12 — NUTS, PGAS, PMMH, Autodiff

**Codebase:** ~23,500 LOC. New since Review #10: PGAS (1090 lines),
PMMH (478 lines), NUTS (315 lines), PGAS gradient (278 lines),
correlated PF (378 lines), CLI sampling (405 lines), CLI fit/pmmh
(620 lines), CLI fit/pgas (386 lines), gradient check test,
PMMH test suite.

---

## Critical Issues

### 1. BUG (Critical): NUTS gradient has double chain rule on prior

pgas.rs lines 932-949:
```rust
// Prior gradient (Normal case):
ll_grad[i] += -(natural - mean) / (sd * sd)
    * transform_deriv(&if2_params[i], z[i]);   // ← already chain-ruled

// Then later:
ll_grad[i] *= transform_deriv(&if2_params[i], z[i]);  // ← SECOND chain rule
```

The prior contribution gets `transform_deriv` applied TWICE. And
`jacobian_grad` (which returns d/dz of log|Jacobian|, already on
z scale) also gets incorrectly multiplied by `transform_deriv`.

The target gradient on the z scale is:
```
d/dz log π(z) = d(ll)/dθ × dθ/dz        (chain rule on LL)
              + d(log_prior)/dθ × dθ/dz  (chain rule on prior)
              + d(log|dθ/dz|)/dz          (Jacobian, already on z)
```

**The correct code:**
```rust
// 1. Chain-rule the LL gradient FIRST (natural → z scale)
ll_grad[i] *= transform_deriv(&if2_params[i], z[i]);

// 2. Add prior gradient ON Z SCALE
match &priors[i] {
    Prior::Flat => {},
    Prior::Normal { mean, sd } => {
        ll_grad[i] += -(natural - mean) / (sd * sd)
            * transform_deriv(&if2_params[i], z[i]);
    }
    Prior::TransformedNormal { mean, sd } => {
        ll_grad[i] += -(z[i] - mean) / (sd * sd);
    }
}

// 3. Add Jacobian gradient (already on z scale, NO chain rule)
ll_grad[i] += jacobian_grad(&if2_params[i], z[i]);
```

**Impact:** NUTS trajectories follow wrong gradients. Leapfrog
will diverge or waste computation. Acceptance rate will be
terrible. This makes NUTS useless until fixed.

**Fix urgency:** Critical — the NUTS mode is broken.

### 2. BUG (Critical): NUTS tree proposal uses deterministic threshold instead of random sampling

nuts.rs lines 229-232:
```rust
let accept = n_dprime as f64 / (n_prime + n_dprime) as f64;
// Use a deterministic choice based on counts (no RNG in recursive tree)
if accept > 0.5 { (z_dprime, log_p_dprime) } else { (z_prime, log_p_prime) }
```

The comment says "no RNG in recursive tree" but the Hoffman &
Gelman (2014) NUTS algorithm requires a random choice at every
level:

```
if uniform() < n'' / (n' + n'')
    θ' ← θ''
```

Without randomness in the tree, the sampler doesn't maintain
detailed balance. The proposal distribution is biased toward
whichever subtree has more valid states, rather than sampling
uniformly from all valid states.

**Fix:** Pass `rng` to `build_tree` and use `rng.uniform()`:
```rust
if rng.uniform() < n_dprime as f64 / (n_prime + n_dprime) as f64 {
    (z_dprime, log_p_dprime)
} else {
    (z_prime, log_p_prime)
}
```

This requires threading the RNG through the recursive calls.
The alternative (accumulate all valid states and sample at the
top) works but is more complex and uses more memory.

**Impact:** Biased posterior samples from NUTS.

### 3. BUG (High): PGAS initial counts for non-reference traced-back particle use deterministic init

pgas.rs lines 696-701:
```rust
let initial_counts = if particle == j_ref {
    reference.initial_counts.clone()
} else {
    let (init_int, _) = model.initial_state(&current_params)?;
    init_int.counts
};
```

After tracing back through ancestry, if the final ancestor isn't
the reference particle, the code uses a FRESH deterministic
initial state instead of the ancestor's stochastic initial state
(which was drawn from Binom(N₀, s0) during CSMC init). The
stochastic initial states from the CSMC particles were not stored
in the history, so they're lost.

**Fix:** Store initial counts per particle alongside the substep
history:
```rust
let initial_counts_history: Vec<Vec<i64>> = counts.iter()
    .map(|c| c.clone())
    .collect();
// ... at the end:
let initial_counts = initial_counts_history[particle].clone();
```

This is needed before the first substep's propagation and should
capture the state AFTER stochastic IVP initialization (line 439-
460) but BEFORE any propagation.

**Impact:** IVP parameters (s0, e0, i0) are incorrectly sampled
when the traced-back trajectory doesn't start from the reference.

### 4. BUG (High): Vec::remove(0) in hot path — O(n) per source group per step

chain_binomial.rs line 446:
```rust
let z = scratch.binomial_z_values.remove(0);
```

`Vec::remove(0)` shifts all remaining elements left. With 4 source
groups, this is called 4 times per step, each shifting O(n_groups)
elements. For a correlated PF with 5000 particles × 780 obs × 7
steps/obs × 4 groups = 109M removes, each O(4) = 436M element
shifts. This is ~10% of total step time for CPM.

**Fix:** Use an index counter instead:
```rust
// Add to StepScratch:
pub binomial_z_idx: usize,

// In step_one setup:
scratch.binomial_z_idx = 0;

// At consumption:
let z = scratch.binomial_z_values[scratch.binomial_z_idx];
scratch.binomial_z_idx += 1;
```

---

## Design Issues

### 5. Duplicate log_jacobian and transform_deriv functions

`log_jacobian` is defined identically in both pmmh.rs (line 253)
and pgas.rs (line 719). `transform_deriv` and `jacobian_grad` are
only in pgas.rs. These should be methods on `Transform` or
`IF2Param`, shared by all inference algorithms.

```rust
// On IF2Param or Transform:
impl IF2Param {
    pub fn log_jacobian(&self, z: f64) -> f64 { ... }
    pub fn transform_deriv(&self, z: f64) -> f64 { ... }
    pub fn jacobian_grad(&self, z: f64) -> f64 { ... }
}
```

### 6. PGAS memory: O(N × T × K) clones per sweep

pgas.rs lines 640-642:
```rust
history_counts.push(counts.iter().map(|c| c.clone()).collect());
history_flows.push(substep_flows.iter().map(|f| f.clone()).collect());
history_gammas.push(substep_gammas.iter().map(|g| g.clone()).collect());
```

Every substep clones all N particles' counts, flows, and gammas.
For He et al. (50 particles × 5600 substeps × 4 compartments),
this is 50 × 5600 × (4+8+1) × 8 bytes ≈ 29 MB per CSMC sweep.
For polio (774 patches × 4 compartments × 200 particles), it's
~50 GB. This won't scale.

**Medium-term fix:** Use a particle ancestry tree (Lindsten &
Schön 2013) that stores only the live particles and traces back
via ancestor indices. Storage drops from O(N×T×K) to O(N×K + T).

**Short-term fix:** At minimum, don't store gammas for every
substep — they're only needed for log_transition_density, which
could recompute them from the deterministic Gamma noise (if the
seed is stored instead).

### 7. Correlated PF sigma_sq evaluated once at t=0

correlated_pf.rs lines 217-238:
```rust
let sigma_sq = model.model.transitions.iter()
    .find_map(|tr| match &tr.draw_method {
        ir::transition::DrawMethod::Overdispersed(_) => {
            // evaluate the expression at t=0
            ...
        }
    })
    .unwrap_or(1.0);
```

The overdispersion parameter is evaluated at t=0 with a zero
initial state. But sigma_sq is typically a model parameter
(sigma_se), not state-dependent. If the expression WERE
state-dependent (e.g., `sigma_se * I / N`), this would silently
use the wrong value for all time steps. The comment says "this is
model-specific" — it should either be evaluated per-step or
asserted to be a Param or Const expression.

### 8. NUTS mass matrix is hardcoded to identity

pgas.rs line 961:
```rust
mass_matrix_inv: vec![1.0; d],
```

The mass matrix should be adapted during warmup, as Stan does.
With identity mass matrix, NUTS is essentially gradient descent
with momentum — it doesn't account for different parameter scales.
For He et al. where R0 ranges over [1, 100] and s0 over
[0.01, 0.10], the identity mass matrix will give terrible
exploration.

**Fix:** After burn-in, compute the empirical variance of each
parameter from the chain and use that as the diagonal mass matrix.
This is the "diagonal Euclidean" metric that Stan uses by default.

### 9. NUTS acceptance probability is binary

pgas.rs line 983:
```rust
let accept_prob = if result.accepted { 0.8 } else { 0.0 };
nuts_step_size = nuts_dual_avg.update(accept_prob);
```

The dual averaging should receive the actual acceptance
probability from the NUTS step (the average over the tree), not
a binary 0/0.8. Stan computes the mean acceptance probability
across all leapfrog steps in the tree and passes that to dual
averaging. Using binary values makes adaptation much noisier.

**Fix:** Return the average acceptance probability from nuts_step
(computed as `n_valid / n_leapfrog`) and pass it to dual averaging.

### 10. Adaptive proposal in PMMH updates with rejected samples

pmmh.rs line 420:
```rust
if let Some(ref mut ap) = adaptive {
    ap.update(&current_transformed);
}
```

The comment says "matching the original Haario algorithm" which
does include all iterations regardless of acceptance. This is
correct for the Haario et al. (2001) AM algorithm. But it means
the proposal covariance is computed from a sequence of mostly-
identical samples (when acceptance rate is low), making the
Cholesky nearly singular. The regularization (epsilon = 1e-6)
handles this, but the quality of the adapted proposal suffers.

Consider: only update on accepted steps, or use a mixture proposal
(Haario adaptation + fixed diagonal).

---

## Test Coverage Gaps

### Missing Tests

**T1. NUTS gradient correctness on the FULL target (prior +
Jacobian + LL).** The gradient_check test only validates the
complete_data_loglik gradient, not the composed target that NUTS
uses (which includes prior and Jacobian gradients). Add a test
that compares the full `log_prob_and_grad` closure against finite
differences. This would have caught bug #1.

**T2. NUTS detailed balance / invariance test.** Run NUTS on a
known 2D Gaussian target for 10K steps. Verify the samples match
the target mean and covariance within statistical tolerance. This
would have caught bug #2.

**T3. PGAS trajectory consistency test.** After a CSMC sweep,
verify that the returned trajectory's initial_counts match the
ancestor's initial_counts (not a fresh deterministic init). Would
have caught bug #3.

**T4. PGAS acceptance rate sanity.** On a simple model where the
posterior is known, verify PGAS acceptance rates are > 10% and
< 90%. Currently no test for PGAS convergence.

**T5. Correlated PF correlation test.** Run the correlated PF
at two nearby parameter values with rho=0.99. Verify the loglik
RATIO has lower variance than the DIFFERENCE of two independent
PFs. This tests that the correlation machinery works.

**T6. Correlated PF matches standard PF in expectation.** At the
same parameters with rho=0 (no correlation), verify the correlated
PF gives the same mean loglik as the standard bootstrap_filter.

**T7. PMMH posterior mean on a known model.** Pure death with
conjugate prior (Gamma-Poisson). The posterior is known
analytically. Verify PMMH posterior mean is within 2 SD of the
analytical mean.

**T8. Gradient check with observations.** The current gradient
test uses empty observations. Add a test with actual observations
and a dmeasure that contributes to the gradient. Currently the
observation gradient is stated as "zero when obs params are fixed"
— but if rho is estimated, the observation gradient is nonzero
and needs testing.

**T9. NUTS with non-identity mass matrix.** Currently the mass
matrix is hardcoded to identity. When adaptive mass matrix is
implemented, test that it improves acceptance rate on an
ill-conditioned target.

---

## Summary

| # | Issue | Severity | Lines to fix |
|---|-------|----------|-------------|
| 1 | NUTS double chain rule | **Critical** | ~15 |
| 2 | NUTS deterministic tree proposal | **Critical** | ~10 + thread RNG |
| 3 | PGAS stochastic init lost in traceback | **High** | ~10 |
| 4 | Vec::remove(0) in hot path | **High (perf)** | ~5 |
| 5 | Duplicate Jacobian functions | **Medium** | ~20 (refactor) |
| 6 | PGAS O(N×T×K) memory | **Medium (scaling)** | ~100 (ancestry tree) |
| 7 | sigma_sq at t=0 only | **Medium** | ~10 |
| 8 | Identity mass matrix | **Medium (UX)** | ~30 |
| 9 | Binary acceptance prob | **Medium** | ~10 |
| 10 | Adaptive proposal on rejections | **Low** | ~5 |

**Fix #1 and #2 before any NUTS usage.** The double chain rule
produces wrong gradients; the deterministic tree selection violates
detailed balance. Together they make NUTS both biased and
inefficient. Fix, add tests T1 and T2, then verify on a known
target before using NUTS on real models.

**Fix #3 before PGAS with IVP parameters.** If s0 is estimated
via PGAS, the current code discards the stochastic initial state
when the traceback doesn't end at the reference particle. This
biases the s0 posterior toward the deterministic init value.

**Fix #4 now** — it's a one-line performance fix that affects
every correlated PF evaluation.

The PGAS and PMMH core algorithms are well-structured. The
transition density computation, CSMC-AS ancestor sampling, and
adaptive proposal machinery are all correct. The issues are in
the NUTS integration (gradient composition, tree sampling) and
some mechanical bugs (memory, hot-path allocation). The test
suite is solid for PMMH but thin for NUTS and PGAS — add the
missing tests before relying on these for inference.
