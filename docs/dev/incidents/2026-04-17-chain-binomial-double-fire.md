# Incident: chain_binomial fires scheduled interventions twice

**Date:** 2026-04-17
**Status:** Resolved in the same commit that added `fit`-path scenario support.
**Severity:** Medium for past simulate runs (wrong transfer arithmetic); zero for past inference runs (fit had no scenario/enable support before 2026-04-17, so toggleable interventions could not be activated during PGAS/PMMH until today).
**Fix commit:** `6e9d642` (bundled — the fix and the intervention/event-default work shared a test fixture).

## Fundamental vs. implementation

**Fundamental:** nothing. The chain-binomial algorithm has no opinion on how many times a scheduled intervention should fire. This was pure implementation drift.

**Implementation:** `rust/crates/sim/src/chain_binomial.rs` had two independent code paths that both invoked `apply_interventions_at(...)` for the same scheduled firing time — once inside `step_one` (at `t_end = t + dt`) and once in the outer `run_chain_binomial` loop (after `t += dt`). Both resolved to the same `current_step = (t / dt).round()` and therefore both matched the same `fire_steps` entry. Every scheduled intervention fired twice.

## Summary

When a scheduled intervention at time `T` was simulated under the chain-binomial backend:

- Inside `step_one`, at the step from `t = T − dt` to `t_end = T`, `apply_interventions_at(T, ...)` fired the intervention once.
- Then in the outer loop, `t += dt` advanced `t` to `T`, and `apply_interventions_at(T, ...)` fired it again.

A `FractionTransfer(fraction = 0.5)` that should have left 50% behind left 25% behind ((1 − 0.5)² = 0.25). An 80% SIA became effectively (1 − 0.2²) = 96%. The effect was deterministic, fraction-dependent, and entirely invisible to a user who had no ground truth to compare against.

Events (`always_active = true`) were **not** affected. `apply_interventions_at` short-circuits events at `rust/crates/sim/src/intervention.rs:50` (`if iv.always_active { continue; }`); events are handled via a separate `inject_event_deltas` path called exactly once per step inside `step_one`. Only toggleable interventions double-fired.

## Blast radius

- **Gillespie and ODE backends:** unaffected. Single intervention-handling path each. Verified by reading `gillespie.rs:131–142, 174` and `ode.rs:194, 230`.
- **`simulate --batch` / `simulate --cas` under chain_binomial with an `--enable` or scenario that activated an intervention:** affected. Effective transfer fraction was `(1 − (1 − f)²) = 2f − f²` instead of `f`. For a 20% transfer that's 36% (not catastrophic); for an 80% transfer that's 96% (very close to a complete sweep). Anyone who tuned a parameter against observed post-intervention data got slightly biased estimates.
- **`fit run` under chain_binomial with scenario-enabled interventions:** *zero* impact on past runs. `fit/runner.rs` had no scenario/enable/disable support before commit `6e9d642` (2026-04-17); toggleable interventions could not be activated during inference. The bug existed in the code path but never executed for users. This is the only reason the blast radius is small.
- **Particle filter `ChainBinomialProcess::step`:** structurally affected — it calls `step_one` directly and had no outer-loop double-fire to begin with, so it was *correctly single-firing*. The PF path was right by accident; `run_chain_binomial` was wrong.

## How we found it

Writing integration tests for the intervention/event-default work (the spec-conformance fix to `fit`/`pfilter`), I set up a minimal IR with an intervention transferring 50% of `S → V` at `t = 10` and asserted `S = 550` after firing (from `S = 1100` post-event). The test failed; actual output was `S = 275`. 275 = 1100 × (1 − 0.5)² = 1100 × 0.25 — immediately suspicious because the square of 0.5 suggests the fraction was applied twice rather than some rounding or off-by-one error. Tracing the two `apply_interventions_at` call sites in `chain_binomial.rs` confirmed it.

## The fix

`rust/crates/sim/src/chain_binomial.rs`: remove the outer-loop call to `apply_interventions_at` in `run_chain_binomial`. `step_one` already fires interventions at `t + dt`, and the particle filter's `ChainBinomialProcess::step` relies on that — keep one canonical path.

