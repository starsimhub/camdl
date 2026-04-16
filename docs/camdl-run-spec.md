# camdl Run System Specification

**Version:** 0.4-draft  
**Date:** 2026-04-15

> **Scope:** This spec describes camdl's complete run system: forward
> simulation (single runs and batches), inference pipelines (`fit.toml`),
> parameter sweeps, predictive workflows, and the provenance/caching
> infrastructure that underlies all of them. It supersedes the
> forward-simulation-only experiment system (experiment.toml v0.6).

---

## 1. Design Principles

### 1.1 Background: How camdl Partitions Model Inputs

A camdl model defines a stochastic simulator whose inputs are partitioned into
three categories (Buffalo 2026):

- **Model parameters (M):** The tuneable knobs — quantities that *could* be
  varied during calibration or sensitivity analysis. Transmission rate β,
  recovery rate γ, reporting probability ρ, etc. Declared in the `.camdl` file's
  `parameters { }` block.

- **Configuration (C):** Structural and runtime choices that are never subject
  to calibration. Population structure, time step, which outputs to record,
  which interventions are enabled. Defined by the `.camdl` model structure plus
  scenario patches.

- **Seed (s ∈ S):** The base random seed for stochastic simulation. Always a
  CLI argument, never baked into a config file.

A simulation is then the mapping Sim(m, c, s) → y, producing trajectories
and observations. Every workflow in this spec — forward simulation, sweeps,
inference, predictive checks — is an operation on these three input layers.

**Scenarios** are deterministic patches σ that modify parameters and/or
configuration from their baseline values: σ(m, c) → (m', c'). They are defined
in the `.camdl` file's `scenarios { }` block and selected at runtime. The
baseline is the identity patch — the model as written, no modifications.

**Inference** operates on a *view* of the parameter space. When fitting a model,
some parameters are estimated (free to vary) while others are held fixed. This
partition — `[estimate]` vs `[fixed]` in fit.toml — defines which parameters
the inference algorithm explores and which it treats as known constants.

### 1.2 File Roles and Separation of Concerns

**One file per concern, no overlap.**

```
model.camdl      → what the model IS (structure, scenarios)
params.toml      → a point m ∈ M (concrete parameter values)
fit.toml         → how inference RUNS (what to estimate, algorithm, data)
batch file       → how a batch RUNS (sweep/scenarios/seeds, via --batch)
```

Each file owns its domain exclusively. The fit file cannot define model
structure — that's what `.camdl` is for. The params file cannot set the
backend — that's a CLI or batch concern. The `.camdl` file cannot set concrete
parameter values (outside of named scenario presets) — that's what external
files are for.

**The model file is self-contained for single runs.** A `.camdl` file with
a params.toml and a seed is everything needed for one simulation — no batch
file or fit config required.

```
┌──────────────────────┬───────────────────────┬───────────────────────────┐
│       Concern        │     Lives in          │     Can override?         │
├──────────────────────┼───────────────────────┼───────────────────────────┤
│ Model structure      │ .camdl only           │ never                     │
│ Parameter names/types│ .camdl only           │ never                     │
│ Parameter values (M) │ params.toml / [fixed] │ --param, sweep, scenario  │
│ Scenarios (σ)        │ .camdl scenarios { }  │ batch adds on top         │
│ Interventions        │ .camdl only           │ scenarios enable/disable  │
│ Backend choice       │ CLI / batch           │ CLI --backend             │
│ Seeds                │ CLI / batch           │ CLI --seed/--seeds        │
│ Sweep / Design       │ CLI / batch           │ never                     │
│ Estimate vs fixed    │ fit.toml only         │ --sweep overrides [fixed] │
│ Inference algorithm  │ fit.toml only         │ CLI --stage               │
│ Priors               │ fit.toml only         │ never                     │
└──────────────────────┴───────────────────────┴───────────────────────────┘
```

### 1.3 Precedence Chains

**For M (parameters) in simulation:**

```
params.toml                          (base values)
  ↓ overridden by
sweep point overrides                (automated M-layer variation)
  ↓ overridden by
scenario params                      (counterfactual modifications to M)
  ↓ overridden by
--param CLI flags                    (convenience, non-persistent)
```

**For M in inference:**

```
[fixed] from_file                    (bulk fixed values)
  ↓ overridden by
[fixed] inline values                (specific overrides)
  ↓ overridden by
--sweep point overrides              (grid of fits)
```

Parameters in `[estimate]` are never overridden — they are the free variables
the algorithm explores.

### 1.4 Core Design Rules

**CLI and file are the same type.** Every batch TOML file deserializes into the
same Rust struct that CLI argument parsing produces. `--batch file.toml` and a
long command line are interchangeable representations of the same job. This is
enforced by deriving both `clap::Parser` and `serde::Deserialize` from shared
types.

**No silent defaults for parameters.** Following camdl's core philosophy,
parameter values are never silently inherited. In `fit.toml`, every model
parameter must appear in exactly one of `[estimate]` or `[fixed]`. Missing a
parameter is a hard error. In simulation, `--params` or `--draws` must cover
all of M. There are no fallback defaults — the user must make every parameter
choice explicit. (See the camdl language spec §4.2 and the scenarios chapter
for the rationale behind this design.)

**Sweeps are orthogonal to everything.** A sweep is "run this thing at multiple
parameter values." It works identically on simulation and inference. The
`--sweep` flag / `[sweep]` section varies parameters across a grid. Sweeps
compose with scenarios (σ layer) and seeds (S layer) via Cartesian product:
total runs = |param_points| × |scenarios| × |seeds|.

**Provenance is structural, not aspirational.** Every run writes metadata
recording exact inputs, hashes, versions, and lineage. The runner validates
consistency before overwriting — stale results from a changed config are never
silently replaced. The types that describe jobs also compute their output paths
and content hashes.

**Draws and sweeps are different operations.** A sweep is a deterministic grid
the user designed. Draws are samples from a distribution (posterior, prior, or
uniform from bounds). They have different provenance, different downstream
semantics, and different output structure. They are separate variants of a sum
type, never conflated.

**Reproducibility is structural.** Every simulation output is
content-addressed by the inputs that produced it. Same inputs → same hash →
same directory. Different inputs → different hash → separate directory. M and
σ are distinct layers — the CLI makes this visible: `--param` operates on M,
`--scenario` and `--enable` operate on σ. They never share flags.

---

## 2. Project Directory Structure

```
project/
├── models/
│   ├── sir.camdl
│   └── seir_nigeria.camdl
├── data/
│   ├── cases.tsv
│   └── lga_pop.tsv
├── params/
│   └── baseline.toml
├── fits/                            # fit.toml configs
│   ├── 01_all_free.toml
│   ├── 02_fix_beta.toml
│   └── 03_rho_sweep.toml
├── batches/                         # simulation batch configs
│   ├── scenario_comparison.toml
│   └── ppc.toml
└── results/                         # ALL output under one tree
    ├── fits/                        # inference results (named dirs)
    │   ├── 01_all_free/
    │   │   ├── mle/
    │   │   │   ├── provenance.json
    │   │   │   ├── traces.tsv
    │   │   │   └── mle_params.toml
    │   │   └── posterior/
    │   │       ├── provenance.json
    │   │       ├── draws.tsv
    │   │       └── diagnostics.json
    │   └── 02_fix_beta/
    │       └── ...
    └── simulate/                    # batch sim results (hash-addressed)
        ├── manifest.json
        ├── model.ir.json
        └── {sim_hash_8}/
            └── {scenario_slug}-{scen_hash_8}/
                └── seed_{n}/
                    ├── traj.tsv
                    └── run.json
```

### 2.1 Why Two Caching Strategies

