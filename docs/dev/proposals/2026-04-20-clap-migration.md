---
status: proposal
date: 2026-04-20
---

# CLI Migration to clap

## Motivation

The CLI currently parses arguments with a hand-rolled
`while i < args.len() { match args[i] { ... } i += 1; }` loop,
duplicated across 7+ entry points. This buys us nothing:

- No bounds-checked value reads (an off-by-one or missing value is
  a panic or wrong-token read, not an error message).
- `--help` text is maintained separately from the flag definitions
  and can drift.
- No shell completions.
- No type coercion — everything is strings, parsed manually at call
  sites with varying error quality.
- Mutual exclusions (`--scenario` vs `--enable`/`--disable`,
  `--seeds` vs `--replicates`, etc.) are silently accepted and
  only fail at runtime.

`clap` with `#[derive(Parser)]` addresses all of these. The derives
are self-documenting: the flag definition and its help text live
together, the types enforce the parse, and `--help` is generated
for free.

## On TOML / serde consolidation

Short answer: **no**. The existing `FitConfigV2` / `FitToml` structs
use `serde + toml` for structured configuration files. That is the
right tool for that job. The CLI layer is a separate surface — it
passes short arguments that direct the runtime; the TOML layer
encodes a complete experiment description. Merging them (e.g.,
`--config FILE` that clap overlays on top of its own args) would add
complexity with no gain. Keep the seam: clap parses argv, serde
parses `.toml`.

## Command hierarchy

`simulate` takes a positional `MODEL` argument, which means clap
cannot also give it subcommands — a command is either positional-arg
or subcommand-parent, not both. The fix is to promote `batch` and
`batch status` to top-level commands. This also makes semantic sense:
batch is a distinct execution mode, not a simulation variant.

Gated commands (`serve`, `voi`) are omitted entirely; they re-enter
when promoted.

```
camdl
  simulate  MODEL [OPTIONS]           forward simulation
  batch     FILE  [OPTIONS]           batch sweep
    status  FILE                      batch job status
  fit
    run     FIT.toml [OPTIONS]        v2 pipeline
    status  [PATH]                    results status
    diff    A.toml B.toml             compare configs
    new     --from SRC [DEST]         derive new config
    where   FIT.toml [--seed N]       print fit output path (scriptable; no computation)
  pfilter   MODEL [OPTIONS]           bootstrap PF at fixed params
  if2       MODEL [OPTIONS]           iterated filtering (standalone)
  profile   MODEL [OPTIONS]           profile likelihood
  eval      MODEL [OPTIONS]           expression evaluation
  data
    split   FILE [OPTIONS]            train/holdout split
  list      [ROOT] [OPTIONS]          browse cached runs
  show      HASH [OPTIONS]            run metadata
  cat       HASH [OPTIONS]            emit run output
  compile / check / inspect           delegated to camdlc
```

## Custom value types

These types need `impl FromStr` (for clap) and carry the parse
error into clap's error path, so the user sees a structured message.

```rust
// --param R0=2.5  →  ParamOverride { name: "R0", value: 2.5 }
pub struct ParamOverride { pub name: String, pub value: f64 }

// --table contact=matrix.tsv  →  TableSpec { name, path }
pub struct TableSpec { pub name: String, pub path: PathBuf }

// --backend gillespie / tau_leap / chain_binomial / ode
// Derive Display + clap::ValueEnum for free --help variant list.
#[derive(Clone, Copy, ValueEnum)]
pub enum Backend { Gillespie, TauLeap, ChainBinomial, Ode }

// --seeds 1:100  or  --seeds 1,2,42
pub enum SeedSpec { Range(u64, u64), List(Vec<u64>) }

// --rw-sd auto  or  --rw-sd "beta=0.05,rho=0.01"
// Map uses HashMap so handlers can do O(1) lookup; duplicate keys error at parse time.
pub enum RwSd { Auto, Map(HashMap<String, f64>) }

// --sweep NAME=V1,V2,...  →  SweepSpec { name: "beta", values: vec![0.1, 0.2, 0.3] }
// Typed so handlers never re-parse strings; values are f64 at parse time.
pub struct SweepSpec { pub name: String, pub values: Vec<f64> }

// --since 1h / 30m / 2d  (for camdl list)
pub struct ListDuration(std::time::Duration);
```

