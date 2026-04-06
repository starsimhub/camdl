---
status: closed
date: 2026-03-28
note: All issues from this review have been addressed and incorporated.
---

# Code Review #10 (Updated) — Full Codebase

**Codebase:** ~23,600 LOC. Changes since last review: fit workflow
with per-stage config, dmeasure compilation from IR, rmeasure/obs_mean
closures, double-buffer resampling, IF2 per-iteration diagnostics
(weighted_var_ratio, q_ratio), cache/provenance wiring, pfilter
--replicates, pfilter trace in validate, scout early-abort on
degenerate filter, scout cooling=0.5 default.

---

## What's correct and well-designed

**dmeasure compilation from IR — major milestone.** dmeasure.rs
compiles the IR's Likelihood Expr fields into closures. No more
hardcoded "rho", "k", "psi" parameter names. The `Projected` Expr
node plumbs the projected observation value into the expression
evaluator. Any observation model expressible in the DSL now works
for inference. The three compiled closures (dmeasure, rmeasure,
obs_mean) cover likelihood, sampling, and mean prediction. Clean.

**Per-stage fit.toml config.** `[scout]`, `[refine]`, `[validate]`
sections with Optional fields. `rw_sd_scale` multiplier.
`pfilter_particles` on validate. All optional with sensible defaults.
The fit.toml is now a complete portable inference specification.

**Cache/provenance wired through.** All three stages call
`check_cache`. `--force` bypasses. This was issue #3 from the
previous review — now fixed.

**Double-buffer resampling.** Both PF and IF2 use `std::mem::swap`.
The 6.2GB clone churn is eliminated.

**Scout defaults improved.** 500 particles, 30 iterations,
cooling=0.5, rw_sd_scale=1.5. Early-abort at iteration 5 with
actionable error message. Much safer than the old 200p/20i/no-cooling.

**IF2 per-iteration diagnostics.** `weighted_var_ratio` (selection
pressure from weights, no resampling noise) and `q_ratio`
(perturbation vs cloud width). These are the right diagnostics for
automated rw_sd tuning.

**StepScratch outside IF2 loop.** Pre-allocated before the iteration
loop. Fixed from previous review.

**fit_state stores all params.** Uses `collect_all_params` for
estimated + fixed. Self-contained across stages.

**Validate writes pfilter_trace.tsv.** Predictions, ESS, ll per
observation. The missing piece for vignette diagnostic plots.

---

## Issues

### 1. BUG (High): rmeasure draws corrupt particle RNG streams

particle_filter.rs:172-174:
```rust
let obs_draws: Vec<f64> = projections.iter().enumerate()
    .map(|(i, &proj)| rmfn(proj, &mut rngs[i]))
    .collect();
```

The rmeasure draws consume random numbers from each particle's RNG
(`rngs[i]`). This happens BEFORE the next observation's propagation,
which uses the same `rngs[i]`. Result: `bootstrap_filter` with
`rmeasure_fn = Some(...)` gives a **different log-likelihood** than
with `rmeasure_fn = None` for the same seed.

Consequences:
- `camdl pfilter --trace` gives a different loglik than `camdl
  pfilter` (no --trace) for the same seed
- Validate's pfilter loglik isn't comparable to a standalone pfilter
- Reproducibility broken: "diagnostic mode changes inference result"

**Fix:** Use a separate RNG stream for rmeasure, derived from but
independent of the particle streams:

```rust
// Allocate alongside the other per-particle RNGs
let mut diag_rngs: Vec<StatefulRng> = (0..n_particles)
    .map(|i| StatefulRng::new(seed ^ (i as u64).wrapping_mul(0xbaadf00d)))
    .collect();

// In the prediction block:
let obs_draws: Vec<f64> = projections.iter().enumerate()
    .map(|(i, &proj)| rmfn(proj, &mut diag_rngs[i]))
    .collect();
```

The invariant: **process RNG streams must be identical whether or
not predictions are computed.** This is how pomp handles it — the
observation model has its own RNG stream.

### 2. BUG (Medium): dmeasure closures allocate per call

dmeasure.rs:27-28, 46-47, 119-120, 139-140:
```rust
let int_s = IntState::new(n_int);
let real_s = RealState::new(n_real);
```

Every dmeasure/rmeasure/obs_mean call allocates IntState and
RealState. The dmeasure is called N×T times (3.9M for measles with
5000 particles). These are dummy contexts — likelihood Exprs use
`Projected` and `Param`, not compartment values.

**Fix:** Pre-allocate at closure creation time and capture by move:

```rust
pub fn compile_dmeasure_pf(...) -> Box<dyn Fn(f64, f64) -> f64> {
    let int_s = IntState::new(n_int);    // allocate ONCE
    let real_s = RealState::new(n_real);  // allocate ONCE
    Box::new(move |projected, observed| {
        eval_likelihood(&likelihood, projected, observed, &params,
                        &compiled, &int_s, &real_s)
    })
}
```

Verify that `eval_likelihood` reads but never mutates int_s/real_s.

### 3. BUG (Medium): global_step i32 overflow at scale

if2.rs:278: `per_step_cooling.powi(global_step as i32)`

With 100 iterations × 780 obs = 78,000, this is fine. For polio
(774 patches × 52 obs × 200 iterations) or any model with >2B
cumulative steps, the `as i32` cast silently overflows, producing
a wildly wrong cooling factor.

**Fix:** `per_step_cooling.powf(global_step as f64)` — one character
change, prevents a catastrophic failure at scale.

### 4. DESIGN: weighted_prediction_diag returns redundant fields

