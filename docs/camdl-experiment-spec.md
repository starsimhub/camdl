# camdl Experiment & Output System Specification

**Version:** 0.5-draft
**Date:** 2026-03-25

> **Scope note:** This spec provides infrastructure for parameter exploration
> and sensitivity analysis. The `[design.*]` system characterizes how model
> sensitivity varies across belief states. Decision-theoretic value of
> information (EVSI) — combining these outputs with a decision problem, prior
> distribution, and utility function — is specified separately in the VOI
> Specification, which consumes this spec's outputs.

---

## 1. Design Principles

**One file per concern, no overlap.**

```
.camdl           → what the model IS (structure, scenarios, default C)
params.toml      → what the values ARE (point m ∈ M)
experiment.toml  → how the analysis RUNS (sweep/design, seeds, comparisons)
```

Each file owns its domain exclusively. The experiment file cannot set parameter
values — that's what params.toml is for. The params file cannot set the backend
— that's what the experiment file is for. The .camdl file cannot set concrete
parameter values — that's what external files are for.

**The model file is self-contained for single runs.** A `.camdl` file with a
`params.toml` and a seed is everything needed for one run — no experiment file
required. The experiment file adds batch infrastructure on top.

**Mirrored structure for overrides.** When the experiment file overrides a
`.camdl` block, it uses the same field names in the same hierarchy. If you know
the `.camdl` syntax, you know the experiment file syntax.

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
┌──────────────────────┬───────────────────────┬───────────────────────────┐
│       Concern        │     Lives in          │     Can override?         │
├──────────────────────┼───────────────────────┼───────────────────────────┤
│ Model structure      │ .camdl only           │ never                     │
│ Parameter names/types│ .camdl only           │ never                     │
│ Parameter values (M) │ params.toml only      │ layered .toml, --param, σ │
│ Scenarios (σ)        │ .camdl scenarios { }  │ experiment merges on top  │
│ Interventions        │ .camdl only           │ scenarios enable/disable  │
│ Backend choice       │ experiment [config]   │ CLI --backend             │
│ Time range           │ .camdl default        │ experiment [config], CLI  │
│ Output schedule      │ .camdl default        │ experiment [config]       │
│ Seeds                │ experiment [config]   │ CLI --seed                │
│ Sweep / Design       │ experiment only       │ never                     │
│ Comparisons          │ experiment only       │ never                     │
└──────────────────────┴───────────────────────┴───────────────────────────┘
```

No row has entries in both "params.toml" and "experiment [config]" — they own
completely disjoint domains.

### 2.2 Precedence Chains

**For C (configuration):**

```
.camdl simulate/output blocks     (structural defaults)
  ↓ overridden by
experiment.toml [config]          (analysis-specific)
  ↓ overridden by
CLI flags                         (convenience, non-persistent)
```

**For M (parameters):**

```
params.toml (first file)          (base values)
  ↓ overridden by
params.toml (later files)         (layered patches)
  ↓ overridden by
sweep / design point overrides    (automated M-layer variation)
  ↓ overridden by
scenario patches (set/scale)      (counterfactual modifications to M)
  ↓ overridden by
--param CLI flags                 (convenience, non-persistent)
```

**For scenarios:**

```
.camdl scenarios { }              (model-level definitions)
  ↓ merged with
experiment.toml [[scenario]]     (experiment adds/overrides by name)
```

On name conflict, the experiment file definition wins (with a note).

---

## 3. Single-Run Workflow (No Experiment File)

The `.camdl` file + `params.toml` is a complete, self-contained specification
for running a model. No experiment file is needed for exploration.

### 3.1 Basic Run

```bash
# Baseline (no scenario), output to stdout
camdl simulate model.camdl --params params.toml --seed 42

# Named scenario from .camdl
camdl simulate model.camdl --params params.toml --scenario with_sia --seed 42

# Ad-hoc intervention toggle (no named scenario)
camdl simulate model.camdl --params params.toml --enable sia_round_1 --seed 42

# Parameter override (M layer — always valid, with or without scenarios)
camdl simulate model.camdl --params params.toml --param beta=0.5 --seed 42

# Scenario + parameter override (σ layer + M layer — both valid)
camdl simulate model.camdl --params params.toml --scenario with_sia --param beta=0.5 --seed 42
```

### 3.2 CLI Flag Rules

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
camdl simulate model.camdl --params p.toml --enable sia_round_1 --param beta=0.5 --seed 42
```

The `--param` override applies **after** the scenario patch in the M precedence
chain: `params.toml → scenario set/scale → --param`.

### 3.3 CLI Parameter Flags

```bash
--params FILE              # load parameter file (can repeat for layering)
--param NAME=VALUE         # override a single parameter (can repeat)
--param-vec PREFIX=FILE    # override indexed params from keyed TSV
```

Without `--output-dir`, output goes to stdout as TSV.

### 3.4 With Content-Addressable Output

```bash
camdl simulate model.camdl --params params.toml --seed 42 --output-dir output/
```

Creates the content-addressable directory structure (§8) for a single run. No
experiment file needed — content addressing works at single-run granularity too.

