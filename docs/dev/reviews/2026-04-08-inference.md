---
status: closed
date: 2026-04-08
last_updated: 2026-04-08
items_total: 12
items_done: 12
items_deferred: 0
note: All review items addressed. TraceWriter, PMMH resume, shared diagnostics, EstimatedParam rename.
---

# Inference Subsystem Code Review

## Executive Summary

The core inference engines (IF2, PGAS, PMMH) are individually solid — the math is right, the particle filter plumbing works, the PGAS CSMC-AS implementation is careful about density/state consistency. But the CLI layer has grown organically with significant duplication across the three methods, and several abstractions that should be shared are instead PGAS-only or method-specific. Below I prioritize by impact: bugs first, then interface unification, then cleanup.

---

## 1. Bugs and Correctness Issues

### 1.1 PMMH trace file written twice

`pmmh.rs` (CLI) writes the trace **twice**: once via the streaming `trace_file` Mutex inside `progress_cb` (lines ~230-245), and again via `write_chain_traces()` after all chains finish (line ~300). The streaming write happens for *every* step regardless of burn-in/thin. The post-hoc `write_chain_traces()` respects burn-in/thin. Result: the `trace.tsv` gets overwritten by the post-hoc writer, so you lose the full streaming trace. But if you're interrupted mid-run, the streaming file has *all* steps including pre-burn-in — which is inconsistent with the PGAS trace that streams only the rows it wants.

**Fix**: Remove `write_chain_traces()` and make the streaming writer respect burn-in/thin, matching PGAS behavior.

### 1.2 PMMH `log_posterior` column is just `log_likelihood`

In PMMH's streaming trace callback (CLI `pmmh.rs` ~line 238):
```rust
write!(f, "{}\\t{:.2}\\t{:.2}\\t{}", step, loglik, loglik, ...)
//                          ^^^^^^ log_posterior = loglik (wrong!)
```
The `log_posterior` column should be `loglik + log_prior`, but the callback only receives `loglik` and `accepted` — not the prior. The post-hoc `write_chain_traces` does compute `log_posterior = step.log_likelihood + step.log_prior` correctly, but this overwrites the streaming file (see 1.1). So if you rely on the streaming output during a long run, `log_posterior` is wrong.

### 1.3 PMMH acceptance rate computed per-chain, not per-parameter

PMMH proposes all parameters jointly (block MH), so `acceptance_rate` is a single scalar per chain. But PGAS does component-wise MH-within-Gibbs, giving per-parameter acceptance rates. The diagnostic reporting and `FitState` output don't distinguish — both write `acceptance_rate` but the semantics differ. This is a documentation/interface issue more than a bug, but it matters for `--resume` and downstream analysis.

### 1.4 `run_quick_pfilter` uses only the first observation stream

`runner.rs::run_quick_pfilter` constructs `project_fn` and `obs_loglik_fn` from `config.flow_indices` and `config.obs_model_ir` — the **first stream** backward-compat convenience fields. Multi-stream models get incorrect loglik estimates from scout/refine/validate's "true loglik" evaluation and PMMH's entire PF-based likelihood. Only PGAS (which uses `obs_stream_specs` directly) handles multi-stream correctly.

**This is a real bug for multi-stream models.** The fix: `run_quick_pfilter` should use the `ObsStreamSpec`-based `joint_obs_weight` path.

### 1.5 PGAS resume: `acceptance_rates` denominator is wrong after resume

In `pgas.rs` (engine), the final acceptance rates are:
```rust
let acceptance_rates: Vec<f64> = total_accepted.iter()
    .map(|&n| n as f64 / config.n_sweeps as f64)
    .collect();
```
On resume, `total_accepted` includes counts from the original run (restored from `ChainResumeState`), but `config.n_sweeps` is the *new* total. If you resume a 5000-sweep run to 10000, the denominator is 10000 but `total_accepted` has counts from all 10000 sweeps (correctly accumulated), so this is actually fine. **However**, the resume code restores `total_accepted` but doesn't account for the fact that the adaptation rate `gamma_rm = ADAPT_C / (1.0 + sweep as f64).sqrt()` uses the *resumed* sweep number — meaning adaptation continues correctly from where it left off. This is correct as-is.

Wait — actually re-reading: `total_accepted` is restored from the resume state, which saves the cumulative count at the time of save. Then sweeps continue from `start_sweep` to `n_sweeps`, incrementing `total_accepted`. The denominator uses `config.n_sweeps` (the new target), not the total number of sweeps actually run. If you originally ran 5000 sweeps with 2000 accepted, then resume to 10000 total with another 2000 accepted, you get `total_accepted = 4000`, `n_sweeps = 10000`, rate = 40%. This is correct.

