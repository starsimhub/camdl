# camdl Experiment & Output System Specification

**Version:** 0.4-draft
**Date:** 2026-03-17

This document specifies the experiment file format, content-addressable output
structure, run management, and provenance guarantees for camdl. It implements
the parameter grammar (Buffalo 2026) at the systems level.

---

## 1. Design Principles

**One file per concern, no overlap.**

```
.camdl         → what the model IS (structure + scenarios + default C)
params.toml    → what the values ARE (point m ∈ M)
experiment.toml → how the BATCH analysis RUNS (runtime C, seed grid, comparisons)
```

Each file owns its domain exclusively. The experiment file cannot set parameter
values — that's what params.toml is for. The params file cannot set the backend
— that's what the experiment file is for. The .camdl file cannot set concrete
parameter values — that's what external files are for.

**The model file is self-contained for single runs.** A `.camdl` file defines
structure, interventions, scenarios, simulation config, and output config. With
a `params.toml` and a seed, it's everything needed for a run — no experiment
file required. The experiment file adds batch infrastructure (seed grids,
parallelism, comparisons, content-addressable output) on top.

**Mirrored structure for overrides.** When the experiment file overrides a
`.camdl` block, it uses the same field names in the same hierarchy. If you
know the `.camdl` syntax, you know the experiment file syntax.

**Reproducibility is structural, not aspirational.** Every output file is
content-addressed by the inputs that produced it. Same inputs → same hash →
same directory. Different inputs → different hash → separate directory.

**M and σ are distinct layers.** Parameter values (M) and scenario patches (σ)
are different operations on different objects. The CLI makes this visible:
`--param` operates on M, `--scenario` and `--enable` operate on σ. They never
share flags.

---

## 2. File Roles and Separation of Concerns

### 2.1 What Goes Where

```
┌──────────────────────┬──────────────────────┬───────────────────────────┐
│       Concern        │     Lives in         │     Can override?         │
├──────────────────────┼──────────────────────┼───────────────────────────┤
│ Model structure      │ .camdl only          │ never                     │
│ Parameter names/types│ .camdl only          │ never                     │
│ Parameter values (M) │ params.toml only     │ layered .toml, --param, σ │
│ Scenarios (σ)        │ .camdl scenarios { } │ experiment merges on top  │
│ Interventions        │ .camdl only          │ scenarios enable/disable  │
│ Backend choice       │ experiment [config]  │ CLI --backend             │
│ Time range           │ .camdl default       │ experiment [config], CLI  │
│ Output schedule      │ .camdl default       │ experiment [config]       │
│ Seeds                │ experiment [config]  │ CLI --seed                │
│ Comparisons          │ experiment only      │ never                     │
└──────────────────────┴──────────────────────┴───────────────────────────┘
```

No row has entries in both "params.toml" and "experiment [config]" — they
own completely disjoint domains. Scenarios are defined in the `.camdl` file
and are available for both single-run exploration and batch experiments.

### 2.2 Precedence Chains

**For C (configuration):**

```
.camdl simulate/output blocks    (structural defaults)
  ↓ overridden by
experiment.toml [config]         (analysis-specific)
  ↓ overridden by
CLI flags                        (convenience, non-persistent)
```

**For M (parameters):**

```
params.toml (first file)         (base values)
  ↓ overridden by
params.toml (later files)        (layered patches)
  ↓ overridden by
scenario patches (set/scale)     (counterfactual modifications to M)
  ↓ overridden by
--param CLI flags                (convenience, non-persistent)
```

**For scenarios:**

```
.camdl scenarios { }             (model-level definitions)
  ↓ merged with
experiment.toml [scenarios]      (experiment adds/overrides by name)
```

Experiment file scenarios are **merged** into `.camdl` scenarios. On name
conflict, the experiment file definition wins (with a note). All
non-conflicting `.camdl` scenarios survive:

```
note: scenario 'with_sia' in experiment.toml overrides 'with_sia'
  from model.camdl. Using experiment file definition.
```

---

## 3. Single-Run Workflow (No Experiment File)

The `.camdl` file + `params.toml` is a complete, self-contained specification
for running a model. No experiment file is needed for exploration.

### 3.1 Basic Run