---

## 4. The Experiment File

### 4.1 Format

TOML. Used only for batch analysis (multiple scenarios × seeds, parameter
sweeps, experimental designs, and comparisons).

### 4.2 Full Example

```toml
# experiment.toml — Nigeria SIA evaluation 2024

[experiment]
name = "Nigeria SIA evaluation 2024"
model = "models/seir_nigeria.camdl"
params = ["params/base.toml", "params/fitted_2024.toml"]

[config]
backend = "gillespie"
dt = 1.0
seeds = { n = 1000 }
parallel = 16
output_dir = "output"

[config.simulate]
from = 0.0
to = 730.0

[config.output.trajectories]
every = 7.0
format = "parquet"

[config.output.flows]
every = 7.0
format = "parquet"

[config.output.summary]
format = "tsv"

# ── Parameter sweep (M layer) ──────────────────────────
[sweep]
vacc_eff = { linspace = { min = 0.1, max = 0.9, n = 9 } }

# ── Scenarios (σ layer) ────────────────────────────────
[[scenario]]
name = "baseline"

[[scenario]]
name = "with_sia"
enable = ["sia"]

# ── Comparisons ────────────────────────────────────────
[compare]
pairs = [
  ["baseline", "with_sia"],
]

[compare.derived]
cases_averted = "reference.total_cases - test.total_cases"
relative_reduction = "cases_averted / reference.total_cases"

# Total runs: 9 sweep × 2 scenarios × 1000 seeds = 18,000
```

### 4.3 [experiment] Fields

| Field    | Type         | Required | Description                                        |
| -------- | ------------ | -------- | -------------------------------------------------- |
| `name`   | string       | yes      | Human-readable experiment name                     |
| `model`  | path         | yes      | Path to `.camdl` model file                        |
| `params` | path or list | yes      | Path(s) to params.toml. List = layered, later wins |

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

**Top-level config keys:**

| Field        | Type   | Default       | Description                                                             |
| ------------ | ------ | ------------- | ----------------------------------------------------------------------- |
| `backend`    | string | `"gillespie"` | `gillespie`, `tau_leap`, `chain_binomial`                               |
| `dt`         | float  | `1.0`         | Step size for discrete-time backends                                    |
| `seeds`      | table  | required      | `{ n = N }` or `{ from, to }` or `{ list = [...] }` or `{ n, start }` |
| `parallel`   | int    | `1`           | Concurrent runs                                                         |
| `output_dir` | path   | `"output/"`   | Root of content-addressable tree                                        |

**[config.simulate]** — mirrors `.camdl` `simulate { }`:

| Field  | Type  | Description                                |
| ------ | ----- | ------------------------------------------ |
| `from` | float | Simulation start time (in model time_unit) |
| `to`   | float | Simulation end time (in model time_unit)   |

**[config.output.trajectories]** and **[config.output.flows]**:

| Field    | Type   | Description                          |
| -------- | ------ | ------------------------------------ |
| `every`  | float  | Output interval (in model time_unit) |
| `format` | string | `"parquet"` or `"tsv"`               |

**[config.output.summary]**:

| Field    | Type   | Description |
| -------- | ------ | ----------- |
| `format` | string | `"tsv"`     |

All `[config]` time values are bare floats in the model's declared `time_unit`.

### 4.5 Seeds

```toml
seeds = { n = 1000 }                # 1, 2, ..., 1000
seeds = { from = 1, to = 1000 }     # range: 1..1000
seeds = { list = [42, 137, 256] }   # explicit list
seeds = { n = 500, start = 1 }      # count from start: 1, 2, ..., 500
```

---

## 5. Scenarios

### 5.1 Where Scenarios Live

Scenarios are defined in the `.camdl` file's `scenarios { }` block. This makes
the model file self-contained — structure and counterfactual questions in one
place.

```
# In the .camdl file
scenarios {
  with_sia {
    enable = [sia]
  }
  high_coverage {
    enable = [sia]
    set = { vacc_eff = 0.95 }
  }
}
```

For single runs, select via CLI: `--scenario with_sia`.

### 5.2 Experiment File Scenarios (Merge-With-Override)

The experiment file can define additional scenarios. These are **merged** with
`.camdl` scenarios:

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

| Key       | Type         | Description                                          |
| --------- | ------------ | ---------------------------------------------------- |
| `enable`  | list[string] | Enable named interventions from `.camdl`             |
| `disable` | list[string] | Disable named interventions (from an all-on default) |
| `set`     | table        | Override parameter values (numeric literals)         |
| `scale`   | table        | Multiply parameter values by factor                  |
| `compose` | list[string] | Apply scenarios in sequence                          |

Interventions are **disabled by default**. Scenarios explicitly enable them.

### 5.4 Scenario × Sweep/Design Interaction

Sweeps (§6) and designs (§7) are orthogonal to scenarios. Their cross product
defines the full run grid:

```
Total runs = |parameter points| × |scenarios| × |seeds|
```

Each (parameter_point, scenario) combination is a synthetic effective scenario.
Parameter point overrides apply first (M layer), then scenario patches apply on
top (σ layer).