**Fit results use named directories** because fits are iterative,
human-driven experiments. You want to see `01_all_free/mle/` in your file
browser, not a hash. You reason about fits by name ("the one where I fixed
beta"). Named directories support this. Cache invalidation is handled by
hash-based staleness detection inside `provenance.json` (see §9), not by
directory naming.

**Simulation results use content-addressed (hash-based) directories** because
batch simulations are reproducible and high-volume. You might run 18,000
simulations across a sweep × scenario × seed grid. Content addressing gives
you free deduplication: same inputs → same hash → same directory → skip. You
never browse these directories manually — you access results through the
manifest or summary tools.

### 2.2 Fit Result Layout

```
results/fits/{fit_name}/
  {stage_name}/
    provenance.json        # inputs, hashes, timing, lineage
    mle_params.toml        # (optimization stages) best parameters
    traces.tsv             # (optimization stages) per-iteration traces
    draws.tsv              # (sampling stages) posterior draws, complete M
    diagnostics.json       # (sampling stages) ESS, R-hat, acceptance
    logliks.tsv            # (pfilter stages) per-replicate logliks
    chain_{n}/             # per-chain output subdirectory
```

### 2.3 Sweep Subdirectories

When a fit is swept over a fixed parameter, each sweep point gets a
subdirectory under the fit name:

```
results/fits/03_rho_sweep/
  rho_0.500/
    mle/...
    posterior/...
  rho_0.100/
    mle/...
  rho_0.020/
    mle/...
```

For multi-parameter sweeps, directory names concatenate with double underscores:
`rho_0.500__k_5.000/`. Within each sweep point directory, the stage layout is
identical to a non-swept fit.

### 2.4 Simulation Result Layout

```
results/simulate/
  manifest.json              # index of all completed runs
  model.ir.json              # compiled model (self-contained)
  {sim_hash_8}/              # model + base params + backend + dt
    {scenario_slug}-{scen_hash_8}/
      seed_{n}/
        traj.tsv
        run.json
```

**Scenario slug:** scenario name lowercased, non-`[a-z0-9_]` replaced with `_`.

**Seed directory:** `seed_{N}` with verbatim u64, no zero-padding.

Example with two scenarios and a sweep point:

```
results/simulate/
  3a7f2c1d/baseline-00000000/seed_1/
  3a7f2c1d/with_sia-f9e2b047/seed_1/
  3a7f2c1d/with_sia_vacc_eff_0.5-a3c1e890/seed_1/
```

A scenario with no overrides, enables, or disables always produces
`scen_hash = sha256("")` → `00000000` prefix, visually identifying it as the
unmodified baseline.

---

## 3. Core Types

### 3.1 SimulateJob — the universal simulation type

```rust
/// Everything needed to run one or more simulations.
/// Deserializes from batch TOML or constructs from CLI args.
/// This is THE type — CLI and file both produce this.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateJob {
    pub model: PathBuf,
    /// Base parameter values. Optional because draws (§3.4) can provide
    /// complete parameter vectors. When None and source is Point or Sweep,
    /// the runner validates that all parameters are covered by --param
    /// overrides or sweep values — missing parameters are a hard error,
    /// not a silent default.
    pub params: Option<PathBuf>,
    #[serde(default = "default_backend")]
    pub backend: Backend,
    #[serde(default = "default_dt")]
    pub dt: f64,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,

    /// Where parameter vectors come from — the central dispatch
    #[serde(flatten)]
    pub source: ParamSource,

    /// σ layer — which scenarios to run (empty = baseline only)
    #[serde(default)]
    pub scenarios: Vec<ScenarioRef>,

    /// S layer
    #[serde(default)]
    pub seeds: Seeds,

    /// Generate synthetic observations from the observations block
    #[serde(default)]
    pub obs: bool,

    /// Parallelism (Rayon thread count)
    #[serde(default = "default_parallel")]
    pub parallel: usize,

    /// GeoJSON file to copy into output for web visualization
    pub geo: Option<PathBuf>,
}
```

### 3.2 ParamSource — where parameter vectors come from

This is the central sum type. It determines the shape of a batch: are you
running one simulation (Point), a designed grid (Sweep), or sampling from a
distribution (Draws)? Exactly one variant is active per job.

```rust
/// Untagged is safe here because the three variants are structurally
/// distinct in TOML: Sweep requires a [sweep] table, Draws requires a
/// [draws] table, and Point is the fallback when neither exists.
/// No valid TOML matches multiple variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamSource {
    /// Deterministic grid: Cartesian product of swept values.
    /// Each point overrides the corresponding key in params.toml.
    Sweep {
        sweep: IndexMap<String, SweepSpec>,
    },

    /// Samples from a file or distribution.
    /// Each row/sample is a complete parameter vector.
    Draws {
        draws: DrawsSpec,
    },

    /// Single point: params.toml + optional CLI overrides.
    /// Default when neither sweep nor draws is specified.
    Point {
        #[serde(rename = "param", default)]
        overrides: Vec<ParamOverride>,
    },
}

impl ParamSource {
    /// Total parameter points in the batch.
    /// Total runs = n_points × |scenarios| × |seeds|.
    pub fn n_points(&self) -> usize {
        match self {
            ParamSource::Point { .. } => 1,
            ParamSource::Sweep { sweep } => {
                sweep.values().map(|s| s.len()).product()
            }
            ParamSource::Draws { draws } => draws.n_points(),
        }
    }
}
```

### 3.3 SweepSpec — parameter grid specification

Multiple swept parameters produce a Cartesian product. For two parameters
with 9 and 5 values respectively, the grid has 45 points — each point is
an (R0, gamma) pair that overrides the base parameter values.

```rust
/// How to generate values for one swept parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SweepSpec {
    /// Explicit values: vacc_eff = [0.1, 0.3, 0.5, 0.7, 0.9]
    List(Vec<f64>),

    /// Generator (tagged by inner key)
    Generator(SweepGenerator),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SweepGenerator {
    #[serde(rename = "linspace")]
    Linspace { min: f64, max: f64, n: usize },

    #[serde(rename = "logspace")]
    Logspace { min: f64, max: f64, n: usize },

    #[serde(rename = "range")]
    Range { min: f64, max: f64, step: f64 },
}

impl SweepSpec {
    pub fn expand(&self) -> Vec<f64> { todo!() }

    /// Count without allocating.
    pub fn len(&self) -> usize {
        match self {
            SweepSpec::List(v) => v.len(),
            SweepSpec::Generator(g) => g.len(),
        }
    }
}

impl SweepGenerator {
    pub fn len(&self) -> usize {
        match self {
            Linspace { n, .. } | Logspace { n, .. } => *n,
            Range { min, max, step } => ((max - min) / step).ceil() as usize + 1,
        }
    }
}

/// A single point in the sweep grid.
pub type SweepPoint = IndexMap<String, f64>;

/// Expand a multi-parameter sweep into its Cartesian product.
pub fn expand_sweep(sweep: &IndexMap<String, SweepSpec>) -> Vec<SweepPoint> {
    let expanded: Vec<(String, Vec<f64>)> = sweep
        .iter()
        .map(|(k, s)| (k.clone(), s.expand()))
        .collect();
    cartesian_product(&expanded)
}
```

| Generator   | Parameters           | Description              |
| ----------- | -------------------- | ------------------------ |
| (bare list) | —                    | Explicit values          |
| `linspace`  | `min`, `max`, `n`    | n evenly-spaced points   |
| `logspace`  | `min`, `max`, `n`    | n log-spaced points      |
| `range`     | `min`, `max`, `step` | Step from min toward max |

All generator args are keyword — no positional ambiguity.

### 3.4 DrawsSpec — parameter samples

Draws represent parameter vectors sampled from a distribution — fundamentally
different from a sweep grid. A sweep is a design you chose; draws are samples
from inference output or a prior. The distinction matters for provenance:
downstream analyses need to know whether results came from a designed grid or
a posterior sample.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source")]
pub enum DrawsSpec {
    /// Load from a TSV file (posterior draws, external samples).
    /// Each row is a complete parameter vector (all of M).
    /// Column names must match model parameter names.
    #[serde(rename = "file")]
    File {
        file: PathBuf,
        /// Stochastic replicates per draw (different seeds, same params).
        #[serde(default = "default_one")]
        replicates: usize,
    },

    /// Sample from declared priors in a fit.toml.
    /// Requires a fit config with [estimate] entries that have
    /// prior = { ... } specifications. Errors if any estimated
    /// parameter lacks a declared prior.
    #[serde(rename = "prior")]
    Prior {
        fit: PathBuf,
        n: usize,
        #[serde(default = "default_one")]
        replicates: usize,
    },

    /// Sample uniformly from parameter bounds.
    /// Uses bounds from the model's parameter declarations
    /// (the `in [lo, hi]` clause on each parameter).
    /// Named honestly: this is NOT a prior — it's space-filling
    /// exploration for model debugging.
    #[serde(rename = "uniform")]
    Uniform {
        n: usize,
        #[serde(default = "default_one")]
        replicates: usize,
    },
}
```

### 3.5 Seeds — S layer specification

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Seeds {
    Single(u64),
    Count { n: u64 },
    Range { from: u64, to: u64 },
    List { list: Vec<u64> },
}

impl Seeds {
    pub fn expand(&self) -> Vec<u64> {
        match self {
            Seeds::Single(s) => vec![*s],
            Seeds::Count { n } => (1..=*n).collect(),
            Seeds::Range { from, to } => (*from..=*to).collect(),
            Seeds::List { list } => list.clone(),
        }
    }
}

impl Default for Seeds {
    fn default() -> Self { Seeds::Single(1) }
}
```

TOML examples:

```toml
seeds = 42                           # single
seeds = { n = 1000 }                 # 1, 2, ..., 1000
seeds = { from = 1, to = 1000 }     # range: 1..=1000
seeds = { list = [42, 137, 256] }    # explicit list
```

### 3.6 ScenarioRef — σ layer

```rust
/// A scenario reference in a batch job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScenarioRef {
    /// Reference a scenario defined in the .camdl file
    Named(String),

    /// Inline definition (in batch TOML [[scenario]] entries)
    Inline {
        name: String,
        #[serde(default)]
        enable: Vec<String>,
        #[serde(default)]
        disable: Vec<String>,
        #[serde(default)]
        params: IndexMap<String, f64>,
    },
}
```

When no `[[scenario]]` entries are defined and no `--scenario` flag is given,
a single implicit baseline (the identity patch — no enables, no disables, no
param overrides) is used. This is not a "default scenario" — it is the absence
of any scenario patch.

### 3.7 Output Path Methods

Output paths are computed by methods on the job types — never constructed
ad-hoc. This guarantees that provenance recording, cache checking, and the
runner all agree on where results live.

```rust
impl SimulateJob {
    /// Canonical output path for a single simulation run.
    pub fn run_path(
        &self,
        ir: &CompiledModel,
        base_params: &ParamSet,
        sweep_point: Option<&SweepPoint>,
        scenario: &ScenarioRef,
        seed: u64,
    ) -> PathBuf {
        let sim_hash = self.sim_hash(ir, base_params);
        let scen_hash = scenario.scen_hash(sweep_point);
        let slug = scenario.slug();

        self.output_dir
            .join("simulate")
            .join(&sim_hash[..8])
            .join(format!("{}-{}", slug, &scen_hash[..8]))
            .join(format!("seed_{}", seed))
    }
}
```

---

## 4. Single-Run Simulation (No Batch File)

The `.camdl` file + `params.toml` is a complete, self-contained specification
for running a model. No batch file or fit config is needed for exploration.

### 4.1 Basic CLI

```bash
# Baseline (no scenario), output to stdout
camdl simulate model.camdl --params params.toml --seed 42

# Named scenario from .camdl
camdl simulate model.camdl --params params.toml --scenario with_sia --seed 42

# Ad-hoc intervention toggle (no named scenario)
camdl simulate model.camdl --params params.toml --enable sia_round_1 --seed 42

# Parameter override (M layer)
camdl simulate model.camdl --params params.toml --param beta=0.5 --seed 42

# Scenario + parameter override (σ layer + M layer — both valid)
camdl simulate model.camdl --params params.toml --scenario with_sia --param beta=0.5 --seed 42
```

### 4.2 CLI Flag Rules

**`--scenario` and `--enable`/`--disable` are mutually exclusive (both σ
layer):**

```bash
# ERROR:
camdl simulate model.camdl --scenario with_sia --enable sia_round_2
# error: --scenario and --enable/--disable are mutually exclusive.
#   --scenario selects a named scenario from the model file.
#   --enable/--disable composes an ad-hoc scenario.
#   To combine both, define a composed scenario in the model file.
```

**`--param` is always valid (M layer, independent of σ layer):**

```bash
# All valid — --param operates on M, independent of scenario choice:
camdl simulate model.camdl --params p.toml --param beta=0.5 --seed 42
camdl simulate model.camdl --params p.toml --scenario with_sia --param beta=0.5 --seed 42
```

### 4.3 Parameter Flags

```bash
--params FILE              # load parameter file (can repeat for layering)
--param NAME=VALUE         # override a single parameter (can repeat)
--param-vec PREFIX=FILE    # override indexed params from keyed TSV
--table NAME=FILE          # supply external() table data
```

### 4.4 Synthetic Observations

```bash
# Generate synthetic observations from the observations block
camdl simulate model.camdl --params p.toml --seed 42 --obs

# Multiple independent replicates
camdl simulate model.camdl --params p.toml --seed 42 --replicates 100 --obs

# Suppress trajectory, only emit observations (SBC workflows)
camdl simulate model.camdl --seed 42 --replicates 1000 --obs-only
```

The observation RNG is independent of the process RNG — adding `--obs` does not
change the trajectory.

---

## 5. Batch Simulation

### 5.1 CLI Invocations

```bash
# ── Multiple scenarios × seeds ───────────────────────────
camdl simulate model.camdl --params p.toml \
    --scenario baseline,with_sia --seeds 1:1000

# ── 1D sweep ─────────────────────────────────────────────
camdl simulate model.camdl --params p.toml \
    --sweep "R0=1,1.5,2,2.5,3,3.5,4,4.5,5" --seeds 1:100

# ── 2D sweep (Cartesian product: 5 × 3 = 15 points) ─────
camdl simulate model.camdl --params p.toml \
    --sweep "R0=1,2,3,4,5" --sweep "gamma=0.05,0.1,0.5" \
    --seeds 1:100

# ── Posterior predictive ─────────────────────────────────
camdl simulate model.camdl \
    --draws results/fits/02_fix_beta/posterior/draws.tsv \
    --replicates 10 --obs

# ── Prior predictive (requires declared priors) ──────────
camdl simulate model.camdl \
    --draws prior --fit fits/02_fix_beta.toml --n 500 \
    --replicates 5 --obs

# ── Uniform space-filling (no Bayesian pretension) ───────
camdl simulate model.camdl \
    --draws uniform --n 500 --replicates 5 --obs

# ── Scenario prediction under posterior uncertainty ──────
camdl simulate model.camdl \
    --draws results/fits/02_fix_beta/posterior/draws.tsv \
    --scenario baseline,with_sia --replicates 10 --obs

# ── From a batch file ────────────────────────────────────
camdl simulate --batch batches/ppc.toml
```

**CLI `--sweep` accepts comma-separated lists only.** Generators (`linspace`,
`logspace`, `range`) are available only in batch TOML `[sweep]` sections.
This keeps the CLI syntax obvious and discoverable — if you need structured
generators, write a batch file.

### 5.2 CLI ↔ Type Mapping

```rust
#[derive(Parser)]
pub struct SimulateCli {
    pub model: Option<PathBuf>,

    #[arg(long)]
    pub params: Option<PathBuf>,

    #[arg(long)]
    pub backend: Option<Backend>,

    #[arg(long)]
    pub dt: Option<f64>,

    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    // ── ParamSource::Point ──
    #[arg(long = "param", value_parser = parse_kv)]
    pub param_overrides: Vec<ParamOverride>,

    // ── ParamSource::Sweep ──
    #[arg(long = "sweep", value_parser = parse_sweep_arg)]
    pub sweeps: Vec<(String, SweepSpec)>,

    // ── ParamSource::Draws ──
    #[arg(long)]
    pub draws: Option<DrawsArg>,

    #[arg(long)]
    pub fit: Option<PathBuf>,       // for --draws prior

    #[arg(long, short = 'n')]
    pub n_draws: Option<usize>,     // for --draws prior/uniform

    #[arg(long)]
    pub replicates: Option<usize>,

    // ── σ layer ──
    #[arg(long, value_delimiter = ',')]
    pub scenario: Vec<String>,

    #[arg(long)]
    pub enable: Vec<String>,

    #[arg(long)]
    pub disable: Vec<String>,

    // ── S layer ──
    #[arg(long)]
    pub seed: Option<u64>,

    #[arg(long, value_parser = parse_seeds)]
    pub seeds: Option<Seeds>,

    // ── Observation generation ──
    #[arg(long)]
    pub obs: bool,

    #[arg(long)]
    pub parallel: Option<usize>,

    // ── Batch file (alternative entry point) ──
    #[arg(long)]
    pub batch: Option<PathBuf>,
}

/// --draws argument on CLI
#[derive(Debug, Clone)]
pub enum DrawsArg {
    File(PathBuf),
    Prior,
    Uniform,
}

impl SimulateCli {
    /// Resolve CLI args into the canonical SimulateJob.
    /// This is the single convergence point: from here on,
    /// the runner doesn't know whether input came from CLI or file.
    pub fn into_job(self) -> Result<SimulateJob> {
        if let Some(batch_path) = self.batch {
            return SimulateJob::from_toml(&batch_path);
        }

        let source = self.resolve_param_source()?;
        let scenarios = self.resolve_scenarios()?;
        let seeds = self.seed.map(Seeds::Single)
            .or(self.seeds)
            .unwrap_or_default();

        Ok(SimulateJob {
            model: self.model
                .ok_or_else(|| anyhow!("model path required"))?,
            params: self.params,
            backend: self.backend.unwrap_or(Backend::Gillespie),
            dt: self.dt.unwrap_or(1.0),
            output_dir: self.output_dir
                .unwrap_or_else(|| PathBuf::from("results")),
            source, scenarios, seeds,
            obs: self.obs,
            parallel: self.parallel.unwrap_or(1),
            geo: None,
        })
    }
}
```

### 5.3 Total Runs Calculation

```
Total runs = |param_points| × |scenarios| × |seeds|

where |param_points| =
  Point:   1
  Sweep:   product of |sweep_i.expand()| for each swept parameter
  Draws:   n_draws × replicates

where |scenarios| =
  empty (no scenarios specified):  1 (implicit baseline)
  otherwise:                       number of [[scenario]] entries
```

### 5.4 Scenario × Sweep Interaction

Sweeps and scenarios are orthogonal. Their cross product defines the full run
grid. Each (sweep_point, scenario) combination produces one effective
configuration. Sweep point overrides apply first (M layer), then scenario
params overlay on top (σ layer).

### 5.5 Batch TOML Examples

```toml
# batches/scenario_comparison.toml
model = "models/sir.camdl"
params = "params/baseline.toml"
backend = "chain_binomial"
dt = 1.0
seeds = { n = 1000 }
output_dir = "results"
parallel = 16

[[scenario]]
name = "baseline"

[[scenario]]
name = "with_sia"
enable = ["sia"]

[[scenario]]
name = "high_coverage"
enable = ["sia"]
params = { sia_cov = 0.95 }

# Total runs: 3 scenarios × 1000 seeds = 3000
```

```toml
# batches/r0_gamma_sweep.toml — 2D Cartesian product
model = "models/sir.camdl"
params = "params/baseline.toml"
seeds = { n = 50 }

[sweep]
R0 = { linspace = { min = 1.0, max = 5.0, n = 9 } }
gamma = { logspace = { min = 0.01, max = 1.0, n = 5 } }
# 9 × 5 = 45 parameter points × 50 seeds = 2250 runs
```

```toml
# batches/ppc.toml — posterior predictive check
model = "models/sir.camdl"
obs = true

[draws]
source = "file"
file = "results/fits/02_fix_beta/posterior/draws.tsv"
replicates = 10
```

```toml
# batches/policy_eval.toml — scenario prediction under uncertainty
model = "models/sir.camdl"
obs = true
seeds = { n = 10 }

[draws]
source = "file"
file = "results/fits/02_fix_beta/posterior/draws.tsv"
replicates = 1

[[scenario]]
name = "baseline"

[[scenario]]
name = "with_sia"
enable = ["sia"]
# Total: n_draws × 2 scenarios × 10 seeds
# Scenarios are EKRNG-coupled within each (draw, seed) pair
```

### 5.6 Execution Flow

The simulation runner:

1. Compiles the `.camdl` model once (or loads `.ir.json` directly)
2. Loads base params if specified
3. Generates the run grid (sweep/draws × scenarios × seeds)
4. Classifies cache hits vs new runs (check for `traj.tsv` at computed path)
5. Executes new runs with Rayon `par_iter`
6. Writes `traj.tsv` and `run.json` per run
7. Writes `manifest.json` at the output root
8. Copies `model.ir.json` and optional `geo/` to output root

---

## 6. FitConfig — the inference type

### 6.1 Overview

A fit.toml specifies a single inference task: which model to fit, what data to
fit it to, which parameters to estimate vs hold fixed, and what inference
algorithm to run. It defines a *view* of the parameter space — the partition
of M into free parameters (explored by the algorithm) and fixed parameters
(held constant). The algorithm then operates in the reduced space of free
parameters. (See Buffalo 2026 for the formal treatment of parameter views,
transforms, and the downward chain from inference coordinates to simulator
output.)

### 6.2 Structure

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitConfig {
    pub model: ModelRef,
    pub data: DataSpec,
    pub output_dir: Option<PathBuf>,

    /// The free parameters: what the inference algorithm estimates.
    pub estimate: IndexMap<String, EstimateSpec>,

    /// The fixed parameters: held constant during inference.
    /// estimate ∪ fixed must cover all model parameters.
    pub fixed: FixedParams,

    /// Inference pipeline stages, executed in declaration order.
    pub stages: IndexMap<String, Stage>,

    /// Optional lineage metadata (not used by the runner).
    pub provenance: Option<FitProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    pub camdl: PathBuf,
}

/// Data file mapping. Keys match observation stream names declared in
/// the .camdl file's observations { } block. The observation model
/// (likelihood family) and projection (which flow/compartment to
/// accumulate) are defined in the .camdl file — fit.toml only provides
/// the data file paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSpec {
    /// Map from observation stream name → data file path.
    /// Keys must match names in the .camdl observations { } block.
    pub observations: IndexMap<String, PathBuf>,

    /// Time threshold for temporal holdout: observations at t > this
    /// value are withheld from training and used for out-of-sample
    /// evaluation. In model time units. Mutually exclusive with
    /// `holdout`.
    pub holdout_after: Option<f64>,

    /// Explicit holdout data files (for non-standard schemes like
    /// spatial leave-one-out). Keys match observation stream names.
    /// Mutually exclusive with `holdout_after`.
    pub holdout: Option<IndexMap<String, PathBuf>>,
}
```

### 6.3 EstimateSpec — free parameters

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimateSpec {
    pub bounds: (f64, f64),

    /// Transform for inference. If omitted, inferred from the parameter's
    /// declared type in the .camdl file: rate → log, probability → logit,
    /// positive → log, real → identity.
    pub transform: Option<Transform>,

    /// Prior distribution. Required for Bayesian methods (PGAS, PMMH).
    /// Optional for MLE (IF2 ignores priors).
    /// Also used by --draws prior for prior predictive checks.
    pub prior: Option<PriorSpec>,

    /// Initial value parameter: perturbed only at t=0 in IF2
    #[serde(default)]
    pub ivp: bool,

    /// Per-parameter random walk SD for IF2.
    /// If omitted, auto-scaled from bounds.
    pub rw_sd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Transform {
    #[serde(rename = "log")]
    Log,
    #[serde(rename = "logit")]
    Logit,
    #[serde(rename = "identity")]
    Identity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "dist")]
pub enum PriorSpec {
    #[serde(rename = "log_normal")]
    LogNormal { mu: f64, sigma: f64 },
    #[serde(rename = "normal")]
    Normal { mu: f64, sigma: f64 },
    #[serde(rename = "beta")]
    Beta { alpha: f64, beta: f64 },
    #[serde(rename = "uniform")]
    Uniform,
    #[serde(rename = "half_normal")]
    HalfNormal { sigma: f64 },
}
```