**No bug here.** Retracted.

---

## 2. Interface Unification — The Main Issue

### 2.1 Three separate trace formats, three separate writers

| Method | Trace columns | Writer location | Streaming? | Resume? |
|--------|--------------|-----------------|------------|---------|
| IF2 | `iteration, loglik, if2_perturbed_loglik, params...` | `runner.rs::write_chain_outputs` | No (post-hoc) | N/A |
| PMMH | `step, log_likelihood, log_posterior, accepted, params...` | `pmmh.rs` inline + `write_chain_traces` | Yes (buggy) | No |
| PGAS | `sweep, log_likelihood, log_posterior, trajectory_renewal, params...` | `pgas.rs` inline | Yes | Yes |

**Proposed unified trace format:**
```
sweep  log_likelihood  log_posterior  accepted  params...
```

Where `accepted` is `1/0` for MCMC methods and omitted (or `NA`) for IF2. The `trajectory_renewal` column is PGAS-specific diagnostic and can go in a separate diagnostics file.

**Proposed abstraction:**

```rust
/// Unified MCMC/inference trace writer.
pub struct TraceWriter {
    writer: BufWriter<File>,
    param_names: Vec<String>,
    /// Whether to include log_posterior column (MCMC only, not IF2)
    include_posterior: bool,
}

impl TraceWriter {
    pub fn new(path: &Path, param_names: &[String], include_posterior: bool, append: bool) -> Self;
    pub fn write_row(&mut self, sweep: usize, ll: f64, log_posterior: Option<f64>, params: &[f64], param_indices: &[usize]);
    pub fn flush(&mut self);
}
```

### 2.2 Resume state is PGAS-only

`ChainResumeState` is deeply PGAS-specific (stores trajectory, mass matrix, NUTS step size). But the *concept* of resume applies to all methods:

- **IF2**: Not useful (runs are fast, restarts are fine)
- **PMMH**: Would be very useful (runs are slow). Needs: current params, current_transformed, current_ll, current_log_prior, adaptive proposal state, current_randoms (for CPM), step count.
- **PGAS**: Already implemented.

**Proposed design:**

```rust
/// Method-agnostic resume metadata.
pub struct ResumeHeader {
    pub config_hash: String,
    pub completed_steps: usize,
    pub param_names: Vec<String>,
    pub params: Vec<f64>,
    pub transformed: Vec<f64>,
    pub current_ll: f64,
}

/// Method-specific resume payload.
pub enum ResumePayload {
    PGAS { trajectory: PGASTrajectory, mass_matrix: MassMatrix, ... },
    PMMH { adaptive_proposal: AdaptiveState, current_randoms: Option<PFRandomState>, ... },
}

pub struct ChainResumeState {
    pub header: ResumeHeader,
    pub payload: ResumePayload,
}
```

The `config_hash` validation logic in `pgas.rs::compute_config_hash` should move to a shared location — the hash computation is identical for PMMH (model + data + priors + bounds + particles + dt).

### 2.3 Diagnostics (Rhat, ESS) computed three times

`compute_diagnostics` is implemented separately in:
- `runner.rs::compute_rhat` (IF2 — Rhat only, uses `IF2Result.iterations[].param_means[spec.index]`)
- `pmmh.rs::compute_diagnostics` (PMMH — Rhat + ESS, uses `PMMHResult.steps[].params[spec.index]`)
- `pgas.rs::compute_diagnostics` (PGAS — Rhat + ESS, uses `PGASSweep.params[spec.index]`)

The PMMH and PGAS versions are *identical* modulo input types. They should share a function:

```rust
/// Compute Rhat and ESS from per-chain parameter traces.
pub fn mcmc_diagnostics(
    chains: &[Vec<Vec<f64>>],  // chains[chain_id][sample_idx][param_idx]
    param_names: &[String],
) -> HashMap<String, (f64, f64)>  // name → (rhat, ess)
```

### 2.4 Prior parsing duplicated

`parse_prior` and `eval_prior_arg` live in `pgas.rs` but are called from both `pgas.rs` and `pmmh.rs` (via `super::pgas::parse_prior`). This is a cross-module dependency that should be in a shared location (`runner.rs` or a new `priors.rs`).

### 2.5 `FitState` doesn't capture method-specific info

`FitState` has `rw_sd: HashMap<String, f64>` which is IF2-specific. PGAS/PMMH don't use it. Meanwhile, PGAS-relevant info like `acceptance_rates` per parameter, `trajectory_renewal`, and `ess` aren't stored. The struct should either be method-polymorphic or have optional method-specific fields.

### 2.6 `log_jacobian` duplicated in PGAS and PMMH