### 5.5 Scenario Manifest in the IR

The compiler serializes scenario definitions into the IR JSON `"scenarios"` field.
The simulation engine ignores this field. The CLI reads it for `--scenario` dispatch:

1. Compile `.camdl` → IR (baseline) + scenario manifest
2. CLI reads manifest when `--scenario NAME` is passed
3. CLI applies the patch (filter interventions, override params) to the `Model`
   struct before constructing `CompiledModel`
4. `CompiledModel::new()` sees a model with the right interventions and param
   values — it doesn't know about scenarios

---

## 6. Parameter Sweep

`[sweep]` defines a deterministic parameter grid. Each parameter gets a set of
values; the Cartesian product of all swept parameters defines the grid.

### 6.1 Sweep Value Specifications

```toml
[sweep]
# Explicit list
vacc_eff = [0.1, 0.3, 0.5, 0.7, 0.9]

# Evenly spaced
vacc_eff = { linspace = { min = 0.1, max = 0.9, n = 9 } }

# Log-spaced (useful for rates, R0)
kappa = { logspace = { min = 0.001, max = 0.1, n = 5 } }

# Range with step
R0 = { range = { min = 1.0, max = 5.0, step = 0.5 } }
```

| Generator  | Parameters              | Description               |
| ---------- | ----------------------- | ------------------------- |
| (bare list)| —                       | Explicit values           |
| `linspace` | `min`, `max`, `n`       | n evenly-spaced points    |
| `logspace` | `min`, `max`, `n`       | n log-spaced points       |
| `range`    | `min`, `max`, `step`    | Step from min toward max  |

All generator args are keyword — no positional ambiguity.

### 6.2 Factorial Sweeps

Multiple swept parameters produce a Cartesian product:

```toml
[sweep]
vacc_eff = { linspace = { min = 0.1, max = 0.9, n = 9 } }
kappa    = { logspace = { min = 0.001, max = 0.1, n = 5 } }
# 9 × 5 = 45 parameter points × scenarios × seeds
```

### 6.3 Sweep Points Are M-Layer Overrides

Each sweep point is equivalent to `--param vacc_eff=0.3` added to every run at
that grid coordinate. Sweep values apply **before** scenario patches in the M
precedence chain. Sweep parameter values fold into `scen_hash` (§8.2).

### 6.4 `[sweep]` and `[design.*]` Are Mutually Exclusive

A single experiment file uses either `[sweep]` (deterministic grid) or
`[design.*]` (space-filling designs) — not both. They answer different
questions: sweeps test specific values, designs characterize sensitivity
across a parameter space.

---

## 7. Experimental Design and Sensitivity Analysis

### 7.1 Overview

`[design.*]` blocks define named **belief states** — parameter ranges
representing "what we currently know." Each design generates a space-filling
sample of the parameter space. Comparing sensitivity indices across designs
characterizes how model sensitivity varies across belief states: which
parameters drive output uncertainty shifts as knowledge narrows?

### 7.2 Named Designs

Each design is a named section with its own method, sample size, and parameter
ranges:

```toml
# Design A: current knowledge — wide uncertainty
[design.current]
method = "sobol"
n = 1024

[design.current.parameters.vacc_eff]
range = { min = 0.1, max = 0.9 }

[design.current.parameters.R0]
range = { min = 1.0, max = 5.0 }

[design.current.parameters.kappa]
range = { min = 0.001, max = 0.1 }
transform = "log"

# Design B: "what if we had better coverage data?"
[design.better_coverage]
method = "sobol"
n = 1024

[design.better_coverage.parameters.vacc_eff]
range = { min = 0.6, max = 0.8 }

[design.better_coverage.parameters.R0]
range = { min = 1.0, max = 5.0 }

[design.better_coverage.parameters.kappa]
range = { min = 0.001, max = 0.1 }
transform = "log"

# Design C: "what if we had better transmission estimates?"
[design.better_transmission]
method = "sobol"
n = 1024

[design.better_transmission.parameters.vacc_eff]
range = { min = 0.1, max = 0.9 }

[design.better_transmission.parameters.R0]
range = { min = 2.0, max = 3.5 }

[design.better_transmission.parameters.kappa]
range = { min = 0.001, max = 0.1 }
transform = "log"
```

### 7.3 Design Methods

| Method    | Runs for k params | Description                          |
| --------- | ----------------- | ------------------------------------ |
| `sobol`   | N(2k + 2)         | Saltelli's scheme for Sobol indices  |
| `lhs`     | N                 | Latin Hypercube Sampling             |
| `random`  | N                 | Uniform random                       |

`sobol` generates structured parameter combinations via Saltelli's quasi-random
sampling scheme, enabling variance decomposition into first-order and
total-order indices per parameter. For n=1024 and k=3 parameters: 1024 × 8 =
8,192 parameter points.

### 7.4 Parameter Specification

```toml
[design.NAME.parameters.PARAM]
range = { min = 0.1, max = 0.9 }    # required
transform = "log"                    # optional: "log" | "logit"
```

