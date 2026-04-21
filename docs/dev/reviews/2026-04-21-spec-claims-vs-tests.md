# Spec claims vs. test coverage audit (2026-04-21)

## Context

The 2026-04-21 table-unit incident (`docs/dev/incidents/2026-04-21-table-unit-annotations-ignored.md`) was the class of failure where **the spec made a claim that no test verified**. §6.1 promised the compiler would normalize table values to the model time unit; the compiler never did; the bug shipped silently.

This document audits the rest of `docs/camdl-language-spec.md` for the same pattern: compiler-behaviour claims without failing tests behind them.

## Methodology

Per-section pass through the spec, with each testable claim classified as:

- **VERIFIED** — a test in `ocaml/test/*` or `rust/crates/*/tests/` would fail if the claim were violated.
- **PARTIAL** — a test touches the area but asserts the IR *shape* or compile success, not the specific numerical or behavioural property the claim asserts.
- **UNVERIFIED** — no test exercises the claim; a regression would compile clean and ship.

Error-code coverage was computed by diffing `"E###"` / `"W###"` strings emitted in `ocaml/lib/` against occurrences in `ocaml/test/**` + fixture filenames. **90 codes are emitted; 26 are tested; 64 have no test** (71% uncovered).

## Priority 1 — silent wrong-answer risk

Claims whose violation would produce numerically wrong output with no error signal. Same failure class as the 2026-04-21 table-unit incident.

### P1.1 — Scenario `scale = { ... }` actually multiplies parameter values at runtime

- **Spec:** §17.1 "`scale` multiplies each named parameter's value by the given factor."
- **Implementation:** `rust/crates/cli/src/util.rs:753` applies the multiplication (`p.value = Some(v * factor)`).
- **Test:** `ocaml/test/test_compiler.ml:761` verifies the factor is stored in `preset_scale`; NO test verifies it's applied at runtime. If the application line were deleted, all scenario-scale-driven sensitivity analysis would silently run at the baseline values.
- **Priority:** HIGH — scenario `scale` is the documented sensitivity-analysis mechanism; a silent no-op here invalidates every `scale = ...` experiment.
- **Fix:** end-to-end Rust integration test: compile a model with `scenarios { sa { scale = { beta = 2.0 } } }`, simulate with `--scenario sa --seed 1`, and assert the trajectory differs from the baseline in the correct direction (or assert `run.json` records the scaled params).

### P1.2 — Scenario `set = { ... }` replaces parameter values at runtime

- **Spec:** §17.1 "`set` replaces the named parameter's value."
- **Test:** `preset_params` field population is tested (`test_compiler.ml:764`); end-to-end runtime application is not tested directly. Similar shape to P1.1.
- **Priority:** HIGH — same risk class.
- **Fix:** same as P1.1.

### P1.3 — `consecutive()` Erlang staging produces `k·sigma` transition rates

- **Spec:** §14 (and examples throughout) imply `consecutive(dim)` + `k * sigma * E[s]` produces Erlang-k latent distribution.
- **Test:** Golden files include `seir_erlang.camdl` / `seir_erlang_staged.camdl` — their `.ir.json` stores the expanded structure but no runtime test asserts the resulting latent period distribution is actually Erlang-k (e.g., mean-variance ratio, CV).
- **Priority:** MEDIUM — Erlang staging is structural sugar; if the expansion is wrong, simulated dynamics differ from spec-described behaviour silently.
- **Fix:** statistical test in `rust/crates/sim/tests/` that forward-simulates an SEIR with `consecutive(e in erlang_3)`, measures the latent-period distribution across many particles, and asserts mean/variance match the analytical Erlang-3.

### P1.4 — `interpolated { data = "file", value_col = ... }` loads file values verbatim (no unit handling)

- **Spec:** §7 says `interpolated` loads CSV values. Does NOT claim unit handling — but also doesn't warn that none happens.
- **Test:** `load_interpolated_for_level` in `expander.ml:2026` has no direct test. Downstream goldens exist but no assertion that loaded values match the file's floats.
- **Priority:** MEDIUM-LOW — not a spec-claim violation (spec silent), but a documentation gap that could lead to the same class of bug as the table-unit incident if a future user writes `value_col = rate_per_year` expecting auto-conversion.
- **Fix:** add a canonical-file-loads-correctly test; explicitly document in the spec that the user is responsible for pre-converting the CSV to model time unit.

### P1.5 — Table `read("file")` path now scales units correctly (NEW, post-fix)

- **Spec:** §6.2 `read(...)` loads long-format TSV.
- **Status:** Now verified. Post-fix (c3308da) unit conversion is applied in both the inline and `read` paths. No existing golden exercises a unit-annotated `read`-loaded table, so the `read` branch of the fix currently has no regression test beyond the manual smoke done during the fix.
- **Priority:** LOW (fix is in; backfill a test).
- **Fix:** add a `table_unit_conversion` case with `read("fixture.tsv")` + unit annotation, assert the loaded values are scaled.