```bash
# Run baseline (no scenario), output to stdout
camdl simulate model.camdl --params params.toml --seed 42

# Run a named scenario defined in the .camdl file
camdl simulate model.camdl --params params.toml --scenario with_sia --seed 42

# Enable a specific intervention (ad-hoc, no named scenario)
camdl simulate model.camdl --params params.toml --enable sia_round_1 --seed 42

# Override a parameter (M layer — works with or without scenarios)
camdl simulate model.camdl --params params.toml --param beta=0.5 --seed 42

# Scenario + parameter override (σ layer + M layer — both valid)
camdl simulate model.camdl --params params.toml --scenario with_sia --param beta=0.5 --seed 42
```

### 3.2 CLI Flag Rules

**`--scenario` and `--enable`/`--disable` are mutually exclusive (both σ layer):**

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
camdl simulate model.camdl --params p.toml --enable sia_round_1 --param beta=0.5 --seed 42
```

The `--param` override applies **after** the scenario patch in the M
precedence chain: `params.toml → scenario set/scale → --param`.

### 3.3 CLI Parameter Flags

```bash
--params FILE              # load parameter file (can repeat for layering)
--param NAME=VALUE         # override a single parameter (can repeat)
--param-vec PREFIX=FILE    # override indexed params from keyed TSV
```

### 3.4 With Content-Addressable Output

```bash
camdl simulate model.camdl --params params.toml --seed 42 --output-dir output/
```

When `--output-dir` is provided, the content-addressable directory structure
is created. Since there is no experiment file, no `experiment.toml` is
frozen — only the merged `params.toml` and per-run `run.json`:

```
output/
  model-{hash}/
    config-{hash}/
      params.toml              # frozen merged baseline parameters
      runs/
        baseline/
          seed-42/
            trajectories.parquet
            flows.parquet
            diagnostics.tsv
            run.json
```

The `run.json` contains full provenance. This is sufficient to reproduce
the run. If a scenario is used, the scenario directory reflects it:

```
runs/with_sia/seed-42/...
```

---

## 4. The Experiment File

### 4.1 Format

TOML. Used only for batch analysis (multiple scenarios × seeds, comparisons).

### 4.2 Full Example

```toml
# experiment.toml — Nigeria SIA evaluation 2024

[experiment]
name = "Nigeria SIA evaluation 2024"
model = "models/seir_nigeria.camdl"
params = ["params/base.toml", "params/fitted_2024.toml"]  # layered; later wins

# ── Runtime configuration ──────────────────────────────

[config]
backend = "gillespie"              # gillespie | tau_leap | chain_binomial
dt = 1.0                           # only for tau_leap / chain_binomial
seeds = { from = 1, to = 1000 }
parallel = 8                       # concurrent runs
output_dir = "output"              # root of content-addressable tree

# Override .camdl simulate block (same field names)
[config.simulate]
from = 0.0                         # in model time_unit
to = 730.0                         # in model time_unit

# Override .camdl output block (same structure)
[config.output.trajectories]
every = 7.0                        # in model time_unit
format = "parquet"

[config.output.flows]
every = 7.0
format = "parquet"

[config.output.summary]
format = "tsv"

# ── Scenarios (merged with .camdl scenarios — overrides on conflict) ──

[scenarios.high_coverage]
enable = ["sia_round_1"]
set = { vacc_frac = 0.95 }

[scenarios.more_transmissible]
scale = { beta = 1.5 }

[scenarios.combined]
compose = ["with_sia", "more_transmissible"]

# ── Comparisons ────────────────────────────────────────

[compare]
pairs = [
    ["baseline", "with_sia"],
    ["baseline", "high_coverage"],
    ["baseline", "delayed_sia"],
]

[compare.derived]
cases_averted = "reference.total_cases - test.total_cases"
relative_reduction = "cases_averted / reference.total_cases"
peak_reduction = "1 - test.peak_I / reference.peak_I"
```

### 4.3 [experiment] Fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Human-readable experiment name |
| `model` | path | yes | Path to `.camdl` model file |
| `params` | path or list | yes | Path(s) to params.toml. List = layered, later wins |

### 4.4 [config] Fields (Closed Schema)

The `[config]` section has a **fixed schema** that mirrors the `.camdl` block
structure exactly — same field names, same hierarchy. `[config]` is optional;
all fields have defaults. Unknown keys produce a clear error:

```
error: unknown config key 'beta' in [config].
  [config] mirrors .camdl block structure. Valid sections:
    [config.simulate]  — override simulate { from, to }
    [config.output.*]  — override output { trajectories, flows, summary }
  For parameter values, use params.toml or a scenario patch.