### 6.4 FixedParams

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixedParams {
    /// Bulk load from a TOML file.
    pub from_file: Option<PathBuf>,

    /// Inline fixed values. Override from_file if both specify a key.
    #[serde(flatten)]
    pub values: IndexMap<String, f64>,
}

impl FixedParams {
    pub fn resolve(&self) -> Result<IndexMap<String, f64>> {
        let mut merged = match &self.from_file {
            Some(path) => load_params_toml(path)?,
            None => IndexMap::new(),
        };
        for (k, v) in &self.values {
            merged.insert(k.clone(), *v);
        }
        Ok(merged)
    }
}
```

### 6.5 Stage — inference pipeline steps

Stages are the verbs of inference: optimize (find the MLE), sample (draw from
the posterior), evaluate (assess fit quality). Each stage runs a specific
algorithm. Stages execute in declaration order; the `starts_from` field
creates dependency edges between them.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum Stage {
    #[serde(rename = "if2")]
    IF2 {
        chains: usize,
        particles: usize,
        iterations: usize,
        cooling: CoolingSpec,
        #[serde(default)]
        starts_from: StartsFrom,
    },

    #[serde(rename = "pgas")]
    PGAS {
        chains: usize,
        particles: usize,
        sweeps: usize,
        #[serde(default)]
        starts_from: StartsFrom,
        #[serde(default)]
        skip_chains: Vec<usize>,
    },

    #[serde(rename = "pmmh")]
    PMMH {
        chains: usize,
        particles: usize,
        iterations: usize,
        #[serde(default)]
        starts_from: StartsFrom,
        #[serde(default)]
        skip_chains: Vec<usize>,
    },

    #[serde(rename = "pfilter")]
    PFilter {
        particles: usize,
        replicates: Option<usize>,
        #[serde(default)]
        starts_from: StartsFrom,
    },
}

impl Stage {
    pub fn starts_from(&self) -> &StartsFrom {
        match self {
            Stage::IF2 { starts_from, .. }
            | Stage::PGAS { starts_from, .. }
            | Stage::PMMH { starts_from, .. }
            | Stage::PFilter { starts_from, .. } => starts_from,
        }
    }

    pub fn requires_priors(&self) -> bool {
        matches!(self, Stage::PGAS { .. } | Stage::PMMH { .. })
    }
}
```

