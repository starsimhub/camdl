# Incident: Scenario `set` / `scale` silently overridden by `--params` file

**Severity:** Critical (silent wrong answer)
**Discovered:** 2026-04-21 by the spec-claims-vs-tests audit — the first
P1 test written (end-to-end scenario runtime application) failed.
**Location:** `rust/crates/cli/src/util.rs::run_simulation`
**Status:** Fixed and tested.

---

## Summary

`docs/camdl-run-spec.md` §1.3 documents the precedence:

```
params.toml (base values)
  ↓ overridden by sweep point overrides
  ↓ overridden by scenario params
  ↓ overridden by --param CLI flags
```

The implementation reversed two layers. `util.rs::run_simulation`
applied scenario `set = { ... }` and `scale = { ... }` at lines
~746-761, then applied the `--params FILE.toml` values at
~769-791. Result: a file value silently **overwrites** a scenario's
`set` value (and the post-scale product). Running

```
camdl simulate model.camdl --params baseline.toml --scenario fast
```

produced the baseline trajectory, not the "fast" scenario trajectory.
Every scenario-scale sensitivity analysis on any model using
`--params` was invalid — silently.

Same failure class as the 2026-04-21 table-unit incident: a spec
claim that nothing tested.

## Concrete reproducer

Pure-death model, mu = 0.1 in params.toml, scenarios:

```camdl
scenarios {
  slow { set = { mu = 0.01 } }
  fast { set = { mu = 0.5  } }
}
```

Baseline `S(20) = 1000 · e^(-0.1·20) ≈ 135.3`.

Pre-fix:

```
simulate model.camdl --params p.toml --scenario slow --backend ode
→ S(20) = 135   # identical to baseline; scenario had zero effect
simulate model.camdl --params p.toml --scenario fast --backend ode
→ S(20) = 135   # identical to baseline
```

Post-fix:

```
--scenario slow →  S(20) ≈ 819   (mu = 0.01)
--scenario fast →  S(20) ≈ 0.045 (mu = 0.5)
```

## Root cause

Order-of-operations bug. Looking only at `util.rs`, scenario
application sat ~50 lines above `--params` file application in the
function body. The programmer who wrote the block presumably
intended "apply scenario first, then let the user layer more on top,"
which is coherent in isolation — but the spec says the opposite, and
the spec is what users rely on. No test cross-checked the claim.

Notably, the scenario-params test in `test_compiler.ml:761` only
verified the IR *stores* `preset_scale = { x = 0.5 }`; it did not
verify the scale was *applied* at runtime. The IR-shape assertion was
the ceiling of what the existing tests reached.

## Fix

Split the scenario resolution block (`util.rs` lines ~694-764) into
two phases:

1. **Intervention filter (enable/disable)** stays where it was —
   intervention filtering is independent of parameter values, and
   scenarios' enable lists should be applied before anything else in
   the scenario layer.
2. **Parameter `set` / `scale`** are pushed through the block as
   `(Vec<(String, f64)>, Vec<(String, f64)>)` and re-applied after
   `--params` + `--param-vec`, before `--param` scalar overrides.

Final precedence (matches spec §1.3):

```
model declaration's parameter defaults
  ↓ overridden by  --params FILE.toml
  ↓ overridden by  --param-vec PREFIX=FILE  (batch "sweep point overrides")
  ↓ overridden by  scenario set / scale
  ↓ overridden by  --param NAME=VALUE CLI
```

## Tests added (TDD — written to fail first)

`rust/crates/cli/tests/scenario_runtime_application.rs`:

- `scenario_set_replaces_mu_value` — baseline vs `set = { mu = 0.01 }`
  vs `set = { mu = 0.5 }` must produce three distinct terminal
  values of S. Also checks the quantitative match to
  `exp(-mu · t_end)`.
- `scenario_scale_multiplies_mu_value` — `scale = { mu = 2.0 }` with
  baseline mu = 0.1 must produce `S(20) ≈ exp(-4) · 1000 ≈ 18`, not
  `exp(-2) · 1000 ≈ 135` (the no-op-scale result). This is the
  explicit assertion that the multiplier is plumbed end-to-end.

Both tests **failed against the pre-fix binary** with the diagnostic
message "Got: slow=135, baseline=135. If these are equal, the
scenario's `set` is not being applied at runtime." They pass
against the fix.

Runs the OCE backend (deterministic) to avoid RNG variability
masking the scenario effect.

## Audit impact

Search across downstream vignettes + book examples for any
`camdl simulate --scenario X --params Y` invocation — every such run
pre-fix produced the baseline, not the scenario. Scope to confirm on
the book / vignettes side. If any published result relied on a
scenario modification via `--params + --scenario`, it's
reproducible but numerically wrong.

The fit-runner path (`cli/src/fit/...`) uses a different parameter
resolution pipeline via `apply_params_file` + `apply_scenario_filter`
directly in sequence; that path was not subject to the same
inversion. Needs verification to confirm, but initial grep suggests
only the simulate path (`run_simulation`) was affected.

## How this could have been avoided

Same answer as the table-unit incident:

1. **An end-to-end test of the documented precedence.** The spec
   explicitly lists the override order; one integration test per
   layer boundary would have surfaced this the moment it regressed.
   The test now added is ~100 lines including fixtures; easy to add
   proactively.
2. **Paired reviewer heuristic: "any documented override order must
   have a test that orders N runs and asserts each pair."** A one-
   sentence policy in `docs/dev/` would catch this class at review.
3. **Closer correspondence between IR-shape tests and runtime
   behaviour tests.** `test_compiler.ml:761` checking
   `preset_scale = 0.5` is necessary but not sufficient — the
   runtime layer that *uses* `preset_scale` deserved its own test.

## Commit reference

Fix: applied to `rust/crates/cli/src/util.rs` with regression test at
`rust/crates/cli/tests/scenario_runtime_application.rs`. 538/538
workspace tests pass (was 536/536 pre-fix + tests).
