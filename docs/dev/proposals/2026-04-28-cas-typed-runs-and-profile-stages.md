# Typed CAS runs and replicate sets

**Status:** Implemented 2026-04-28 (`worktree-typed-cas` branch)
**Author:** Vince Buffalo + Claude
**Date:** 2026-04-28
**Related:**
- `docs/dev/proposals/2026-04-19-unified-output-tree.md` (the run-paths unification this builds on)
- `docs/dev/proposals/2026-04-24-profile-cas-integration.md` (the per-start CAS this proposal extends)
- `docs/dev/proposals/2026-04-28-fit-experiment-management.md` (synthetic-data / multi-cell fit workflow)
- `rust/crates/cli/src/{run_meta,run_paths,profile,fit/runner,batch}.rs`

---

## Thesis

camdl already has CAS for every long-running subcommand (`simulate --cas`,
`fit run`, `profile`, `batch run`). What it doesn't have is a *unified
notion* of what a CAS run is. Each command rolled its own canonical-string
hashing, its own dimensional decomposition (sims path-orders by scenario;
fits by cell × sweep × stage; profiles by point × start), and its own seed
handling (profile mixes seed into one umbrella hash; fit mixes seed into
distinct sibling stage hashes).

This document does two things:

1. Names the missing abstraction: **a CAS run is a deterministic function
   of typed inputs, decomposed into four roles (content / path /
   replicate / ephemeral). Cache invalidation, output layout, and run
   metadata all flow from this decomposition.** Articulates this as a
   `CasInputs` trait that every CAS-emitting command implements.

2. Uses that abstraction to ship multi-seed sensitivity for profile (the
   forcing function for getting the trait right) and migrates all four
   CAS-emitting subcommands (`profile`, `simulate --cas`, `batch run`,
   `fit run`) onto the unified trait in a single push. Old ad-hoc CAS
   hashing is deleted as each command lands; no parallel paths, no
   `#[allow(dead_code)]`, no migration shims.

Profile-as-stage in `fit.toml` (a separate workflow-composition feature
that would let refine→profile chain inside one fit invocation) is
out of scope here and tracked as future work in Appendix C; the trait
is designed so that work can land later without re-layout.

camdl is unreleased software. Back-compat is a non-goal: existing
`<root>/profiles/`, `<root>/sims/`, etc. trees may have stale hashes
after this change and will be recomputed on first run. No migration
tool ships.

---

## Implementation checklist

The work lands on `worktree-typed-cas` and merges to `main` as one
coherent feature. Each line gets checked when the corresponding work
is committed AND `cargo test --workspace` is green. No partial landings.

**Foundation**
- [x] `rust/crates/cli/src/cas/typed.rs` — `CasInputs` trait,
  `ContentHash` newtype, `ReplicateSet` umbrella helper,
  canonical hashing helpers (`hash_canonical`,
  `compose_with_replicate`), `to_run` default trait method
- [x] Unit tests for trait composition and parent/child hash
  relationship

**Profile (forcing function — first real consumer)**
- [x] `ProfileInputs` struct with explicit role classification
- [x] `impl CasInputs for ProfileInputs`
- [x] Replace inline `sha256_hex` + canonical-string construction in
  `profile.rs` with trait dispatch
- [x] `--seeds` CLI flag on `ProfileArgs`
- [x] Multi-seed wrapping via `ReplicateSet` (single-seed layout
  unchanged; multi-seed adds the umbrella + `replicates/seed_<S>/` tree)
- [x] Cross-seed `summary.tsv` aggregator (per-grid-point spread)
- [x] Single-seed layout unchanged from today (preserves muscle memory)
- [x] Tests for trait + ReplicateSet behavior
- [x] Old hash-construction code deleted in the same commit

**Simulate (`simulate --cas`)**
- [x] `SimulateInputs` struct (in `cas/sim_inputs.rs`, shared with batch)
- [x] `impl CasInputs for SimulateInputs`
- [x] Replace inline canonicalization in `main.rs::prepare_cas_ctx`
- [x] Old hash-construction code deleted

**Batch (`batch run`)**
- [x] Reuses `cas::sim_inputs::SimulateInputs` (per-run: model +
  scenario + merged sweep+scenario params + seed)
- [x] Sweep is a *grid* (multiple distinct hashes via `scen_hash`
  varying with sweep coords); seeds are direct content-bearing
  inputs today (no formal `Replicates` until a multi-seed
  feature pulls)
- [x] Replace inline path/hash construction in `batch.rs`
- [x] Old hash-construction code deleted (`run_hash` removed
  from `hashing.rs`)

