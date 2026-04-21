pub mod types;

use std::path::PathBuf;
use clap::{Parser, Subcommand, Args};
use types::{Backend, ListDuration, ParamOverride, ParamVecSpec, RwSd, SeedSpec, SweepSpec, TableSpec};

// ─── Top-level ────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "camdl",
    about = "Stochastic compartmental model simulation and inference",
    version,
    disable_help_subcommand = true,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Log verbosity (overrides RUST_LOG; CLI > env > default warn)
    #[arg(long, global = true, default_value = "warn",
          value_name = "LEVEL",
          help_heading = "Global options")]
    pub verbosity: log::LevelFilter,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a forward simulation
    #[command(alias = "sim")]
    Simulate(SimulateArgs),

    /// Run a batch sweep from a TOML manifest
    Batch(BatchArgs),

    /// Inference pipeline (MLE, posterior sampling, evaluation)
    #[command(subcommand)]
    Fit(FitCommand),

    /// Standalone bootstrap particle filter at fixed parameters
    Pfilter(PfilterArgs),

    /// Standalone iterated filtering (IF2/MIF2)
    #[command(alias = "mif2")]
    If2(If2Args),

    /// Profile likelihood via parallel IF2 over a parameter grid
    Profile(ProfileArgs),

    /// Evaluate time-dependent expressions against a model
    Eval(EvalArgs),

    /// Data utilities
    #[command(subcommand)]
    Data(DataCommand),

    /// Browse cached simulation runs as a table
    List(ListArgs),

    /// Show full metadata for a cached run
    Show(ShowArgs),

    /// Emit trajectory or observation output from a cached run
    Cat(CatArgs),

    /// Compile a .camdl model to IR JSON (delegates to camdlc)
    Compile(DelegateArgs),

    /// Parse and type-check a .camdl model (delegates to camdlc)
    Check(DelegateArgs),

    /// Print model structure (delegates to camdlc)
    Inspect(DelegateArgs),
}

// ─── Fit subcommands ──────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum FitCommand {
    /// Run the inference pipeline from a fit.toml
    Run(FitRunArgs),
    /// Show completion status of a fit
    Status(FitStatusArgs),
    /// Compare two fit.toml configs
    Diff(FitDiffArgs),
    /// Derive a new fit.toml from an existing one
    New(FitNewArgs),
    /// Resolve and print the fit output directory path
    Where(FitWhereArgs),
}

// ─── Data subcommands ─────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum DataCommand {
    /// Split a data TSV into train and holdout sets
    Split(DataSplitArgs),
}

// ─── Shared flat arg groups ───────────────────────────────────────────────────

/// `--params FILE` (repeatable) + `--param NAME=VALUE` (repeatable) +
/// `--table NAME=FILE` (repeatable)
#[derive(Args, Clone, Default)]
pub struct ModelOverrides {
    /// Parameter TOML file (may be repeated)
    #[arg(long, value_name = "FILE")]
    pub params: Vec<PathBuf>,

    /// Single parameter override, e.g. --param R0=2.5 (may be repeated)
    #[arg(long, value_name = "NAME=VALUE")]
    pub param: Vec<ParamOverride>,

    /// External table for table-lookup expressions, e.g. --table contact=matrix.tsv
    #[arg(long, value_name = "NAME=FILE")]
    pub table: Vec<TableSpec>,
}

/// `--scenario` XOR `--enable`/`--disable`
#[derive(Args, Clone, Default)]
pub struct ScenarioArgs {
    /// Named scenario defined in the model (conflicts with --enable/--disable)
    #[arg(long, conflicts_with_all = ["enable", "disable"])]
    pub scenario: Option<String>,

    /// Enable an intervention (may be repeated; conflicts with --scenario)
    #[arg(long, conflicts_with = "scenario")]
    pub enable: Vec<String>,

    /// Disable an intervention (may be repeated; conflicts with --scenario)
    #[arg(long, conflicts_with = "scenario")]
    pub disable: Vec<String>,
}

/// `--backend` + `--dt`
#[derive(Args, Clone)]
pub struct SimBackend {
    /// Simulation backend (default: gillespie)
    #[arg(long)]
    pub backend: Option<Backend>,