`range` defines the sampling bounds. `transform` changes the sampling
space: `"log"` samples uniformly in log space (appropriate for rates and R0);
`"logit"` samples uniformly in logit space (appropriate for probabilities
bounded away from 0 and 1).

Parameters not mentioned in a design are held at their base values from
params.toml.

### 7.5 Design × Scenario Interaction

Each design crosses with every scenario:

```
Runs per design = N(2k+2) × |scenarios| × |seeds|
Total runs = sum over all designs
```

### 7.6 Cross-Design Comparison

Each named design encodes a belief state. Sensitivity indices under each design
answer: "given these beliefs, which parameters drive output uncertainty?"

Cross-design comparison shows how the sensitivity landscape shifts:

- **Design A (current):** vacc_eff explains 62% of peak_I variance
- **Design B (better coverage):** vacc_eff drops to 15%, R0 rises to 55%
- **Design C (better transmission):** R0 drops to 8%, vacc_eff still 58%

These are properties of the model's response surface in each parameter region,
not decision-theoretic quantities. For formal value of information analysis
(EVSI), see the VOI Specification.

**Important caveats:** This is a sensitivity landscape comparison, not a
Bayesian posterior calculation. The "variance explained" is conditional on
uniform sampling over the specified ranges. Different ranges produce different
indices. The `assumptions.txt` file (§13.4) makes all assumptions explicit.

### 7.7 Output Structure for Designs

```
{output_dir}/
  designs/
    current/
      parameter_points.tsv      # N(2k+2) × k matrix, always written
      runs/
        {sim_hash}/...
    better_coverage/
      parameter_points.tsv
      runs/
        {sim_hash}/...
```

`parameter_points.tsv` is always written regardless of whether
`camdl experiment analyze` is run — it's available for external reanalysis.

---

## 8. Content-Addressable Output Structure

### 8.1 Directory Layout

```
{output_dir}/
  manifest.json
  model.ir.json
  runs/
    {sim_hash_8}/
      {scenario_slug}-{scen_hash_8}/
        seed_{seed}/
          traj.tsv
          run.json
```

Example with two scenarios and a sweep point:

```
runs/3a7f2c1d/baseline-00000000/seed_1/
runs/3a7f2c1d/with_sia-f9e2b047/seed_1/
runs/3a7f2c1d/with_sia_vacc_eff_0.5-a3c1e890/seed_1/
```

After tweaking `with_sia` only — baseline cache is untouched:

```
runs/3a7f2c1d/with_sia-d4e2a391/seed_1/   ← new hash
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

**Sim hash** — model + base params + backend + dt:

```
sim_hash = sha256(
    model_ir_json_bytes
    + canonical_sorted(base_param key=value pairs)
    + backend + dt + camdl_version
)   # full 64-char hex; first 8 used in dir name
```

`model_ir_json_bytes` is the compiled IR — it captures all structural content:
compartments, parameters, transitions, interventions, init, etc.

**Scenario hash** — scenario delta (sweep/design point values are included):

```
scen_hash = sha256(
    sorted(enable list)
    + sorted(disable list)
    + canonical_sorted(scenario param overrides)
    + canonical_sorted(sweep/design point overrides)
)   # full 64-char hex; first 8 used in dir name
```

`scen_hash` covers only the _delta_. Base params are already in `sim_hash`.
Renaming a scenario without changing its definition preserves `scen_hash` and
reuses cached runs. A scenario with no overrides, enables, or disables always
hashes to the same value, producing `00000000` in the directory name.

### 8.3 Cache Reuse Matrix

| What changed                       | sim_hash  | scen_hash         | Reuse               |
| ---------------------------------- | --------- | ----------------- | ------------------- |
| Model / base params                | changes   | —                 | none                |
| Backend or dt                      | changes   | —                 | none                |
| Scenario A's overrides             | unchanged | A changes, B same | B's runs reused     |
| Sweep point values                 | unchanged | affected only     | other points reused |
| Add more seeds                     | unchanged | unchanged         | all existing reused |
| Rename a scenario                  | unchanged | unchanged         | reused              |

### 8.4 Manifest

`manifest.json` at the output root lists every completed run:

```json
{
  "experiment_name": "Nigeria SIA evaluation 2024",
  "runs": [
    {
      "scenario": "with_sia",
      "sweep_point": { "vacc_eff": 0.3 },
      "seed": 1,
      "run_path": "3a7f2c1d/with_sia-f9e2b047/seed_1"
    }
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
  "sweep_point": { "vacc_eff": 0.3 },
  "design": null,
  "design_point_index": null,
  "seed": 42,
  "backend": "gillespie",
  "dt": 1.0,
  "camdl_version": "0.1.0",
  "wall_time_seconds": 1.23,
  "timestamp": "2026-03-25T14:30:00Z"
}
```

Cache key: if the run directory exists, the run is skipped. `--force` to re-run.

---

## 10. Comparisons

### 10.1 Pair Specification

```toml
[compare]
pairs = [
  ["baseline", "with_sia"],
  ["baseline", "high_coverage"],
  ["with_sia", "high_coverage"],   # non-baseline reference is valid
]
```

Each pair is `[reference, test]`. The keyword `baseline` refers to the identity
patch.

### 10.2 CRN Coupling

Paired scenarios share the same seed. The coupling guarantee depends on the
scenario type:

**Propensity-preserving scenarios** (`enable`/`disable` only, no `set`/`scale`):
Trajectories are **byte-identical** until the first enabled intervention fires.
Both scenarios have identical states, identical propensities, and consume
identical RNG draws up to that point. The paired difference after the
intervention captures the exact causal effect.

**Parameter-modifying scenarios** (`set`/`scale`): Trajectories are **correlated
but never identical**. Different parameter values produce different propensities
from t=0, so RNG draws map to different events. CRN still reduces variance
compared to independent seeds.

### 10.3 Derived Quantities

```toml
[compare.derived]
cases_averted = "reference.total_cases - test.total_cases"
relative_reduction = "cases_averted / reference.total_cases"
peak_reduction = "1 - test.peak_I / reference.peak_I"
abs_difference = "abs(reference.peak_I - test.peak_I)"
```

`reference.QUANTITY` and `test.QUANTITY` access summary values. `QUANTITY` must
match a name in the model's `output { summary { } }` block (language spec §17).
Previously-defined derived quantities can be referenced by name.

### 10.4 Derived Expression Language

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

**Evaluation order:** topological sort of the dependency graph. Cycles are an
error.

---

## 11. Parameter Files

### 11.1 Baseline Parameters (params.toml)

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
default in the declaration). The CLI validates at load time and reports missing
parameters with their declared types.

### 11.2 Layered Parameters

```toml
[experiment]
params = ["params/base.toml", "params/fitted_2024.toml"]
```

Later files override earlier ones. `fitted_2024.toml` might contain only
`beta = 0.35` and `rho = 0.42`, overriding those values from `base.toml`.

### 11.3 Indexed Parameter Overrides

For spatial models (`R0[patch]` expanding to 238 scalar parameters), use
`--param-vec` to supply values from a keyed TSV:

```tsv
R0_kano     2.1
R0_lagos    1.8
R0_sokoto   2.4
```

```bash
camdl simulate model.camdl --params p.toml --param-vec R0=r0_init.tsv
```

Matched by parameter name (not position). Error if a constructed name doesn't
match, or if a matching parameter has no entry.

---

## 12. Output File Schemas

### 12.1 Trajectories

One row per output time. Format: `t: float64`, then one column per compartment
(`int64` for stochastic compartments, `float64` for real-valued).

### 12.2 Flows

One row per output time. One `uint64` column per transition (flow count since
previous output).

### 12.3 Summary

One row per seed. Columns from `output { summary { } }` in `.camdl`:

```tsv
seed	peak_I	total_cases	final_size
42	4523	89201	0.089
```

**Summary functions:** `max(expr)`, `min(expr)`, `cumulative(transition)`,
`at(expr, timepoint)`.

**Valid timepoints for `at()`:** `t_start`, `t_end`, or numeric literal in
model time_unit. User-defined timepoints (v0.2+).

### 12.4 Diagnostics

Per-transition firing statistics, written unconditionally per run:

```tsv
transition_name	total_firings	mean_propensity	max_propensity
```

### 12.5 Comparisons

One row per seed, written to `analysis/comparisons/{ref}-vs-{test}.tsv`:

```tsv
seed	cases_averted	relative_reduction	peak_reduction
```

---

## 13. CLI Commands

### 13.1 Single Run

```bash
camdl simulate model.camdl --params params.toml --seed 42
camdl simulate model.camdl --params params.toml --scenario with_sia --seed 42
camdl simulate model.camdl --params params.toml --param beta=0.5 --seed 42
camdl simulate model.camdl --params params.toml --param-vec R0=r0.tsv --seed 42
```

### 13.2 Experiment Execution (Rust + Rayon + indicatif)

```bash
camdl experiment run EXPERIMENT.toml
camdl experiment run EXPERIMENT.toml --parallel 16
camdl experiment run EXPERIMENT.toml --force
camdl experiment run EXPERIMENT.toml --scenario with_sia
camdl experiment run EXPERIMENT.toml --resume
```

All simulation is Rust-native with Rayon parallelism and indicatif progress
bars:

```
Nigeria SIA evaluation 2024
  ████████████████████░░░░░░░░░ 12,450 / 18,000 runs  69%  [2:14<~1:01]
  baseline × vacc_eff=0.5: 500/500 seeds ✓
  with_sia × vacc_eff=0.5: 412/500 seeds ...
  Cached: 3,200 runs reused
```

The experiment runner:
1. Compiles the `.camdl` model once
2. Loads and layers params.toml files
3. Generates the run grid (sweep × scenarios × seeds OR design × scenarios × seeds)
4. Calls `plan_runs()` to classify cache hits vs new runs
5. Executes new runs with Rayon `par_iter` + indicatif
6. Writes atomic output files (write to `.tmp`, rename)

### 13.3 Post-Processing (Rust)

```bash
camdl experiment summarize EXPERIMENT.toml
camdl experiment compare EXPERIMENT.toml
camdl experiment status EXPERIMENT.toml
```

Pipeline: `run → summarize → compare`.

### 13.4 Sensitivity Analysis (Rust)

```bash
camdl experiment analyze EXPERIMENT.toml
camdl experiment analyze EXPERIMENT.toml --design current
camdl experiment analyze EXPERIMENT.toml --json
camdl experiment analyze EXPERIMENT.toml --bootstrap 2000 --confidence 0.99
```

Reads `parameter_points.tsv` + `outputs.tsv` for each design. Computes Sobol
indices and bootstrap confidence intervals in Rust. Writes TSV outputs to
`analysis/sensitivity/{design}/`. Optional `--json` writes alongside the TSV.

See §14 for output file schemas.

### 13.5 Verification

```bash
camdl experiment verify EXPERIMENT.toml     # check run.json hashes
camdl experiment check EXPERIMENT.toml      # detect stale results
```

---

## 14. Sensitivity Analysis: `camdl experiment analyze`

### 14.1 Overview

`camdl experiment analyze` reads simulation outputs and computes global
sensitivity indices in Rust. No Python required for the computation step.
Python (`camdl-analysis`) is used only for figure generation (§15).

### 14.2 Sobol Index Computation (Saltelli 2010)

For each design using `method = "sobol"`, the structured sample matrices
(A, B, A_Bi) generated during `experiment run` are used to compute variance-
based sensitivity indices.

For n samples and k parameters, given output Y for a single scalar summary:

- A-block: rows 0..n → Y_A
- B-block: rows n..2n → Y_B
- A_Bi block i: rows (2+i)×n..(3+i)×n → Y_ABi

Saltelli et al. (2010) estimators, equations (b) and (f):

```
V_total = Var(Y_A)
S1[i]   = (1/n) Σ_j Y_B[j] * (Y_ABi[j] - Y_A[j])  / V_total    [eq. (b)]
ST[i]   = (1/2n) Σ_j (Y_A[j] - Y_ABi[j])^2          / V_total    [eq. (f)]
```

Bootstrap confidence intervals: resample rows of (A, B, A_Bi) jointly,
recompute S1 and ST 1000 times (configurable with `--bootstrap`), take the
α/2 and 1-α/2 quantiles.

### 14.3 `[analyze]` Block (Optional Convenience)

For experiments that want built-in Sobol computation without the Python package,
add an optional `[analyze]` block to the experiment TOML:

```toml
[analyze]
sobol_indices = true
outputs = ["peak_I", "total_cases", "cases_averted"]
confidence = 0.95
```

If absent, all float columns in `outputs.tsv` are analyzed. `--outputs X,Y,Z`
on the CLI overrides for ad-hoc use.

This covers the common case (Sobol S1/ST + bootstrap CIs). The Python package
(`camdl-analysis`) provides richer analysis: Morris screening, cross-design VOI
waterfall, sweep figures, and the `Experiment` API.

### 14.4 Output Files

All sensitivity output is written to `{output_dir}/analysis/sensitivity/{design}/`:

**`sobol_indices.tsv`:**

```tsv
design	output	parameter	S1	S1_ci_low	S1_ci_high	ST	ST_ci_low	ST_ci_high
current	peak_I	vacc_eff	0.62	0.58	0.66	0.71	0.67	0.75
current	peak_I	R0	0.23	0.19	0.27	0.29	0.25	0.33
better_coverage	peak_I	vacc_eff	0.15	0.11	0.19	0.22	0.18	0.26
better_coverage	peak_I	R0	0.55	0.51	0.59	0.63	0.59	0.67
```

**`convergence.tsv`:** Indices recomputed at n/8, n/4, n/2, n — shows whether
estimates have stabilized.

```tsv
design	output	parameter	n_samples	S1	ST
current	peak_I	vacc_eff	128	0.58	0.69
current	peak_I	vacc_eff	256	0.61	0.70
current	peak_I	vacc_eff	512	0.62	0.71
current	peak_I	vacc_eff	1024	0.62	0.71
```

**`assumptions.txt` (auto-generated, always written):**

```
Sensitivity analysis assumptions:

Methodology:
  Sobol first-order and total-order indices (Saltelli 2010)
  Bootstrap confidence intervals (1000 resamples, 95% level)

Sampling:
  Uniform over specified parameter ranges
  Output variance decomposition is CONDITIONAL on these bounds
  Different bounds (designs) produce different indices

Design "current":
  vacc_eff ∈ [0.1, 0.9] (uniform)
  R0 ∈ [1.0, 5.0] (uniform)
  kappa ∈ [0.001, 0.1] (log-uniform)

Design "better_coverage":
  vacc_eff ∈ [0.6, 0.8] (uniform)
  R0 ∈ [1.0, 5.0] (uniform)
  kappa ∈ [0.001, 0.1] (log-uniform)

Cross-design comparison:
  ST shift from "current" → "better_coverage" shows which parameters
  become more or less influential as coverage knowledge narrows.
  These are sensitivity landscape properties, NOT decision-theoretic
  quantities. For formal value of information analysis (EVSI), see
  the VOI Specification (voi.toml).
```

The assumptions file is non-negotiable — it is always generated and states
exactly what was assumed. Raw data (`parameter_points.tsv`, `outputs.tsv`) is
always available for external reanalysis.

If `--json` is passed, `sobol_indices.json` is written alongside the TSV with
equivalent content.

---

## 15. Python Analysis Package (`camdl-analysis`)

A separate Python package for figure generation. Computation is handled
entirely by `camdl experiment analyze` (§14). Python reads TSV outputs and
generates matplotlib figures.

### 15.1 Architecture

```
python/
  pyproject.toml          # matplotlib, polars, defopt
  camdl_analysis/
    __init__.py
    cli.py                # defopt entry points
    sensitivity.py        # read sobol_indices.tsv → bar chart
    morris.py             # read morris_indices.tsv → mu*/sigma scatter
    voi.py                # read multi-design sobol_indices.tsv → waterfall
    scatter.py            # read outputs.tsv → scatter matrix
    convergence.py        # read convergence.tsv → line plot
```

**No computation in Python for Sobol.** First-order and total-order index values
come from Rust-generated TSV files. Morris screening (`mu*_i`, `sigma_i`)
may be computed in Python since it does not need the structured Saltelli matrices.

### 15.2 CLI (defopt)

```bash
# Sobol bar chart per design
camdl-analysis sensitivity experiment.toml --output figures/

# VOI waterfall: cross-design variance reduction
camdl-analysis voi experiment.toml --output figures/

# Scatter matrix: parameter values vs output values
camdl-analysis scatter experiment.toml --design current --output figures/

# Convergence diagnostic: index estimate vs N
camdl-analysis convergence experiment.toml --design current --output figures/

# Sweep result figures: output distributions across sweep grid
camdl-analysis sweep-figures experiment.toml --output figures/

# All analysis + figures in one step
camdl-analysis all experiment.toml --output figures/
```

All commands read from `{output_dir}/analysis/` (resolved via experiment.toml)
and write figures to `--output`. VOI results are written to
`{output_dir}/analysis/voi/comparison.tsv`.

### 15.3 Figure Descriptions

**Sobol bar chart (per design):** Grouped bars, one pair per parameter (S1
darker, ST lighter), CI whiskers. Side-by-side panels across designs show how
the sensitivity landscape shifts.

**VOI waterfall:** Horizontal bars, one per information acquisition (design B
vs A, C vs A). Length = variance reduction. Sorted by magnitude. Answers
"which data is most worth collecting?"

**Scatter matrix (per design):** Lower triangle: parameter value vs output
value. Diagonal: marginal histograms. Upper triangle: Spearman correlations.
Shows nonlinearities Sobol indices compress.

**Convergence diagnostic:** Index estimate vs N with CI band. Shows whether
estimates have stabilized. Suggests increasing N if not converged.

**Morris screening (when `method = "morris"`):** mu* vs sigma scatter,
one point per parameter. High mu*, low sigma → large linear effect. High
sigma → nonlinear or interactive. Useful for screening large parameter sets
(>10) before a full Sobol analysis.

---

## 16. Worked Example: Dangerous Middle

Testing whether intermediate vaccination coverage creates increased cVDPV2
emergence risk (the "dangerous middle" hypothesis).

### 16.1 Sweep Experiment

```toml
[experiment]
name = "Dangerous middle: coverage sweep"
model = "models/seir_nigeria_reversion.camdl"
params = ["params/base.toml"]

[config]
backend = "gillespie"
seeds = { n = 500 }
parallel = 16
output_dir = "output/dangerous_middle"

[sweep]
vacc_eff = { linspace = { min = 0.05, max = 0.95, n = 19 } }

[[scenario]]
name = "baseline"

[[scenario]]
name = "with_sia"
enable = ["sia"]

[compare]
pairs = [["baseline", "with_sia"]]

[compare.derived]
cases_averted = "reference.total_cases - test.total_cases"
emergence_events = "test.cum_reversion"
```

**19 coverage levels × 2 scenarios × 500 seeds = 19,000 runs.**

Expected result: `P(emergence_events > threshold)` vs `vacc_eff` produces a
non-monotonic curve peaking at intermediate coverage.

### 16.2 Sensitivity Characterization

"Is the dangerous middle's location more sensitive to coverage uncertainty or
R0 uncertainty?" — characterize how the model's sensitivity landscape shifts
as each uncertainty is resolved. For formal value of information analysis
(EVSI) using these outputs, see the VOI Specification.

```toml
[experiment]
name = "Dangerous middle: VOI analysis"
model = "models/seir_nigeria_reversion.camdl"
params = ["params/base.toml"]

[config]
backend = "gillespie"
seeds = { n = 100 }
parallel = 16
output_dir = "output/dangerous_middle_voi"

[design.current]
method = "sobol"
n = 512

[design.current.parameters.vacc_eff]
range = { min = 0.1, max = 0.9 }

[design.current.parameters.R0]
range = { min = 1.0, max = 5.0 }

[design.better_coverage]
method = "sobol"
n = 512

[design.better_coverage.parameters.vacc_eff]
range = { min = 0.3, max = 0.6 }

[design.better_coverage.parameters.R0]
range = { min = 1.0, max = 5.0 }

[design.better_transmission]
method = "sobol"
n = 512

[design.better_transmission.parameters.vacc_eff]
range = { min = 0.1, max = 0.9 }

[design.better_transmission.parameters.R0]
range = { min = 2.0, max = 3.5 }

[[scenario]]
name = "with_sia"
enable = ["sia"]
```

**3 designs × N(2×2+2) = 3 × 3,072 = 9,216 parameter points × 1 scenario
× 100 seeds = 921,600 runs.** Expensive — use fewer seeds and lean on the
cross-design comparison structure.

```bash
camdl experiment run experiment_voi.toml --parallel 16
camdl experiment analyze experiment_voi.toml
camdl-analysis sensitivity experiment_voi.toml
camdl-analysis voi experiment_voi.toml --output figures/
```

---

## 17. Caching, Staleness, and Provenance

### 16.1 Cache Hit

A run is a cache hit when
`runs/{sim_hash_8}/{scenario_slug}-{scen_hash_8}/seed_{N}/` already exists. The
directory path encodes all inputs: changing the model, base params, backend, dt,
or camdl version changes `sim_hash_8`; changing a scenario's enable/disable
lists or param overrides changes `scen_hash_8`.

Scenario changes only invalidate runs for that scenario — other scenarios'
directories are unaffected.

### 16.2 Provenance Guarantees

1. **Self-contained.** `manifest.json` + `model.ir.json` describe the analysis
   completely.
2. **Content-addressed.** `sim_hash` and `scen_hash` in `run.json` audit all
   inputs.
3. **Version-stamped.** Different camdl versions → different `sim_hash` →
   different directories.
4. **Deterministic.** Same inputs → byte-identical output (single platform).

**Not guaranteed:** cross-machine floating-point reproducibility.

---

## 18. Relationship to the Parameter Grammar

| Grammar concept      | Implementation                                       |
| -------------------- | ---------------------------------------------------- |
| **Sim(m, c, s) → Y** | One run: `runs/{sim_hash_8}/{scen_hash_8}/seed_{s}/` |
| **M**                | `parameters { }` in `.camdl`                         |
| **C**                | `.camdl` structure + `[config]` overrides            |
| **S**                | `seeds` in experiment or `--seed`                    |
| **Point m ∈ M**      | `params.toml` (layered)                              |
| **Scenario σ**       | `scenarios { }` (merged with experiment)             |
| **Sweep**            | Deterministic M-layer grid                           |
| **Design**           | Named belief state → space-filling sample            |
| **VOI**              | See VOI Specification (voi.toml)                     |
| **Baseline σ₀**      | Identity patch                                       |
| **σ₂ ∘ σ₁**          | `compose = ["A", "B"]`                               |
| **View V**           | `view.toml` (v0.2+)                                  |
| **Transform T_V**    | `priors.toml` (v0.2+)                                |

---

## 19. Implementation Phases

### v0.1-core (implemented)

Single-run CLI, scenario support, batch experiment execution with Rayon
parallel + indicatif, content-addressable output, `plan_runs()` cache
classification.

### v0.1-sweep

`[sweep]` with `linspace`, `logspace`, `range`, explicit lists. Sweep ×
scenario Cartesian product. ~60 lines Rust in `experiment.rs`.

### v0.1-post

Summary computation, derived expression evaluator (~120 lines Rust),
`camdl experiment summarize`, `compare`.

### v0.2-design

`[design.*]` named belief states. Saltelli sampling (sobol), LHS, random.
Design × scenario execution. `parameter_points.tsv` output.
New `sampling.rs` module.

### v0.2-analysis

`camdl experiment analyze` Rust subcommand. Sobol index computation with
bootstrap CIs. Auto-generated `assumptions.txt`.

### v0.2-python

`camdl-analysis` Python package. Sobol figures from Rust TSV outputs.
Morris screening. Cross-design sensitivity comparison. Sweep result figures.

### v0.2-inference

`view.toml`, `priors.toml`. Scoring primitive. Particle filter, IF2.
`camdl fit`.

---

## Appendix A: Analyze Block

See §14.3.

---

## Appendix B: Runtime Testing Protocol

Every new feature in the experiment system should be tested at three levels:

1. **Unit test** — function produces correct output for known input. Examples:
   sweep expansion, Sobol estimator, hash stability, cache hit/miss classification.

2. **Integration test** — minimal experiment.toml compiles, runs, and produces
   the expected directory structure. Check that `run.json` is present, hashes
   are stable, and adding seeds reuses existing runs.

3. **Golden test** — fixed experiment + fixed seed produces byte-identical
   output across versions. Run `make update-expected` to regenerate, review
   the diff, commit all three (fixture + IR + expected) together.

For the analyze subcommand, add a validation test against an analytically
tractable model where expected Sobol indices are known (e.g., additive linear
model y = a·x₁ + b·x₂).
