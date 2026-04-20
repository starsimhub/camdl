---
status: addressed (pending inference sub-review)
date: 2026-04-19
scope: rust/ engine subsystem — ir crate, sim crate (foundations + backends + intervention), plus partial inference pass
reviewer: external (via `scripts/review-zip.sh engine`)
---

## Resolution status

**Addressed:**
- RC1 — `ir::validate::validate` now runs on every model load in
  `cli/src/util.rs::load_model` and `run_simulation`.
- RC2 — EKRNG references scrubbed from specs, CLAUDE.md, code
  comments; dead `event_key` IR field removed cross-language.
- RC3 — `InitialConditions::FromDistribution` now hard-errors
  instead of silently zero-initializing.
- RM1 — `tau_leap` now uses Euler-multinomial for shared-source
  transitions (mirrors chain_binomial).
- RM2 — new `eval_stats` module with atomic counters on every
  silent-fallback site (Div/Pow/UnOp/NegBinomial/Binomial).
- RM3 — `TableLookup` `OobPolicy::Error` now panics in the hot
  path instead of log-and-clamp.
- RM6 — `eval_table_expr` gained the same Pow / UnOp domain
  guards as `eval_expr` / `eval_resolved`.
- RM7 — `ode_integrator::rk4_step` caps at `RK4_DT_MAX = 0.5` and
  sub-steps longer gaps.
- RM8 — ODE backend uses fractional integer state during RK4
  substeps via `EvalCtx::int_float_override`. Rounding only at
  snapshot time.
- RM10 — gillespie/tau_leap `debug_assert`s now check pre-clamp
  state via the clamp return value.
- Rm4 — `apply_interventions_at` rejects non-finite t.
- Rm5 — unreplaced `TableSource::External` errors at
  `CompiledModel::new` instead of panicking downstream.
- Rm8 — `fire_steps` uses `BTreeSet<i64>` for deterministic
  iteration.
- Rn2 — `map_or(false, ...)` → `is_ok_and(...)` in trace_enabled.

**Confirmed not a bug after sanity check:**
- Rm2 — `RATE_EPSILON` already shared via `pub const` imported by
  `inference/pgas.rs`; no drift risk.
- Rm3 — `apply_interventions_at` using `model.simulation.dt` is
  consistent with how `fire_steps` is built; safe.

**Deferred / low priority:**
- RM4, RM5 — rng fallbacks are IF2-specific defensive code; left
  alone (now at least observable via `eval_stats`).
- RE1–RE5 — serde nits around untagged enums, `Option` fields
  without `#[serde(default)]`, version string. Not bugs; papercuts.
- Rm1, Rm6, Rm7, Rm9 — trace-loop OnceLock, FlowVec bounds check,
  balance-negative warn, Gillespie linear event selection. Nits /
  perf work, not correctness.
- Rn1, Rn3, Rn4, Rn6, Rn7, Rn8 — documentation, idiomatic-Rust,
  width, and wrapper-struct nits.

**Still unread (separate review needed):**
- Inference subdirectory (~3700 lines): `particle_filter.rs`,
  `if2.rs`, `obs_loglik.rs`, `pgas.rs` (1718 lines),
  `pgas_grad.rs`, `nuts.rs`, `pmmh.rs`, `correlated_pf.rs`,
  `multi_stream_obs.rs`, `prior.rs`. Scientific-correctness
  central. Highest remaining risk surface.
- CLI (~6600 lines): `main.rs`, `batch.rs`, `util.rs`, `browse.rs`
  plus hashing / cas / run_meta. Lower scientific risk.

# Engine code review — 2026-04-19

Companion to the same-day compiler review. Scope covered:

- All of `rust/crates/ir/` (~1K lines): `lib.rs`, `model.rs`, `expr.rs`,
  `validate.rs`, `parameter.rs`, `table.rs`, `transition.rs`.
- All of `rust/crates/sim/src/` foundations: `lib.rs`, `config.rs`,
  `error.rs`, `state.rs`, `rng.rs`, `resolved_expr.rs`,
  `propensity.rs`, `compiled_model.rs`.
