# Audit: `camdl fit` experiment management — subcommands, CAS layout, gaps

Date: 2026-04-27
Project: camdl
Tags: cli, audit, cas, fit, ux, experiments
Verified-against: HEAD = `d9d5ab7`
Proposal: [`docs/dev/proposals/2026-04-28-fit-experiment-management.md`](../proposals/2026-04-28-fit-experiment-management.md)
  — the design that grew out of this audit. Read this audit for
  *why*; read the proposal for *what we'll build, in what order*.

## The problem (top-of-file)

A scientist iterating on a model — say, a measles SEIR — does not run *one*
fit. They run **dozens**, each varying along one or more axes:

- **Bounds** widened or narrowed on a parameter (R0 from `[1, 100]` to
  `[40, 80]` after seeing scout chains drift to 100).
- **Estimate ↔ fixed split** (move `iota` from `[fixed]` to `[estimate]`
  to see if the data identifies it; pin `gamma` after profile-likelihood
  shows it's collinear with σ).
- **Priors** added or tuned (`log_normal(log(50), 0.4)` on R0 to regularize).
- **Data variations** (full 21-year vs 5-year window; swap measles for
  rubella; subset of streams).
- **Stage variations** (just scout vs scout+refine vs scout+pgas; different
  cooling schedules).
- **Sweep cells within a single fit** (--sweep over fixed values to scan
  a hyperparameter slice).
- **Synthetic SBC replicates** (N datasets × M fit_seeds for calibration).

Each of these — except the last two — produces a **separate fit
directory**, keyed by content hash. Today camdl writes them faithfully
(every variation produces a distinct `results/fits/<stem>-<hash[:8]>/`
tree, never silently overwritten — that part of the CAS is solid).
But the **user-facing tools to navigate, compare, clean up, and reason
about the experiment workspace are largely absent**.

What this looks like in practice:

```
$ ls results/fits/
fit_he2010-04ab12cd  fit_he2010-1f3c45ee  fit_he2010-2a8b7901  fit_he2010-3c4d5e6f
fit_he2010-4f5e6d7c  fit_he2010-5a1b2c3d  fit_he2010-67abcdef  fit_he2010-7890abcd
fit_he2010-89def012  fit_he2010-9a8b7c6d  ...
```

A user with 30 fits of the same model under bounds-exploration cannot
ask:

- "Which of these had `iota` in `[estimate]`?"
- "What's the best loglik across all converged fits?"
- "Show me the fits that didn't converge so I can re-run or delete them."
- "Which two fits differ only in `R0` bounds? What did that change?"
- "Delete every fit older than two weeks where the gate failed."
- "I want a table: one row per fit, columns = config-diff summary +
  best loglik + Â + ESS at MLE."

`camdl list` gives them a flat directory list. `camdl fit summary` is
single-fit, MLE-only. `camdl fit diff` is config-only (doesn't read
results). Nothing exists to aggregate or prune. The CAS does the
correct thing on disk; the **experiment-management surface above the
CAS is the gap**.

This audit catalogs what exists, what each command actually does (verified
in code at HEAD = `d9d5ab7`), and where the gaps are. A separate proposal
will design the missing surface.

---

## 1. The CAS / output tree (verified)

### 1.1 Default root

`crates/cli/src/run_paths.rs:25`:

```rust
pub const DEFAULT_OUTPUT_ROOT: &str = "results";
```

Resolution precedence: CLI `--output` > config-file `output_dir` > default
`results/` (line 29-33). **Note:** docs in older proposals say `output/`;
the code says `results/`. Code wins. Some doc strings still say `output/`
— minor staleness but worth a sweep.

### 1.2 Tree shape (actual, post-2026-04-19 unification)

```
results/
├── sims/                                        # camdl simulate --cas, batch run
│   └── <stem>-<sim_hash[:8]>/                   # model + base params + backend + dt + version
│       └── <scenario-slug>-<scen_hash[:8]>/     # scenario delta (enable/disable/overrides)
│           └── seed_<n>/
│               ├── traj.tsv                     # trajectory
│               ├── run.json                     # RunKind::Simulate
│               └── obs/<obs_hash[:8]>-<obs_seed>/  # optional, per (obs_model, obs_seed)
│                   ├── <stream>.tsv
│                   └── obs.json
│
├── fits/                                        # camdl fit run
│   └── <stem>-<fit_hash[:8]>/                   # fit_content_hash: model + data + fit.toml bytes
│       ├── run.json                             # RunKind::Fit (top-level)
│       ├── real/                                # real-data fits
│       │   └── fit_<seed>/                      # per fit_seed (start-sensitivity grid)
│       │       └── <sweep_slug>/                # only when --sweep was used
│       │           ├── scout/                   # per-stage subdirs
│       │           │   ├── run.json             # RunKind::FitStage
│       │           │   ├── fit_state.toml
│       │           │   ├── final_params.toml
│       │           │   ├── mle_params.toml
│       │           │   ├── chain_evaluations.tsv
│       │           │   ├── diagnostics.json
│       │           │   └── chain_<n>/
│       │           │       ├── parameter_traces.tsv
│       │           │       └── final_params.toml
│       │           ├── refine/  ...
│       │           ├── validate/  ...
│       │           ├── pgas/  ...
│       │           └── pmmh/  ...
│       ├── synthetic/                           # synthetic-data fits (SBC etc.)
│       │   ├── data/                            # generated datasets (one per sim_seed)
│       │   │   └── ds_NN.tsv
│       │   └── ds_NN/
│       │       └── fit_<seed>/
│       │           └── <sweep_slug>/<stage>/...
│       ├── summary.tsv                          # grid-level summary (camdl batch's fit-mode)
│       ├── coverage.tsv                         # synthetic-mode SBC coverage (when applicable)
│       └── sweep_failures.tsv                   # cells that failed the gate (when applicable)
│
└── profiles/                                    # camdl profile
    └── <stem>-<profile_hash[:8]>/
        ├── run.json                             # RunKind::Profile
        └── points/<idx:05d>/
            ├── focal.toml                       # which (focal_param: value, ...) this point pins
            └── start_<k>/
                ├── run.json                     # RunKind::FitStage (per grid-point × start mini-fit)
                ├── fit_state.toml
                ├── ...
```

Key facts verified against code:

- **Fits are content-hash-keyed.** `FitConfigV2::fit_content_hash` (line 767)
  hashes `(model IR bytes, data files, fit.toml bytes)`. No silent
  overwrites: any edit to bounds / fixed / priors / data / stages
  produces a new hash → new directory. **This is the only thing the user
  can rely on for "didn't I already run this?"**
- **The 8-char hash prefix in the directory name** is the CAS key the
  user sees. The full 64-char hash lives in `run.json`.
- **Data hashes are normalized** (sorted by name, then content) so
  reordering streams produces the same hash.
- **The fit hash is seed-independent.** Different `--seed N` invocations
  share a fit dir but live in different `fit_<seed>` subdirs — that's
  what `per_fit_prefix` (config_v2.rs:805) computes.
- **Stage-level CAS** (`FitStage`) hashes on stage algorithm config + seed.
  Re-running with `--force` is required to invalidate.

### 1.3 What's actually written per fit

Per `cmd_fit_run_v2` in `fit/mod.rs:189` (verified line-by-line):

| file | what it carries | written by |
|---|---|---|
| `fits/<stem>-<hash>/run.json` | top-level `Run { kind: Fit }` with `FitMeta` (model_hash, fit_toml_path, fit_toml_hash, data_hashes, estimated, fixed, stages_declared, ic_free) | line 270 |
| `fits/<stem>-<hash>/<sub>/<stage>/run.json` | per-stage `Run { kind: FitStage }` with `FitStageMeta` (method, seed, n_chains, algorithm, best_loglik, best_chain, starts_from, parent_profile_hash) | line 1133 |
| `<stage>/fit_state.toml` | inter-stage handoff: best_loglik, best_chain, start_values, rw_sd, tail_chain_agreement, chain_clean_logliks, chain_clean_ses, ivp_params, resolved_gate, resolved_clean_eval | line 825 |
| `<stage>/mle_params.toml` | winner θ̂ + extensive `[provenance]` (input_hash, model_hash, data_hashes, backend, dt, loglik, n_particles, ess_at_mle, timestamp) | line 845 |
| `<stage>/final_params.toml` | clean-eval winner θ̂ + `[provenance]` (chain, loglik, se) — post-strip; `winning_candidate_label` was dropped in `20d48fe` ([clean-eval strip](2026-04-27-clean-eval-strip.md)) | runner.rs |
| `<stage>/chain_evaluations.tsv` | M-replicate clean-eval score table, one row per chain (post-strip; the prior 3-row-per-chain layout with a `candidate` column was dropped in `20d48fe`) | runner.rs |
| `<stage>/diagnostics.json` | structured warning list | line 875 |
| `<stage>/chain_<n>/parameter_traces.tsv` | per-iteration param means (one file per chain) | runner.rs |
| `<stage>/chain_<n>/final_params.toml` | per-chain winner θ̂ | runner.rs |
| `summary.tsv` | grid-level summary (one row per cell, when grid > 1) | grid_summary.rs:38 |
| `coverage.tsv` | SBC coverage per parameter (synthetic mode only) | grid_summary.rs:96 |
| `sweep_failures.tsv` | which cells failed the compound gate (when --sweep used) | fit/mod.rs:1166 |

### 1.4 What is NOT written

- `<stage>_summary.json` files were planned but never written by `cmd_fit_run_v2`.
  Documented elsewhere; reconciled in `24c41de` to remove stale doc claims.
- A `manifest.json` at the fit-dir level listing every cell × stage. Each
  stage has its own `run.json`; the parent `run.json` is `RunKind::Fit`
  with stage *names* (`stages_declared`) but no per-cell breakdown.

---

## 2. `camdl` subcommands today (verified)

Live dispatch from `crates/cli/src/main.rs:255-271`:

| command | dispatcher | scope | reads | writes |
|---|---|---|---|---|
| `camdl simulate [--cas]` | `run_simulate` | one forward sim | model + params | `sims/.../traj.tsv` + `run.json` (CAS mode) |
| `camdl batch run` | `batch::cmd_batch_run` | sweep of sims or fits over a grid | batch.toml | `sims/<...>/seed_<n>/` per cell + `manifest.json` |
| `camdl batch status` | `batch::cmd_batch_status` | progress of an ongoing batch | manifest.json | nothing |
| `camdl fit run` | `fit::cmd_fit_run_v2` | full inference pipeline | fit.toml v2 | full `fits/<stem>-<hash>/...` tree |
| `camdl fit status` | `fit::cmd_fit_status` | walks tree, lists completed stages with one-line per stage | tree | nothing |
| `camdl fit summary` | `fit::cmd_fit_summary` | per-stage interpretation block (compound gate, params, chains, provenance) | fit_state + final_params + mle_params | nothing (renders to stdout) |
| `camdl fit diff` | `fit::cmd_fit_diff` | side-by-side **config** diff (estimate↔fixed moves, bounds, priors, stage changes) | two fit.toml files | nothing |
| `camdl fit new` | `fit::cmd_fit_new` | scaffold a derived fit.toml | one fit.toml | new fit.toml |
| `camdl fit where` | `fit::cmd_fit_where` | print resolved output dir for a fit.toml | fit.toml | nothing |
| `camdl pfilter` | `pfilter::cmd_pfilter` | one-shot particle filter | model + params + data | optional save-filtering, save-prequential |
| `camdl if2` | `if2::cmd_if2` | standalone IF2 (rarely used; `fit run` is preferred) | model + params + data | per-chain TSVs |
| `camdl profile` | `profile::cmd_profile` | profile likelihood over focal axes × n_starts | model + params + data + grid | `profiles/<stem>-<hash>/...` tree |
| `camdl eval` | `eval::cmd_eval` | evaluate a time-dependent expression on a model | model | TSV to stdout |
| `camdl data split` | `data::cmd_data_split` | split a TSV into train + holdout | TSV | two TSVs |
| `camdl list` | `browse::cmd_list` | flat table of cached sims and fits | tree | nothing |
| `camdl show <hash>` | `browse::cmd_show` | one cached run's metadata | run.json | nothing |
| `camdl cat <hash>` | `browse::cmd_cat` | dump traj.tsv or one obs stream | run files | stdout |
| `camdl compare` | `compare::cmd_compare` | **multi-model** prequential comparison (Δelpd, CRPS, PIT) | prequential.json from N fits | table / md / json |
| `camdl compile` / `check` / `inspect` | passthrough to camdlc (OCaml) | model authoring | .camdl | IR JSON / diagnostics |

### 2.1 `camdl list` — what it actually does

Walks `<root>/sims/` (3-level: sim_hash / scenario / seed) and
`<root>/fits/` (one level: stem-hash). Filters by `--model`, `--scenario`,
`--since`. Default: `--limit 20` of each. Output is a two-section table
(fits then sims) or `--format json`.

For fits, each row shows: hash, model, fit.toml path, when, version,
wall time. **Crucially: no parameter values, no convergence status, no
loglik, no Â.** The user gets a list of hashes, not a comparison.

`--parent <profile_hash>`: enumerates a profile's grid-point children.
Useful but narrow.

There's no:
- `--converged` filter
- `--gate-failed` filter
- `--with-stage pgas` filter
- comparison view between two listed fits
- prune / delete subcommand

### 2.2 `camdl show` — what it actually does

Loads one `run.json` and pretty-prints its fields (path, model, scenario,
seed, hashes, created, version, argv, traj.tsv size). For fits: same
shape, plus `estimate`, `fixed`, `stages`, `fit.toml hash`, wall time.

No interpretation: no per-stage Â, no winner θ̂, no gate verdict. That
lives in `camdl fit summary`. `show` is the run-metadata view; `summary`
is the interpretation view. The split is reasonable but the user has to
know to call both.

### 2.3 `camdl fit summary` (today's state)

`fit_summary.rs:44`:

```rust
const MLE_STAGES: &[&str] = &["scout", "refine", "validate"];
```

- Hard-codes the stage names. PGAS / PMMH stages are silently skipped.
  User-named stages (e.g. `deep_scout`) are silently skipped.
- One fit_dir at a time. No cross-fit aggregation.
- Renders gate verdict, parameter table, per-chain clean-eval table,
  provenance cross-checks.
- `--format text|json|md|latex`, `--params-only`, `--strict`, `--stage`,
  `--no-color`.
- Schema version `1` for `--format json`.

**Gaps from a "describe one fit" perspective**:
1. Bayesian stages don't render anything (the design smell already
   filed).