### 6.6 StartsFrom — dependency edges

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StartsFrom {
    /// Name of a previous stage in this fit.toml
    Stage(String),

    /// Path to an external results directory
    Directory(PathBuf),

    /// Random starts from parameter bounds (default)
    #[serde(rename = "random")]
    Random,
}

impl Default for StartsFrom {
    fn default() -> Self { StartsFrom::Random }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CoolingSpec {
    /// Fraction of initial perturbation magnitude remaining at the final
    /// iteration. 0.70 means perturbations shrink to 70% of their starting
    /// scale over the full run. Lower values = more aggressive cooling
    /// (better for exploration/scout); higher values = gentler cooling
    /// (better for refinement near an optimum).
    Fixed(f64),
    #[serde(rename = "auto")]
    Auto,
}
```

### 6.7 FitProvenance — lineage metadata

```rust
/// Optional metadata linking this fit to a parent.
/// Not used by the runner — purely for human navigation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitProvenance {
    pub derived_from: Option<PathBuf>,
    pub reason: Option<String>,
}
```

### 6.8 Output Path Methods

```rust
impl FitConfig {
    pub fn fit_dir(&self, config_path: &Path) -> PathBuf {
        let name = config_path.file_stem().unwrap().to_str().unwrap();
        let output_root = self.output_dir.clone()
            .unwrap_or_else(|| PathBuf::from("results"));
        output_root.join("fits").join(name)
    }