```

**Top-level config keys** (runtime-only, no `.camdl` equivalent):

| Field | Type | Default | Description |
|---|---|---|---|
| `backend` | string | `"gillespie"` | `gillespie`, `tau_leap`, `chain_binomial` |
| `dt` | float | `1.0` | Step size for discrete-time backends |
| `seeds` | table | required | `{ from = N, to = M }` or `{ list = [...] }` or `{ n = N, start = M }` |
| `parallel` | int | `1` | Concurrent runs |
| `output_dir` | path | `"output/"` | Root of content-addressable tree |

**[config.simulate]** — mirrors `.camdl` `simulate { }`:

| Field | Type | Description |
|---|---|---|
| `from` | float | Simulation start time (in model time_unit) |
| `to` | float | Simulation end time (in model time_unit) |

**[config.output.trajectories]** — mirrors `.camdl` `output { trajectories { } }`:

| Field | Type | Description |
|---|---|---|
| `every` | float | Output interval (in model time_unit) |
| `format` | string | `"parquet"` or `"tsv"` |

**[config.output.flows]** — mirrors `.camdl` `output { flows { } }`:

| Field | Type | Description |
|---|---|---|
| `every` | float | Output interval (in model time_unit) |
| `format` | string | `"parquet"` or `"tsv"` |

**[config.output.summary]** — mirrors `.camdl` `output { summary { } }`:

| Field | Type | Description |
|---|---|---|
| `format` | string | `"tsv"` |

All `[config]` time values are bare floats in the model's declared
`time_unit`. The DSL converts `2 'years → 730.5` at compile time; the
experiment file uses the converted value directly.

### 4.5 Seeds

```toml
seeds = { from = 1, to = 1000 }        # range: 1, 2, ..., 1000
seeds = { list = [42, 137, 256] }       # explicit list
seeds = { n = 500, start = 1 }          # count from start: 1, 2, ..., 500
```

---

## 5. Scenarios

### 5.1 Where Scenarios Live

Scenarios are defined in the `.camdl` file's `scenarios { }` block (language
spec §18). This makes the model file self-contained — a reader sees the
model structure AND the counterfactual questions in one place.

```
# In the .camdl file
scenarios {
  with_sia {
    enable = [sia_round_1]
  }
  high_coverage {
    enable = [sia_round_1]
    set = { vacc_frac = min(vacc_frac * 1.2, 1.0) }
  }
}
```

The `.camdl` `scenarios { }` block has the full DSL expression parser, so
`set` values can be arbitrary expressions (including parameter references,
conditionals, and function calls). This is the expressive path for complex
scenario patches.

For single runs, scenarios are selected via CLI:

```bash
camdl simulate model.camdl --params p.toml --scenario with_sia --seed 42
```

### 5.2 Experiment File Scenarios (Merge-With-Override)

The experiment file can define additional scenarios. These are **merged**
with `.camdl` scenarios — not replaced:

- Experiment file scenarios are added to the `.camdl` scenario set
- On name conflict, the experiment file definition wins
- Non-conflicting `.camdl` scenarios remain available

```toml
# .camdl defines: with_sia, high_coverage
# experiment.toml defines: high_coverage (override), delayed_sia (new)
# Merged result: with_sia (from .camdl), high_coverage (from experiment),
#                delayed_sia (from experiment)
```

A note is emitted on override:

```
note: scenario 'high_coverage' in experiment.toml overrides 'high_coverage'
  from model.camdl. Using experiment file definition.
```

### 5.3 Scenario Operations

Each scenario is a named patch σ: M × C → M × C. The baseline scenario
(identity patch) is always available implicitly.

| Key | Type | Description |
|---|---|---|
| `enable` | list[string] | Enable named interventions from `.camdl` |
| `disable` | list[string] | Disable named interventions (from an all-on default) |
| `set` | table | Override parameter values with numeric literals |

`enable`/`disable` reference interventions in the `.camdl` file's
`interventions { }` block. Interventions must exist in the model.

**Intervention default state:** interventions are **disabled by default**
(the baseline identity patch fires no interventions). Scenarios explicitly
enable them with `enable = [...]`.