The outer loop retains an `iv_idx` advance step so the per-firing bookkeeping (knowing how many have fired) stays in sync, but the firing itself no longer happens twice.

Before (buggy):

```rust
// run_chain_binomial
step_one(...)?;         // fires interventions at t+dt
t += dt;
if iv_times.get(iv_idx).copied().is_some_and(|iv| (iv - t).abs() < cfg.dt * 0.5) {
    apply_interventions_at(t, ...)?;   // ← FIRES AGAIN
    while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + cfg.dt * 0.5 {
        iv_idx += 1;
    }
}
```

After (fixed):

```rust
step_one(...)?;         // fires interventions at t+dt (canonical)
t += dt;
// Bookkeeping: advance iv_idx past any intervention that fired
// in step_one this step. The firing itself happens in step_one.
while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + cfg.dt * 0.5 {
    iv_idx += 1;
}
```

## Regression test

`rust/crates/sim/tests/interventions.rs::chain_binomial_fires_scheduled_intervention_exactly_once` — builds a trivial model with `S = 1000, V = 0`, a single scheduled `FractionTransfer(0.5)` at `t = 10`, and asserts `S = 500` (single-fire) rather than `S = 250` (double-fire) at any snapshot post-firing. Would have fired red against the old code; passes against the fix.

## How did this escape for so long

Three things lined up:

1. **Existing intervention tests were unit tests of `apply_interventions_at` itself**, not of a full chain_binomial run. They correctly verified that *one call* to the function transferred the right number of particles. They didn't exercise whether the chain_binomial run loop called the function twice.
2. **No golden uses `interventions {}` activated by a scenario** in the repo. The golden suite's interventions are either in `events {}` (always_active; unaffected) or declared but not enabled by any of the tested scenarios (so they never fired). The end-to-end integration path was never exercised with a toggleable intervention under chain_binomial.
3. **`fit` had no scenario/enable support until today**, so users running Bayesian inference under chain_binomial could not reach the code path even if they wanted to. The buggy code was dormant for inference users.

The combination meant the bug sat undetected until an integration test explicitly asserted the arithmetic.

## Followup actions

- **Done in `6e9d642`:** the fix itself; regression test; this incident report.
- **Done in `6e9d642`:** the intervention/event-default work that would have surfaced this bug regardless. Every integration test of scenario-enabled interventions under chain_binomial is now a guard against this class of regression.
- **Not done; worth considering:**
  - Add a chain_binomial integration test that uses a model with multiple scheduled interventions at different times (not just one) to guard against a future regression where the outer-loop call returns in a different form.
  - Audit `gillespie.rs` for a similar double-fire pattern (it has both a "absorbing state" branch that calls `apply_interventions_at` and a "boundary crossed during event draw" branch that also calls it — but they're mutually exclusive per-step, so it's structurally immune). Verified fine by reading, but a unit test would be nice.
  - Document in `docs/camdl-run-spec.md` §2.3.1 (the Gillespie/tau-leap/ODE/discrete-time intervention semantics) that intervention firing is the responsibility of `step_one` in chain_binomial, not the outer loop. Would have prevented this regression if read by whoever added the outer-loop call.

## Lessons

- **Fire-count invariants need end-to-end tests, not unit tests.** `apply_interventions_at` had thorough unit coverage. `chain_binomial::run_chain_binomial` had no coverage of "does this thing fire each intervention exactly once." The unit vs. integration boundary is exactly where this kind of bug hides — each component does its job correctly; the composition doesn't.
- **"Obvious" arithmetic regressions are easy to catch once observed, hard to see until someone looks for them.** `S = 275` instead of `S = 550` is a factor of two, immediately visible to a human who wrote the test. The reason nobody looked: nobody had a model-level assertion that said "after one 50% transfer, S should halve."
- **Dormant bugs in buggy-on-paper code can be zero-impact until another feature lights up the path.** The double-fire lived in production for months without anyone hitting it because `fit` couldn't activate interventions. The instant `fit` gained scenario support (today), users would have hit this. Landing both in the same commit turns a release regression into an internal detail.
