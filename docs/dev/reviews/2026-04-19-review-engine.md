Let me plan the read order strategically. Given the scope (~27K lines) and the findings from the OCaml side I want to verify:

1. **`ir/` crate** (1.0K lines) — First. Tells me whether deserialization is strict. If I see the Rust ir crate silently accepting nulls / defaulting fields, Round 1's C1 and C5 become catastrophic. Key files: `lib.rs`, `model.rs`, `expr.rs`, `validate.rs`.

2. **`sim/` foundations** — `lib.rs`, `state.rs`, `rng.rs`, `resolved_expr.rs`, `propensity.rs`, `compiled_model.rs`. Before reading the backends.

3. **Backends** — `gillespie.rs`, `chain_binomial.rs`, `tau_leap.rs`, `ode.rs`, `intervention.rs`. Looking for the EKRNG/CRN claims, the intervention handling per §2.3.1, and the multinomial competing-risks fix.

4. **Inference** — `particle_filter.rs`, `if2.rs`, `obs_loglik.rs`, `pgas.rs`, `pgas_grad.rs`, `nuts.rs`. Scientific-correctness central. pgas is huge (1718 lines) so I'll probably read it in sections.

5. **CLI** — only if time permits; mostly I care about `main.rs` dispatch and whether documented flags exist.

6. **Tests** — spot-check coverage gaps analogous to the OCaml side (does anything test the overdispersed path? the ODE path? the chain-binomial competing-risks fix that the blog said got applied?).Now this is important. Let me check — on the OCaml side, `InterpMethod` was `method_: string` (`ir.mli:53`), and serde emitted `"method": str i.method_` (`serde.ml:310`). The Rust side uses `#[serde(rename_all = "snake_case")] InterpMethod { Linear | Constant | Spline }` but the field name is just `pub method: InterpMethod`. Serde will expect `"method": "linear"` / `"constant"` / `"spline"`. OCaml emits whatever string the user wrote. Let me check this.Let me check whether `validate::validate` is actually called during the simulation pipeline.## Summary — what I got through

I was able to read the full `ir/` crate (all ~1K lines) before running out of tool budget. That crate alone turned up some real issues. I did not get to `sim/`, `cli/`, or the tests — those are the next batches.

## Findings from the IR crate

### Critical