2. Sweep cells within a fit aren't exposed — `summary` walks
   `<fit_dir>/<stage>/fit_state.toml` directly, ignoring the
   `<fit_dir>/real/fit_<seed>/<sweep_slug>/<stage>/...` actual layout.
   On a real v2 fit run, the path includes `real/fit_<seed>/` and
   possibly `<sweep_slug>/`; the current `summary` won't find anything.
   Verified by reading `fit_summary.rs:109-113`:

   ```rust
   let stage_dir = format!("{}/{}", dir, stage);
   if !Path::new(&stage_dir).join("fit_state.toml").exists() {
       continue;
   }
   ```

   It looks for `<fit_dir>/scout/fit_state.toml`, but the v2 path is
   `<fit_dir>/real/fit_<seed>/scout/fit_state.toml`. No fit produced
   by `cmd_fit_run_v2` puts `fit_state.toml` directly under the fit_dir.

#### How this happened (verified via git)

This is not a regression — it's a stale mental model that survived
because there was no integration test holding the two pieces honest:

| event | commit | date |
|---|---|---|
| v2 layout: wrap all fit outputs under `real/fit_<seed>/` | `5f1e704` | 2026-04-18 |
| `fit summary` shipped with `<fit_dir>/<stage>/fit_state.toml` walker | `4bb27af` | 2026-04-25 |