- All backends: `gillespie.rs`, `chain_binomial.rs`, `tau_leap.rs`,
  `ode.rs`, `ode_integrator.rs`, `intervention.rs`, `simulate.rs`,
  `output.rs`, `transition_diagnostics.rs`.
- Inference module structure only (top-level `mod.rs`). The inference
  subdirectory proper (~3700 lines) and the CLI (~6600 lines) are
  not yet covered.

## Summary

**Strong:** IR deserialization is structurally sound — the serde
discriminants are unique enough that untagged enums disambiguate
cleanly. `chain_binomial.rs` correctly implements Euler-multinomial
competing-risks sampling after an earlier ESS-degradation fix.
`resolved_expr.rs` has a well-factored two-tier evaluator
(construction-time vs hot-path). `CompiledModel::new` does sensible
local integrity checks.

**Weak:** `ir::validate::validate` is defined but never called —
same pattern as the OCaml side (M1 in the compiler review). The
runtime has a pervasive silent-fallback style (Div-by-zero → 0,
NaN → 0, Pow-overflow → 0, TableLookup OOB → clamp, Binomial
fallback, NegBinomial fallback to Poisson) that turns
scientific-correctness failures into logged warnings at best.

**Alarming:** EKRNG was specified in detail in the language and run
specs and referenced in code comments as if it existed, but was
never implemented — that design was abandoned in favor of a plain
ChaCha8 stateful PRNG. The spec sections and comments are stale and
should be scrubbed.

## Findings

### Critical

**RC1. `ir::validate::validate` is never called from the runtime.**
`grep -rn "validate::validate\|ir::validate\|validate(" crates/sim/src crates/cli/src`
returns nothing. This mirrors the OCaml-side M1 (`Validate.validate`
also uncalled). Both sides have thorough integrity checkers; neither
runs them. Combined with every OCaml-side silent-fallback bug (C1
overdispersed→Poisson, C2 dim_value_index→0, C3 stoich base-name,
C5 ODE dropped, C6 empty actions, C7 "?" compartments), the Rust
runtime has no structural safety net between "compiler emitted
garbage" and "simulation proceeds to give wrong answers."
`CompiledModel::new` does some local checks (unknown compartment
`compiled_model.rs:404`, unknown ODE compartment line 457,
real-compartment-in-stoich line 408) but these are partial.
`validate.rs` has a complete integrity battery. Wiring
`ir::validate::validate(&model)?` into the CLI model-load path is a
one-line fix that turns every silent bug into a loud one.

**RC2. EKRNG references are stale and should be removed.** EKRNG
(event-keyed counter-based PRNG with Philox/Threefry, placebo-test
invariant, scenario-coupling guarantee) was an earlier design that
was abandoned in favor of the current plain ChaCha8 `StatefulRng`.
The spec sections, code comments, and a golden fixture still
reference it as if it existed:

- `docs/compartmental-ir-spec.md:826–891` — entire section
  specifying EKRNG.
- `docs/camdl-run-spec.md`, `docs/camdl-language-spec.md` — scattered
  mentions.
- `rust/crates/sim/src/rng.rs:13` — comment "Use a different
  derivation than EkRng so seeds don't collide."
- `rust/crates/sim/src/gillespie.rs:78` — "(ekrng.rs is available if
  needed)".
- `ir/golden/sir_placebo_ekrng.ir.json` — fixture named for the
  abandoned "placebo test" invariant.

