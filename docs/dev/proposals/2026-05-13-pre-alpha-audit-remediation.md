# Pre-alpha audit remediation

**Status:** Proposal
**Date:** 2026-05-13
**Motivation:** Per-finding response to `docs/dev/reviews/2026-05-12-full-audit.md` (8C / 14H / 21M); decides per item whether to fix-now, fix-right, gate-with-error, or defer, with reasoning. Frames the alpha as: a skeptical user running their own AI-assisted code review should not find low-hanging silent-failure bugs.

This is a companion to the audit. The audit is the *what*; this is the *what we're going to do about it and why*. Each finding header repeats the bug summary inline so this document is readable without flipping back.

---

## 0. How did so many bugs creep through this far?

Worth stating before the per-item plan, because several findings share root causes that are not "we missed it" but "the way we work currently lets this class of bug land." The remediation plan should attack the patterns, not just the instances.

**Pattern 1: tests assert correctness, not detection.** Almost every test in `rust/crates/sim/tests/` exercises the happy path: "given this model and these params, the trajectory matches this golden TSV." Very few tests assert that *errors fire when they should*. C5 (negative-count clamp), C6 (div-by-zero → 0), C4 (missing prior → uniform), and C3 (silent backend dropping `balance{}`) are all bugs where the silent-fallback path passes every existing test by construction. We need a "denial test" pattern: for each fallback path, an explicit test that triggers it and asserts it errors.

**Pattern 2: diagnostic infrastructure landed before detection logic.** The `DiagnosticKind` enum (`inference/diagnostic.rs`) was designed as a forward-looking catalogue: 22 variants covering the full taxonomy of things we'd want to surface. Roughly a third are wired; the rest were added with the expectation that the detection sites would be filled in as they came up. They mostly weren't. Same shape with `EvalStats` (H5) — counters in place, no reader. The lesson: a detection variant or counter without a wired reader is a TODO, and TODOs decay into dead code. Either wire it on the same PR or don't add the type.

**Pattern 3: two-language IR is hand-mirrored, drift is invisible until something breaks at the seam.** H6 (`param_kind: Option<String>`), H7 (`Interpolated.method` typed in Rust, raw string in OCaml), H14 (validation duplicated with different surfaces), M12 (`ModelStructure` deserialized + never read) all derive from C8: `ir/schema.json` and `ir/VERSION` were declared as "the contract" in CLAUDE.md but are referenced nowhere in source. There is no mechanism that catches when one side renames a field and the other doesn't. The handshake has to become real or the document needs to stop claiming there is one.

