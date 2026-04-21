---
status: open
date: 2026-04-20
scope: rust/ — design quality pass: type design, DRY, SOLID, dead code, API shape (full codebase; inference layer + sim crate in depth)
reviewer: internal (full codebase sweep post-clap-migration)
---

## Resolution status

| Finding | Status | Notes |
|---------|--------|-------|
| RdM1 — Transform/EstimatedParam in if2.rs | ✅ Resolved | Moved to types.rs; re-exported from if2.rs (2026-04-20) |
| RdM2 — rate_grads name-keyed linear search | ✅ Resolved | rate_grads_indexed in ResolvedModel; resolve_rate_grad_for_run in pgas_grad; hot loop is index-only (2026-04-20) |
| RdM3 — resume state 5 common fields, restore_z_values in pgas | ✅ Partially resolved | restore_z_values moved to types.rs (2026-04-20); struct layout unchanged (bincode compat) |
| RdM4 — log_transition_density_substep 162 lines undecomposed | ✅ Resolved | compute_source_group_probs + exit_and_split_log_density extracted; gamma_idx invariant now in compute_source_group_probs (2026-04-20) |
| Rdm1 — config boilerplate, no shared trait | ✅ Resolved | InferenceConfig trait in traits.rs; impl for IF2Config, PGASConfig, PMMHConfig (2026-04-20) |
| Rdm2 — 1e-300 bare literal | ✅ Resolved | LOG_PROB_FLOOR constant in types.rs; all 8+ sites replaced (2026-04-20) |
| Rdm3 — RNG init DRY | ✅ Resolved | init_particle_rngs in types.rs; used by PF, PGAS, IF2 (2026-04-20) |
| Rdm4 — RESAMPLE_RNG_STREAM seeding | ✅ Resolved | RESAMPLE_RNG_STREAM constant in types.rs (2026-04-20) |
| Rdn1 — primitive obsession in observation streams | Deferred — v1.0 | Newtype sweep is not a correctness concern; deferred until API surface is stable |
| Rdn2 — duplicate lower/upper bounds on EstimatedParam | Open | Too many call sites; skipped per safety review |
| Rdn3 — n_obs/steps_per_obs coupling in PMMHConfig | ✅ Resolved | Fields removed; run_pmmh now takes observations and computes them internally (2026-04-20) |

---

# Rust design quality review — 2026-04-20

Full pass over `rust/crates/{ir,sim,observe,io,cli}/`. Focus areas:
DRY violations in the inference layer, type-design problems in core
simulation primitives, SOLID issues, and any remaining dead code not
caught by the earlier engine and CLI reviews.

**Scope boundaries:** findings in the engine review (`2026-04-19-review-engine.md`)
and CLI review (`2026-04-20-review-cli.md`) are not repeated here even
if they touch the same files. Items Im1–Im9 and RC1–Rn8 from those
reviews are open in parallel. This review addresses orthogonal concerns:
module organisation, shared-type ownership, hot-path data structures,
and long critical functions.

## Summary

**Strong:** The resolved-expression design in `resolved_expr.rs` is the
right abstraction — expressions are compiled to index-offset closures
at model construction, so the simulation inner loop touches no heap
collections. `error.rs` is a proper `thiserror` enum; errors don't
escape as bare `String` from the `sim` crate. `compiled_model.rs`
validates stoichiometry, compartment kinds, and balance constraints at
construction time. The capability-bitflag dispatch in `lib.rs` is clean.

**Needs work:** `EstimatedParam` and `Transform` — two foundational
inference types shared by all three algorithms — are defined in `if2.rs`
and re-imported by PGAS and PMMH with `use super::if2::EstimatedParam`.
If IF2 is ever removed or renamed the whole transform machinery
disappears with it. The gradient evaluator in `pgas_grad.rs` does a
linear `param_names.iter().position(...)` scan on a `Vec<(String,
ResolvedExpr)>` per gradient term per transition per substep — a design
that worked during development but becomes a hot-path O(n_params) lookup
at inference scale. `log_transition_density_substep` is 162 lines of
correctness-critical math with no sub-function decomposition.
`1e-300` appears as a bare literal at eight or more sites across four
files with no constant and no comment explaining the choice.

## Findings

### Major

**RdM1. `EstimatedParam` and `Transform` defined in `if2.rs`, imported
by PGAS and PMMH via `use super::if2`.**

`inference/if2.rs:104–168`:

```rust
pub enum Transform {
    Log { lo: f64, hi: f64 },
    Logit { lo: f64, hi: f64 },
    None,
}

pub struct EstimatedParam {
    pub name: String,
    pub index: usize,
    pub transform: Transform,
    // ...
}

impl EstimatedParam {
    pub fn to_transformed(&self, x: f64) -> f64 { ... }
    pub fn from_transformed(&self, z: f64) -> f64 { ... }
    pub fn log_jacobian(&self, z: f64) -> f64 { ... }
}
```

`inference/pgas.rs:24`:
```rust
use crate::inference::if2::EstimatedParam;
```
`inference/pmmh.rs:21`:
```rust
use super::if2::EstimatedParam;
```

Both algorithms are coupled to `if2.rs` for a type that has nothing
IF2-specific about it. `Transform` and `EstimatedParam` are shared
infrastructure for all inference algorithms. Their three methods
(`to_transformed`, `from_transformed`, `log_jacobian`) form the
canonical contract for parameter-scale management across the entire
inference stack.

The ownership problem: if IF2 is removed, disabled, or renamed, PGAS
and PMMH lose their `Transform` type. The import path makes this look
like a deliberate decision but it is an artifact of IF2 being
implemented first. There is no re-export from `mod.rs` that would make
the public API stable.

A secondary issue: because `log_jacobian` is a method on `EstimatedParam`
(which holds the full parameter spec including bounds), callers that need
only the Jacobian must carry the full struct. `pgas.rs:1451` calls
`if2_params[i].log_jacobian(z[i])` in a tight loop; the struct is used
only for its `transform` field at that call site.

Fix: move `Transform` and `EstimatedParam` (including the three
transform methods) to `inference/types.rs`. Re-export from
`inference/mod.rs` for external use. Update import sites in `if2.rs`,
`pgas.rs`, `pmmh.rs`, `pgas_grad.rs`.

---

**RdM2. `rate_grads: Vec<Vec<(String, ResolvedExpr)>>` uses name-keyed
linear search inside the hot gradient loop.**

`compiled_model.rs:346`:
```rust
pub rate_grads: Vec<Vec<(String, ResolvedExpr)>>,
```

`pgas_grad.rs:99–105`:
```rust
let mut d_rate = vec![0.0; d];
for (name, resolved_grad) in &model.resolved.rate_grads[tr_idx] {
    if let Some(i) = param_names.iter().position(|pn| pn == name) {
        d_rate[i] = eval_resolved(resolved_grad, &ctx) / n_src as f64;
    }
}
```

The same pattern repeats at `pgas_grad.rs:205–210`. This code is called
once per gradient term, per transition, per substep, across all particles
and sweeps. `param_names.iter().position(...)` is O(n_params) per call.
For a model with 8 estimated parameters and 20 transitions, each substep
does up to 160 linear scans. At 100 substeps × 500 particles × 1000
sweeps = 8 billion operations (most short-circuit, but the allocation
pattern is still cache-hostile).

The deeper issue is that `rate_grads` uses `String` keys at all — the
OCaml compiler emits `rate_grad` keyed by parameter name (since it
operates on names), but the Rust runtime has already built a
`param_index_map: HashMap<String, usize>` at `CompiledModel::new` time.
The gradient list could be resolved to param indices once at construction
and stored as `Vec<Vec<(usize, ResolvedExpr)>>`, reducing the hot-loop
lookup to an array index.

A missing-param-name gradient is currently silently ignored
(`if let Some(i) = ...` drops the `None` case). This means if the OCaml
compiler emits a gradient for a parameter that the CLI's `--estimate`
list doesn't include, the gradient is silently zero. No warning, no
error. Whether this is correct semantics needs documentation: is it
intentional that gradients for non-estimated parameters are dropped?

Fix: add a `resolve_rate_grads` step in `CompiledModel::new` (or in the
CLI model-load path just before gradient evaluation begins) that converts
`Vec<Vec<(String, ResolvedExpr)>>` to `Vec<Vec<(usize, ResolvedExpr)>>`
given the set of estimated parameter names. Warn (or error) if a
gradient name is not in the estimated set. Store the resolved form in
`CompiledModel::rate_grads_resolved` alongside the original for
debugging.

---

**RdM3. `ChainResumeState` and `PMMHResumeState` carry five common
fields with no shared type, duplicating the serialisation contract.**

`pgas.rs:164–190` (`ChainResumeState`):
```rust
pub config_hash: String,
pub params: Vec<f64>,
pub transformed: Vec<f64>,
pub param_names: Vec<String>,
pub current_ll: f64,
```

`pmmh.rs:87–114` (`PMMHResumeState`):
```rust
pub config_hash: String,
pub params: Vec<f64>,
pub transformed: Vec<f64>,
pub param_names: Vec<String>,
pub current_ll: f64,
```

