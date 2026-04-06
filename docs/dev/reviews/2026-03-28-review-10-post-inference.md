---
status: closed
date: 2026-03-28
note: All issues from this review have been addressed and incorporated.
---

# Code Review #10 — Full Codebase (Post-Inference)

**Codebase:** ~24,400 LOC. New since Review #9: `fit/` module (~2,500
lines across config, state, provenance, runner, scout, refine,
validate, status), rayon particle parallelization, StepScratch
allocation fix, intervention handling in step_one, IVP support,
updated IF2 with per-step cooling.

---

## What's correct and well-designed

### Fit workflow — excellent architecture
The scout → refine → validate pipeline is implemented end-to-end.
Exhaustive partition validation works. `fit.toml` parsing and bounds
checking are clean. `fit_state.toml` handoff between stages works.
`mle_params.toml` with provenance header is generated correctly.
`camdl fit status` reads all stages and produces colored output with
boundary warnings. The spec's design vision is faithfully implemented.

### Provenance — two-hash system is correct
`input_hash` covers model + data + fit.toml + seed + version.
`content_hash` covers the parameter values for tamper detection.
`verify_content_hash` correctly parses TOML comments to extract the
declared hash. The cache check in `check_cache` looks for the hash
in fit_record.json and summary JSONs.

### Interventions in step_one — fixed correctly
Interventions now fire inside `step_one` (chain_binomial.rs:423-435).
The implementation reuses scratch.int_s, copies counts in, applies
interventions, copies back only if any fired. The tolerance is
dt/2, matching the main simulation loop. This was the critical bug
from Review #9 — now fixed.

### StepScratch — allocation elimination
Pre-allocated buffers for IntState, propensities, draws, deltas,
handled flags, and probs. One per particle, reused across all
timesteps. The 16-19% speedup the benchmarks showed confirms this
was a real bottleneck.

### Rayon particle parallelization — correctly batched
Both particle_filter.rs and if2.rs use `par_iter_mut()` with one
dispatch per observation interval. Each thread runs all sub-steps
for its particles, keeping state in cache. Sync only at observation
times (for resampling). This is the correct design.

### Multi-chain IF2 moved to rayon
runner.rs:458 uses `into_par_iter()` for chains. Nested parallelism
(chains × particles) uses the same rayon pool. Work-stealing handles
the load balancing automatically.

### MAD-based auto rw_sd — correctly implemented
runner.rs:542-647. Collects best-loglik parameters per chain,
computes median and MAD, classifies chains as "good" (within 3×MAD),
errors if n_good < n_chains/2. Floor at 1% of median prevents
convergence stall. This matches the spec exactly.

### Observation time alignment — hard error
runner.rs:296-311. Validates that every observation time is a
multiple of dt. Hard error, not snap. Correct.

### IF2 cooling — matches pomp semantics
if2.rs:206-207. Per-filtering-step cooling factor computed as
`cooling_fraction^(1/(target_iters × n_obs))`. With 780 obs and
target_iters=50, this gives the correct slow cooling. The critical
50× mismatch from earlier is fixed.

### IVP parameters — correctly skipped after t=0
if2.rs:286. `if spec.ivp { continue; }` in the observation-time
perturbation loop. IVP params are only perturbed in the initial
perturbation block (line 245). Correct.

---

## Issues

### 1. BUG (High): Fixed param values wrong in chain final_params.toml

runner.rs:688-690:
```rust
base_params.iter().enumerate()
    .find(|(_, _)| true) // need param_index
    .map_or(0.0, |(_, &v)| v)
```

The `.find(|(_, _)| true)` ALWAYS returns the first element of
`base_params`. For any fixed parameter (not in `if2_params`), the
chain's `final_params.toml` will contain the value of `base_params[0]`
instead of the correct value. If `base_params[0]` is R0=56.8, then
every fixed parameter (N0, mu, k, psi, cohort) will be written as
56.8 in the output file.

**Impact:** Every chain's `final_params.toml` has wrong values for
fixed parameters. If anyone uses these files directly (rather than
`mle_params.toml` which uses `collect_all_params` correctly), they
get garbage.

**Fix:** The function needs `compiled.param_index` to map parameter
names to indices:

```rust
// Add param_index to the function signature, or pass compiled
let value = if let Some(spec) = if2_params.iter().find(|p| p.name == *name) {
    result.mle[spec.index]
} else if let Some(&idx) = param_index.get(name.as_str()) {
    base_params[idx]
} else {
    0.0
};
```

### 2. BUG (Medium): load_profiles returns empty map always

status.rs:206-229. The `load_profiles` function has a lifetime
issue comment ("We can't return &str here with owned data") and
discards all profile data:

```rust
let _ = (name, lo, hi);  // line 221 — discards the data!
```