**Pattern 4: speed of frontier features outran observability.** The most recent ~2 months added PGAS+NUTS, autodiff, capabilities, prequential evaluation, profile-CAS integration, fit-experiment management. Each added inference math and CLI surface. Counter wires, diagnostic dispatch, CLI flag plumbing have less code-review attention because they are "obvious" — and so they are skipped, or a TODO comment is left ("currently zero because typically a constant" — C1's actual inline rationale). A skeptic running grep for `currently`, `for now`, `typically`, `TODO`, `FIXME`, `XXX` in `inference/` will surface most of the silent-fallback findings in one pass; we should do that grep ourselves before alpha.

**Pattern 5: CLAUDE.md "delete dead code on sight" is a recent rule.** Files like `ModelStructure` (M12), `SimulationConfig.{rng_seed, time_semantics}` (M13), the never-constructed `DiagnosticKind` variants (H4) accumulated before the policy hardened. The policy is enforceable in CI (clippy `#[deny(dead_code)]` at the workspace level for non-test code), and *not* enforcing it means CLAUDE.md is aspirational documentation, which is worse than not having the rule.

**What the remediation plan changes process-wise:**

- Add a CI lint pass: `cargo clippy -- -D dead_code` at workspace root (allow per-test, not per-module).
- Add a "denial test" pattern as a documented expectation: every `SimError` variant, every `DiagnosticKind` variant, every CLI error path should have a test that triggers it. Track unwired variants in `INFLIGHT.md` until covered.
- Add a "wire it now" PR rule: no `DiagnosticKind`, `SimError`, or `EvalStats`-style counter merged without at least one production call site and one detection test.
- Add `make audit-greps` target that runs the skeptic's first-pass greps (`currently`, `for now`, `typically`, `silently`, `TODO`, `FIXME`) on the inference and IR crates and fails on net-new occurrences.

---

## 1. Critical — per-finding plan

### C1. PGAS gradient drops obs-likelihood and gamma-density derivatives

**Bug.** `complete_data_loglik_grad` in `pgas_grad.rs:298-317` adds `obs_model.log_likelihood_from_flows_and_counts(...)` to the *value* (line 314) but never adds its gradient. Same for the per-substep gamma-multiplier density. Inline comments dismiss this with "currently zero because σ² is typically a constant (not estimated)" and "Observation density (gradient is zero when obs params are fixed)." When the estimated set includes any obs-likelihood parameter (`rho`, `psi`, `k`) or any parameter driving an overdispersion σ² expression, NUTS sees zero gradient for that coordinate while the log-posterior responds. Symptom: silent posterior bias on those coordinates with no preflight check.

**Decision: solve it right (path ii), and add the preflight gate (path i) as a defense-in-depth backstop that survives the fix.**

The audit offers two paths:

- (i) Hard error at `run_pgas` entry when an estimated param appears in obs-likelihood or σ² and isn't covered by `rate_grads_for_run`.
- (ii) Thread the obs-likelihood / gamma derivatives through `complete_data_loglik_grad` using the existing compiler autodiff.

Vince's instinct (favour ii) is correct. (i) would limit what users can fit — they could not put a prior on `rho` or `k` and run the production Bayesian method. For a public-health tool, that constraint would push users toward fitting obs params with IF2 and bolting them onto a PGAS run, which is exactly the sort of two-tool stitching that produces silently miscalibrated uncertainty. PGAS is documented as the production Bayesian method (CLAUDE.md); it has to handle the parameters users actually want to estimate.

The work decomposes into three pieces, all of which are tractable:

1. **Obs-distribution analytic gradients.** `obs_loglik.rs` already imports digamma — Poisson, NegBinomial, Binomial, and Normal log-PMFs all have closed-form derivatives w.r.t. their natural parameters. We need d/dθ for `rho` (rate scaling), `psi` (Poisson/NegBin offset), `k` (NegBin dispersion), and any `Normal { mean, sd }` parameters. These are textbook; the only engineering work is plumbing them through `eval_likelihood_resolved` so the dispatcher returns `(value, grad)` not just `value`.

2. **Gamma-density gradient w.r.t. σ².** `Gamma(g; dt/σ², σ²/dt)` has a closed-form derivative w.r.t. σ² (involves digamma). When σ² is a parameter expression rather than a constant, chain-rule through the existing `rate_grad` infrastructure: the compiler already emits gradient expressions for arbitrary rate exprs, σ² is just another rate-shaped expr, treat it the same way.

3. **Compiler emission of `obs_param_grad` and `sigma_sq_grad`.** Mirror the existing `rate_grad` codegen path in `ocaml/lib/ir/autodiff.ml`. Source-to-source on the obs-distribution arg expressions and σ² expressions. Resulting IR carries a parallel `obs_param_grads` block.

The preflight gate (i) survives this work as a backstop: if a future obs distribution gets added without its gradient, or an estimated parameter appears in an expression slot that wasn't audited for gradient coverage, fail-fast at `run_pgas` entry rather than silently bias. The gate is one day; (1)–(3) is the audit's "a week" estimate, which seems right. Land them in that order: gate first (closes the silent-bias hole immediately), then (1) closes most cases the gate would reject, then (2)–(3) close the remaining ones.

**Why "is the issue we aren't doing X" doesn't apply here:** unlike C2, this isn't a missing eval — the *value* is correct. The bug is purely on the gradient side, and the fix is to compute the missing terms, not to add another evaluation pass.

**Test:** `tests/pgas_grad_obs_param.rs` — finite-difference check on `complete_data_loglik_grad` with an estimated `rho` parameter; existing `gradient_check.rs:20` only exercises rate-only gradients.

---

### C2. IF2 returns MLE from the iteration that maximized the perturbed log-likelihood

**Bug.** `if2.rs:526-528` selects `best_iter` by `if2_perturbed_loglik.total_cmp`. The same file documents that field at lines 125-129: "IF2 perturbed-model log-likelihood (internal diagnostic only). Peaks early due to perturbation smoothing, then declines as cooling progresses. NOT useful for model assessment or convergence — use `loglik` instead." The clean-PF `loglik` is populated *post-hoc* by the caller, so it can't be used as the selection key inside `run_if2` without adding a fallback.

**Vince's question: "is the issue that we aren't doing a second loglik eval?"**

Partly — but not exactly. We *are* doing the second eval. `cli/src/fit/runner.rs:1183-1209` runs a clean PF every 10 iterations after `run_if2` returns and writes back into `it.loglik`. The bug is that `run_if2` makes its `best_iter` decision *before* that post-hoc fill-in runs, using the field its own docstring tells you not to use.

So there are three plausible fixes; ordered by quality:

1. **Defer selection to the caller.** Remove `mle` from `IF2Result`. The caller already has clean `it.loglik` populated post-hoc and already does an even better re-evaluation in `loglik_eval` (`runner.rs:1225`). The "best chain × best iteration" selection logic lives there. `IF2Result` becomes a pure record of what happened during the run; downstream code picks the winner. This is the right answer — `run_if2` shouldn't be making model-assessment claims because it doesn't have the right information at that point.

2. **Pick `last_iter`, not `argmax(perturbed)`.** IF2 theory: as cooling progresses, the particle swarm in parameter space contracts to a delta at the MLE. So under proper cooling, the *last* iteration is the MLE by construction. An earlier iteration where perturbation noise happened to push the perturbed loglik higher is *not* the MLE — it's a wider swarm where one realisation happened to score well. This is a 1-line fix and it removes the worst symptom.

3. **Keep selection inside `run_if2`, but move it after a clean-PF eval pass that `run_if2` runs itself.** Duplicates work the caller already does; rejected.

**Decision: do both (1) and (2), in that order.** (2) immediately because it's a 1-line fix that can land today and stops `result.mle` being actively wrong; (1) because the right architectural shape is for `run_if2` to be a value-free record of the trajectory and for the caller — which has access to clean re-eval — to pick the winner. After (1), `result.mle` goes away and consumers route to `loglik_eval_outcome` instead, which already has the right semantics.

Note that Vince's intuition about "second loglik eval" is the right *type* of fix — the issue is that the second eval exists but isn't being used at the right point. The fix is to use it, not to add another one.

**Test:** `tests/if2_mle_selection.rs` — synthetic IF2 run with a deliberately noisy perturbed loglik; assert `mle` matches the last-iteration mean (under 2) or that the caller's selection matches `loglik_eval`'s (under 1).

---

### C3. `balance {}` silently dropped by tau_leap, gillespie, and ode backends

**Bug.** Applied in `chain_binomial.rs:407,432` and `inference/pgas.rs:736,1279`; **no reference** in `tau_leap.rs`, `gillespie.rs`, or `ode.rs`. Worst case: user fits with chain-binomial and forward-simulates with `tau_leap` (faster); simulated trajectories no longer conserve population.

**Decision: option (b) — `BALANCE` capability, hard error on dispatch.**

The audit's two options:

- (a) Apply balance at end of each substep on tau_leap, gillespie, ode — mirroring chain_binomial.
- (b) Declare `BALANCE` capability; only chain_binomial supports it; dispatch fails fast on mismatch.

(a) sounds like the user-friendly answer but it is wrong in two ways. First, `balance{}` is *defined* as "the residual compartment after all transitions and events have fired," and the firing semantics are different across backends. On Gillespie, transitions fire asynchronously with no notion of a substep, so "apply balance at end of each substep" needs a definition of "substep" that doesn't naturally exist. On ODE, compartments are real-valued and balance reduces to an algebraic identity that the integrator should be enforcing structurally, not as a corrective post-step. Pretending the construct works on every backend by patching it on each path's terms papers over a real semantic difference and produces backends that look interoperable but aren't.

Second, `balance{}` exists primarily for conservation tracking on the chain-binomial path (population conservation under binomial splits). On a properly-formed ODE that includes the balance equation explicitly, you don't need `balance{}` at all — you just write the equation. On Gillespie, you also don't need it because transitions exactly conserve their reactants and products by construction. So forcing the construct onto those backends solves a non-problem and creates a real one (semantic drift).

(b) preserves the model author's intent: if you wrote `balance{}`, you're declaring that this model needs the chain-binomial residual fix. Asking for any other backend means you have a different model, not the same one with a different solver.

Implementation:
1. Add `BALANCE` to `Capabilities` in `sim/src/lib.rs`.
2. `CompiledModel::required_capabilities()` sets `BALANCE` whenever `model.balance.is_some()`.
3. Only chain_binomial's `Simulate::capabilities()` declares `BALANCE`.
4. Dispatch produces the standard hard error with the existing capability-mismatch hint format.

Take this opportunity to write a docs note explaining *why* balance is chain-binomial-only (the conservation-under-binomial-splits motivation) so future contributors don't try to "extend" it to other backends.

**Test:** `tests/balance_capability.rs::balance_model_rejected_by_tau_leap` — load any model with `balance{}` block, attempt tau_leap simulation, assert capability error.

---

### C4. Estimated parameters with no explicit prior silently get improper uniform

**Bug.** `runner.rs:1932-1957` (`resolve_prior` falls back to `Prior::Flat`); `prior.rs:91` (`Prior::Flat.log_density = 0`); `prior.rs:181` (silently maps `PriorDist::Fixed` → `Prior::Flat`). Code-review brief explicitly calls this out.

**Decision: hard error. No prior, no run.**

This is non-negotiable and Vince has already documented the reasoning in the code-review brief: "If the user omits a prior and the system silently uses an 'uninformative' default, that default appears in the posterior. Make it explicit. No prior, no run." For polio decision support, a credible interval reported as Bayesian but actually a likelihood profile is the worst-case communication failure.

The `PriorDist::Fixed → Prior::Flat` coercion at `prior.rs:181` is worse than the missing-prior case. Setting `prior = { fixed = ... }` *means* the user pinned the value. Silently treating it as flat means we executed the opposite of what the config said. Delete the conversion entirely; if a parameter is in a "prior slot" but typed `Fixed`, that's a config error, return a diagnostic explaining the user should either move it to the fixed-params section or provide a real prior.

Implementation:
1. `resolve_prior` returns `Result<Prior, FitConfigError>` instead of `Prior`.
2. New diagnostic `E5xx: prior_required { param_name }` listing offending params; hint text suggests the closest valid `prior = { ... }` block.
3. Drop the `PriorDist::Fixed → Prior::Flat` conversion at `prior.rs:181`; replace with `E5xx: fixed_in_prior_slot { param_name }`.
4. CLI translates the error into the standard fit.toml diagnostic format used elsewhere by the v2 config validator.

**Test:** `tests/fit_missing_prior_errors.rs` — fit.toml with an estimated parameter and no prior block; assert `cmd_fit_run_v2` exits with `E5xx_prior_required` and lists the param name.

---

### C5. Compartment counts below zero silently clamped to 0 in chain-binomial

**Bug.** `chain_binomial.rs:407-416`. Any overshoot in the binomial split, or an `Action::Add` with a negative resolved value (`intervention.rs:248-262`, only `log::warn!`s), silently zeros the compartment. Population not conserved; trajectory continues.

**Decision: typed error at the simulator boundary, with a layered policy that distinguishes inference from forward simulation.**

This finding has two distinct subcases that look identical in the audit but need different treatment. Bundling them under one fix would either crash inference runs that previously worked (bad) or quietly accept config bugs (also bad). Separating:

**Subcase 1 — Binomial overshoot during dynamics** (`chain_binomial.rs:407-416`). Happens because particles explore parameter regions where `rate·dt → 1`. This is *expected* behaviour during inference: PGAS, IF2, and PF will visit such regions in the course of estimating where the MLE / posterior mass is. A prior reviewer correctly flagged that hard-erroring at the simulator level here would crash production fit runs — `−Inf` log-likelihood for the offending particle is the inference-correct response (the particle dies in resampling, the chain continues exploring elsewhere).

The right design is layered:

- **Simulator primitive raises the typed error.** `chain_binomial::step_one` returns `Result<_, SimError::NegativeCount { ... }>`. The simulator does not silently clamp; the failure mode is structural and named.
- **Inference layers catch the error and convert to `−Inf`.** PGAS, IF2, PF wrap `step_one` calls; on `SimError::NegativeCount`, return `f64::NEG_INFINITY` for that particle's contribution and increment a counter (`degenerate_step_count` in the inference diagnostics). No crash.
- **Forward-simulation CLI propagates as a user-facing error.** `cmd_simulate` has only one parameter set; if that parameter set produces overshoot, the user wants to know — either to use a smaller `dt`, switch to a multinomial split, or fix the rates. Silent clamping here is exactly what hides modeling errors. Hard error with a hint pointing at `--dt` and the offending compartment.

This preserves the prior reviewer's "−Inf is less destructive" point (true at the inference layer) while still removing the silent failure mode the audit calls out (the *primitive* shouldn't be in the business of choosing 0 over a real population count).

**Subcase 2 — `Action::Add` with negative resolved value** (`intervention.rs:248-262`). Different beast. The user wrote a fit.toml or DSL expression that resolves to a negative add — this is structurally a config bug, not parameter exploration. There's no inference scenario where you "discover" that an intervention should add a negative number of individuals to a compartment. Always hard error, regardless of caller (forward-sim or inference). The current `log::warn!` is wrong because warnings get lost in a fit-run's stderr and the bias accumulates silently.