A 7-day gap. The summary command was written *after* the v2 layout
shipped, but its path-walker was modelled on the pre-v2 tree. There
was no end-to-end test that ran `cmd_fit_run_v2` and then invoked
`cmd_fit_summary` against the resulting directory; if there had been,
the file-not-found would have been caught the moment the test ran.
The unit tests in `fit_summary.rs` set up synthetic `<dir>/<stage>/fit_state.toml`
fixtures by hand — exercising the walker against its own assumed shape,
not the shape `cmd_fit_run_v2` produces.

**The internal docs reinforced the wrong shape** (this is the more
embarrassing finding):

- `docs/camdl-inference-spec.md:482-490` describes the stage tree as
  `scout/fit_state.toml`, `refine/fit_state.toml`, `validate/fit_state.toml`
  with no `real/fit_<seed>/` prefix. Same at lines 403, 680, 795, 1027.
- `docs/inference.md:768` shows `scout/    fit_state.toml      (stage,
  starts_from = random)` — also without the v2 wrapper.

Both were written before v2 and never updated. Anyone reading the
spec to write `fit summary` would have walked exactly the path
`fit_summary.rs` walks. The spec needs the same fix.

**Two findings, two issues:**

1. `cmd_fit_summary` walks a v1 path that no v2 fit produces. Table-
   stakes bug (silent "no MLE stages found" on every real fit dir).
2. No integration test runs `cmd_fit_run_v2` end-to-end and then
   invokes `cmd_fit_summary`. This is the process gap that let (1)
   ship. Until we add such a test, every future cross-tool feature
   risks the same drift.

Issue 1 should be filed and fixed immediately; issue 2 belongs in
the experiment-management proposal as a structural commitment.

### 2.4 `camdl fit diff` — config-only

Reads two fit.toml files. Shows: estimate↔fixed moves, bounds changes,
prior changes, stage adds/removes/setting-changes. Does **not** read the
fit *results* — there's no view of "fit A converged to R0=56.8, fit B
converged to R0=58.1, Δ=+1.3."

### 2.5 `camdl compare` — multi-model prequential, not config-iteration

Reads `prequential.json` artifacts from N fit dirs and renders a table
with Δelpd, paired SE, Δcrps, PIT 90% coverage, E_T (evidence in
likelihood-units). Refuses structurally-unfair comparisons (T_score
mismatch).

