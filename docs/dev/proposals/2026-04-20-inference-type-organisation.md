---
status: partially-implemented
date: 2026-04-20
implemented: 2026-04-20
deferred:
  - R2 (BaseResumeState extraction): bincode v1 serialises fields in declaration order;
    embedding changes field ordering and would silently corrupt existing resume checkpoints.
    Deferred until bincode is replaced or a migration path is available.
  - R3 (remove duplicate lower/upper on EstimatedParam): too many call sites across pgas.rs
    and CLI construction paths; deferred to v1.0 cleanup pass alongside Rdn2.
---

# Inference type organisation — move shared types out of `if2.rs`

Companion to `docs/dev/reviews/2026-04-20-review-rust-design.md`
(RdM1, RdM3, Rdn2). Three related changes that move foundational
inference types from algorithm-specific files to the module where
they belong.

---

## R1. Move `Transform` and `EstimatedParam` to `inference/types.rs`

**Problem:** `Transform` and `EstimatedParam` are defined in
`inference/if2.rs:104–168` but are the shared representation of
parameter-scale management for every inference algorithm. Both PGAS
and PMMH import them via `use crate::inference::if2::EstimatedParam`
and `use super::if2::EstimatedParam` respectively. If IF2 is removed
or refactored, the type definition disappears from under its dependents.
The `impl EstimatedParam` block — `to_transformed`, `from_transformed`,
`log_jacobian` — is the canonical transform contract for the entire
inference stack, but its home gives no hint of that.

**Proposed change:** Move the following from `if2.rs` to `types.rs`:

```rust
// ── types.rs ──────────────────────────────────────────────────────

/// The unconstrained-space transform applied to an estimated parameter.
/// Matches Stan's lower/upper bounded parameter conventions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Transform {
    /// log transform with bounds clamping on inverse.
    /// Correct for rates, positive quantities, counts.
    Log { lo: f64, hi: f64 },
    /// Scaled logit mapping [lo, hi] → (−∞, +∞).
    /// Correct for probabilities and fractions.
    Logit { lo: f64, hi: f64 },
    /// No transform. For unconstrained real parameters.
    None,
}

/// A single estimated parameter: its name, position in the
/// full model parameter vector, declared transform and bounds,
/// and per-algorithm adaptation state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EstimatedParam {
    pub name: String,
    pub index: usize,
    pub transform: Transform,
    pub lower: f64,
    pub upper: f64,
    pub rw_sd: f64,
}

impl EstimatedParam {
    /// Map a natural-scale value to the unconstrained (z) scale.
    pub fn to_transformed(&self, x: f64) -> f64 { ... }

    /// Map an unconstrained value back to the natural scale.
    pub fn from_transformed(&self, z: f64) -> f64 { ... }

    /// log |dθ/dz| — required for the MH ratio when proposing on z.
    pub fn log_jacobian(&self, z: f64) -> f64 { ... }
}
```

Update imports:
- `if2.rs`: remove the definition; add `use super::types::{Transform,
  EstimatedParam};`.
- `pgas.rs`: change `use crate::inference::if2::EstimatedParam;` →
  `use crate::inference::types::EstimatedParam;`.
- `pmmh.rs`: change `use super::if2::EstimatedParam;` →
  `use super::types::EstimatedParam;`.
- `pgas_grad.rs`: same.

Re-export from `inference/mod.rs` if downstream CLI crates import them:
```rust
pub use types::{Transform, EstimatedParam};
```

**Scope:** Four import-site changes plus the move itself. No logic
changes; tests pass without modification.

---

## R2. Extract `BaseResumeState` from `ChainResumeState` and `PMMHResumeState`

**Problem:** Both resume state structs carry the same five fields with
near-identical docstrings:

`pgas.rs:164–189`:
```rust
pub config_hash: String,
pub params: Vec<f64>,
pub transformed: Vec<f64>,
pub param_names: Vec<String>,
pub current_ll: f64,
```

`pmmh.rs:87–100`:
```rust
pub config_hash: String,
pub params: Vec<f64>,
pub transformed: Vec<f64>,
pub param_names: Vec<String>,
pub current_ll: f64,
```

The `config_hash` field is the critical one: it is computed by
`compute_config_hash` in the CLI, compared on resume, and if the hash
strategy ever changes (adding a new field, changing serialisation),
both structs must be updated together. Currently nothing enforces this.

The `param_names` field has an additional hazard: `pgas.rs:186–189`
documents "Empty for legacy states (before this field was added)" and
the resume logic handles this. PMMH has the same field but no such
guard — a legacy PMMH resume state with an empty `param_names` would
silently reorder parameters incorrectly on resume.

**Proposed change:** Extract to `inference/types.rs`:

```rust
/// Fields shared by every algorithm's resume checkpoint.
/// Serialised to `chain_N/resume_state.bin` via bincode.
#[derive(Clone, Serialize, Deserialize)]
pub struct BaseResumeState {
    /// Identifies the statistical problem; resume is rejected if this
    /// differs from the current config hash. Computed by the CLI's
    /// `compute_config_hash`.
    pub config_hash: String,
    /// Current natural-scale parameter values (full model param vector).
    pub params: Vec<f64>,
    /// Current unconstrained-scale values (one per estimated param).
    pub transformed: Vec<f64>,
    /// Estimated parameter names in the same order as `transformed`.
    /// Used to reorder on resume when param ordering may differ from
    /// the saved run. May be empty for states written before this
    /// field was added — callers must handle the empty case as
    /// "no reordering".
    pub param_names: Vec<String>,
    /// Current complete-data (or marginal) log-likelihood.
    pub current_ll: f64,
}

impl BaseResumeState {
    /// Reorder `self.params` and `self.transformed` to match
    /// `target_names`. No-op if `self.param_names` is empty
    /// (legacy state — caller bears responsibility for ordering).
    pub fn reorder_params(&mut self, target_names: &[String]) { ... }
}
```

Update `ChainResumeState` and `PMMHResumeState` to embed the base:

```rust
pub struct ChainResumeState {
    #[serde(flatten)]
    pub base: BaseResumeState,
    pub completed_sweeps: usize,
    pub trajectory: PGASTrajectory,
    pub mass_matrix: super::nuts::MassMatrix,
    pub nuts_step_size: f64,
    pub log_proposal_sd: Vec<f64>,
    pub total_accepted: Vec<usize>,
}

pub struct PMMHResumeState {
    #[serde(flatten)]
    pub base: BaseResumeState,
    pub completed_steps: usize,
    pub n_accepted: usize,
    pub adaptive: Option<AdaptiveProposal>,
    pub current_randoms: Option<super::correlated_pf::PFRandomState>,
    pub current_log_prior: f64,
    pub map_params: Vec<f64>,
    pub map_loglik: f64,
    pub map_log_posterior: f64,
}
```

The PMMH legacy-empty `param_names` guard is now handled by
`BaseResumeState::reorder_params` in one place. Both algorithms call it
on resume; neither can forget.

Note on `#[serde(flatten)]`: bincode does not support flattening. If the
resume state is serialised via bincode (as it is today), use composition
with explicit field delegation rather than `flatten`:

```rust
impl ChainResumeState {
    pub fn config_hash(&self)  -> &str      { &self.base.config_hash }
    pub fn params(&self)       -> &[f64]    { &self.base.params }
    pub fn transformed(&self)  -> &[f64]    { &self.base.transformed }
    pub fn param_names(&self)  -> &[String] { &self.base.param_names }
    pub fn current_ll(&self)   -> f64       { self.base.current_ll }
}
```

**Scope:** `pgas.rs` (struct definition + ~10 field access sites),
`pmmh.rs` (same), `types.rs` (new struct). CLI resume-loading sites in
`fit/pgas.rs` and `fit/pmmh.rs` update field accesses via delegation.
Bincode-serialised files on disk are unaffected — the on-disk field
order does not change if the embedded struct's fields are ordered the
same as they were before embedding.

---

## R3. Resolve the duplicate bounds fields on `EstimatedParam`

**Problem (Rdn2):** `EstimatedParam` carries both `lower: f64` /
`upper: f64` and, for `Log`/`Logit` transforms, `Transform { lo, hi }`
which also encode the bounds. For `Transform::None`, `lower`/`upper`
are unused. For `Log`/`Logit`, `lo == lower` and `hi == upper` should
hold by construction, but nothing enforces this.

**Proposed change:** Remove `lower` and `upper` as stand-alone fields.
Derive them from the transform at the point of use:

```rust
impl EstimatedParam {
    pub fn lower(&self) -> f64 {
        match &self.transform {
            Transform::Log  { lo, .. } | Transform::Logit { lo, .. } => *lo,
            Transform::None => f64::NEG_INFINITY,
        }
    }
    pub fn upper(&self) -> f64 {
        match &self.transform {
            Transform::Log  { hi, .. } | Transform::Logit { hi, .. } => *hi,
            Transform::None => f64::INFINITY,
        }
    }
}
```

Update the two call sites that access `.lower` / `.upper` directly
(in `pgas.rs:1219–1220`, preflight diagnostic):
```rust
// Before
let lo = p.to_transformed(p.lower.max(1e-10));
let hi = p.to_transformed(p.upper.min(1e10));

// After
let lo = p.to_transformed(p.lower().max(1e-10));
let hi = p.to_transformed(p.upper().min(1e10));
```

**Scope:** Remove two struct fields from `EstimatedParam` (~5 read
sites, all in `pgas.rs`). Update CLI construction sites that set
`lower`/`upper` when building `EstimatedParam` from a `FitConfigV2`
(they instead set the bounds inside the `Transform` variant, which
they already do).

**Note:** Do R1 first — moving `EstimatedParam` to `types.rs` makes
the field-removal a single-file change rather than a cross-file one.

---

## Ordering

R1 → R3 → R2.

R1 (move types) is a pure refactor; any test suite that passes before
passes after. R3 (remove duplicate fields) is easiest done immediately
after the move, while `EstimatedParam` is already being edited. R2
(base resume state) can follow independently once R1 is done.

All three are mechanical; none change simulation or inference
semantics.
