---
status: in-progress
date: 2026-04-11
last_updated: 2026-04-11
items_total: 13
items_done: 10
items_deferred: 3
note: "#10 (runner.rs split), #11 (vestigial closures), #13 (CPM correlation test) deferred."
---

## Inference Review

### Algorithmic Correctness Issues

#### 1. `pgas_grad.rs`: Gamma density gradient is included but disabled in `pgas.rs`

`pgas_grad.rs` implements `log_gamma_density_grad_substep` and calls it in
`complete_data_loglik_grad`. But `pgas.rs`'s `complete_data_loglik` has the
gamma density **commented out** with a TODO referencing the spatial PGAS
incident. This means NUTS gets a gradient that includes the gamma density term,
but the log-posterior it's differentiating **doesn't include that term**. The
gradient and the objective function disagree.

This is a subtle correctness bug: NUTS uses `log_prob_and_grad` where the value
comes (indirectly) from `complete_data_loglik` (no gamma term) but the gradient
comes from `complete_data_loglik_grad` (with gamma term). The sampler will be
biased — it's following a gradient surface that doesn't match the density it's
sampling from.

**Recommendation:** Either re-enable the gamma density in `complete_data_loglik`
(fixing the gamma_idx alignment issue noted in the incident), or disable it in
`complete_data_loglik_grad` too. They must agree.

#### 2. `correlated_pf.rs`: `sigma_sq` is evaluated at zero state, not current state