`pgas.rs` and `pmmh.rs` both define `fn log_jacobian(param: &IF2Param, z: f64) -> f64` with identical implementations. This should be a method on `IF2Param` or in the `if2` module.

---

## 3. Flow Logic Issues

### 3.1 PGAS `on_sweep` callback fires for ALL sweeps, but `sweeps` vec only collects post-burn-in/thinned

The `progress_cb` in `pgas.rs` (CLI) writes a trace row for *every* sweep, but the engine's `PGASResult.sweeps` only includes post-burn-in thinned samples. This is actually correct behavior (you want the full trace file for diagnostics, and the returned `sweeps` for posterior analysis). But it means the trace file and `sweeps` vec have different lengths, which could confuse downstream consumers.

### 3.2 PMMH `FitState.best_loglik` uses MAP loglik, not marginal

PMMH writes `best_loglik: map_result.map_loglik` to `fit_state.toml`, which is the PF estimate at the MAP params. PGAS writes `best_loglik: best_sweep.log_complete_data_ll`, which is the complete-data LL. These are fundamentally different quantities (marginal vs. complete-data) stored under the same field name. Downstream stages that read `fit_state.toml` might misinterpret.

### 3.3 `FitRunConfig` is IF2-centric

The struct carries `if2_config: IF2Config` and `if2_params: Vec<IF2Param>` — names that don't make sense for PGAS/PMMH. The `IF2Config` fields like `cooling_fraction`, `cooling_target_iters`, `simplex_groups` are irrelevant for MCMC methods. Renaming would help: `estimated_params: Vec<ParamSpec>` (or even just `params`) and splitting config into `SimConfig` (dt, n_particles) + method-specific config.

---

## 4. Robustness Improvements

### 4.1 `steps_per_obs` hardcoded to `(1/dt * 7.0).round()` in PMMH CPM

In `pmmh.rs` (both CLI and engine), the CPM random state is sized with:
```rust
let steps_per_obs = (config.dt.recip() * 7.0).round() as usize;
```
This assumes weekly observations with dt=1 day. For non-weekly data (daily, biweekly, monthly), this silently under- or over-allocates the random noise array. It should be computed from actual observation spacing:
```rust
let steps_per_obs = ((observations[1].time - observations[0].time) / dt).round() as usize;
```

### 4.2 No validation that `--resume` sweeps > original sweeps

PGAS resume silently does nothing if `start_sweep >= config.n_sweeps`:
```rust
if start_sweep >= config.n_sweeps {
    eprintln!("warning: chain already completed...");
}
```
But it still writes the resume state and fit_state.toml with the original results. Should probably `return Ok(())` early or require `n_sweeps > completed_sweeps`.

### 4.3 `process::exit(1)` in library-ish code

Several places in the CLI code call `std::process::exit(1)` inside closures or after errors that should propagate as `Result`. For example, `run_one_chain` in `runner.rs` calls `process::exit(1)` on chain error rather than returning `Err`. This makes testing harder and prevents cleanup.

---

## 5. Recommended Refactoring Order

1. **Fix `run_quick_pfilter` multi-stream bug** (1.4) — correctness, affects PMMH + IF2 validation
2. **Fix PMMH double-write / wrong `log_posterior`** (1.1, 1.2) — correctness
3. **Extract `log_jacobian` to shared location** (2.6) — easy win, 5 min
4. **Extract `parse_prior` to shared location** (2.4) — easy win, 5 min
5. **Extract shared `mcmc_diagnostics`** (2.3) — moderate, eliminates 80 lines of duplication
6. **Compute `steps_per_obs` from data** (4.1) — correctness for non-weekly models
7. **Design unified `TraceWriter`** (2.1) — foundation for PMMH resume
8. **Implement PMMH `--resume`** (2.2) — uses TraceWriter + shared config hash
9. **Rename `if2_params` → `estimated_params`** throughout (3.3) — large but mechanical

---

## 6. Summary of Shared Abstractions Needed

| Abstraction | Currently | Should be |
|-------------|-----------|-----------|
| `log_jacobian` | Duplicated in `pgas.rs` + `pmmh.rs` | Method on `IF2Param` or in `if2.rs` |
| `parse_prior` | In `pgas.rs`, called from `pmmh.rs` | `runner.rs` or `priors.rs` |
| Rhat/ESS | 3 implementations | One shared function |
| Trace writer | 3 ad-hoc writers | `TraceWriter` struct |
| Resume state | PGAS-only | Header + method-specific payload |
| Config hash | PGAS-only | Shared (identical logic for PMMH) |
| PF loglik (multi-stream) | `run_quick_pfilter` (single-stream) | Use `ObsStreamSpec` path |
| `FitState.best_loglik` | Means different things | Add `loglik_type: "marginal"|"complete_data"` |