**`set` in `.camdl` `scenarios { }`:** values are scalar expressions
resolved at compile time (parameter references are allowed). **`set` in
experiment TOML `[scenarios]`:** numeric literals only (TOML limitation).

> **Not yet implemented:** `scale` (multiplicative factor) and `compose`
> (scenario composition) are planned for a future release. Define composed
> scenarios directly in the `.camdl` file's `scenarios { }` block for now.

### 5.4 Scenario Manifest in the IR

The compiler serializes scenario definitions into a top-level `"scenarios"`
field of the IR JSON. The simulation engine ignores this field — the IR
represents the fully resolved baseline model. The CLI reads the manifest
for `--scenario` dispatch:

1. `camdlc compile model.camdl` emits the IR (baseline) plus the scenario
   manifest
2. The Rust CLI reads the manifest when `--scenario NAME` is passed
3. The CLI applies the patch (filter interventions, override params) to the
   `Model` struct before constructing `CompiledModel`
4. `CompiledModel::new()` sees a model with the right interventions and
   param values — it doesn't know about scenarios

---

## 6. Parameter Files

### 6.1 Baseline Parameters (params.toml)

A point m ∈ M. One key-value pair per declared parameter:

```toml
beta = 0.3
gamma = 0.1
sigma = 0.2
rho = 0.4
k = 5.0
N0 = 1000000
I0 = 10
```

Every parameter declared in the `.camdl` file must have a value here (or a
default in the declaration). The CLI validates at load time and reports
missing parameters with their declared types.

### 6.2 Layered Parameters

```toml
[experiment]
params = ["params/base.toml", "params/fitted_2024.toml"]
```

Later files override earlier ones. `fitted_2024.toml` might contain only
`beta = 0.35` and `rho = 0.42`, overriding those from `base.toml`.

### 6.3 Indexed Parameter Overrides

For spatial models (`R0[patch]` expanding to 774 scalar parameters):

```tsv
R0_kano	2.1
R0_lagos	1.8
R0_sokoto	2.4
```

```bash
camdl simulate model.camdl --params p.toml --param-vec R0=r0_init.tsv
```

Matched by **parameter name** (not position). Errors if a constructed name
doesn't match, or if a matching parameter has no entry.

### 6.4 Priors and Views (v0.2+)

```toml
# priors.toml
[beta]
prior = "log_normal"
mu = 0.0
sigma = 1.0
transform = "log"
```

```toml
# view.toml — V from the parameter grammar
[view]
free = ["beta", "gamma", "rho", "I0"]
```

---

## 7. Comparisons

### 7.1 Pair Specification

```toml
[compare]
pairs = [
    ["baseline", "with_sia"],
    ["baseline", "high_coverage"],
    ["with_sia", "high_coverage"],    # non-baseline reference is valid
]
```

Each pair is `[reference, test]`. The keyword `baseline` refers to the
identity patch.

### 7.2 CRN Coupling

Paired scenarios are simulated with the same seed. The coupling guarantee
depends on the scenario type:

**Propensity-preserving scenarios** (`enable`/`disable` only, no `set`/`scale`):
Trajectories are **byte-identical** until the first enabled intervention fires.
Both scenarios have identical states, identical propensities, and consume
identical RNG draws up to that point. The paired difference after the
intervention captures the exact causal effect.

**Parameter-modifying scenarios** (`set`/`scale`): Trajectories are
**correlated but never identical**. Different parameter values produce different
propensities from t=0, so RNG draws map to different events. CRN still reduces
variance compared to independent seeds (the correlation comes from shared RNG
state), but there is no identical prefix. The variance reduction depends on
how much the parameter change affects propensities.

Both types produce valid paired comparisons — the CRN correlation ensures
that natural stochastic variation (weather, random timing) is shared across
scenarios, isolating the effect of the intervention or parameter change.

### 7.3 Derived Quantities

```toml
[compare.derived]
cases_averted = "reference.total_cases - test.total_cases"
relative_reduction = "cases_averted / reference.total_cases"
peak_reduction = "1 - test.peak_I / reference.peak_I"
abs_difference = "abs(reference.peak_I - test.peak_I)"
```

`reference.QUANTITY` and `test.QUANTITY` access summary values. `QUANTITY`
must match a name in the model's `output { summary { } }` block (language
spec §17). Previously-defined derived quantities can be referenced by name.