    /// Step size for discrete-time backends (default: 1.0)
    #[arg(long)]
    pub dt: Option<f64>,
}

/// Core inference knobs shared by pfilter / if2 / profile
#[derive(Args, Clone)]
pub struct InferenceCore {
    /// Number of particles
    #[arg(long)]
    pub particles: usize,

    /// Step size
    #[arg(long, default_value_t = 1.0)]
    pub dt: f64,

    /// RNG seed
    #[arg(long, default_value_t = 1)]
    pub seed: u64,

    /// Rayon thread count (0 = all available cores)
    #[arg(long, default_value_t = 0, env = "CAMDL_PARALLEL")]
    pub parallel: usize,
}

/// `--obs NAME` + `--flow NAME`
#[derive(Args, Clone, Default)]
pub struct FlowProjection {
    /// Observation block name (required when model has more than one)
    #[arg(long)]
    pub obs: Option<String>,

    /// Flow name for incidence projection (overrides obs model default)
    #[arg(long)]
    pub flow: Option<String>,
}

// ─── simulate ─────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Basic simulation, output to stdout
  camdl simulate sir.camdl --params p.toml --seed 42

  # Named scenario
  camdl simulate sir.camdl --params p.toml --scenario with_sia --seed 42

  # Generate synthetic observations alongside the trajectory
  camdl simulate sir.camdl --params p.toml --obs cases.tsv --seed 42

  # Cache output under ./results (reruns with same inputs are instant)
  camdl simulate sir.camdl --params p.toml --seed 42 --cas
  camdl list        # browse cached runs

  # Multi-seed ensemble
  camdl simulate sir.camdl --params p.toml --seeds 1:100

  # Posterior predictive check from a fit's draws
  camdl simulate sir.camdl --draws posterior.tsv --replicates 10 --obs ppc.tsv
")]
pub struct SimulateArgs {
    /// IR JSON or .camdl model file
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    /// Parameter vector file (may be repeated), e.g. --param-vec beta=FILE
    #[arg(long, value_name = "PREFIX=FILE")]
    pub param_vec: Vec<ParamVecSpec>,

    /// Named scenarios (may be repeated; conflicts with --enable/--disable)
    #[arg(long = "scenario", conflicts_with_all = ["enable", "disable"])]
    pub scenarios: Vec<String>,

    /// Enable an intervention (may be repeated; conflicts with --scenario)
    #[arg(long, conflicts_with = "scenarios")]
    pub enable: Vec<String>,

    /// Disable an intervention (may be repeated; conflicts with --scenario)
    #[arg(long, conflicts_with = "scenarios")]
    pub disable: Vec<String>,

    #[command(flatten)]
    pub backend: SimBackend,

    /// RNG seed for a single run (conflicts with --seeds)
    #[arg(long, default_value_t = 1, conflicts_with = "seeds",
          env = "CAMDL_SEED")]
    pub seed: u64,

    /// Multiple seeds: range (1:100) or list (1,2,42); conflicts with --replicates
    #[arg(long, conflicts_with_all = ["replicates"])]
    pub seeds: Option<SeedSpec>,

    /// Stochastic replicates per parameter point (conflicts with --seeds)
    #[arg(long, conflicts_with = "seeds")]
    pub replicates: Option<usize>,

    /// Parameter draw source: path to params TSV, "uniform", or "prior"
    #[arg(long)]
    pub draws: Option<String>,

    /// fit.toml supplying priors for --draws prior
    #[arg(long, requires = "draws")]
    pub fit: Option<PathBuf>,

    /// Number of parameter draws (for --draws uniform/prior)
    #[arg(short = 'n', long)]
    pub n_draws: Option<usize>,

    /// Trajectory output file (default: stdout)
    #[arg(short, long, env = "CAMDL_OUTPUT")]
    pub output: Option<PathBuf>,

    /// Write synthetic observations to a single TSV (all streams)
    #[arg(long, conflicts_with_all = ["obs_dir", "obs_only"])]
    pub obs: Option<PathBuf>,

    /// Write one TSV per observation stream to a directory
    #[arg(long, conflicts_with_all = ["obs", "obs_only"])]
    pub obs_dir: Option<PathBuf>,

    /// Like --obs but suppress trajectory output
    #[arg(long, conflicts_with_all = ["obs", "obs_dir", "output"])]
    pub obs_only: Option<PathBuf>,

    /// Print resolved run plan without simulating
    #[arg(long)]
    pub dry_run: bool,

    /// Write output to content-addressable cache
    #[arg(long)]
    pub cas: bool,

    /// Root directory for --cas output
    #[arg(long, default_value = "./output", env = "CAMDL_OUTPUT_DIR")]
    pub output_dir: PathBuf,

    /// Concurrent simulation runs
    #[arg(long, env = "CAMDL_PARALLEL")]
    pub parallel: Option<usize>,

    /// Re-run even if cached output already exists
    #[arg(long)]
    pub force: bool,
}

