---
status: new
date: 2026-04-13
reviewer: external
items_total: 8
items_done: 0
items_deferred: 0
note: "Round 2 inference CLI review. Covers trait migration gaps, DRY issues, runner.rs structure."
---

## Inference CLI Review (Round 2)

### What's Fixed Since Round 1

| R1 Item | Status |
|---------|--------|
| Profile loglik uses IF2 perturbed loglik | **Fixed** — now uses `run_quick_pfilter` for true PF loglik |
| Closure plumbing duplicated 6× | Partially fixed — IF2/PF use traits, but plumbing moved from closures to StreamSpec construction (still 6 copies) |
| runner.rs mechanical split | Partially done — `provenance.rs`, `state.rs`, `trace_writer.rs`, `status.rs` extracted; runner still 1292 lines |

### New Infrastructure (All Well-Designed)

- **`state.rs`** (50 lines) — `FitState` struct for inter-stage handoff via
  `fit_state.toml`. Clean serde, includes `input_hash`, `camdl_version`,
  `loglik_type` (marginal vs complete_data vs IF2). Load/save are
  straightforward.

- **`provenance.rs`** (220 lines) — Input hashing (model + data + config +
  seed + version) and content hashing (tamper detection for
  `mle_params.toml`). Deterministic via sorted keys. Short hashes (8 hex
  chars) for human readability.

- **`trace_writer.rs`** (99 lines) — Shared streaming TSV writer for MCMC
  traces (PGAS, PMMH). Handles append mode for `--resume`, periodic
  flushing, thread-safe via Mutex.

- **`status.rs`** (323 lines) — Colored fit progress summary with per-stage
  completion status, Rhat convergence, staleness warnings. Good UX addition.

- **True loglik evaluation** — `run_chains_with_per_chain_params` now
  evaluates true (unperturbed) PF loglik every 10 iterations for all chains,
  then overwrites `final_loglik` with the true value. This means the chain
  selection (best chain by loglik) uses the correct metric, not the IF2
  inflated metric. This was a subtle but important bug from R1.

### Bugs

**1. `eval_correlated` in PMMH CLI still uses the pre-trait API.** Lines
326-342 of `pmmh.rs` construct `step_fn`, `project_fn`, and `obs_ll`
closures by hand and pass them to `bootstrap_filter_correlated`. This is the
ONLY remaining call site that uses the old closure-based API. It's also
single-stream-only (`config.flow_indices`, `config.obs_model_ir` — the
backward-compat convenience fields). If someone uses correlated PMMH with
multi-stream observations, this will use only the first stream.

**2. `bootstrap_filter_correlated` was not migrated to traits.** The
correlated PF still takes the old 12-argument signature. This blocks
migrating the PMMH CLI. Should take `&P: ProcessModel` +
`&dyn ObservationModel` like the regular `bootstrap_filter`.

**3. `FitRunConfig` still carries backward-compat convenience fields.**
`flow_indices` and `obs_model_ir` are documented as "backward compat
convenience" for the first stream — they exist solely so the PMMH
correlated path can use them. Once `bootstrap_filter_correlated` is
trait-ified, these can be deleted.

### DRY Issue

**4. The `ObsStream → StreamSpec` mapping is repeated 6 times.** Every call
site constructs:

```rust
let process = ChainBinomialProcess::new(config.compiled.clone());
let obs_model = MultiStreamObsModel::new(
    config.streams.iter().map(|s| StreamSpec {
        flow_indices: s.flow_indices.clone(),
        ir_model: s.obs_model_ir.clone(),
        observations: s.data.iter().map(|o| o.value).collect(),
        obs_times: config.observations.iter().map(|o| o.time).collect(),
    }).collect(),
    config.compiled.clone(),
);
```

This should be a method on `FitRunConfig`:

```rust
impl FitRunConfig {
    pub fn build_process(&self) -> ChainBinomialProcess {
        ChainBinomialProcess::new(self.compiled.clone())
    }
    pub fn build_obs_model(&self) -> MultiStreamObsModel {
        MultiStreamObsModel::new(
            self.streams.iter().map(|s| StreamSpec { ... }).collect(),
            self.compiled.clone(),
        )
    }
}
```

Then every call site becomes `let process = config.build_process();
let obs_model = config.build_obs_model();` — two lines instead of ten.

### Code Quality

**5. `runner.rs` is still 1292 lines.** The `provenance.rs` and `state.rs`
extractions helped, but the core file hasn't been split. Recommended
decomposition:

| Target file | Functions | ~Lines |
|------------|-----------|--------|
| `runner.rs` | `FitRunConfig::build`, `run_one_chain`, `run_chains_*`, `print_preflight` | ~500 |
| `params.rs` | `build_if2_params`, `build_if2_params_from_specs`, `derive_transform`, `auto_rw_sd*`, `collect_all_params`, `parse_prior`, `eval_prior_arg` | ~350 |
| `output.rs` | `write_chain_outputs`, `write_diagnostics`, `format_param_value` | ~100 |
| `convergence.rs` | `compute_rhat`, `compute_rhat_ess`, `auto_rw_sd`, `median`, `mad` | ~200 |

This is mechanical and low-risk.

**6. Diagnostic integration is clean but inconsistent.** Some code paths push
to the collector when one is available, but fall back to raw `eprintln!` when
collector is `None`:

```rust
if let Some(c) = collector {
    c.push(DiagnosticKind::MultimodalLikelihood { ... });
} else {
    eprintln!("...");
}
```

This dual path means the same diagnostic has two rendering codepaths — one
typed (via the collector), one ad-hoc (via `eprintln`). When the collector is
always present (which it should be), the `else` branches become dead code.
Recommend making the collector non-optional in the public API and always
using it.

**7. The true-loglik evaluation interval is hardcoded to 10.** Every 10
iterations, ALL chains get a PF evaluation. With 8 chains × 500 particles,
this is a meaningful compute cost. For scout (30 iterations), this means 3
evaluations per chain — fine. For validate (100 iterations), that's 10
evaluations per chain × 4 chains × 5000 particles = substantial. The
interval should scale with the number of iterations, or be configurable. At
minimum, the number of eval particles is already capped at 500 which is
good.

**8. `auto_rw_sd_from_value_pub` vs `auto_rw_sd_from_value`.** There are two
functions that do the same thing — the `_pub` version is public, the other is
private. They have identical implementations. Delete one.

### Summary

| Priority | Item | Type |
|----------|------|------|
| **Bug** | #1: PMMH correlated uses old closure API | Stale code |
| **Bug** | #2: `bootstrap_filter_correlated` not trait-ified | Migration gap |
| **DRY** | #4: StreamSpec construction 6× | Easy fix — helper method |
| **Cleanup** | #5: runner.rs 1292 lines | Mechanical split |
| **Cleanup** | #6: Collector should be non-optional | Remove dual codepath |
| **Cleanup** | #3: Backward-compat convenience fields | Delete after #2 |
| **Minor** | #7: Hardcoded eval interval | Configurable |
| **Minor** | #8: Duplicate `auto_rw_sd` functions | Delete one |

The inference CLI is in good shape. The trait migration landed cleanly for
IF2 and PF. The profile loglik fix was the most important correctness issue
and it's resolved. The main remaining work is: (a) trait-ify
`bootstrap_filter_correlated` so PMMH can drop the old API, (b) add
`build_process`/`build_obs_model` helpers on `FitRunConfig` to eliminate the
6× StreamSpec construction, and (c) the mechanical `runner.rs` split. All
three are low-risk and could be done in an afternoon.