Place all of these in `cli/src/args/types.rs`.

## Shared `Args` flat structs (`#[command(flatten)]`)

These capture groups of flags that appear in multiple commands.
Each is `#[derive(Args, Clone)]`. Commands include them via
`#[command(flatten)]`.

### `ModelOverrides` — parameter loading, used by 6+ commands

```rust
#[derive(Args, Clone, Default)]
pub struct ModelOverrides {
    /// Parameter TOML file(s) — may be repeated
    #[arg(long, value_name = "FILE")]
    pub params: Vec<PathBuf>,

    /// Single parameter override, e.g. --param R0=2.5
    #[arg(long, value_name = "NAME=VALUE")]
    pub param: Vec<ParamOverride>,

    /// External table for table-lookup expressions, e.g. --table contact=matrix.tsv
    #[arg(long, value_name = "NAME=FILE")]
    pub table: Vec<TableSpec>,
}
```

### `ScenarioArgs` — scenario/intervention selection, used by 4+ commands

```rust
#[derive(Args, Clone, Default)]
#[group(conflicts_with_all = ["enable", "disable"])]   // --scenario XOR --enable/--disable
pub struct ScenarioArgs {
    /// Named scenario defined in the model
    #[arg(long)]
    pub scenario: Option<String>,

    /// Enable an intervention (may repeat; conflicts with --scenario)
    #[arg(long)]
    pub enable: Vec<String>,

    /// Disable an intervention (may repeat; conflicts with --scenario)
    #[arg(long)]
    pub disable: Vec<String>,
}
```

### `SimBackend` — backend + step size, used by simulate + fit paths

```rust
#[derive(Args, Clone)]
pub struct SimBackend {
    #[arg(long, default_value = "gillespie")]
    pub backend: Backend,

    /// Step size for discrete-time backends
    #[arg(long, default_value_t = 1.0)]
    pub dt: f64,
}
```

### `InferenceCore` — shared inference knobs, used by pfilter/if2/profile

```rust
#[derive(Args, Clone)]
pub struct InferenceCore {
    #[arg(long)]
    pub particles: usize,

    #[arg(long, default_value_t = 1.0)]
    pub dt: f64,

    #[arg(long, default_value_t = 1)]
    pub seed: u64,

    /// Rayon thread count (0 = all available)
    #[arg(long, default_value_t = 0)]
    pub parallel: usize,
}
```

### `FlowProjection` — `--flow` + `--obs`, used by pfilter/if2/profile

```rust
#[derive(Args, Clone, Default)]
pub struct FlowProjection {
    /// Observation block name (when model has more than one)
    #[arg(long)]
    pub obs: Option<String>,

    /// Flow name for incidence projection (overrides obs model default)
    #[arg(long)]
    pub flow: Option<String>,
}
```

## Full command structs

### `simulate`

```rust
#[derive(Parser)]
pub struct SimulateArgs {
    /// IR JSON or .camdl model file
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    #[command(flatten)]
    pub scenario: ScenarioArgs,

    #[command(flatten)]
    pub backend: SimBackend,

    /// Single seed
    #[arg(long, default_value_t = 1, conflicts_with = "seeds")]
    pub seed: u64,

    /// Multiple seeds: range (1:100) or list (1,2,42); conflicts with --replicates
    #[arg(long, conflicts_with_all = ["seed", "replicates"])]
    pub seeds: Option<SeedSpec>,

    /// Stochastic replicates per parameter point; conflicts with --seeds
    #[arg(long, conflicts_with = "seeds")]
    pub replicates: Option<usize>,

    /// Parameter draw source: path to TSV, "uniform", or "prior"
    #[arg(long)]
    pub draws: Option<String>,

    /// fit.toml for --draws prior
    #[arg(long, requires = "draws")]
    pub fit: Option<PathBuf>,

    /// Number of draws (for --draws uniform/prior)
    #[arg(short = 'n', long)]
    pub n_draws: Option<usize>,

    /// Trajectory output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Write synthetic observations (all streams in one file)
    #[arg(long, conflicts_with_all = ["obs_dir", "obs_only"])]
    pub obs: Option<PathBuf>,

    /// Write one TSV per observation stream
    #[arg(long, conflicts_with_all = ["obs", "obs_only"])]
    pub obs_dir: Option<PathBuf>,

    /// Like --obs but suppress trajectory output
    #[arg(long, conflicts_with_all = ["obs", "obs_dir", "output"])]
    pub obs_only: Option<PathBuf>,

    #[arg(long)]
    pub dry_run: bool,

    /// Cache output in content-addressable storage
    #[arg(long)]
    pub cas: bool,

    #[arg(long, default_value = "./results")]
    pub output_dir: PathBuf,

    #[arg(long)]
    pub parallel: Option<usize>,

    #[arg(long)]
    pub force: bool,
}
```