    pub fn stage_dir(&self, config_path: &Path, stage_name: &str) -> PathBuf {
        self.fit_dir(config_path).join(stage_name)
    }

    pub fn swept_stage_dir(
        &self, config_path: &Path, sweep_point: &SweepPoint, stage_name: &str,
    ) -> PathBuf {
        self.fit_dir(config_path)
            .join(sweep_point_slug(sweep_point))
            .join(stage_name)
    }

    pub fn draws_path(&self, config_path: &Path, stage_name: &str) -> PathBuf {
        self.stage_dir(config_path, stage_name).join("draws.tsv")
    }

    pub fn mle_params_path(&self, config_path: &Path, stage_name: &str) -> PathBuf {
        self.stage_dir(config_path, stage_name).join("mle_params.toml")
    }
}

fn sweep_point_slug(point: &SweepPoint) -> String {
    point.iter()
        .map(|(k, v)| format!("{}_{:.3}", k, v))
        .collect::<Vec<_>>()
        .join("__")
}
```

### 6.9 Completeness Validation

The runner calls this at load time, before any stage executes. This enforces
camdl's "no silent defaults" rule.

```rust
impl FitConfig {
    pub fn validate(&self, ir: &CompiledModel) -> Result<()> {
        let model_params: BTreeSet<&str> = ir.parameters.keys()
            .map(|s| s.as_str()).collect();
        let estimated: BTreeSet<&str> = self.estimate.keys()
            .map(|s| s.as_str()).collect();
        let fixed_resolved = self.fixed.resolve()?;
        let fixed: BTreeSet<&str> = fixed_resolved.keys()
            .map(|s| s.as_str()).collect();

        // estimate ∩ fixed = ∅
        let overlap: Vec<_> = estimated.intersection(&fixed).collect();
        if !overlap.is_empty() {
            bail!("parameters in both [estimate] and [fixed]: {}\n  \
                   Each parameter must be in exactly one section.",
                  overlap.iter().join(", "));
        }

        // estimate ∪ fixed = model_params
        let covered: BTreeSet<_> = estimated.union(&fixed).cloned().collect();
        let missing: Vec<_> = model_params.difference(&covered).collect();
        if !missing.is_empty() {
            bail!("parameters neither estimated nor fixed: {}\n  \
                   Every model parameter must appear in [estimate] or [fixed].",
                  missing.iter().join(", "));
        }

        let extra: Vec<_> = covered.difference(&model_params).collect();
        if !extra.is_empty() {
            bail!("parameters not in model: {}", extra.iter().join(", "));
        }

        // Bayesian stages require priors
        for (name, stage) in &self.stages {
            if stage.requires_priors() {
                let missing_priors: Vec<_> = self.estimate.iter()
                    .filter(|(_, spec)| spec.prior.is_none())
                    .map(|(name, _)| name.as_str())
                    .collect();
                if !missing_priors.is_empty() {
                    bail!("stage '{}' requires priors, but missing for: {}",
                          name, missing_priors.join(", "));
                }
            }
        }

        self.validate_stage_dag()?;
        Ok(())
    }
}
```

---

## 7. The Fit CLI

### 7.1 Invocations

```bash
camdl fit run fits/01_all_free.toml
camdl fit run fits/01_all_free.toml --stage mle
camdl fit run fits/02_fix_beta.toml --stage posterior \
    --starts-from results/fits/01_all_free/mle