This is the *predictive* comparison surface (which model predicts better),
not the *interpretive* one (which fit's MLE looks like what). Different
audience, different schema. Both legitimate. The naming is fine.

---

## 3. The four axes of variation

A fit can vary along multiple orthogonal axes. Today's tools handle them
unevenly:

| axis | what changes | storage | tool today | gap |
|---|---|---|---|---|
| **stages within one fit** | scout vs scout+refine vs scout+pgas | `<fit_dir>/<sub>/fit_<seed>/<stage>/` | `fit summary` (broken on v2 paths; MLE-only) | walk all stages, render per method |
| **sweep cells** | `--sweep R0=10,20,30` Cartesian over `[fixed]` values | `<fit_dir>/<sub>/fit_<seed>/<sweep_slug>/<stage>/` | nothing | per-cell view + grid-overview matrix |
| **synthetic SBC replicates** | N datasets × M fit_seeds | `<fit_dir>/synthetic/ds_NN/fit_<seed>/...` | `summary.tsv` (auto-written), `coverage.tsv` (auto) | per-cell summary command; aggregate at the right axis |
| **fit.toml variations** | bounds, fixed↔estimate, priors, data, model itself | **separate fit_dirs** keyed by content hash | nothing — `list` shows them flat | this is the largest gap |

The first three are **within-fit-dir** axes. They live under one `fits/<stem>-<hash>/`
tree. They share a fit_content_hash by definition.

The fourth is **cross-fit-dir**: each variation is a new fit_content_hash,
new directory, new everything. The CAS handles this *correctly* — distinct
fits never collide. But there's no UX for browsing or comparing them.

---

## 3.5. The missing unifying ADT: result heterogeneity across methods

A theme that keeps surfacing as we extend the experiment-management
surface: every fit-method produces a *different shape of result*, and
nothing in the code expresses that heterogeneity as a sum type. This
section argues we need one — and sketches what it looks like.

### What `RunKind` already does

`run_meta.rs:51-71` typed the **input** side cleanly:

```rust
enum RunKind {
    Simulate(SimulateMeta),
    Fit(FitMeta),
    FitStage(FitStageMeta),
    Profile(ProfileMeta),
}
```

Every directory under `results/` self-describes via this enum (one of
the post-2026-04-19 unification's biggest wins). But `FitStageMeta`
has only `method: String` and `algorithm: serde_json::Value` — the
**stage's algorithm config**. The actual *outputs* — what θ̂, what
diagnostics, what convergence verdict — are absent from the typed
schema. They live in stage-specific files, parsed ad-hoc by
whichever consumer reaches in.

### What each method actually produces

Verified against current writers:

| method | scalar interpretation | per-chain artifacts | aggregate artifacts |
|---|---|---|---|
| **if2** | `best_loglik`, `best_chain`, `max_chain_agreement` (Â), gate verdict (compound: Â + decibans-spread), `ess_at_mle`, `ess_min` | `chain_<n>/parameter_traces.tsv`, `chain_<n>/final_params.toml` | `fit_state.toml`, `mle_params.toml`, `final_params.toml`, `chain_evaluations.tsv`, `diagnostics.json`, `chain_starts.tsv`, `diagnostics.tsv` |
| **pgas** | `n_samples`, `posterior_mean[θ]`, `posterior_q025[θ]`, `posterior_q975[θ]`, `ess_per_param`, `max_rhat`, per-param `acceptance_rate` | `chain_<n>/trace.tsv`, `chain_<n>/trajectory_<NNNNNN>.tsv` | `draws.tsv` (complete-M posterior), per-param acceptance summary |
| **pmmh** | `n_samples`, `posterior_mean[θ]`, `ess`, `max_rhat`, scalar `acceptance_rate`, `map_loglik` | `chain_<n>/trace.tsv` | (none beyond per-chain traces) |

These are not just different field sets — they're different *kinds*
of object. An IF2 result is a **point estimate plus convergence
diagnostics**. A PGAS / PMMH result is a **posterior approximation**.
Code that summarises or aggregates these can't squash them into one
struct without losing the distinction the user actually cares about:
"is this an MLE I can report, or a posterior I can sample from?"

**Pfilter is deliberately not in this enum.** The standalone
`camdl pfilter` is a likelihood evaluator on already-fixed
parameters; it produces no θ̂, runs as its own subcommand (not as a
fit stage), and writes to `results/sims/...` paths the fit-tree
walker doesn't visit. `FitStageMeta.method` values in the v2
pipeline are exactly `{"if2", "pgas", "pmmh"}`. Including a pfilter
variant would force every consumer of `MethodResult` (table rows,
`fit diff`, prune) to write a "this variant has no parameters,
render —" branch that the walker can never actually reach.
Smaller enum, fewer dead paths.

### Today: heterogeneity is implicit

There is no `MethodResult` type. Each consumer (`fit_summary`, future
`fit_table`, prequential's `compare`) parses whatever files it knows
about for whatever methods it knows about, and silently skips the
rest. `fit_summary`'s hard-coded `MLE_STAGES = ["scout", "refine",
"validate"]` (§2.3) is one face of this; another is that
`grid_summary::read_cell_row` knows how to parse `mle_params.toml`
but not `draws.tsv`.

This is exactly the kind of stringly-typed dispatch the project's
design-philosophy CLAUDE.md tells us to avoid ("Make illegal states
unrepresentable — use ADTs, not stringly-typed or flag-riddled
data"). A method named "if4" sneaking into `FitStageMeta.method`
would not produce a compile error today; it would silently fall
through every consumer.

### Proposed: `MethodResult` enum, mirroring `RunKind`

```rust
/// Loaded interpretation of a completed fit-stage. Mirrors the
/// `RunKind` pattern: each variant carries the typed payload its
/// method produces, so consumers pattern-match instead of
/// stringly-dispatching on `method: String`. Three variants — pfilter
/// is excluded by design (it's a CLI evaluator, never a fit-stage).
pub enum MethodResult {
    If2(If2StageResult),
    Pgas(PgasStageResult),
    Pmmh(PmmhStageResult),
}

/// Compound scout-convergence gate verdict, today's IF2 gate
/// (Â leg + decibans-spread leg with SE-aware floor; see `gating.rs`).
/// String projection used in `table_row.gate_verdict`:
///   Pass → "pass", FailA → "fail_a", FailDb → "fail_db",
///   FailBoth → "fail_both".  Bayesian rows render "n/a" because
///   the IF2 gate doesn't apply.
pub enum GateVerdict {
    Pass,
    FailA,        // Â leg failed (max chain disagreement above threshold)
    FailDb,       // decibans-spread leg failed (chain logliks too dispersed)
    FailBoth,
}

pub struct If2StageResult {
    pub best_loglik: f64,
    pub best_chain: usize,
    pub theta_hat: BTreeMap<String, f64>,    // winner θ̂ (clean-eval)
    pub max_chain_agreement: f64,             // Â
    pub gate_verdict: GateVerdict,
    pub ess_at_mle: Option<EssSummary>,       // ess_min / ess_mean / ess_min_step
    pub n_chains: usize,
    pub n_iter: usize,
    pub clean_eval: CleanEvalSummary,         // per-chain (loglik, se) post-strip
                                              // — no candidate label, see
                                              // docs/dev/notes/2026-04-27-clean-eval-strip.md
}

pub struct PgasStageResult {
    pub n_samples: usize,
    pub posterior_mean: BTreeMap<String, f64>,
    pub posterior_q025: BTreeMap<String, f64>,
    pub posterior_q975: BTreeMap<String, f64>,
    pub ess_per_param: BTreeMap<String, f64>,
    pub max_rhat: f64,
    pub acceptance_per_param: BTreeMap<String, f64>,
    pub n_chains: usize,
}

pub struct PmmhStageResult {
    pub n_samples: usize,
    pub posterior_mean: BTreeMap<String, f64>,
    pub ess: BTreeMap<String, f64>,
    pub max_rhat: f64,
    pub acceptance_rate: f64,
    pub map_loglik: f64,
    pub n_chains: usize,
}
```

The walker (§6.0 below) returns `Vec<StageNode>` where each
`StageNode` carries:

```rust
pub struct StageNode {
    pub stage_dir: PathBuf,
    pub run: Run,                     // RunKind::FitStage
    pub method: String,               // copy of run.kind.method, for filtering
    // Deliberately *not* present: a `fit_state_path` field. fit_state.toml
    // is an IF2 artifact; PGAS/PMMH don't write one. Consumers that need
    // the full result load it via `MethodResult::load_from(&stage_dir)`,
    // which dispatches by method.
}

impl MethodResult {
    pub fn load_from(stage_dir: &Path, method: &str) -> Result<Self> {
        match method {
            "if2"   => Ok(MethodResult::If2(If2StageResult::load(stage_dir)?)),
            "pgas"  => Ok(MethodResult::Pgas(PgasStageResult::load(stage_dir)?)),
            "pmmh"  => Ok(MethodResult::Pmmh(PmmhStageResult::load(stage_dir)?)),
            unknown => Err(format!("unknown fit-stage method `{}` in {}/run.json",
                                   unknown, stage_dir.display())),
        }
    }
}
```

This is the typed counterpart to `RunKind` for *outputs*. Every
consumer that produces a per-stage view — `fit summary`, `fit
table`, `fit diff` (results-aware), prequential `compare` — gets a
single typed entry point and pattern-matches the variant. New
methods get added by extending the enum, which produces compile
errors at every consumer that doesn't handle them yet. That's
exactly what we want.

### Why this matters for the experiment-management proposal

Without `MethodResult`:
- `fit table` ends up with method-specific columns interleaved
  (`if2.best_loglik`, `pgas.posterior_mean.R0`, `pmmh.acceptance_rate`)
  in a single Frankenstein row schema, OR it silently flattens to
  a "best loglik" column that means different things per method.
- `fit summary --format json` schema can't be stable if it has to
  accommodate "any method's outputs" via untyped `serde_json::Value`.
- Each consumer reinvents the file-discovery walk; the v2-layout bug
  in §2.3 is a direct consequence.

With `MethodResult`:
- `fit table` has a typed `Vec<StageNode>` and can render
  per-method column blocks (or pivot to method-uniform columns where
  meaningful: best_loglik / map_loglik are unifiable; `Â` and `Rhat`
  are not).
- `fit summary --format json` can ship per-method schemas with
  variant-tag (`{"method": "if2", ...}` / `{"method": "pgas", ...}`)
  and version each independently.
- Walker, summary, table, prune, and results-aware diff all share
  one entry point.

The proposal will commit to this structure. The audit's purpose here
is to surface the design need; the proposal will own the schema
versions and migration path.

---

## 4. Concrete user workflows that don't work well today

### 4.1 "I'm iterating on bounds; show me my last week's fits"

```
$ camdl list --kind=fit --since=1w
```

Returns a flat table with hashes and fit.toml paths. The user has to
remember which path corresponded to which experiment idea. There's no
way to see the *bounds difference* between fits at a glance — that's
buried inside each fit's stored fit.toml. There's no `--config-summary`
column or equivalent.

### 4.2 "Which of my recent fits converged?"

The information exists in `fit_state.toml.tail_chain_agreement` and the
gate verdict is computable from there. `camdl list` does not surface it.
The user must:

```bash
for d in results/fits/*/real/fit_*/refine; do
  camdl fit summary "$(dirname "$(dirname "$d")")"  # broken — see §2.3
done
```

…and even when `summary` is fixed (§2.3 path bug), this still spawns N
processes and the user re-aggregates by hand.

### 4.3 "Which two fits differ only in σ_se prior?"

`fit diff` only takes two fit.toml *paths*. To diff two completed fits:

```bash
camdl fit diff \
  results/fits/fit_he2010-04ab12cd/<embedded fit.toml? where?> \
  results/fits/fit_he2010-1f3c45ee/<...>
```

But the executed fit.toml isn't stored in a stable path inside the fit
dir. `FitMeta.fit_toml_path` (in `run.json`) is the *original* path on
the user's machine, which may have moved or changed. There's no
"recovered fit.toml" file that always lives at
`<fit_dir>/fit.toml.original` or similar. **Verified**: a grep of
`cmd_fit_run_v2` shows no fit.toml-archive write.

### 4.4 "Delete fits older than two weeks that didn't converge"

There is no `camdl prune` / `camdl rm` / `camdl gc` subcommand. The user
runs `find` + `rm -rf`. This is fine for power users but not what we
want for a tool intended to "save lives" via reliable epi modeling —
unintended deletion of a needed fit is a silent-disaster waiting to
happen. We should at minimum offer:

- a dry-run mode showing what would be deleted, with selection criteria
- a "marked stale" flag (don't actually delete, just hide from list)

### 4.5 "Show me one wide table: 30 fits × all the things I care about"

This is the `camdl fit table` command shape the user liked. Doesn't exist.

---

## 5. What CAS does well today (don't break)

- **Content-hash directory naming**: `fit_he2010-1f3c45ee` is human-recognizable
  (the stem) and uniquely identified (the hash prefix). Two fits with
  the same name and different content land in different directories.
- **Seed-independent fit hash**: `--seed 1` and `--seed 2` share a
  fit_dir but live in different `fit_<seed>` subdirs. No collision.
- **`run.json` per directory** (post-2026-04-19 unification): every
  hashed dir self-describes via a `Run { kind: ... }`. `camdl list`
  can walk and discover without an external index.
- **`Run.argv` round-trips**: `camdl show <hash>` prints the original
  command, including all flags. Reproducibility is "first-class."
- **Atomic run.json writes** (write-then-rename, `run_meta.rs:280`):
  partial writes don't corrupt the discovery layer.

Any new experiment-management command should *consume* this layer, not
parallel it.

---

## 6. Proposal sketch (not the full proposal — direction only)

The previous version of this section listed five orthogonal commands.
The user pushback was sharper: don't patch `summary`'s walker; **build
the walker once and have every consumer share it**. That changes the
sequencing fundamentally — the walker is the foundation, every
command rides on top.

### 6.0 The walker comes first: `fit_tree::walk_fit_dir`

A new module `crates/cli/src/fit/fit_tree.rs` exposes one canonical
function:

```rust
/// Walk a single fit directory (`results/fits/<stem>-<hash>/`) and
/// return one StageNode per completed fit-stage run found within.
/// Discovers stages by locating every `run.json` of `RunKind::FitStage`
/// under the dir — independent of any layout convention beyond that.
pub fn walk_fit_dir(fit_dir: &Path) -> io::Result<Vec<StageNode>>;

/// Walk the top-level `results/fits/` and return one entry per fit_dir
/// (no per-stage expansion). Use for `fit table`'s outer loop, then
/// call `walk_fit_dir` per row to load stage detail on demand.
pub fn walk_fits_root(root: &Path) -> io::Result<Vec<FitDirEntry>>;
```

`StageNode` is method-agnostic by construction:

```rust
pub struct StageNode {
    /// Absolute path to the stage directory.
    pub stage_dir: PathBuf,
    /// The stage's `run.json` (already parsed). Method, seed, n_chains,
    /// best_loglik, parent, and stage name all live here.
    pub run: Run,
    /// Convenience: the `<sub>/fit_<seed>/<sweep_slug>/` parent triple
    /// extracted from the path, for grouping in tables.
    pub axes: StageAxes,
}

pub struct StageAxes {
    pub data_kind: DataKind,         // Real | Synthetic { ds_idx: usize }
    pub fit_seed: u64,
    pub sweep_slug: Option<String>,  // None when no --sweep
}
```

Note what's **not** here: no `fit_state_path`. Bayesian stages don't
write `fit_state.toml`, and exposing it as a field on `StageNode`
would bake an IF2 assumption into the type. Consumers that need the
typed result call `MethodResult::load_from(&node.stage_dir, &node.run.method())`
(see §3.5) and pattern-match the variant.

This single function replaces three separate walkers in the current
codebase:
- `fit_summary.rs:109-113` (the buggy v1 walker)
- `grid_summary::iter_cells` (works on cells, doesn't classify by method)
- `browse::resolve_stage_by_hash` (single-dir hash lookup)

All three callers refactor to use `walk_fit_dir`. Once the walker
exists, fixing summary is a six-line change: replace the loop body
with `walk_fit_dir(&fit_dir)?.into_iter().filter(|n| n.run.method() == "if2")`.
The v1-layout bug closes as a side-effect, and the integration test
(see §6.7) pins it shut.

### 6.1 Fix `camdl fit summary` (now a thin consumer)

After the walker lands, `cmd_fit_summary` becomes:

```rust
let nodes = fit_tree::walk_fit_dir(&fit_dir)?;
for node in &nodes {
    match MethodResult::load_from(&node.stage_dir, &node.run.method())? {
        MethodResult::If2(r)  => render_if2_block(&fmt, &node, &r),
        MethodResult::Pgas(r) => render_pgas_block(&fmt, &node, &r),
        // ...
    }
}
```

This naturally extends to Bayesian stages (the design-smell that has
been deferred). The `--stage <name>` flag still narrows; the stage
list comes from the walker, not a hard-coded constant. The
`MLE_STAGES` constant gets deleted.

### 6.2 `camdl fit table <root>` — the cross-fit aggregator

Walks `results/fits/*/` via `walk_fits_root`, then per-fit calls
`walk_fit_dir` to find the terminal stage, loads `MethodResult`,
projects to one wide row per fit:

```
fit_id    label                   stem        config_diff_from_baseline   stages   method  converged   best_ll    R0     σ_se   Δll vs best   age
04ab12cd  narrow R0, take 1       fit_he2010  R0 ∈ [40,80] (was [1,100])  s+r      if2     ✓          -3804.9    56.8   0.115  0             3d
1f3c45ee  iota free               fit_he2010  + iota in [estimate]        s+r      if2     ✗          -3791.2    57.1   0.114  +13.7         5d
2a8b7901  prior on R0             fit_he2010  log_normal(R0)              s+r+v    if2     ✓          -3805.1    56.7   0.116  -0.2          1w
3c4d5e6f  pgas baseline           fit_he2010  → bayesian (added pgas)     pgas     pgas    ✓          —          56.9   0.116  —             4d
```

Filters: `--converged` / `--gate-failed` / `--with-stage <name>` /
`--with-method <if2|pgas|pmmh>` / `--model <hash>` /
`--since <duration>` / `--label-pattern <glob>`.
Output: `--format text|json|md|csv`. JSON is the contract for the
book pipeline + future dashboards.

#### `summary ⊆ table` invariant

`fit summary` and `fit table` share the per-fit row schema. To make
this enforceable rather than aspirational:

- `fit summary --format json` includes a top-level `table_row` block
  containing exactly the schema `fit table --format json` emits per
  row. Same field names, same types, same nullability, same units.
- A schema test asserts byte-equality:
  ```rust
  let summary_json = run_cmd("fit summary <fit_dir> --format json");
  let table_json   = run_cmd("fit table results/fits --format json --hash <h>");
  assert_json_eq!(summary_json["table_row"], table_json["rows"][0]);
  ```
  If a schema field is added to one without the other, the test fails.

The win: there's no "table view" vs "summary view" semantic drift.
A scientist who wants to know what columns `fit table` will surface
just reads `fit summary --format json` for one fit and looks at the
`table_row` block. The **table view is a horizontal stack of summary
rows**, by construction.

#### Schema for `table_row`

Versioned (`schema.version: 1`). All fields nullable for methods
that don't define them — the `MethodResult` variant determines
which are populated.

```jsonc
{
  "fit_id":        "04ab12cd",          // hash[:8]
  "fit_hash":      "04ab12cd...",       // full 64-char
  "label":         "narrow R0, take 1", // see §6.4
  "stem":          "fit_he2010",
  "model_hash":    "...",
  "stages":        ["scout", "refine"],
  "method":        "if2",               // terminal stage method ∈ {"if2","pgas","pmmh"}
  "config_diff_from_baseline": { ... }, // structured, see §6.3
  "converged":     true,
  "gate_verdict":  "pass",              // if2: pass|fail_a|fail_db|fail_both ; bayesian: "n/a"
  "best_loglik":   -3804.9,             // if2: best_loglik ; pmmh: map_loglik ; pgas: null
  "max_chain_agreement": 1.04,          // if2 only — Â (NOT Gelman-Rubin); null otherwise
  "max_rhat":      null,                // pgas/pmmh only — Gelman-Rubin R̂; null for if2
  "acceptance_rate": null,              // pmmh only (scalar); pgas reports per-param; null for if2
  "ess_at_mle":      { "min": 412, "mean": 850, "min_step": 17 },  // if2 only; null otherwise
  "ess_posterior":   null,              // pgas/pmmh only — posterior-chain ESS; null for if2
  "params":          { "R0": 56.8, "sigma_se": 0.115 },  // if2: θ̂ ; pmmh/pgas: posterior_mean
  "delta_ll_vs_best": 0.0,
  "age_seconds":   259200,
  "created_at":    "2026-04-24T18:30:21Z",
  "stale":         false,
  "stale_reason":  null
}
```

**Two ESS columns, on purpose.** IF2's `ess_at_mle` is the
particle-filter ESS evaluated at θ̂ — it's a likelihood-evaluation
diagnostic. PGAS/PMMH's `ess_posterior` is the effective sample
size of the posterior chain — a different quantity entirely
(autocorrelation in MCMC, not particle weight degeneracy). Naming
them the same column would silently conflate two diagnostics that
mean different things. The schema test from §6.2 enforces that no
future field rename merges them.

### 6.3 `config_diff_from_baseline`: structured, not a string

Earlier sketches showed `config_diff_from_baseline` as a free-form
string (`"R0 ∈ [40,80] (was [1,100])"`). That's fine for the text
view but useless for JSON consumers. The JSON shape is structured;
the text view is a deterministic projection.

```jsonc
"config_diff_from_baseline": {
  "baseline_hash":   "1f3c45ee",
  "model_changed":   false,                  // explicit: did the model IR hash differ?
  "estimate_added":  ["iota"],
  "estimate_removed": [],
  "fixed_added":     [],
  "fixed_removed":   ["iota"],
  "bounds_changed":  [
    { "param": "R0", "from": [1.0, 100.0], "to": [40.0, 80.0] }
  ],
  "priors_changed":  [
    { "param": "R0", "from": null, "to": "log_normal(log(50), 0.4)" }
  ],
  "data_hashes": {
    "added":    [],                          // streams present in this fit, not baseline
    "removed":  [],                          // streams in baseline, not this fit
    "modified": []                           // same name, different content hash
  },
  "stages_changed": {
    "added":   [],
    "removed": [],
    "settings_changed": []                   // [{ stage, key, from, to }]
  }
}
```

The text view renders this structure deterministically (e.g.
`+ iota in [estimate]; bounds R0 [1,100]→[40,80]`). When
`model_changed: true` the comparison crosses an expensive boundary
— the baseline isn't a meaningful "same fit, different config"
neighbour anymore — and the text view says so explicitly:
`(model changed; comparison limited)`. Tools can fall back to
"show only this fit's row" rather than guessing.

The detailed `data_hashes` triple matters because data swaps are
the most consequential change (a fit on rubella vs measles is *not*
the same experiment, even with identical fit.toml stage settings).
Today this would be one opaque "data_hashes differ" line; the
detailed triple lets the user see "ah, you swapped the streptococcal
serotype-B series, that's why R0 shifted."

### 6.4 Conditional-mandatory user labels

The CAS hash (`04ab12cd`) is unique but unreadable. Stems
(`fit_he2010`) are readable but collide. Users iterating across
30 variations can't distinguish them by either. **A short user-supplied
label closes the gap.**

The user pushed for this to be near-mandatory rather than purely
optional ("almost worth making mandatory, or no?"). The conclusion:
**conditionally mandatory** — the system nudges aggressively but
doesn't block one-off exploration.

#### Surface

- `camdl fit run --label "narrow R0, take 1" model.fit.toml`
  attaches the label at run-creation time. Label gets written to
  `<fit_dir>/run.json` under `FitMeta.label: Option<String>`.
- `camdl fit label <hash> "new label"` adds or updates a label
  post-hoc (re-writes `run.json` atomically; tracked in
  `argv_history`, see §6.5).

#### Validation rules

- Labels must be non-empty after trim. **`--label ""` is rejected at
  parse time** (clap-level validator, not a runtime check). Empty
  labels would defeat the indexing purpose and create silent UX
  ambiguity (does "" mean "no label" or "label is the empty string"?).
- Labels must match `^[a-zA-Z0-9 ,._-]{1,64}$` — letters, digits,
  spaces, commas, dot, underscore, hyphen; up to 64 chars after
  trim. **Spaces and commas are allowed** because labels are
  display strings, not filesystem paths or shell tokens; users
  will write `"narrow R0, take 1"` and `"take 1, attempt 2"`
  naturally — that's exactly how scientists write log entries.
- Two fits in `results/fits/` may share a label (it's an annotation,
  not a key); duplicate-label detection happens in `fit list/table`,
  with a warning, not a write-time error.

#### Atomicity for `fit label <hash>`

A subtle case: what if the user runs `fit label <hash> "x"` while
`cmd_fit_run_v2 <hash>` is still writing the same `run.json` (e.g.
mid-stage)? Two options were considered:

- (a) Error if the fit is still running. Detect via
  `Run.wall_time_seconds.is_none()` — the field is only populated
  after all stages finish. Simple, atomic, no lock files. Chosen.
- (b) Lock-file-based coordination. More general but adds a new
  lifecycle artifact. Rejected as unnecessary complexity.

Implementation: `fit label` reads `run.json`, inspects
`wall_time_seconds`. If `None`, exit code 2 with
`error: cannot label a fit that is still running (wall_time not yet recorded). Wait for the fit to finish, or pass --label at fit run time.`

**Concurrent `fit label` invocations on the same hash are
last-write-wins.** The project does not coordinate label edits
across processes — two terminals racing each other will produce
whatever the last `rename(2)` lands as the final label. This is
acceptable in practice because labels are single-user annotations;
two CI jobs labelling the same hash simultaneously would already
be a workflow smell. If we ever need stronger guarantees, a flock
on `run.json` would be the minimal extension.

#### Encouraging labels without enforcing them

Hard-mandatory labels would block CTF-style exploration. Soft-
optional labels would mean nobody uses them. Middle ground:

- `camdl fit list` emits an end-of-output warning when ≥ N unlabelled
  fits are present, suggesting `camdl fit label`. The threshold is
  configurable via `CAMDL_UNLABELED_THRESHOLD` env var (default 5),
  not a CLI flag — it's a per-user preference, not a per-invocation
  decision.
- `camdl fit table` shows `<unlabelled>` (dim) in the label column;
  fits with labels stand out visually.
- The book's vignettes ALL use `--label`, modelling the practice.

#### Workflow

```bash
$ camdl fit run --label "measles-bounds-tight" he2010.fit.toml
$ camdl fit run --label "measles-iota-free"    he2010.fit.toml
$ camdl fit list
fit_id    label                  stem        method  converged   age
04ab12cd  measles-bounds-tight   fit_he2010  if2     ✓           3d
1f3c45ee  measles-iota-free      fit_he2010  if2     ✗ Â         5d
2a8b7901  <unlabelled>           fit_he2010  if2     ✓           1w
warning: 1 unlabelled fit. Consider `camdl fit label <hash> <label>`.
         (set CAMDL_UNLABELED_THRESHOLD to control this warning)
```

### 6.5 `camdl fit prune` — trash before delete

Selection criteria:
- `--gate-failed --older-than 7d`
- `--orphan` — fits whose `fit_toml_path` no longer exists
- `--unlabelled --older-than 14d` — interactive, not automatic
- `--label-pattern <glob>` — explicit user-driven cleanup

Output flags:
- `--dry-run` (default!) — print what would be moved, exit.
- `--mark-stale` — flag `Run.stale: { reason: String, at: String }`
  in `run.json`; don't move the directory. `fit list` / `fit table`
  hide stale entries by default; `--show-stale` reveals them.
- `--force` — actually delete. **Even with `--force`, the directory
  is moved to a trash sibling first**, not unlinked.

#### Trash format

Pruned fit directories move to `results/.trash/<hash[:8]>-<ISO8601>/`:

```
results/.trash/
  04ab12cd-2026-04-27T18:30:21Z/
  1f3c45ee-2026-04-27T18:30:23Z/
```

The format is `<hash[:8]>-<ISO8601>`, **not** unix-timestamp-based.
Reasons:
- Human-sortable in `ls` — recent prunes group at the bottom
  alphabetically.
- ISO 8601 is already the project's canonical timestamp format
  (`cas::iso8601_utc`).
- Recovery is `mv results/.trash/<dir> results/fits/` — same name,
  no parsing needed beyond the prefix.
- `find results/.trash -mtime +30 -exec rm -rf {} \;` is one line
  for a real cleanup cron, separate from the prune UX.

#### `argv_history` as the per-fit audit log

`run.json` carries an `argv_history: Vec<HistoryEntry>` field
recording **every operation that mutated this fit_dir**, not just
`fit run` invocations. The same field captures `fit label`,
`fit prune --mark-stale`, and resurrections (re-running a
stale-flagged hash). This makes `run.json` a self-contained audit
log: one read tells you the full lifecycle of the fit.

```jsonc
"argv_history": [
  { "argv": ["camdl", "fit", "run", "--label", "narrow R0, take 1", "he2010.fit.toml"],
    "at":   "2026-04-22T12:00:00Z" },
  { "argv": ["camdl", "fit", "prune", "--mark-stale", "04ab12cd"],
    "at":   "2026-04-25T08:00:00Z",
    "reason": "iota Â=1.4 after 1000 iters" },
  { "argv": ["camdl", "fit", "run", "he2010.fit.toml"],
    "at":   "2026-04-27T09:15:00Z",
    "context": "resurrection",
    "cleared_stale_reason": "iota Â=1.4 after 1000 iters" }
]
```

Three things this shape gets right that the earlier sketch didn't:

1. **`--mark-stale` writes its own entry with `reason`.** The
   stale flag is an event in the fit's history, not a state
   stored only on `Run.stale`. (The state field is also kept for
   query convenience — `fit list` doesn't want to scan history
   to know whether a fit is currently flagged.)
2. **Resurrection is the next `fit run`, not a `fit label`.** The
   resurrection entry references the prior stale reason via
   `cleared_stale_reason` rather than re-quoting it as
   user-supplied context. Forensic readers see "resurrected on
   2026-04-27, the reason that had been recorded was X" — which
   is the actual diagnostic question.
3. **`fit label` events appear in the same log.** Future
   reviewers (or Claude in a later session) can reconstruct
   "when did this fit get its current label" from the same
   history without consulting an external system.

CAS-friendly: prune always operates on **whole fit_dirs** (the
content-hash unit). Never partial.

### 6.6 Stretch (deferred): results-aware `camdl fit diff <hash_a> <hash_b>`

> **Note:** the proposal that grew out of this audit
> ([`docs/dev/proposals/2026-04-28-fit-experiment-management.md`](../proposals/2026-04-28-fit-experiment-management.md))
> **defers this command entirely** to a future iteration. The
> ADT + walker foundation it builds on still ships; the command
> itself does not. Today's config-only `cmd_fit_diff` is left
> untouched. See the proposal's "Future work" section for the
> rationale (`fit table` may subsume most pair-wise workflows;
> we'd rather observe what users actually run before locking in
> the shape). The sketch below is preserved as motivation for the
> ADT design, not as a commitment.

Today's `fit diff` takes two fit.toml paths. A future extension
would take two fit hashes (or paths to fit dirs) and diff both
*config* and *results*. With `walk_fit_dir` + `MethodResult` in
place, the command becomes a rendering layer:

```
$ camdl fit diff 04ab12cd 1f3c45ee
config:
  iota: [fixed] = 0.001 → [estimate]
results (terminal stage = refine; method = if2 vs if2):
  R0:        56.8 → 57.1   (Δ +0.3)
  σ_se:      0.115 → 0.114 (Δ -0.001)
  best_ll:   -3804.9 → -3791.2 (Δ +13.7)
  Â (max):   1.04 (R0) → 1.18 (iota)  ✗ regressed
gate:
  fit A:  ✓ pass
  fit B:  ✗ Â leg failed (iota Â=1.18)
```

If the two fits use different methods (`if2` vs `pgas`), diff
declines to compare results scalar-for-scalar:
`results: methods differ (if2 vs pgas) — reporting per-fit blocks separately.`
Pattern-match on `(MethodResult, MethodResult)`; only same-variant
pairs render aligned columns. This is the natural pair-wise companion
to `fit table`'s single-row view.

### 6.7 Integration tests + spec/code parity check

The §2.3 bug shipped for two intertwined reasons: (a) `fit_summary`
unit tests fixturized their own assumed shape and never ran against
real `cmd_fit_run_v2` output, and (b) `docs/camdl-inference-spec.md`
described the pre-v2 layout, reinforcing the wrong mental model
for anyone reading the spec to write the walker. Closing this bug
class requires a defence on both axes.

#### Deliverable A: end-to-end integration test

```rust
#[test]
fn fit_summary_walks_real_fit_run_v2_output() {
    // Run a real fit (small fixture model, 2 chains, 5 iters) end-to-end.
    let fit_dir = exec_fit_run_v2(/* ... */);

    // Assert the resulting tree shape is what cmd_fit_summary walks.
    let nodes = fit_tree::walk_fit_dir(&fit_dir).unwrap();
    assert!(!nodes.is_empty(), "walker found no stages in {}", fit_dir.display());

    // Assert summary renders something for every if2 stage.
    let json = exec_fit_summary_json(&fit_dir);
    assert!(!json["stages"].as_array().unwrap().is_empty());
}
```

This single test prevents any future "summary command shipped
against a layout the runner doesn't produce" silent failure. It's
the cheapest insurance against the most expensive class of bug
(silent wrong answers on the inference output the user trusts).

#### Deliverable B: spec/code parity check

A second test parses the layout diagrams in
`docs/camdl-inference-spec.md` and asserts every documented path
exists on a fresh `cmd_fit_run_v2` output:

```rust
#[test]
fn spec_layout_diagrams_match_fit_run_v2_output() {
    let fit_dir = exec_fit_run_v2(/* ... */);
    let documented_paths = parse_layout_diagrams(
        "docs/camdl-inference-spec.md"
    );
    for relpath in &documented_paths {
        let abspath = fit_dir.join(relpath);
        assert!(abspath.exists(),
            "spec documents `{}` but it is not produced by cmd_fit_run_v2",
            relpath.display());
    }
}
```

`parse_layout_diagrams` is a small ad-hoc parser over the fenced
ASCII trees the spec uses. The forcing function is what matters:
once this test exists, **the spec cannot drift from the code
without breaking CI**. Anyone editing the spec sees the test
fail; anyone refactoring the layout sees the test fail. The
mental-model drift that produced the §2.3 bug becomes
mechanically detectable.

The same harness can be extended to `docs/inference.md` and to
the `Appendix A` paths in this audit's earlier draft. The cost
of writing it is small; the cost of *not* having it is exactly
the bug we're fixing.

---

## 7. What's already partially built

Don't reinvent these:

- **`grid_summary::write_summary`** (was `summary.rs`, renamed in
  `4d42fc5`) writes a per-fit-dir `summary.tsv` automatically when there
  are >1 cells. It's an internal helper; could be the body of the
  `fit table` command, scaled up.
- **`grid_summary::read_cell_row`** parses `mle_params.toml` headers
  back into `(loglik, params, content_hash)` rows. Half of `fit table`'s
  work is already a function call away.
- **`browse::resolve_stage_by_hash`** (browse.rs:485) walks the tree
  finding a stage whose hash starts with a given prefix. Useful for
  hash-prefix UX in `prune` / `diff`.

---

## 8. Doc / reality drift uncovered during this audit

- The 2026-04-19 unified-output-tree proposal references `output/` as
  the root; code uses `results/` (changed in hardening ship-now #7).
  Some doc strings in `cas.rs` still say `output/`. Worth a sweep.
- `<stage>_summary.json` was documented but never written by v2; cleaned
  up in `24c41de`.
- `camdl fit summary` is documented as accepting `<fit_dir>` but its
  path-walking is broken for the v2 layout (§2.3). Recommend: fix the
  code AND the doc references that misled the original author.
- **`docs/camdl-inference-spec.md` describes the pre-v2 stage layout**
  in multiple places: lines 482–490 walk through `scout/fit_state.toml`
  → `refine/fit_state.toml` → `validate/fit_state.toml` with no
  `real/fit_<seed>/` wrapper. Same shape repeats at lines 403, 680,
  795, 1027. This is the spec that anyone implementing a tree-walker
  in 2026-04 would have read first; it's literally the v1 layout. The
  spec needs an atomic update with the v2 layout AND a one-paragraph
  callout saying "stages live under `real/fit_<seed>/[<sweep_slug>/]`
  in v2 layout (commit `5f1e704`, 2026-04-18)."
- **`docs/inference.md:768`** shows the same v1 path. Same fix.
- `prepare_v1_cell` etc. v1 helpers carried "Retained in case a fit-bridge
  path re-uses it" comments through 7+ days; deleted in `f797985`.

The spec drift in this last batch is the most consequential: it
reinforced a wrong mental model that produced a real silent-failure
bug. The proposal's §6.7 builds two defences against this class:
the end-to-end integration test (Deliverable A) AND the
mechanical spec/code parity check that parses the layout diagrams
in `camdl-inference-spec.md` and asserts every documented path
exists on a fresh fit dir (Deliverable B). Both are committed
artifacts in the proposal.

---

## 9. What this audit doesn't try to settle

- **The exact schema fields of `MethodResult` variants**. §3.5
  sketches the shape; final field lists belong in the proposal,
  with version 1 baseline and migration notes. The structural claim
  — that the enum exists and consumers pattern-match it — is what
  the audit commits to.
- **The exact `fit prune` selection grammar**. Needs care to avoid
  "git rm -rf the wrong thing" UX. Default to `--dry-run`, trash
  even on `--force`.
- **Whether to write a stable `<fit_dir>/fit.toml.original`** for
  diff-against-results to be reliable. Recommend yes; one-line addition
  to `cmd_fit_run_v2`. Listed in §10 step 7.
- **Method-aware `fit summary`** — was its own design-smell thread;
  this audit absorbs it (§6.1) since walker + `MethodResult` is the
  natural way to dispatch per method.
- **How labels round-trip across `fit derive`** — when the user
  derives a new fit.toml from an existing one, does the label carry?
  Default: no (a new variation deserves a fresh label). Override with
  `--inherit-label`. Settled in proposal.

## 10. Recommended next steps

Sequence (revised — walker-first, proposal-first):

0. **File issue #1 immediately**: `cmd_fit_summary` walks v1 layout;
   no v2 fit produces matching paths (§2.3). One-paragraph repro
   pointing at `fit_summary.rs:109-113` and `5f1e704`. This is the
   table-stakes silent-failure that justifies fixing summary at all.

1. **Write the experiment-management proposal** —
   `docs/dev/proposals/2026-04-28-fit-experiment-management.md`.
   Owns: walker (§6.0), `MethodResult` ADT (§3.5), `fit table`
   (§6.2), structured `config_diff` (§6.3), labels (§6.4), `fit
   prune` w/ trash (§6.5), results-aware `fit diff` (§6.6),
   integration-test commitment (§6.7), versioned JSON schema for
   each. Issue #2 ("no integration test for summary against v2
   layout") is captured as a section here, not a separate ticket —
   it's a structural commitment, not a one-off bug.

2. **Implement walker + `MethodResult` first** —
   `crates/cli/src/fit/fit_tree.rs` + extending `MethodResult` in
   `run_meta.rs` (or a sibling module). No user-facing change yet;
   pure foundation. Lands with unit tests against fixture trees AND
   the integration test (§6.7) that exercises `cmd_fit_run_v2 →
   walk_fit_dir`.

3. **Refactor `fit summary` to consume the walker** (§6.1). Closes
   issue #1 as a side effect of (2). The integration test from (2)
   pins this shut.

4. **Ship `fit table`** (§6.2) — highest-value single addition.
   `summary ⊆ table` byte-equality test in CI.

5. **Ship `fit prune`** (§6.5) — dry-run default, trash-before-delete,
   ISO 8601 trash naming.

6. **Ship results-aware `fit diff`** (§6.6) — small ergonomic win,
   reuses (2)–(4).

7. **Write `<fit_dir>/fit.toml.original`** — one-line addition to
   `cmd_fit_run_v2` that archives the fit.toml at the time of the
   run. Makes `fit diff` reliable when source files have moved or
   changed.

8. **Sweep stale spec references** — update
   `docs/camdl-inference-spec.md:482-490` and `docs/inference.md:768`
   to reflect the v2 layout (`real/fit_<seed>/<stage>/...`).

After all of this lands, `camdl list / show / cat` stays as the low-
level walk; `fit summary` is single-fit interpretation; `fit table`
is the cross-fit aggregator; `fit prune` is the safe-cleanup tool;
`fit diff` is the pair-wise interpretation diff. **All five
consume one walker and one `MethodResult` enum** — the unifying ADT
the project has been missing.

---

## Appendix A: Quick reference — paths the user might want

```text
# A specific fit dir
results/fits/<stem>-<hash[:8]>/

