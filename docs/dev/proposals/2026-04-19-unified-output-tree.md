---
status: proposal
date: 2026-04-19
---

# Unify the Simulate and Fit Output Trees

## Motivation

camdl currently writes two parallel result trees with two separate
metadata schemas and two separate browse stories:

- `output/runs/…` — written by `camdl simulate --cas`,
  `camdl simulate batch`. Readable via `camdl list / show / cat`.
- `results/fits/…` — written by `camdl fit run` and legacy fit
  subcommands. Not readable via any CLI browse command; user has
  to `cd` around.

Both trees use content-addressable caching internally: sim runs
hash on `(model_hash, base_params, backend, dt, version) × (scenario
deltas) × seed`; fit runs hash on
`(model, data, estimate, fixed, stage, seed)`. But the schemes live
in different modules, produce different `run.json` shapes, and can
drift independently — which they already have, twice, during recent
weeks.

The user-facing rough edges:

- "Where did my result go?" has two answers depending on which
  verb produced it.
- `camdl list` surfaces simulations but not fits. Finding a
  recent fit means filesystem archaeology.
- Two provenance schemas (`cas::RunMeta`, `fit::provenance::StageProvenance`)
  duplicate ~80 % of their fields.
- Multiple hashing primitives (`sim_hash`, `scen_hash`, `config_hash`,
  `compute_content_hash`, `file_content_hash`, `compute_input_hash`)
  with overlapping purposes, maintained by different code paths.

Results are results. They should live under one root with one
metadata schema discriminated by kind, browsed by one CLI.

## Design principle

A **run** is a content-hashed directory under a single `output/`
root. Its metadata is a `Run` struct with shared fields (hash,
version, created_at, argv) and a `kind: RunKind` enum that branches
into the shape-specific payload.

The directory *content* legitimately differs between kinds
(trajectories vs stage-structured fit outputs), so the path layout
inside each kind stays shape-specific. The unification happens at
three seams: the root, the metadata schema, and the browse CLI.

## Target layout

```
output/                             # single root; was `output/` for sims + `results/` for fits
  sims/
    <sim_hash[:8]>/
      <scenario-slug>-<scen_hash[:8]>/
        seed_<N>/
          traj.tsv
          run.json              # kind = "simulate"
          obs/<obs_hash[:8]>-<obs_seed>/
            <stream>.tsv
            obs.json
  fits/
    <fit_hash[:8]>/             # hashes fit.toml + model IR + data file content
      run.json                  # kind = "fit" — top-level fit metadata
      real/
        fit_<seed>/
          scout/
            run.json            # kind = "fit-stage" — per-stage metadata
            chain_starts.tsv
            chain_N/parameter_traces.tsv
            fit_state.toml
            mle_params.toml
          refine/
            run.json
            ...
          summary.tsv
      synthetic/                # for [synthetic] fits
        ds_01/
          fit_<seed>/
            scout/ refine/
        data/ds_01.tsv
        truth.toml
        summary.tsv
        coverage.tsv
```

Three migration-visible changes versus today:

1. Root renamed from `results/fits/…` to `output/fits/…`.
2. Top-level fit directory keyed by `fit_hash[:8]` (an 8-char
   content prefix) instead of the fit.toml basename. Human-
   friendly names remain recoverable via `camdl list` which reads
   the fit.toml path from `run.json`.
3. Each fit run gets a top-level `run.json` summarising the whole
   fit, in addition to the existing per-stage `run.json` (also
   renamed — the per-stage file was called `run.json` in both
   systems but had different shape).

## Types — before and after

### Today

Two nearly-identical metadata structs in different modules:

```rust
// cli/src/cas.rs
pub struct RunMeta {
    pub model: String,
    pub model_hash: String,
    pub scenario: String,
    pub sim_hash: String,
    pub scen_hash: String,
    pub seed: u64,
    pub backend: String,
    pub dt: f64,
    pub version: String,
    pub created_at: String,
    pub argv: Vec<String>,
    pub sweep_point: HashMap<String, f64>,
}

// cli/src/fit/provenance.rs
pub struct StageProvenance {
    pub camdl_version: String,
    pub timestamp: String,
    pub config_hash: String,
    pub fit_config: String,
    pub stage: String,
    pub model: String,
    pub model_hash: String,
    pub data_hashes: HashMap<String, String>,
    pub estimated: Vec<String>,
    pub fixed: HashMap<String, f64>,
    pub algorithm: serde_json::Value,     // ⚠ typed escape hatch
    pub starts_from: Option<StartsFromProv>,
    pub derived_from: Option<String>,
    pub seed: u64,
    pub wall_time_seconds: f64,
    pub best_loglik: Option<f64>,
    pub best_chain: Option<usize>,
}
```

Note the field-name drift: `version` vs `camdl_version`, `created_at`
vs `timestamp`, `sim_hash` vs `config_hash`. Same information, three
different names across the two schemas.

### Proposed

One `Run` struct with a kind-discriminator enum. Shared fields
pulled to the top; everything kind-specific lives in the variant:

```rust
// cli/src/run_meta.rs (new module replacing cli/src/cas.rs metadata types
// and cli/src/fit/provenance.rs StageProvenance)

/// Metadata written to `run.json` at the top of every content-hashed
/// run directory in the output tree. Shared fields go here; kind-
/// specific fields live inside `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Content hash for this run. Full hex; the 8-char prefix is used
    /// in the filesystem path.
    pub hash: String,
    /// camdl version at write time (e.g. "0.1.0+abc1234").
    pub version: String,
    /// ISO 8601 UTC timestamp at completion.
    pub created_at: String,
    /// Original argv that produced this run — `camdl show <hash>`
    /// prints it back for reproducibility.
    pub argv: Vec<String>,
    /// Total wall time for the run (seconds). Always set, even for
    /// cache hits (which record time to cache-hit detection, ≈ 0).
    pub wall_time_seconds: f64,
    /// Kind-specific payload.
    pub kind: RunKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RunKind {
    /// One simulate invocation. The directory contains `traj.tsv`
    /// and optional `obs/` subdirectories (see `obs_dir`).
    Simulate(SimulateMeta),
    /// A complete fit (potentially multi-stage). The directory
    /// contains per-stage subdirectories, each with its own
    /// stage-level `Run`.
    Fit(FitMeta),
    /// One stage of a fit. The directory is a child of a `Fit` run.
    FitStage(FitStageMeta),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateMeta {
    pub model: String,
    pub model_hash: String,
    pub scenario: String,
    pub sim_hash: String,
    pub scen_hash: String,
    pub seed: u64,
    pub backend: String,
    pub dt: f64,
    /// Sweep-point param values (empty for single-run `--cas`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sweep_point: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitMeta {
    pub model: String,
    pub model_hash: String,
    pub fit_toml_path: String,
    pub fit_toml_hash: String,
    /// Keys = stream name, values = data file content hash.
    pub data_hashes: HashMap<String, String>,
    /// Names of parameters in `[estimate]`.
    pub estimated: Vec<String>,
    /// Resolved fixed params (name → numeric value).
    pub fixed: HashMap<String, f64>,
    /// Names of stages declared in fit.toml, in execution order.
    pub stages_declared: Vec<String>,
    /// ic_free flag — so the browse CLI can label fits appropriately.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ic_free: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitStageMeta {
    /// Hash of the parent fit (top-level `fit_hash`).
    pub fit_hash: String,
    /// Stage name within the fit (e.g. "scout", "refine").
    pub stage: String,
    /// Stage method — "if2", "pgas", "pmmh", "pfilter".
    pub method: String,
    /// Stage-level input hash (scope: this stage only).
    pub stage_hash: String,
    pub seed: u64,
    pub n_chains: usize,
    pub best_loglik: Option<f64>,
    pub best_chain: Option<usize>,
    /// Reference to the stage this one started from, if any
    /// (e.g. refine → scout). Present in fit_state.toml already;
    /// duplicated here for browse-time display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_from: Option<StartsFromRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartsFromRef {
    pub stage: String,
    pub stage_hash: String,
}
```

The `algorithm: serde_json::Value` escape hatch goes away. Method-
specific details (`cooling_fraction`, `n_particles`, etc.) live in
`fit_state.toml` alongside the algorithm's output, not smuggled into
the provenance schema as untyped JSON.

### Hashing

One module, one helper per hash with a precise name. Today's
overlapping functions collapse:

| Today | After |
|---|---|
| `hashing::model_hash(ir_json)` | keep as-is |
| `hashing::sim_hash(model_hash, params, backend, dt)` | keep as-is |
| `hashing::scen_hash(enable, disable, params)` | keep as-is |
| `fit::provenance::compute_config_hash_v2(...)` | renamed `hashing::fit_hash(fit_toml, model_hash, data_hashes)` — shape unchanged |
| `fit::provenance::compute_input_hash(...)` | renamed `hashing::stage_hash(fit_hash, stage_name, stage_config, seed)` |
| `fit::provenance::compute_content_hash(params)` | deleted — unused after unification |
| `fit::provenance::file_content_hash(path)` | renamed `hashing::file_hash(path)` and exported |
| `hashing::canonical_params` | keep |
| `hashing::slug` | keep |

Net: one file, five primary hash helpers (`model_hash`, `sim_hash`,
`scen_hash`, `fit_hash`, `stage_hash`) plus the two low-level
utilities (`canonical_params`, `file_hash`). Currently these are
spread across two modules with three slightly-different content-
hash variants.

### Cache-check types

Today there are two cache-status enums that do the same thing:

```rust
// cas path — via has_cached_traj(dir) boolean
pub enum CacheStatus { Match, Mismatch, NotFound }
// fit path — via check_config_hash(stage_dir, hash)
pub enum ConfigCacheStatus { Match, Stale { stored, current }, NotFound }
```

Collapse to one:

```rust
pub enum CacheStatus {
    /// Run directory exists and its stored hash matches the expected hash.
    Hit { run_dir: PathBuf, stored_hash: String },
    /// Directory exists but hash differs (stale cache).
    Stale { run_dir: PathBuf, stored: String, current: String },
    /// Directory doesn't exist yet.
    Miss,
}
```

Stale semantics (the fit-side enum had this, sim didn't) become
universal. One `check_cache(run_dir, expected_hash)` function used
by both sims and fits.

## Path construction

One module for path construction, taking `Run` and producing the
canonical directory:

```rust
// cli/src/run_paths.rs (new)

/// The root of the output tree. Defaults to `./output`; overridable
/// via CLI `--output-dir`, fit.toml `output_dir`, or batch.toml
/// `output_dir`. Single resolver so the three paths can't drift.
pub fn output_root(cli: Option<&str>, config: Option<&str>) -> PathBuf {
    cli.map(PathBuf::from)
        .or_else(|| config.map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("output"))
}

pub fn sim_run_dir(root: &Path, sim_hash: &str, scenario: &str, scen_hash: &str, seed: u64) -> PathBuf {
    root.join("sims")
        .join(&sim_hash[..8])
        .join(format!("{}-{}", slug(scenario), &scen_hash[..8]))
        .join(format!("seed_{}", seed))
}

pub fn fit_run_dir(root: &Path, fit_hash: &str) -> PathBuf {
    root.join("fits").join(&fit_hash[..8])
}

pub fn fit_stage_dir(
    root: &Path, fit_hash: &str,
    source: FitSource,              // Real { fit_seed } | Synthetic { ds_idx, fit_seed }
    stage: &str,
) -> PathBuf {
    let base = fit_run_dir(root, fit_hash);
    match source {
        FitSource::Real { fit_seed } =>
            base.join("real").join(format!("fit_{}", fit_seed)).join(stage),
        FitSource::Synthetic { ds_idx, fit_seed } =>
            base.join("synthetic").join(format_dataset_dir(ds_idx))
                .join(format!("fit_{}", fit_seed)).join(stage),
    }
}
```

Every path-computation site in the codebase routes through these
four functions. Today there are at least eight places that construct
fit-output paths by hand (scout.rs, refine.rs, validate.rs, pmmh.rs,
pgas.rs, mod.rs v2 dispatch, etc.) — fragile and already has drifted
(the `synthetic/` prefix was added in one place and needed five more
edits to propagate).

## Browse CLI

One extension to `browse::cmd_list`. The walk becomes:

```rust
fn discover_runs(root: &Path) -> Vec<Run> {
    walk(root.join("sims")).filter_map(load_run_json)
        .chain(walk(root.join("fits")).filter_map(load_run_json))
        .chain(walk_fit_stages(root.join("fits")).filter_map(load_run_json))
        .collect()
}
```

`camdl list` output learns a `kind` column. `camdl show <hash>` and
`camdl cat <hash>` key on the hash prefix and resolve to the
appropriate `run_dir` via the shared resolver. Since sim and fit
hashes live in separate subtrees, there's no collision risk even if
two different kinds happen to share an 8-char prefix.

New filter flags (backward-compat: all existing `camdl list` flags
keep their meaning):

```
camdl list --kind sim          # only simulations
camdl list --kind fit          # only top-level fits (not stages)
camdl list --kind fit-stage    # only individual stages
camdl list                     # all, sorted by created_at DESC
```

## Code smells this surfaces (and fixes)

1. **Field-name drift across duplicated metadata.** `version` vs
   `camdl_version`, `created_at` vs `timestamp`, `sim_hash` vs
   `config_hash` — same concepts, different spellings across the
   two schemas. ADT consolidation fixes by construction.

2. **`algorithm: serde_json::Value` escape hatch in
   `StageProvenance`.** Method-specific metadata stuffed into an
   untyped JSON field because the struct-level design gave up.
   Method-specific details belong in `fit_state.toml` where they
   already live; the provenance struct just names the method.

3. **Three content-hash helpers** (`model_hash`, `sim_hash`,
   `compute_content_hash`, `file_content_hash`,
   `compute_input_hash`, `compute_config_hash_v2`) with overlapping
   purpose. Post-unification: five clearly-named helpers, each with
   a documented input scope.

4. **Two cache-status enums** (`CacheStatus`, `ConfigCacheStatus`)
   with near-identical semantics. Consolidate to one that captures
   `Hit | Stale | Miss` — the Stale variant (fit-side only today)
   becomes universal.

5. **`output_dir` string scattered across fit.toml, batch.toml, and
   CLI flags** with three separate resolvers. Single resolver
   (`output_root()`) used by all entry points.

6. **Path-building by `format!` at eight-plus sites** across fit
   module files. Single `fit_stage_dir()` helper.

7. **`StartsFromProv { source: String, source_hash: Option<String> }`
   with `source` as a path string** — renamed to `StartsFromRef { stage:
   String, stage_hash: String }` using the stage name (not path) so
   the reference survives a tree reorganisation. Looking up the
   path is the caller's job.

## Exceptions and edge cases

1. **Obs subdirectories** (`obs/<obs_hash>-<obs_seed>/`) stay inside
   the simulate branch. They're a simulate-specific concept (obs
   draws derived from a trajectory), don't compose with fits, and
   the existing two-level hash structure (traj × obs) is the right
   shape. `RunKind::Simulate` doesn't carry obs metadata at the top
   level; `obs.json` stays at `obs/<hash>-<seed>/obs.json` under each
   sim run.

2. **Fit stages inside synthetic grids** (`synthetic/ds_NN/fit_<seed>/
   <stage>/`). Each stage still gets its own `FitStage` run.json,
   but the stage_hash is computed relative to the ds_NN data file
   so that regenerating one dataset doesn't invalidate stages from
   sibling datasets. This matches today's behaviour; just
   making the hashing explicit.

3. **Legacy `camdl fit scout | refine | validate` subcommands.** Go
   through `FitConfig` (v1 FitToml) which has `[fit] output_dir`.
   Post-migration the output_dir defaults to `output/` and these
   paths land at `output/fits/<fit_hash[:8]>/…` just like v2 fits.
   Keeps the legacy subcommands working.

4. **Provenance files outside the hashed tree.** Some batch outputs
   today include a top-level summary file at
   `<output_dir>/runs/manifest.json`. This predates the CAS split
   and already lives outside any individual run's hash. Moves to
   `output/sims/manifest.json`, no semantic change.

5. **Serve command** (`camdl serve`) reads the `output_dir` and
   exposes it as static files over HTTP. Needs to know the new
   tree shape. Change: replace the `/runs/` URL prefix with a
   dual-prefix router (`/sims/` and `/fits/`) that maps 1:1 to
   the on-disk structure. No other behavioural change.

## Migration

**Breaking change, no compat.** Per the project's no-backwards-
compat stance: the old `results/fits/…` and `output/runs/…` trees
are abandoned. Existing result directories stay on disk, readable
by any copy of the old binary; the new binary writes only to
`output/…`. No migration script — users who want to preserve old
results can rename the directory or re-run. Documented as a note
in the inference-spec and run-spec migration subsections.

The old `camdl fit scout/refine/validate` subcommands keep working,
just writing to the new tree location. Scripts that grep for
`results/fits/<name>/refine/mle_params.toml` need to become
`output/fits/<hash[:8]>/real/fit_<seed>/refine/mle_params.toml` —
the structured parts (`fit_<seed>/<stage>/mle_params.toml`) survive;
only the root changes.

Alternative considered: a compat-mode CLI flag that writes both
trees during a transition window. Rejected — doubles write traffic,
creates a "which tree do I read?" question every script has to
answer, and sets up exactly the kind of silent drift this proposal
exists to prevent. Clean break is cheaper.

## Implementation plan

One sequenced PR chain. Not a single commit because the type
refactors cut across crates and need independent testing.

**Commit 1 — hashing consolidation.** Move `file_content_hash`,
`compute_content_hash`, `compute_config_hash_v2`, `compute_input_hash`
from `fit::provenance` into `crate::hashing`. Rename to
`file_hash`, `fit_hash`, `stage_hash`. No behavioural change; all
call sites update in lockstep. Tests: existing hash-roundtrip tests
keep passing; add one asserting `fit_hash` and `sim_hash` produce
the same output as the old functions for a known input
(regression-against-drift).

**Commit 2 — `Run` / `RunKind` type introduction.** Add the new
module `run_meta`. Add `impl From<RunMeta> for Run`, leave the old
structs in place for one commit. Every new write-site reaches for
`Run`; reads still go through the old types. Tests: serde roundtrip
for each `RunKind` variant, field-completeness tests.

**Commit 3 — `run_paths` module + call-site migration.** Replace
the eight+ hand-rolled path constructions with the four helpers.
One commit per caller file if it helps review, or batch. Tests:
path-shape regression per helper.

**Commit 4 — switch write sites to new `Run`, output-tree root
rename.** Scout / refine / validate / pmmh / pgas / v2 dispatch
all write to `output/fits/…` instead of `results/fits/…`. `cas`
write path produces `kind = Simulate` `Run`. Old types deleted
here. Breaking change commit.

**Commit 5 — browse CLI extension.** `camdl list` walks both
branches, adds `--kind` filter, extends table output with kind
column. `camdl show / cat` update their hash-prefix matching to
search both subtrees. Tests: integration test that creates a sim
run + a fit run + lists them both.

**Commit 6 — docs + migration notes.** Update
`camdl-run-spec.md` (§2 project directory structure, §5 batch
output, §6 FitConfig output_dir semantics). Update
`camdl-inference-spec.md` (§4 fit_state paths, §3.7 replicate fits
paths). Update `camdl-book` — wherever it says `results/fits/`,
replace with `output/fits/<hash[:8]>/`.

**Total estimated LOC change:**

| Commit | Added | Removed | Net |
|---|---|---|---|
| 1 (hashing consolidation) | ~80 | ~120 | −40 |
| 2 (Run/RunKind types) | ~180 | −(delete old types in commit 4) | +180 |
| 3 (run_paths module + call sites) | ~90 | ~140 | −50 |
| 4 (write-site migration + delete old types) | ~60 | ~300 | −240 |
| 5 (browse extension) | ~140 | ~40 | +100 |
| 6 (docs) | ~80 | ~40 | +40 |
| **Total** | **~630** | **~640** | **−10** |

Honest LOC answer: close to net zero, maybe a modest reduction.
The win is structural, not textual — `Run` with an ADT discriminator
makes illegal states unrepresentable (can't have a `sim_hash` on a
fit run, can't have a `fit_toml_hash` on a simulate run); field-
name drift becomes impossible; the two cache-status enums merge;
path construction centralises. Bugs prevented by these invariants
aren't bugs anyone has to write tests for today because the type
system catches them.

## Test plan

Most existing tests survive unchanged — hashes remain stable
(commit 1 proves this with a regression test against known hashes),
per-chain outputs, fit_state.toml, mle_params.toml are unchanged.
New tests:

- **`run_kind_roundtrip`** — serde round-trip for each `RunKind`
  variant, fields present.
- **`legacy_path_migration_breaks_cleanly`** — existing `results/fits/`
  is not read by the new binary; test asserts a clear error
  message when a user points `camdl list` at the old root (not
  silently missing, not silently accepting).
- **`camdl_list_surfaces_fits`** — end-to-end: run one sim, run
  one fit, `camdl list --kind=fit` shows exactly the fit.
- **`camdl_show_resolves_by_prefix_across_kinds`** — hash prefix
  "abc12345" could theoretically collide between a sim and fit;
  assert `camdl show abc12345` handles both subtrees and produces
  a disambiguation error only if two runs in the SAME subtree
  collide (sim vs fit can't collide because they live in different
  directories).
- **`fit_stage_dir_for_synthetic_round_trips`** — `fit_stage_dir`
  with `FitSource::Synthetic { ds_idx: 3, fit_seed: 101 }` produces
  `synthetic/ds_03/fit_101/<stage>/`, matching today's layout for
  synthetic grids.
- **`hash_stability_vs_pre_unification`** — `model_hash`,
  `sim_hash`, `scen_hash`, `fit_hash` all produce the same bytes
  as the pre-unification functions for a known input. Regression
  guard: anyone who breaks this during the refactor sees CI fail
  with a crisp diff.

## Out of scope

- **Hash-scheme upgrades.** Everything keeps its existing hash
  inputs. The proposal is to *rename and consolidate*, not to
  change what's hashed. If you wanted to add, e.g., the dt to the
  fit_hash (which it currently doesn't include), that's a separate
  proposal.

- **Auto-migration of existing result trees.** Old `results/fits/`
  and `output/runs/` trees stay as-is; the new binary writes to the
  new tree; users can delete the old trees when they're done with
  them. No compat layer.

- **`camdl list` as a full TUI / richer browsing.** Keep the
  tabular output. UX polish is a follow-up.

- **Unifying `fit_state.toml` and `run.json`.** They're different
  things — `fit_state` is the handoff contract between pipeline
  stages; `run.json` is the browse-time provenance. Keeping them
  separate keeps stages composable.

- **Per-run HTTP browse via `camdl serve`.** Serve keeps working
  (trivial URL-prefix update), but a richer "browse your output
  tree from the web" UI is follow-up.

## Why this design is clean

- **Illegal states unrepresentable.** `Run` + `RunKind` means you
  can't construct a sim metadata that carries fit-specific fields
  or vice versa. Today's two parallel structs rely on convention
  alone.

- **One conceptual model: "a run is a content-hashed directory."**
  Every result in camdl maps to this model. The kinds are just
  variants of the same thing.

- **One root, one browse.** `output/` + `camdl list` is the whole
  story. Users stop asking "which directory?"; docs have one
  layout section.

- **Path construction centralises.** `run_paths::*` replaces
  eight-plus hand-rolled `format!` sites. The downstream agent's
  "refine lives at results/fits/… but also sometimes at
  output/…?" confusion goes away because there's one answer.

- **ADT eliminates a class of drift bugs.** Field-name drift
  (`version` vs `camdl_version`, `created_at` vs `timestamp`) and
  semantic drift (two cache-status enums, two config-hash funcs)
  can't recur: the type system enforces a single source of truth.

- **Honest LOC impact acknowledged.** Net ~0 lines. Win is
  structural correctness, not textual reduction. Selling this as
  a size win would be inaccurate; selling it as a correctness-
  by-construction win is honest.