camdl fit run fits/base.toml --sweep "rho=0.5,0.1,0.02,0.005"
camdl fit run fits/base.toml --sweep "rho=0.5,0.1" --sweep "k=5,10,20"
camdl fit run fits/01.toml --stage posterior --skip-chains 2,4
camdl fit run fits/01_all_free.toml --force
camdl fit status fits/
camdl fit diff fits/01_all_free.toml fits/02_fix_beta.toml
camdl fit new --from fits/01_all_free.toml fits/02_fix_beta.toml
```

### 7.2 CLI Type

```rust
#[derive(Parser)]
pub struct FitRunCli {
    pub config: PathBuf,

    #[arg(long)]
    pub stage: Option<String>,

    #[arg(long)]
    pub starts_from: Option<PathBuf>,

    #[arg(long)]
    pub seed: Option<u64>,

    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    #[arg(long)]
    pub force: bool,

    /// Resume a partially completed sampling stage (PGAS/PMMH).
    /// Continues from the last completed sweep/iteration.
    #[arg(long)]
    pub resume: bool,

    #[arg(long = "sweep", value_parser = parse_sweep_arg)]
    pub sweeps: Vec<(String, SweepSpec)>,

    #[arg(long = "skip-chains", value_delimiter = ',')]
    pub skip_chains: Vec<usize>,

    #[arg(long)]
    pub parallel: Option<usize>,
}
```

**Seeds and fits.** Unlike `SimulateJob`, `FitConfig` has no `seeds` field. A
fit uses a single base seed (default: 1, override with `--seed N`). Chain seeds
are derived deterministically as `base_seed + chain_index`. If you want multiple
independent fits at different seeds, run the command multiple times with
different `--seed` values.

### 7.3 Sweep Semantics for Fits

The `--sweep` flag on `camdl fit run` overrides a parameter in `[fixed]` at
each grid point. The swept parameter must be in `[fixed]`, not `[estimate]` —
sweeping an estimated parameter is a type error.

This means a parameter naturally *promotes* from fixed to swept with zero
config changes:

```toml
[fixed]
rho = 0.5
```

```bash
camdl fit run fit.toml --sweep "rho=0.5,0.1,0.02"
```

Each sweep point runs the full pipeline independently.

---

## 8. fit.toml Examples

### 8.1 MLE with IF2

```toml
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[estimate]
beta  = { bounds = [0.01, 2.0] }
gamma = { bounds = [0.05, 1.0] }
rho   = { bounds = [0.001, 1.0] }
k     = { bounds = [0.1, 100.0] }

[fixed]
N0 = 1000000
I0 = 10

[stages.mle]
method = "if2"
chains = 8
particles = 1000
iterations = 80
cooling = 0.70
```

### 8.2 MLE + Posterior Sampling

```toml
[provenance]
derived_from = "fits/01_all_free.toml"
reason = "beta mixing poor in PGAS (ESS < 50), fixing at MLE"