### `simulate batch`

```rust
#[derive(Parser)]
pub struct BatchArgs {
    pub file: PathBuf,

    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    #[arg(long)]
    pub parallel: Option<usize>,

    #[arg(long)]
    pub dry_run: bool,

    #[arg(long)]
    pub force: bool,
}
```

### `fit run`

```rust
#[derive(Parser)]
pub struct FitRunArgs {
    pub config: PathBuf,

    #[arg(long)]
    pub stage: Option<String>,

    #[arg(long, default_value_t = 1)]
    pub seed: u64,

    #[arg(long)]
    pub force: bool,

    /// Requires --stage
    #[arg(long, requires = "stage")]
    pub starts_from: Option<String>,

    /// Cartesian sweep: --sweep NAME=V1,V2,...  (may repeat)
    #[arg(long, value_name = "NAME=V1,V2,...")]
    pub sweep: Vec<SweepSpec>,

    #[arg(long)]
    pub allow_nonconverged_scout: bool,
}
```

### `pfilter`

```rust
#[derive(Parser)]
pub struct PfilterArgs {
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    #[command(flatten)]
    pub scenario: ScenarioArgs,

    #[command(flatten)]
    pub inference: InferenceCore,

    #[command(flatten)]
    pub flow: FlowProjection,

    #[arg(long)]
    pub data: PathBuf,

    #[arg(long, default_value_t = 1)]
    pub replicates: usize,

    /// Write per-obs diagnostics; "-" for stdout
    #[arg(long)]
    pub trace: Option<String>,

    #[arg(short, long)]
    pub output: Option<PathBuf>,

    #[arg(long)]
    pub save_final_state: Option<PathBuf>,

    /// Draw N trajectory samples from the smoothing distribution
    #[arg(long)]
    pub save_paths: Option<PathBuf>,

    #[arg(long, default_value_t = 1)]
    pub n_paths: usize,

    #[arg(long)]
    pub save_filtering: Option<PathBuf>,
}
```

### `if2`

```rust
#[derive(Parser)]
pub struct If2Args {
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    #[command(flatten)]
    pub scenario: ScenarioArgs,

    #[command(flatten)]
    pub inference: InferenceCore,

    #[command(flatten)]
    pub flow: FlowProjection,

    #[arg(long)]
    pub data: PathBuf,

    #[arg(long)]
    pub iterations: usize,

    #[arg(long)]
    pub chains: Option<usize>,

    /// Preset config: scout, refine, validate
    #[arg(long, conflicts_with_all = ["chains", "iterations", "cooling"])]
    pub regime: Option<String>,

    #[arg(long)]
    pub cooling: Option<f64>,

    #[arg(long)]
    pub rw_sd: Option<RwSd>,

    /// Parameters to hold fixed during estimation
    #[arg(long, value_delimiter = ',')]
    pub fixed: Vec<String>,

    /// Initial-value-problem parameters
    #[arg(long, value_delimiter = ',')]
    pub ivp: Vec<String>,

    #[arg(short, long)]
    pub output: Option<PathBuf>,

    #[arg(long)]
    pub output_dir: Option<PathBuf>,
}
```

### `profile`