// ─── batch ────────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Run a batch sweep
  camdl batch run sweep.toml --parallel 8

  # Dry-run: show the resolved sweep grid without simulating
  camdl batch run sweep.toml --dry-run

  # Force rerun, ignoring cached outputs
  camdl batch run sweep.toml --force
")]
pub struct BatchArgs {
    /// Batch TOML manifest file
    pub file: PathBuf,

    /// Override output_dir from the manifest
    #[arg(long, env = "CAMDL_OUTPUT_DIR")]
    pub output_dir: Option<PathBuf>,

    /// Override parallel thread count
    #[arg(long, env = "CAMDL_PARALLEL")]
    pub parallel: Option<usize>,

    /// Print resolved sweep grid without running
    #[arg(long)]
    pub dry_run: bool,

    /// Re-run even if output exists
    #[arg(long)]
    pub force: bool,
}

/// `camdl batch status FILE`
#[derive(Args)]
#[command(after_help = "\
Examples:
  # Count completed vs pending runs for a sweep
  camdl batch status sweep.toml
")]
pub struct BatchStatusArgs {
    /// Batch TOML manifest file
    pub file: PathBuf,
}

// ─── fit ──────────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Run the full inference pipeline declared in fit.toml
  camdl fit run fit.toml --seed 1

  # Run with a specific RNG seed for reproducibility
  camdl fit run fit.toml --seed 42

  # Force rerun even if cached results match
  camdl fit run fit.toml --seed 1 --force
")]
pub struct FitRunArgs {
    /// Fit configuration file (v2 TOML)
    pub config: PathBuf,

    /// Run only this stage by name
    #[arg(long)]
    pub stage: Option<String>,

    /// RNG seed (default: 1)
    #[arg(long)]
    pub seed: Option<u64>,

    /// Re-run and overwrite stale cache
    #[arg(long)]
    pub force: bool,

    /// Starting-point directory or short run hash (requires --stage)
    #[arg(long, requires = "stage")]
    pub starts_from: Option<String>,

    /// Cartesian sweep over a fixed parameter: --sweep NAME=V1,V2,...  (may repeat)
    #[arg(long, value_name = "NAME=V1,V2,...")]
    pub sweep: Vec<SweepSpec>,

    /// Proceed even if prior scout stage failed convergence gate
    #[arg(long)]
    pub allow_nonconverged_scout: bool,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Summarize progress / convergence for a fit
  camdl fit status fit.toml
")]
pub struct FitStatusArgs {
    /// Fit config file or results directory
    pub path: Option<PathBuf>,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Compare two fit.toml configurations side-by-side
  camdl fit diff fit-a.toml fit-b.toml
")]
pub struct FitDiffArgs {
    /// First fit config
    pub a: PathBuf,
    /// Second fit config
    pub b: PathBuf,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Scaffold a new fit.toml from an existing one
  camdl fit new base.toml --output variant.toml
")]
pub struct FitNewArgs {
    /// Source fit.toml to derive from
    #[arg(long)]
    pub from: PathBuf,

    /// Destination path for the new config
    pub dest: PathBuf,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Print the content-addressed output directory for a fit.toml
  camdl fit where fit.toml
")]
pub struct FitWhereArgs {
    /// Fit config file
    pub config: PathBuf,