The docstrings are near-identical. Five fields, five comments, in two
files. The `config_hash` field has the highest maintenance risk: it is
computed by `compute_config_hash` in the CLI and compared on resume.
If the hashing strategy ever changes (e.g., to include a new field), the
change must be applied consistently to both resume states. The current
design has no type-level enforcement of this.

`param_names` has a known fragility: the comment in `pgas.rs:186–189`
says "Empty for legacy states (before this field was added)." That
backward-compatibility note exists only in PGAS — PMMH has no equivalent
note. If a legacy PMMH resume state is loaded, the empty `param_names`
is a silent failure mode: `pmmh.rs:289–295` uses `param_names` to
reorder parameters on resume, and an empty list would reorder nothing,
giving wrong parameter associations with no diagnostic.

Fix: extract a `BaseResumeState`:
```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct BaseResumeState {
    pub config_hash: String,
    pub params: Vec<f64>,
    pub transformed: Vec<f64>,
    pub param_names: Vec<String>,
    pub current_ll: f64,
}
```
Embed it in both `ChainResumeState` and `PMMHResumeState`. The legacy
`param_names`-empty guard can live on the base struct's `reorder_params`
method, applied consistently by both algorithms. Move to
`inference/types.rs` alongside `EstimatedParam`.

---

**RdM4. `log_transition_density_substep` is 162 lines of correctness-
critical math with no sub-function decomposition.**

`pgas.rs:226–387`:

The function computes the complete-data log-transition density for one
substep of the CSMC-AS backward pass. It is the most scientifically
sensitive function in the codebase — a latent bug here would cause PGAS
to produce incorrect posteriors without any observable runtime error.

The function's logical structure:

1. **Setup** (lines 235–248): resolve compartment count totals, build
   `handled` bitmap.
2. **Per-source-group loop** (lines 252–368): for each source group,
   compute total exit propensity, overdispersion gammas, multinomial
   exit-count density, and conditional split density.
   - Overdispersion section (lines 270–285): collect Gamma densities for
     overdispersed transitions.
   - Exit density section (lines 290–332): NegBinomial density for total
     exits given gammas, or Binomial if no overdispersion.
   - Split density section (lines 334–365): Multinomial density for how
     exits split across destinations.
3. **Remaining transitions** (lines 370–387): deterministic and
   zero-rate transitions (Poisson at zero rate contributes 0 log-prob).

None of these three stages is extracted to a named sub-function. The
`gamma_idx` counter (line 265) is incremented across both the loop setup
and the overdispersion section, making it hard to verify that the index
advances in exactly the same order as `step_one` in
`chain_binomial.rs:246–341` — which it must, or the density is evaluated
at the wrong gamma values.

The cross-function invariant `gamma_idx` tracks (that the PGAS density
evaluator consumes gammas in the same order that `step_one` emits them)
is load-bearing for correctness but is documented only in a comment
(`pgas.rs:265`), not encoded in any type.

Fix: extract at minimum:
```rust
fn gamma_density_for_group(
    model: &CompiledModel, group: &[usize],
    gammas: &[f64], gamma_idx: &mut usize,
    rate: f64,
) -> f64 { ... }

fn exit_log_density(
    total_exits: u64, total_rate: f64,
    overdispersion: Option<f64>,
) -> f64 { ... }

fn split_log_density(
    group: &[usize], flows: &[u64],
    probs: &[(usize, f64)],
) -> f64 { ... }
```
Each sub-function maps to one paragraph of the mathematical derivation,
making the relationship to the paper verifiable by inspection.

---

### Minor

**Rdm1. `n_particles` and `dt` repeated independently in three
inference config structs with no shared type or trait.**

`if2.rs:213–241` (`IF2Config`), `pgas.rs:34–62` (`PGASConfig`),
`pmmh.rs:27–53` (`PMMHConfig`) all have:

```rust
pub n_particles: usize,
pub dt: f64,
```

The fields are not identical in semantics:
- IF2 also carries `t_start: f64` (line 231); PGAS reads `t_start` from
  the model at runtime; PMMH doesn't have `t_start` at all.
- IF2 carries `skip_first_obs_from_loglik: bool` (line 240); the other
  two don't.

This makes a naïve `CommonInferenceConfig` struct impractical — the
fields that are common are few and their interpretations differ. However,
there is a weaker fix: define a trait:
```rust
pub trait InferenceConfig {
    fn n_particles(&self) -> usize;
    fn dt(&self) -> f64;
}
```
and implement it for all three. This enforces that the field names and
types are consistent, and lets shared code (e.g., progress logging,
particle-state sizing, benchmarks) be written generically. It also
documents the guaranteed-shared surface.