---

## Priority 2 — loud-error semantics

Claims of the form "the compiler must emit E### on input X". **64 of 90 emitted error codes have no test.** Partial catalog of the highest-value gaps follows. Full list at the end of this section.

### P2.1 — E200 (missing required first positional path arg in `read`/`external`)

- **Spec:** §6.2 `read(PATH, column = ...)`; first positional is the path string.
- **Emitted:** `expander.ml:extract_path_arg` when the first positional is missing/non-identifier.
- **Tested:** No. `grep E200 ocaml/test/` returns nothing.
- **Risk:** A mistyped `read(column = "x")` without path falls into the diagnostic path, but if the diagnostic is ever downgraded or the helper refactored, the silent behaviour (table becomes empty `Inline []`) takes over.

### P2.2 — E204 (partial stratification stoichiometry)

- **Spec:** §5.1 destination compartment must name all dimensions the source is indexed by.
- **Tested:** Agent flagged this; no `e204_*.camdl` error fixture exists.
- **Risk:** Invalid models silently compile with the wrong stoichiometry.

### P2.3 — E220 (`date()` without an `origin` declaration)

- **Spec:** §2.3 `date("YYYY-MM-DD")` requires a top-level `origin`.
- **Tested:** test_compiler has 1 `E220` reference (the string "E220" appears in grep); let me spot check — actually it's only used in error message construction, not a direct test. Needs verification.

### P2.4 — E401–E408 (forcing function argument/shape errors)

- **Spec:** §7 lists forcing-function kwargs (`amplitude`, `period`, `phase`, `breakpoints`, `values`, etc.).
- **Tested:** 0 of E401-E408 appear in any test file or fixture directory.
- **Risk:** 8 distinct validation errors for forcing blocks — any of them could silently degrade to no-op if a refactor drops the check.

### P2.5 — E500–E511 (observation likelihood / prior validation)

- **Spec:** §13.2 likelihood families; §28 (or equivalent) prior distributions.
- **Tested:** E230-E235 (prior codes) mostly tested; E500-E511 are likely observation/runtime codes with no direct test. 12 codes, 0 tested.
- **Risk:** Observation-model misconfigurations compile clean until the runtime explodes (bad error location vs a compile diagnostic).

### P2.6 — E260–E276 (expansion / indexing errors)

- **Spec:** §5, §9, §26 (expansion) — indexing errors for mismatched dims, unknown index variables, etc.
- **Tested:** None of E260-E276 appear in tests. 17 codes, 0 tested.
- **Risk:** Stratification + indexing is the most mistake-prone area of the DSL; if any of these checks regresses, users get wrong IR silently (same class as E263 table-lookup bug fixed 2026-04-19 via C2 review).

### P2.7 — Warnings W100 / W200 / W203 / W301 / W311

- **Spec:** W-codes are advisory but documented; if suppressed by a refactor the user loses early signal.
- **Tested:** W103, W201, W310 tested (3/8); W100, W200, W203, W301, W311 untested (5/8).

### Full list of emitted-but-untested codes

```
E101 E102 E103 E104 E105 E106 E107 E108 E109
E200 E205 E206 E207 E208 E209 E210 E211 E212 E214 E215 E218 E219
E260 E261 E262 E263 E264 E265 E266 E270 E271 E272 E273 E274 E275 E276
E307 E308
E401 E402 E403 E404 E405 E406 E407 E408
E500 E501 E502 E503 E504 E505 E506 E507 E508 E509 E510 E511
E600
W100 W200 W203 W301 W311
```

64 codes. Plan: create one `errors/eNNN_*.camdl` fixture per code (each ~15-line minimum-reproducer), register via the existing `negative_golden` test suite. Cost: ~1-2 days of systematic work; payoff: every diagnostic becomes a regression trip-wire.

---

## Priority 3 — structural expansion claims

Claims about how the DSL expands into flat IR. Golden tests cover these indirectly (the stored IR represents an expansion), but the *specific claim* (e.g., "N compartments × |dim| strata = N×|dim| compartments") isn't asserted as an invariant.

### P3.1 — `let` bindings are inlined at use sites

- **Spec:** §9 "`let N = S + I + R` is inlined wherever `N` appears."
- **Tested:** No direct assertion. Goldens use `let` extensively, so inlining is implicitly exercised, but if inlining silently stopped happening (`let` treated as a runtime lookup instead), the goldens might still compile — just producing different IR we wouldn't notice without a baseline.
- **Fix:** assert that a model with `let N = S + I` produces transitions containing `BinOp { Add, Pop "S", Pop "I" }` in its rate tree, NOT `Let` / `Ref` nodes.

