---
title: "chain_binomial fire-step misalignment under sub-day dt (gh#53)"
date: 2026-05-09
severity: high
status: resolved
detection: gh#52 Richardson dt-check vs pomp on He et al. 2010 measles
fix-commit: 2141c84
issue: https://github.com/vsbuffalo/camdl/issues/53
related: gh#52 (Richardson dt-check)
---

# Cohort fire-step misalignment under sub-day dt

## Summary

`camdl pfilter --backend chain_binomial` agreed with the canonical
pomp reference (He et al. 2010 measles, London weekly cases
1944–1964) to within 0.3 nats at `--dt 1.0` day, and **diverged by
5862 nats at the literature MLE** under `--dt 0.125` day. pomp's
loglik stayed flat across dt halvings (~10 nat spread); camdl's
crashed monotonically (-5816 → -7119 → -9370 → -11678) with
particle-filter SE exploding (5 → 42 → 349 → 240 nats) as dt
shrank. Synthetic recovery passed at every dt. The compound
scout-convergence gate passed. Single-dt validation against pomp
at dt=1 passed. Every camdl-internal validator gave green; only
the gh#52 Richardson ladder against pomp at multiple dts
surfaced the bug.

Root cause: `CompiledModel.fire_steps` baked step indices at
compile time using `model.simulation.dt`, but the runtime
integrator's dt could differ. At dt=0.5 day on a model declared
at dt=1.0 day, the cohort impulse `at_day 258, every 365.25 days`
fired at wall time 129 (= step 258 × 0.5) and recurred every
half-year, instead of firing once per year on day 258.

Fix shipped as 2141c84. Validation reproduced on the same
reproducer post-fix shows lit_MLE flat to within ~30 nats across
the dt ladder (within PF SE), matching pomp's profile.

## Timeline

| When | What |
|---|---|
| **2026-05-07** | gh#52 (Richardson dt-check) shipped. Auto-runs at end of every IF2 fit; halving-ladder pfilter eval + two-leg verdict. |
| **2026-05-08** | gh#53 filed. camdl-book vignette `he2010-pomp` + `bench/pomp` ran the canonical pomp reference at multiple dts; observed the 5862-nat divergence at the lit MLE. |
| **2026-05-09 morning** | Bisect run 1: `sigma_se = 0.001` (Gamma noise effectively off). Divergence still ~11k nats at dt=1→0.5, but flat past dt=0.5. Hypothesis (1) Gamma noise scaling partially exonerated. |
| **2026-05-09 morning** | Bisect run 2: `cohort = 0.0001` (impulse pulse off). camdl ladder flattens to ~30 nats spread across all dts. **Hypothesis (2) confirmed**: cohort impulse-pulse handling. |
| **2026-05-09 morning** | Code archaeology: `compiled_model.rs:561` baked fire_steps at compile time using `model.simulation.dt`. Runtime dt comes in via `SMCConfig.dt`. The two could disagree, and did — silently. |
| **2026-05-09 afternoon** | Fix shipped (2141c84). Re-ran reproducer: lit_MLE ladder ~30 nats spread, scout_dt1.0 ~17 nats spread, all within PF SE. PF-SE inflation gone. |

## What passed (failed-to-detect surface)

This is the load-bearing section. Eight independent validators all
*passed* on this fit:

1. **Synthetic recovery** at the misconfigured dt — same dt on
   simulator and inference loop, discretization error cancels in
   the recovery metric. Identified as a structural blind spot in
   the gh#52 proposal's "Note: synthetic recovery shares this dt
   and cannot detect dt bias by itself" warning text — but until
   gh#53 we hadn't found a real-world case where dt bias was
   simulator-side rather than fit-side.
2. **Compound scout-convergence gate** (Â + decibans-spread). All
   chains converged on parameters; per-chain logliks within
   threshold. Â was not the failure mode here.
3. **Single-dt benchmark vs pomp** at dt=1 day — agreed to 0.3
   nats. The bug only surfaces when runtime dt ≠ compile-time
   dt; at dt=1 they coincided.
4. **Unit tests** in `crates/sim/`: ~470 unit tests, all green.
   `intervention.rs` had tests for FractionTransfer floor-vs-round,
   for transfer count parsing, for cohort-pulse application — but
   no test of dt-invariant fire counts under varying runtime dt.
5. **Golden-IR round-trip** (L1 in the L1–L9 testing layer
   taxonomy). Tests serialization, not simulation semantics.
6. **Determinism tests** (L4). Tests that the same seed produces
   the same output. Independent of dt-correctness.
7. **Spatial-density unit tests** (L4). Tests step_one's flow
   accumulation. Doesn't exercise interventions at sub-day dt.
8. **Statistical-distribution tests** (L7). Tests that
   distributions match theory. The He2010-class environmental
   noise pattern wasn't in this layer.

What **didn't** pass — and only because it was deliberately added
two days before the bug surfaced:

9. **L9 external validation against pomp at multiple dts**
   (de facto, via the gh#52 Richardson ladder). This is the only
   layer that catches dt-side bugs by construction. Pre-gh#52,
   even L9 only checked at one dt and would have passed.

## Detection mechanism — V&V case study

The Richardson dt-check (gh#52) was designed for a different
failure mode: detecting *fit-side* dt bias where the user's
chosen dt is too coarse for the model's process and the MLE is
discretization-fictive. Its terminal output explicitly tells the
user "Re-fit at dt ≤ <dt_min> before interpreting θ̂."

That advice is correct for the boarding-school SIR reproducer
(gh#52 §"Concrete reproducer") where the underlying model is
dt-stable in continuous time but the MLE shifts under coarse dt.
It would have been **wrong** for gh#53: re-fitting at finer dt
on a buggy integrator produces *more* discretization-fictive
output, more confidently. The post-mortem patch in
`c4867f4` adds a high-magnitude caveat (`leg2_abs > 100·τ_se_aware`):
when the dt-check trips at >100× the threshold (gh#52 case: 6.7×;
gh#53 case: 1052×), the verdict text now says

  > "For failures of this magnitude, the discretization itself may
  > be the issue — the integrator may be exhibiting a sub-step
  > numerical bug, not just a too-coarse fit dt. Cross-check
  > against an independent reference (e.g. pomp's pfilter) before
  > re-fitting."

Without that patch, gh#52 would have led users into worse fits
on this exact bug. *With* it, gh#52 + gh#53 form a complete
diagnostic stack: ladder fails → magnitude check splits "too
coarse fit" from "broken integrator" → external-reference
cross-check resolves which.

## Magnitude

Mean loglik divergence (camdl − pomp), 8 replicates × 4000 particles,
He2010 lit MLE on London 1944–1964 weekly cases:

| dt | camdl ll | pomp ll | Δ (camdl − pomp) |
|---|---:|---:|---:|
| 1.000 | −5815.82 | −5816.13 | +0.31 |
| 0.500 | −7119.10 | −5807.94 | **−1311.16** |
| 0.250 | −9369.62 | −5810.70 | **−3558.92** |
| 0.125 | −11678.29 | −5806.60 | **−5871.69** |

Particle-filter standard error inflated 50× over the dt ladder at
the lit MLE (5.03 → 42.18 → 349.28 → 240.13 nats). Post-fix SE
is bounded 2.9–5.0 nats across the same ladder.

Severity: **high**. The bug:
- Affected the canonical published benchmark (He et al. 2010
  measles, the most-cited validation case for stochastic
  compartmental MLE in pomp's documentation).
- Was undetectable by every camdl-internal validator.
- Produced PPC-level outputs (predicted weekly cases) that look
  like model misspecification — exactly the failure mode users
  *expect* a PPC to diagnose. Without external validation, a
  user would conclude "He2010 doesn't fit at fine dt" and shift
  the model, not the simulator.

## Root cause — code-level

`crates/sim/src/compiled_model.rs:559-568` (pre-fix):

```rust
let fire_steps: Vec<std::collections::BTreeSet<i64>> = {
    use crate::intervention::intervention_fire_times;
    let dt = model.simulation.dt.unwrap_or(1.0);   // ← compile-time dt
    model.interventions.iter().map(|iv| {
        let times = intervention_fire_times(&iv.schedule);
        times.iter()
            .map(|&ft| (ft / dt).round() as i64)
            .collect()
    }).collect()
};
```

This builds a `Vec<BTreeSet<i64>>` of *step indices*, baked using
the model file's declared `simulation.dt` (defaulting to 1.0 if
absent). It's stored on `CompiledModel`, which is constructed
once and shared across simulation runs at potentially different
dts.

`crates/sim/src/intervention.rs:60` (pre-fix), the runtime
checking site:

```rust
let dt = model.model.simulation.dt.unwrap_or(1.0);
let current_step = (t / dt).round() as i64;
if model.fire_steps[iv_idx].contains(&current_step) { ... }
```

Reads `model.simulation.dt` again — same value used to bake the
indices, so the index lookup is internally consistent... if and
only if the runtime stepper is also walking at that dt. But the
runtime integrator gets its dt from `SMCConfig.dt`
(`crates/sim/src/inference/traits.rs`), which originates from the
user's `--dt` flag and may differ from `model.simulation.dt`.

The mismatch was silent because both sides of the lookup were
internally consistent (no NaN, no out-of-range — the indices
just pointed to the wrong wall times). The integrator advanced
via `t += cfg.dt` at the runtime dt; `current_step` was computed
via `model.simulation.dt`; the lookup matched whenever
`current_step` (computed at compile dt) coincided with a baked
index. For He2010 cohort (`at_day 258, every 365.25 days`) at
runtime dt=0.5 with compile dt=1.0:

- Baked: `fire_steps = {258, 623, 989, ...}` (step indices).
- Runtime step counter: walks 0, 1, 2, ... at intervals of 0.5
  days each, so step 258 = wall time 129, step 623 = wall time
  311.5, etc.
- The intervention "fired" at wall times 129, 311.5, 494.5, ...
  — twice a year, six months early.

## Architectural lesson

The compile/runtime seam — the boundary between `CompiledModel`
(produced once by the OCaml→IR→Rust compile path) and the
simulator (run repeatedly with potentially different config
parameters) — must only carry **dt-invariant artifacts**. Any
artifact whose value depends on a config parameter that the
runtime can override must be derived at runtime, not baked at
compile time.

`fire_steps` violated this. So did the per-callsite inlined
`(t / dt).round() as i64` arithmetic in
`apply_interventions_at` and `inject_event_deltas`, which silently
used `model.simulation.dt` instead of the runtime dt — a second
seam violation that compounded with the first.

The fix consolidates the conversion into one entrypoint:
`sim::time::time_to_step(t, dt) -> i64`, with eight unit tests
covering the dt-scaling correctness that was previously implicit.
The compile-side artifact is now `CompiledModel.fire_times:
Vec<Vec<f64>>` (continuous, dt-invariant); the runtime view is
derived once per sim run via `CompiledModel::resolve_fire_steps(dt)`.

## Remediation — fix-commit 2141c84

| Component | Change |
|---|---|
| `sim::time` (new module) | `time_to_step` + `fire_times_to_steps` helpers, 8 unit tests covering dt-scaling, rounding, NaN/zero/negative dt debug-asserts. |
| `CompiledModel.fire_steps` → `.fire_times` | dt-invariant fire times stored at compile time; runtime view derived via `resolve_fire_steps(dt)`. |
| `apply_interventions_at` | Now takes `fire_steps: &[BTreeSet<i64>]` and `dt: f64` parameters; no `model.simulation.dt` fallback. |
| `inject_event_deltas` | Same shape — takes `fire_steps` parameter. |
| `step_one` (chain_binomial) | Takes `fire_steps` parameter. |
| `ChainBinomialProcess` | Stores `fire_steps` resolved at construction; `new()` takes `dt`. |
| Each backend (chain_binomial, tau_leap, ode) | Calls `model.resolve_fire_steps(cfg.dt)` once at sim start. Gillespie continues to use `model.simulation.dt` since it has no cfg.dt of its own. |
| `FitRunConfig::build_process` | New `build_process_with_dt(dt)` variant; gh#52 Richardson dt-check uses it to build a process per ladder rung at the rung's dt. |
| Test fixtures | All updated for new function signatures. No semantic test changes. |
| Regression tests (new) | 5 tests in `tests/intervention_dt_invariance.rs` pinning the structural invariant: integrated fire count over a fixed wall-time interval is dt-invariant. |

## Process change — L9 multi-dt promotion

This bug was structurally invisible to single-dt L9 (external
validation against pomp at one dt). Gh#52's Richardson ladder
*was* the multi-dt validator, but it shipped as a per-fit
diagnostic, not a CI gate. **Follow-up**: promote
`tests/external/he2010_multi_dt/` from a one-off bench to a
permanent CI L9 case — assert camdl matches pomp at the lit MLE
across `dt ∈ {1, 0.5, 0.25}` within (e.g.) 50 nats per dt rung.
File as a separate issue once landing tests on the fix has
settled.

A weaker but cheaper gate: extend the dt_check's high-magnitude
warning to include a structured TOML field
(`integrator_bug_suspected: bool`) so downstream tooling
(camdl-book chapter renderers) can refuse to publish chapters
where this fires. The c4867f4 patch already does the user-facing
warning; the structured field is a small follow-up.

## What didn't change

The fix does not alter the *intent* of any model file — `at_day
258, every 365.25 days` still means "fire on day 258 each year."
What changed is camdl's ability to interpret that correctly when
the runtime integrator walks at a dt different from
`model.simulation.dt`. Any fit that used `--dt`-equal-to-the-
model's-declared-`dt` (the dominant case in the camdl-book
vignettes prior to the gh#52 work) was always correct.

## Affected work

- **`camdl-book/vignettes/he2010-pomp/`** — paused at the time of
  detection. Chapter prose framing about dt as a model commitment
  remains correct; the worked example showing the dt-check FAIL
  was ambiguous (fit-side vs camdl-side) prior to the c4867f4
  high-magnitude patch + this fix. Chapter can resume; the
  pre-fix "FAIL" verdict on the lit MLE was caused by gh#53,
  not by a real fit-side dt issue, and the vignette should
  cite this incident as the canonical example of the
  Richardson-ladder validator earning its keep.
- **`camdl-book/guide/fitting/likelihood.qmd`** (boarding-school)
  — was fine and remains so. None of the suspect code paths
  exercised by gh#53 (Gamma noise, periodic interventions,
  covariates) appeared in that model. The dt-check FAIL on the
  dt=1 fit there *is* a genuine fit-side dt issue, exactly as
  the chapter teaches.
- **gh#51 survey-seeded fits** — fits at the standard `dt = 1.0`
  (which match pomp) are unaffected. dt-check verdicts on those
  fits at sub-day dt should be re-evaluated post-fix.
- **All published camdl fit results to date** — limited to
  pre-alpha; the only consumers are the camdl-book vignettes and
  the gh#52/53 reproducer artifacts. Nothing external.