### 7.4 Derived Expression Language

Evaluated per-seed: for seed 42, `reference.total_cases` looks up
`(reference_scenario, seed=42)` in the summary.

**Grammar:**

```
derived_expr := derived_expr '+' derived_expr
              | derived_expr '-' derived_expr
              | derived_expr '*' derived_expr
              | derived_expr '/' derived_expr
              | FLOAT
              | IDENT                          # previously defined derived quantity
              | IDENT '.' IDENT               # reference.X or test.X
              | 'abs' '(' derived_expr ')'
              | 'min' '(' derived_expr ',' derived_expr ')'
              | 'max' '(' derived_expr ',' derived_expr ')'
              | '(' derived_expr ')'
```

Standard precedence (`*/` before `+-`). Three built-in functions. Each
expression evaluates to a scalar per seed.

**Evaluation order:** topological sort of the dependency graph. Cycles are
an error.

**Implementation:** ~120 lines of Rust in `derived_eval.rs`.

### 7.5 Dependency on Summary Post-Processing

Pipeline:

```
camdl experiment run         → trajectories per (scenario, seed)
camdl experiment summarize   → summary TSVs per scenario
camdl experiment compare     → comparison TSVs using derived expressions
```

Summary computation requires the model's `output { summary { } }` block
(language spec §17.3).

---

## 8. Content-Addressable Output Structure

### 8.1 Directory Layout

```
{output_dir}/
  manifest.json
  model.ir.json
  geo/boundaries.geojson          (if geo= specified)
  runs/
    {sim_hash_8}/                 # model + base params + backend + dt
      {scenario_slug}-{scen_hash_8}/   # scenario overrides
        seed_{seed}/              # individual run
          traj.tsv
          run.json
```

Example with two scenarios:

```
runs/3a7f2c1d/baseline-00000000/seed_1/
runs/3a7f2c1d/with_sia-f9e2b047/seed_1/
```

After tweaking `with_sia` only — baseline cache is untouched:

```
runs/3a7f2c1d/with_sia-d4e2a391/seed_1/   ← new
runs/3a7f2c1d/baseline-00000000/seed_1/   ← reused
```

After changing base params — nothing reused:

```
runs/cc8b1a90/baseline-00000000/seed_1/
runs/cc8b1a90/with_sia-f9e2b047/seed_1/
```

Scenario slug: scenario name lowercased, non-`[a-z0-9_]` replaced with `_`.
Seed directory: `seed_{N}` with verbatim u64, no zero-padding.

### 8.2 Hash Computation

**Sim hash** — full simulation identity (model + base params + backend + dt):

```
sim_hash = sha256(
    model_ir_json_bytes
    + canonical_sorted(base_param key=value pairs)
    + backend
    + dt
    + camdl_version
)   # full 64-char hex; first 8 used in dir name
```

`model_ir_json_bytes` is the compiled IR — it captures all structural
content: compartments, parameters, transitions, interventions, init, etc.

**Scenario hash** — scenario delta identity:

```
scen_hash = sha256(
    sorted(enable list)
    + sorted(disable list)
    + canonical_sorted(scenario param overrides)
)   # full 64-char hex; first 8 used in dir name
```

A scenario with no overrides, enables, or disables always hashes to the
same value (`sha256("") = ...`), producing `00000000` in the directory name.

`scen_hash` covers only the *delta* — the scenario's own enable/disable
lists and param overrides. Base params are already in `sim_hash`. Renaming
a scenario without changing its definition preserves `scen_hash` and
reuses cached runs.

### 8.3 Cache Reuse Matrix

| What changed | sim_hash | scen_hash | Reuse |
|---|---|---|---|
| IR / model | changes | — | none |
| base params | changes | — | none |
| backend or dt | changes | — | none |
| scenario A's enable/disable/params | unchanged | A changes, B same | B's runs reused |
| add more seeds | unchanged | unchanged | all existing reused |
| rename a scenario | unchanged | unchanged | reused (same sim) |

### 8.4 Manifest

`manifest.json` at the output root lists every completed run:

```json
{
  "runs": [
    { "scenario": "baseline", "seed": 1, "run_path": "3a7f2c1d/baseline-00000000/seed_1" },
    { "scenario": "with_sia", "seed": 1, "run_path": "3a7f2c1d/with_sia-f9e2b047/seed_1" }
  ]
}
```