### P3.2 — `stratify(by = dim)` expands N compartments to N × |dim|

- **Spec:** §5.
- **Tested:** Goldens exercise stratification; count-invariant isn't asserted directly.
- **Fix:** test compiling `compartments {S, I, R}` + `stratify(by = age)` with 3 age levels produces exactly 9 compartments named `S_c1`, `S_c2`, ..., `R_c3`.

### P3.3 — Coupling sugar expands to full sum-over-dims form

- **Spec:** §10 shows sugar `@ beta * S * I / N { coupling[age = C_age] }` expanding to the explicit `sum(b in age, C_age[a,b] * I[b] / N[b])` form.
- **Tested:** No direct expansion equality test. Goldens exist for coupled models (sir_coupling.camdl) but only verify compile-success + IR hash, not the specific expansion shape.
- **Fix:** one test that compiles both the sugar form and the primitive form, asserts their rate expressions are structurally equal.

### P3.4 — `consecutive((s, s_next) in consecutive(dim))` pairs adjacent levels only

- **Spec:** §14 (Erlang section).
- **Tested:** Partial — seir_erlang_staged.camdl is a golden, but the count of generated transition pairs (should be |dim|−1, not |dim|²) isn't asserted.
- **Fix:** assert that `consecutive(erlang_E)` with 3 levels produces exactly 2 expanded transitions.

### P3.5 — `incidence(transition[p])` positional + `incidence(transition[patch = p])` named both sum over unspecified dims

- **Spec:** §13.1 (recently clarified in commit 3960453).
- **Tested:** No. This was the subject of the recent spec clarification but no test asserts the behavioural claim (both forms produce the same `CumulativeFlow` IR node, summing over unspecified dims).
- **Fix:** compile a model with both forms side-by-side, assert the resulting `CumulativeFlow` projections are equal.

---

## Priority 4 — documented defaults

Easy-to-test, easy-to-miss. Changing one line in a default can ripple through downstream results.

### P4.1 — Default `time_unit` when unspecified

- **Spec:** Currently ambiguous — spec examples all declare `time_unit = 'days` explicitly; behaviour of an omitted declaration isn't documented.
- **Tested:** `expander.ml:46` sets default `Days`. No test asserts this.
- **Fix:** test a model with no `time_unit` declaration, assert it produces IR consistent with days.

### P4.2 — Default backend is `chain_binomial`

- **Spec:** Language spec §21 CLI docs.
- **Tested:** `cli/tests/backend_provenance.rs::standalone_params_use_chain_binomial_default_in_run_json` ✓ verified.
- **Status:** OK (tested, but via Rust end-to-end, not at spec level).

### P4.3 — Default `dt = 1.0` for discrete backends

- **Spec:** CLI docs.
- **Tested:** Used throughout but no test asserts `if dt is unspecified, the run uses 1.0`.
- **Risk:** LOW — trivially caught by any diverging test.

### P4.4 — `scenarios { }` default (interventions off, events on)

- **Spec:** §17 "toggleable interventions default OFF; events always fire unless explicitly disabled."
- **Tested:** `rust/crates/cli/tests/intervention_event_defaults.rs` ✓ verified.
- **Status:** OK.

### P4.5 — `simulate { from, to }` defaults when omitted

- **Spec:** from default? to default?
- **Tested:** No. `expander.ml` has defaults; no test asserts the values.
- **Risk:** LOW — any mismatched end time would produce wrong trajectories, likely detected downstream. But it's a silent-wrong-answer class in principle.

---

## Summary and next actions

| Priority | Count | Risk                              | Cost  |
|----------|-------|-----------------------------------|-------|
| P1 (silent wrong answer) | 5 | Matches table-unit incident class | 1-2 days |
| P2 (loud-error codes)    | 64 codes emitted, 0 tested | Regressions would silently downgrade diagnostics | 1-2 days (fixture-driven, scales linearly) |
| P3 (structural claims)   | 5 | Medium — goldens mask most drift | 0.5 day |
| P4 (defaults)            | 2 of 5 untested | Low — likely caught downstream | 2 hours |

**Recommended order:**

1. P1.1–P1.2 (scenario set/scale runtime application) — highest blast radius, easy test to write.
2. P2 bulk fixture campaign — mechanical, 64 small fixtures; could be delegated or done in a single focused pass.
3. P3.1 (let-binding inlining) — the `incidence()` one (P3.5) is next since we literally just clarified the spec.
4. P1.3 (Erlang statistical test) — more involved but directly guards a common epi-modelling construct.

This audit covers **~20 high-value gaps out of an estimated 80-100 testable spec claims**. A complete claim-by-claim pass would take another ~1 day; the gaps surfaced here are the ones with the highest likelihood of producing a table-unit-class silent failure.
