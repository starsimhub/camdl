# camdl Experiment & Output System Specification

**Version:** 0.3-draft
**Date:** 2026-03-17

This document specifies the experiment file format, content-addressable output
structure, run management, and provenance guarantees for camdl. It implements
the parameter grammar (Buffalo 2026) at the systems level.

---

## 1. Design Principles

**One file per concern, no overlap.**

```
.camdl         → what the model IS (structure + default C + scenarios)
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

---

## 2. File Roles and Separation of Concerns

### 2.1 What Goes Where

```
┌──────────────────────┬──────────────────────┬───────────────────────────┐
│       Concern        │     Lives in         │     Can override?         │
├──────────────────────┼──────────────────────┼───────────────────────────┤
│ Model structure      │ .camdl only          │ never                     │
│ Parameter names/types│ .camdl only          │ never                     │
│ Parameter values     │ params.toml only     │ layered .toml, --param, σ │
│ Scenarios            │ .camdl scenarios { } │ experiment file (wins)    │
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
scenario patches                 (counterfactual modifications)
  ↓ overridden by
--param CLI flags                (convenience, non-persistent)
```

**For scenarios:**

```
.camdl scenarios { }             (model-level definitions)
  ↓ overridden by
experiment.toml [scenarios]      (experiment-level, if present — fully replaces .camdl scenarios)
```

If the experiment file has a `[scenarios]` section, the `.camdl` scenarios
are ignored entirely. The experiment file wins. A warning is emitted if both
define a scenario with the same name:

```
note: scenario 'with_sia' in experiment.toml shadows 'with_sia' from model.camdl
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

# Enable a specific intervention (shorthand for a scenario)
camdl simulate model.camdl --params params.toml --enable sia_round_1 --seed 42

# Override a parameter
camdl simulate model.camdl --params params.toml --param beta=0.5 --seed 42
```

`--scenario NAME` looks up the scenario in the `.camdl` file's
`scenarios { }` block. Error if not found.

`--enable NAME` is shorthand for a scenario that enables the named
intervention. It's equivalent to a scenario `{ enable = [NAME] }`.

### 3.2 With Content-Addressable Output

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

The `run.json` contains full provenance (model file path, params files,
seed, backend, scenario, camdl version). This is sufficient to reproduce
the run.

If a scenario is used:

```bash
camdl simulate model.camdl --params params.toml --scenario with_sia --seed 42 --output-dir output/
```

```
output/
  model-{hash}/
    config-{hash}/
      params.toml
      runs/
        with_sia/
          seed-42/
            ...
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

# ── Scenarios (optional — falls back to .camdl scenarios) ──

[scenarios.baseline]
# Identity patch — no modifications.

[scenarios.with_sia]
enable = ["sia_round_1"]

[scenarios.high_coverage]
enable = ["sia_round_1"]
set = { vacc_frac = 0.95 }

[scenarios.delayed_sia]
enable = ["sia_round_1"]
set = { sia_time = 365.0 }

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
all fields have defaults. Unknown keys produce a clear error directing the
user to the correct location:

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

**[config.simulate]** — mirrors `.camdl` `simulate { }` block:

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

The mirroring is exact: same field names in both places. The only difference
is that `.camdl` uses unit literals (`7 'days`) while experiment TOML uses
bare floats in the model's declared time_unit. The DSL converts
`2 'years → 730.5` at compile time; the experiment file uses the converted
value directly.

### 4.5 Seeds

Three forms:

```toml
seeds = { from = 1, to = 1000 }        # range: 1, 2, ..., 1000
seeds = { list = [42, 137, 256] }       # explicit list
seeds = { n = 500, start = 1 }          # count from start: 1, 2, ..., 500
```

---

## 5. Scenarios

### 5.1 Where Scenarios Live

Scenarios are defined in the `.camdl` file's `scenarios { }` block (language
spec §18). This makes the model file self-contained — a reader can see the
model structure AND the counterfactual questions in one place.

```
# In the .camdl file
scenarios {
  with_sia {
    enable = [sia_round_1]
  }
  high_coverage {
    enable = [sia_round_1]
    set = { vacc_frac = 0.95 }
  }
}
```

For single runs, scenarios are selected via CLI:

```bash
camdl simulate model.camdl --params p.toml --scenario with_sia --seed 42
```

For batch experiments, the experiment file can optionally define its own
scenarios. If `[scenarios]` is present in the experiment file, it **fully
replaces** the `.camdl` scenarios (no merging).