[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[estimate]
gamma = { bounds = [0.05, 1.0], prior = { dist = "log_normal", mu = -2.0, sigma = 1.0 } }
rho   = { bounds = [0.001, 1.0], prior = { dist = "beta", alpha = 2.0, beta = 5.0 } }
k     = { bounds = [0.1, 100.0], prior = { dist = "half_normal", sigma = 10.0 } }

[fixed]
beta = 0.34
N0 = 1000000
I0 = 10

[stages.mle]
method = "if2"
chains = 4
particles = 2000
iterations = 60
cooling = 0.95
starts_from = "results/fits/01_all_free/mle"

[stages.posterior]
method = "pgas"
chains = 4
particles = 50
sweeps = 5000
starts_from = "mle"

[stages.evaluate]
method = "pfilter"
particles = 10000
replicates = 100
starts_from = "mle"
```

### 8.3 Large Model with from_file

```toml
[model]
camdl = "models/seir_nigeria.camdl"

[data]
holdout_after = 5474

[data.observations]
weekly_cases = "data/nigeria_afp.tsv"

[estimate]
beta       = { bounds = [0.01, 5.0], prior = { dist = "log_normal", mu = 0.0, sigma = 1.0 } }
sigma      = { bounds = [0.05, 1.0] }
gamma      = { bounds = [0.05, 1.0] }
rho        = { bounds = [0.001, 0.5], prior = { dist = "beta", alpha = 2.0, beta = 10.0 } }
k          = { bounds = [0.1, 100.0] }
alpha      = { bounds = [0.0, 1.0] }
phi_season = { bounds = [0.0, 365.25] }
import_rate = { bounds = [0.0001, 1.0] }

[fixed]
from_file = "params/nigeria_fixed.toml"
vacc_frac = 0.80

[stages.mle]
method = "if2"
chains = 8
particles = 2000
iterations = 100
cooling = 0.70

[stages.posterior]
method = "pgas"
chains = 6
particles = 100
sweeps = 10000
starts_from = "mle"
```

---

## 9. Provenance and Cache Invalidation

### 9.1 The Hybrid Strategy

Fit results use **named directories** for human readability and **hash-based
staleness detection** for cache invalidation. Simulation results use
**content-addressed directories** where the hash IS the directory name.

### 9.2 Simulation Hash Computation

**sim_hash** — model + base params + backend + dt:

```
sim_hash = sha256(
    model_ir_json_bytes              # compiled model (deterministic)
    + canonical_sorted(base_param key=value pairs)
    + backend_string                 # "gillespie", "tau_leap", etc.
    + dt_bytes                       # f64 little-endian
    + camdl_version_string
)
```

Full 64-character hex string; first 8 characters used in directory name.

**scen_hash** — scenario delta only:

```
scen_hash = sha256(
    sorted(enable list)
    + sorted(disable list)
    + canonical_sorted(scenario param overrides)
    + canonical_sorted(sweep/design point overrides)
)
```

`scen_hash` covers only the *delta*. Base params are already in `sim_hash`.
Renaming a scenario without changing its definition preserves `scen_hash` and
reuses cached runs. Sweep point values are included in `scen_hash` because
they affect the simulation at that grid coordinate.

**Canonical sorting** means: sort parameter key-value pairs lexicographically
by key name, then serialize each as `key=value` with full-precision float
formatting. This ensures hash stability across HashMap iteration order.

### 9.3 Simulation Cache Rules

A simulation run is a **cache hit** when
`{sim_hash_8}/{scenario_slug}-{scen_hash_8}/seed_{N}/traj.tsv` exists.

| What changed               | sim_hash  | scen_hash         | Reuse               |
| -------------------------- | --------- | ----------------- | -------------------- |
| Model / base params        | changes   | —                 | none                 |
| Backend or dt              | changes   | —                 | none                 |
| Scenario A's overrides     | unchanged | A changes, B same | B's runs reused      |
| Sweep point values         | unchanged | affected only     | other points reused  |
| Add more seeds             | unchanged | unchanged         | all existing reused  |
| Rename a scenario          | unchanged | unchanged         | reused (same delta)  |
| camdl version              | changes   | —                 | none                 |

### 9.4 Fit Staleness Detection

For fits, there are no hash-based directory names. Instead, each stage writes
a `config_hash` into its `provenance.json`. On re-run:

1. The runner computes the current `config_hash` from: model IR hash, data
   file hash, the full `[estimate]` spec, the resolved `[fixed]` values, the
   stage's algorithm settings, and the camdl version.

2. If `provenance.json` exists and its `config_hash` matches → cache hit, skip.

3. If `provenance.json` exists but the hash differs → **error**:

```
error: stage 'mle' has stale results (config changed since last run)
  stored config_hash: a1b3c4d5
  current config_hash: f9e2b047
  Changes detected:
    [estimate.beta] bounds: [0.01, 2.0] → [0.01, 5.0]
    [stages.mle] cooling: 0.70 → 0.95
  Options:
    --force    overwrite existing results
    camdl fit new --from fits/01.toml fits/01_v2.toml
```

4. `--force` overwrites without error.

```rust
pub struct ConfigHasher;

impl ConfigHasher {
    pub fn compute(
        ir: &CompiledModel,
        data_path: &Path,
        estimate: &IndexMap<String, EstimateSpec>,
        fixed: &IndexMap<String, f64>,
        stage_name: &str,
        stage: &Stage,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(ir.to_canonical_bytes());
        hasher.update(&hash_file(data_path));
        for (name, spec) in estimate.iter() {
            hasher.update(name.as_bytes());
            hasher.update(&serde_json::to_vec(spec).unwrap());
        }
        for (name, val) in fixed.iter() {
            hasher.update(name.as_bytes());
            hasher.update(&val.to_le_bytes());
        }
        hasher.update(stage_name.as_bytes());
        hasher.update(&serde_json::to_vec(stage).unwrap());
        hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
        hex::encode(hasher.finalize())
    }
}
```

### 9.5 provenance.json — per-stage output (fits)

```json
{
  "camdl_version": "0.7.0",
  "timestamp": "2026-04-15T10:30:00Z",
  "config_hash": "a1b3c4d5e6f7...",
  "fit_config": "fits/02_fix_beta.toml",
  "fit_config_hash": "b2c3d4e5...",
  "stage": "posterior",
  "model": "models/sir.camdl",
  "model_hash": "3a7f2c1d...",
  "data_hash": "f9e2b047...",
  "estimated": ["gamma", "rho", "k"],
  "fixed": {
    "beta": 0.34,
    "N0": 1000000,
    "I0": 10
  },
  "algorithm": {
    "method": "pgas",
    "chains": 4,
    "particles": 50,
    "sweeps": 5000
  },
  "starts_from": {
    "source": "results/fits/02_fix_beta/mle",
    "source_hash": "e8f1a2b3..."
  },
  "derived_from": "fits/01_all_free.toml",
  "wall_time_seconds": 3847.2
}
```

### 9.6 run.json — per-run metadata (simulations)

```json
{
  "sim_hash": "3a7f2c1d...",
  "scen_hash": "f9e2b047...",
  "scenario": "with_sia",
  "seed": 42,
  "model_hash": "...",
  "camdl_version": "0.7.0",
  "backend": "chain_binomial",
  "dt": 1.0
}
```

For sweep runs, sweep point values are included:

```json
{
  "sim_hash": "3a7f2c1d...",
  "scen_hash": "a3c1e890...",
  "scenario": "with_sia",
  "seed": 1,
  "sweep_point": { "vacc_eff": 0.5 }
}
```

### 9.7 manifest.json — simulation batch index

```json
{
  "model": "models/seir_nigeria.camdl",
  "scenarios": ["baseline", "with_sia"],
  "seeds": [1, 2, 3],
  "total_runs": 6,
  "completed": 6,
  "output_dir": "results",
  "runs": [
    {
      "scenario": "with_sia",
      "seed": 1,
      "run_path": "3a7f2c1d/with_sia-f9e2b047/seed_1"
    }
  ]
}
```

---

## 10. Output File Schemas

### 10.1 Trajectories (traj.tsv)

One row per output time. First line is a version comment.

```
#camdl traj v1
t	S	E	I	R	flow_infection	flow_recovery
0.0	999990	0	10	0	0	0
1.0	999985	3	11	1	5	1
2.0	999978	5	14	3	7	3
```

Column types:

- `t`: float64 (simulation time)
- Compartment columns: int64 for integer compartments, float64 for `real`
- `flow_*` columns: uint64, cumulative firings since previous output time

### 10.2 Summary Tables

For batch runs, `camdl summarize` reads trajectory files and computes:

```tsv
scenario	seed	peak_I	tpeak_I	final_I	integral_I	...
baseline	1	342	45.0	0	12847	...
with_sia	1	218	52.0	0	8934	...
```

Summary statistics computed automatically for every non-time column:

| Statistic     | Definition                        |
| ------------- | --------------------------------- |
| `peak_X`      | Maximum value of X                |
| `tpeak_X`     | Time of peak (first occurrence)   |
| `final_X`     | Last value of X                   |
| `integral_X`  | Sum of X across all output times  |

### 10.3 draws.tsv — Complete Parameter Vectors

Posterior draws output contains ALL model parameters, not just the estimated
subset. Fixed parameters are constant across rows:

```tsv
gamma	rho	k	beta	N0	I0
0.098	0.042	8.3	0.34	1000000	10
0.102	0.039	9.1	0.34	1000000	10
0.095	0.045	7.8	0.34	1000000	10
```

This makes posterior predictive checks self-contained — no `--params` needed.
The draws file IS the complete parameter specification. You can see what was
estimated vs fixed by inspecting column variance.

Written by the runner at the end of sampling stages:

```rust
impl FitConfig {
    pub fn write_complete_draws(
        &self,
        stage_draws: &[IndexMap<String, f64>],
        output_path: &Path,
    ) -> Result<()> {
        let fixed = self.fixed.resolve()?;
        let mut writer = csv::WriterBuilder::new()
            .delimiter(b'\t')
            .from_path(output_path)?;

        let all_names: Vec<&str> = self.estimate.keys()
            .chain(fixed.keys())
            .map(|s| s.as_str())
            .collect();
        writer.write_record(&all_names)?;

        for draw in stage_draws {
            let row: Vec<String> = all_names.iter()
                .map(|name| {
                    draw.get(*name)
                        .or_else(|| fixed.get(*name))
                        .map(|v| format!("{}", v))
                        .unwrap_or_default()
                })
                .collect();
            writer.write_record(&row)?;
        }
        Ok(())
    }
}
```

---

## 11. Predictive Workflows

### 11.1 Prior Predictive Check

*"Does my model, under priors, generate data that looks plausible?"*

```bash
camdl simulate models/sir.camdl \
    --draws prior --fit fits/02_fix_beta.toml --n 500 \
    --replicates 5 --obs
```

Samples 500 parameter vectors from the joint prior declared in fit.toml's
`[estimate]` block. Fixed parameters are filled in from `[fixed]`. Runs 5
stochastic replicates per draw. Generates synthetic observations.

**Requires priors.** If any estimated parameter lacks a `prior` field:

```
error: --draws prior requires priors for all estimated parameters.
  Missing priors: beta, k
  Add prior = { dist = "...", ... } to [estimate.beta] and [estimate.k]
```

### 11.2 Posterior Predictive Check

*"Does my fitted model generate data that looks like the real data?"*

```bash
camdl simulate models/sir.camdl \
    --draws results/fits/02_fix_beta/posterior/draws.tsv \
    --replicates 10 --obs
```

### 11.3 Scenario Prediction Under Posterior Uncertainty

*"What would happen under an SIA, given what we learned from the data?"*

```bash
camdl simulate models/sir.camdl \
    --draws results/fits/02_fix_beta/posterior/draws.tsv \
    --scenario baseline,with_sia \
    --replicates 10 --obs
```

For each (draw, seed) pair, both scenarios are simulated with the same EKRNG
state, producing coupled counterfactual trajectories. This enables paired
comparisons (cases_averted = baseline - with_sia) that properly propagate
posterior uncertainty.

### 11.4 Uniform Exploration

*"What does the model do across parameter space?"*

```bash
camdl simulate models/sir.camdl \
    --draws uniform --n 500 --replicates 1
```

Samples uniformly from parameter bounds. No Bayesian pretension — this is
space-filling exploration for model debugging.

### 11.5 Simulation-Based Calibration (SBC)

*"Does my inference pipeline recover known parameters?"*

```bash
# Step 1: Generate synthetic datasets at known parameters
camdl simulate models/sir.camdl \
    --draws prior --fit fits/02.toml --n 200 \
    --replicates 1 --obs --output-dir results/sbc/synthetic

# Step 2: Fit each (shell loop for now)
for dir in results/sbc/synthetic/*/; do
    camdl fit run fits/sbc_template.toml \
        --override-data "$dir/synthetic.tsv" \
        --output-dir "results/sbc/fits/$(basename $dir)"
done
```

---

## 12. Priors: Where They Live

Priors are declared in fit.toml's `[estimate]` block:

```toml
[estimate]
gamma = { bounds = [0.05, 1.0], prior = { dist = "log_normal", mu = -2.0, sigma = 1.0 } }
rho   = { bounds = [0.001, 1.0], prior = { dist = "beta", alpha = 2.0, beta = 5.0 } }
```

**Why not in the model file?** Priors are analysis choices that vary between
fits. Different researchers may use different priors for the same model.
Following camdl's separation of concerns, the `.camdl` file defines structure;
fit.toml defines the analysis.

**When are priors required?** Bayesian sampling methods (PGAS, PMMH) use the
prior density in their acceptance ratios. MLE methods (IF2) ignore priors.
The runner validates at load time.

**Priors do double duty.** The same prior declarations are used both by the
inference algorithm and by `--draws prior` for prior predictive checks.

---

## 13. Utility Commands

### 13.1 `camdl fit status`

```
$ camdl fit status fits/

fits/01_all_free.toml
  estimate: beta, gamma, rho, k
  fixed:    N0, I0
  stages:
    mle        [done]  8 chains, best loglik = -342.1
    posterior   —

fits/02_fix_beta.toml  (derived from: fits/01_all_free.toml)
  estimate: gamma, rho, k
  fixed:    beta=0.34, N0, I0
  stages:
    mle        [done]  4 chains, best loglik = -340.8
    posterior  [done]  4 chains, 5000 sweeps, ESS: γ=312 ρ=189 k=445
    evaluate   [done]  10000 particles, loglik = -341.2 ± 0.8
```

### 13.2 `camdl fit diff`

```
$ camdl fit diff fits/01_all_free.toml fits/02_fix_beta.toml

Parameter changes:
  beta:  [estimate] bounds=[0.01, 2.0]  →  [fixed] 0.34

Stage changes:
  mle:       chains 8→4, particles 1000→2000, cooling 0.70→0.95
  posterior:  (new) pgas, 4 chains, 50 particles, 5000 sweeps
  evaluate:   (new) pfilter, 10000 particles, 100 replicates
```

### 13.3 `camdl fit new`

```
$ camdl fit new --from fits/01_all_free.toml fits/02_fix_beta.toml

Created fits/02_fix_beta.toml
  [provenance] derived_from = "fits/01_all_free.toml"
  [stages.mle] starts_from = "results/fits/01_all_free/mle"
```

### 13.4 `camdl summarize`

```bash
camdl summarize results/simulate/
```

Reads trajectory files and produces per-scenario summary tables with
automatically computed statistics (peak, time-of-peak, final value, integral)
for every non-time column. Output written to `results/simulate/summary/`.

---

## Appendix A: CLI Reference

```
camdl simulate MODEL [OPTIONS]
  --params FILE             Load parameter values (repeatable)
  --param NAME=VALUE        Override single parameter (repeatable)
  --param-vec PREFIX=FILE   Override indexed params from keyed TSV
  --table NAME=FILE         Supply external table data
  --backend BACKEND         gillespie|tau_leap|chain_binomial|ode
  --dt DT                   Step size for discrete-time backends
  --seed N                  Single seed
  --seeds SPEC              Multiple seeds: "1:1000", "{n=100}", "{list=[42,137]}"
  --scenario NAME[,NAME]    Named scenarios (comma-separated)
  --enable NAME             Enable intervention (mutually exclusive with --scenario)
  --disable NAME            Disable intervention
  --sweep "NAME=V1,V2,..."  Parameter sweep, list syntax (repeatable; Cartesian product)
  --draws SOURCE            "path.tsv" | "prior" | "uniform"
  --fit FILE                fit.toml for --draws prior
  -n N                      Number of draws (prior/uniform)
  --replicates N            Stochastic replicates per draw
  --obs                     Generate synthetic observations
  --obs-only                Suppress trajectory, only emit observations
  --parallel N              Concurrent runs
  --output-dir DIR          Output root (default: results/)
  --batch FILE              Load all settings from TOML
  --force                   Re-run cached results

camdl fit run CONFIG [OPTIONS]
  --stage NAME              Run specific stage only
  --starts-from DIR         Override starts_from for target stage
  --seed N                  RNG seed override
  --sweep "NAME=V1,V2,..."  Sweep over fixed params (repeatable; Cartesian product)
  --skip-chains N[,N]       Skip specific chain indices
  --resume                  Resume partially completed sampling stage
  --parallel N              Concurrent sweep points
  --output-dir DIR          Output root override
  --force                   Re-run (overwrite stale results)

camdl fit status [DIR]         Show pipeline status and lineage
camdl fit diff A.toml B.toml   Show differences between fit configs
camdl fit new --from A B       Create derived fit config with lineage
camdl summarize DIR            Compute summary statistics from trajectories

camdl simulate --batch FILE    Equivalent to camdl experiment run (deprecated)
```

---

## Appendix B: Parameter Files

### B.1 params.toml — a point m ∈ M

```toml
beta = 0.3
gamma = 0.1
sigma = 0.2
rho = 0.4
k = 5.0
N0 = 1000000
I0 = 10
```

One key-value pair per declared parameter. Used by `camdl simulate` for
forward simulation and by fit.toml's `[fixed] from_file` for bulk fixed
values.

### B.2 Indexed Parameter Overrides

For spatial models with indexed parameters (`R0[patch]`), use `--param-vec`:

```tsv
R0_kano     2.1
R0_lagos    1.8
R0_sokoto   2.4
```

```bash
camdl simulate model.camdl --params p.toml --param-vec R0=r0_init.tsv
```

Matched by parameter name (not position).