**RC1. `ir::validate::validate` is never called from anywhere in the runtime.** `grep -rn "validate::validate\|ir::validate\|validate(" crates/sim/src crates/cli/src` returns nothing. This mirrors exactly the OCaml-side Round 1 M1 (OCaml's `Validate.validate` also uncalled). Both sides have thorough integrity checkers. Neither runs them. The resulting operational state is: the compiler emits IR, the runtime deserializes it with serde, and if the OCaml compiler ever emits garbage (see the entire silent-fallback pattern from Rounds 1-3: C1 overdispersed→Poisson, C5 ODE dropped, C6 empty transfer actions, C7 "?" compartment names), the Rust runtime happily loads it and simulates against nonsense.

Concretely, `ir::validate` would catch every one of these OCaml-side bugs:
- C3 (under-indexed transition with unexpanded base name) — caught by `UnknownCompartmentInStoichiometry`
- C5 (ODE dropped) — caught by `MissingOdeEquation`
- C6 (transfer without fraction/count → empty actions) — *not* directly caught (validate doesn't check action well-formedness), but real compartment typos in actions would
- C7 (src/dst = `"?"`) — caught by `UnknownCompartmentInStoichiometry` if the resulting "?" appeared in stoich, but transfer actions are separate
- Duplicate names, zero deltas, unknown refs throughout — all caught

Fix: call `ir::validate(&model)` after `ir::from_str` in whatever path loads models in `cli/src`. One function call, the whole integrity net goes live. I'll confirm the call site when I read the CLI.

### Major

**RE1. `TimeExpr { pub time: () }` round-trips as `{"time": null}` — but the OCaml emits `{"time": null}` via `obj [("time", null)]`.** Let me confirm: OCaml `serde.ml:111` emits `obj [("time", null)]` → `{"time": null}`. Rust deserializes via `TimeExpr { time: () }` — serde represents `()` as `null`, so the JSON `{"time": null}` deserializes. Confirmed fine.

But there's a subtler issue: the `Expr` enum is `#[serde(untagged)]`. That means serde tries each variant in order and picks the first that parses. `ConstExpr { value: f64 }` (serialized field name `const`), `ParamExpr { param: String }`, etc., all use unique key names, so they disambiguate. **But** for the `ProjectedExpr { projected: () }` case: the JSON `{"projected": null}` has to not match any earlier variant. Since no earlier variant has a field named `projected`, it'll fall through to the right arm. OK.

The real risk with untagged enums: if the JSON has *no* recognized key, serde's error is `"data did not match any variant"` with no hint about *which* key was unexpected. If a user hand-writes a malformed IR with `{"poisson": null}` in a rate position, the error is generic. Given the "error messages are a feature" stance, this is worth noting but low-priority.

**RE2. `Parameter::value: Option<f64>` doesn't have a `#[serde(default)]`.** `model.rs:149` (`initial_conditions`) and similar are required fields. The OCaml serde always emits these. OK in the normal flow. But if anyone hand-writes IR, missing `value` → deserialization fails rather than `None`ing out. Worth checking whether that's intentional ("hand-crafted IR must declare value explicitly as null") or a mis-spec.

**RE3. `Parameter::transform: Option<Transform>` and `initial_value: Option<f64>` likewise lack `#[serde(default)]`.** `parameter.rs:55–56`. OCaml emits these as `null` when not set, which serde-Option decodes as `None`. Fine for machine-generated IR. But again, a hand-crafted IR omitting these fails load. Maybe intentional, worth documenting.

**RE4. `Transition::event_key: Option<String>` doesn't have `#[serde(default)]`.** `transition.rs:45`. Same as above.

**RE5. Model field `presets` renames from `scenarios` to `presets` on load but keeps the `scenarios` key in JSON via `#[serde(rename = "scenarios")]`.** `model.rs:153`: `#[serde(default, rename = "scenarios")] pub presets: Vec<Preset>`. This means the field is called `presets` in Rust but `scenarios` in JSON, matching the OCaml serde output (`serde.ml:884`: `("scenarios", arr (List.map preset_to_json m.presets))`). Consistent, just confusing nomenclature. OCaml side called it `presets` in `ir.mli:227`, emits as `"scenarios"`, Rust loads as `presets`. Fine — but if anyone greps for "scenarios" they find one OCaml spot; if they grep for "presets" they find another. Minor inconsistency, flagging for future refactor cleanliness.

**RE6. `InterpMethod` allows `Constant` but OCaml `ir.mli:53`'s `method_: string` is opaque.** If the OCaml compiler emits `{"method": "constant"}` (not a possible literal from the DSL based on what I read), Rust accepts. If OCaml emits `{"method": "step"}` (some unconventional spelling), Rust fails. The OCaml compiler side uses `get_str_kw "method" "linear"` (expander.ml:1802) which just picks up whatever string the user typed. So a user who writes `method = stepwise` gets `{"method": "stepwise"}` in IR, and Rust fails at load with `unknown variant "stepwise"`. That's not a bad failure mode (clear error), but the OCaml side should validate the value at compile time rather than letting it propagate silently to the runtime. Flagging as an OCaml retrospective gap.

**RE7. `Preset::compose: Vec<String>` is accepted but unused semantically.** `model.rs:90–91`. I'll check what Rust does with `compose` when I read the scenario-application code in `sim/`. If nothing uses it, it's dead data just like `parameter_groups` and `data_contract` — round-tripped from JSON but never consulted.

**RE8. `StoichiometryEntry(pub String, pub i64)` is a tuple struct that serde serializes as a 2-element JSON array `["S", -1]`.** OCaml emits the same (`serde.ml:215–216`). Consistent, good — but the `i64` vs OCaml's `int` is a width concern. On 32-bit platforms OCaml's `int` is 31 bits; `i64` on Rust always 64. Deltas fit in i32 easily (they're ±1 mostly), so no actual overflow risk. Nit.

**RE9. `BalanceSpec` deserializes a non-empty object but there's no `#[serde(default)]` on its sub-fields.** `model.rs:122–126`. If the OCaml compiler emits `"balance": null`, the outer `Option<BalanceSpec>` becomes `None` correctly. If it emits `"balance": {}` (empty object — hypothetically), deserialization fails cleanly with "missing field target". OK.

### Minor

**Rm1. `CompartmentKind` deserialization is `#[serde(rename_all = "snake_case")]` → accepts `"integer"` / `"real"`.** Consistent with OCaml output. Not a bug, noting for completeness.

**Rm2. `Likelihood` enum variants are snake_case: `"poisson"`, `"neg_binomial"`, `"normal"`, etc.** Matches OCaml output. Good.

**Rm3. `Expr::Time(TimeExpr { time: () })` and `Expr::Projected(ProjectedExpr { projected: () })` — the unit-valued wrapper structs are awkward but serde-correct.** The alternative would be a tagged enum, but that changes the wire format. Keeping as-is is the right call if the JSON contract is fixed.

**Rm4. `#[serde(untagged)]` on `Expr` means serialization is "try-each-variant-until-one-matches."** For a 27K-line codebase with many rate expressions evaluated per step, the parse cost of this is nonlinear — for each JSON object, serde has to attempt each of the 11 variants. This is a **load-time** cost, not a hot-loop cost (load happens once), so not a concern for simulation performance. But for tools that re-parse IR repeatedly (e.g., CLI pipelines that chain many invocations), worth measuring.

**Rm5. `validate.rs:171–177` — the `Expr::Projected(_)` case has a comment about `allow_projected` but doesn't actually gate on it.** 
```rust
Expr::Projected(_) => {
    if !allow_projected {
        // We don't emit an error here currently; the schema validator handles it.
    }
}
```
The `// We don't emit an error here currently` admission is exactly the loose-semantics smell. The comment says "schema validator handles it" — but there's no schema validator running at load time that I can see. If a user writes `rate = projected * 10` in a transition (not a likelihood), the Rust validator silently accepts it, and then `resolved_expr` or `propensity` will crash or produce garbage when it hits `Projected` where it expects a concrete value. Either wire the error in, or remove the dead branch and the misleading comment.

### Nits

**Rn1. Each of ConstExpr, ParamExpr, PopExpr, etc., is wrapped in another struct (`BinOpWrap`, `UnOpWrap`, etc.) for the `untagged` enum to discriminate.** The double wrapping is a serde artifact — documented as such in `expr.rs:137`. It's correct, just noisy at the AST walking sites.

**Rn2. `TableLookupExpr::indices: Vec<Expr>` matches the OCaml side's `indices: expr list` post-flattening.** Both sides always emit a single-element `[Const n]` for the flattened linear index. If Rust ever sees multi-element indices, it's a compiler bug on the OCaml side. The Rust validator doesn't check this invariant. Flagging for the sim-side review: when I read the evaluator, I'll check whether it assumes length 1 (a soft invariant) or handles multi-index (a relic of the spec's N-dim design).

**Rn3. `version: String` in `Model` — no version check at load.** `model.rs:133`. The OCaml emits `"0.3"` unconditionally (`expander.ml:2604`). If a future compiler change bumps to `"0.4"`, Rust loads it silently. Per CLAUDE.md "backwards compatibility is a non-goal" this might be fine — IR doesn't need to stay forward-compatible — but there's no load-time check for "this Rust expects version X." Worth adding for the same reason Validate should be wired in.

## What's next

Still to cover on the Rust side:
1. **`sim/src/` foundations** — `lib.rs`, `state.rs`, `rng.rs`, `resolved_expr.rs`, `propensity.rs`, `compiled_model.rs`. Critical for understanding how expressions are compiled to fast form and how CRN is implemented.
2. **Backends** — `gillespie.rs` (359), `chain_binomial.rs` (439), `tau_leap.rs` (190), `ode.rs` (264), `intervention.rs` (248). The scientific correctness surface — the multinomial competing-risks fix (per the blog post I saw the reference to), the Gillespie intervention restart semantics, the ODE integration for PDMP.
3. **Inference** — 7 files totaling ~3700 lines. `particle_filter.rs` (241), `if2.rs` (634), `obs_loglik.rs` (444), `pgas.rs` (1718!), `pgas_grad.rs` (344), `nuts.rs` (428), `pmmh.rs` (525), `correlated_pf.rs` (406), `multi_stream_obs.rs` (349), `prior.rs` (218). This is where silent scientific wrongness would bite hardest.
4. **CLI** — `main.rs` (2160), `batch.rs` (1356), `util.rs` (1249), `browse.rs` (884). Plus hashing/cas/run_meta. Big surface, but lower-risk than the scientific code; mostly flag-parsing and orchestration.
5. **Tests** — 20 test files in `sim/tests/`, 3 in `cli/tests/` (I haven't counted cli yet). Will tell me the Rust-side coverage story.

Priority for next batch: `sim/src/{lib,state,rng,resolved_expr,propensity,compiled_model}.rs` — ~1500 lines — to understand the foundational contract. Then backends. Then inference. Say the word when you want me to continue.

This is a huge finding. Claude.md describes EKRNG (Event-Keyed RNG) as the design. compartmental-ir-spec.md:826–891 is an entire section on EKRNG. The spec says:

U_e = g(seed, event_key_e) with Philox/Threefry counter-based PRNG
"Placebo Test": executing an irrelevant extra draw must produce bitwise-identical trajectories
"Scenario Coupling": baseline vs intervention trajectories identical pre-intervention

But the Rust rng.rs only has StatefulRng wrapping ChaCha8. No EKRNG. Let me verify there isn't an ekrng module I missed:Ran a command, viewed a fileRan a command, viewed a fileConfirmed. The comment says EKRNG was deliberately skipped. And ekrng.rs is available if needed — but it's not present in the codebase (grep -l ekrng returns nothing). That's a comment lie. Let me now read the core evaluator.Viewed 2 filesViewed 2 filesNow the propensity module:Pivoted toward examining extensive compiled model filePivoted toward examining extensive compiled model fileNow let me look at the backend implementations. Start with Gillespie since it has the CRN claim:Read gillespie.rsRead gillespie.rsNow let me scan the critical chain_binomial backend where the multinomial competing-risks fix lives:Viewed 3 filesViewed 3 filesWait — critical issue. The chain-binomial step_one uses the Euler-multinomial algorithm which is the correct fix per the blog. But look at tau-leap: independent Poisson draws per transition, no competing-risks handling. This is the classic tau-leap approximation — it's fine for non-competing transitions but wrong for competing exits from the same source compartment. Let me check if tau-leap is supposed to behave this way, and let me look at intervention.rs:Viewed 3 filesViewed 3 filesLet me scan apply_intervention vs inject_event_deltas closely for a discrepancy. I noticed one.
Look at apply_intervention lines 170–193 for FractionTransfer: it computes transfer = floor(src * frac). Same for AbsoluteTransfer (line 206): transfer = min(n.round(), counts[src]). These work off live state.
But inject_event_deltas lines 118–128 for AbsoluteTransfer: computes n.round() then n.min(snapshot.counts[sl]) — where snapshot is the value at start of step (scratch.int_s at line 411 was copy_from_slice(counts) which is the state before deltas were applied). But counts here is the end-of-step state before intervention (after deltas), not the start. Wait — let me re-check.
Actually in chain_binomial at line 411, scratch.int_s.counts.copy_from_slice(counts) where counts is already after apply pending_deltas. So apply_interventions_at uses post-delta state. But that's for non-always-active — inject_event_deltas is for always-active events and called before deltas are applied, operating on the start-of-step snapshot.
There's a subtle inconsistency in semantic meaning. Per spec §2.3.1, events in events { … } are always-active and inject atomic deltas (the "event deltas" pattern is per the OCaml CLAUDE.md mention). Interventions in interventions { … } default to non-always-active. Both go through apply_interventions_at for the scheduled-time firings.
Let me look at apply_intervention for Add (line 230–244): the comment on line 242 says else if let Some(local) = model.global_to_real[global] { real_s.values[local] += n } — this uses the raw n, not the rounded count. So Add to real = raw value; Add to integer = rounded count. That's a subtle inconsistency but probably intentional (integer compartments are integer-valued).
Let me now look at the simulate and output modules:Viewed 3 filesViewed 3 filesNow I have enough of the foundations + backends to report. Let me also quickly check the inference module directory:Read inference modRead inference modThat's a good map. Let me now synthesize the findings. This is a lot to write up.
Summary — what I got through this batch
Full read of:

All of sim/src/ foundations: lib.rs (44), config.rs (73), error.rs (38), state.rs (122), rng.rs (121), resolved_expr.rs (442 full), propensity.rs (327), compiled_model.rs (718)
All four backends: gillespie.rs (359), chain_binomial.rs (439), tau_leap.rs (190), ode.rs (264), plus ode_integrator.rs (66), intervention.rs (248), simulate.rs (23), output.rs (18), transition_diagnostics.rs (87)
The inference module's top-level mod.rs (39) and trait structure

That's ~3300 lines. The inference subdirectory proper (~3700 lines) and the CLI (~6600 lines) remain.
Findings
Critical
RC2. EKRNG is specified, documented in the IR spec, and referenced in code comments — but completely absent from the runtime. compartmental-ir-spec.md:826–891 mandates event-keyed counter-based PRNG (Philox/Threefry) with the "placebo test" invariant (adding irrelevant extra draws must not change trajectories) and "scenario coupling" guarantee (baseline vs intervention identical pre-intervention). rng.rs:13 has a comment "Use a different derivation than EkRng so seeds don't collide" — implying EkRng exists. gillespie.rs:78 says "(ekrng.rs is available if needed)". A grep for EkRng|ekrng|Philox|Threefry across the codebase turns up only these comments. The file doesn't exist.
What the code does have:

rng.rs is a plain ChaCha8 stateful PRNG
gillespie.rs:72–78 documents that CRN ("same seed → identical trajectories pre-intervention") is achieved via sequential-draw coincidence from shared stateful state

The CRN claim in that comment is false in general. Consider a scenario-coupled pair where the scenario adds an early (scheduled but ineffective, say fraction = 0) intervention. The intervention application consumes no RNG in inject_event_deltas / apply_intervention (the delta computations are deterministic given state), so the base case holds for that specific construction. But the moment the scenario changes any quantity that affects even a single draw before the intervention time — including changing seed, adding an overdispersion expression whose value differs by roundoff, or reordering any transition — the shared-seed guarantee is gone. The spec's EKRNG formulation is what robustly gives scenario coupling; the current code gives it only in the narrow case where both scenarios hit the RNG in bitwise-identical order. For realistic scenario counterfactuals this will not hold.
Three separate issues rolled up here:

The spec promises EKRNG; the code doesn't implement it.
Comments in the code claim an ekrng.rs exists when none does.
The CRN claim for scenario coupling is weaker than the comment implies — it depends on identical RNG consumption order, which is a fragile invariant under any realistic model edit.

Priority: critical because this is the mechanism by which scenario counterfactuals get precise coupled estimates. PGAS's conditional-SMC also relies on exact coupling. Without EKRNG, any claims in docs/ about variance-reduced counterfactuals are not defensible. Fix: either implement EKRNG as spec'd, or update the spec and all downstream claims to describe what the code actually does (a stateful PRNG with no coupling guarantee beyond identical-execution-trace).
RC3. RC1 restated with full impact: ir::validate::validate is never called. Combined with every OCaml-side silent-fallback bug I documented (C1 overdispersed→Poisson, C2 dim_value_index→0, C3 stoich base-name, C5 ODE dropped, C6 empty actions, C7 "?" compartments), the Rust runtime has no structural safety net between "compiler emitted garbage" and "simulation proceeds to give wrong answers." CompiledModel::new does some local checks (unknown compartment on line 404, unknown ODE compartment on line 457, real-compartment-in-stoich on line 408) — but these are partial. validate.rs has a complete integrity battery; it just doesn't run. Wiring ir::validate::validate(&model)? into the path that loads JSON in the CLI is a one-line fix that turns every silent bug into a loud one.
RC4. compiled_model.rs:378 — parameter without value is a hard error, but initial_conditions::FromDistribution is silently treated as "use zero defaults."
rustInitialConditions::FromDistribution(_) => {
    // Not supported in sim at runtime; use default zeros
}
compiled_model.rs:711–713. If the OCaml compiler ever emits "initial_conditions": { "from_distribution": {...} } (which would happen if the DSL had a init { S ~ uniform(900, 999) } syntax — I didn't see the parser support this, but the serde type is defined in ir/model.rs:38), the Rust runtime starts all compartments at 0 and doesn't tell anyone. Another silent-wrong-answer primitive. At minimum: return Err(SimError::Validation("FromDistribution initial conditions not yet supported in sim")). Ideally: wire the draw into the inference-side prior sampling path.
RC5. chain_binomial.rs:220 — scratch.int_s.counts.copy_from_slice(counts) and thereafter scratch.int_s is treated as a const snapshot — but scratch.int_s is never updated as the step proceeds, which means inject_event_deltas (called at line 359) uses the start-of-step state as "snapshot," not the post-transition state. This is actually intentional per the comment on intervention.rs:60–67 ("evaluated from snapshot, applied atomically") — events see pre-transition state. But there's an edge case: what if an event's action expression references a compartment state that was just modified by a transition in the same step? E.g., an event if S == 0 then add(I, 1) should fire when S hits 0 this step, but because snapshot is start-of-step, the condition is evaluated against the pre-step S, which may be nonzero. This may be intentional "events are atomic wrt transitions in the same step" semantics, but the docs (docs/camdl-ir-spec.md incidents/chain_binomial_double_fire.md — I haven't read it but the comment at chain_binomial.rs:147–149 refers to it) should nail this down. Semantically it's either a bug or a quirk; at minimum it needs a design note.
Major
RM1. tau_leap.rs uses independent Poisson draws per transition, with no competing-risks protection for transitions sharing a source compartment. tau_leap.rs:125–138:
rustfor (i, &lambda) in propensities.iter().enumerate() {
    let mean = lambda * dt;
    let count = match draws[i] {
        ResolvedDraw::Poisson => rng.poisson(mean),
        ...
    };
    for &(local, delta) in &model.transition_stoich[i] {
        int_s.counts[local] += delta * count as i64;
    }
}
// Clamp
let clamped = int_s.clamp_nonneg();
If a source compartment S has 100 individuals and two competing exits (S → E at rate β·I and S → V at rate ν), tau-leap may draw, say, 80 infections and 50 vaccinations — 130 exits from 100 people. The clamp_nonneg at line 141 silently converts this to 0, but the "extra" 80 + 50 − 100 = 30 people are now phantom additions at the destinations E and V. Total population is not conserved; conservation is silently broken.
Compare chain_binomial, which correctly uses the Euler-multinomial algorithm (lines 246–341) precisely to avoid this. The same approach should apply in tau-leap for transitions in source_groups. This is why pomp::reulermultinom exists.
Given that tau-leap is registered as a full Simulate backend (SimConfig::TauLeap) and reports OVERDISPERSION | REAL_COMPARTMENTS capabilities, users will select it thinking "tau-leap = chain-binomial with different noise." It's not. Fix: port the source_groups multinomial logic from chain_binomial into tau_leap. Until then, tau_leap is unsafe for any model with competing exits — which is basically every realistic epi model (SEIR → E → I vs E → dies, etc.).
Worth noting: the comment at the top of the chain-binomial block (line 253–257) explicitly warns about this: "The old algorithm systematically over-counted total exits, causing particle trajectories to drift and ESS to degrade." That fix was applied to chain-binomial. Tau-leap was not updated.
RM2. propensity.rs:72–85 and rng.rs use a silent-fallback pattern: Div by zero → 0.0 with log::debug, Pow producing NaN/Inf → 0.0 with log::warn, NaN from UnOp → 0.0 with log::warn. These are the Rust mirror of the OCaml-side endemic silent fallback. Logging isn't enough — for inference runs with millions of steps, the logs are either ignored (default log level) or a firehose. A rate that suddenly hits Pow-overflow silently becomes 0 for an entire trajectory, and ESS degrades for reasons the user cannot diagnose.
Compare resolved_expr.rs:244 (the hot-path version) which does the same thing without even the log warning: if b == 0.0 { 0.0 } else { a / b }. The two paths (eval_expr and eval_resolved) have the same silent-fallback behavior but the resolved one dropped the log message — so the already-inadequate signal is worse in the path that's actually used at runtime. Fix: thread a SimError::DivisionByZero up (which is defined but unused! Check error.rs:21), or make the fallback behavior policy-controlled.
This is made worse by the fact that eval_resolved is infallible by design (it returns f64, not Result) — so any error has to be either swallowed or panic. The team chose swallow. I'd argue the design choice itself is wrong: an eval_resolved_checked variant that returns Result<f64, SimError> should exist and be used in inference paths where wrongness is a scientific bug, with the infallible variant reserved for tight hot loops that can assert a precondition first.
RM3. resolved_expr.rs:288–314 — TableLookup::Error policy silently clamps instead of erroring.
rustOobPolicy::Error => {
    // Defensive: clamp instead of panic. The Error policy
    // means the model author wanted strict bounds, but in the
    // resolved hot path we can't return Result. This matches
    // the defensive approach used for div-by-zero and NaN.
    if table_idx_val < 0 || table_idx_val >= n {
        log::warn!(
            "resolved table lookup: index {} out of bounds [0, {}), clamping",
            table_idx_val, n
        );
        table_idx_val.clamp(0, n - 1)
    } else {
        table_idx_val
    }
}
The Error policy in the IR (ir/table.rs:9) exists specifically because the author said "I want bounds checked strictly." The hot-path evaluator defeats this with log+clamp. Compare propensity.rs:234–240 (the slow eval_expr path), which correctly returns SimError::TableLookup(...). So the same model gets different behavior depending on which eval path is used — strict errors in construction-time eval_table_expr, strict errors in eval_expr, silent clamp in the hot path eval_resolved. At minimum the three should agree.
This is also why SimError::TableLookup exists but is rarely actually returned. Fix same as RM2: add a checked variant and use it in inference, or pre-validate all table index expressions at CompiledModel::new time to guarantee they can't produce out-of-bounds under any param vector the inference will explore.
RM4. rng.rs:88–94 — binomial() fallback when Binomial::new fails: return n if p > 0.5, else 0.
rustmatch Binomial::new(n, p.clamp(0.0, 1.0)) {
    Ok(b) => b.sample(&mut self.0),
    Err(_) => if p > 0.5 { n } else { 0 },
}
The comment at line 86 defends this as "particles with extreme parameters get -inf loglik and are resampled away." That's probably true for IF2 but a worry for any use case that isn't IF2. Specifically: during Gillespie exact simulation, this fallback would produce a deterministic "either all N transition or none" outcome silently. Gillespie doesn't call binomial() per se, but chain_binomial does (chain_binomial.rs:311, 328). If the binomial draw in a source group falls into this fallback, the trajectory for that step becomes deterministic-maximal or deterministic-zero. For small N (early epidemic, rare strata), this could dominate.
The Binomial::new fails only on invalid inputs (n > i64::MAX as u64 or similar), which shouldn't happen with valid state. But p.clamp(0.0, 1.0) on the previous line catches the p-out-of-range case, so how can Binomial::new fail after clamp? Answer: n is u64 so it can't be negative, and p is clamped; Binomial::new can fail when n == 0 (returns Err) — which actually shouldn't produce an error, so this branch is defending against an edge case that shouldn't arise. The whole guard is probably dead code. Either way, document it or remove it.
RM5. rng.rs:41–55 — neg_binomial fallback to Poisson when sigma_sq ≤ 0 or shape is degenerate. This is the mirror of the OCaml side's silent-fallback-to-Poisson from my Round 2 C1, but on the evaluation side. The comment at lines 44–48 justifies it: "shape < 1e-6 means sigma_sq >> dt: the Gamma is degenerate... IF2 will push sigma_se away...". Same concern as RM4: this defense is specific to IF2. In pure simulation mode, or in PGAS/PMMH's exploration of posterior, a particle hitting sigma_sq >> dt gets silently downgraded to Poisson noise, giving an inflated likelihood compared to the true generative model at those params. The posterior's "rejection" of extreme sigma_sq is supposed to come from the data likelihood, not a RNG fallback. Subtle but real.
RM6. compiled_model.rs:711 — FromDistribution init silently ignored (already flagged as RC4 above; promoted to critical because it's a silent wrong init).
RM7. compiled_model.rs:185–233 — eval_table_expr is a separate evaluator from eval_expr/eval_resolved that must be kept in sync. The comment on line 182–184 says exactly this: "BinOp/UnOp arms MUST match the semantics in eval_expr — if a new operator is added there, it must be added here too." This is a maintenance landmine. If someone adds a new BinOp to expr.rs, they have to remember to update three evaluators (eval_expr, eval_resolved, eval_table_expr) and two derivative evaluators (eval_expr_deriv, eval_resolved_deriv). The three regular evaluators also have inconsistent behaviors on edge cases:

eval_table_expr:205 — Div by zero returns 0, no log
eval_expr:72–79 — Div by zero returns 0, log::debug
eval_resolved:243–245 — Div by zero returns 0, no log at all

Plus eval_table_expr uses Pow => a.powf(b) raw (no NaN/Inf guard), which is inconsistent with the other two. A model whose inline table values use Pow can silently produce NaN in a table (entering table_values_cache) and then be read back as NaN during simulation (where eval_resolved has NaN-replacement-by-zero logic for UnOp but not for table lookups). Fix: consolidate to a single evaluator with a restricted-context mode, or at minimum add a test that verifies the three evaluators agree for the same input.
RM8. gillespie.rs:161–167 — "advance real state to boundary using RK4" for real compartments uses a single RK4 step over the entire gap between events, which can be arbitrarily large. If t_next is far from the current event time (which in Gillespie exact mode is variable and unbounded), RK4 accuracy degrades quadratically. No adaptive stepping. For any model where real compartments have stiff dynamics (environmental reservoirs with fast decay + slow accumulation), Gillespie + real-compartments is numerically unsafe.
The TODO at line 163 says "TODO(v0.2): replace with PDMP thinning for real compartments". That's acknowledgment. Fix: adaptive RK45 or Runge-Kutta-Fehlberg with step size control, or at least sub-step if dt > dt_max.
RM9. ode.rs:55–57 — in ODE backend, integer state is rounded at each RK4 sub-step evaluation (int_vals.iter().map(|&x| x.max(0.0).round() as i64)). This means: a model with integer compartment S=100 and rate β·S gives derivative β·100; but if any upstream term pushes S to 100.49, the rounding to 100 gives derivative β·100. Next sub-step, S = 100 + β·100·dt/2, still rounds to 100 if dt is small. The discretization of integer compartments inside an ostensibly continuous ODE introduces O(1/N) systematic error in propensities and O(1/N) quantization noise in trajectories.
The comment at lines 42–45 acknowledges this: "Introducing O(1/N) relative error... negligible for N > ~100 but can cause premature extinction for very small compartment values (< ~10)." That's honest, but this backend is registered alongside the stochastic ones and users picking "ODE" for a deterministic-ODE approximation of an SIR model get this quantization noise applied to their "deterministic" run. Either:

Rename this backend to DeterministicApproximation to signal it's not a pure ODE
Actually integrate the integer compartments as real-valued within the RK4 (drop the round-trip through IntState), and only round at snapshot time

The current behavior is neither fish nor fowl.
RM10. gillespie.rs:247–250 — debug_assert! on non-negativity, but the preceding clamp_nonneg() on line 239 may have silently fixed a negativity from an undetected bug. The debug_assert! passes even when clamp_nonneg clamped a negative value, because the clamp happens first. So the assert is effectively testing "did clamp work" which it always does. Not testing "was state valid without clamping." Same in tau_leap at line 145–148 and chain_binomial (via post-clamp logic). These debug_asserts provide false confidence.
Minor
Rm6. chain_binomial.rs:369–392 — trace-steps code uses static HEADER: OnceLock<bool> inside the hot loop. The *HEADER.get_or_init(|| { eprint!(...); eprintln!(); true }) pattern reads as "print header once, then read the bool every step." Fine functionally but paying the OnceLock read cost every step for the lifetime of the simulation even when tracing is off (tracing is guarded by trace_enabled() earlier at line 370 — OK, so it's only when trace is on). Noting for cleanliness.
Rm7. chain_binomial.rs:22 — RATE_EPSILON: f64 = 1e-15 is exported as pub const with the comment "must be identical to avoid simulation/density mismatch" — but log_transition_density_substep (which the comment refers to) is in the inference module and I haven't confirmed it actually imports this constant. If someone ever changes one value and not the other, the chain-binomial likelihood and its density evaluation diverge silently. Would benefit from a shared-source-of-truth test: #[test] fn density_epsilon_matches_step_epsilon() that asserts they're literally the same constant.
Rm8. intervention.rs:80–81 — let current_step = (t_end / dt).round() as i64; with dt pulled from the model simulation config. But in practice the actual step size is determined by the config passed to run_chain_binomial, not model.simulation.dt. Look at chain_binomial.rs:135: let dt = cfg.dt.min(cfg.t_end - t); — uses config's dt, which may differ from model.simulation.dt. Then apply_interventions_at computes current_step = t / model.simulation.dt which uses the model's dt. If the two differ (e.g., runner uses dt=0.5 to refine a model declared with dt=1.0), the current_step computation silently goes wrong and interventions either don't fire or fire at wrong times.
Fix: pass the actual runtime dt into apply_interventions_at rather than pulling from the model.
Rm9. intervention.rs:47 — current_step = (t / dt).round() as i64 uses round() which can produce 0 at t=0+epsilon (fine) but also uses as i64 cast which truncates silently on NaN/Inf. If t is NaN (from an earlier bug), round() is NaN, and NaN as i64 = 0 in Rust (platform-dependent on older versions but typically clamped or zeroed). Another silent misfire. Worth a guard.
Rm10. compiled_model.rs:476–483 — External table placeholder is vec![], and the comment at line 478 says "If still empty at simulation time, propensity eval will error." I don't see where this errors. If an external table is never replaced and the user runs a sim, table_values_cache[idx] is empty; eval_resolved::TableLookup at line 315 does cached[i as usize] which panics (index out of bounds) if the cached Vec is empty and i = 0. The spec promises an error; the code delivers a panic. Fix: explicit check at model-compile time that every TableSource::External has been replaced before simulation.
Rm11. state.rs:88 — FlowVec::add accesses self.counts[transition_idx] without bounds check. Panics if transition_idx is out of range. Low risk in practice (callers iterate in-bounds), but noted.
Rm12. chain_binomial.rs:431 — balance target that goes negative gets a log::warn but the simulation continues. Comment at 394–396 says "may legitimately go negative... skip clamp... particle filter should penalize via bad trajectories." For inference this is the right behavior. For straight simulation, it means the output TSV has negative compartment counts — users will be confused. Maybe emit a hard error when running under Simulate (non-inference) context? The backend doesn't know whether it's being called from inference or not, but a flag on the config could distinguish.
Rm13. compiled_model.rs:304–306 — fire_steps: Vec<HashSet<i64>> uses std::collections::HashSet (randomized-hash). Iteration order is nondeterministic, but the code only uses .contains(), so iteration order doesn't matter. Still, for reproducibility auditing, BTreeSet<i64> or a sorted Vec<i64> + binary_search would be a nicer fit. Minor.
Rm14. gillespie.rs:211–220 — linear search through cumulative propensities for event selection. For models with >100 transitions, this becomes a hot-path bottleneck. An alias table or Walker-alias-method lookup amortizes to O(1). Not a correctness issue, but the comment at the top of gillespie talks about "billions of times in the inference inner loop" — linear event selection doesn't scale.
Nits
Rn4. rng.rs:79–87 — the docstring for binomial says "inverse CDF (small np)" but the fallback code is if p > 0.5 { n } else { 0 }, which is not an inverse-CDF approximation. The inverse-CDF path is in correlated_pf::binomial_quantile (referenced at chain_binomial.rs:306) — that's a separate function. The binomial() fallback is a step function. Docstring is misleading.
Rn5. chain_binomial.rs:198 — map_or(false, |v| v == "1") could be .is_ok_and(|v| v == "1"). Clippy nit.
Rn6. config.rs:36–38 — SimConfig::variant_name() returns &'static str, used only in error messages. Fine but Display + Debug derives would cover this.
Rn7. compiled_model.rs:474 — let vals: Result<Vec<f64>, SimError> = ...collect() — idiomatic Rust would use ? directly on the collect, not bind to a typed variable first.
Rn8. gillespie.rs:78 — that comment about ekrng.rs being available is inaccurate (no such file). Remove or update.
Overall state of the engine after these three batches
Five themes have emerged:

The silent-fallback pattern is on both sides of the compile/run boundary. OCaml expander silently falls back to Poisson / "?" / 0.0 (Round 2–3). Rust runtime silently falls back on division, NaN, Pow overflow, table bounds violations, extreme parameter degenerates. Neither side calls its Validate module. Together, compiler emits silently-wrong IR, runtime silently consumes it, inference silently converges to the wrong posterior.
EKRNG is the biggest gap. The spec makes strong claims about scenario coupling and placebo-test invariants. The implementation provides neither. This affects what can honestly be said about counterfactual scenarios and PGAS coupling.
Tau-leap has a correctness bug that chain-binomial fixed. Multinomial competing-risks needs to be applied to tau-leap.
The three expression evaluators drift. eval_expr, eval_resolved, eval_table_expr all have slightly different behaviors on edge cases (Div by 0, Pow NaN, table OOB) even though semantically they should agree. Consolidation + a cross-evaluator consistency test would fix this.
The ODE backend's integer-round discretization makes its "deterministic" claim weak for small compartments. Either rename it or round-free it.

Remaining unread: inference subdirectory (~3700 lines — the scientific heart) and CLI (~6600 lines — lower risk but big surface). When you're ready, the inference next is the most important batch — any silent wrongness there directly translates to bad posteriors.


This is a **huge** finding. Claude.md describes EKRNG (Event-Keyed RNG) as the design. `compartmental-ir-spec.md:826–891` is an entire section on EKRNG. The spec says:
- `U_e = g(seed, event_key_e)` with Philox/Threefry counter-based PRNG
- "Placebo Test": executing an irrelevant extra draw must produce bitwise-identical trajectories
- "Scenario Coupling": baseline vs intervention trajectories identical pre-intervention

But the Rust `rng.rs` only has `StatefulRng` wrapping ChaCha8. No EKRNG. Let me verify there isn't an ekrng module I missed:Confirmed. The comment says EKRNG was deliberately skipped. And `ekrng.rs is available if needed` — but it's not present in the codebase (`grep -l ekrng` returns nothing). That's a comment lie. Let me now read the core evaluator.Now the propensity module:
