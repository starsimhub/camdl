---
status: closed
date: 2026-04-03
note: All issues from this review have been addressed and incorporated.
---

# Inference Stack Code Review — 2026-04-03

Audit of all commits from `d4ba098` (session start) through `bc951d5` (current HEAD).
Covers: particle filter, IF2, profile likelihoods, fit workflow, provenance system,
OOS validation/holdout, chain-binomial backend, observation model compilation,
scout, data tooling.

All findings verified against current code. One original finding (#7) was wrong —
the guard exists. Everything else confirmed.

---

## Critical

None after re-verification.

---

## High

### H1 — Holdout loglik never persisted to any output file

**File:** `rust/crates/cli/src/fit/validate.rs` ~132–149
`train_ll` and `holdout_ll` are computed correctly when `[holdout]` is present,
logged via `eprintln!`, and returned as `Some(f64)` in a tuple — then never used
again. They are not written to `fit_record.json`, `validate_summary.json`,
`pfilter_loglik.txt`, or any other artifact. The entire OOS validation result
exists only in stderr and disappears when the process exits.

**Impact:** Downstream automation (model comparison, cross-validation loops) cannot
read the OOS loglik from any output file.

### H2 — `BetaBinomial` dmeasure returns `−∞` unconditionally, no error emitted

**File:** `rust/crates/sim/src/inference/dmeasure.rs` ~89–92
```rust
Likelihood::BetaBinomial(_) => {
    // Not yet implemented
    f64::NEG_INFINITY
}
```
Any model with a `beta_binomial` observation block silently collapses the
particle filter to all-zero weights from the first observation. `loglik = −∞`,
no error message, no indication of why. Should be an explicit error/exit.

---

## Medium

### M1 — `pfilter_trace.tsv` `observed` column is NaN for all holdout rows

**File:** `rust/crates/cli/src/fit/validate.rs` ~385
```rust
let obs_val = config.observations.get(i).map_or(f64::NAN, |o| o.value);
```
`config.observations` contains only training data. For indices `i >= n_train`,
`get(i)` returns `None` and `obs_val = NaN`. The trace file shows NaN in the
`observed` column for all holdout timepoints, making it misleading for
diagnostics without any warning.

### M2 — `fit_report.txt` content_hash is always the hash of an empty map

**File:** `rust/crates/cli/src/fit/validate.rs` ~648–649
```rust
provenance::compute_content_hash(&HashMap::new())
```
`write_fit_report` hashes an empty map instead of the MLE parameters. The
content_hash in `fit_report.txt` is always the same sentinel (hash of `{}`),
useless for reproducibility tracking. Compare: `write_fit_record` at line 595
correctly passes `all_params`.

### M3 — Profile curvature computed at grid midpoint, not MLE

**File:** `rust/crates/cli/src/fit/validate.rs` ~517–526
```rust
let mid_idx = n_grid / 2;   // always the center of the grid
```
The curvature (and thus the quadratic CI approximation) is computed at the
geometric center of the profile grid, not at the actual MLE location within
that grid. For parameters whose MLE is not near the center of their bounds,
the curvature estimate is wrong and the CIs are unreliable.

### M4 — `cooling_target_iters = n_iterations` deviates from pomp's cf50 semantics

**File:** `rust/crates/cli/src/if2.rs` ~362
`IF2Config::cooling_target_iters` is set to `n_iterations` at the call site.
The `IF2Config` documentation states "Matches pomp's cooling.fraction.50
semantics when `cooling_target_iters = 50`." For any `n_iterations != 50`
the cooling schedule silently diverges from pomp. Scout regime uses 30
iterations, so cooling is ~40% more aggressive than the pomp-equivalent.

### M5 — Degenerate scout not blocked from seeding refine/validate

**File:** `rust/crates/cli/src/fit/scout.rs`, `fit/refine.rs`, `fit/validate.rs`
When the particle filter collapses during scout (all weights zero, `best_loglik = −∞`),
`scout.rs` still writes `fit_state.toml` with that value. Neither `refine.rs`
(~line 26) nor `validate.rs` (~line 35) check `prior_state.best_loglik.is_finite()`
before launching expensive IF2 chains seeded from garbage start values.

### M6 — Profile IF2 hardcoded to 1000 particles

**File:** `rust/crates/cli/src/fit/validate.rs` ~459
```rust
n_particles: 1000,
```
Profile likelihood runs always use 1000 particles regardless of `pfilter_particles`
in `fit.toml` or the n_particles used for the main fit. For models that require
more particles to avoid filter collapse, profile likelihoods silently degenerate:
all loglik = −∞ → flat profile → CI spans full parameter bounds.

### M7 — `pfilter_loglik.txt` writes total loglik, not train-only, when holdout is active

**File:** `rust/crates/cli/src/fit/validate.rs` ~255–259
`loglik` written to `pfilter_loglik.txt` is `pf_result.loglik`, the sum over
all `ll_increments` (train + holdout). The filename and convention suggest
this is the fit assessment loglik, but when holdout is active it conflates
training and holdout fits. Combined with H1, there is no file anywhere that
records the split train/holdout logliks.

### M8 — `--obs-model` flag silently ignored when IR has observation block

**File:** `rust/crates/cli/src/if2.rs` ~370–398
When `model.observations.first()` is `Some`, the IR observation block is used
and `--obs-model normal` (any non-default user value) is silently dropped.
The message emitted is "using observation model '{}' from IR" — no mention
that the user's flag was overridden. A user migrating from `--obs-model` to
IR blocks who forgets to remove the flag gets no diagnostic.

### M9 — `final_states` flow accumulators always zeroed

**File:** `rust/crates/sim/src/inference/particle_filter.rs` ~213–225
`state.reset_flows()` is called on line 213 after the last observation interval
before `swarm.states` is moved into `final_states: Some(swarm.states)`. The
returned particle states have `flow_<transition>` accumulators all zero, even
though users requesting `--save-final-state` presumably want to know the flows
from the final simulation interval.

---

## Low

### L1 — Holdout sort + static `n_train` split index is architecturally fragile

**File:** `rust/crates/cli/src/fit/validate.rs` ~108–123
After concatenating train + holdout obs, the combined vector is sorted by time.
`n_train` is computed before sorting as the count of training observations.
The overlap guard checks only `holdout_min_t > train_max_t` (no interleaving),
but if this guard ever relaxes, sorting would invalidate the static `n_train`
split index silently.

### L2 — `holdout_stream` key extracted but never used

**File:** `rust/crates/cli/src/fit/validate.rs` ~96
```rust
let (holdout_stream, holdout_path) = holdout_data.iter().next()...;
```
`holdout_stream` (the stream name) is a dead binding — only `holdout_path` is
used. Consequence: if holdout data requires a different observation model than
training, the stream name cannot be used to look it up. Related to H2 root cause.

### L3 — `final_states` always allocated regardless of `--save-final-state`

**File:** `rust/crates/sim/src/inference/particle_filter.rs` ~225
`final_states: Some(swarm.states)` is unconditional. The comment says "Only
populated when `save_final_state` is true" but the flag is never consulted in
`bootstrap_filter`. Every pfilter call in IF2's inner loop allocates a full
N_particles × state copy unnecessarily.

### L4 — No `--disable` flag in `pfilter` arg parser

**File:** `rust/crates/cli/src/pfilter.rs` ~38–77
`pfilter` parses `--enable` but not `--disable`. The `simulate` command has
both. Not documented as intentional.

### L5 — `.unwrap()` on `param_index.get()` in `collect_all_params`

**File:** `rust/crates/cli/src/fit/runner.rs` ~1059
```rust
compiled.param_index.get(p.name.as_str()).copied().unwrap()
```
If model and compiled model are ever out of sync (partial load failure),
this panics with no user-readable error message. Called from multiple sites
in `validate.rs` and `refine.rs`.

### L6 — `NaN` in `observed` column formatted inconsistently in pfilter trace

**File:** `rust/crates/cli/src/fit/validate.rs` ~386
Other numeric columns use `{:.0}` (integer formatting). Holdout `observed`
rows produce `NaN` which passes through to TSV without the integer rounding,
inconsistent with the rest of the file. Downstream parsers expecting integer
counts may choke.

### L7 — `observe` crate is an empty stub

**File:** `rust/crates/observe/src/lib.rs`
Content: `// Stub — observation sampling not yet implemented.`
All observation model logic lives in `sim/src/inference/dmeasure.rs`.
The architectural separation described in `CLAUDE.md` (`cli → io → observe → sim → ir`)
is not implemented.

---

## Summary table

| ID | Severity | File | Issue |
|----|----------|------|-------|
| H1 | High     | `fit/validate.rs:132` | Holdout loglik never written to any output file |
| H2 | High     | `sim/inference/dmeasure.rs:89` | BetaBinomial returns −∞ silently (not implemented) |
| M1 | Medium   | `fit/validate.rs:385` | pfilter_trace.tsv observed column NaN for holdout rows |
| M2 | Medium   | `fit/validate.rs:649` | fit_report.txt content_hash is always hash of empty map |
| M3 | Medium   | `fit/validate.rs:519` | Profile curvature at grid midpoint, not MLE |
| M4 | Medium   | `cli/if2.rs:362` | cooling_target_iters=n_iterations diverges from pomp cf50 |
| M5 | Medium   | `fit/scout.rs`+`refine.rs` | Degenerate scout (-inf) not blocked from seeding refine |
| M6 | Medium   | `fit/validate.rs:459` | Profile IF2 hardcoded 1000 particles |
| M7 | Medium   | `fit/validate.rs:257` | pfilter_loglik.txt conflates train+holdout loglik |
| M8 | Medium   | `cli/if2.rs:370` | --obs-model flag silently dropped when IR has obs block |
| M9 | Medium   | `sim/inference/pf.rs:213` | final_states flow accumulators always zeroed before save |
| L1 | Low      | `fit/validate.rs:108` | Holdout sort + static n_train split index fragile |
| L2 | Low      | `fit/validate.rs:96` | holdout_stream key extracted but never used |
| L3 | Low      | `sim/inference/pf.rs:225` | final_states always allocated (ignores --save-final-state) |
| L4 | Low      | `cli/pfilter.rs:38` | --disable flag missing from pfilter (present in simulate) |
| L5 | Low      | `fit/runner.rs:1059` | .unwrap() on param_index.get() with no error message |
| L6 | Low      | `fit/validate.rs:386` | NaN observed rows formatted inconsistently in trace TSV |
| L7 | Low      | `observe/src/lib.rs` | observe crate is an empty stub |