---

**Rdm2. `1e-300` appears as a bare literal at eight or more sites across
four files with no named constant and no explanation.**

Occurrences confirmed:
- `if2.rs:68,71,125,200` — barycentric log-transform floor, log-transform
  clamp, perturbation SD floor.
- `pgas.rs:505` — Gamma log-density floor.
- `obs_model.rs:144,230,231,280` — Bernoulli, BetaBinomial log-density
  floors.
- `correlated_pf.rs:132` — underflow guard in binomial quantile loop.

All uses mean the same thing: "replace 0 (or negative) with a floor
value small enough that `ln(floor)` ≈ −690 rather than −∞, so
log-weights remain finite." The choice of `1e-300` is not arbitrary —
it is approximately `f64::MIN_POSITIVE` (5×10⁻³²⁴) divided by roughly
10⁻²⁴, giving a log-probability of about −690, safely above the
log-sum-exp underflow threshold for any realistic particle count. But
none of this is written down anywhere.

If someone edits one site to `1e-15` (thinking "very small is fine"),
they silently change the effective log-probability floor from −690 to
−34 — which could bias inference when extreme particles are supposed to
have negligible but non-zero weight. There is no test that catches this.

Fix: define in `inference/types.rs`:
```rust
/// Floor for ln() arguments to avoid −∞ log-weights.
/// ≈ f64::MIN_POSITIVE × 1e24 → ln ≈ −690, safely above
/// the underflow threshold for any realistic particle count.
/// Do not lower below f64::MIN_POSITIVE (5e-324).
pub const LOG_PROB_FLOOR: f64 = 1e-300;
```
Replace all eight sites. Add a test asserting
`LOG_PROB_FLOOR.ln() < -600.0`.

---

**Rdm3. Particle RNG initialisation pattern duplicated with a subtle
variant in IF2.**

`particle_filter.rs:87–89`:
```rust
let mut rngs: Vec<StatefulRng> = (0..n)
    .map(|i| StatefulRng::new_stream(seed, i as u64))
    .collect();
```

`if2.rs:417–420`:
```rust
let rngs: Vec<StatefulRng> = (0..n)
    .map(|i| StatefulRng::new_stream(seed, stream_base | (i as u64)))
    .collect();
```

`pgas.rs:657–659`:
```rust
let mut rngs: Vec<StatefulRng> = (0..n_particles)
    .map(|i| StatefulRng::new_stream(seed, i as u64))
    .collect();
```

The IF2 variant uses `stream_base | i` where `stream_base` is a
per-iteration offset (`(iter as u64) << 32`). This is the correct design
for IF2's iteration-level RNG separation but differs from the PF and PGAS
forms, which use bare particle index. A reader comparing the three
implementations has to reason carefully about whether the difference is
intentional — it is, but it's not obvious.

This is related to `IM1` in the inference review (which flagged the
earlier XOR seeding as incorrect and is now fixed). The `new_stream`
forms are sound. The issue here is purely DRY: the intent and variant
should be documented, and if a fourth algorithm is added it's not clear
which form to follow.

Fix: add a helper to `inference/types.rs`:
```rust
/// Initialize per-particle RNG streams from a base seed and
/// an optional per-iteration stream offset (IF2) or zero (PF/PGAS).
pub fn init_particle_rngs(
    seed: u64,
    n: usize,
    stream_offset: u64,
) -> Vec<StatefulRng> {
    (0..n)
        .map(|i| StatefulRng::new_stream(seed, stream_offset | (i as u64)))
        .collect()
}
```
PF and PGAS call with `stream_offset = 0`; IF2 passes its iteration-
based offset. The difference is visible and intentional at every call
site.

---

**Rdm4. `PMMHConfig` carries `n_obs` and `steps_per_obs` as explicit
fields — values derivable from the observation model and config.**

`pmmh.rs:49–52`:
```rust
/// Number of observations (for sizing PFRandomState).
pub n_obs: usize,
/// Substeps per observation interval (= obs_spacing / dt).
pub steps_per_obs: usize,
```

Both fields are computed by the CLI driver and passed into `PMMHConfig`.
They are not user-facing settings — they are consequences of `dt` and
the observation data. This creates a coupling where the CLI must know
internal sizing details of the PMMH implementation. If `PFRandomState`
is later refactored to size itself lazily, the CLI still passes these
fields (and they become silently ignored).

Compare `PGASConfig`, which does not carry `n_obs` or `steps_per_obs`
— PGAS computes both internally from `model` and `obs_model` at
algorithm start (`pgas.rs:437, 576`). The asymmetry is unintentional.