The function always returns an empty HashMap. This means
`camdl fit status` never shows profile CIs even when profiles
have been computed. The status output is missing the
`CI: [52.1, 62.3]` information.

**Fix:** Change the return type to `HashMap<String, (f64, f64)>`
(owned String keys):

```rust
fn load_profiles(base_dir: &str) -> HashMap<String, (f64, f64)> {
    let mut profiles = HashMap::new();
    // ... parse ...
    profiles.insert(name.to_string(), (lo, hi));
    profiles
}
```

### 3. BUG (Medium): Cache check not wired through

scout.rs:17: `// TODO: cache check via input_hash once provenance
is wired through`. The provenance module has `check_cache` and
`compute_input_hash` fully implemented, but scout (and refine,
validate) never call them. The `--force` flag exists in the CLI
parser but has no effect — every run always executes.

**Fix:** At the start of each stage, compute input_hash and call
`check_cache`. If `CacheStatus::Match` and `!force`, print the
skip message and return early.

### 4. BUG (Medium): Resampling still clones entire swarm

particle_filter.rs:154 and if2.rs:308-309:
```rust
let old_states: Vec<ParticleState> = swarm.states.clone();
let old_params = particle_params.clone();
```

For IF2 with 1000 particles, each clone copies ~1000 ×
(n_compartments + n_transitions) × 8 bytes of state PLUS 1000 ×
n_params × 8 bytes of params. This happens at every observation
time (780 times for measles). Total: ~780 × 2 × 80KB = ~125MB of
clone churn per IF2 iteration. With 50 iterations, that's 6.2GB.

The double-buffer pattern eliminates this entirely:

```rust
// Allocate once outside the loop
let mut states_buf: Vec<ParticleState> = /* same size as states */;

// At each observation:
for (i, &src) in indices.iter().enumerate() {
    states_buf[i].counts.copy_from_slice(&states[src].counts);
    states_buf[i].flow_accumulators.copy_from_slice(&states[src].flow_accumulators);
}
std::mem::swap(&mut states, &mut states_buf);
```

### 5. DESIGN: Observation model not evaluated from IR expressions

The fit runner (runner.rs:386-402) hardcodes the observation model
as either NegBin or DiscretizedNormal, with parameters looked up
by name ("rho", "k", "psi"). The IR has a rich observation model
with Expr fields for mean, dispersion, and variance — but the fit
system doesn't evaluate these expressions. It reconstructs the
observation model from hardcoded parameter names.

This means:
- A model with `rho` named `reporting_rate` won't work
- A model with a different variance formula won't work
- The IR observation model is not actually used for inference

**Fix (medium-term):** Compile the observation model's Expr fields
into a `dmeasure` function the same way propensities are compiled.
The dmeasure takes (projected, observed, params, t) and returns
log-likelihood. The fit runner calls this instead of hardcoding
likelihood families.

For now, document that the fit system assumes standard parameter
names ("rho", "k", "psi") and two likelihood families.

### 6. DESIGN: Scout initial_loglik approximation is crude

scout.rs:72-74:
```rust
let initial_loglik = chain_results.results.iter()
    .map(|(_, r)| r.iterations.first().map_or(f64::NEG_INFINITY, |it| it.log_likelihood))
    .fold(f64::INFINITY, f64::min);
```

This takes the WORST chain's FIRST iteration loglik as the
"initial loglik." First-iteration loglik from random starts is
dominated by the random perturbation, not the starting params.
The spec says initial_loglik should be "pfilter at starting params
before fitting" — a clean pfilter at the model defaults with no
perturbation.

**Fix:** Before running scout chains, run one pfilter at the
starting params (200 particles is fine) and record that loglik.
This costs seconds and gives an honest baseline.

### 7. DESIGN: fit_state.toml stores estimated params only in start_values

FitState.start_values only contains the estimated parameters
(scout.rs:66-68 maps over `config.if2_params`). Fixed parameter
values are not stored. When the next stage loads the state and
applies start_values to base_params (runner.rs:79-83), fixed params
keep their model defaults. This is correct IF the model defaults
haven't changed — but if someone edits the model between scout and
refine, the fixed param values silently change. The state file
doesn't capture the full parameter vector.

**Fix:** Store all parameter values in start_values, not just
estimated ones. Then the fit_state is fully self-contained.

### 8. MINOR: Duplicate input_hash computation in refine and validate

Both refine.rs:139-150 and validate.rs have identical
`compute_input_hash` functions. Extract to a shared function in
runner.rs or provenance.rs.

### 9. MINOR: format_param in provenance.rs vs format_value in runner.rs

Two nearly identical formatting functions:
- provenance.rs:86 `format_param`
- runner.rs:698 `format_value`

Same logic, different names. Extract to one function.

### 10. MINOR: ESS thresholds in status.rs are hardcoded for N=10000