### 5.2 Scenario Operations

Each scenario is a named patch σ: M × C → M × C. The baseline scenario
(identity patch) is always available implicitly.

| Key | Type | Description |
|---|---|---|
| `enable` | list[string] | Enable named interventions from `.camdl` |
| `disable` | list[string] | Disable named interventions |
| `set` | table | Override parameter values with **numeric literals** |
| `scale` | table | Multiply parameter values by a **numeric factor** |
| `compose` | list[string] | Apply named scenarios in sequence |

`enable`/`disable` reference interventions defined in the `.camdl` file's
`interventions { }` block. Interventions must exist in the model; scenarios
only control which are active.

**`set` and `scale` take numeric literals only.** No expressions, no
parameter references, no conditionals. This is a TOML limitation — TOML
values are typed literals. For expression-based scenario patches (e.g.,
`vacc_rate = min(vacc_rate * 1.2, 1.0)`), use the `.camdl` file's
`scenarios { }` block, which has the full DSL expression parser.

```toml
# Valid — numeric literals only
[scenarios.high_coverage]
set = { vacc_frac = 0.95, sia_time = 365.0 }

[scenarios.more_transmissible]
scale = { beta = 1.5 }
```

`scale` on a `probability` parameter that would exceed [0,1] is an error.

### 5.3 Composition and Overlap Detection

Scenarios compose left-to-right: `compose = ["A", "B"]` means σ_B ∘ σ_A.
The result of A is the input to B.

The compiler computes write sets for each scenario. If two composed
scenarios have overlapping write sets, it warns:

```
warning[W300]: scenarios 'A' and 'B' both modify parameter 'beta'.
  Composition order matters: 'A' is applied first, then 'B'.
```

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

The experiment file supports multiple params files:

```toml
[experiment]
params = ["params/base.toml", "params/fitted_2024.toml"]
```

Later files override earlier ones. `fitted_2024.toml` might contain only
`beta = 0.35` and `rho = 0.42`, overriding those from `base.toml`.

### 6.3 Indexed Parameter Overrides

For spatial models (`R0[patch]` expanding to 774 scalar parameters), a
keyed TSV file provides bulk overrides:

```tsv
R0_kano	2.1
R0_lagos	1.8
R0_sokoto	2.4
```

```bash
camdl simulate model.camdl --params params.toml --param-vec R0=r0_init.tsv
```

Matched by **parameter name** (not position). Errors if a constructed name
doesn't match any parameter, or if a matching parameter has no entry.

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
# view.toml — implements V from the parameter grammar
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
identity patch. For each pair and each seed, both scenarios are simulated
with the same seed (CRN coupling — same seed produces identical
trajectories up to the point where the scenario patch causes divergence;
see §12.1).

### 7.2 Derived Quantities

```toml
[compare.derived]
cases_averted = "reference.total_cases - test.total_cases"
relative_reduction = "cases_averted / reference.total_cases"
peak_reduction = "1 - test.peak_I / reference.peak_I"
abs_difference = "abs(reference.peak_I - test.peak_I)"
```

`reference.QUANTITY` and `test.QUANTITY` access summary values from the
reference and test scenarios. `QUANTITY` must match a name declared in the
model's `output { summary { ... } }` block (language spec §17).

Previously-defined derived quantities can be referenced by name.

### 7.3 Derived Expression Language

Derived expressions are evaluated per-seed: for seed 42,
`reference.total_cases` looks up the summary row for
`(reference_scenario, seed=42)`.

**Grammar:**

```
derived_expr := derived_expr '+' derived_expr
              | derived_expr '-' derived_expr
              | derived_expr '*' derived_expr
              | derived_expr '/' derived_expr
              | FLOAT
              | IDENT                          # previously defined derived quantity
              | IDENT '.' IDENT               # reference.total_cases or test.peak_I
              | 'abs' '(' derived_expr ')'
              | 'min' '(' derived_expr ',' derived_expr ')'
              | 'max' '(' derived_expr ',' derived_expr ')'
              | '(' derived_expr ')'
```

Standard precedence (`*/` before `+-`). Three built-in functions: `abs`,
`min`, `max`. No conditionals, no compartment references, no
time-dependence. Each expression evaluates to a scalar per seed.

**Evaluation order:** topological sort of the dependency graph. If
`relative_reduction` references `cases_averted`, `cases_averted` is
evaluated first regardless of declaration order. Cycles are an error.

