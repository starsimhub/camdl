# camdl types reference & flow guide

Working memo. Covers the type model across `ir`, `sim`, and `cli`,
and — more importantly — *how data flows through the boxes* from a
user's CLI invocation to disk artefacts. This is the way Vince
thinks about code: the type catalogue answers "what exists," the
flow narrative answers "what moves," and most code smells live at
the seams where one shape crosses into another.

State of the world: `feat/v1-fit-cleanup` and the four post-cleanup
refactors are merged. `Run.label` lives at the top level. `camdl
label` is top-level. Profile is always a `RunKind::ReplicateSet`
umbrella (N=1 trivially). Show coverage is uniform via a single
`ResolvedRun + match on kind` dispatch. `--resume` is wired for
PGAS/PMMH with an identity-vs-extension hash split. `FitConfigV2`
is the only fit-config schema.

---

## Table of contents

1. [Layered architecture, at a glance](#1-layered-architecture)
2. [Crate-by-crate type catalogue](#2-crate-by-crate-type-catalogue)
3. [The CAS abstraction — `CasInputs` + `Run`](#3-the-cas-abstraction)
4. [Flow narrative: per-command type pipelines](#4-flow-narrative)
5. [Cross-crate seams and conversions](#5-cross-crate-seams)
6. [Type-flow smells flagged on this pass](#6-type-flow-smells)

---

## 1. Layered architecture

The codebase is three concentric layers connected by two narrow
seams:

```
                  ┌──────────────────────────────────────┐
USER input ─────▶ │  cli   (argv → typed args → orchestra)│
                  │   args/, fit/, profile.rs, batch.rs  │
                  │   browse.rs, main.rs, util.rs        │
                  └──────┬─────────────────────────────┬─┘
                         │ (CLI→sim seam)              │ (CLI→ir seam)
                         ▼                             ▼
                  ┌─────────────────┐         ┌──────────────────┐
                  │  sim            │ ◀──── │  ir              │
                  │  inference/, … │ uses    │  Model + IR types│
                  └─────────────────┘         └──────────────────┘

                  ╠═══ FILE-BACKED ARTEFACTS ═══╣
                  run.json, traj.tsv, profile.tsv, summary.tsv,
                  fit_state.toml, mle_params.toml, draws.tsv, …
```

**`ir`** is the structural truth: a `Model` is a flat, fully-expanded
declarative description of compartments, transitions, observations,
interventions, parameters. No randomness, no simulation logic.

**`sim`** consumes a `Model`, compiles it into `CompiledModel`, and
executes — Gillespie / tau-leap / chain-binomial / ODE, plus
inference (IF2, PGAS, PMMH, particle filter). All randomness lives
here; all compute time lives here.

**`cli`** orchestrates: parses argv, loads files, drives `sim`,
emits artefacts. Owns three pieces that don't fit anywhere else:
the **CAS abstraction** (content-addressable on-disk layout), the
**fit.toml v2 schema** (a higher-level user-facing config that
compiles to `sim` types), and the **browse pipeline** (`list` /
`show` / `cat` / `label`). Everything in `cli/src/` is glue between
the user and the sim.

The two seams are where most design risk concentrates:

- **CLI → sim**: e.g., `fit::config_v2::PriorSpec` → `sim::Prior`,
  `FitConfigV2` → `FitRunConfig` → `IF2Config`. Each conversion is a
  place where the user's mental model meets the algorithm's needs.
- **CLI → ir**: e.g., `Model` reads (load + scenario filter +
  parameter overlay), `ir::ObservationModel` ↔ `ObsStream`. Mostly
  one-way: the IR is treated as immutable input.

---

## 2. Crate-by-crate type catalogue

This section is dense by design — when refactoring, you want one
place that names every type so you can see what's near what.

### 2.1 `ir` crate

The IR is what the OCaml compiler emits and what `sim` consumes. It
has no methods that do work — every type is either pure data or a
`validate()` helper.

| File | Type | Role |
|---|---|---|
| `model.rs` | **`Model`** | Top-level. Owns parameters, structure (compartments, transitions, observations, interventions), initial conditions, simulation config, presets. The single public re-export from `ir`. |
| `model.rs` | `Compartment`, `CompartmentKind` | Compartment + Integer/Real flag. |
| `model.rs` | `InitialConditions` | `Explicit | Parameterized | FromDistribution` |
| `model.rs` | `Preset` | A scenario: name + `enable`/`disable`/`scale`/param-overrides. |
| `model.rs` | `OutputSchedule`, `RegularOutputSchedule` | Trajectory output time grid. |
| `model.rs` | `SimulationConfig` | t_start / t_end / dt / seed / time_semantics. |
| `model.rs` | `BalanceSpec`, `Dimension`, `ModelStructure` | Structural sub-types. |
| `parameter.rs` | **`Parameter`** | Name, bounds, prior, transform, kind, value. |
| `parameter.rs` | `PriorDist` | 8-variant enum: Uniform, Normal, LogNormal, HalfNormal, Beta, Gamma, Exponential + their fields. |
| `parameter.rs` | `HierarchicalPrior` | Expression-based prior referencing other parameters. |
| `parameter.rs` | `Transform` | `Log | Logit | Identity` (IR's flavor). |
| `transition.rs` | **`Transition`** | Reactants, products, rate (`Expr`), stoichiometry. |
| `observation.rs` | **`ObservationModel`** | Projection + schedule + likelihood. |
| `observation.rs` | `Projection` | Compartment / incidence / expression. |
| `observation.rs` | `Likelihood` | 6-variant: Poisson, NegBinomial, Normal, Binomial, BetaBinomial, Bernoulli. |
| `observation.rs` | `ObservationSchedule`, `RegularSchedule` | Time grid for observations. |
| `intervention.rs` | **`Intervention`** | Schedule + actions (transfers, adds, sets). |
| `expr.rs` | **`Expr`** | Total-first-order AST: `Const | Param | Pop | PopSum | Time | BinOp | UnOp | Cond | TimeFunc | TableLookup`. No recursion. |
| `table.rs` | `Table`, `TableSource`, `OobPolicy` | Compact lookup tables (contact matrices, age-specific rates). |
| `time_func.rs` | `TimeFunction` | Named time-only functions (sinusoidal, piecewise, interpolated, periodic). |
| `ode_equation.rs` | `OdeEquation` | RHS expression for a continuous compartment. |
| `validate.rs` | `ValidationError` | Returned by `Model::validate()` on contract violations. |

### 2.2 `sim` crate

#### Core

| File | Type | Role |
|---|---|---|
| `lib.rs` | **`Capabilities`** (bitflags) | `OVERDISPERSION`, `REAL_COMPARTMENTS`. Backends declare what they support; models declare what they need; mismatch errors at dispatch. |
| `compiled_model.rs` | **`CompiledModel`** | Pre-compiled `ir::Model`: flattened param index, time-funcs, tables, transitions, default params. The unit of work for every backend. |
| `state.rs` | `Trajectory`, `Snapshot`, `IntState`, `RealState`, `FlowVec` | The output of one simulation. |
| `lib.rs` | `Simulate` (trait) | The backend interface: `simulate(model, config, seed) -> Trajectory`. |
| Multiple | `GillespieSim`, `TauLeapSim`, `ChainBinomialSim`, `OdeSim` | The four backends. |
| `config.rs` | `SimConfig`, `GillespieConfig`, `TauLeapConfig`, `ChainBinomialConfig`, `OdeConfig` | Backend hyperparams. |
| `error.rs` | `SimError` | Backend errors. |

#### Inference (`sim/src/inference/`)

The inference module is the largest single subsystem in the workspace.

| File | Type | Role |
|---|---|---|
| `types.rs` | **`EstimatedParam`** | The narrow CLI→sim contract for "this parameter is being inferred": name, index in param vector, initial, rw_sd, transform, lower, upper, ivp, rw_sd_auto. |
| `types.rs` | **`Transform`** | `None | Log{lo,hi} | Logit{lo,hi}`. The bound-aware version, distinct from `ir::parameter::Transform` (which is unbounded). |
| `types.rs` | `ParticleState`, `ParticleSwarm` | Particle filter substrate. |
| `prior.rs` | **`Prior`** | 9-variant runtime prior: `Flat`, `Uniform`, `Normal`, `TransformedNormal`, `HalfNormal`, `Beta`, `Gamma`, `Exponential`, `Hierarchical(ir::HierarchicalPrior)`. Carries `Prior::from_ir(ir::PriorDist)`. |
| `prior.rs` | `Scale` | `Natural | Transformed` — log-density evaluation contract. |
| `traits.rs` | `ProcessModel`, `DensityProcess`, `ObservationModel<S>`, `Resettable` | The four inference traits. Bootstrap PF needs `ProcessModel + ObservationModel`; PGAS/PMMH need `DensityProcess`. |
| `traits.rs` | `SMCConfig` | `n_particles`, `resampling_threshold`, `skip_first_obs_from_loglik` (ic-free). |
| `chain_binomial_process.rs` | `ChainBinomialProcess` | The discrete-time process driver wired into PF/IF2/PGAS. |
| `multi_stream_obs.rs` | **`MultiStreamObsModel`**, `StreamSpec`, `StreamProjection` | Multi-likelihood evaluator (one PF, many observation streams). |
| `if2.rs` | **`IF2Config`**, `IF2Result`, `IF2IterResult`, `ParamIterDiag`, `Observation`, `SimplexGroup` | Iterated filtering (Ionides 2015). |
| `pgas.rs` | **`PGASConfig`**, `PGASResult`, `PGASSweep`, `PGASTrajectory`, `SubstepRecord`, `IVPMapping`, `CSMCDiagnostics`, `LogLikComponents`, **`ChainResumeState`** | Particle Gibbs with Ancestor Sampling — production Bayesian. |
| `pmmh.rs` | **`PMMHConfig`**, `PMMHResult`, `PMMHStep`, `PMMHResumeState`, `AdaptiveProposal` | Particle Marginal MH (experimental). |
| `nuts.rs` | `NUTSConfig`, `MassMatrix`, `NUTSStepResult`, `DualAveraging` | Used by PGAS for the θ\|X update. |
| `particle_filter.rs` | `Observation`, `PFilterResult`, `PredictionDiag`, `PrequentialRecorded` | Bootstrap PF. |
| `prequential.rs` | `PrequentialStep`, `PrequentialTrace`, `PrequentialWarning`, `Provenance` | Prequential model-comparison support. |
| `dmeasure.rs`, `obs_loglik.rs` | Helpers | Observation log-PMFs + analytical gradients (digamma etc.). |
| `pgas_grad.rs` | `Grad` types | Gradient evaluation for PGAS (uses compiler-emitted `rate_grad`). |
| `hierarchical.rs` | `ParamEnv` (trait), `NamedParams<'a>` | Plugged into `Prior::Hierarchical` for evaluating expression-based priors. |
| `diagnostic.rs` | `DiagnosticCollector`, `Diagnostic`, `DiagnosticKind`, `Severity` | Cross-stage diagnostic log; rendered to stderr + persisted to `diagnostics.json`. |
| `ancestor_trace.rs` | `AncestorTrace`, `SampledPath` | PGAS trajectory bookkeeping. |
| `correlated_pf.rs` | `PFRandomState` | Experimental correlated PF (out-of-scope per file comment). |

### 2.3 `cli` crate

The largest crate by surface area (~129 public types, ~13k LOC). I'm
listing the load-bearing ones; the rest are local glue.

#### 2.3.1 Run metadata (`cli/src/run_meta.rs`)

The universal envelope every `run.json` deserializes into.

| Type | Role |
|---|---|
| **`Run`** | Universal envelope: `hash, version, created_at, argv, wall_time_seconds, label, kind`. |
| **`RunKind`** | Tagged union of 5 payloads (`#[serde(tag="kind")]`): `Simulate, Fit, FitStage, Profile, ReplicateSet`. |
| `SimulateMeta` | model, model_hash, scenario, sim_hash, scen_hash, seed, backend, dt, sweep_point, from_fit_hash. |
| `FitMeta` | model_hash, fit_toml_path/hash, data_hashes, estimated, fixed, stages_declared, ic_free. |
| `FitStageMeta` | fit_hash, stage, method, seed, n_chains, algorithm (json), best_loglik/chain, starts_from, derived_from, parent_profile_hash + indices. |
| `ProfileMeta` | model, focal_params, grid (`Vec<GridAxis>`), n_starts, if2_config_hash, base_params_hash, seed_base, total_jobs. |
| `GridAxis` | One profile axis: param + values list. |
| `StartsFromRef` | `(stage, stage_hash)` for stage lineage. |
| `CacheStatus` | `Hit | StaleHash | Missing` — return type of `Run::check_cache`. |

`Run::write` is atomic (tmp-then-rename). `Run::read` parses
run.json. `Run::check_cache(dir, expected_hash)` checks the stored
hash against an expected value (used by simulate's cache-hit short-
circuit).

#### 2.3.2 CAS abstraction (`cli/src/cas/`)

| File | Type | Role |
|---|---|---|
| `typed.rs` | **`CasInputs`** (trait) | Every CAS-emitting command implements this for its single-realization input set. |
| `typed.rs` | **`ContentHash`** | 64-char hex newtype. `from_bytes`, `from_hex`, `full()`, `short()` (8 chars for path prefix). |
| `typed.rs` | `hash_canonical(&[(field, value)])` | Helper: sorted-key sha256. |
| `typed.rs` | `compose_with_replicate(inner, dim, key)` | Helper: composes a child hash from `inner_hash + dim_name + key`. |
| `typed.rs` | **`ReplicateSet`** | Layout helper for an umbrella over N children. Holds `inner_hash, dim_name, keys, child_kind`. Computes parent_hash, child_dir paths. |
| `typed.rs` | `ReplicateSetMeta` | The `RunKind::ReplicateSet` payload (serialized form). |
| `sim_inputs.rs` | **`SimulateInputs`** | One simulate run's typed inputs. Implements `CasInputs`. |
| `fit_inputs.rs` | **`FitInputs`**, **`StageInputs`** | The fit umbrella + per-stage leaves. Each implements `CasInputs`. |
| `mod.rs` | `iso8601_utc(t)`, `RunBuffer` | Misc helpers. |

`ProfileInputs` (defined in `profile.rs`) is the fourth `CasInputs`
impl. Living next to the command it serves rather than under `cas/`
is intentional — its `if2_config_hash` and `base_params_hash` are
profile-specific.

#### 2.3.3 Fit config schema (`cli/src/fit/config_v2.rs`)

The user-facing fit.toml schema. Everything here is on the *user*
side of the seam — algorithm-agnostic, declarative, validated.

| Type | Role |
|---|---|
| **`FitConfigV2`** | Top of the fit.toml schema. Holds `model, data, scenario/enable/disable, ic_free, config (backend+dt), estimate (IndexMap), fixed, stages (IndexMap)`. |
| `EstimateSpecV2` | Per-parameter inference spec: `bounds, transform, prior, start, ivp, rw_sd`. |
| **`PriorSpec`** | 7-variant enum (tagged on `dist` field): `LogNormal, Normal, Beta, Uniform, HalfNormal, Gamma, Exponential`. |
| **`Transform`** | CLI's flavor: `Log | Logit | Identity`. |
| `FixedParams` | Either inline values or a TOML file path (resolved at runtime). |
| `DataSpec` | `observations: IndexMap<stream_name, file_path>`. |
| `BackendConfig` | `backend, dt`. |
| **`Stage`** | Tagged union (tag = method): `IF2 {chains, particles, iterations, cooling, starts_from, loglik_eval, gate}`, `PGAS {chains, particles, sweeps, ...}`, `PMMH {chains, particles, iterations, ...}`, `PFilter {particles, replicates, starts_from}`. |
| `StartsFrom` | `Default | StageName(String)` — references an earlier stage by name. |
| `LoglikEvalConfig` | Clean-eval re-scoring config (n_particles, n_replicates). |
| `GateConfig` | Compound scout-convergence gate (Â floor + decibans spread). |
| `SyntheticConfig` | Synthetic-data fitting (true_params, n_datasets). |

Key methods on `Stage`:
- `method_name() -> &str`
- `requires_priors() -> bool` (PGAS / PMMH only)
- `chains() -> usize`
- **`identity_payload() -> serde_json::Value`** — hashable subset
  *omitting* the extension dimension (PGAS `sweeps`, PMMH `iterations`).
  Drives `provenance::fit_stage_hash` so `--resume` can extend a
  chain without invalidating its identity.

#### 2.3.4 Fit runtime state (`cli/src/fit/`)

The internal types between fit.toml and on-disk artefacts.

| File | Type | Role |
|---|---|---|
| `runner.rs` | **`FitRunConfig`** | Built from `FitConfigV2 + prior_state`. Holds `compiled` (Arc), `model`, `model_ir_json`, `base_params`, `param_names`, `estimated_params`, `observations`, `streams`, `if2_config`, `n_chains`, `seed`, `ic_free`, `loglik_eval`, `gate`. The runtime-side counterpart of `FitConfigV2`. |
| `runner.rs` | `ObsStream` | Per-stream wrapper: name, projection (sim::StreamProjection), obs_model_ir, data. |
| `runner.rs` | `ChainResults` | Output of N parallel IF2 chains: `results, best_chain, best_loglik, chain_agreement, loglik_eval`. |
| `state.rs` | **`FitState`** | Persisted per-stage state in `fit_state.toml`: stage, seed, timestamp, params, best_loglik/chain, hashes, gate verdict, resolved configs. |
| `loglik_eval.rs` | `LoglikEvalOutcome`, `PerChain`, `OverallWinner` | Output of clean-eval re-scoring: per-chain θ̂ and SE, the winner. |
| `pgas.rs` | `PgasStageOpts` | Subset of `Stage::PGAS` actually consumed by the runner. |
| `pmmh.rs` | `PmmhStageOpts` | Same for PMMH. |
| `provenance.rs` | `fit_stage_hash` (fn) | sha256 over (model + data + estimate + fixed + stage_name + identity_payload + seed + version). |
| `provenance.rs` | `mle_params_tamper_hash` (fn) | 8-char hash on `mle_params.toml` payloads (drift detection). |
| `fit_summary.rs` | `FitSummaryDoc`, `StageSummary`, `MlePostProcessSummary`, `MleTableRow`, … | Render-side types for `camdl fit summary`. Targets text/JSON/MD/LaTeX. |
| `fit_table.rs` | `FitTableEntry`, `Format` | Cross-fit aggregate for `camdl fit table`. |
| `table_row.rs` | `TableRow`, `TableRowSchema`, `TableRowError` | One row of a fit table. |
| `config_diff.rs` | `ConfigDiff` | Diff between two fit configs (identity / equivalent / different). |
| `method_result.rs` | `MethodResult`, `MethodView` | Adapter that abstracts IF2/PGAS/PMMH/PFilter result shape behind one shape for table/summary. |
| `fit_tree.rs` | `FitNode`, `walk_fits_root`, `walk_fit_dir` | Filesystem walker over fit trees. |

#### 2.3.5 Browse / list / show / cat (`cli/src/browse.rs`)

| Type | Role |
|---|---|
| `RunEntry` | Cached `Simulate` listing entry: run, meta, rel_path, created, traj_bytes. |
| `FitEntry` | Cached `Fit` listing entry: run, meta, rel_path, created. |
| `ProfileEntry` | Cached profile-umbrella listing entry: run, model/focal/shape display fields, n_seeds. |
| **`ResolvedRun`** | Single resolved run, kind-agnostic: `run, abs_path, rel_path, created`. The output of `resolve_any`; consumed by `cmd_show` and `cmd_cat` via one match-on-kind dispatch. |
| `KindFilter` | `Sim | Fit | Profile | All` for `camdl list --kind`. |

Renderers (one per kind, dispatched by `show()`): `show_simulate`,
`show_fit`, `show_fit_stage`, `show_profile_leaf`, `show_replicate_set`.

#### 2.3.6 CLI args (`cli/src/args/mod.rs`)

| Type | Wires to |
|---|---|
| `SimulateArgs` | `run_simulate` |
| `BatchArgs`, `BatchStatusArgs` | `cmd_batch_run`, `cmd_batch_status` |
| `FitRunArgs`, `FitStatusArgs`, `FitSummaryArgs`, `FitDiffArgs`, `FitTableArgs`, `FitNewArgs`, `FitWhereArgs` | The seven `cmd_fit_*` entry points |
| **`LabelArgs`** | `cmd_label` (top-level) |
| `PfilterArgs` | `cmd_pfilter` |
| `If2Args` | `cmd_if2` |
| `ProfileArgs` | `cmd_profile` |
| `EvalArgs` | `cmd_eval` |
| `DataSplitArgs` | `cmd_data_split` |
| `ListArgs`, `ShowArgs`, `CatArgs` | `cmd_list`, `cmd_show`, `cmd_cat` |
| `CompareArgs` | `cmd_compare` |

Sub-arg structs (composed via `#[command(flatten)]`): `ModelOverrides`
(`--param`, `--params`, `--table`), `ScenarioArgs` (`--scenario`),
`InferenceCore` (`--particles`, `--dt`, `--seed`, `--parallel`),
`SimBackend` (`--backend`, `--dt`), `FlowProjection` (`--flow`).

Type helpers in `args/types.rs`: `ParamKv` (`NAME=VALUE`), `RwSd`
(`auto | NAME=N,...`), `SeedSpec` (`1,2,3` or `1:5`), `Grid`
(`V1,V2,...` or `lin(min,max,n)` or `log10(min,max,n)`), `SweepSpec`
(`NAME=Grid`), `ProgressMode` (`auto | pretty | plain | none`).

#### 2.3.7 Other / cross-cutting

| File | Role |
|---|---|
| `hashing.rs` | sha256 helpers — `model_hash`, `sim_hash`, `scen_hash`, `file_hash`, `path_stem_slug`, `canonical_params`. The shared low-level building block for the CAS impls. |
| `run_paths.rs` | `output_root`, `sim_run_dir`, `fit_run_dir`, `profile_point_dir`, `profile_point_start_dir`. The single source of truth for the on-disk layout. |
| `sampling.rs` | Space-filling sample helpers: `PriorSpec` (batch's flavour), `DesignParam`, `DesignPoints`, Sobol/LHS/random generators. Used by `batch run`. |
| `progress.rs` | Indicatif vs plain-log progress mode. |
| `version.rs` | `VERSION_SHORT` (e.g. `"0.1.0+5f18aea"`). |
| `util.rs` | `SimRun`, `apply_scenario_filter`, `load_model`, `apply_params_file`, `derive_chain_seed`, `write_traj_tsv`, … the shared infrastructure used by every command. |

---

## 3. The CAS abstraction

The single most important type-design decision in the CLI. Every
content-addressable subcommand goes through this:

```text
        ┌────────────────────────────┐
        │  per-command typed inputs  │
        │  (SimulateInputs,          │
        │   FitInputs, StageInputs,  │
        │   ProfileInputs)           │
        └────────────┬───────────────┘
                     │ impl CasInputs
                     ▼
        ┌──────────────────────────────────┐
        │  CasInputs trait                 │
        │    content_hash() -> ContentHash │
        │    cas_path(root) -> PathBuf     │
        │    run_kind() -> RunKind         │
        │    to_run() -> Run        (default)│
        └────────────┬─────────────────────┘
                     │
       ┌─────────────┼─────────────┐
       ▼             ▼             ▼
   run.json       on disk     hash for
   (RunKind     <root>/...    cache check
   payload)
```

**Four roles every input plays** (from `cas/typed.rs` doc):

- **Content** → in hash. Determines validity. Model IR bytes, data
  bytes, algorithm hyperparams, `seed` for stochastic methods,
  `starts_from` upstream lineage.
- **Path** → in path. Determines readability. The 8-char hash
  prefix plus a human stem.
- **Replicate** → parent-child relationship. Inputs that *vary* an
  otherwise-identical run for sensitivity analysis.
- **Ephemeral** → nowhere. `--parallel`, progress mode, output
  mirror paths. Recorded in `argv` for forensics, not in any hash.

**ReplicateSet umbrella.** Multi-realization runs (multi-seed
profile, multi-dataset synthetic fit) get an umbrella with N
children, where `parent_hash = h(inner_hash, dim, sorted_keys,
child_kind)` and each child's hash = `compose_with_replicate(
inner_hash, dim, key)`. As of the post-collapse refactor, profile
*always* uses this shape — N=1 is the trivial case.

---

## 4. Flow narrative

Each subsection traces a command's path from argv to disk.
Boxes-and-arrows view; the goal is to surface the natural seams.

### 4.1 `camdl simulate` (one trajectory)

```text
argv ──▶ SimulateArgs ──┐
                        │ build SimRun (util.rs)
                        ▼
                   util::SimRun ─────────────────────┐
                        │ load_model(ir_path)        │
                        │ apply_scenario_filter      │
                        │ apply_params_files +       │
                        │   --param overrides        │
                        ▼                            │
                   ir::Model ──▶ CompiledModel       │
                        │                            │
                        │ build SimulateInputs       │
                        ▼                            │
                   SimulateInputs ──┬─ content_hash()│
                                    ├─ cas_path()    │
                                    └─ run_kind() ──▶ RunKind::Simulate
                                                     │
                   Run::check_cache(dir, hash) ◀─────┘
                       │
                       ├ Hit → read traj.tsv, return
                       └ Miss/Stale → run backend
                                       │
                                       ▼
                                   sim::Simulate ──▶ Trajectory
                                       │
                                       ├ write_traj_tsv → traj.tsv
                                       ├ obs sampling   → obs/<...>/
                                       └ Run::write     → run.json
```

Key files: `main.rs:312` (`run_simulate`), `main.rs:945`
(`prepare_cas_ctx`), `cas/sim_inputs.rs`, `util.rs`. The `--params
mle.toml` lineage is captured via `from_fit_hash` (read from the
mle.toml's `parent_fit_hash` provenance, written into
`SimulateMeta`).

### 4.2 `camdl batch run` (sweep grid of simulates)

```text
argv ──▶ BatchArgs
            │ load batch.toml
            ▼
        BatchExp ──▶ explode to grid of BatchRunSpec
                          │
                          │ par_iter (rayon)
                          ▼
                    For each (scenario × sweep_point × seed):
                       └── delegates to simulate path (4.1)
                           with `sweep_point` populated
```

The novel bit is `manifest.json` at the batch root — a list of all
grid-point hashes for downstream tooling. Also a sampling design
hook (`sampling::DesignPoints` for Sobol/LHS/random) used when the
batch.toml declares one.

### 4.3 `camdl fit run` (the heaviest command)

```text
argv ──▶ FitRunArgs (incl. --resume, --stage, --label, --sweep, --seed)
            │
            │ FitConfigV2::load(fit.toml)
            ▼
       FitConfigV2 ──┬── validate(model_params)
                     ├── fit_content_hash() ──▶ FitInputs ──▶ Run::Fit (umbrella)
                     ▼
                 Per stage in stages_to_run, per cell (synthetic ds_NN × seed):
                     │
                     │ fit_stage_hash(model, data, estimate, fixed,
                     │                stage_name, stage.identity_payload(),
                     │                seed, version)
                     ▼
                 StageInputs ──▶ Run::FitStage (per-stage leaf)
                     │
                     │ FitRunConfig::build(fit, prior_state, n_chains, ...)
                     ▼
                 FitRunConfig ──┬── compiled : Arc<CompiledModel>
                                ├── estimated_params : Vec<EstimatedParam>
                                ├── streams : Vec<ObsStream>
                                ├── if2_config : IF2Config
                                └── ...
                     │
                     │ dispatch on Stage variant:
                     ▼
       ┌─────────────────────────────────────────────────────────┐
       │ IF2     → run_if2_with_progress * n_chains              │
       │            → IF2Result per chain                        │
       │            → loglik_eval re-scoring (clean θ̂, SE)       │
       │            → ChainResults (best_chain, gate verdict)    │
       │ PGAS    → pgas::run_stage(fit, name, stage, dir, opts,  │
       │            seed, force, use_nuts, dense_mass, resume,   │
       │            starts_from)                                 │
       │            → resume_state.bin (per chain)               │
       │            → trace.tsv, draws.tsv                       │
       │ PMMH    → pmmh::run_stage(...)                          │
       │            → similar                                    │
       │ PFilter → bootstrap PF at fixed θ                       │
       │            → loglik.tsv, optional traces                │
       └─────────────────────────────────────────────────────────┘
                     │
                     ▼
                 FitState (toml) + mle_params.toml +
                 Run::FitStage write + (PGAS/PMMH only)
                 chain_<n>/{trace.tsv, resume_state.bin,
                            trajectories/}
```

Key files: `fit/mod.rs:168` (`cmd_fit_run_v2`), `fit/runner.rs:97`
(`FitRunConfig::build`), `fit/pgas.rs`, `fit/pmmh.rs`,
`fit/loglik_eval.rs`, `fit/provenance.rs:299` (`fit_stage_hash`).

The **identity-vs-extension split** lives at the
`stage.identity_payload()` call inside `fit_stage_hash`: PGAS's
`sweeps` and PMMH's `iterations` are *not* in the hash, so resume
can extend a chain by changing only that field.

### 4.4 `camdl profile` (always a ReplicateSet)

```text
argv ──▶ ProfileArgs (incl. --seeds, --label)
            │
            │ build ProfileInputs (template, seed-overwritten per child)
            ▼
       ProfileInputs.inner_hash() (seed-free)
            │
            │ ReplicateSet { inner_hash, "seed", keys=["seed_X", ...], "profile" }
            ▼
       parent_hash = ReplicateSet.parent_hash()
            │
            │ write umbrella Run::ReplicateSet at <root>/profiles/<stem>-<short>/
            ▼
       For each seed:
           ├─ child_dir = umbrella/replicates/seed_<n>/
           ├─ ProfileInputs { seed }.run_kind() ──▶ Run::Profile (per-seed leaf)
           └─ For each (grid_point, start):
                  ├─ <seed_dir>/points/{idx:05d}/start_{k}/
                  ├─ Run if2 ──▶ Run::FitStage with parent_profile_hash
                  └─ profile.tsv rollup at the seed level
       │
       ▼
       summary.tsv at the umbrella (cross-seed aggregate; 1 row at N=1)
```

Key file: `profile.rs:179` (`cmd_profile`).

### 4.5 `camdl label` (top-level, kind-agnostic)

```text
argv ──▶ LabelArgs { hash, label, root }
            │
            │ validate_label (regex)
            │ walk <root>/{sims,fits,profiles}/** for run.json
            │ match Run.hash by prefix (across kinds)
            ▼
       Run ──▶ run.label = Some(...)
            │ atomic write (tmp-then-rename)
            ▼
       run.json updated
```

Key file: `fit/mod.rs:1608` (`cmd_label`). Refuses to relabel a
running fit (wall_time_seconds == 0.0 sentinel).

### 4.6 `camdl list` / `show` / `cat`

```text
list ──▶ discover_runs / discover_fits / discover_profiles
            │
            ▼
       Vec<RunEntry> | Vec<FitEntry> | Vec<ProfileEntry>
            │ filter (--since, --kind, --label-pattern)
            ▼
       comfy_table render with HASH + LABEL columns

show ──▶ resolve_any(root, key)
            │ walks <root>/{sims,fits,profiles}/**/run.json
            │ matches Run.hash prefix; for sims also matches sim_hash
            ▼
       ResolvedRun { run, abs_path, rel_path, created }
            │ match run.kind { ... }
            ▼
       show_simulate / show_fit / show_fit_stage /
       show_profile_leaf / show_replicate_set

cat ──▶ resolve_any
            │
            ▼
       ResolvedRun ──▶ match kind:
           Simulate → traj.tsv (or obs/*/<stream>.tsv with --stream)
           ReplicateSet → summary.tsv
           Profile → profile.tsv
           Fit / FitStage → error: ambiguous
```

The post-collapse pattern: every kind goes through one `match
run.kind` dispatch, no parallel `Resolved` enum.

### 4.7 `camdl pfilter` / `camdl if2` (no-CAS standalones)

These bypass CAS entirely. They take CLI args, build a sim setup
inline, run, and write trace/diagnostic TSVs directly. No
`run.json`, no cache, not surfaced by `camdl list`. Use case: quick
interactive exploration; the persistent path is `fit run` with a
`pfilter` / `if2` stage in the fit.toml.

---

## 5. Cross-crate seams

This is where most type-design risk concentrates. Each seam is a
shape-conversion point between two layers' worldviews.

### 5.1 `FitConfigV2` → `FitRunConfig` (CLI → sim setup)

`runner.rs:97` `FitRunConfig::build(fit, prior_state, n_chains,
n_particles, n_iterations, cooling, seed, random_starts) ->
FitRunConfig`.

Field-by-field mapping (~225 LOC):

| `FitConfigV2` field | `FitRunConfig` field | Conversion |
|---|---|---|
| `model.camdl` (path) | `compiled : Arc<CompiledModel>` | `load_model + apply_scenario_filter + CompiledModel::new` |
| `scenario` / `enable` / `disable` | (applied to `model` before compile) | mutually exclusive validation |
| `fixed` (`FixedParams`) | (overlaid into `base_params`) | `FixedParams::resolve()` (file or inline) |
| `estimate` (`IndexMap<EstimateSpecV2>`) | `estimated_params : Vec<EstimatedParam>` | `build_if2_params_from_specs` → see 5.2 below |
| `data.observations` | `streams : Vec<ObsStream>` | `load_observations` per stream + `StreamProjection::from_ir` |
| `config.dt`, `config.backend` | `if2_config.dt` | direct |
| `ic_free` | `if2_config.skip_first_obs_from_loglik`, `ic_free` | direct |
| (n_chains, particles, iterations, cooling come from the dispatcher) | `n_chains`, `if2_config` | per-stage knobs |

This is the central conversion. Every fit downstream of `cmd_fit_run_v2`
goes through it.

### 5.2 `EstimateSpecV2` → `EstimatedParam` (the parameter contract)

`runner.rs:583` `build_if2_params_from_specs(estimate, model,
compiled, base_params)`.

```text
(EstimateSpecV2.bounds OR ir::Parameter.bounds)  ──┐
EstimateSpecV2.transform OR derive_transform()    ──┤
EstimateSpecV2.start OR base_params[idx]          ──┤  ──▶ EstimatedParam
EstimateSpecV2.rw_sd (optional)                   ──┤        { name, index, initial,
EstimateSpecV2.ivp (optional)                     ──┘          rw_sd, transform, lower,
                                                                upper, ivp, rw_sd_auto }
```

`derive_transform` (runner.rs:536) is the precedence chain:
fit.toml override → IR `param_kind` → fallback heuristic. The seam
point: CLI's `Transform` (`Log | Logit | Identity`, unbounded) maps
to sim's `Transform` (`None | Log{lo,hi} | Logit{lo,hi}`, bounded)
by attaching `(lower, upper)`.

### 5.3 Prior conversion — *the four-PriorSpec smell*

There are **four distinct prior representations** in the workspace:

| Type | Where | Shape | Scope |
|---|---|---|---|
| `ir::parameter::PriorDist` | `ir/src/parameter.rs` | enum, 8 variants | Embedded in IR; what the OCaml compiler emits. |
| `cli::fit::config_v2::PriorSpec` | `cli/src/fit/config_v2.rs` | enum, 7 variants (no Hierarchical) | What users write in `fit.toml`. |
| `cli::sampling::PriorSpec` | `cli/src/sampling.rs` | struct, stringly-typed (`dist: String`) | Used by `batch run` for VOI importance weighting. |
| `sim::inference::Prior` | `sim/src/inference/prior.rs` | enum, 9 variants (incl. Hierarchical) | The runtime evaluator. |

Conversions:

```
ir::PriorDist ──Prior::from_ir()──▶ sim::Prior
config_v2::PriorSpec ──prior_spec_to_prior()──▶ sim::Prior
                                        (runner.rs:1492)

resolve_prior(name, fit.estimate, model, default=Flat):
    1. fit.toml's PriorSpec (if set)        → prior_spec_to_prior
    2. else IR's PriorDist (if set)         → Prior::from_ir
    3. else IR's HierarchicalPrior (if set) → Prior::Hierarchical
    4. else                                 → Prior::Flat
```

`sampling::PriorSpec` is a *fifth* form that doesn't even share
variants with the others (it's stringly-typed with optional fields).
Used in exactly one place (`batch.rs:63`'s `DesignParam.prior` for
VOI weighting); it doesn't currently feed into `sim::Prior`.

**Smell.** This is the single biggest schema dedup opportunity. See
`CLEANUP-prior-types.md` and §6.1.

### 5.4 IR `ObservationModel` → sim `MultiStreamObsModel`

Per stream in `runner.rs:230`:

```text
fit.data.observations[name] = path
   │
   │ load_observations(path, name, dt) ──▶ Vec<sim::Observation>
   │
   │ ir::Model.observations.find(name) ──▶ ir::ObservationModel
   │     │
   │     │ StreamProjection::from_ir(ir.projection, compiled, name)
   │     ▼
   │ sim::StreamProjection
   │
   ▼
ObsStream { name, projection, obs_model_ir, data }

vec![ObsStream] ──▶ MultiStreamObsModel::new(StreamSpec[])
```

The IR's `ObservationModel.likelihood` is consumed inside
`MultiStreamObsModel`'s `log_likelihood` evaluation — never
explicitly converted to a sim type.

### 5.5 Stage results → `FitStageMeta.algorithm`

After a stage finishes, the dispatcher builds a `FitStageMeta` for
the per-stage `run.json`. The `algorithm` field is
`serde_json::Value` — a free-form blob carrying method-specific
knobs and convergence diagnostics. Per method:

- **IF2**: `{method, chains, particles, iterations, cooling}` plus
  the loglik-eval winner's θ̂ and per-chain table.
- **PGAS**: `{method, chains, particles, sweeps, burn_in, thin}`
  plus `n_sweeps_done` (resume support), R̂, ESS, acceptance rates.
- **PMMH**: similar to PGAS, plus proposal-tuning state.
- **PFilter**: `{method, particles, replicates}` plus loglik mean/SD.

The `serde_json::Value` shape is a deliberate punt — the human-
readable record doesn't need a typed schema, and the stages have
genuinely different parameter sets. `MethodResult` /
`MethodView` (in `fit/method_result.rs`) is the typed reader-side
adapter that pulls back what the table/summary renderers need.

### 5.6 `Trajectory` → `traj.tsv`

`util::write_traj_tsv(model, trajectory, path)` reads the model
structure to figure out compartment ordering, then walks
`trajectory.snapshots` and writes `t \t state_1 \t ... \t state_N
\t flow_1 \t ...` (flows optional). One-way; the file is the
artefact, never re-parsed back into a `Trajectory`. The reverse
direction — TSV → particle filter `Vec<Observation>` — only needs
two columns (`t`, `value`) and lives in `pfilter`/runner code.

---

## 6. Type-flow smells

These are observations to refactor (or defer). Listed roughly by
ROI of fix-now vs. ignore.

### 6.1 The four `PriorSpec` representations (high ROI)

See §5.3. Tracked separately in `CLEANUP-prior-types.md`. Two
candidate fixes:

- **Minimum**: delete `cli::sampling::PriorSpec`. It's stringly-
  typed, doesn't feed into `sim::Prior`, and the VOI workflow could
  use `config_v2::PriorSpec` directly.
- **Full**: collapse to two types — `ir::PriorDist` (serialization /
  schema) and `sim::Prior` (runtime evaluator), with `from_ir` as
  the bridge. `config_v2::PriorSpec` becomes an alias / re-export of
  `ir::PriorDist`. This requires the IR to gain Hierarchical (it
  doesn't have it as a `PriorDist` variant — it lives separately as
  `HierarchicalPrior`).

Either way, the seam at `runner.rs:1492` (`prior_spec_to_prior`)
goes away.

### 6.2 Two `Transform` types — `ir::Transform` and `sim::Transform`

`ir::parameter::Transform` (`Log | Logit | Identity`) is unbounded —
it carries no bounds. `sim::inference::types::Transform` (`None |
Log{lo,hi} | Logit{lo,hi}`) is the *bounded* runtime version.

The conversion happens silently in `EstimatedParam` construction:
the IR transform is a kind discriminator and the sim transform
attaches `(lower, upper)` from `EstimateSpecV2.bounds`. There's
also a third — `cli::fit::config_v2::Transform` — which is a
faithful clone of `ir::Transform`.

This is fine in practice (the bounds attachment is a real semantic
step) but worth flagging: if you ever want to write `impl
TryFrom<ir::Transform> for sim::Transform`, you'd need to thread
bounds through the conversion, which clarifies what's happening.

### 6.3 `cli::sampling::PriorSpec` is stringly-typed

`pub struct PriorSpec { pub dist: String, pub alpha: ..., ... }` —
panics-on-dispatch (`other => format!("{}(?)", other)`). Should be
an enum like `config_v2::PriorSpec`. Tracked under §6.1.

### 6.4 `FitStageMeta.algorithm: serde_json::Value`

A typed `enum AlgorithmDiagnostics { IF2(...), PGAS(...), ... }`
would be safer (no key-mistype risk in the renderers) but
significantly heavier. Current `MethodResult` adapter approach is
acceptable; defer.

### 6.5 `RunEntry` / `FitEntry` / `ProfileEntry` redundancy

Each is `{run, meta, paths, created}` where `meta` is a destructured
copy of `run.kind`'s payload. Stored alongside for direct field
access without repeated `match`. Now that `cmd_show` and `cmd_cat`
go through `ResolvedRun + match` and don't use these typed entries,
the typed entries only exist for `cmd_list`'s table renderers.

Cleanup option: have `cmd_list` also use `ResolvedRun` and `match
&run.kind { ... }` per row, deleting the three entry types. The
typed-meta convenience in renderers is preserved by binding inside
the match arm.

Estimated diff: ~80 LOC removed, ~30 added. ROI is "nicer," not
"unblocks anything."

### 6.6 `loglik_eval` and `gate` defaulting in `FitRunConfig`

`FitRunConfig::build` sets these to `Default::default()` and the
real values get patched in by the dispatcher (`fit/mod.rs`)
*after* construction. That's a two-step initialization smell —
the defaults are wrong-but-harmless because they get overwritten,
but a bug that forgets to overwrite gets `(4000, 8)` for
`loglik_eval` and never fires the gate.

Minimum fix: make these fields `Option<...>` and have the
dispatcher require an explicit set. Bigger fix: pass them as
`build()` arguments.

### 6.7 PGAS / PMMH stage opts revert v1 knobs to defaults

After the v1 cleanup, several v1-only knobs (`tempering`,
`max_treedepth`, `trajectory_warmup`, `csmc_sweeps_per_nuts`,
`n_trajectories` for PGAS; analogous for PMMH) revert to defaults
because v2's `Stage::PGAS` / `Stage::PMMH` doesn't surface them.
Documented in the v1-cleanup commit message.

This is an outstanding feature-completeness gap, not a smell —
listed here because the doc-survey will surface it. If anyone
needs those knobs, surfacing them in `Stage::PGAS` / `Stage::PMMH`
plus the `PgasStageOpts::from_stage` / `PmmhStageOpts::from_stage`
adapters is mechanical.

### 6.8 `DEFAULT_N_TRAJECTORIES` constant in pgas.rs

`pgas.rs` has `const DEFAULT_N_TRAJECTORIES: usize = 200;` set
inline. After v1 cleanup it's the only knob path; should either
be in `Stage::PGAS` or documented as fixed.

### 6.9 `--starts-from` is `Option<&str>` everywhere

The `starts_from` arg is shaped as `Option<&str>` through pgas.rs
and pmmh.rs entry points, but `Stage::PGAS.starts_from` is a typed
`StartsFrom` enum. They get joined at the dispatcher level
(`mod.rs`'s `effective_starts.as_deref()`). The narrow stringy
shape is fine at the call boundary; just worth knowing the seam
exists.

### 6.10 `IF2Result` / `PGASResult` / `PMMHResult` shapes are diagnostic-heavy

Each result type carries 10+ fields of inference diagnostics, and
the `FitStageMeta.algorithm` JSON has to flatten them by hand at
the dispatcher. A smaller `StageDiagnostics` trait that each result
type implements (`fn into_meta_json(&self) -> Value`) would
centralize the per-method diagnostic schema. Defer; the current
pattern is verbose but the correctness pressure is low (it's
diagnostic-only output).

### 6.11 `cli/src/util.rs` is a kitchen sink (~1300 LOC)

`SimRun`, `apply_scenario_filter`, `load_model`, `apply_params_file`,
`derive_chain_seed`, `write_traj_tsv`, `resolve_ir_path`, …. The
file is doing too much. Splittable into `util/io.rs` (load/write),
`util/scenario.rs` (filter + apply), `util/seed.rs` (derive). Pure
mechanical refactor; defer until there's a reason.

### 6.12 No `From` / `Into` impls across the seams

Every CLI→sim conversion is a free function (`prior_spec_to_prior`,
`derive_transform`, `StreamProjection::from_ir`). Idiomatic Rust
would make these `From` / `TryFrom` impls. The current state is
functional and grep-discoverable, just slightly less idiomatic.

---

**Bottom line.** The codebase is in good shape post-cleanup. The
big architectural decisions (CAS abstraction, FitConfigV2 as the
single fit-config schema, kind-tagged `Run` envelope, identity vs
extension hash split for resume) all hold up under the type-flow
view. The only structural smell with non-trivial ROI is the
four-`PriorSpec` situation (§6.1). Everything else is either
deferred-defer (§6.4, §6.5, §6.10, §6.11) or one-commit cleanup
(§6.6, §6.8).

If we want to shave one more thing before alpha, **prior-type
unification (§6.1)** is the natural next pass — it's the last place
where the CLI/sim/IR seam shows obvious duplication.