**Migration risk.** This *will* surface latent issues in existing models. Any production config that's been running with overshoot under the silent clamp will start producing `−Inf` particles in inference, and the user will see a non-zero `degenerate_step_count` in diagnostics. That's the right outcome — they should see it — but we should:

1. Run the typhoid and polio production configs (in `camdl-book`) before alpha and audit any non-zero `degenerate_step_count`. If high, the model needs a smaller `dt` before alpha ships.
2. Add an explicit opt-out for forward sim: `--allow-negative-clamp` (mirrors `--allow-degenerate-rates` from C6) for users who have a defensible reason to keep the old behaviour. Default off; opt-in is loud.
3. Document in the alpha changelog: "Chain-binomial overshoot now produces typed errors. Inference runs continue with `−Inf` particles; forward simulation halts. If your forward sim previously ran with the silent clamp, run with `--allow-negative-clamp` to restore old behaviour, or reduce `--dt`."

Implementation:
1. New variant `SimError::NegativeCount { compartment: String, attempted_value: i64, t: f64, cause: NegativeCountCause }` where `NegativeCountCause = BinomialOvershoot | InterventionAddNegative`.
2. `chain_binomial.rs:407-416`: replace clamp with `Err(SimError::NegativeCount { cause: BinomialOvershoot, ... })`.
3. `intervention.rs:248-262`: replace `log::warn!` with `Err(SimError::NegativeCount { cause: InterventionAddNegative, ... })`.
4. Inference layer wraps simulator calls; on `BinomialOvershoot`, increment `degenerate_step_count` and return `−Inf`; on `InterventionAddNegative`, propagate (config bug).
5. `cmd_simulate` adds `--allow-negative-clamp`; when set, the runtime catches `BinomialOvershoot` and clamps with a `log::warn!` (same as today, but explicitly opted-in).

**Test:** `tests/chain_binomial_invariants.rs::overshoot_errors_not_clamps` — model with two competing transitions whose `1 - exp(-r_i dt)` sum exceeds 1; assert hard error from `simulate_step` (forward-sim path) and `−Inf` increment + counter bump from the PGAS path. Plus `intervention_add_negative_errors` for the Subcase 2 path.

---

### C6. Division by zero, NaN/Inf in `Pow`, and negative `Sqrt` silently return `0` in rate evaluation

**Bug.** `propensity.rs:86-101` (Div, Pow, Mod); `propensity.rs:117-124` (Sqrt of negative → 0; NaN absorbed → 0); `resolved_expr.rs:261,444`. `EvalStats` records but no one reads (see H5).

**Decision: fail-fast as default; gate the silent path behind `--allow-degenerate-rates`.**

The audit notes "this is the single most insidious class of bug in the inference math because it can pass every unit test and still corrupt every scenario projection." Concur. The spatial example in the audit (small patch empties at runtime, force-of-infection silently zeros, user reads "outbreak ended in patch X") is a textbook case where silent degeneracy looks like correct dynamics but isn't.

Two design questions worth deciding now:

**(a) Should `--allow-degenerate-rates` exist at all?** Yes, narrowly. There is one legitimate case: a model where `beta * I / N` with `N=0` is *defined* to be 0 because the patch has no population and no force-of-infection makes physical sense. The user should opt into that interpretation explicitly; the default is "this is a numerical accident, halt." When the flag is set, the runtime still increments the `EvalStats` counter (now actually read — see H5), so the user gets a count of how often the silent path fired.

**(b) Where does the error fire?** At `eval_expr` in `propensity.rs`. The error has to carry enough context to be debugged — `SimError::NumericalCollapse { expr_kind: DivByZero | PowNanInf | SqrtNegative, source_location: Option<SourceLoc>, t: f64 }`. The OCaml IR already carries source locations on rate expressions; thread them through to the Rust runtime so the diagnostic can point at the offending line of DSL.

Implementation:
1. New `SimError::NumericalCollapse` variant with the kind/location/time payload.
2. `eval_expr` returns the error instead of `0` for the three cases.
3. `--allow-degenerate-rates` flag on `simulate`, `pfilter`, `if2`, `fit run` subcommands; threads through to a `propensity::EvalConfig { allow_degenerate: bool }`.
4. Counters (H5) surface the count regardless of flag state.

**Test:** `tests/numerical_collapse.rs::div_by_zero_errors_not_silences` and `::pow_nan_errors`. Plus a positive test with `--allow-degenerate-rates` confirming the silent path still works when explicitly requested.

---

### C7. PGAS post-burn-in `n_divergent` and `n_max_treedepth` not surfaced

**Bug.** Declared at `pgas.rs:1391-1392`, accumulated when `rung == 0` at `:1526-1532`, logged at burn-in boundary `:1717-1739`. `PGASResult` constructor at `:1807-1812` does not include them. The `DiagnosticKind::DivergentTransitions` and `::MaxTreeDepthHits` variants exist in `inference/diagnostic.rs:72-81` with full render/hint plumbing, constructed nowhere (see H4).

**Decision: surface in `PGASResult` *and* wire the diagnostic variants. Same root cause as H4; pair the fix.**

This is mechanical but high-impact. A user signing off on a PGAS posterior with 200 unsurfaced post-burn-in divergences is exactly the failure mode the diagnostics module exists to prevent. The infrastructure for the surface is already built — the missing piece is the assignment.

Implementation:
1. Add to `PGASResult`:
   - `pub n_divergent: usize`
   - `pub n_max_treedepth: usize`
   - `pub swap_acceptance_rates: Vec<f64>` (per-rung, addresses M18)
2. Populate at construction (`pgas.rs:1807`).
3. CLI `cmd_fit_run_v2` emits these into `diagnostics.json` alongside existing fields.
4. Wire `DiagnosticKind::DivergentTransitions` and `::MaxTreeDepthHits` constructors at the gating module (probably `cli/src/fit/gating.rs`); thresholds: any divergence post-burn-in fires `DivergentTransitions`; `n_max_treedepth > 0.05 * n_post_burn_in` fires `MaxTreeDepthHits` (matching Stan convention).
5. Wire `LowSwapRate` from `swap_acceptance_rates` (rung-pair acceptance < 0.10).

**Test:** `tests/pgas_diagnostics_surfaced.rs` — run PGAS with deliberately tight `max_treedepth` to force tree-depth hits; assert they appear in `PGASResult` and trigger `MaxTreeDepthHits` in the diagnostic stream.

---

### C8. `ir/schema.json` and `ir/VERSION` are inert — no single source of truth for the IR contract

**Bug.** `ir/schema.json` (822 lines) is referenced by no source file. `ir/VERSION` (literal `0.3`) is read by nothing. CLAUDE.md says "the IR is the contract"; in reality both sides are hand-mirrored. Schema drift manifests as `serde::Error` at golden-test time (best case) or wrong-but-parseable simulation (worst case — see C3, H6, H7, H14, M12).

**Vince's question: "what should we do there, what is cleanest and best?"**

There are three real architectural answers; the cleanest depends on how much codegen tooling we want to take on.