**Namespace at evaluation time:**

```
reference.X  → summary value X from the reference scenario
test.X       → summary value X from the test scenario
IDENT        → a previously evaluated derived quantity
FLOAT        → literal constant
```

**Implementation:** ~120 lines of Rust in `derived_eval.rs`. A
`DerivedExpr` enum, a recursive descent parser, and an `eval` function
taking a `HashMap<String, f64>` environment.

### 7.4 Dependency on Summary Post-Processing

Derived comparisons require that summary values have been computed. The
pipeline:

```
camdl experiment run         → trajectories per (scenario, seed)
camdl experiment summarize   → summary TSVs per scenario (one row per seed)
camdl experiment compare     → comparison TSVs using derived expressions
```

Summary computation is post-processing: read trajectory parquet, evaluate
summary expressions from the model's `output { summary { } }` block
(language spec §17.3), write one row per seed to
`analysis/summaries/{scenario}.tsv`.

---

## 8. Content-Addressable Output Structure

### 8.1 Directory Layout

```
{output_dir}/
  model-{model_hash}/
    config-{config_hash}/
      experiment.toml              # frozen copy (only if experiment file used)
      params.toml                  # frozen merged baseline parameters
      runs/
        baseline/
          seed-1/
            trajectories.parquet
            flows.parquet
            diagnostics.tsv
            run.json
          seed-2/
            ...
        with_sia/
          seed-1/
            ...
      analysis/
        summaries/
          baseline.tsv
          with_sia.tsv
        ensembles/
          baseline.parquet
          with_sia.parquet
        comparisons/
          baseline-vs-with_sia.tsv
```

### 8.2 Structural Separation

Two reserved top-level directories under each config hash:

- **`runs/`** — scenario directories with seed subdirectories
- **`analysis/`** — post-processing outputs (summaries, ensembles, comparisons)

Scenario names live inside `runs/`. Even if a user names a scenario
`analysis`, it appears as `runs/analysis/seed-42/` — no collision.

Scenario names are identifiers (`[a-zA-Z_][a-zA-Z0-9_]*`). Seed
directories use `seed-N` format (hyphen delimiter, visually distinct from
underscore-heavy scenario names).

### 8.3 Hash Computation

**Model hash** — structural identity of the `.camdl` file:

```
model_hash = sha256(structural_content)[:12]
```

Structural content (from language spec §20.1): compartments, stratify,
parameters (names and types), tables (structure + inline values), let
bindings, functions, transitions, observations, ode, interventions (full
specification including timing and magnitude), init, time_unit.

Excluded: simulate, output, scenarios, data file paths.

**Config hash** — full analysis identity:

```
config_hash = sha256(
    model_hash
    + sorted(merged param key=value pairs)
    + sorted(data file content hashes)
    + backend
    + dt (if applicable)
    + camdl_version
)[:12]
```

The `camdl_version` ensures different software versions get separate
output directories.

### 8.4 Frozen Inputs

```
config-{hash}/
  experiment.toml    # frozen copy (only when experiment file is used)
  params.toml        # frozen merged baseline (always)
```

The frozen `experiment.toml` records the original file paths for
traceability. The frozen `params.toml` is the resolved merged values —
the authoritative record for reproduction. Original files may be edited;
frozen copies never change.

When `camdl simulate --output-dir` is used without an experiment file,
only `params.toml` is frozen (there is no experiment file to copy).

---

## 9. Run-Level Metadata

Each run produces a `run.json`:

```json
{
  "model_hash": "a3f8b2c1d4e5",
  "config_hash": "7c9a1b2e3f4d",
  "input_hash": "e4f5a6b7c8d9",
  "scenario": "with_sia",
  "seed": 42,
  "backend": "gillespie",
  "camdl_version": "0.3.0",
  "model_file": "models/seir_nigeria.camdl",
  "params_files": ["params/base.toml", "params/fitted_2024.toml"],
  "wall_time_seconds": 1.23,
  "timestamp": "2024-11-15T14:30:00Z",
  "transitions_fired": 1482301,
  "zero_firing_transitions": 0
}
```

**Input hash** uniquely identifies one cell:

```
input_hash = sha256(config_hash + scenario_name + seed)[:12]
```

Cache key: if `run.json` exists with matching `input_hash`, the run is
skipped.

---

## 10. Output File Schemas