    /// Print per-seed cell directory instead of fit root
    #[arg(long)]
    pub seed: Option<u64>,
}

// ─── pfilter ──────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Loglik at fixed parameters
  camdl pfilter sir.camdl --params p.toml --data cases.tsv \\
      --particles 5000 --seed 1

  # Multiple replicate filters for loglik SD
  camdl pfilter sir.camdl --params p.toml --data cases.tsv \\
      --particles 2000 --replicates 10

  # Save smoothing paths (ancestor-traced) for plotting vs data
  camdl pfilter sir.camdl --params p.toml --data cases.tsv \\
      --particles 5000 --n-paths 20 --save-paths paths.tsv

  # Prequential out-of-sample evaluation
  camdl pfilter sir.camdl --params p.toml --data cases.tsv \\
      --particles 5000 --save-prequential preq
")]
pub struct PfilterArgs {
    /// IR JSON or .camdl model file
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    #[command(flatten)]
    pub scenario: ScenarioArgs,

    #[command(flatten)]
    pub inference: InferenceCore,

    #[command(flatten)]
    pub flow: FlowProjection,

    /// Observation data TSV (with time column)
    #[arg(long)]
    pub data: PathBuf,

    /// Number of independent filter runs
    #[arg(long, default_value_t = 1)]
    pub replicates: usize,

    /// Write per-observation diagnostics TSV; use "-" for stdout
    #[arg(long)]
    pub trace: Option<String>,

    /// Output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Write final particle states to this TSV
    #[arg(long)]
    pub save_final_state: Option<PathBuf>,

    /// Write N trajectory samples from smoothing distribution to this path
    #[arg(long)]
    pub save_paths: Option<PathBuf>,

    /// Number of trajectories for --save-paths
    #[arg(long, default_value_t = 1)]
    pub n_paths: usize,

    /// Write per-step particle states and log-weights to this TSV
    #[arg(long)]
    pub save_filtering: Option<PathBuf>,

    /// Write {STEM}.tsv (per-step log score, CRPS, PIT, ESS) + {STEM}.json
    /// (full typed PrequentialTrace) for the plug-in one-step-ahead
    /// predictive at the fixed parameters. See
    /// docs/dev/proposals/2026-04-20-prequential-evaluation.md.
    #[arg(long)]
    pub save_prequential: Option<String>,

    /// With --save-prequential, drop per-particle predictive samples
    /// from {STEM}.json. Keeps scalar scores, shrinks the file.
    #[arg(long)]
    pub no_save_samples: bool,
}

// ─── if2 ──────────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # IF2 from scratch with explicit rw-sd map
  camdl if2 sir.camdl --data cases.tsv \\
      --rw-sd \"R0=5,sigma=0.01\" --particles 2000 --iterations 100

  # Use a regime preset (scout / refine / validate)
  camdl if2 sir.camdl --data cases.tsv --regime scout --rw-sd auto

  # Multiple chains in parallel
  camdl if2 sir.camdl --data cases.tsv --rw-sd auto \\
      --regime refine --chains 4 --parallel 4
")]
pub struct If2Args {
    /// IR JSON or .camdl model file
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    #[command(flatten)]
    pub scenario: ScenarioArgs,

    #[command(flatten)]
    pub inference: InferenceCore,

    #[command(flatten)]
    pub flow: FlowProjection,

    /// Observation data TSV
    #[arg(long)]
    pub data: PathBuf,

    /// IF2 iterations
    #[arg(long, conflicts_with = "regime")]
    pub iterations: Option<usize>,

    /// Number of chains
    #[arg(long, conflicts_with = "regime")]
    pub chains: Option<usize>,

    /// Cooling schedule factor (0–1)
    #[arg(long, conflicts_with = "regime")]
    pub cooling: Option<f64>,

    /// Preset configuration: scout, refine, or validate
    #[arg(long, conflicts_with_all = ["chains", "iterations", "cooling"])]
    pub regime: Option<String>,

    /// Random-walk standard deviations, e.g. "beta=0.05,rho=0.01" or "auto"
    #[arg(long)]
    pub rw_sd: Option<RwSd>,

    /// Parameters to hold fixed during estimation (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub fixed: Vec<String>,