particle_filter.rs:263-266 sets all 8 fields of PredictionDiag to
the same 4 values:

```rust
obs_mean: mean, obs_q05: quantile(0.05), ...
state_mean: mean, state_q05: quantile(0.05), ...  // identical!
```

The caller (lines 179-184) picks from two different calls to this
function, so the result is correct. But the function's contract is
confusing — it appears to distinguish obs from state but doesn't.

**Fix:** Return a simple `(mean, q05, q50, q95)` tuple. Build
PredictionDiag explicitly in the caller from two tuple results.

### 5. DESIGN: Normal likelihood sd/variance ambiguity

dmeasure.rs:76: `discretized_normal_logpmf_tol(observed, mean, sd * sd, DEFAULT_TOL)`

The IR's NormalLikelihood has a field called `sd`, but the logpmf
takes variance. The code squares `sd`. This works for He et al.
where the DSL expresses `sd = sqrt(variance_formula)`. But if
someone declares `sd = variance` by mistake, the squaring produces
garbage.

**Recommendation:** Add a `DiscretizedNormal` variant to the IR
Likelihood enum with explicit `mean` and `variance` Expr fields.
The standard `Normal` keeps `mean` and `sd`. The He et al. model
uses `DiscretizedNormal` with the variance formula directly. No
squaring ambiguity.

### 6. DESIGN: loglik_sd from batch means is approximate

validate.rs:278-292 estimates SD from 10 batch means of one run.
This underestimates the true SD when ll_increments are correlated
(which they are — epidemic waves span many observations).

Not a bug — but the output should say "approximate SD" and the
fit_report should recommend `camdl pfilter --replicates 100` for
a precise estimate.

### 7. MINOR: Obs model resolution fallback in fit runner

runner.rs:334-349: If the `[data]` key doesn't match an observation
block name, the runner falls back to transmission metadata
heuristic. For `camdl fit`, this fallback is harmful — the user
specified a data stream name that doesn't exist in the model, and
the runner silently uses a different observation model.

**Fix:** Error when the key doesn't match, instead of falling back.
The fallback was for backward compatibility with models without
observation blocks, but `camdl fit` should require them.

### 8. MINOR: IF2 diagnostics not written to output files

`weighted_var_ratio` and `q_ratio` are computed per-parameter
per-iteration but `write_diagnostics` only outputs chain/iteration/
loglik. The agent can't read the diagnostics from files for
automated rw_sd adjustment.

**Fix:** Add wvr and q_ratio columns to parameter_traces.tsv, or
create a separate diagnostics_detailed.tsv per chain.

---

## Features not fully wired

### A. Fixed param value overrides in fit.toml

The spec says `[fixed]` should support `N0 = 2462500` to override
model defaults. Config parses `HashMap<String, toml::Value>` which
accepts numbers. But does the runner apply numeric fixed values to
`base_params`? If the runner only checks for `true` values, numeric
overrides are silently ignored.

### B. Profile CIs in status

Previous review noted `load_profiles` returned an empty HashMap
due to a lifetime issue. Verify this is now fixed with String keys.

### C. `camdl fit run` one-command mode

Not yet implemented but the most impactful UX feature. The vignette
workflow is `scout → refine → validate → status` — four commands.
A single `camdl fit run fit.toml` that chains all stages would be
the cleanest user experience.

---

## Workflow design assessment (Stan comparison)

The workflow is approaching Stan-quality UX in several ways:

**What's Stan-like (good):**
- Exhaustive partition (like Stan requiring all parameters declared)
- Convergence diagnostics per chain (Rhat, like Stan's R-hat)
- Profile likelihoods for identifiability (Stan doesn't do this
  automatically — camdl is ahead here)
- Provenance hashing (Stan doesn't have this at all)
- `fit status` colored summary (like Stan's `print(fit)` but more
  informative)

**What Stan does better that camdl should match:**
- Stan's `pairs()` plot equivalent: pairwise parameter scatterplots
  from the IF2 chain traces. Critical for spotting correlations and
  ridges. The data is in parameter_traces.tsv — just needs a Python
  plotting script or a `camdl fit plot` command.
- Stan warns about specific pathologies: divergent transitions,
  max treedepth, E-BFMI. camdl's equivalent warnings exist (low
  ESS, boundary pile-up, degenerate filter) but should be more
  prominent in the status output.
- Stan's summary includes n_eff (effective sample size of the
  posterior). For IF2, the equivalent is the number of "effective
  chains" — Rhat-based, which camdl already computes.

**What camdl does that Stan can't:**
- Plug-and-play inference (no transition density needed)
- Profile likelihoods from the fit workflow
- Scout → refine → validate with auto-tuned rw_sd
- Provenance hashing and cache validation
- Multi-chain IF2 with rayon (Stan's chains are separate processes)

---

## Priority

| # | Issue | Impact | Fix |
|---|-------|--------|-----|
| 1 | rmeasure corrupts RNG | **Correctness** | ~10 lines |
| 3 | global_step overflow | **Correctness at scale** | 1 line |
| 2 | dmeasure allocates per call | **Performance** | ~10 lines |
| 8 | Diagnostics not in files | **Agent workflow** | ~20 lines |
| 7 | Obs model fallback | **Silent wrong results** | ~5 lines |
| 4 | Redundant PredictionDiag | **Clarity** | ~15 lines |
| 5 | sd/variance ambiguity | **Robustness** | ~20 lines |
| 6 | loglik_sd labeling | **Honesty** | ~2 lines |

Fix #1 first — it affects loglik reproducibility and comparisons
between pfilter runs with and without diagnostics.