### 10.1 Trajectories (trajectories.parquet)

One row per output time:

```
t: float64
{compartment_name}: int64/float64    # one per expanded compartment
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

One row per seed, columns from `[compare.derived]`:

```
seed	cases_averted	relative_reduction	peak_reduction
```

---

## 11. CLI Commands

### 11.1 Single Run (No Experiment File)

```bash
# Baseline
camdl simulate model.camdl --params params.toml --seed 42

# Named scenario from .camdl file
camdl simulate model.camdl --params params.toml --scenario with_sia --seed 42

# Enable a specific intervention
camdl simulate model.camdl --params params.toml --enable sia_round_1 --seed 42

# Parameter override
camdl simulate model.camdl --params params.toml --param beta=0.5 --seed 42

# With content-addressable output
camdl simulate model.camdl --params params.toml --seed 42 --output-dir output/
```

Without `--output-dir`, output goes to stdout as TSV.

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
camdl experiment check EXPERIMENT.toml     # check for stale results
```

---

## 12. Caching, Staleness, and Provenance

### 12.1 CRN Coupling

Paired scenario comparison uses Common Random Numbers: both scenarios run
with the same seed, producing identical trajectories up to the point where
the scenario patch causes state divergence (typically an intervention time).
Pre-intervention trajectories are byte-identical because the same seed
produces the same sequential RNG draws when states and propensities match.
Post-intervention, trajectories diverge naturally as states differ.

This gives valid paired comparisons without per-event keying overhead. The
treatment effect (cases_averted, etc.) is computed per-seed, and the
distribution across seeds quantifies uncertainty.

### 12.2 Cache Hit

A run is a cache hit when `runs/{scenario}/seed-{N}/run.json` exists and
its `input_hash` matches `sha256(config_hash + scenario + seed)[:12]`.

### 12.3 Provenance Guarantees

1. **Self-contained.** Frozen files in the output directory describe the
   analysis completely.
2. **Tamper-evident.** `input_hash` is a function of all inputs.
3. **Version-stamped.** Different camdl versions → different directories.
4. **Deterministic.** Same inputs → byte-identical output (single platform).

**Not guaranteed:** data file immutability (content hashes detect changes
but frozen copies don't include large external data); cross-machine
floating-point reproducibility.

---

## 13. Relationship to the Parameter Grammar

| Grammar concept | Implementation |
|---|---|
| **Sim(m, c, s) → Y** | One cell: `runs/{scenario}/seed-{s}/` |
| **M** | `parameters { }` in `.camdl` |
| **C** | `.camdl` structure + `[config]` overrides |
| **S** | `seeds` in experiment or `--seed` on CLI |
| **Point m ∈ M** | `params.toml` (layered) |
| **Scenario σ** | `scenarios { }` in `.camdl` or `[scenarios]` in experiment |
| **Baseline σ₀** | Identity patch |
| **σ₂ ∘ σ₁** | `compose = ["A", "B"]` |
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
  │ σ         apply scenario patch
  ▼
(m', c')      patched parameters + configuration
  │ Sim(·,·,s)
  ▼
y ∈ Y         trajectory → runs/{scenario}/seed-{s}/
```

---

## 14. Implementation Phases

### v0.1-core: Content-addressable output + single-run

- Hash computation (model hash, config hash, input hash)
- Directory creation with `runs/` and `analysis/` separation
- `run.json` metadata with `params_files` field
- Frozen params.toml
- `--output-dir` on `camdl simulate`
- `--scenario NAME` on `camdl simulate` (lookup from `.camdl` scenarios)
- Cache hit detection

### v0.1-batch: Experiment execution

- `camdl experiment run` with scenario × seed iteration
- `[config]` loading with closed schema validation
- `[config.simulate]` and `[config.output.*]` override merging
- Scenario `enable`/`disable` wiring (filter model interventions)
- Frozen experiment.toml (only for experiment runs)
- `--parallel N`, `--resume`, `camdl experiment status`

### v0.1-post: Post-processing

- Summary computation from trajectory parquets (`output { summary { } }`)
- `at(expr, timepoint)` with `t_start`, `t_end`, numeric literals
- Ensemble stacking
- Derived expression evaluator (~120 lines Rust)
- `camdl experiment summarize`, `compare`, `verify`, `check`

### v0.2: Inference

- `view.toml`, `priors.toml`
- Scoring primitive
- Particle filter, IF2
- `camdl fit`