    /// Initial-value-problem parameters (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub ivp: Vec<String>,

    /// Write IF2 iteration-by-iteration diagnostics TSV
    #[arg(long)]
    pub trace: Option<PathBuf>,

    /// Output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Write per-chain traces and summary to this directory
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
}

// ─── profile ──────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # 1D profile likelihood for R0 via parallel IF2
  camdl profile sir.camdl --data cases.tsv \\
      --param R0 --grid 0.5:5:20 --particles 2000

  # 2D profile (R0 × sigma)
  camdl profile sir.camdl --data cases.tsv \\
      --param R0 --grid 0.5:5:10 \\
      --param sigma --grid 0.1:1.0:10
")]
pub struct ProfileArgs {
    /// IR JSON or .camdl model file
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    #[command(flatten)]
    pub scenario: ScenarioArgs,

    #[command(flatten)]
    pub inference: InferenceCore,

    #[command(flatten)]
    pub flow: FlowProjection,

    /// Observation data TSV
    #[arg(long)]
    pub data: PathBuf,

    /// Profile grid: --sweep NAME=V1,V2,...  (repeat for 2D+)
    #[arg(long, value_name = "NAME=V1,V2,...", required = true)]
    pub sweep: Vec<SweepSpec>,

    /// IF2 iterations per grid point
    #[arg(long, default_value_t = 50)]
    pub iterations: usize,

    /// Independent IF2 starts per grid point
    #[arg(long, default_value_t = 3)]
    pub starts: usize,

    /// Cooling schedule
    #[arg(long, default_value_t = 0.95)]
    pub cooling: f64,

    /// Random-walk SDs
    #[arg(long)]
    pub rw_sd: Option<RwSd>,

    /// Parameters to hold fixed (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub fixed: Vec<String>,

    /// Profile TSV output (default: stdout)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

// ─── eval ─────────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Evaluate one or more expressions on a time grid
  camdl eval sir.camdl --params p.toml \\
      --expr \"beta,gamma\" --from 0 --to 730 --every 1

  # Inspect a forcing function over time
  camdl eval sir.camdl --params p.toml \\
      --expr \"seasonal(t)\" --from 0 --to 365
")]
pub struct EvalArgs {
    /// IR JSON or .camdl model file
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    /// Expression names to evaluate (comma-separated)
    #[arg(long, value_delimiter = ',', required = true)]
    pub expr: Vec<String>,

    /// Time grid start
    #[arg(long, default_value_t = 0.0, conflicts_with = "at")]
    pub from: f64,

    /// Time grid end
    #[arg(long, default_value_t = 100.0, conflicts_with = "at")]
    pub to: f64,

    /// Time grid step
    #[arg(long, default_value_t = 1.0, conflicts_with = "at")]
    pub every: f64,

    /// Specific time points (comma-separated; conflicts with --from/--to/--every)
    #[arg(long, value_delimiter = ',')]
    pub at: Vec<f64>,

    /// Output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

// ─── data split ───────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Split a data TSV at t=100 into training + holdout sets
  camdl data split cases.tsv --at-time 100 \\
      --train train.tsv --holdout holdout.tsv
")]
pub struct DataSplitArgs {
    /// Input data TSV
    pub file: PathBuf,

    /// Split at this time value (conflicts with --fraction)
    #[arg(long, conflicts_with = "fraction")]
    pub at_time: Option<f64>,

    /// Split at this fraction of rows, 0–1 (conflicts with --at-time)
    #[arg(long, conflicts_with = "at_time")]
    pub fraction: Option<f64>,

    /// Name of the time column (auto-detected if absent)
    #[arg(long)]
    pub time_col: Option<String>,

    /// Training set output path
    #[arg(long)]
    pub train: Option<PathBuf>,

    /// Holdout set output path
    #[arg(long)]
    pub holdout: Option<PathBuf>,
}

// ─── browse ───────────────────────────────────────────────────────────────────

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Show the most recent cached runs and fits
  camdl list

  # Filter by model, scenario, or recency
  camdl list --model sir
  camdl list --scenario baseline
  camdl list --since 1h

  # Only simulate runs or only fits
  camdl list --kind sim
  camdl list --kind fit

