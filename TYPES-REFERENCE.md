# camdl CLI types & metadata reference

> **TODO: delete this file after review.** Throwaway working memo,
> untracked. ~125 public types in `rust/crates/cli/src/`; this document
> covers the load-bearing ones organised by layer + responsibility,
> plus pointers to the rest.
>
> **Update 2026-04-28:** the v1 fit-config cleanup
> (`feat/v1-fit-cleanup`, branch tip `3f3b3e4`) landed. This document
> reflects the post-cleanup state: the legacy `FitToml` schema and
> `FitConfigV2::to_legacy_toml()` bridge are gone; `FitConfigV2` is
> the single fit-config schema across the runner, the dispatcher, and
> on-disk. Sections that previously called out the v1 ↔ v2 split have
> been edited to reflect the simpler post-cleanup picture.

Purpose: enough type-level visibility to identify refactor / dedup /
re-architecture opportunities. Covers the data model that backs
`run.json`, the CAS abstraction, per-command typed inputs, fit config
schema, fit runtime state, fit summary types, browse/list/show
plumbing, CLI arg structs, and filesystem layout helpers.

---

## Table of contents

1. [Core data model — what's in `run.json`](#1-core-data-model)
2. [CAS abstraction — typed inputs](#2-cas-abstraction)
3. [Per-command typed inputs (CasInputs impls)](#3-per-command-typed-inputs)
4. [Fit config schema (fit.toml v2)](#4-fit-config-schema)
5. [Fit runtime state](#5-fit-runtime-state)
6. [Fit method results](#6-fit-method-results)
7. [Fit summary documents](#7-fit-summary-documents)
8. [Browse / list / show pipeline](#8-browse--list--show-pipeline)
9. [CLI argument types](#9-cli-argument-types)
10. [Filesystem layout & hashing helpers](#10-filesystem-layout--hashing)
11. [Other / cross-cutting](#11-other--cross-cutting)
12. [Refactor opportunities surfaced by this audit](#12-refactor-opportunities)

---

## 1. Core data model

Defined in `rust/crates/cli/src/run_meta.rs`. This is what every
`run.json` deserialises into — the universal envelope plus a tagged
union over five kinds.

### `Run` — universal envelope

```rust
pub struct Run {
    pub hash: String,                  // 64-char content hash
    pub version: String,               // camdl version at write time
    pub created_at: String,            // ISO-8601 UTC
    pub argv: Vec<String>,             // for reproducibility
    pub wall_time_seconds: f64,
    pub kind: RunKind,                 // discriminator + payload
}
```

Every `run.json` has these six fields. `kind` decides what's inside.

Methods: `Run::write(&self, dir)` (atomic tmp-then-rename),
`Run::read(dir)`, `Run::check_cache(dir, expected_hash) -> CacheStatus`.

### `RunKind` — five payloads (tagged union)

```rust
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RunKind {
    Simulate(SimulateMeta),         // one trajectory: model × scenario × seed
    Fit(FitMeta),                   // a complete fit (umbrella)
    FitStage(FitStageMeta),         // one stage of a fit (scout, refine, etc.)
    Profile(ProfileMeta),           // a profile-likelihood scan (single seed)
    ReplicateSet(ReplicateSetMeta), // umbrella over N seed-distinct children
}
```

JSON output uses `"kind": "fit"` (etc.) as a tag, with the meta's
fields flattened beside it inside the outer `kind` object.

### Meta payloads

#### `SimulateMeta`

```rust
pub struct SimulateMeta {
    pub model: String,                       // display only
    pub model_hash: String,                  // 64-char IR hash
    pub scenario: String,                    // name or "baseline"
    pub sim_hash: String,                    // model + base params + backend + dt
    pub scen_hash: String,                   // enable/disable/overrides
    pub seed: u64,
    pub backend: String,
    pub dt: f64,
    pub sweep_point: HashMap<String, f64>,   // empty for plain --cas
    pub from_fit_hash: Option<String>,       // sim → fit lineage (when --params is an mle)
}
```

#### `FitMeta`

```rust
pub struct FitMeta {
    pub model: String,
    pub model_hash: String,
    pub fit_toml_path: String,
    pub fit_toml_hash: String,
    pub data_hashes: HashMap<String, String>,
    pub estimated: Vec<String>,              // names from [estimate]
    pub fixed: HashMap<String, f64>,         // resolved fixed params
    pub stages_declared: Vec<String>,
    pub ic_free: bool,
    pub label: Option<String>,               // ← issue #24 wants this on Run
}
```

#### `FitStageMeta`

```rust
pub struct FitStageMeta {
    pub fit_hash: String,                    // backref to parent fit
    pub stage: String,                       // "scout", "refine", etc.
    pub method: String,                      // "if2", "pgas", "pmmh", "pfilter"
    pub seed: u64,
    pub n_chains: usize,
    pub algorithm: serde_json::Value,        // method-specific knobs (untyped)
    pub best_loglik: Option<f64>,
    pub best_chain: Option<usize>,
    pub starts_from: Option<StartsFromRef>,  // refine ← scout
    pub derived_from: Option<String>,        // `camdl fit derive` workflows
    pub parent_profile_hash: Option<String>, // when this is a profile-grid leaf
    pub profile_point_idx: Option<usize>,    //   ↳
    pub profile_start_idx: Option<usize>,    //   ↳
}
```

#### `ProfileMeta`

```rust
pub struct ProfileMeta {
    pub model: String,
    pub model_hash: String,
    pub focal_params: Vec<String>,
    pub grid: Vec<GridAxis>,                 // value list per focal
    pub n_starts: usize,                     // IF2 starts per grid cell
    pub if2_config_hash: String,             // diagnostic
    pub base_params_hash: String,            // diagnostic
    pub seed_base: u64,                      // (per-seed in current code; legacy name)
    pub total_jobs: usize,
}
```

#### `ReplicateSetMeta`

```rust
pub struct ReplicateSetMeta {
    pub dim_name: String,                    // "seed", "dataset_idx"
    pub keys: Vec<String>,                   // ["seed_1", "seed_2", "seed_3"]
    pub child_kind: String,                  // "profile", "fit", "simulate"
    pub inner_content_hash: String,          // seed-free hash all children share
}
```

### Supporting types

```rust
pub struct GridAxis {
    pub param: String,
    pub values: Vec<f64>,
}

pub struct StartsFromRef {
    pub stage: String,
    pub stage_hash: Option<String>,           // None for legacy ""
}

pub enum CacheStatus {
    Hit,
    Stale { stored: String, current: String },
    Miss,
}
```

---

## 2. CAS abstraction

Defined in `rust/crates/cli/src/cas/typed.rs`. The trait + helpers
that every CAS-emitting command goes through.

### `ContentHash` — newtype

```rust
pub struct ContentHash(String);   // 64-char hex SHA-256

impl ContentHash {
    pub fn from_bytes(bytes: &[u8]) -> Self;
    pub fn from_hex(hex: impl Into<String>) -> Self;
    pub fn full(&self) -> &str;   // 64 chars
    pub fn short(&self) -> &str;  // first 8
}
```

### `CasInputs` trait

```rust
pub trait CasInputs {
    fn content_hash(&self) -> ContentHash;
    fn cas_path(&self, root: &Path) -> PathBuf;
    fn run_kind(&self) -> RunKind;

    fn to_run(&self, version: String, argv: Vec<String>, wall_time_seconds: f64) -> Run {
        Run {
            hash: self.content_hash().full().to_string(),
            version,
            created_at: super::iso8601_utc(std::time::SystemTime::now()),
            argv,
            wall_time_seconds,
            kind: self.run_kind(),
        }
    }
}
```

Four implementors today: `SimulateInputs`, `ProfileInputs`,
`FitInputs`, `StageInputs`.

### `ReplicateSet` — umbrella helper (NOT a CasInputs)

```rust
pub struct ReplicateSet {
    pub inner_hash: ContentHash,             // seed-free content
    pub dim_name: String,                    // "seed", "dataset_idx"
    pub keys: Vec<String>,                   // ["seed_1", "seed_2"]
    pub child_kind: String,                  // "profile", "fit"
}

impl ReplicateSet {
    pub fn parent_hash(&self) -> ContentHash;
    pub fn child_dir(&self, parent_dir: &Path, key: &str) -> PathBuf;
    pub fn run_kind(&self) -> RunKind;       // RunKind::ReplicateSet(...)
}
```

### Hash helpers

```rust
pub fn hash_canonical(fields: &[(&str, &str)]) -> ContentHash;
pub fn compose_with_replicate(
    inner: &ContentHash, dim_name: &str, key: &str,
) -> ContentHash;
```

---

## 3. Per-command typed inputs

The four `CasInputs` impls. Each lives in its own module under
`cas/` (except `ProfileInputs` which lives in `profile.rs`).

### `SimulateInputs` (`cas/sim_inputs.rs`)

Used by both `simulate --cas` and `batch run`'s per-run write site.

```rust
pub struct SimulateInputs {
    pub model_path: String,                  // display only
    pub model_stem: Option<String>,          // path prefix
    pub scenario: String,
    pub model_hash: String,
    pub base_params_canonical: String,       // for sim_hash
    pub backend: String,
    pub dt: f64,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    pub scen_params: HashMap<String, f64>,   // merged scenario + sweep
    pub seed: u64,
    pub from_fit_hash: Option<String>,
    pub sweep_point: HashMap<String, f64>,   // display, recorded in SimulateMeta
}
```

`content_hash` = compose_with_replicate(h(sim_hash, scen_hash), "seed", seed).

### `ProfileInputs` + `ProfileIf2Config` (`profile.rs`)

```rust
pub struct ProfileInputs {
    pub model_path: String,
    pub stem: Option<String>,
    pub model_hash: String,
    pub base_params_hash: String,
    pub focal_grid: Vec<GridAxis>,
    pub fixed: Vec<String>,                  // names of fixed params
    pub if2_config: ProfileIf2Config,
    pub starts_from_lineage: Option<String>,
    pub seed: u64,
}

pub struct ProfileIf2Config {
    pub n_particles: usize,
    pub n_iterations: usize,
    pub cooling: f64,
    pub dt: f64,
    pub n_starts: usize,
}
```

`inner_hash()` excludes seed (used as ReplicateSet umbrella).
`content_hash()` = compose_with_replicate(inner, "seed", seed).

### `FitInputs` + `StageInputs` (`cas/fit_inputs.rs`)

```rust
pub struct FitInputs {
    pub fit_content_hash: String,            // pre-computed, seed-free
    pub stem: Option<String>,
    pub meta: FitMeta,                       // payload for run.json
}

pub struct StageInputs {
    pub fit_stage_hash: String,              // pre-computed, seed-inclusive
    pub stage_dir: PathBuf,                  // pre-computed by runner
    pub meta: FitStageMeta,
}
```

Both wrap legacy hashing (`fit_content_hash`, `fit_stage_hash`);
the trait surface is the consumer-facing API.

---

## 4. Fit config schema

Defined in `rust/crates/cli/src/fit/config_v2.rs`. Parses `fit.toml`.

### Top-level

```rust
pub struct FitConfigV2 {
    pub model: ModelRef,
    pub data: Option<DataSpec>,              // mutually exclusive with synthetic
    pub synthetic: Option<SyntheticSpec>,    //   ↳
    pub fit_seeds: Option<Vec<u64>>,
    pub fit_starts: Option<FitStarts>,
    pub output_dir: Option<String>,
    pub estimate: IndexMap<String, EstimateSpecV2>,  // ordered
    pub fixed: FixedParams,
    pub stages: IndexMap<String, Stage>,             // ordered
    pub config: FitBackendConfig,
    pub scenario: Option<String>,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    pub ic_free: Option<bool>,
    pub provenance: Option<FitProvenance>,
}
```

### Sub-types

```rust
pub struct ModelRef { pub camdl: String }

pub struct FitBackendConfig {
    pub backend: String,                     // default "chain_binomial"
    pub dt: f64,                             // default 1.0
}

pub struct DataSpec {
    pub observations: IndexMap<String, String>,
    pub holdout_after: Option<f64>,
    pub holdout: Option<IndexMap<String, String>>,
}

pub struct SyntheticSpec {
    pub true_params: String,                 // ground-truth file
    pub sim_seeds: SeedsSpec,
    pub datasets: Option<usize>,
    pub scenario: Option<String>,
}

#[serde(untagged)]
pub enum SeedsSpec {
    List(Vec<u64>),
    Range(String),                           // "1:20"
}

pub enum FitStarts { ModelDefault, Prior }

pub struct EstimateSpecV2 {
    pub bounds: (f64, f64),
    pub transform: Option<Transform>,
    pub prior: Option<PriorSpec>,
    pub ivp: bool,
    pub rw_sd: Option<f64>,
    pub start: Option<f64>,
}

pub enum Transform { Log, Logit, Identity }

#[serde(tag = "dist")]
pub enum PriorSpec {
    LogNormal { mu: f64, sigma: f64 },
    Normal { mu: f64, sigma: f64 },
    Beta { alpha: f64, beta: f64 },
    Uniform,
    HalfNormal { sigma: f64 },
}

pub struct FixedParams {
    pub from_file: Option<String>,
    pub values: IndexMap<String, f64>,
}
```

### Stages — tagged by method

```rust
#[serde(tag = "method")]
pub enum Stage {
    IF2 {
        chains: usize, particles: usize, iterations: usize, cooling: f64,
        starts_from: StartsFrom,
        loglik_eval: LoglikEvalConfig,
        gate: GateConfig,
    },
    PGAS {
        chains: usize, particles: usize, sweeps: usize,
        starts_from: StartsFrom,
        burn_in: Option<usize>, thin: Option<usize>,
    },
    PMMH {
        chains: usize, particles: usize, iterations: usize,
        starts_from: StartsFrom,
        burn_in: Option<usize>, thin: Option<usize>,
    },
    PFilter {
        particles: usize,
        replicates: Option<usize>,
        starts_from: StartsFrom,
    },
}

pub enum StartsFrom {
    Stage(String),                           // "scout"
    Directory(PathBuf),                      // /path/to/external/fit
    Random,
}

pub struct LoglikEvalConfig {
    pub n_particles: usize,                  // default 4000
    pub n_replicates: usize,                 // default 8
    pub combine: CombineMode,                // default LogMeanExp
}

pub enum CombineMode { LogMeanExp, Mean }

pub struct GateConfig {
    pub a_thresh: f64,                       // default 1.01
    pub decibans_thresh: f64,                // default 30.0 (with SE-aware floor)
}

pub struct FitProvenance {
    pub derived_from: Option<String>,
    pub reason: Option<String>,
}
```

---

## 5. Fit runtime state

### `FitState` (`fit/state.rs`)

`fit_state.toml` — inter-stage handoff file written at each stage's
end. Picked up by downstream stages (refine reads scout's fit_state).

```rust
pub struct FitState {
    pub stage: String,
    pub seed: u64,
    pub timestamp: String,
    pub input_hash: Option<String>,
    pub camdl_version: Option<String>,
    pub best_loglik: f64,
    pub initial_loglik: f64,
    pub best_chain: usize,
    pub n_chains: usize,
    pub n_good_chains: Option<usize>,
    pub start_values: HashMap<String, f64>,
    pub rw_sd: HashMap<String, f64>,
    pub loglik_type: Option<String>,         // "marginal" / "complete_data" / "if2"
    pub acceptance_rate: Option<f64>,        // PGAS/PMMH only
    pub tail_chain_agreement: HashMap<String, f64>,  // per-param Â
    pub ivp_params: Vec<String>,
    pub chain_logliks: Vec<f64>,             // per-chain final
    pub chain_eval_logliks: Vec<f64>,        // per-chain clean-eval
    pub chain_eval_ses: Vec<f64>,            // SE on clean-eval
    pub resolved_loglik_eval: Option<LoglikEvalConfig>,
    pub resolved_gate: Option<GateConfig>,
    // (more fields — gate verdict, ESS, etc.)
}
```

### Runner config (`fit/runner.rs`)

```rust
pub struct FitRunConfig {
    // Bundles the whole runner-side config: model, observations,
    // estimated/fixed param specs, IF2/PGAS hyperparams, etc.
    // ~30 fields. The runner's working struct, distinct from
    // FitConfigV2 (which is the on-disk schema).
    //
    // Build entry point: `FitRunConfig::build(&FitConfigV2, ...)`.
    // No FitToml shim — the v1 fit-config types were deleted in the
    // v1-cleanup pass; the runner consumes v2 directly.
}

pub struct ChainResults {
    // Per-chain raw IF2 outputs before clean-eval.
}

pub struct ParamSpec {
    pub name: String,
    pub rw_sd: Option<f64>,
    pub transform: Option<String>,    // "log" | "logit" | "identity"
    pub ivp: bool,
}

pub struct ObsStream {
    // One observation stream's data + projection metadata.
}
```

### Per-stage opts (`fit/pgas.rs`, `fit/pmmh.rs`)

```rust
pub struct PgasStageOpts {            // pgas::run_stage(...)
    pub n_chains: usize,
    pub n_particles: usize,
    pub n_sweeps: usize,
    pub burn_in: usize,
    pub thin: usize,
}
pub struct PmmhStageOpts {            // pmmh::run_stage(...)
    pub n_chains: usize,
    pub n_particles: usize,
    pub n_steps: usize,
    pub burn_in: usize,
    pub thin: usize,
}
```

Each is built from a `Stage::PGAS { ... }` / `Stage::PMMH { ... }`
variant via `from_stage(&Stage)` and passed verbatim into the
respective `run_stage` entry point. Carries only the per-stage
knobs that v2 exposes; v1-only knobs (`tempering`, `max_treedepth`,
`adapt`, `proposal_from`, `rho`, `n_trajectories`, ...) default to
the v1 values inside the runner.

### Loglik-eval (`fit/loglik_eval.rs`)

```rust
pub struct ChainScore {
    pub chain_id: usize,
    pub loglik: f64,
    pub se: f64,
    pub theta_hat: HashMap<String, f64>,
    // ...
}

pub struct LoglikEvalOutcome {
    pub per_chain: Vec<ChainScore>,
    // ...
}

pub struct FilterStats {
    // PF ESS / acceptance summaries
}
```

### Provenance (`fit/provenance.rs`)

`mle_params.toml` provenance block — written into the TOML as a
[provenance] section; readable by `simulate --params <mle>` for
back-tracking.

```rust
pub struct MleProvenance {
    pub fit_hash: String,
    pub stage: String,
    pub seed: u64,
    pub backend: String,
    pub dt: f64,
    // ... + content_hash, log_likelihood, n_particles, etc.
}

pub struct MleMetadata {
    pub data: Vec<DataEntry>,
    pub model: ModelEntry,
    pub fit: FitEntry,
}

pub struct DataEntry { pub stream: String, pub path: String, pub hash: String }

pub enum ContentVerification {
    Match, Stale { stored: String, current: String }, Missing,
}
```

### Synthetic (`fit/synthetic.rs`)

```rust
pub struct SyntheticDataset {
    pub idx: usize,                          // 1-based for path: ds_01
    pub seed: u64,
    pub data_path: PathBuf,
    pub true_params: HashMap<String, f64>,
}
```

### Gating (`fit/gating.rs`)

```rust
pub enum ScoutGateVerdict {
    Pass,
    FailA { max_a_hat: f64, threshold: f64, /* ... */ },
    FailDb { spread_db: f64, threshold: f64, /* ... */ },
    FailBoth { /* ... */ },
}
```

---

## 6. Fit method results

`fit/method_result.rs`. Typed view of a completed fit-stage's
output. Mirrors `RunKind` for *outputs* (whereas `RunKind` describes
the *kind of artifact*).

```rust
#[serde(tag = "method", rename_all = "lowercase")]
pub enum MethodResult {
    If2(If2StageResult),
    Pgas(PgasStageResult),
    Pmmh(PmmhStageResult),
    // PFilter is excluded — it's never a fit-stage today.
}

pub enum GateVerdict { Pass, FailA, FailDb, FailBoth }

pub struct If2StageResult {
    pub best_loglik: f64,
    pub best_chain: usize,
    pub theta_hat: BTreeMap<String, f64>,    // estimated params only
    pub max_chain_agreement: f64,            // Â (NOT Gelman R̂)
    pub gate_verdict: GateVerdict,
    pub ess_at_mle: Option<EssSummary>,
    pub n_chains: usize,
    pub n_iter: usize,
}

pub struct EssSummary {
    pub ess_min: f64,
    pub ess_mean: f64,
    pub ess_min_step: Option<usize>,
}

pub struct PgasStageResult {
    pub n_samples: usize,
    pub posterior_mean: BTreeMap<String, f64>,
    pub posterior_q025: BTreeMap<String, f64>,
    pub posterior_q975: BTreeMap<String, f64>,
    pub ess_per_param: BTreeMap<String, f64>,
    pub max_rhat: f64,                       // Gelman R̂ (NOT IF2 Â)
    pub acceptance_per_param: BTreeMap<String, f64>,
    pub n_chains: usize,
}

pub struct PmmhStageResult {
    pub n_samples: usize,
    pub posterior_mean: BTreeMap<String, f64>,
    pub ess: BTreeMap<String, f64>,
    pub max_rhat: f64,
    pub acceptance_rate: f64,                // scalar (PMMH: full-vector proposals)
    pub map_loglik: f64,
    pub n_chains: usize,
}

pub enum MethodResultError {
    UnknownMethod { method: String, stage_dir: PathBuf },
    Io { stage_dir: PathBuf, message: String },
}
```

> **Note the `Â` vs `R̂` distinction.** They're computed differently
> (Â is per-param IF2-trace tail-agreement; R̂ is Gelman-Rubin MCMC
> convergence). Field comments + table renderers must not merge them.

---

## 7. Fit summary documents

`fit/fit_summary.rs`. Typed JSON-emittable representation of a fit
summary (`camdl fit summary --format json | text | md | latex`). Each
emitter walks this same shape; renaming drift between emitters
(e.g. `clean_ll` → `loglik`, issue #26) happens at this layer.

```rust
pub struct FitSummaryDoc {
    pub fit_dir: String,
    pub schema: SchemaInfo,
    pub stages: Vec<StageReport>,
    pub heuristics: Option<HeuristicReport>,
}

pub struct SchemaInfo {
    pub version: String,
    pub camdl_version: String,
}

pub struct StageReport {
    pub name: String,
    pub method: String,
    pub n_chains: usize,
    pub best_loglik: Option<f64>,
    pub initial_loglik: Option<f64>,
    pub stage_progression: Option<StageProgression>,
    pub gate: Option<GateReport>,
    pub method_result: Option<MethodResult>,
    pub parameters: Vec<ParameterReport>,
    pub chains: Vec<ChainReport>,
    pub provenance: Option<ProvenanceReport>,
    pub resolved_loglik_eval: Option<LoglikEvalConfig>,
    pub resolved_gate: Option<GateConfig>,
}

pub struct GateReport {
    pub max_a_hat: f64,
    pub max_a_param: Option<String>,
    pub a_thresh: f64,
    pub a_passes: bool,
    pub delta_db: Option<f64>,
    pub threshold_db: Option<f64>,
    pub db_passes: Option<bool>,
    pub overall_pass: Option<bool>,
    pub threshold_source: String,
}

pub struct StageProgression {
    pub previous_stage: String,
    pub delta_nats: f64,
}

pub struct ParameterReport {
    pub name: String,
    pub estimate: f64,
    pub chain_agreement: Option<f64>,        // Â
    pub ivp: bool,
}

pub struct ChainReport {
    pub chain_id: usize,
    pub clean_loglik: f64,
    pub clean_se: f64,
    pub is_winner: bool,                     // data-side name; renderers say "selected"
}

pub struct ProvenanceReport {
    pub final_params_matches_mle_params: Option<bool>,
    pub fit_state_winner_matches_final_params: Option<bool>,
    pub stale_camdl_version: Option<String>,
}

pub struct HeuristicReport { /* recommendations from gate failures, etc. */ }
```

---

## 8. Browse / list / show pipeline

`rust/crates/cli/src/browse.rs`. Reader-side types for `camdl list /
show / cat`. Six parallel-ish entry types; the candidate for
consolidation discussed in `SHOW-TYPES-EXPLAINER.md`.

### Entry types (parallel structures)

```rust
pub struct RunEntry {                        // for sims
    pub run: Run,
    pub meta: SimulateMeta,                  // duplicates run.kind
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub created: SystemTime,
    pub traj_bytes: u64,
}

pub struct FitEntry {                        // for fit umbrellas
    pub run: Run,
    pub meta: FitMeta,
    pub rel_path: String,
    pub created: SystemTime,
}

pub struct ProfileEntry {                    // for list (profiles section)
    pub run: Run,
    pub rel_path: String,
    pub created: SystemTime,
    pub model: String,                       // pre-extracted display fields
    pub focal: String,
    pub shape: String,
    pub n_seeds: usize,
}

pub struct ReplicateSetEntry {               // for show on multi-seed profile umbrella
    pub run: Run,
    pub meta: ReplicateSetMeta,
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub created: SystemTime,
}

// Proposed for issue #25 (FitStage in show):
struct FitStageEntry {
    run: Run,
    meta: FitStageMeta,
    rel_path: String,
    abs_path: PathBuf,
    created: SystemTime,
}

// Proposed for single-seed Profile show gap:
struct ProfileShowEntry {
    run: Run,
    meta: ProfileMeta,
    rel_path: String,
    abs_path: PathBuf,
    created: SystemTime,
}
```

### Resolved enum

```rust
enum Resolved {
    Sim(RunEntry),
    Fit(FitEntry),
    ReplicateSet(ReplicateSetEntry),
    // Proposed:
    // Profile(ProfileShowEntry),
    // FitStage(FitStageEntry),
}
```

### Loaders

```rust
fn load_run_common(dir, cwd) -> Option<(Run, SystemTime, String)>;
fn load_sim_entry(dir, cwd) -> Option<RunEntry>;
fn load_fit_entry(dir, cwd) -> Option<FitEntry>;
fn load_replicate_set_entry(dir, cwd) -> Option<ReplicateSetEntry>;
// Proposed: load_profile_entry, load_fit_stage_entry
```

Pattern: each loader calls `load_run_common`, then matches `run.kind`
to extract the typed meta + clones it into the Entry. The match
returns `None` on kind mismatch.

### Discoverers / resolvers

```rust
fn discover_runs(root) -> Result<Vec<RunEntry>, String>;
fn discover_fits(root) -> Result<Vec<FitEntry>, String>;
fn discover_profiles(root) -> Result<Vec<ProfileEntry>, String>;

fn resolve_any(root, key) -> Result<Resolved, String>;
fn list_profile_children(root, parent_hash, args);  // for `--parent <hash>`
fn resolve_stage_by_hash(root, hash_prefix);
```

### Filter

```rust
enum KindFilter { Sim, Fit, Profile, All }
```

### Show printers

```rust
fn show_sim(entry: &RunEntry);
fn show_fit(entry: &FitEntry);
fn show_replicate_set(entry: &ReplicateSetEntry);
// Proposed: show_profile, show_fit_stage
```

---

## 9. CLI argument types

`rust/crates/cli/src/args/`. Two files: `types.rs` (shared parsed
types) + `mod.rs` (per-command Args structs).

### Shared parsed types (`args/types.rs`)

Implementing `FromStr` for clap's value parsers:

```rust
pub struct ParamOverride { pub name: String, pub value: f64 }   // --param NAME=VALUE
pub struct TableSpec { pub name: String, pub path: PathBuf }    // --table NAME=FILE
pub struct ParamVecSpec { pub prefix: String, pub file: String }
pub struct ListDuration(pub Duration);                          // --since 1h/30m/2d

pub enum Backend { Gillespie, ChainBinomial, TauLeap, Ode }
pub enum ProgressMode { Auto, Pretty, Plain, None }

pub enum SeedSpec {
    Range { from: u64, to: u64 },
    List(Vec<u64>),
}

pub struct SweepSpec {
    pub name: String,
    pub grid: Grid,
}
pub enum Grid {                              // --sweep NAME=...
    List(Vec<f64>),
    Linear { min: f64, max: f64, n: usize },
    Log10 { min: f64, max: f64, n: usize },
}

pub enum RwSd {
    Auto,
    Map(HashMap<String, Option<f64>>),
}
```

### Shared arg groups (`args/mod.rs`)

Flattened into per-command Args:

```rust
pub struct ModelOverrides {                  // --params + --param + --table
    pub params: Vec<PathBuf>,
    pub param: Vec<ParamOverride>,
    pub table: Vec<TableSpec>,
}

pub struct ScenarioArgs {                    // --scenario | --enable + --disable
    pub scenario: Option<String>,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
}

pub struct SimBackend { pub backend: Option<Backend>, pub dt: Option<f64> }

pub struct InferenceCore {                   // shared: pfilter / if2 / profile
    pub particles: usize,
    pub dt: f64,
    pub seed: u64,
    pub parallel: usize,
}

pub struct FlowProjection { pub obs: Option<String>, pub flow: Option<String> }
```

### Per-command Args (selected)

There are 25+ `Args` structs. Highlights:

```rust
pub struct SimulateArgs { /* model, scenarios, seeds, replicates, output, --cas, ... */ }
pub struct ProfileArgs { /* model, sweep: Vec<SweepSpec>, seeds: Option<SeedSpec>, ... */ }
pub struct FitRunArgs { /* fit_path, sweep, seed, stage, label, ... */ }
pub struct ListArgs { /* root, kind, parent, since, model, scenario, label_pattern, ... */ }
pub struct ShowArgs { /* root, target */ }
pub struct CatArgs { /* root, target, stream */ }
pub struct CompareArgs { /* config, paths, baseline, metrics, format, ... */ }
pub struct PfilterArgs { /* model, params, data, particles, replicates, ... */ }
pub struct If2Args { /* particles, iterations, cooling, etc. — legacy direct cmd */ }
pub struct BatchArgs { /* file, output_dir, parallel, dry_run, force */ }
pub struct EvalArgs { /* model, expr, time grid */ }
pub struct DataSplitArgs { /* train/test/holdout TSV split */ }
pub struct FitSummaryArgs { /* dir, stage, format, params_only, strict */ }
pub struct FitTableArgs { /* root, hash, label_pattern, format, since_seconds */ }
pub struct FitDiffArgs { /* two fit dirs to diff their configs */ }
pub struct FitNewArgs { /* scaffold a new fit.toml */ }
pub struct FitWhereArgs { /* find a fit by hash */ }
pub struct FitLabelArgs { /* re-label by hash; will become camdl label */ }
pub struct FitStatusArgs { /* progress / completion of a fit */ }
pub struct BatchStatusArgs { /* per-experiment batch progress */ }
```

### Format enums

```rust
pub enum FitSummaryFormat { Text, Json, Md, Latex }
pub enum FitTableFormat { Plain, Md, Json }
```

---

## 10. Filesystem layout & hashing

### Layout helpers (`run_paths.rs`)

```rust
pub const DEFAULT_OUTPUT_ROOT: &str = "results";

pub fn output_root(cli, config) -> PathBuf;       // CLI > config > $CAMDL_OUTPUT_DIR > default
pub fn sim_run_dir(root, stem, sim_hash, scenario, scen_hash, seed) -> PathBuf;
pub fn sim_run_rel(stem, sim_hash, scenario, scen_hash, seed) -> String;
pub fn fit_run_dir(root, fit_toml_stem, fit_hash) -> PathBuf;
pub fn profile_point_dir(profile_dir, point_idx) -> PathBuf;
pub fn profile_point_start_dir(profile_dir, point_idx, start_idx) -> PathBuf;
```

Note: `profile_run_dir` was deleted in the typed-CAS migration —
`ProfileInputs::cas_path` builds it now.

### The canonical layout

```
<root>/
  sims/                                          # simulate --cas
    <model_stem>-<sim_hash[:8]>/                 # one model+config combo
      <scenario_slug>-<scen_hash[:8]>/           # one scenario delta
        seed_<N>/
          run.json                               # RunKind::Simulate
          traj.tsv
          obs/<obs_hash>-<obs_seed>/             # optional: obs draws
            <stream>.tsv

  fits/                                          # fit run
    <stem>-<fit_hash[:8]>/                       # one fit umbrella
      run.json                                   # RunKind::Fit
      fit.toml.original                          # archived input
      real/                                      # OR synthetic/ds_NN/
        fit_<seed>/                              # one fit_seed cell
          <sweep_slug>/                          # optional: per sweep point
            <stage_name>/
              run.json                           # RunKind::FitStage
              mle_params.toml                    # IF2 winner θ̂
              fit_state.toml                     # inter-stage handoff
              chain_evaluations.tsv              # per-chain clean-eval
              <stage>_summary.json               # MethodResult
              ...

  profiles/                                      # profile (single-seed flat)
    <stem>-<profile_content_hash[:8]>/
      run.json                                   # RunKind::Profile
      profile.tsv                                # rollup
      points/{NNNNN}/
        focal.toml                               # pinned focal values
        start_<K>/
          run.json                               # RunKind::FitStage (with parent_profile_hash)
          mle.toml

  profiles/                                      # profile (multi-seed nested)
    <stem>-<parent_hash[:8]>/
      run.json                                   # RunKind::ReplicateSet
      summary.tsv                                # cross-seed aggregate
      replicates/
        seed_<S>/                                # one per replicate seed
          run.json                               # RunKind::Profile
          profile.tsv                            # per-seed rollup
          points/.../start_K/                    # same as single-seed shape

  runs/                                          # batch run (single output dir)
    <scenario_slug>-<scen_hash[:8]>/seed_<N>/    # similar to sims
```

### Hashing helpers (`hashing.rs`)

```rust
pub fn model_hash(ir_json: &str) -> String;       // canonical IR → 64-char hex
pub fn sim_hash(model_h, params_canon, backend, dt) -> String;
pub fn scen_hash(enable, disable, params: &HashMap<String,f64>) -> String;
pub fn file_hash(path: &str) -> Option<String>;   // 8-char hex of file bytes
pub fn sha256_hex(bytes: &[u8]) -> String;        // full 64-char SHA-256
pub fn fit_content_hash(model_ir, data_files, fit_toml_bytes) -> String;
pub fn canonical_params(params: &HashMap<String,f64>) -> String;
pub fn slug(name: &str) -> String;                // filesystem-safe
pub fn path_stem_slug(path: &str) -> Option<String>;
```

`fit/provenance.rs::fit_stage_hash(model_ir, observations, estimate,
fixed, stage_name, &Stage, seed) -> Result<String, String>` — fit's
seed-inclusive per-stage hash.

---

## 11. Other / cross-cutting

### `util.rs::SimRun`

The runner-level "I want to simulate this" struct. Used by both
`simulate` and `batch` to share the simulation invocation pipeline.

```rust
pub struct SimRun {
    pub ir_path: String,
    pub params_files: Vec<String>,
    pub overrides: HashMap<String, f64>,
    pub scenario_name: Option<String>,
    pub adhoc_enable: Vec<String>,
    pub adhoc_disable: Vec<String>,
    pub backend: String,
    pub dt: f64,
    pub seed: u64,
    // ...
}
```

### `batch.rs` types

```rust
pub struct ScenarioEntry { /* one [[scenario]] entry from a batch.toml */ }
pub enum RunDecision { CacheHit, CacheMiss }
pub struct RunPlan {
    pub scenario: String,
    pub seed: u64,
    pub sweep_overrides: HashMap<String, f64>,
    pub run_dir: String,
    pub run_path: String,
    pub decision: RunDecision,
    // ...
}
```

(Plus internal `SweepSpec`, `LinspaceSpec`, `RangeSpec` for the
batch.toml's `[sweep]` section — distinct from `args::types::SweepSpec`.)

### `sampling.rs`

```rust
pub struct PriorSpec { /* sampling-side, distinct from config_v2::PriorSpec */ }
pub struct DesignParam { /* one row of an experiment design */ }
pub struct DesignPoints { /* full design matrix */ }
```

### `progress.rs`

```rust
pub enum Resolved {                              // ← unrelated to browse::Resolved
    Auto, Pretty, Plain, None,
}
pub struct Throttle { /* rate-limit progress prints */ }
```

### `fit/fit_tree.rs`

```rust
pub struct FitDirEntry {
    pub fit_dir: PathBuf,
    pub run: Run,
    pub fit_meta: FitMeta,
}

pub struct StageNode { /* one stage in a fit's stage tree */ }
pub struct StageAxes { /* per-cell stage decomposition: real/synthetic, fit_seed, sweep */ }
pub enum DataKind { Real, Synthetic { dataset_idx: usize } }
```

### `fit/table_row.rs`

```rust
pub struct TableRowSchema { /* version + column order for fit table output */ }
pub struct TableRow { /* one row of fit table — a stage's summary, flat */ }
pub enum TableRowError { /* missing files, bad parses */ }
```

### `fit/config_diff.rs`

```rust
pub struct ConfigDiff { /* result of diffing two FitConfigV2 */ }
pub struct BoundsChange { /* parameter bounds delta */ }
pub struct PriorChange { /* prior-spec delta */ }
pub struct DataHashesDiff { /* observation file changes */ }
pub struct StagesChanged { /* added/removed/modified stages */ }
pub struct StageSettingsChange { /* per-stage knob changes */ }
```

### `fit/grid_summary.rs`

```rust
pub struct SummaryRow { /* one row of a fit-grid summary.tsv */ }
```

### `fit/trace_writer.rs`

```rust
pub struct TraceWriter { /* streaming writer for chain traces */ }
```

---

## 12. Refactor opportunities

Concrete observations after walking the whole inventory.

### A. The 6-parallel-Entry-types pattern in browse.rs

Already discussed in `SHOW-TYPES-EXPLAINER.md`. `RunEntry / FitEntry /
ReplicateSetEntry / ProfileEntry` (+ two more proposed for #25 and
single-seed Profile show) all have the same shape `(Run, paths, time,
[extras])` with a redundant `meta: XMeta` field that's just a clone of
`run.kind`'s payload. **Consolidating into a single `LoadedRun` saves
~30 LOC even while adding two new show paths.**

### B. Two unrelated `Resolved` enums

`browse::Resolved` (sim/fit/replicate-set discriminator for show
dispatch) and `progress::Resolved` (auto/pretty/plain/none progress
mode) share a name. Trivial rename of progress's enum to
`ProgressModeResolved` or similar would prevent confusion.

### C. Two unrelated `PriorSpec` types

`config_v2::PriorSpec` (fit.toml's prior declaration, tagged by
`dist`) and `sampling::PriorSpec` (runtime sampling primitive, unrelated
shape). Separate concerns, but the shared name is a footgun. Either
disambiguate with module-qualified imports everywhere or rename one.

### D. `ProfileMeta.seed_base` is misnamed post-typed-CAS

Field name from when single-seed profile derived per-start seeds
from a `seed_base` via XOR. Today the field carries the actual seed of
this profile run. Rename to `seed` for clarity (cosmetic; serde-tagged
in run.json).

### E. `ProfileMeta.if2_config_hash` and `base_params_hash` are
redundant

These were CAS-relevant under the legacy hashing scheme. Now
`ProfileInputs::content_hash()` is the cache key; these fields are
diagnostic display-only. Could either drop them or rename to make the
"display only" status explicit. Same logic for `SimulateMeta.sim_hash`
/ `scen_hash` — they're path components, present in run.json for
display, but the trait's `content_hash` is the actual key.

### F. `label` lives on `FitMeta` but should live on `Run`

Issue #24. Today only fits can have labels; profiles, sims, and
replicate-set umbrellas can't. Lifting `label: Option<String>` to the
top-level `Run` struct makes labels universal and `camdl label
<hash>` kind-agnostic. ~150 LOC change, not big.

### G. `MethodResult` and `RunKind::FitStage` carry overlapping but
non-equal information

`RunKind::FitStage(FitStageMeta)` is what's in `run.json` —
backref-heavy provenance fields. `MethodResult` is the *typed
interpretation* of a completed stage — typed by method, with
posterior summaries, gate verdicts, ESS. They live at different
layers (data-on-disk vs. typed-loaded-output) and that's fine, but
the fields that overlap (`best_loglik`, `best_chain`, `n_chains`)
duplicate. A `MethodResult::from_fit_stage_meta_and_state(stage_meta,
fit_state)` constructor centralises the projection.

### H. `args::types::SweepSpec` vs `batch::SweepSpec`

The CLI's `--sweep NAME=lin(...)` parser produces
`args::types::SweepSpec`. The batch.toml's `[sweep.x] linspace = ...`
produces a *separate* internal `SweepSpec` type in `batch.rs`. Both
expand to `Vec<f64>`. Could share a type if we want one "sweep
specification" concept.

### I. `LoglikEvalConfig` and `GateConfig` are referenced from both
config and FitState

They're declared in `config_v2.rs` (input config) and embedded into
`FitState` (output state, carrying the *resolved* effective values).
This is correct (the resolved values may differ from declared ones
when defaults kick in), but the dual-location is a footgun for
schema migrations. Worth a comment in both locations cross-referencing.

### J. `Run.hash` semantics depends on kind

```rust
/// Content hash for this run, full 64-char hex. Scope depends on `kind`:
///   - Simulate: hash of (sim_hash, scen_hash, seed).
///   - Fit: seed-independent content hash of (fit.toml, model IR, data files).
///   - FitStage: stage-scope config hash from fit_stage_hash (includes seed).
```

This is documented but conceptually confusing — `Run.hash` for a
`Fit` is the umbrella hash, for `FitStage` it's the seed-scoped
stage hash. After typed CAS, they're all `inputs.content_hash()` —
which makes the meaning uniform IF you read it as "the typed-CAS
hash for whatever this run *is*." Worth a doc-comment refresh.

### K. `Run::write` is called many times during a single fit

The umbrella `run.json` is written at fit start (with wall_time=0)
and rewritten at fit end (with real wall time). Each stage's
`run.json` is written at stage end. In a multi-cell fit
(`fit_seeds`+ synthetic), the umbrella is rewritten N×M times even
though only wall_time changes. Not a perf problem but worth noting:
last-write-wins, atomic-rename ensures consistency.

### L. There is no `Show` trait

Every kind has a `show_X(entry)` function. They share structure
(path / kind / hashes / created / version / argv) but with kind-
specific middle sections. A `Show` trait hasn't been justified by a
second consumer; if reader/writer split or external rendering
becomes a need, that's the moment.

---

## Summary count

- **Core data model**: 10 types (`Run`, `RunKind` + 5 metas + 3
  supporting). Stable; rename `ProfileMeta.seed_base` and lift
  `FitMeta.label` to `Run.label` are the only proposed changes.
- **CAS abstraction**: 4 types (trait + ContentHash + ReplicateSet +
  ReplicateSetMeta). Stable.
- **Per-command CasInputs impls**: 4 (SimulateInputs, ProfileInputs,
  FitInputs, StageInputs). Stable.
- **Fit config schema**: ~16 types (`FitConfigV2` + sub-types). v1
  cleanup landed — the legacy `FitToml` + 6 sub-types are gone, and
  there's no bridge layer.
- **Fit runtime**: ~12 types. Mostly load-bearing. Adds two new
  per-stage-opts structs (`pgas::PgasStageOpts`, `pmmh::PmmhStageOpts`)
  carried at the dispatch site for the v2-only PGAS / PMMH entry
  points.
- **Fit method results**: 8 types (MethodResult + 3 stage results +
  EssSummary + GateVerdict + 2 errors). Stable.
- **Fit summary**: 9 types. Renderer-side; subject to
  display-naming drift (issue #26).
- **Browse / list / show**: 6 types (5 entries + Resolved).
  Consolidation candidate (the "LoadedRun" discussion).
- **CLI args**: 25+ Args structs + 10 shared parsed types + 2 format
  enums. Stable, well-isolated by clap convention.
- **Cross-cutting**: ~10 types in batch / sampling / progress / utils
  / fit subdirs. Stable but with two name collisions (`Resolved`,
  `PriorSpec`).

Approximate total: **~125 public types** in `cli/src/` (down from
~135 pre-cleanup; ~7 v1 fit-config types deleted, +2 stage-opts
structs added).