The web app constructs trajectory URLs as `GET /runs/{run_path}/traj.tsv`.

---

## 9. Run-Level Metadata

Each run produces a `run.json`:

```json
{
  "sim_hash": "3a7f2c1d...",
  "scen_hash": "f9e2b047...",
  "scenario": "with_sia",
  "seed": 42,
  "backend": "tau_leap",
  "dt": 1.0,
  "camdl_version": "0.1.0",
  "model_file": "model.camdl",
  "params_file": "params.toml",
  "wall_time_seconds": 1.23,
  "timestamp": "2026-03-23T14:30:00Z"
}
```

Cache key: if the run directory (`runs/{sim_hash_8}/{slug}-{scen_hash_8}/seed_{N}/`)
already exists, the run is skipped. Pass `--force` to re-run.

---

## 10. Output File Schemas

### 10.1 Trajectories (trajectories.parquet)

One row per output time:

```
t: float64
{compartment_name}: int64/float64
```

### 10.2 Flows (flows.parquet)

One row per output time:

```
t: float64
{transition_name}: uint64            # flow count since previous output
```

### 10.3 Summary (analysis/summaries/{scenario}.tsv)

One row per seed. Columns from `output { summary { } }`:

```
seed	peak_I	total_cases	final_size
42	4523	89201	0.089
```

Computed post-simulation by `camdl experiment summarize`.

**Summary functions** (language spec §17.3):

```
max(expr)                  maximum value over all output times
min(expr)                  minimum value
cumulative(transition)     total firings over entire simulation
at(expr, timepoint)        value at specific time
```

**Valid timepoints for `at()`:**
- `t_start` — simulation start (reserved identifier from `simulate { from }`)
- `t_end` — simulation end (reserved identifier from `simulate { to }`)
- Numeric literal — `at(I, 365.0)` (in model time_unit)
- User-defined timepoints from `timepoints { }` block (v0.2+)

### 10.4 Diagnostics (diagnostics.tsv)

Per-transition firing statistics, written unconditionally:

```
transition_name	total_firings	mean_propensity	max_propensity	first_firing	last_firing
```

### 10.5 Ensembles (analysis/ensembles/{scenario}.parquet)

All seeds stacked:

```
seed: uint64
t: float64
{compartment_name}: int64/float64
```

### 10.6 Comparisons (analysis/comparisons/{ref}-vs-{test}.tsv)

One row per seed:

```
seed	cases_averted	relative_reduction	peak_reduction
```

---

## 11. CLI Commands

### 11.1 Single Run (No Experiment File)

```bash
# Baseline
camdl simulate model.camdl --params params.toml --seed 42

# Named scenario from .camdl
camdl simulate model.camdl --params params.toml --scenario with_sia --seed 42

# Ad-hoc intervention toggle
camdl simulate model.camdl --params params.toml --enable sia_round_1 --seed 42

# Parameter override (M layer — always valid, with or without scenario)
camdl simulate model.camdl --params params.toml --param beta=0.5 --seed 42
camdl simulate model.camdl --params params.toml --scenario with_sia --param beta=0.5 --seed 42

# Content-addressable output
camdl simulate model.camdl --params params.toml --seed 42 --output-dir output/
```

**Flag rules:**
- `--scenario` and `--enable`/`--disable` are **mutually exclusive** (both σ)
- `--param` is **always valid** (M layer, independent of σ)
- Without `--output-dir`, output goes to stdout as TSV

### 11.2 Experiment Execution

```bash
camdl experiment run EXPERIMENT.toml
camdl experiment run EXPERIMENT.toml --force
camdl experiment run EXPERIMENT.toml --scenario with_sia
camdl experiment run EXPERIMENT.toml --scenario with_sia --seed 42
camdl experiment run EXPERIMENT.toml --resume
```

`--parallel N` runs N simulations concurrently. File writes are atomic.

### 11.3 Post-Processing

```bash
camdl experiment summarize EXPERIMENT.toml
camdl experiment compare EXPERIMENT.toml
camdl experiment status EXPERIMENT.toml
```

Pipeline: `run → summarize → compare`.

### 11.4 Verification