  # Machine-readable JSON
  camdl list --format json
")]
pub struct ListArgs {
    /// Root directory to scan (default: ./output)
    #[arg(default_value = "./output", env = "CAMDL_OUTPUT_DIR")]
    pub root: PathBuf,

    /// Filter by model path substring
    #[arg(long)]
    pub model: Option<String>,

    /// Filter by scenario name
    #[arg(long)]
    pub scenario: Option<String>,

    /// Show only runs created within this duration (e.g. 1h, 30m, 2d)
    #[arg(long)]
    pub since: Option<ListDuration>,

    /// Filter by run kind: sim, fit, or both
    #[arg(long, default_value = "both")]
    pub kind: String,

    /// Maximum number of results to display
    #[arg(long, default_value_t = 50, conflicts_with = "all")]
    pub limit: usize,

    /// Show all results (no limit)
    #[arg(long)]
    pub all: bool,

    /// Output format: human (default) or json
    #[arg(long)]
    pub format: Option<String>,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Resolve a run by short hash prefix
  camdl show abc1234

  # Path to a stage directory also works
  camdl show results/fits/sir-8a3f12b4/refine

  # JSON output for scripting
  camdl show abc1234 --format json
")]
pub struct ShowArgs {
    /// Short hash prefix or path to run directory
    pub target: String,

    /// Root output directory to search (default: ./output)
    #[arg(long, default_value = "./output", env = "CAMDL_OUTPUT_DIR")]
    pub root: PathBuf,

    /// Output format: human (default) or json
    #[arg(long)]
    pub format: Option<String>,
}

#[derive(Args)]
#[command(after_help = "\
Examples:
  # Emit the trajectory for a cached run
  camdl cat abc1234

  # Select a particular observation stream
  camdl cat abc1234 --stream weekly_cases
")]
pub struct CatArgs {
    /// Short hash prefix or path to run directory
    pub target: String,

    /// Root output directory to search (default: ./output)
    #[arg(long, default_value = "./output", env = "CAMDL_OUTPUT_DIR")]
    pub root: PathBuf,

    /// Observation stream name (when run has multiple streams)
    #[arg(long)]
    pub stream: Option<String>,
}

// ─── compare ──────────────────────────────────────────────────────────────────

/// `camdl compare` — multi-model prequential comparison table.
///
/// Reads prequential.json from ≥2 fit stage dirs (or a compare.toml)
/// and renders a baseline-centered comparison.
/// See docs/dev/proposals/2026-04-20-prequential-evaluation.md §8.
#[derive(Args)]
#[command(after_help = "\
Examples:
  # Compare two fits by prequential scores (table output)
  camdl compare fits/det/pfilter fits/stoch/pfilter --baseline det

  # Three-way, markdown output for pasting into a paper
  camdl compare fits/a/pf fits/b/pf fits/c/pf --format md

  # Reproducible preset via compare.toml
  camdl compare --config compare.toml

  # Render despite different T_score across fits (Δ columns → '—')
  camdl compare fits/a/pf fits/b/pf --allow-mismatched-horizon
")]
pub struct CompareArgs {
    /// Stage directories (or .json paths) to compare — need ≥2 when
    /// --config is not used
    pub paths: Vec<String>,

    /// compare.toml with [[model]] entries (baseline/metrics/format
    /// also loadable from the file)
    #[arg(long)]
    pub config: Option<String>,

    /// Reference model for Δ columns (default: argmax elpd)
    #[arg(long)]
    pub baseline: Option<String>,

    /// Metrics to display (comma-separated: elpd, crps, pit_cov90)
    #[arg(long = "metric", alias = "metrics")]
    pub metrics: Option<String>,

    /// Output format: table (default), md, json
    #[arg(long, default_value = "table")]
    pub format: String,

    /// Render even if T_score differs across models (Δ columns → '—')
    #[arg(long)]
    pub allow_mismatched_horizon: bool,
}

// ─── Delegated commands (compile / check / inspect → camdlc) ─────────────────

/// All args are passed through verbatim to camdlc.
/// Set CAMDLC_PATH to override the camdlc binary location.
#[derive(Args)]
pub struct DelegateArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}