```rust
#[derive(Parser)]
pub struct ProfileArgs {
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    #[command(flatten)]
    pub scenario: ScenarioArgs,

    #[command(flatten)]
    pub inference: InferenceCore,

    #[command(flatten)]
    pub flow: FlowProjection,

    #[arg(long)]
    pub data: PathBuf,

    #[arg(long)]
    pub focal: String,

    /// Grid values for focal param: --grid 10,20,30
    #[arg(long, value_delimiter = ',')]
    pub grid: Vec<f64>,

    #[arg(long, default_value_t = 1000)]
    pub particles: usize,

    #[arg(long, default_value_t = 50)]
    pub iterations: usize,

    #[arg(long, default_value_t = 3)]
    pub starts: usize,

    #[arg(long, default_value_t = 0.95)]
    pub cooling: f64,

    #[arg(long)]
    pub rw_sd: Option<RwSd>,

    /// Parameters to hold fixed
    #[arg(long, value_delimiter = ',')]
    pub fixed: Vec<String>,

    #[arg(short, long)]
    pub output: Option<PathBuf>,
}
```

### `eval`

```rust
#[derive(Parser)]
pub struct EvalArgs {
    pub model: PathBuf,

    #[command(flatten)]
    pub model_overrides: ModelOverrides,

    /// Expression names to evaluate (comma-separated)
    #[arg(long, value_delimiter = ',', required = true)]
    pub expr: Vec<String>,

    #[arg(long, default_value_t = 0.0)]
    pub from: f64,

    #[arg(long, default_value_t = 100.0)]
    pub to: f64,

    #[arg(long, default_value_t = 1.0, conflicts_with = "at")]
    pub every: f64,

    /// Specific time points (comma-separated); conflicts with --every
    #[arg(long, value_delimiter = ',', conflicts_with = "every")]
    pub at: Vec<f64>,

    #[arg(short, long)]
    pub output: Option<PathBuf>,
}
```

### `data split`

```rust
#[derive(Parser)]
pub struct DataSplitArgs {
    pub file: PathBuf,

    /// Split at a specific time value
    #[arg(long, conflicts_with = "fraction")]
    pub at_time: Option<f64>,

    /// Split at this fraction of rows (0–1)
    #[arg(long, conflicts_with = "at_time")]
    pub fraction: Option<f64>,

    #[arg(long)]
    pub time_col: Option<String>,

    #[arg(long)]
    pub train: Option<PathBuf>,

    #[arg(long)]
    pub holdout: Option<PathBuf>,
}
```

### Browse commands

```rust
#[derive(Parser)]
pub struct ListArgs {
    #[arg(default_value = "./results")]
    pub root: PathBuf,

    #[arg(long)]
    pub model: Option<String>,

    #[arg(long)]
    pub scenario: Option<String>,

    #[arg(long)]
    pub since: Option<ListDuration>,

    #[arg(long, default_value = "both")]
    pub kind: String,

    #[arg(long, default_value_t = 50, conflicts_with = "all")]
    pub limit: usize,

    #[arg(long)]
    pub all: bool,

    #[arg(long)]
    pub format: Option<String>,
}

#[derive(Parser)]
pub struct ShowArgs {
    pub target: String,          // hash prefix or path

    #[arg(long)]
    pub format: Option<String>,
}

#[derive(Parser)]
pub struct CatArgs {
    pub target: String,

    #[arg(long)]
    pub stream: Option<String>,
}
```

## Binary rename and bash wrapper removal

`camdl` is currently a bash script (`bin/camdl`) that routes
`compile`/`check`/`inspect` to `camdlc` and everything else to the
`camdl-sim` Rust binary. The Rust binary already replicates this
routing internally via `util::find_camdlc()` / `delegate_to_camdlc()`
with a full discovery chain:

1. Sibling binary (same dir as running executable — release layout)
2. `CAMDLC_PATH` env var override
3. System PATH

The bash script is therefore redundant. As part of Phase 1:

- Rename the Cargo binary target from `camdl-sim` → `camdl`
  (`rust/crates/cli/Cargo.toml` `[[bin]]` name field).
- Delete `bin/camdl`.
- Update `Makefile`: `CAMDL_SIM` → `CAMDL`, install target,
  uninstall target, `sim` target, integration test invocation.
- Update stale comments in `examples/` that reference `camdl-sim`.