Lines ~250-260 compute `sigma_sq` for the gamma shape/scale by evaluating the
overdispersion expression with a **zero IntState** (`IntState::new(n_int)`). If
`σ²` depends on compartment counts (which it can — it's a general `Expr`), this
gives the wrong value for every substep. The precomputed shape/scale would be
incorrect.

**Recommendation:** Either assert that overdispersion expressions are
state-independent (which the IR validator should enforce), or evaluate σ²
per-substep per-particle using the particle's actual state. The latter is what
`step_one` does.

#### 3. `correlated_pf.rs`: `steps_per_obs` assumes uniform spacing

`obs_dt` is computed from `observations[1].time - observations[0].time`, and
this single value is used to size the entire `PFRandomState`. If observation
spacing is non-uniform (e.g., weekly reports with missing weeks), the noise
arrays are misaligned — some substeps get noise from the wrong observation
interval. This would manifest as broken correlation structure, defeating the
purpose of CPM.

**Recommendation:** Either validate that observations are uniformly spaced
(error if not), or size the noise arrays from the actual maximum number of
substeps across all intervals.

#### 4. `nuts.rs`: Cholesky fallback silently degrades

In `cholesky_lower`, when a diagonal element is non-positive (matrix not
positive definite), it silently substitutes `1e-10`. This means a corrupted or
degenerate mass matrix produces a Cholesky factor that looks valid but
represents a completely different matrix. NUTS will run, but its geometry will
be wrong — potentially causing the sampler to explore poorly without any
diagnostic signal.

**Recommendation:** Return a `Result` or at least `log::warn!` when the fallback
fires. The PMMH version (`cholesky_lower` in `pmmh.rs`) correctly returns
`false` on failure. This is also a DRY issue — there are **two independent
Cholesky implementations** (`nuts.rs` and `pmmh.rs`) with different error
handling.

#### 5. `validate.rs`: Profile likelihood uses IF2 perturbed loglik, not true loglik

`run_profiles` evaluates each profile point via a short IF2 run and takes
`r.final_loglik` — which is the IF2 **perturbed** model loglik, not a true
PF-evaluated loglik. Profile confidence intervals computed from perturbed
logliks are biased (perturbation smoothing inflates them). The main chains in
`run_validate` correctly evaluate true loglik via `run_quick_pfilter`, but
profiles skip this step.

**Recommendation:** Run a quick PF at each profile point's MLE, same as done for
the main chains. This costs ~21 PF evaluations per parameter (at 200-500
particles each), which is fast compared to the 21 IF2 runs already being done.

### Design Issues

#### 6. `phi()` is duplicated between `obs_loglik.rs` and `correlated_pf.rs`

The standard normal CDF implementation appears identically in both files. The
`correlated_pf.rs` version even has a comment "same as obs_loglik::normal_cdf".

**Recommendation:** Delete `correlated_pf::phi`, import
`obs_loglik::normal_cdf`.

#### 7. Two Cholesky implementations with different APIs

`nuts.rs::cholesky_lower` returns `Vec<f64>` and silently fixes non-PD matrices.
`pmmh.rs::cholesky_lower` takes `&mut [f64]` in-place and returns `bool`. These
implement the same algorithm with different error handling and calling
conventions.

**Recommendation:** Extract a single
`fn cholesky_lower(a: &[f64], d: usize) -> Option<Vec<f64>>` in a shared module
(e.g., `inference/linalg.rs`). Both callers get correct error handling.

#### 8. `runner.rs` median/MAD computation is duplicated

`auto_rw_sd` computes median and MAD of a `Vec<f64>` with 20 lines of
sort-and-index code. This exact pattern (sort, pick middle, compute absolute
deviations, sort again, pick middle) appears twice — once for full MAD and once
for good-chain MAD. It's a natural utility function.

**Recommendation:** `fn median(v: &mut [f64]) -> f64` and
`fn mad(v: &[f64], median: f64) -> f64`.

#### 9. `obs_model.rs`: Binomial loglik manually inlined instead of calling `binom_logpmf`

The `Binomial` branch in `eval_likelihood_resolved` manually computes
`lgamma(n+1) - lgamma(k+1) - lgamma(n-k+1) + k*ln(p) + (n-k)*ln(1-p)`. This is
literally `binom_logpmf(k, n, p)` from `obs_loglik.rs`, but without the boundary
checks (p=0, p=1, k>n).

**Recommendation:** Call `binom_logpmf`. Gets the boundary checks for free.

#### 10. `runner.rs` is 1287 lines — multiple responsibilities

`runner.rs` handles: model loading, parameter construction, IF2 execution,
progress bars, preflight reporting, Rhat computation, MAD-based rw_sd
calibration, chain output writing, prior parsing, config hashing, and parameter
formatting. At least 4 of these (Rhat, MAD calibration, output writing, prior
parsing) are pure functions that could live in separate modules.

**Recommendation:** Extract at minimum `diagnostics.rs` (Rhat, ESS) and
`output.rs` (write_chain_outputs, write_diagnostics, format_param_value).

#### 11. `run_quick_pfilter` builds obs_stream_specs AND single-stream closures

Lines 189-215 build multi-stream `obs_stream_specs` AND single-stream
`project_fn`/`obs_loglik_fn`. The latter are only used because
`bootstrap_filter`'s signature requires them even when `joint_obs_fn` is `Some`.
This is vestigial — if `joint_obs_fn` overrides everything, the single-stream
closures are never called but still built.

**Recommendation:** Make `bootstrap_filter` take `project_fn`/`obs_loglik_fn` as
`Option` when `joint_obs_fn` is `Some`. Or accept the wasted closure
construction as harmless.

### Testing Gaps

#### 12. No gradient-vs-finite-difference test for `complete_data_loglik_grad`

`obs_loglik.rs` has excellent gradient-vs-FD tests for each distribution. But
`pgas_grad.rs` — which computes the gradient of the _entire_ complete-data
log-likelihood through the Euler-multinomial chain rule — has no such test. A
test that constructs a simple SIR trajectory, evaluates `complete_data_loglik`
and `complete_data_loglik_grad` at a point, then verifies each component against
finite differences, would catch bugs like #1 above.

#### 13. No test for `correlated_pf.rs` correlation properties

The CPM implementation has no test verifying that correlated PF evaluations
actually produce correlated likelihoods. A test that runs two PF evaluations at
nearby parameters with high ρ and checks that
`Var(LL₁ - LL₂) < Var(LL₁) + Var(LL₂)` would validate the correlation mechanism.

### Summary by Priority

| Priority     | Item                                         | Type            | Risk                                       |
| ------------ | -------------------------------------------- | --------------- | ------------------------------------------ |
| **Critical** | #1: Gamma density grad/value mismatch        | Correctness bug | NUTS bias for overdispersed models         |
| **High**     | #2: σ² evaluated at zero state in CPM        | Correctness bug | Wrong gamma shape if σ² is state-dependent |
| **High**     | #5: Profile loglik uses perturbed IF2        | Correctness bug | Inflated profile CIs                       |
| **Med**      | #3: Non-uniform obs spacing in CPM           | Correctness     | Broken correlation, silent                 |
| **Med**      | #4: Silent Cholesky degeneracy               | Diagnostic gap  | No warning when mass matrix is broken      |
| **Med**      | #12: No grad-vs-FD test for full LL gradient | Testing gap     | Would have caught #1                       |
| **Low**      | #6, #7, #8, #9: DRY violations               | Code quality    | ~80 LOC savings                            |
| **Low**      | #10, #11: Structural                         | Maintainability | runner.rs too large                        |

The algorithmic implementations are solid — the NUTS, CSMC-AS, and CPM-MCMC
implementations are faithful to the reference papers. The transition density /
gradient derivations are correct in isolation. The bugs are at the _integration
boundaries_: where pgas.rs calls pgas_grad.rs (#1), where correlated_pf.rs
precomputes values it should compute per-step (#2), and where validate.rs takes
a shortcut on profile evaluation (#5). These are the classic "it works for the
test model but breaks for the general case" issues.