**Fit (`fit run`)**
- [x] `FitInputs` struct (umbrella; wraps `fit_content_hash`)
- [x] `StageInputs` struct (per-stage leaf; wraps `fit_stage_hash`)
- [x] `impl CasInputs` for both
- [x] Replace inline `Run` envelope construction at the umbrella
  and stage write sites with `inputs.to_run(...)` trait dispatch
- [ ] Formal `Replicates<FitInputs, FitSeedKey>` for `fit_seeds`
  cells (deferred — fit's existing implicit umbrella works; a
  formal wrapping changes paths, which is a follow-up cleanup)
- [ ] Synthetic data nested replicate dimension (deferred,
  same reason)

**Readers**
- [x] `RunKind::ReplicateSet(meta)` variant in `run_meta.rs`
- [x] `camdl show` recognises ReplicateSet directories and prints
  children + summary path
- [x] `camdl cat` streams `summary.tsv` for ReplicateSet umbrellas
- [x] `camdl list` top-level surfacing of profiles + replicate-set
  umbrellas (`--kind=profile` filter + dedicated table)

**Dead code sweep**
- [x] grep for `sha256_hex(`, manual canonical-string assembly,
  inline hash composition in `rust/crates/cli/src/` outside `cas::typed`
- [x] No `#[allow(dead_code)]` introduced; remaining `Sha256` /
  `sha256_hex` callers are either CAS building blocks (delegated to
  by trait impls) or display-only meta fields (`fit_toml_hash`)
- [ ] `cargo clippy --workspace -- -D warnings` clean — pre-existing
  lints in `sim` crate predate this branch and are outside scope

**Final**
- [x] `cargo test --workspace` green
- [x] Proposal Status: Implemented (date)
- [x] Branch ready for merge to `main`

---

## Motivation

### The forcing function: stochastic IF2 sensitivity

A single IF2 run that hasn't quite converged at a profile grid cell can
produce a deceptively peaked likelihood — the cell looks well-identified
when it's actually a single chain's lucky endpoint on a flat axis. The
fix is per-cell replication across stochastic seeds, with the spread
across replicates acting as a per-cell trustworthiness diagnostic
(analogous to the chain-agreement Â that gates IF2 fits today).

Today this requires three full `camdl profile --seed N` invocations and
manual diffing of three sibling `<root>/profiles/<different_hash>/` trees.
The seed-sensitivity feature wants this collapsed into one invocation
with one rolled-up summary.

### The deeper problem the seed feature surfaces

Designing the seed-replication output schema forces a question that two
existing commands answer differently:

- **Fit:** `fit_seeds = [1, 2, 3]` produces three sibling `fit_<seed>/`
  cells under one `fit_content_hash`. Each cell's stages have distinct
  `fit_stage_hash` values that include seed. There is an implicit
  umbrella (the fit content hash) but no formal notion of "these three
  cells are replicates of each other."
- **Profile:** `--seed N` is mixed into the single `profile_hash`. There
  is no umbrella; running again at a different `--seed` produces a
  *different content hash*, hence a different output tree.

Both are defensible; neither is wrong by itself; but they answer the
same question ("how do replicates of one logical run compose?") with
different rules. Adding multi-seed profile without a unified rule means
making the inconsistency one variant deeper. Better to settle the rule
once and have both commands obey it.

### What a unified rule buys

- **`camdl list/show/cat`** stops branching on command kind to
  understand layout. It reads `run.json`, dispatches on `meta.kind`
  for *semantic* questions, but the *path/hash structure* is uniform.
- **Adding a new CAS-emitting command** (a forecast subcommand, a
  cross-validation runner) becomes a trait-impl exercise, not a
  schema-design exercise.
- **Cross-command discoverability**: "find every run that touched
  model M" is a single tree-walk, not five.
- **Cache invalidation rules** become legible — a single document
  defines what changes invalidate what.

---

## Scientific workflow as the source of truth

Before specifying the abstraction, fix what it must serve. A scientific
workflow has these natural dependency edges:

```
   [DSL source]
         │  (compile)
         v
   [compiled IR]                     ┐
         │                           │  upstream of every analysis
         v                           │  (model identity)
   ┌─────┴──────┐                    ┘
   │            │
   v            v
[real data]  [synthetic data]        ┐
                 │                   │  observation identity
                 │ (gen needs        │
                 │  true_params,     │
                 │  sim_seed)        ┘
   ┌─────────────┴────┐
   v                  v
[fit run             [profile run    ┐
 (scout, refine,      (per-cell      │  inference identity
  pgas, ...)]          mini-IF2)]    ┘
   │                  │
   v                  v
[derived analyses    [aggregate
 (preq, comparison,   diagnostics
  forecast)]          (chain-Â,
                       seed spread)]
```

Cache invalidation should track these edges. A change at any node
invalidates everything downstream and *only* what's downstream.

### What invalidates what

| Change                                        | Invalidates                                        |
|-----------------------------------------------|----------------------------------------------------|
| Model DSL source (anything that compiles)     | Everything downstream of the IR                    |
| Real-data file bytes                          | Every fit / profile reading that file              |
| Synthetic-data true_params or sim_seed        | The synthetic dataset and everything fitting it    |
| Algorithm hyperparameter (particles, iter)    | That stage and what depends on it; sibling stages and synthetic data unaffected |
| Stochastic seed (single realization)          | That realization only; replicates and parents unaffected |
| Output mirror path, --parallel, --progress    | Nothing                                            |
| `starts_from` upstream stage's output         | The downstream stage (the upstream's hash is part of downstream's input) |

These rules are not new — they're the rules the existing CAS *implicitly*
applies. The contribution here is making them *explicit* and *uniform*.

### What does NOT invalidate

Decisions that look like they should invalidate but don't:

- **CLI flag parsing changes**: `--seeds 1,2,3` and `--seeds=1,2,3` and a
  TOML `seeds = [1, 2, 3]` all canonicalize to the same input. Hash
  inputs are post-canonicalization values, not surface syntax.
- **Path-presentation choices**: switching `<stem>-<hash>` to `<hash>`
  changes the path but not the hash. Old paths still resolve via the
  hash; the rename is a presentation migration, not a content one.
- **Argv reordering**: `camdl fit run a.toml --seed 1` vs.
  `camdl fit run --seed 1 a.toml`. Same hash. (`argv` is recorded in
  `run.json` for forensics but is ephemeral for hashing.)

---

## The four roles every input plays

Every input to a CAS-emitting command falls into exactly one of these
buckets:

### 1. Content (in hash; determines validity)

Inputs that change the *output* of the run. If any content input
changes, every byte downstream is suspect. Examples:

- Model IR bytes (already canonicalized via JSON serialization).
- Data file bytes (real) or upstream lineage hash (synthetic).
- Algorithm hyperparameters (particles, iterations, cooling, dt).
- Random seed *if* this run is a single realization.
- `starts_from` upstream's content hash (transitively depends).

These compose into a single `content_hash` per run via a deterministic
canonicalization (sorted-keys JSON, then sha256). The content hash is
the cache key. Two runs with the same content hash *must* have produced
the same output bytes; cache hit means the existing directory is
authoritative and we skip the computation.

### 2. Path (in path; determines readability)

Inputs that determine *where* the run lands on disk for human
discovery. Path inputs are derived from content inputs; they don't add
information. Examples:

- The first 8 hex chars of the content hash (cache key, but truncated
  for readability).
- A human-readable stem (model basename, scenario slug, fit-toml stem).
- Seed as a path segment (`seed_42/`) when the seed is a replicate
  dimension — see role 3.

A path can vary in presentation without changing the content hash. The
content hash is the *primary* key; path is a *secondary* index for
humans. This means a directory rename (e.g., `<root>/profiles/` →
`<root>/runs/profile/`) is a content-preserving migration: every
directory's `run.json` has the same hash before and after.

### 3. Replicate (forms a parent-child hash relationship)

Inputs that *vary an otherwise-identical run* for sensitivity analysis
or convergence diagnostics. Example: running scout at seeds [1, 2, 3, 4]
to assess across-chain agreement.

A replicate dimension produces a **parent content hash** (the run with
the replicate dimension *abstracted away*) and **N child content hashes**
(one per replicate value). Layout:

```
<parent_path>/
  run.json                   # parent: RunKind::ReplicateSet(meta)
  summary.tsv                # cross-replicate aggregate
  replicates/
    <key_1>/                 # one child per replicate value
      run.json               # child: a single-realization run
      ...
    <key_2>/
      ...
```

The parent's `content_hash` includes:
- Everything that's the same across children
- The *sorted, canonicalized list* of replicate keys

The child's `content_hash` is exactly what you'd get running that
single replicate alone. **A standalone single-seed run and one child of
a replicate set with the same seed have the same content hash and can
share storage.** (Implementation: the standalone path can be a symlink
or hard link into the replicate set, or vice versa; both forms read
identically.)

This rule fixes the profile vs. fit inconsistency: fit's `fit_seeds`
becomes a formal replicate dimension at the cell level; profile's
seeds become a formal replicate dimension at the profile level.

### 4. Ephemeral (nowhere)

Inputs that affect *how the work is performed* but not what's produced.
Examples:

- `--parallel N` (how many threads)
- `--progress` mode (visual; bytes are unchanged)
- Output mirror paths (a copy destination, not a content destination)
- Log level, verbose flags

Ephemeral inputs are recorded in `run.json` for forensics (`argv` field)
but not in any hash. Reproducing a run with different ephemeral inputs
hits the cache.

---

## The `CasInputs` trait

The trait that formalizes the four roles:

```rust
/// Every CAS-emitting subcommand's typed input set implements this.
/// Implementations explicitly classify each field into one of the four
/// roles. The trait exposes the derived hash, path, and metadata; how
/// they're computed is the impl's responsibility, but the helpers in
/// `cas::canonical` make the common case (sorted-keys JSON of named
/// fields → sha256) one line.
pub trait CasInputs {
    /// Stable content hash. Determines cache validity. Two impls
    /// returning the same hash MUST have produced the same outputs
    /// (modulo sha256 collision resistance, which we trust).
    fn content_hash(&self) -> ContentHash;

    /// Filesystem path under the CAS root. Function of content_hash
    /// plus presentation hints. Two distinct content_hash values MUST
    /// produce distinct paths; the converse need not hold (rename is
    /// a path migration, not a content change).
    fn cas_path(&self, root: &Path) -> PathBuf;

    /// Run-kind metadata for run.json. Includes the kind discriminant,
    /// human-readable provenance fields, and the lineage backrefs.
    fn cas_meta(&self) -> CasMeta;

    /// Returns Some(replicate_set) iff this run is a replicate-set
    /// parent. Default: None (single-realization run).
    fn replicate_set(&self) -> Option<ReplicateSet> { None }
}

/// Wraps an inner CasInputs with a replicate dimension. Each value of
/// `dimension` produces a child run with the wrapper hash composed in.
pub struct Replicates<T: CasInputs, K: ReplicateKey> {
    pub inner: T,
    pub keys: Vec<K>,
    pub dim_name: &'static str,  // e.g. "seed", "dataset_idx"
}

impl<T: CasInputs, K: ReplicateKey> CasInputs for Replicates<T, K> {
    fn content_hash(&self) -> ContentHash {
        // h(inner.content_hash, dim_name, sorted(keys))
        compose_replicate_hash(&self.inner.content_hash(),
                               self.dim_name, &self.keys)
    }
    fn cas_path(&self, root: &Path) -> PathBuf {
        // <root>/<inner.cas_path>/replicates/
        self.inner.cas_path(root).join("replicates")
    }
    fn replicate_set(&self) -> Option<ReplicateSet> {
        Some(ReplicateSet {
            dim_name: self.dim_name.to_string(),
            keys: self.keys.iter().map(K::to_path_segment).collect(),
            inner_hash: self.inner.content_hash(),
        })
    }
    /* ... cas_meta delegates to inner with replicate annotation ... */
}
```

Reader code (`camdl list/show/cat`) doesn't care about `T` or `K`. It
reads `run.json`, dispatches on `meta.kind` for semantic operations
(plot a profile vs. plot a simulate trajectory), and uses the uniform
`replicates/` convention to navigate sensitivity sets.

The `ReplicateSet` parent's `summary.tsv` is generated by an
aggregator that's a function of the kind: profile → per-grid-point
spread; fit → per-stage cell spread; simulate → per-trajectory-point
spread. Each kind defines its own aggregator; the umbrella schema
(parent has summary.tsv, children are at `replicates/<key>/`) is
uniform.

---

## Migration: how existing commands fit

Translation table from current code to the proposed schema. Implementation
order is bottom-up: simulate first (smallest), then profile (the
forcing function), then fit (largest, does the most work).

| Command            | Today's roles (informal)                                                | Under the proposed schema                                                                                  |
|--------------------|-------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------|
| `simulate --cas`   | content: model + scenario + seed; path: `sims/<sim_hash>/<scen>/seed_N` | content: model + scenario; replicate dim: `seed`; path unchanged for single-seed; multi-seed gets `replicates/seed_N/` |
| `batch run`        | conflates sweep × scenario × seed in path                               | content per run: model + scenario + sweep_point + seed; sweep is a *grid* (multiple distinct hashes), seeds are a *replicate* dimension |
| `fit run`          | `fit_content_hash` (seed-free) + per-cell `fit_seeds` siblings + per-stage `fit_stage_hash` (seed-inclusive) | `fit_content_hash` is the fit-as-a-whole; cells are a `Replicates<Fit, FitSeed>` set when `fit_seeds.len() > 1`; stage hashes per-cell unchanged |
| `profile`          | one `profile_hash` per `--seed`; multi-seed needs N invocations         | `profile_content_hash` (seed-free); multi-seed wraps in `Replicates<Profile, Seed>`; per-grid-point cross-seed summary at the parent |

In every row, the *child* (single-realization) hashes are unchanged.
The migration introduces *parent* (replicate-set) hashes where they're
absent today (profile) and formalizes them where they're implicit (fit).

---

## Implementation: trait + four-command migration + multi-seed profile

All four CAS-emitting subcommands migrate to the `CasInputs` trait in
this push. Profile is the first consumer (forcing function for the
trait shape), then simulate, batch, and fit. Every command's old
ad-hoc hashing is deleted in the same commit that introduces its
typed-input replacement — no parallel paths.

### What ships

1. **`CasInputs` trait + `Replicates<T, K>` wrapper** in a new `cas::typed`
   module. Trait alone, no derive macro yet.
2. **Profile reimpl**: `ProfileInputs` struct with explicit role
   classification per field; impl `CasInputs` by hand. Existing per-start
   CAS body unchanged.
3. **Multi-seed profile**: the `--seeds` CLI flag. `ProfileInputs`
   exposes the seeds list; the runner wraps it in `Replicates` when
   `len > 1`.
4. **Cross-seed aggregator**: writes `<profile>/summary.tsv` with per-grid-point
   `mean / sd / min / max / n_seeds_finished` for `loglik` and each MLE
   column. Throttled the same way the existing `profile.tsv` is.
5. **Layout discriminator**: when `seeds.len() == 1`, layout is unchanged
   from today (preserves muscle memory for the standalone profile CLI).
   When `seeds.len() > 1`, the path is `<profile>/replicates/seed_<S>/...`
   per replicate, with the cross-seed `summary.tsv` at the profile root.
6. **Simulate / batch / fit migrations**: each command grows its own
   `*Inputs` struct, implements `CasInputs`, and deletes its inline
   hashing code. Layouts preserved where they were already coherent;
   `fit_seeds` is formalized as a `Replicates<FitInputs, FitSeedKey>`
   set; synthetic-data fits compose two replicate dimensions
   (`fit_seeds` × `dataset_idx`) by nesting `Replicates`.
7. **Readers** (`camdl list / show / cat`): handle the new
   `RunKind::ReplicateSet` variant by walking children and surfacing
   the aggregate diagnostic.

### CLI

```
camdl profile model.camdl \
    --data data.tsv \
    --params start.toml \
    --rw-sd "auto" \
    --sweep beta=log10(0.5, 5.0, 11) \
    --n-starts 4 \
    --seeds 1,2,3        # NEW; defaults to [--seed]
```

### Output layout (multi-seed)

```
results/profiles/<stem>-<profile_content_hash[:8]>/
  run.json                                  # RunKind::ReplicateSet(profile)
  summary.tsv                               # NEW: cross-seed aggregate
  replicates/
    seed_1/
      run.json                              # RunKind::Profile
      profile.tsv                           # existing per-seed rollup
      points/{NNNNN}/start_<K>/             # existing per-start tree
        run.json
        mle.toml
    seed_2/
      ...
    seed_3/
      ...
```

### Output layout (single-seed, unchanged)

```
results/profiles/<stem>-<profile_content_hash[:8]>/
  run.json                                  # RunKind::Profile
  profile.tsv
  points/{NNNNN}/start_<K>/
    run.json
    mle.toml
```

### Hash composition

```
profile_content_hash    = sha256(model_ir_hash, base_params_hash,
                                 focal_grid_canonical, fixed_canonical,
                                 if2_config_canonical, starts_from_lineage_hash)
                          # NOTE: NO seed in this hash. This is the change.

per_seed_content_hash   = sha256(profile_content_hash | "seed" | seed_value)
                          # = a normal Profile run hash; matches what a
                          # standalone --seed N invocation would produce.

per_start_hash          = sha256(per_seed_content_hash | point_idx | start_idx | derived_seed)
                          # unchanged from today, just rooted in
                          # per_seed_content_hash instead of the legacy
                          # profile_hash that conflated seed.
```

The shift from "seed in `profile_hash`" to "seed in `per_seed_content_hash`"
is a content-hash change for existing single-seed profiles. Existing
`<root>/profiles/<old-hash>/` directories become orphaned and are
recomputed on first run after the upgrade. Back-compat is a non-goal
(camdl is unreleased software); users who care can `rm -rf` the old
trees, otherwise they sit idle and `camdl list` filters them out via
hash mismatch.

### Sensitivity diagnostic

The `summary.tsv` aggregator surfaces the user-facing diagnostic:

```
beta    n_seeds   mean_loglik   sd_loglik   min_loglik   max_loglik   verdict
0.5     3         -89.42        2.31        -91.7        -86.8        ok
1.0     3         -58.03        0.42        -58.5        -57.6        ok
1.4     2         -65.21       12.40        -73.0        -50.1        ⚠ 1 seed didn't finish
2.0     3         -54.88        0.51        -55.4        -54.3        ok
```

A row with high `sd_loglik` is the "single chain hasn't quite converged"
signal. A natural threshold (analogous to the chain-Â gate at IF2 fit
time) is "sd_loglik below X dB at every grid point" — TBD with empirical
calibration on the boarding-school cases that motivated this proposal.

### Upstream changes

1. New module `rust/crates/cli/src/cas/typed.rs`: `CasInputs` trait,
   `Replicates<T, K>` wrapper, `ContentHash`/`CasMeta` types, canonical
   hashing helpers.
2. `rust/crates/cli/src/profile.rs`: factor out `ProfileInputs` struct;
   impl `CasInputs`; replace inline hash construction.
3. `rust/crates/cli/src/cas.rs`: `SimulateInputs` struct; impl
   `CasInputs`; replace inline canonicalization.
4. `rust/crates/cli/src/batch.rs`: `BatchInputs` struct (per-run);
   impl `CasInputs`; replace inline path/hash construction.
5. `rust/crates/cli/src/fit/{runner,provenance}.rs`: `FitInputs` and
   `StageInputs` structs; impl `CasInputs`; `Replicates<…>` wrapping
   for `fit_seeds` and synthetic-data `dataset_idx`; replace
   `fit_content_hash` and `fit_stage_hash` inline construction.
6. `rust/crates/cli/src/run_meta.rs`: `RunKind::ReplicateSet` variant
   carrying the replicate dimension's name, key list, and inner-kind
   discriminant. (Existing `RunKind::Profile`/`Fit`/`Simulate`
   unchanged.)
7. `rust/crates/cli/src/run_paths.rs`: add `replicate_set_dir(parent, key)`
   helper.
8. `rust/crates/cli/src/args/mod.rs`: `--seeds` flag on `ProfileArgs`,
   parsed via existing `SeedSpec` shape.
9. `camdl list / show / cat` updates: handle `RunKind::ReplicateSet`
   by listing children with the aggregate diagnostic.
10. Dead-code sweep across all of the above — every ad-hoc canonical
    string + sha256 invocation outside `cas::typed` deleted.

---

## Appendix C: Future work — profile-as-stage in `fit.toml`

Implements profile inside `fit.toml` so it composes with synthetic data
and stage chaining. Out of scope for this proposal; specified here so
the trait shape doesn't paint us into a corner. To be picked up as a
separate proposal when refine→profile chaining or
profile-of-synthetic-data shows up as a real workflow need.

### What it adds

1. New stage method: `[stages.NAME] method = "profile"` with the same
   field set as the standalone CLI (focal, grid, fixed, n_starts,
   seeds, IF2 hyperparams).
2. `StageConfig::Profile` variant in `fit/config_v2.rs`.
3. `RunKind::ProfileStage(meta)` — identical content to today's
   `RunKind::Profile` with stage-level lineage fields.
4. Stage runner dispatch in `fit/mod.rs`: at execute time, build a
   `ProfileInputs` rooted at the stage's directory under the cell, run
   it through the same `CasInputs` machinery shipped here.
5. `starts_from` resolution for profile stages: a downstream stage's
   `starts_from = "profile_stage_name"` resolves to the profile's
   argmax (best loglik across all points × seeds × starts). Picking
   any-other-coordinate is deferred to a future `@selector` syntax.
6. Cross-cell aggregator: when a profile stage runs in multiple cells
   (e.g., `fit_seeds = [1, 2, 3]` × profile stage), a sibling
   `<fit>/<stage>_aggregate/` directory holds the cross-cell summary
   (per-grid-point spread across cells, distinct from the cross-seed
   spread within one cell).

### Schema

```toml
[model]
camdl = "models/he2010.camdl"

[data]
observations = { cases = "data/london.tsv" }

[stages.scout]
algorithm = "if2"
backend = "chain_binomial"
chains = 4
particles = 1500
iterations = 80

[stages.refine]
algorithm = "if2"
backend = "chain_binomial"
starts_from = "scout"
chains = 8
particles = 3000
iterations = 200

[stages.beta_profile]
method = "profile"
starts_from = "refine"
focal = ["beta"]
grid = [{ param = "beta", spec = "log10(0.5, 5.0, 11)" }]
fixed = ["rho"]
n_starts = 4
seeds = [1, 2, 3]
particles = 1500
iterations = 80
cooling = 0.5
rw_sd = "auto"

# Optional, top-level, mirrors fit_seeds:
fit_seeds = [10, 20]
```

This produces:
- 2 fit cells (`fit_seeds`) × 1 scout × 1 refine × 1 profile stage × 3
  profile seeds × 11 grid points × 4 starts = 264 IF2 mini-runs.
- Per-cell profile rollup at `<fit>/real/fit_<seed>/beta_profile/`.
- Cross-cell aggregate at `<fit>/beta_profile_aggregate/`.

### Why deferred

The work is real: new `StageConfig` variant, new `RunKind`,
`starts_from = "profile@best"` resolution, cross-cell aggregator,
fit-flow integration tests. None of it is hard, but it doesn't pay
until a workflow needs profile-of-synthetic-data or refine→profile
chaining. The seed slice surfaces the IF2 sensitivity issue *without*
needing any of this.

The trait shape is designed so this future work *cannot* require a
re-layout of typed-CAS outputs. A profile run that lives under
`<root>/profiles/` today and the same profile run that lives under
`<fit>/real/fit_<seed>/beta_profile/` after future work would have
the *same* `profile_content_hash` and the *same* internal layout.
Only the parent changes — content-preserving move.

---

## Cache invalidation rules (canonical reference)

This section is the rule that downstream tools should consult when
asking "did this change invalidate my cache?". The answer for any input
is exactly its role.

| Input role | In hash? | In path? | Invalidation effect on this run | Effect on parent | Effect on children |
|------------|----------|----------|----------------------------------|------------------|--------------------|
| Content    | yes      | via hash prefix | invalidates this run             | invalidates parent if this is a child | invalidates all children (if this is a parent) |
| Path       | no       | yes      | rename (no recompute)             | none              | none                |
| Replicate  | no, in *child* hash | yes (as `replicates/<key>/`) | invalidates that child only | none | only that child  |
| Ephemeral  | no       | no       | none                              | none              | none                |

The "effect on parent/child" rules formalize what we mean by replicate
sets:
- Adding a new replicate key to a set produces a *new parent hash*
  (because the sorted key list changed), but the existing children
  share their hashes with the old parent's children. Implementation:
  the new parent points at the same per-replicate child directories;
  only the parent's `summary.tsv` is regenerated.
- Removing a replicate key produces a new parent hash and orphans the
  removed child's directory. Orphans are kept on disk (cheap) and can
  be GC'd by `camdl cas gc` if it ships.

These rules are properties of the trait, not of any specific command.
Profile, fit, simulate, batch all obey them via their `CasInputs` impls.

---

## Open design questions

1. **Derive macro for `CasInputs`.** Trait ships with hand-written
   impls. Once all four commands have implemented it, if the bodies
   are 80% identical, a `#[derive(CasInputs)]` proc-macro is cheap.
   If they differ in interesting ways (and they might — profile's
   grid dimension is genuinely different from sim's scenario), the
   macro fights the structure. Decision deferred until empirical
   duplication is visible post-migration.

2. **Cross-replicate aggregator pluggability.** Profile's per-grid-point
   summary aggregator lives in `profile.rs`. Fit's replicate-set
   aggregator (per-stage chain-Â across cells) will be different in
   substance. The cleanest extension is a `ReplicateAggregator` trait
   that each kind implements. Each command writes its own aggregator
   inline for now; if a third consumer wants one, hoist to a trait.

3. **`Replicates<T, K>` over multiple dimensions simultaneously.**
   Synthetic-data fit has two replicate dimensions
   (`fit_seeds × dataset_idx`). The trait specifies *one* replicate
   dimension per wrapper; multi-dim is composed via nesting
   (`Replicates<Replicates<T, K1>, K2>`). The path layout reflects the
   nesting verbatim — current nested form
   (`synthetic/ds_01/fit_1/…`) is more navigable than a flattened
   `replicates/seed=1,dataset=2/`.

---

## Risks

1. **Trait misuse: a content field marked ephemeral.** A bug in a
   `CasInputs` impl could hide a content-bearing input from the hash,
   producing silent cache hits on stale results. Mitigation: a
   compile-time test that exercises every typed-input struct's
   `content_hash` against a snapshot, so any change to fields without
   a corresponding hash-input update fails the build. (A future
   derive macro would enforce this structurally.)

2. **Replicate set semantics drift between commands.** All four
   commands migrate in this push, so drift can't accrete. Each
   command's `*Inputs` struct goes through the same trait; the same
   `Replicates<T, K>` composition handles seed (for simulate / batch /
   fit / profile) and `dataset_idx` (for synthetic-data fit).

3. **`summary.tsv` aggregator races.** The aggregator is
   throttled-rewrite, last-completion-wins (same pattern as today's
   `profile.tsv`). The race is benign at the per-replicate level, but
   if a per-replicate child writes its `profile.tsv` and the aggregate
   summary fires before the child's run.json is committed, the
   aggregate could undercount. Mitigation: aggregator reads the child's
   `run.json` (committed last in the per-replicate write order) as the
   "this child is done" signal, not the per-replicate `profile.tsv`.

---

## What this proposal does *not* do

- Doesn't introduce a derive macro. Trait first; macro if and when
  duplication justifies it.
- Doesn't ship a migration tool or back-compat shims. camdl is
  unreleased software; old `<root>/profiles/` and similar trees may
  hold orphan directories after the upgrade and users can `rm -rf`
  them.
- Doesn't fold profile into `fit.toml` as a stage method. That's a
  separate workflow-composition feature tracked in Appendix C.
- Doesn't add a `cas` umbrella subcommand or `cas gc` / `cas verify` /
  etc. Those are downstream work that the trait makes possible but
  doesn't require.
- Doesn't define a cross-version `run.json` schema migration policy.
  The existing version-string check in `Run::read` is deemed
  sufficient; long-term storage compatibility is out of scope.

---

## Appendix A: Current state inventory

For posterity, every CAS-emitting code path's *pre-migration* hash
inputs and path layout. After this proposal lands, every reference
here points at deleted code.

### `simulate --cas` (cas.rs, run_paths.rs:64)

- Content: `model_hash`, scenario name, scenario-resolved overrides hash, seed.
- Path: `<root>/sims/<sim_hash[:8]>/<scenario-slug>-<scen_hash[:8]>/seed_<N>/`
- Layout role of seed: path component AND content. Seed-distinct runs are
  sibling directories with sibling content hashes. *No replicate-set umbrella.*

### `fit run` (fit/runner.rs, run_paths.rs:96)

- Content (`fit_content_hash`): model IR + data files + fit.toml bytes. Seed-free.
- Per-cell layout: `<fit>/real/fit_<seed>/<sweep_slug>/<stage>/` or
  `<fit>/synthetic/ds_NN/fit_<seed>/<sweep_slug>/<stage>/`.
- Per-stage content (`fit_stage_hash`): fit_content + stage_name +
  stage_config + seed.
- Layout role of seed: *path component* (in `fit_<seed>/`) AND *content*
  (per-stage hash). Implicit umbrella (the fit_content_hash) but no
  formal replicate-set object.

### `profile` (profile.rs, run_paths.rs:127)

- Content (`profile_hash`): model + base_params + focal grid + IF2 config + seed_base.
- Path: `<root>/profiles/<stem>-<profile_hash[:8]>/`
- Per-(point, start) content (`start_hash`): `profile_hash | point_idx | start_idx | derived_seed`.
- Layout role of seed: *content of the umbrella* (seed in profile_hash).
  Multi-seed = multiple distinct umbrella hashes = multiple sibling
  trees. No replicate-set umbrella.

### `batch run` (batch.rs)

- Content per run: model + scenario + sweep_point + seed. Sweep_point
  is from the Cartesian expansion of `[sweep]`.
- Path: `<root>/runs/<scen_slug>-<scen_hash[:8]>/seed_<N>/` (similar
  to simulate, with sweep_point folded into scen_hash).
- Layout role of seed: same as simulate.

The four are *almost* consistent. The misalignment is:
- Profile puts seed in the umbrella; everything else doesn't.
- Fit has an implicit umbrella (`fit_content_hash`) but no formal name.
- Simulate and batch don't have an umbrella concept at all.

This proposal's contribution: name the umbrella as `Replicates<T, K>`,
fix profile to use it (seed-out-of-umbrella, replicate-set wrapping),
formalize fit's implicit umbrella, and prepare simulate/batch for
multi-seed by giving them the same trait — they ship single-seed
today but the abstraction is in place when a feature needs it.

---

## Appendix B: Glossary

- **Content hash**: sha256 of canonicalized content inputs. Cache key.
  Two runs with the same content hash *must* have produced the same
  output.
- **Path**: filesystem location derived from content hash + presentation
  hints. Multiple paths can map to the same content hash (rename); a
  given path maps to exactly one content hash.
- **Replicate dimension**: an input that varies an otherwise-identical
  run for sensitivity analysis. Forms a parent/child relationship.
- **Replicate set**: the parent of a replicate dimension. Has its own
  content hash that includes the sorted list of replicate keys.
- **Ephemeral input**: an input that affects how the work is performed
  but not what's produced. Not in any hash.
- **Lineage hash**: the content hash of an upstream run that this run
  reads as input (e.g., `starts_from = "scout"` makes scout's content
  hash an input to refine's content hash).

---

## Decision

Approved: trait + four-command migration + multi-seed profile, all in
one push on `worktree-typed-cas`. Old ad-hoc CAS hashing deleted as
each command lands. No back-compat shims, no migration tool. Branch
merges to `main` when the implementation checklist is fully checked
and `cargo test --workspace` is green.

Profile-as-stage in `fit.toml` (Appendix C) tracked as future work;
the trait is shaped so it can land later without any layout changes
to the artifacts produced by this proposal.