The delegated commands (`compile`, `check`, `inspect`) use
`#[arg(trailing_var_arg = true, allow_hyphen_values = true)]` to
collect all remaining argv tokens and pass them to
`delegate_to_camdlc`. `CAMDLC_PATH` is the single documented env var
for overriding the OCaml binary location; add it to the `--help` text
for these commands via `#[arg(env = "CAMDLC_PATH")]` on a hidden arg
or a note in the long_about.

## Migration plan

The codebase is unreleased so there is no backwards-compatibility
constraint on the argv surface. Do it in three phases.

### Phase 1 — Add clap, rename binary, define type system

1. Add `clap = { version = "4", features = ["derive"] }` to
   `cli/Cargo.toml`. Rename binary target `camdl-sim` → `camdl`.
2. Delete `bin/camdl`. Update `Makefile` references.
3. Run integration tests (`make test-golden`, `cas_integration`,
   `synthetic_fit_grid`) — they must pass before any handler changes.
   This is the baseline gate for Phase 2.
4. Create `cli/src/args/types.rs`: `ParamOverride`, `TableSpec`,
   `Backend`, `SeedSpec`, `SweepSpec`, `RwSd`, `ListDuration` — all
   with `impl FromStr`. `RwSd::Map` uses `HashMap<String, f64>`.
5. Create `cli/src/args/mod.rs`: all flat `Args` structs and the
   top-level `Cli` / `Command` enum.
6. Wire `Cli::parse()` at the top of `main()`. Replace the manual
   `match all_args[0]` dispatcher entirely — clap is the dispatcher.
7. Remove the `print_*_help()` family.

### Phase 2 — Migrate handlers

Each step: replace the arg-parsing loop in the handler with the
typed `Args` struct, run `make test`, fix any broken integration tests
before moving on.

Order (easiest first):
1. `data split`, `cat`, `show`, `list`
2. `eval`, `compile`/`check`/`inspect`
3. `simulate`
4. `batch`, `batch status`
5. `pfilter`
6. `if2`, `profile`
7. `fit run`, `fit status`, `fit diff`, `fit new`, `fit where`

After step 7: review pass to confirm `InferenceCore`, `ModelOverrides`,
and `FlowProjection` still fit all their consumers. Refactor the
shared structs if needed before declaring Phase 2 complete.

`serve` and `voi` are gated. When promoted, add their `Args` structs,
wire dispatch, and slot into the order: `serve` after step 1,
`voi run` after step 7.

### Phase 3 — Polish

- Add `#[arg(env = "CAMDL_OUTPUT_DIR")]`, `#[arg(env = "CAMDL_PARALLEL")]`,
  `#[arg(env = "CAMDL_BACKEND")]` to the relevant fields. CLI flag
  overrides env var overrides default — document this precedence
  explicitly in the affected `--help` strings.
- Add a hidden `completions` subcommand that emits shell completion
  scripts (fish, bash, zsh) via `clap_complete`.
- Error handling consolidation (`die()`/`or_die()`) is a **separate PR**
  after clap lands — do not bundle.

## What to watch for

**`--param NAME=VALUE` with `=` in the value.** clap's default
`value_parser` for `Vec<T>` with a custom `FromStr` passes the full
token after the flag to `from_str`. `"R0=2.5"` parses correctly.
`"--param=R0=2.5"` (flag+value joined with `=`) also works because
clap splits on the first `=`. No problem here.

**`--saves-paths N PATH` (two positional args after the flag).**
Use `#[arg(long, num_args = 2)]` — clap supports this natively.

**`--sweep NAME=V1,V2,...` — commas inside the value.** Do NOT use
`value_delimiter = ','` here; the values themselves contain commas.
Parse `Vec<String>` and split manually inside the handler.

**The delegated commands (`compile`, `check`, `inspect`).** These
pass all remaining args to a subprocess. Use `#[arg(trailing_var_arg = true)]`
to collect them into `Vec<String>` without clap trying to parse them.

**`--verbosity` default.** The current `env_logger` integration reads
`RUST_LOG`. Clap can expose `--verbosity` as a flag that sets the
filter; the `RUST_LOG` fallback can remain via `env_logger`'s
`try_init()`. Don't fight the existing log setup — just add the
`--verbosity` arg and call `env_logger::Builder::from_env(...).parse_filters(verbosity).init()`.