```bash
camdl experiment verify EXPERIMENT.toml    # check run.json hashes
camdl experiment check EXPERIMENT.toml     # detect stale results
```

---

## 12. Caching, Staleness, and Provenance

### 12.1 Cache Hit

A run is a cache hit when
`runs/{sim_hash_8}/{scenario_slug}-{scen_hash_8}/seed_{N}/` already exists.
The directory path encodes all inputs: changing the model, base params,
backend, dt, or camdl version changes `sim_hash_8`; changing a scenario's
enable/disable lists or param overrides changes `scen_hash_8`.

Scenario changes only invalidate runs for that scenario — other scenarios'
directories are unaffected.

### 12.2 Provenance Guarantees

1. **Self-contained.** `manifest.json` + `model.ir.json` describe the analysis completely.
2. **Content-addressed.** `sim_hash` and `scen_hash` in `run.json` audit all inputs.
3. **Version-stamped.** Different camdl versions → different `sim_hash` → different directories.
4. **Deterministic.** Same inputs → byte-identical output (single platform).

**Not guaranteed:** cross-machine floating-point reproducibility.

---

## 13. Relationship to the Parameter Grammar

| Grammar concept | Implementation |
|---|---|
| **Sim(m, c, s) → Y** | One cell: `runs/{scenario}/seed-{s}/` |
| **M** | `parameters { }` in `.camdl` |
| **C** | `.camdl` structure + `[config]` overrides |
| **S** | `seeds` in experiment or `--seed` on CLI |
| **Point m ∈ M** | `params.toml` (layered) |
| **Scenario σ** | `scenarios { }` in `.camdl` (merged with experiment) |
| **Baseline σ₀** | Identity patch |
| **σ₂ ∘ σ₁** | `compose = ["A", "B"]` |
| **M override** | `--param` CLI flag |
| **View V** | `view.toml` (v0.2+) |
| **Transform T_V** | `priors.toml` (v0.2+) |

The downward chain:

```
z ∈ Z_V       inference engine proposes a vector
  │ T_V⁻¹     back-transform (exp, expit)
  ▼
p ∈ P_V       free parameter values
  │ κ_V       fill in fixed values from params.toml
  ▼
m ∈ M         complete parameter set
  │ --param   CLI overrides (if any)
  ▼
m' ∈ M        parameter set with CLI overrides
  │ σ         apply scenario patch
  ▼
(m'', c')     patched parameters + configuration
  │ Sim(·,·,s)
  ▼
y ∈ Y         trajectory → runs/{scenario}/seed-{s}/
```

---

## 14. Implementation Phases

### v0.1-core: Single-run CLI

- `camdl simulate MODEL --params FILE --seed N` → TSV to stdout
- `--backend`, `--dt`, `--param`, `--param-vec`, `--params` flags
- Diagnostics TSV written unconditionally

### v0.1-scenarios: Scenario support *(implemented)*

- `.camdl` `scenarios { }` block: `enable`/`disable`/`set` operations
- Scenario manifest serialized in IR JSON `"presets"` field
- `--scenario NAME` on `camdl simulate` (lookup from manifest)
- `--enable NAME` / `--disable NAME` ad-hoc scenario flags
- Mutual exclusion: `--scenario` vs `--enable`/`--disable`
- `--param` always valid alongside either mode
- Baseline = no interventions; scenarios explicitly enable them

### v0.1-batch: Experiment execution

- `camdl experiment run` with scenario × seed iteration
- `[config]` loading with closed schema validation
- `[config.simulate]` and `[config.output.*]` override merging
- Scenario merge-with-override (experiment + `.camdl`)
- Scenario `enable`/`disable` wiring (filter model interventions)
- Frozen experiment.toml (only for experiment runs)
- `--parallel N`, `--resume`, `camdl experiment status`

### v0.1-post: Post-processing

- Summary computation from trajectory parquets (`output { summary { } }`)
- `at(expr, timepoint)` with `t_start`, `t_end`, numeric literals
- Ensemble stacking
- Derived expression evaluator (~120 lines Rust)
- `camdl experiment summarize`, `compare`, `verify`, `check`

### v0.2: Inference + timepoints

- `view.toml`, `priors.toml`
- Scoring primitive
- Particle filter, IF2
- `camdl fit`
- User-defined `timepoints { }` block wired through expander
- `scale` and `compose` scenario operations