Fix: remove `n_obs` and `steps_per_obs` from `PMMHConfig`. Let the PMMH
algorithm derive them from `obs_model.n_observations()` and
`(obs_spacing / config.dt).round() as usize` at entry, exactly as PGAS
does. Update the CLI call site to drop those fields.

---

### Nit

**Rdn1. Primitive obsession: `t_start: f64` and `dt: f64` in three
inference configs with no newtype guards.**

`IF2Config:229–231`, `PGASConfig:39`, `PMMHConfig:30`. Time and
time-step are raw `f64`. A caller passing `dt` where `t_start` is
expected (or vice versa) gets no compile-time feedback. The codebase
is careful about naming but not type-safe.

Not a correctness concern today — the naming convention is consistent
and the values are not interchangeable in practice. Worth a newtype
sweep before v1.0:
```rust
pub struct SimTime(pub f64);
pub struct TimeDelta(pub f64);
```

---

**Rdn2. `EstimatedParam` carries `lower: f64` and `upper: f64`
redundantly alongside `transform: Transform { lo, hi }`.**

`if2.rs:97–101`:
```rust
pub struct EstimatedParam {
    pub name: String,
    pub index: usize,
    pub lower: f64,
    pub upper: f64,
    pub transform: Transform,
    // ...
}
```

`Transform::Log { lo, hi }` and `Transform::Logit { lo, hi }` already
carry the bounds. `lower` and `upper` are only distinct from the
transform bounds when `transform == Transform::None` (no clamping
applies). For `None`, `lower` and `upper` are unused in the current
transform methods. This creates a two-source-of-truth situation: if
the CLI sets `lower=0.01` but `transform = Transform::Log { lo: 0.001,
hi: 10.0 }`, the clamping behaviour in `from_transformed` uses `lo`
not `lower`. Whether these are always in sync is enforced only by the
CLI construction logic, not by the type.

Fix: either (a) derive `lower` / `upper` from the `Transform` variant
and remove the duplicate fields, or (b) document clearly that
`lower`/`upper` on the struct are the *user-visible* bounds and the
`lo`/`hi` inside `Transform` are the *clamping* bounds (and enforce
`lo == lower`, `hi == upper` at construction time).

---

**Rdn3. `resample_rng` seeded with a bare XOR constant in PF and
PGAS even after the IM1 fix.**

`particle_filter.rs:123`:
```rust
let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));
```

`pgas.rs:717`:
```rust
let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));
```

The same magic constant in both files. The IM1 fix (per-particle streams)
correctly separated particle RNGs, but `resample_rng` still uses the old
XOR-with-constant pattern. The constant `0xdeadbeef` is not documented
(why this value? what property does it give?). Using `new_stream` with
a reserved high stream index (e.g., `StatefulRng::new_stream(seed,
u64::MAX)`) would be consistent with the new pattern and self-documenting.

---

## Cross-cutting themes

**1. Core inference types live in the wrong module.**
`EstimatedParam`, `Transform`, and the resume state common fields are
all in algorithm-specific files. The actual shared infrastructure module
`inference/types.rs` defines `InferenceResult`, `ParamSpec`,
`EstimateInit`, and a handful of utility types — but not the types used
most widely. Moving `EstimatedParam` and `Transform` to `types.rs`
(RdM1) and extracting `BaseResumeState` (RdM3) would make the module
structure reflect the actual dependency graph.

**2. Hot-path data structures chosen for construction convenience, not
evaluation performance.**
`rate_grads: Vec<Vec<(String, ResolvedExpr)>>` (RdM2) and
`resample_rng` (Rdn3) both reflect what is convenient to build (name
strings from the OCaml compiler, a scalar seed) rather than what is
fast to evaluate. `resolved_expr.rs` already proves the team knows how
to do this correctly — the gradient store just hasn't received the same
treatment yet.

**3. Magic constants without a single source of truth.**
`1e-300` (Rdm2) and `0xdeadbeef` (Rdn3) appear in multiple files
without constants or comments. Each represents a deliberate numerical
choice that should be documented and centralised. In scientific software
these choices can affect inference quality; their rationale belongs in
the code.

**4. Correctness-critical functions that exceed reviewable size.**
`log_transition_density_substep` at 162 lines (RdM4) is the clearest
case. The function is correct as far as can be verified, but the
verification itself is harder than it should be because the mathematical
structure (three stages: overdispersion, exit, split) is not reflected
in the code structure. For software informing public health decisions,
the relationship between code and derivation should be as transparent as
possible.