The CRN claim in `gillespie.rs:72–78` ("same seed → identical
trajectories pre-intervention") relies on sequential-draw
coincidence from the shared stateful state. This holds only when
both scenarios consume the RNG in bitwise-identical order; any edit
that reorders draws breaks it. Docs should describe that narrower
guarantee explicitly rather than the stronger EKRNG one.

Fix: strip EKRNG sections from the three specs, replace code
comments with accurate descriptions of the current ChaCha8
behavior, optionally rename the `sir_placebo_ekrng` fixture if it
reads as misleading. Same pass should sweep `CLAUDE.md` for any
EKRNG wording.

**RC3. `InitialConditions::FromDistribution` is silently treated as
"use zero defaults."** `compiled_model.rs:711–713`:

```rust
InitialConditions::FromDistribution(_) => {
    // Not supported in sim at runtime; use default zeros
}
```

If the OCaml compiler ever emits `"initial_conditions":
{"from_distribution": {...}}` (the serde type is defined in
`ir/model.rs:38`), the Rust runtime starts all compartments at 0
and doesn't tell anyone. Another silent-wrong-answer primitive. At
minimum return `Err(SimError::Validation("FromDistribution initial
conditions not yet supported in sim"))`. Ideally wire the draw into
the inference-side prior sampling path.

### Major

**RM1. `tau_leap.rs` uses independent Poisson draws per transition
with no competing-risks protection.** `tau_leap.rs:125–145`:

```rust
for (i, &lambda) in propensities.iter().enumerate() {
    let mean = lambda * dt;
    let count = match draws[i] {
        ResolvedDraw::Poisson => rng.poisson(mean),
        ...
    };
    for &(local, delta) in &model.transition_stoich[i] {
        int_s.counts[local] += delta * count as i64;
    }
}
let clamped = int_s.clamp_nonneg();
```

If source compartment S has 100 individuals and two competing exits
(S→E at rate β·I, S→V at rate ν), tau-leap may draw 80 infections
and 50 vaccinations — 130 exits from 100 people. `clamp_nonneg`
silently converts this to 0 at the source, but the "extra" 30
people are now phantom additions at the destinations E and V. Total
population is not conserved.

`chain_binomial.rs:246–341` correctly uses Euler-multinomial
precisely to avoid this (same reason `pomp::reulermultinom`
exists). The comment at chain_binomial lines 253–257 even warns
about it: "The old algorithm systematically over-counted total
exits, causing particle trajectories to drift and ESS to degrade."
That fix was applied to chain-binomial but not ported to tau-leap.

Users who pick `SimConfig::TauLeap` expecting "chain-binomial with
different noise" get an unsafe approximation for any realistic epi
model (SEIR E→I vs E→dies, SIR + vaccination, etc.).

Fix: port the `source_groups` multinomial logic from chain_binomial
into tau_leap, or mark tau_leap unsafe pending that work.

**RM2. Pervasive silent-fallback in expression evaluation.**
- `propensity.rs:72–85` — Div-by-zero → 0.0 with `log::debug`.
- `propensity.rs` — Pow producing NaN/Inf → 0.0 with `log::warn`.
- `resolved_expr.rs:244` — hot-path Div-by-zero → 0.0 with **no
  log at all** (the logging was dropped from the eval_expr → eval_resolved
  migration).
- `rng.rs` — multiple degenerate-param fallbacks (see RM4, RM5).

For inference runs with millions of steps, logs are either ignored
(default log level) or a firehose. A rate that hits Pow-overflow
silently becomes 0 for an entire trajectory; ESS degrades for
reasons the user cannot diagnose.

Note `SimError::DivisionByZero` exists (`error.rs:21`) but is
effectively unused. `eval_resolved` is infallible by design (returns
`f64`, not `Result`), so any error has to be swallowed or panic —
the team chose swallow.

Fix options:
- Add an `eval_resolved_checked` variant that returns
  `Result<f64, SimError>`. Use it in inference paths where
  correctness is load-bearing.
- Reserve the infallible variant for tight hot loops that have
  asserted a precondition.

**RM3. `TableLookup` `OobPolicy::Error` silently clamps in the hot
path.** `resolved_expr.rs:288–314`:

```rust
OobPolicy::Error => {
    // Defensive: clamp instead of panic. The Error policy
    // means the model author wanted strict bounds, but in the
    // resolved hot path we can't return Result.
    if table_idx_val < 0 || table_idx_val >= n {
        log::warn!("resolved table lookup: index {} out of bounds …");
        table_idx_val.clamp(0, n - 1)
    } else { table_idx_val }
}
```

The `Error` policy exists specifically because the author asked for
strict bounds. The hot-path evaluator defeats this with log+clamp.
`propensity.rs:234–240` (slow `eval_expr` path) correctly returns
`SimError::TableLookup(…)`. `compiled_model.rs::eval_table_expr` also
errors. Three evaluators, three different policies for the same
input.

Fix: add a checked variant (see RM2), or pre-validate all table
index expressions at `CompiledModel::new` time to guarantee they
can't produce OOB under any explored param vector.

**RM4. `rng::binomial` has a fallback that is effectively dead
code.** `rng.rs:88–94`:

```rust
match Binomial::new(n, p.clamp(0.0, 1.0)) {
    Ok(b) => b.sample(&mut self.0),
    Err(_) => if p > 0.5 { n } else { 0 },
}
```

`p` is clamped to `[0, 1]` immediately before, so `Binomial::new`
can't fail from `p`. `n: u64` can't be negative. The error arm only
fires on `n == 0` — in which case the fallback "return n if p > 0.5
else 0" always returns 0, which is correct. So the fallback is
either (a) dead code guarding against something that can't happen,
or (b) a paper over an actual rust-rand edge case I haven't found.

The comment at line 86 defends this as "particles with extreme
parameters get -inf loglik and are resampled away" — an IF2-specific
rationale that doesn't apply elsewhere. Either document why the
fallback is load-bearing, or remove it and let `Binomial::new` panic
if ever hit.

**RM5. `rng::neg_binomial` falls back to Poisson on degenerate
shape.** `rng.rs:41–55`. The comment at lines 44–48 justifies it for
IF2 ("shape < 1e-6 means sigma_sq >> dt; the Gamma is degenerate;
IF2 will push sigma_se away"). Same concern as RM4: this defense is
specific to IF2. In pure simulation mode, or PGAS/PMMH exploration,
a particle hitting sigma_sq >> dt silently downgrades to Poisson
noise, giving an inflated likelihood at those params compared to
the true generative model. The posterior's rejection of extreme
sigma_sq should come from the data likelihood, not a RNG fallback.

**RM6. Three expression evaluators drift on edge cases.**
`eval_expr` (`propensity.rs`), `eval_resolved` (`resolved_expr.rs`),
and `eval_table_expr` (`compiled_model.rs:185–233`) all evaluate
the same AST but disagree on boundary behavior:

| edge case       | eval_expr          | eval_resolved      | eval_table_expr    |
|-----------------|--------------------|--------------------|--------------------|
| Div by zero     | 0, `log::debug`    | 0, no log          | 0, no log          |
| Pow NaN/Inf     | 0, `log::warn`     | 0, no log          | raw `powf` (NaN)   |
| TableLookup OOB | `SimError`         | log + clamp        | `SimError`         |

Plus `eval_table_expr` uses `Pow => a.powf(b)` raw with no guard —
so inline-table computations with Pow can produce NaN that enters
`table_values_cache` and is read back at simulation time.

The comment at `compiled_model.rs:182–184` says "BinOp/UnOp arms
MUST match the semantics in eval_expr — if a new operator is added
there, it must be added here too." This is exactly the maintenance
landmine the code warns about, but the existing three arms already
disagree.

Fix: consolidate to a single evaluator with a restricted-context
mode, or at minimum add a cross-evaluator consistency test.

**RM7. Gillespie advances real compartments with a single RK4 step
over the event gap.** `gillespie.rs:161–167`. If `t_next` is far
from the current event time (Gillespie gaps are unbounded), RK4
accuracy degrades as step size grows. For models with stiff real
compartments (environmental reservoirs with fast decay + slow
accumulation), Gillespie+real-compartments is numerically unsafe.

The TODO at line 163 acknowledges this: "replace with PDMP thinning
for real compartments." Until then, adaptive stepping or a cap at
`dt_max` would be a stopgap.

**RM8. ODE backend rounds integer compartments inside each RK4
substep.** `ode.rs:55–57`:

```rust
int_vals.iter().map(|&x| x.max(0.0).round() as i64)
```

Rounding inside the integrator quantizes integer state; for small
N the quantization dominates. For S=100 with rate β·S, a substep
puts S at 100.49, rounding gives β·100, and the next substep does
the same. The comment at lines 42–45 acknowledges this: "O(1/N)
relative error … premature extinction for very small compartment
values (< ~10)." That's honest, but the backend is registered as
"ODE" alongside the stochastic ones, and users expect a pure ODE
run.

Fix: integrate integer compartments as real-valued within the RK4,
round only at snapshot time. Or rename to
`DeterministicApproximation`.

**RM9. `debug_assert!`s after `clamp_nonneg` provide false
confidence.** `gillespie.rs:247–250`, `tau_leap.rs:145–148`,
similar in chain_binomial. The clamp happens first, then the assert
tests "did the clamp work" — not "was the raw state valid." Any
undetected negativity from a backend bug silently passes these
asserts.

### Minor

**RE1. `Expr` is `#[serde(untagged)]` — load-time parse cost scales
with variant count.** Load happens once so not a sim-performance
issue, but CLI tools that re-parse IR (pipelines chaining many
invocations) pay the 11-variant-per-object cost. Worth measuring
before any perf optimization work.

**RE2. `Expr::Projected` in `validate.rs:171–177` has a commented-
out allow_projected gate that is never enforced.** The `// We don't
emit an error here currently; the schema validator handles it`
comment refers to a schema validator that doesn't exist at load
time. If a user writes `rate = projected * 10` in a transition (not
a likelihood), the Rust validator silently accepts it and downstream
eval will crash or produce garbage.

**RE3. `Parameter::{value, transform, initial_value}` and
`Transition::event_key` lack `#[serde(default)]`.** Hand-written IR
missing these fields fails to load. Fine for machine-generated IR,
worth documenting as "you must emit these explicitly, even as
null."

**RE4. `Preset::compose: Vec<String>` is parsed but its runtime use
unclear.** `model.rs:90–91`. Need to read scenario-application code
in the CLI to confirm whether it's dead data. Flagged pending
follow-up.

**RE5. No version check at `Model` load.** `model.rs:133`. OCaml
emits `"0.3"` unconditionally. A future bump would be silently
accepted. Per "backwards compatibility is a non-goal" this is fine
today — but a `Err` on mismatch would fail loud when a stale runtime
meets a newer IR.

**Rm1. `chain_binomial.rs:369–392` uses `static HEADER: OnceLock<bool>`
inside the trace loop.** The `OnceLock` read is paid every step even
when tracing is off (guarded by `trace_enabled()` earlier, so it's
only paid when trace is on — fine, just noisy).

**Rm2. `chain_binomial.rs:22` — `pub const RATE_EPSILON: f64 =
1e-15`** must match `log_transition_density_substep` in the inference
module. No test enforces this; if someone edits one constant, the
simulation and density diverge silently. Add an
`#[test] fn density_epsilon_matches_step_epsilon()`.

**Rm3. `intervention.rs:80–81` — `current_step = (t_end / dt).round()
as i64` uses `model.simulation.dt`, not the runtime dt passed via
config.** `chain_binomial.rs:135` uses `cfg.dt.min(…)` which may
differ (e.g., runner refines `dt=0.5` on a model declared `dt=1.0`).
If they diverge, intervention step index is wrong and interventions
fire at the wrong time. Fix: pass the runtime dt into
`apply_interventions_at`.

**Rm4. `intervention.rs:47` — `(t / dt).round() as i64` truncates
silently on NaN.** `NaN as i64` is 0 on current Rust. A guard would
catch upstream bugs.

**Rm5. `compiled_model.rs:476–483` — unreplaced `TableSource::External`
doesn't error; `eval_resolved::TableLookup` panics (index out of
bounds) on the empty cached vec.** Spec promises an error; code
delivers a panic. Explicit check at compile time that every
External has been replaced.

**Rm6. `state.rs:88 — FlowVec::add` has no bounds check.** Low risk
(callers iterate in-bounds), noted.

**Rm7. `chain_binomial.rs:431` — balance target going negative emits
`log::warn` and continues.** Correct for inference ("particle filter
penalizes bad trajectories") but confusing for pure simulation runs
where negative counts appear in the output TSV. A config flag could
distinguish.

**Rm8. `compiled_model.rs:304 — fire_steps: Vec<HashSet<i64>>`** uses
a randomized-hash HashSet. Iteration order is nondeterministic;
callers only use `.contains()` so behavior is deterministic. For
reproducibility auditing, `BTreeSet` or sorted `Vec` + binary search
is cleaner.

**Rm9. `gillespie.rs:211–220` — linear search through cumulative
propensities for event selection.** Fine for small models; for
>100 transitions this becomes a hot-path bottleneck. Walker alias
amortizes to O(1).

### Nits

**Rn1. `rng.rs:79–87` docstring mentions "inverse CDF" but the
fallback is a step function `if p > 0.5 { n } else { 0 }`.** The
inverse-CDF path is in `correlated_pf::binomial_quantile` — a
separate function. Docstring is misleading.

**Rn2. `chain_binomial.rs:198 — map_or(false, |v| v == "1")`** could
be `.is_ok_and(|v| v == "1")`. Clippy nit.

**Rn3. `config.rs:36–38 — SimConfig::variant_name()`** used only in
error messages. `Display + Debug` derives would cover.

**Rn4. `compiled_model.rs:474 — let vals: Result<Vec<f64>, SimError>
= ...collect()`** — idiomatic Rust uses `?` directly on `collect`.

**Rn5. `gillespie.rs:78`** — "ekrng.rs is available if needed"
comment is inaccurate (no such file). Remove or update (see RC2).

**Rn6. `ir::StoichiometryEntry(pub String, pub i64)`** — width
disparity vs OCaml's 31/63-bit `int`. Deltas are ±1 in practice;
nit only.

**Rn7. `Model::version: String`** — stringly-typed, no parsing into
a `semver::Version`. Low priority.

**Rn8. `BinOpWrap`/`UnOpWrap` wrapper structs** for the
`#[serde(untagged)]` enum discriminator. Correct, documented at
`expr.rs:137`, just noisy at AST-walk sites.

## Cross-cutting themes

1. **Silent-fallback pattern on both sides of the compile/run
   boundary.** OCaml expander silently falls back to Poisson / "?" /
   0.0 (compiler review §2–3). Rust runtime silently falls back on
   division, NaN, Pow overflow, table bounds, extreme parameter
   degenerates. Neither side calls its `Validate` module. Together:
   compiler emits silently-wrong IR, runtime silently consumes it,
   inference silently converges to the wrong posterior.
2. **EKRNG is stale doc debt, not a missing feature.** The spec
   still describes an abandoned PRNG design. Scrub the spec and
   code comments so the documented contract matches what the
   stateful ChaCha8 PRNG actually provides.
3. **Tau-leap has a correctness bug that chain-binomial fixed.**
   Multinomial competing-risks needs to be applied to tau-leap.
4. **The three expression evaluators drift.** `eval_expr`,
   `eval_resolved`, `eval_table_expr` have different edge-case
   behaviors that should agree. Consolidation + cross-evaluator
   consistency test.
5. **ODE backend's integer-round discretization is neither fish nor
   fowl.** Quantizes integer state during "deterministic"
   integration; the backend's name implies a pure ODE the code
   doesn't deliver.

## Not yet covered

- Inference subdirectory (~3700 lines): `particle_filter.rs`,
  `if2.rs`, `obs_loglik.rs`, `pgas.rs` (1718 lines), `pgas_grad.rs`,
  `nuts.rs`, `pmmh.rs`, `correlated_pf.rs`, `multi_stream_obs.rs`,
  `prior.rs`. Scientific-correctness central; next batch priority.
  One initial finding flagged in-progress: CSMC-AS ancestor
  sampling in `pgas.rs` needs a closer read — the construction does
  not obviously respect the correct conditional-path marginal.
- CLI (~6600 lines): `main.rs`, `batch.rs`, `util.rs`, `browse.rs`
  plus hashing / cas / run_meta. Lower scientific risk; mostly
  flag-parsing and orchestration.
- Tests: 20 test files in `sim/tests/`, 3 in `cli/tests/`.