# That fit's run.json (top-level)
results/fits/<stem>-<hash[:8]>/run.json

# A specific stage's fit_state (real-data, no sweep)
results/fits/<stem>-<hash[:8]>/real/fit_<seed>/<stage>/fit_state.toml

# A specific stage's fit_state (synthetic, with sweep)
results/fits/<stem>-<hash[:8]>/synthetic/ds_<NN>/fit_<seed>/<sweep_slug>/<stage>/fit_state.toml

# Per-stage MLE
results/fits/<stem>-<hash[:8]>/real/fit_<seed>/<stage>/mle_params.toml

# Clean-eval winner (canonical, loadable via `pfilter --params`)
results/fits/<stem>-<hash[:8]>/real/fit_<seed>/<stage>/final_params.toml

# Sim run cached by --cas
results/sims/<stem>-<sim_hash[:8]>/<scenario>-<scen_hash[:8]>/seed_<n>/

# Profile run
results/profiles/<stem>-<profile_hash[:8]>/points/<NNNNN>/start_<k>/
```

## Appendix B: `Run` schema cheat-sheet

`crates/cli/src/run_meta.rs:18-71`:

```rust
struct Run {
    hash: String,           // 64 hex
    version: String,        // camdl version
    created_at: String,     // ISO 8601 UTC
    argv: Vec<String>,
    wall_time_seconds: f64,
    kind: RunKind,
}

enum RunKind {
    Simulate(SimulateMeta),
    Fit(FitMeta),
    FitStage(FitStageMeta),
    Profile(ProfileMeta),
}
```

`FitMeta` carries `model_hash`, `fit_toml_path`, `fit_toml_hash`,
`data_hashes`, `estimated`, `fixed`, `stages_declared`, `ic_free`. The
fields are enough to compute `fit table`'s "config_diff" column without
re-reading fit.toml — every load-bearing input is hashed and named.