**Option A — JSON Schema as authoritative, codegen both sides.** `schema.json` becomes the source of truth. Rust types generated via `schemars` (or `quicktype`); OCaml types generated via `atdgen` (with a small `atd` schema mirroring the JSON Schema, since `atdgen` doesn't consume JSON Schema directly — there is no clean direct path on the OCaml side). Pros: one file, one contract, mechanically enforced. Cons: OCaml side needs hand-maintained `atd` that mirrors the JSON Schema, defeating most of the benefit; Rust derive macros for `schemars` produce schemas with structural details (e.g., serde tagging) that are awkward to mirror in JSON Schema. In practice this option is more friction than it appears; most teams that try it end up with schema-as-documentation anyway.

**Option B — OCaml IR as authoritative, schema.json generated, version handshake enforced.** `ocaml/lib/ir/ir.ml` is the source of truth (already the most expressive — has sums, dimcheck, the compiler builds against it). Add `make schema` target that generates `ir/schema.json` from OCaml. Add `IR_VERSION` constant in OCaml; emit in IR envelope; Rust `include_str!`s `ir/VERSION` and rejects mismatched envelopes with `IrVersionMismatch { expected, found }`. Add a CI step that runs `make schema && git diff --exit-code ir/schema.json` so a Rust- or OCaml-side change that desyncs the schema fails CI. Pros: low tooling burden, plays to existing strengths, makes schema.json honest documentation again. Cons: Rust types still hand-maintained — drift between Rust and OCaml is not mechanically caught, only flagged at golden-test time.

**Option C — OCaml IR authoritative, Rust types generated from OCaml.** Like B but go further: generate Rust types from OCaml IR via a small ocaml-to-rust codegen pass (or more pragmatically: emit `quicktype`-compatible JSON Schema from OCaml, then `quicktype --rust` generates Rust). Pros: actually closes the drift loop. Cons: build dependency on a Node.js tool (`quicktype`) for production code; the generated Rust loses idiomatic touches (custom `serde` impls, derived traits beyond the basics) that the hand-written types currently have.

**Decision: Option B for the alpha, with the explicit door open to C post-alpha.**

The reason is leverage. Option B:
- Closes the immediate "documentation lies" problem (schema.json becomes generated, so it can't lie).
- Closes the version-handshake problem (envelope + `include_str!` is ~30 lines on each side).
- Adds CI enforcement that catches the most common drift mode (Rust changes shape, OCaml doesn't).
- Lands in days, not weeks.

Option C is the right long-term answer but it requires a build-system commitment (Node.js in CI, generated-code review etiquette, custom-serde-impl preservation strategy) that isn't worth taking on under alpha pressure. We should ship B, see how often the CI golden-diff actually catches drift in real use, and revisit C in 2–3 months when we have data.

Concrete plan for Option B:

1. **OCaml side.**
   - `ocaml/lib/ir/version.ml`: `let ir_version = "0.4"` (bump from 0.3 — every alpha change is breaking by default per CLAUDE.md "backwards compatibility is a non-goal").
   - `ocaml/lib/ir/serde.ml`: emit envelope `{ "ir_version": "0.4", "validated_by": "ocaml-compiler-vX", "model": <existing> }`.
   - `ocaml/bin/dump_schema.ml`: generates `ir/schema.json` from the OCaml types via inspection of the serializer; wired into `make schema`.
   - `ocaml/lib/ir/validate.ml`: documents which invariants are OCaml-only; emits the `validated_by` marker.
2. **Rust side.**
   - `rust/crates/ir/src/version.rs`: `pub const IR_VERSION: &str = include_str!("../../../../ir/VERSION");`
   - `IrEnvelope { ir_version: String, validated_by: Option<String>, model: Model }` struct; deserialize entry points (`from_str`, `from_reader`) parse the envelope and check `ir_version == IR_VERSION.trim()` before unwrapping; mismatch → `IrError::VersionMismatch { expected, found }`.
   - `validate.rs`: if `validated_by.is_some()`, skip the OCaml-mirrored structural checks; otherwise re-run them (covers the hand-edited-IR-JSON case from H14).
3. **Build.**
   - `make schema` generates `ir/schema.json` from OCaml.
   - CI step: `make schema && git diff --exit-code ir/schema.json` (fails if generated schema has drifted from committed copy).
   - CI step: round-trip every golden IR through both sides; fail if either rejects.
4. **VERSION file.**
   - `ir/VERSION` becomes `0.4`. Bumped on every breaking IR change (already required per CLAUDE.md atomic-commit rule for IR schema changes, just newly enforced).

This is the minimum that makes the contract real. It does not solve hand-mirrored Rust types — that's deferred to Option C — but it does mean *drift gets caught at CI* rather than at customer site.

**Bonus from this fix:** unblocks several Highs and Mediums that share the root cause:
- H6 (`param_kind: Option<String>`) becomes a typed enum with the same fix on both sides; envelope version bump captures the breaking change.
- H7 (`Interpolated.method` typed in Rust, raw string in OCaml) — same shape, same fix.
- M12 (`ModelStructure` deserialized + never read) — schema-generation pass surfaces unused IR fields by inspection; deletion lands in same commit as 0.3 → 0.4 bump.
- M13 (`SimulationConfig.{rng_seed, time_semantics}` deserialized + never read) — same.

---

## 2. High — per-finding plan

### H1. NUTS outer-tree combine uses `n_prime/(n_valid + n_prime)` with slice-indicator counts

**Bug.** `nuts.rs:244-250` outer combine; slice indicator at `:323`. The leaf uses Algorithm 3-style slice indicators; the outer combine uses Algorithm 6-style ratio. Hybrid; possibly correct but undocumented.

**Decision: change the outer combine to match Alg 6 exactly (audit's option a).** Rationale: Hoffman & Gelman 2014 is the canonical reference cited in the code's own comment at `:348`. Either we follow it or we cite a different paper; "we deviate from H&G in a way we haven't proven preserves the target" is not acceptable for a method that drives polio posteriors. The change is one line: `let accept_prob = (n_prime as f64 / n_valid as f64).min(1.0);` Add an inline citation to H&G Alg 6 line 4.

**Test:** `tests/nuts_invariance.rs::stationary_under_alg6_combine` — short Gaussian target, assert empirical mean and variance match analytic within MCMC-SE bounds.

---

### H2. Discretized-normal observation likelihood uses A&S 7.1.26 erf (1.5e-7 abs error) in the tails

**Bug.** `obs_loglik.rs:166-179` (`normal_cdf`), `:213-226`. `Φ(z_hi) − Φ(z_lo)` for tail observations dominated by approximation noise. Polio AFP surveillance is exactly the regime this hits.

**Decision: switch to `libm::erfc`-based formula (audit's recommendation).** This is a half-day fix with measurable downstream impact. `libm::erfc` already in the dependency tree (used elsewhere); the formula `Φ(z) = 0.5 * erfc(-z / √2)` and the interval-difference variant that avoids subtracting near-1 values are both standard. Pair with a test against `scipy.stats.norm.cdf` at z=±10 (current implementation: garbage; libm: agrees to ~1e-15).

**Test:** `tests/obs_loglik_tails.rs::discretized_normal_matches_scipy_at_z10`.

---

### H3. Catastrophic cancellation `1.0 - p_total` in PGAS when `rate·dt` is large

**Bug.** `pgas.rs:301-302, :319`; gradient at `pgas_grad.rs:165, :171`. For `total_rate * dt ≫ 1`, `1 - exp(-large)` collapses to floating-point noise; gradient `(n-k) / (1-p)` blows up. Comments admit "tolerated divergences."

**Decision: carry `(p, q)` as a pair throughout (audit's recommendation).** The `binom_logpmf(k, n, p, q)` overload accepting both is the right primitive. Gradient form `k/p − (n-k)/q` has no subtraction. Half-day fix; same grade of work as H2.

Note that this also subsumes M6 (`binom_logpmf` floor near `p ≈ 1`) — once we carry `q` directly, the floor can be removed.

**Test:** `tests/binom_logpmf_extreme_p.rs::matches_scipy_at_logit_minus10_to_10`.

---

### H4. ~14 of 22 `DiagnosticKind` variants never constructed

**Bug.** Listed in audit; underlying quantities (ESS in `particle_filter.rs:285`, trajectory renewal in `pgas.rs:1724`, swap rates in `pgas.rs:1745-1750`) are computed but no constructor wires them.

**Decision: wire every variant or delete it. No middle ground.**

Per the process discussion in §0, an unwired diagnostic variant is a TODO that decayed into dead code. Two paths per variant:

1. If we have a real detection threshold and a real call site, wire it. Most of these are wireable today: `LowESS` at the end of any PF pass; `ParamNearBound` after each MCMC chain finalises; `LowSwapRate` from C7's surfaced acceptance rates; `GammaDensityDisabled` when the gamma-density toggle is off; `DivergentTransitions` and `MaxTreeDepthHits` from C7's surfaced fields.
2. If the variant was speculative and we don't have a real detection logic in mind (e.g., `ResumeConfigMismatch`, `ResumeParamMissing`, `AutoRwSdNoConsensus` — all of which depend on infrastructure we haven't built), delete the variant.

The work is in choosing thresholds. Defensible defaults from literature:
- `LowESS`: `ess.last() < 0.10 * n_particles` (standard PF heuristic)
- `LowESSAtMLE`: `ess.last() < 0.05 * n_particles` at MLE-eval pass
- `ParamNearBound`: posterior 5th or 95th percentile within 1% of bound
- `MaxTreeDepthHits`: `n_max_treedepth > 0.05 * n_post_burn_in` (Stan)
- `DivergentTransitions`: any post-burn-in (Stan)
- `LowSwapRate`: rung-pair acceptance < 0.10 over the last 100 swaps

Each gets a test that triggers the threshold and asserts the diagnostic appears in the output stream.

---

### H5. `EvalStats` degenerate-evaluation counters incremented everywhere, read nowhere

**Bug.** `eval_stats.rs:16-67` defines counters; `rng.rs:58, :68, :115`, `resolved_expr.rs:262, :269, :297` increment them. No reader.

**Decision: surface in CLI per audit, AND make the surface meaningful by tying to C6.**

After C6 lands, the most common case (silent div-by-zero) becomes an error rather than a counter increment. The remaining counter increments (binomial fallback, neg-binomial → poisson fallback) are legitimate "this happened, you should know about it" signals that don't warrant halting. Surface them.

In `cmd_simulate`, `cmd_pfilter`, `cmd_if2`, `cmd_fit_run_v2`: snapshot `EvalStats::snapshot()` at start, diff at end, emit `eval_stats.json` to the run dir whenever any counter incremented. ~10 lines per CLI entry point.

If the C6 fix is done first, the counter set may shrink; reflect that in the snapshot struct.

---

### H6. `param_kind` is `Option<String>` on both sides despite finite domain

**Bug.** `parameter.rs:127-130`, `ir.ml:270`, `expander.ml:1628`. Valid kinds {rate, probability, positive, count, real}; OCaml has a proper sum (`ast.ml:70`); compiler downgrades to string for IR; Rust pattern-matches string literals everywhere. Same defect on `table.cell_kind` (gh#32).

**Decision: typed enum on both sides.** Bundle with C8's IR version bump (0.3 → 0.4) so we don't pay two breaking-change taxes. `enum ParamKind { Rate, Probability, Positive, Count, Real }` with `#[serde(rename_all = "snake_case")]` in `parameter.rs`; mirror in `ir.ml`; drop the `param_kind_to_string` adapter. Add `UnknownParamKind` validation on the deserialize boundary.

---

### H7. Schema drift: `Interpolated.method` typed enum in Rust, raw string in OCaml

**Bug.** `time_func.rs:19-31` (Rust enum); `ir.ml:85` (OCaml string). Typo `"cubic"` passes OCaml validation, crashes Rust at deserialize.

**Decision: same shape as H6.** Mirror `interp_method = Linear | Constant | Spline` in `ir.ml`/`serde.ml`. Validate at the OCaml deserialize boundary. Sweep for the same pattern in `time_semantics`, `output.format`, anywhere else; bundle with H6 / C8 / 0.4 bump.

---

### H8. CSMC ancestor-sampling categorical uses post-resample predecessor states

**Bug.** `pgas.rs:826-859` (`prev_counts` set from `counts` after resampling shuffle; reference slot corrected at line 859); `:891-902` ancestor-sampling weights pair "transition density from slot j's post-resample state to ref flows" with "particle j's slot identity."

**Decision: cache pre-resample state per audit.** This is a real bug — ancestor sampling is supposed to categoricalise over the *pre-step* particle ensemble. The fix is clean: `let prev_counts_for_ancestor = counts.clone()` immediately on entering each substep (before the resampling block at 802-825); use it (with the reference-slot correction) at line 891-902. The existing `prev_counts` save can stay for the rest of the loop.

**Test:** `tests/pgas_ancestor_sampling.rs::pre_resample_state_used` — seed-controlled run with deliberately heterogeneous patch prevalences; assert ancestor selection matches the pre-resample ensemble's transition density.

---

### H9. Two diverged `systematic_resample` implementations

**Bug.** `resampling.rs:18-40` vs `correlated_pf.rs:421-442` (`sorted_systematic_resample`). Same cumsum loop and weight normalisation; only difference is the uniform source.

**Decision: one canonical implementation; delete the other (audit's recommendation).** `systematic_resample_with_u(log_weights, u) -> Vec<usize>`; `systematic_resample(log_weights, rng)` becomes a one-line wrapper; CPM passes `base_uniform` to the `_with_u` form. Delete `sorted_systematic_resample`. Pure cleanup; no behavioural change at fixed seed.

---

### H10. PGAS ancestor-sampling categorical bypasses canonical `normalize_log_weights`

**Bug.** `pgas.rs:1017-1034` (`sample_categorical_log`); called from `:909` and `:960`. Re-implements softmax + categorical inverse-CDF; degenerate-case fallback differs from the canonical helper.

**Decision: extract `categorical_log` to `inference/resampling.rs`; replace both call sites.** The fix is the audit's: `categorical_log(log_weights, &mut rng) -> Option<usize>`, document the degenerate-policy contract in one place. Pair with H9 (same module, same testing infrastructure, same review attention).

---

### H11. Inference indices have no newtypes — `ParticleIdx`, `CompartmentIdx`, `ObsIdx`, `StratumIdx` all bare `usize`

**Bug.** Indices swirl as bare `usize` through `pgas.rs`, `state.rs`, `inference/types.rs`. Compile-time cost of `pub struct ParticleIdx(usize);` is zero; nothing prevents `pgas.rs:1054` indexing `initial_counts[stratum_idx]` instead of `[compartment_idx]`.

**Decision: incremental rollout, starting with the highest-leverage indices.** Defer the full sweep to post-alpha — newtype churn touches every call site and risks introducing bugs of its own under time pressure. For alpha, do the two indices most likely to be confused in the inference math:

1. `CompartmentIdx` (used in PGAS, PF, observation models, init).
2. `ParticleIdx` (used everywhere; confusion with `ChainIdx` and `RungIdx` is the most subtle hazard).

Add the newtypes to `sim/inference/types.rs`. Migrate one crate at a time; the type system flags every site that needs updating, so the work parallelises cleanly. `ObsIdx`, `TransitionIdx`, `StratumIdx` and the value newtypes (`LogWeight`, `LogDensity`, `Probability`, `Rate`) defer to post-alpha unless time allows.

---

### H12. `--record-prequential` and `--record-ancestry` silently no-op outside PFilter stage

**Bug.** `args/mod.rs:465-472` only `requires = "stage"`; consumed only in `fit/mod.rs:1192-1248`. `camdl fit run config.toml --stage scout --record-prequential` silently drops the flag.

**Decision: runtime check, hard error.** After the stage type is resolved in `cmd_fit_run_v2`: if `a.record_prequential || a.record_ancestry` and the stage is not PFilter, exit with `error: --record-prequential requires --stage <pfilter-stage>` listing valid PFilter stages from the current config (so the user knows what to pass). 10 lines.

---

### H13. `--parallel` / `CAMDL_PARALLEL` ignored by `camdl pfilter`

**Bug.** Declared on `InferenceCore` at `args/mod.rs:78-79`; embedded into `PfilterArgs` at `:773`; consumed only in `if2.rs:100,282,372` and `profile.rs:333,849-851`. `pfilter.rs` has no `parallel` or `rayon` reference.

**Decision: wire it (audit's recommendation).** In `pfilter.rs::cmd_pfilter`, build a rayon pool from `a.inference.parallel` (matching the `if2.rs:369-374` idiom) before the replicate loop at line 209; replace the loop body's `iter()` with `into_par_iter()`. Half-day; user-facing impact obvious.

---

### H14. Validation logic duplicated OCaml↔Rust with subtly different surfaces

**Bug.** `validate.ml` (130 LOC) vs `validate.rs` (337 LOC); dim-check OCaml-only (`dimcheck.ml`, 940 LOC). Hand-edited IR JSON bypasses dim-check entirely.

**Decision: bundle into the C8 envelope work.** The `validated_by` marker in the IR envelope is the natural place to encode "which validations have already run." After C8:

- OCaml emits `validated_by: "ocaml-compiler-vX.Y"`.
- Rust's `validate.rs` checks the marker; if present, skips OCaml-mirrored structural checks; if absent, re-runs them.
- A documented list of "OCaml-only invariants" (dim-check chief among them) captures what survives only on the OCaml path.

Long-term, `dimcheck.ml` should probably move to a shared crate (the OCaml-Rust bridge here is awkward). Defer to post-alpha.

---

## 3. Medium — grouped plan

The 21 Mediums split naturally into "fix during alpha sprint as bundled cleanups" and "defer to post-alpha." Per-item rationale below.

### Medium — fix during alpha sprint

These are short, high-signal, and most are subsumed by Critical/High fixes:

- **M1.** `normalize_log_weights` falls back to uniform on all `-inf`. Fix: `swarm.ess() > 0` check in `bootstrap_filter` and `csmc_as`; return `-inf` increment + `n_collapsed` flag. *Ties to H4 — wire `LowESS` here.*
- **M2.** IF2 `cooling_target_iters` uses `n_obs` instead of `(1 + n_obs)`. 1-line fix.
- **M3.** `transformed_sd` delta-method singular at log lower bound. Fix: cap the perturbation or use `rw_sd` directly in transformed space. *Ties to H4 — `AutoRwSd` diagnostic should fire when this hits.*
- **M4.** PMMH `acceptance_rate` divides by total `n_steps`, not post-burn-in. 2-line fix.
- **M6.** `binom_logpmf` near `p ≈ 1` floor. *Subsumed by H3.*
- **M7.** `ESS` inlines softmax. Fix: route through `log_sum_exp` + `normalize_log_weights`. Pair with H10 cleanup.
- **M9.** `Expr` derives `PartialEq` over `f64`. Fix: hand-write bitwise equality on `value.to_bits()`. Half-hour; one-line review-bait gone.
- **M10.** `Trajectory::default()` produces empty trajectory. Fix: delete the `Default` impl; force `Trajectory::with_initial(...)`. CLAUDE.md "delete dead code" applies directly.
- **M12.** `ModelStructure` deserialized + never read. *Subsumed by C8 — delete in 0.3 → 0.4 schema bump commit.*
- **M13.** `SimulationConfig.{rng_seed, time_semantics}` deserialized + never read. *Subsumed by C8.*
- **M14.** Missing compartments in `init {}` default to 0 silently. Fix: `validate.ml` enumerates missing compartments → diagnostic E411. Same shape as C4 — silent default → hard error.
- **M15.** Default `--seed 1` indistinguishable from user-supplied `--seed 1`. Fix: `Option<u64>`; if absent, draw + log + persist. Half-hour; surface in run-metadata.
- **M17.** `Cond` predicate `pred > 0.0` with no float-equality safety on `Time`. Fix: dim-check rule in `dimcheck.ml` against `Eq`/`Neq` on `Time`.
- **M18.** PGAS swap-acceptance rates only logged to stderr. *Subsumed by C7.*

### Medium — defer to post-alpha

These need real engineering work that doesn't affect the skeptic's first-pass review:

- **M5.** Correlated PF sort key collapses dissimilar particles. Choppala 2016 Hilbert-curve sort is non-trivial; CPM is experimental anyway (`pmmh.rs` is gated). Defer.
- **M8.** Cholesky-times-z inlined in PMMH instead of `nuts.rs::matvec_lower`. Cleanup, low priority. Defer.
- **M11.** No `.mli` files in `ocaml/lib/`. Substantial refactor; alpha doesn't depend on it. Defer.
- **M16.** `--dt 1.0` default applied silently across backends. Needs design (what's the right per-backend default? per-time-unit default?). Defer to a focused proposal.
- **M19.** No OCaml-side `autodiff.ml` finite-difference test. Worth doing but doesn't affect alpha-user perception. Schedule for first post-alpha sprint.
- **M20.** No PGAS recovery test. Same — high-value, not surface-visible. Schedule for first post-alpha sprint.
- **M21.** Conservation tests don't cover silent-clamp path. *Subsumed by C5's negative-test.*

---

## 4. Sequencing

Ordered for parallelism and unblocking:

**Sprint 1 — visible rot (3 days).** C2, M9, M10, M12, M13, H12, H13. All small. Removes the "obvious" hits a skeptical Claude Code review would surface in 30 minutes.

**Sprint 2 — diagnostic surfacing (3 days).** C7 + H4 + H5 paired. Same root cause; same module; same testing pattern. After this sprint, divergences, low ESS, near-bound parameters, and degenerate-eval counts all reach the user.

**Sprint 3 — silent → loud (3 days).** C3 (BALANCE capability), C4 (no prior, no run), C5 (negative-count error), C6 (numerical-collapse error). All convert silent fallbacks to errors. Each gets a denial test.

**Sprint 4 — IR contract (4 days).** C8 (envelope + version handshake + generated schema + CI golden-diff). H6 (param_kind enum). H7 (interp_method enum + sweep). M14 (missing compartments → E411). Bundle as the 0.3 → 0.4 IR bump.

**Sprint 5 — inference correctness (5 days).** C1 (preflight gate day 1; full obs-likelihood gradient days 2–5). H1 (NUTS Alg 6). H2 (libm::erfc). H3 (q-pair throughout). H8 (cache pre-resample state). Each ships with a paired regression test.

**Sprint 6 — DRY cleanup + selective newtypes (3 days).** H9, H10 (one canonical resampling/categorical). H11 (CompartmentIdx + ParticleIdx only). M1, M2, M3, M4, M7, M15, M17, M18 — bundled small fixes.

**Total: ~3 weeks of focused work.** Sprints 1–3 are the alpha-skeptic-defence floor; ship those even if 4–6 slip. Sprints 4–6 are the correctness floor; if any one slips past alpha, surface it explicitly in the alpha announcement so users know what's still pending.

---

## 5. Process changes (land alongside Sprint 1)

These are CI/policy changes that prevent the audit's findings from re-occurring. None of them require code changes in the inference math; all should land before Sprint 1 closes so the rest of the work benefits from them.

1. **`cargo clippy -- -D dead_code` at workspace root** in CI. Allow per-test (`#[cfg(test)]` modules) but not per-module. Catches H4-class rot at PR time.
2. **`make audit-greps` target.** Greps `inference/`, `ir/`, `propensity.rs`, `obs_loglik.rs` for `currently`, `for now`, `typically`, `silently`, `TODO`, `FIXME`, `XXX`, `unwrap_or(0)`, `clamp`, `.min(1).max(0)`. Fails CI on net-new occurrences. Each existing occurrence gets either a fix (Sprint 5) or a documented exception in `docs/dev/known-noise.md` (delete-on-sight when fixed).
3. **Denial-test policy.** Document in `CLAUDE.md`: every `SimError` variant, every `DiagnosticKind` variant, every CLI error path requires a test that triggers it. Track in `INFLIGHT.md`. CI gate: variants without a denial test produce a clippy warning (custom lint or convention).
4. **"Wire it now" PR rule.** Document in `CLAUDE.md`: no new `DiagnosticKind`, `SimError`, or stats counter merged without (a) at least one production call site and (b) one denial test. Reviewers reject otherwise.
5. **IR change checklist.** Document in `CLAUDE.md`: every IR schema change bumps `ir/VERSION`, regenerates `ir/schema.json` via `make schema`, updates both language types in the same atomic commit. The CI golden-diff (Sprint 4) enforces the schema regen.

---

## 6. Root-cause sweep — supplement

Vince's request: per-finding 5-whys, looking for double-wins where a single
change addresses both the symptom and the architectural root cause. Below are
the cases where the sweep surfaced something the per-issue plan missed. For
findings whose plan is already root-cause-aware (e.g., C8 itself *is* the
root-cause fix), no entry below — the §1–3 plan stands.

### Cross-cutting findings discovered during the sweep

These are patterns the audit found in one place that the sweep showed are
present in *several* places. The per-issue plan would fix one occurrence each;
flagging here so the sprint actually fixes the class.

**S1. `eval_expr` is `Result`-typed but its match arms still return `Ok(0.0)` for failure cases.**

`propensity.rs:39` declares `pub fn eval_expr(...) -> Result<f64, SimError>`. The signature is correct. But inside the match arms, the audit's C6 cases return `Ok(0.0)` instead of `Err(...)`:

- `propensity.rs:97-104` (Div by zero → `Ok(0.0)`)
- `propensity.rs:105-111` (Pow → NaN/Inf → `Ok(0.0)`)
- `propensity.rs:112` (Mod by zero → `Ok(0.0)` inline)
- `propensity.rs:130` (Sqrt of negative → `Ok(0.0)`)
- `propensity.rs:138-143` (UnOp NaN → `Ok(0.0)`)

This is worse than the audit framed it. A reviewer skimming the *signature*
sees proper error handling; a reviewer reading the *bodies* finds silent zeros
wrapped in `Ok(...)`. The C6 fix as written (return
`SimError::NumericalCollapse`) is correct, but the root cause is
"Result-ification was done at the boundary, not inside the match." Going
forward, any `eval_expr`-shaped function should have a
`clippy::result_unit_err`-style check that flags `Ok(<sentinel>)` returns from
arms that *should* be errors.

**Double-win:** C6's fix should also delete the `eval_propensities` swallowing
pattern at `propensity.rs:212-252` — those four `.unwrap_or(0.0)` calls discard
`eval_expr`'s errors back to the same silent-zero behaviour the C6 fix is
removing. Without this companion change, C6's fix doesn't actually reach the
call sites that matter (rate evaluation goes through `eval_propensities`, which
silently masks the error).

**S2. The `log::warn! + silent clamp` anti-pattern is in 6 places, not 1 (C5).**

C5 was framed as a chain-binomial clamp. The pattern is universal:

- `chain_binomial.rs:442` — balance compartment went negative, warn + continue
- `tau_leap.rs:215` — clamped negative compartments at t={t}
- `gillespie.rs:249` — clamped negative integer compartments at t={t}
- `intervention.rs:253` — event adding negative count, warn + continue
- `propensity.rs:108` (Pow → NaN → warn + 0)
- `propensity.rs:139` (UnOp NaN → warn + 0)

Every one of these is "we detected a bad state, told the log, continued anyway." The combination is a recognisable anti-pattern: if it's worth logging, it's worth surfacing in a typed way; if it's not worth surfacing, it's not worth logging. Treat this as a single C5+C6-shaped fix that touches all 6 sites with typed errors and the same routing rule (forward-sim halts; inference catches and converts to `−Inf` + counter increment). Add a clippy-style audit grep to `make audit-greps`: any new `log::warn!` line in `sim/` triggers review for "should this be a typed error?"

**S3. `Option<String>` for finite-domain fields exists in 5 IR locations, not 1 (H6).**

`grep "Option<String>" rust/crates/ir/src/` shows:

- `transition.rs:13` — `origin_kind: Option<String>` (likely small enum: `Compartment | External | ...`)
- `transition.rs:14-15` — `source_compartment, dest_compartment: Option<String>` (these are compartment refs — should be `Option<CompartmentId>` newtype, not raw string)
- `table.rs:55` — `cell_kind: Option<String>` (gh#32 already noted, same finite-domain shape as `param_kind`)
- `parameter.rs:130` — `param_kind: Option<String>` (the H6 case)
- `intervention.rs:68` — `base_name: Option<String>` (genuine free string; leave as-is)
- `model.rs:136,138` — `description, origin: Option<String>` (genuine free strings; leave as-is)

The H6 fix as written closes one site. The double-win is auditing all `Option<String>` IR fields for "is this a finite domain or a reference?" in the same C8 envelope-bump commit. The non-genuine-string fields (`origin_kind`, `source_compartment`, `dest_compartment`, `cell_kind`, `param_kind`) should all become typed enums or newtypes. This is the same friction-reducing case for both directions: typo-resistant on the OCaml side, exhaustive on the Rust side.

**S4. The "compute-the-quantity-then-don't-surface-it" pattern is the root cause behind C7, H4, H5, and M18.**

Confirmed by inspection. Specific examples beyond the audit:

- `pgas.rs:949-956` computes `n_degenerate` and the threshold `pct > 10.0` is already in code; emits `log::warn!`; doesn't construct `DiagnosticKind::DegenerateAncestorSampling`. Four lines from a wired diagnostic.
- `obs_model.rs:125-140` uses `static AtomicBool` to warn-once on non-integer observed values; should construct `DiagnosticKind::ObsModelMismatch`.
- `linalg.rs:18` (Cholesky `log::warn!`) — should construct an `LinalgFailure` diagnostic or propagate the error.

The audit's H4 fix lists the unwired variants but understates how many *detection conditions are already implemented* — they just emit `log::warn!` instead of constructing the diagnostic. Single-pattern fix:

```rust
// Before:
if pct > 10.0 {
    log::warn!("CSMC-AS: {}/{} ({:.0}%) degenerate", n_degenerate, n_substeps, pct);
}

// After:
if pct > 10.0 {
    diagnostics.push(DiagnosticKind::DegenerateAncestorSampling { n_degenerate, n_substeps });
}
```

Sprint 2 (diagnostic surfacing) should grep for `log::warn!` in `inference/` and convert each to a typed diagnostic where one fits. Adds maybe 4 hours of work to Sprint 2 and probably wires another 3-4 of H4's unwired variants for free.

### Per-finding 5-whys

For findings where the root cause analysis surfaces something beyond what's in §1–3.

---

**C1 (PGAS gradient drops obs-likelihood derivatives).**

5 whys:
1. Why is the gradient missing obs terms? Comment says "Currently zero because σ² is typically a constant (not estimated)."
2. Why was that assumption baked in? The PGAS implementation was originally designed for the rate-parameter case; obs-param estimation was added later.
3. Why didn't the later work update the gradient? Because `complete_data_loglik_grad` and the obs-param-estimation feature are in different modules; no signature link forced the update.
4. Why is there no signature link? `EstimatedParam` is a `usize` index; `complete_data_loglik_grad` takes a parameter vector and a `rate_grads_for_run` set. There is no type-level relationship between "this index is estimated" and "this index has a gradient covered."
5. Root cause: **no compile-time contract between "what's in the estimated set" and "what gradient terms must exist."**

**Double-win:** Beyond the per-issue fix, define a `RequiresGradient` trait or marker that any contributor to log-posterior must implement. Construct PGAS with a check: for each `EstimatedParam`, assert that *all* log-posterior contributors covering that parameter have a gradient. Fails to compile (or fails fast at construction) when a future contributor is added without its gradient. Same shape as the C1 preflight gate but lifted into the type system; the gate becomes the runtime backstop, the trait check becomes the build-time guarantee.

---

**C2 (IF2 returns wrong MLE).**

5 whys:
1. Why select on `if2_perturbed_loglik`? Because the clean loglik isn't computed at `run_if2` return time.
2. Why isn't it computed? Because the clean PF eval is a separate, slower pass run by the caller.
3. Why does `run_if2` return `mle` despite not having the right info? Because `IF2Result` was designed as "the answer" and `mle` looked like a natural field on it.
4. Why does the result type include a derived field? Because at the time, there was only one consumer and putting the selection logic next to the run logic was natural.
5. Root cause: **result types include "derived/selected" fields beyond the raw record of what happened.** This pattern probably exists in `PGASResult`, `PMMHResult`, etc. — anywhere a "best" or "MLE" or "estimate" field is on a result type whose constructor doesn't have access to the data needed to make the selection.

**Double-win:** Audit all inference result types for derived fields. Strip selection logic into separate functions the caller invokes after re-evaluation. Possibly: introduce `IF2Trace` / `PGASTrace` (raw record) vs `IF2Estimate` / `PGASEstimate` (caller-constructed after re-eval). Cleaner type design and removes the C2-shape bug class entirely.

---

**C3 (balance silently dropped on backends).**

5 whys:
1. Why does balance silently drop? Backends don't apply it.
2. Why don't they apply it? They don't check for it.
3. Why don't they check? `balance` is in the IR but not in `Capabilities`.
4. Why isn't it in Capabilities? When `balance` was added, no one updated the capability check.
5. Root cause: **adding an IR field with semantic effect doesn't require declaring its capability.** No structural rule says "if you add an `Option<T>` to `Model` whose `Some` value affects dynamics, you must declare a capability."

**Double-win:** Add a doc-comment convention and a unit test that enumerates `Model`'s fields and asserts each is either (a) declared in `Capabilities`, (b) explicitly marked `#[doc = "always-on"]`, or (c) explicitly marked `#[doc = "config-only, no runtime effect"]`. Catches the class of bug rather than the instance. Pair with a `make capability-coverage` audit step.

---

**C4 (no prior, no run).**

5 whys:
1. Why does missing prior fall back to Flat? `resolve_prior` returns `Prior` not `Result`.
2. Why is it non-fallible? CLI didn't want to plumb errors through.
3. Why does the CLI not validate fit.toml first? Because fit.toml validation is partial — it checks structure but not cross-section invariants like "every estimated param has a prior."
4. Why is validation partial? Because the validator was built incrementally; cross-section checks weren't added.
5. Root cause: **fit.toml lacks a strict-mode validator that runs before any inference setup.** Same root cause likely behind M14 (missing compartments → silent 0), M15 (default seed indistinguishable), and other "config silently underspecified" findings.

**Double-win:** Add `cmd_fit_validate` that runs every cross-section check fit.toml should pass and exits non-zero on any failure. Make `cmd_fit_run_v2` invoke it as the first step (so users can also run it standalone, e.g., for CI). Bundles C4, M14, M15 (and similar) under one validator with one denial-test pattern.

---

**C6 (silent div-by-zero).** See **S1** above — the Result-ification was done at the boundary but not inside the match arms. The double-win is fixing both `eval_expr` *and* `eval_propensities`'s `.unwrap_or(0.0)` swallowing in the same pass.

---

**C7 + H4 + H5 + M18 (compute-and-don't-surface).** See **S4** above. The double-win is converting `log::warn!` in `inference/` to typed diagnostics in the same pass; wires several H4 variants for free.

---

**C8 (IR contract).** Already a root-cause fix as proposed.

---

**H3 (catastrophic cancellation).**

5 whys:
1. Why does PGAS use `1.0 - exp(-x)`? Math-textbook form.
2. Why not `-expm1(-x)`? Contributor didn't know or didn't think to.
3. Why didn't review catch it? No documented "use stable form" rule for inference.
4. Why no rule? Numerical-stability rules have been added incrementally as bugs surfaced.
5. Root cause: **no codified numerical-stability checklist for inference contributions.** And: the same bug pattern exists in 4 places (`chain_binomial.rs:302`, `tau_leap.rs:169`, `pgas_grad.rs:165`, `pgas.rs:301`) — `(1.0 - (-x).exp()).clamp(0.0, 1.0)` everywhere, not just in PGAS.

**Double-win:** H3's fix should extract a single primitive `prob_from_total_rate(rate, dt) -> (p, q)` to a `numerics.rs` module and replace all 4 call sites. Add to `make audit-greps`: any new `1.0 - .*\.exp()` or `1.0 -.*exp(-` triggers review. Adds a paragraph to a (new) `docs/dev/numerical-stability-rules.md` covering the standard cases (`expm1`, `log1p`, `logsumexp`, erfc-based-CDF, q-pair binomials).

---

**H8 (CSMC ancestor sampling wrong index).**

5 whys:
1. Why does `prev_counts` get re-indexed after resample? Reused as a working buffer.
2. Why was it reused? Memory efficiency / the variable was already there.
3. Why is the reuse silent (no compile error)? Same name, same type, same scope.
4. Why is variable reuse for distinct semantic purposes a problem? Because the two roles (ancestor-sampling source vs transition-density source) have different correctness requirements.
5. Root cause: **variable reuse for distinct semantic meanings.** Probably present elsewhere in PGAS — `pgas.rs` is dense with intermediate buffers reused across substeps.

**Double-win:** H8's fix should rename `prev_counts` to its specific semantic purpose at each use site. Probable double-win: `prev_counts_for_ancestor_sampling`, `prev_counts_for_transition_density`. Sweep `pgas.rs` for similar reuse patterns; if found, rename in the same commit. Add to coding-standards: prefer distinct names for distinct semantic uses, even at the cost of a few extra `clone()` calls.

---

**H9 + H10 (duplicated resampling / ancestor-sampling implementations).**

5 whys:
1. Why duplicated? Each user needed slightly different parameterisation (RNG vs precomputed-uniform; resample vs categorical).
2. Why didn't they extend the existing function? Adding a parameter is more work than copy-paste.
3. Why is copy-paste so cheap? No "primitives module" convention; resampling primitives live in the same files as their callers.
4. Why no primitives module? Inference primitives accreted as needed.
5. Root cause: **no canonical home for inference-numerical primitives.** Same root cause as H3.

**Double-win:** Pair H3 + H9 + H10 + M7 + M8 fixes — extract every reusable inference primitive to a single `inference/numerics.rs` (or `primitives.rs`) module. The set is small: `categorical_log`, `systematic_resample_with_u`, `prob_from_total_rate`, `binom_logpmf(k, n, p, q)`, `matvec_lower`. One module, ~200 lines, covers the recurring need. Future contributors have a clear "look here first" home.

---

**H11 (no newtypes for indices).**

5 whys:
1. Why bare `usize`? Newtypes have boilerplate (`From`, `Deref`, `Add`...).
2. Why is the boilerplate annoying? Rust's macro story for newtypes is incomplete; either you write it by hand or pull in `derive_more`/`nutype`.
3. Why not pull in such a crate? Dependency aversion / convention.
4. Why does the convention persist? Inertia; the team got used to bare usize.
5. Root cause: **the cost of introducing newtypes is friction-bound, not principle-bound.** A small `idx_newtype!` macro in `sim/inference/types.rs` would close it.

**Double-win:** Land a tiny `idx_newtype!($name)` macro that generates the standard newtype + `From<usize>` + `Index<$name>` impls. Use it for `CompartmentIdx` and `ParticleIdx` (the alpha-sprint pair). Subsequent rollouts (`ObsIdx`, `TransitionIdx`, `StratumIdx`) cost one line each. Removes the friction that's been blocking the work.

---

**H12 + H13 (CLI flags silently no-op).**

5 whys:
1. Why are these flags silently ignored? Declared on shared structs (`InferenceCore`); consumed only in some paths.
2. Why not validated by clap? clap can't express "this flag requires this subcommand value."
3. Why no runtime validation? Each consumer's validation is its own; no shared "did anything actually consume this flag?" check.
4. Why no shared check? Each subcommand was added independently with its own arg-parsing logic.
5. Root cause: **CLI flags declared on shared structs have no consumption guarantee.** Same shape as M15 (default seed indistinguishable from user-supplied) — a CLI flag that exists but has no observable effect.

**Double-win:** For every shared argument struct (`InferenceCore`, `OutputCore`, etc.), add an integration test that exercises every consumer subcommand with the flag set and asserts the flag took effect (parallel actually parallelises; record-prequential actually records; seed actually changes the trace). Adds confidence that the args/consumers wiring is real, not aspirational. Catches the H12/H13 class entirely.

---

**M9 (Expr derives PartialEq over f64).**

5 whys:
1. Why derived PartialEq? Tests want to compare ASTs.
2. Why use derive over hand-written? Less code.
3. Why does derive produce wrong f64 semantics? `f64: PartialEq` ignores `NaN ≠ NaN` per IEEE 754; derive uses the field types' impls.
4. Why didn't anyone notice? No test exercises NaN equality on `Expr::Const`.
5. Root cause: **`#[derive(PartialEq)]` on any type containing `f64` is silently wrong.** Probably present elsewhere — every IR struct deriving `PartialEq` (every struct in `ir/src/expr.rs`, given the `head -20` output earlier showed `#[derive(... PartialEq ...)]` on ~20 types) potentially inherits this defect.

**Double-win:** Audit every `#[derive(PartialEq)]` in `ir/` for f64 fields; hand-write bitwise equality on `value.to_bits()` for the few that need it. Add a clippy lint or `make audit-greps` check that flags new `#[derive(PartialEq)]` on types with f64 fields. Single-class fix, multi-site impact.

---

**M10 (Trajectory::default empty).**

5 whys:
1. Why does Trajectory implement Default? Some early caller wanted it.
2. Why does Default produce empty? f64 fields default to 0; Vec<T> defaults to empty.
3. Why does that pass review? `Default` derivation is invisible to readers; the derive macro doesn't say "this type has no sensible default."
4. Why is the empty default wrong here? Because every consumer crashes on empty.
5. Root cause: **`#[derive(Default)]` is overused for types where there is no sensible default.** Same shape as M9 — derived trait silently wrong.

**Double-win:** Sweep `ir/src/` and `sim/src/` for `#[derive(Default)]`; for each, ask "is the all-zero/all-empty value a valid instance?" Most will pass; a few (probably `Trajectory`, possibly `IF2Result`, `PGASResult` if they exist) won't. Replace `Default` with named constructors. Same pattern as the `Option<String>` sweep (S3) and the PartialEq sweep — derive macros silently producing wrong semantics.

---

**M14 (missing compartments default to 0 silently).**

5 whys:
1. Why default to 0? Initial conditions are `HashMap<String, f64>`-shaped; missing keys take 0.
2. Why no validation that all compartments are listed? Validator wasn't extended.
3. Why does the validator have gaps? Same root as C4 — fit.toml/IR validators built incrementally.
4. Why incremental? Each feature added its own validation; no central rule.
5. Root cause: same as C4. **Validators are partial because they grew incrementally.**

**Double-win:** subsumed by C4's `cmd_fit_validate` if extended to also cover IR-level structural completeness checks. One validation entry point covers C4, M14, M15 (and likely several future M-tier findings).

---

**M16 (--dt 1.0 default applied silently).**

5 whys:
1. Why `--dt 1.0` default? Picked at clap definition time.
2. Why 1.0 rather than `time_unit`-aware? clap doesn't know about `time_unit`.
3. Why doesn't the runtime check `--dt` against `time_unit`? No cross-validation between IR-derived semantics and CLI-supplied numerics.
4. Why is there no cross-validation? Same root as C4 / M14 — validation is partial.
5. Root cause: same as C4. Bundles into the same `cmd_fit_validate` work.

---

### Process changes the sweep adds

In addition to §5:

6. **`make audit-greps` should also flag (extending §5):**
   - `1\.0\s*-\s*.*\.exp\(\)` (catastrophic cancellation candidate; should be `expm1`).
   - `\.unwrap_or\(0\.0\)` and `\.unwrap_or\(0\)` in `sim/`, `cli/` (silent fallback).
   - `#\[derive\(.*PartialEq.*\)\]` on types with `f64` fields (M9-class).
   - `#\[derive\(.*Default.*\)\]` (sweep candidate — confirm sensible default).
   - `Option<String>` in `ir/src/` (finite-domain audit candidate).
   - `log::warn!` in `sim/` (typed-diagnostic candidate).
7. **New file `docs/dev/numerical-stability-rules.md`** — short codification: when to use `expm1`, `log1p`, `logsumexp`, `erfc`-based CDF, q-pair binomials. Becomes the reference for inference reviewers.
8. **`make capability-coverage`** — enumerates `Model` fields and asserts each is declared in `Capabilities`, marked always-on, or marked config-only. Catches C3-class bugs.
9. **CLI consumption tests** — for every shared argument struct, an integration test exercising every consumer with the flag set, asserting observable effect. Catches H12/H13-class bugs.

---

### What the sweep did not change

For the avoidance of doubt, the per-issue plans for these findings stand without modification — the 5-whys produced no double-win:

C5 (already revised), H1, H2, H6, H7, H14, M1, M2, M3, M4, M5, M6, M7, M8, M11, M12, M13, M17, M19, M20, M21.

(Several of these are *included in* a double-win identified above — H6 in S3, H9/H10 in the "primitives module" pattern, M12/M13 in C8 — but their per-issue actions don't change.)

---

## Appendix — what stays as-is

Per the audit's "What's clean" section:

- `sim::time` as single time-conversion entry point (gh#53).
- `step_csmc` calls `step_one`; `log_transition_density_substep` is a deliberate density shadow with parity contract.
- `log_sum_exp` has one canonical implementation.
- `eval_likelihood_resolved` is the single observation-likelihood dispatcher.
- `apply_interventions_at` guards NaN `t`; table OOB defaults to Error; `InitialConditions::FromDistribution` is a hard error.
- `dimcheck.ml` covers per-day/per-week/probability distinctions.

These are the seams the audit verified and chose not to flag. Don't touch them as part of this remediation; the work above respects these contracts.