status.rs:81-83:
```rust
if min > 2500.0 { "✓ filter is healthy" }
else if min > 500.0 { "~ filter is marginal" }
else { "✗ filter is degenerate" }
```

These thresholds (2500, 500) assume N=10000 particles. With N=5000,
an ESS of 2500 is 50% — healthy. But the threshold should be
relative to N, not absolute. Use N/4 and N/10 as thresholds.

### 11. MINOR: StepScratch allocated per iteration in IF2

if2.rs:234:
```rust
let mut scratches: Vec<StepScratch> = (0..n)
    .map(|_| StepScratch::new(model))
    .collect();
```

This allocates N StepScratch buffers at every IF2 iteration
(50-100 iterations). Should allocate once before the iteration
loop and reuse. Move outside the `for iter in 0..config.n_iterations`
block.

---

## What's NOT wired through (spec vs implementation gaps)

### A. Experiment system doesn't know about fit provenance

The experiment system loads params via `load_params_toml` (util.rs),
which parses TOML values but ignores the provenance comment header.
When `experiment.toml` references `fit/validate/mle_params.toml`,
the experiment system doesn't verify the content hash, doesn't
record the input hash in its own provenance, and doesn't warn if
the MLE file has been modified.

**Recommendation:** When loading a params file, check if it has a
`# Content hash:` line. If so, verify it and emit a warning if
modified. Optionally record the input_hash in the experiment run's
provenance.

### B. Profile likelihoods don't use the common runner infrastructure

validate.rs has its own `run_profiles` function that constructs IF2
configs and closures independently. The profile code in
cli/src/profile.rs (for `camdl profile`) is a third implementation.
Three places that construct dmeasure closures, step_fn closures,
and IF2 configs.

**Recommendation:** Consolidate into runner.rs. The fit workflow's
profiles and the standalone `camdl profile` should both call the
same backend.

### C. Observation model in fit is not derived from IR

(See issue #5 above.) The IR has `ObservationModel` with `Likelihood`
variants and `Projection` types. The fit system partially uses these
(it resolves `CumulativeFlow` projections) but hardcodes the
likelihood evaluation. The IR observation model is 80% wired — the
last 20% (compiling likelihood Exprs into a dmeasure function)
would make the system fully general.

---

## Priority summary

| # | Issue | Impact | Fix effort |
|---|-------|--------|------------|
| 1 | Fixed params wrong in final_params.toml | **Correctness** — wrong output files | ~5 lines |
| 2 | load_profiles always empty | **UX** — status missing CIs | ~10 lines |
| 3 | Cache not wired through | **UX** — --force has no effect, reruns always | ~30 lines |
| 4 | Resampling clone overhead | **Performance** — 6.2GB clone churn per IF2 | ~30 lines |
| 11 | StepScratch per iteration | **Performance** — unnecessary re-allocation | ~5 lines (move outside loop) |
| 5 | Obs model not from IR Exprs | **Generality** — hardcoded likelihood families | ~100 lines |
| 6 | initial_loglik is approximate | **Accuracy** — misleading baseline | ~20 lines |
| 7 | fit_state stores partial params | **Robustness** — model edits break chain | ~10 lines |
| 10 | ESS thresholds hardcoded | **UX** — wrong for N≠10000 | ~5 lines |
| 8,9 | Code duplication | **Maintenance** | ~15 lines |

---

## Highest-ROI next features

### 1. Wire cache through (fixes #3)
The provenance system is fully implemented but unused. Adding 3
calls to `check_cache` (one per stage) makes `--force` meaningful
and prevents wasted reruns. ~30 lines, immediate UX win.

### 2. Double-buffer resampling (fixes #4)
The clone is the biggest remaining performance bottleneck in the
hot path. With rayon parallelization already in place, the clone
is synchronous overhead that all threads wait for. ~30 lines for
a measurable speedup.

### 3. Compile observation model from IR (fixes #5)
This unblocks models with non-standard observation parameter names
and custom variance formulas. It also makes the fit system correct
by construction — the same observation model is used for simulation
and inference. ~100 lines but high value for polio deployment.

### 4. Multi-stream observations
For polio (AFP + environmental surveillance), the pfilter needs to
sum log-likelihoods across multiple observation streams at each
time point. The IR already supports multiple `ObservationModel`
entries. The fit.toml already supports multiple `[data]` entries.
The runner currently errors on `fit.data.len() != 1`. Removing
that restriction and looping over streams in the dmeasure function
is ~50 lines.

### 5. ESS-adaptive resampling
When ESS > N/2, skip resampling to preserve particle diversity.
Accumulate weights instead of resetting. Critical for polio where
most observations are uninformative (0 AFP cases). ~20 lines in
the PF, with a `--resample-threshold` flag.
