# camdl Experiment & Output System Specification

**Version:** 0.1-draft **Date:** 2026-03-17

This document specifies the experiment file format, content-addressable output
structure, run management, and provenance guarantees for camdl. It implements
the parameter grammar (Buffalo 2026) at the systems level.

---

## 1. Design Principles

**Reproducibility is structural, not aspirational.** Every output file is
content-addressed by the inputs that produced it. Same inputs → same hash → same
directory. Different inputs → different hash → separate directory. There is no
way to produce ambiguous provenance.

**The experiment file is the unit of analysis.** A `.toml` file that fully
specifies what to run: which model, which parameters, which scenarios, which
seeds, which backend. This file is committed to git, included in paper
supplements, shared with collaborators. It answers "what analysis did you run?"
completely.

**Model ≠ parameterization ≠ analysis.** The `.camdl` file defines structure (M
and C). The `params.toml` defines a point m ∈ M. The `experiment.toml` defines
the analysis (scenarios, seeds, comparisons). These are three separate files
with three separate concerns.

**Outputs are immutable.** Once written, a result directory is never modified.
Re-running with the same inputs skips computation (cache hit). Re-running with
different inputs produces a new directory. The `--force` flag re-computes and
overwrites.

---

## 2. The Experiment File

### 2.1 Format

TOML. Chosen for: human readability, git-diffability, comment support, and
unambiguous semantics (no YAML gotchas).

### 2.2 Full Example

```toml
# experiment.toml — Nigeria SIA evaluation 2024

[experiment]
name = "Nigeria SIA evaluation 2024"
model = "models/seir_nigeria.camdl"
params = "params/fitted_2024.toml"
backend = "gillespie" # gillespie | tau_leap | ode | chain_binomial
dt = 1.0 # only for tau_leap / chain_binomial
seeds = { from = 1, to = 1000 }
output_dir = "output" # root of content-addressable tree

[experiment.output]
trajectory_step = 7.0 # days between trajectory snapshots
trajectory_format = "parquet" # parquet | tsv
flow_step = 7.0
flow_format = "parquet"
summary_format = "tsv"

# ── Scenarios ──────────────────────────────────────────

[scenarios.baseline]
# Identity patch — no modifications. Always present implicitly;
# listing it here is optional but makes the intent explicit.

[scenarios.with_sia]
enable = ["sia_round_1"]

[scenarios.high_coverage]
enable = ["sia_round_1"]
set = { vacc_frac = 0.95 }

[scenarios.delayed_sia]
enable = ["sia_round_1"]
set = { sia_time = 365.0 }

[scenarios.combined]
compose = ["with_sia", "more_transmissible"]

[scenarios.more_transmissible]
scale = { beta = 1.5 }

# ── Comparisons ────────────────────────────────────────

[compare]
pairs = [
  ["baseline", "with_sia"],
  ["baseline", "high_coverage"],
  ["baseline", "delayed_sia"],
]

[compare.derived]
cases_averted = "baseline.total_cases - scenario.total_cases"
relative_reduction = "cases_averted / baseline.total_cases"
peak_reduction = "1 - scenario.peak_I / baseline.peak_I"
```

### 2.3 Field Reference

**`[experiment]` (required)**

| Field        | Type   | Required | Description                                                |
| ------------ | ------ | -------- | ---------------------------------------------------------- |
| `name`       | string | yes      | Human-readable experiment name                             |
| `model`      | path   | yes      | Path to `.camdl` model file                                |
| `params`     | path   | yes      | Path to `params.toml` (baseline m ∈ M)                     |
| `backend`    | string | no       | `gillespie` (default), `tau_leap`, `ode`, `chain_binomial` |
| `dt`         | float  | no       | Step size for discrete-time backends (default: 1.0)        |
| `seeds`      | table  | yes      | `{ from = N, to = M }` or `{ list = [1, 2, 3] }`           |
| `output_dir` | path   | no       | Root of output tree (default: `output/`)                   |

**`[experiment.output]` (optional)**

| Field               | Type   | Default     | Description                       |
| ------------------- | ------ | ----------- | --------------------------------- |
| `trajectory_step`   | float  | 1.0         | Days between trajectory snapshots |
| `trajectory_format` | string | `"parquet"` | `parquet` or `tsv`                |
| `flow_step`         | float  | 7.0         | Days between flow snapshots       |
| `flow_format`       | string | `"parquet"` | `parquet` or `tsv`                |
| `summary_format`    | string | `"tsv"`     | `tsv` only for now                |

**`[scenarios.NAME]` (optional, one per scenario)**

Each scenario is a named patch σ: M × C → M × C. The baseline scenario (identity
patch) is always available. Scenario operations:

| Key       | Type         | Description                                 |
| --------- | ------------ | ------------------------------------------- |
| `enable`  | list[string] | Enable named interventions                  |
| `disable` | list[string] | Disable named interventions                 |
| `set`     | table        | Override parameter values: `{ beta = 0.5 }` |
| `scale`   | table        | Multiply parameter values: `{ beta = 1.5 }` |
| `compose` | list[string] | Apply named scenarios in sequence           |

The compiler validates: `scale` on a `probability` parameter that would exceed
[0,1] is an error. `compose` with overlapping write sets produces a
non-commutativity warning.

**`[compare]` (optional)**

| Key     | Type               | Description                                           |
| ------- | ------------------ | ----------------------------------------------------- |
| `pairs` | list[list[string]] | 2-element lists of `[reference, test]` scenario names |

**`[compare.derived]` (optional)**

Derived quantities computed from paired scenario summaries. Each key is a new
quantity name; the value is an expression using `baseline.QUANTITY` and
`scenario.QUANTITY` where `QUANTITY` matches a name in the model's
`output { summary { ... } }` block.

### 2.4 Seeds

Three forms:

```toml
seeds = { from = 1, to = 1000 } # range: generates 1, 2, ..., 1000
seeds = { list = [42, 137, 256] } # explicit list
seeds = { n = 500, start = 1 } # count from start: 1, 2, ..., 500
```

### 2.5 Scenario Composition

Scenarios compose left-to-right: `compose = ["A", "B"]` means σ_B ∘ σ_A. The
result of A is the input to B. This matches the whitepaper's composition
semantics.

Commutativity checking: the compiler computes write sets for each scenario
(which parameters each `set` or `scale` touches). If two composed scenarios have
overlapping write sets, the compiler emits:

```
warning[W300]: scenarios 'A' and 'B' both modify parameter 'beta'.
  Composition order matters: 'A' is applied first, then 'B'.
```

---

## 3. Parameter Files

### 3.1 Baseline Parameters (`params.toml`)

A point m ∈ M. One key-value pair per parameter:

```toml
# params.toml
beta = 0.3
gamma = 0.1
sigma = 0.2
mu = 0.0000548
rho = 0.4
k = 5.0
N0 = 1000000
I0 = 10
```

Every parameter declared in the `.camdl` file's `parameters { }` block must have
a value here (or a default in the declaration). The CLI validates this at
experiment load time and reports missing parameters with their declared types.

### 3.2 Priors (`priors.toml`, v0.2+)

```toml
[beta]
prior = "log_normal"
mu = 0.0
sigma = 1.0
transform = "log"

[rho]
prior = "beta"
alpha = 2.0
beta = 5.0
transform = "logit"

[mu]
prior = "fixed"
```

### 3.3 Views (`view.toml`, v0.2+)

Implements V = (F, fixed_V) from the parameter grammar. Specifies which
parameters are free vs fixed during inference:

```toml
[view]
free = ["beta", "gamma", "rho", "I0"]
# All parameters not listed here are fixed at their params.toml values.
```

Free parameters are varied by the inference engine; all other parameters are
held fixed at their `params.toml` values. Views are only relevant for
`camdl fit` (v0.2+) — they have no effect on forward simulation.

### 3.4 Indexed Parameter Overrides

For spatial models with many parameters (`R0[patch]` expanding to 774 scalar
parameters), a keyed TSV file provides bulk overrides:

```tsv
# r0_init.tsv
R0_kano	2.1
R0_lagos	1.8
R0_sokoto	2.4
```

```bash
camdl simulate model.camdl --params params.toml --set-vec r0_init.tsv
```

The file is name-value pairs. The CLI matches by parameter name (not position).
Errors if a name in the file doesn't match any parameter, or if a model
parameter matching the prefix has no entry in the file.

---

## 4. Content-Addressable Output Structure

### 4.1 Directory Layout

```
{output_dir}/
  model-{model_hash}/
    config-{config_hash}/
      experiment.toml              # copy of the experiment spec (frozen)
      params.toml                  # copy of the baseline parameters (frozen)
      runs/
        baseline/
          seed-1/
            trajectories.parquet
            flows.parquet
            summary.tsv
            diagnostics.tsv
            run.json
          seed-2/
            ...
          seed-1000/
            ...
        with_sia/
          seed-1/
            ...
        high_coverage/
          seed-1/
            ...
      analysis/
        ensembles/
          baseline.parquet         # all seeds stacked, one row per (seed, time)
          with_sia.parquet
        comparisons/
          baseline-vs-with_sia.tsv
          baseline-vs-high_coverage.tsv
```

### 4.2 Hash Computation

**Model hash** — structural identity of the `.camdl` file:

```
model_hash = sha256(structural_content)[:12]
```

Structural content includes: compartments, stratify, parameters (names and types
only), tables (structure and inline values), let bindings, functions,
transitions, observations, ode, interventions, init, time_unit.

Excluded: simulate, output, scenarios, data file paths (content covered by
config hash).

**Config hash** — full analysis identity:

```
config_hash = sha256(
    model_hash
    + sorted(params key=value pairs)
    + sorted(data file content hashes)
    + backend
    + dt (if applicable)
    + camdl_version
)[:12]
```

The `camdl_version` is included because a bug fix or numerical change in the
runtime could produce different output from identical inputs. Results from camdl
0.3.0 and 0.3.1 get separate directories automatically.

**Scenario directories** use bare scenario names (not hashed). Scenario names
are identifiers (`[a-zA-Z_][a-zA-Z0-9_]*`) controlled by the parser — no
collision risk with `runs/` or `analysis/`.

**Seed directories** use `seed-N` format. The hyphen delimiter is visually
distinct from the underscore-heavy scenario names (`with_sia/seed-42/`).

### 4.3 Reserved Directory Names

Two directories are reserved under each config hash:

- `runs/` — contains all scenario directories and their seed subdirectories
- `analysis/` — contains post-processing outputs (ensembles, comparisons)

These names are structurally separated from scenario names. Even if a user names
a scenario `analysis`, it appears as `runs/analysis/seed-42/` — no collision
with the top-level `analysis/` directory.

### 4.4 Frozen Inputs

The experiment spec and baseline parameters are **copied** into the output
directory at run time. This ensures the output is self-contained even if the
source files are later modified or moved:

```
config-7c9a1b2e3f4d/
  experiment.toml    # frozen copy of the input experiment spec
  params.toml        # frozen copy of the baseline parameter values
```

These frozen copies are the authoritative record of what produced the results.
The original files may be edited; the frozen copies never change.

---

## 5. Run-Level Metadata

Each individual run produces a `run.json`:

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
  "params_file": "params/fitted_2024.toml",
  "wall_time_seconds": 1.23,
  "timestamp": "2024-11-15T14:30:00Z",
  "transitions_fired": 1482301,
  "zero_firing_transitions": 0
}
```

The `input_hash` is computed from:

```
input_hash = sha256(config_hash + scenario_name + seed)[:12]
```

This uniquely identifies one cell of the experiment grid. It's the content
address for caching: if `input_hash` matches an existing `run.json`, the run is
a cache hit and can be skipped.

---

## 6. Output File Schemas

### 6.1 Trajectories (`trajectories.parquet`)

One row per output time. Columns:

```
t: float64                         # simulation time
{compartment_name}: int64/float64  # one column per expanded compartment
```

For a model with 8 expanded compartments and 100 output times, this is a 100 × 9
table. Column names match the IR's expanded compartment names (e.g., `S_child`,
`I_adult`).

### 6.2 Flows (`flows.parquet`)

One row per output time. Columns:

```
t: float64                           # simulation time
{transition_name}: uint64            # cumulative flow since previous output
```

Flow counts are reset at each output boundary. The column `infection_child`
gives the number of `infection_child` events that fired between the previous
output time and this one.

### 6.3 Summary (`summary.tsv`)

One row per run. Columns are defined by the model's `output { summary { } }`
block:

```
seed	peak_I	total_cases	final_size
42	4523	89201	0.089
43	4891	91033	0.091
```

Summary is computed post-simulation by the CLI from trajectory and flow data.

### 6.4 Diagnostics (`diagnostics.tsv`)

Per-transition firing statistics. Written unconditionally after every run:

```
transition_name	total_firings	mean_propensity	max_propensity	first_firing	last_firing
infection_child	14523	0.342	1.207	0.003	729.841
infection_adult	8901	0.198	0.892	0.012	729.997
recovery_child	14201	0.100	0.100	2.841	729.999
```

The CLI warns on zero-firing transitions after simulation.

### 6.5 Ensemble (`analysis/ensembles/{scenario}.parquet`)

All seeds stacked. Columns:

```
seed: uint64
t: float64
{compartment_name}: int64/float64
```

Produced by `camdl experiment summarize` after all runs complete.

### 6.6 Comparisons (`analysis/comparisons/{ref}-vs-{test}.tsv`)

Paired scenario comparison. One row per seed:

```
seed	cases_averted	relative_reduction	peak_reduction
42	12034	0.135	0.087
43	11891	0.131	0.082
```

Columns are the derived quantities from `[compare.derived]`. Produced by
`camdl experiment compare` after ensemble summaries are available.

---

## 7. CLI Commands

### 7.1 Experiment Execution

```bash
# Run the full experiment (all scenarios × all seeds)
camdl experiment run EXPERIMENT.toml [--force] [--parallel N]

# Run a single scenario
camdl experiment run EXPERIMENT.toml --scenario with_sia

# Run a single cell
camdl experiment run EXPERIMENT.toml --scenario with_sia --seed 42

# Resume (skip completed runs, re-run failed)
camdl experiment run EXPERIMENT.toml --resume
```

**Execution order:** The CLI iterates `scenarios × seeds`. For each cell, it
computes `input_hash`. If `runs/{scenario}/seed-{N}/run.json` exists and its
`input_hash` matches, the run is skipped (cache hit). Otherwise, the run
executes.

**Parallelism:** `--parallel N` runs N simulations concurrently. Each run is
independent (no shared state). File writes are atomic (write to temp, rename on
completion).

### 7.2 Post-Processing

```bash
# Build ensemble summaries from completed runs
camdl experiment summarize EXPERIMENT.toml

# Compute paired comparisons
camdl experiment compare EXPERIMENT.toml

# Show experiment status (how many runs complete, failed, pending)
camdl experiment status EXPERIMENT.toml
```

### 7.3 Verification

```bash
# Verify all completed runs match their metadata
camdl experiment verify EXPERIMENT.toml

# Recompute hashes and check for stale results
camdl experiment check EXPERIMENT.toml
```

`verify` reads each `run.json`, recomputes the `input_hash` from the frozen
experiment spec and params, and reports any mismatches.

### 7.4 Single-Run (No Experiment File)

For quick exploration without an experiment file:

```bash
camdl simulate model.camdl --params params.toml --seed 42
camdl simulate model.camdl --params params.toml --seed 42 --scenario with_sia
camdl simulate model.camdl --params params.toml --seeds 1:100 --output-dir output/
```

When `--output-dir` is provided, the content-addressable directory structure is
created. Without it, output goes to stdout (TSV).

---

## 8. Caching and Staleness

### 8.1 Cache Hit

A run is a cache hit when:

1. `runs/{scenario}/seed-{N}/run.json` exists
2. The `input_hash` in `run.json` matches
   `sha256(config_hash + scenario + seed)[:12]`
3. The `config_hash` in `run.json` matches the current config hash

If all three hold, the run is skipped. This means changing the model file, any
parameter value, the backend, or the camdl version invalidates the cache for all
runs.

### 8.2 Staleness Detection

`camdl experiment check` recomputes hashes from current source files and
compares against frozen copies in the output directory. It reports:

```
Experiment: Nigeria SIA evaluation 2024
  Model:  models/seir_nigeria.camdl
  Params: params/fitted_2024.toml

  model hash:  a3f8b2c1d4e5  (current: a3f8b2c1d4e5)  ✓ match
  config hash: 7c9a1b2e3f4d  (current: 7c9a1b2e3f4d)  ✓ match

  Runs: 6000 / 6000 complete
  Stale: 0

  ✓ all results are current
```

If the model or params have changed:

```
  model hash:  a3f8b2c1d4e5  (current: b4c9d3e2f1a0)  ✗ STALE
  config hash: 7c9a1b2e3f4d  (current: 8d0b2c3e4f5a)  ✗ STALE

  All 6000 runs are stale — model has changed since last run.
  Run 'camdl experiment run experiment.toml' to recompute.
```

### 8.3 Force Recompute

`--force` ignores the cache and recomputes all runs. The old results are
overwritten atomically (write to temp, rename on success).

---

## 9. Provenance Guarantees

### 9.1 What's Guaranteed

Given an output directory `model-X/config-Y/`:

1. **Self-contained.** The frozen `experiment.toml` and `params.toml` inside the
   directory contain everything needed to reproduce the results (except the
   `.camdl` source and external data files, which are referenced by path and
   content-hashed).

2. **Tamper-evident.** The `input_hash` in each `run.json` is a function of all
   inputs. If any input changes, the hash changes. `camdl experiment verify`
   checks this.

3. **Version-stamped.** Each `run.json` records the `camdl_version`. Results
   from different software versions live in different `config-` directories (the
   version is part of the config hash).

4. **Deterministic.** Same `(model, params, scenario, seed, backend, version)` →
   byte-identical output. This is enforced by the CRN determinism test (same
   seed → same trajectory).

### 9.2 What's NOT Guaranteed

- **Data file immutability.** If `read_csv("data/contacts.csv")` is modified
  between runs, the config hash changes (data file content is hashed), but the
  old output directory still references the old data. The frozen copies don't
  include external data files (they may be large). The `run.json` records the
  data file content hashes for auditing.

- **Cross-machine reproducibility.** Floating-point arithmetic may differ across
  CPU architectures. The determinism guarantee is within a single platform +
  compiler combination.

---

## 10. Relationship to the Parameter Grammar

The experiment system implements the parameter grammar (Buffalo 2026) at the
systems level:

| Grammar concept           | Experiment system implementation                     |
| ------------------------- | ---------------------------------------------------- |
| **Sim(m, c, s) → Y**      | One cell in the `scenarios × seeds` grid             |
| **M** (parameter space)   | `params.toml` — all declared parameters              |
| **C** (configuration)     | Model structure (`.camdl`) + `[experiment]` settings |
| **S** (seed)              | `seeds = { from = 1, to = 1000 }`                    |
| **σ** (scenario patch)    | `[scenarios.NAME]` in experiment file                |
| **σ_baseline**            | Identity patch (no modifications)                    |
| **σ₂ ∘ σ₁** (composition) | `compose = ["A", "B"]`                               |
| **V** (parameter view)    | `view.toml` (v0.2+, for inference)                   |
| **T_V** (transforms)      | `priors.toml` transform field (v0.2+)                |
| **Content addressing**    | `model_hash` × `config_hash` × `scenario` × `seed`   |

The downward chain from inference coordinates to simulation output:

```
z ∈ Z_V       inference engine proposes a vector
  │ T_V⁻¹     back-transform (exp, expit)
  ▼
p ∈ P_V       free parameter values
  │ κ_V       fill in fixed values from params.toml
  ▼
m ∈ M         complete parameter set
  │ σ         apply scenario patch from [scenarios.NAME]
  ▼
(m', c')      patched parameters + configuration
  │ Sim(·,·,s)
  ▼
y ∈ Y         trajectory → runs/{scenario}/seed-{s}/
```

Every arrow is determined by external configuration. The `.camdl` file defines
the structural skeleton; the experiment system fills in the rest.

---

## 11. Implementation Phases

### Phase 1: Content-addressable output (v0.1)

- Hash computation (model hash, config hash)
- Directory creation with `runs/` and `analysis/` separation
- `run.json` metadata
- Frozen experiment/params copies
- `--output-dir` flag on `camdl simulate`
- Cache hit detection (skip if input_hash matches)

### Phase 2: Experiment execution (v0.1)

- `camdl experiment run` with scenario × seed iteration
- `--parallel N` concurrent execution
- `--resume` for interrupted experiments
- `camdl experiment status` progress reporting

### Phase 3: Post-processing (v0.1)

- `camdl experiment summarize` — ensemble stacking
- `camdl experiment compare` — paired scenario comparisons
- Derived quantity evaluation from `[compare.derived]`

### Phase 4: Verification (v0.1)

- `camdl experiment verify` — hash checking
- `camdl experiment check` — staleness detection

### Phase 5: Inference integration (v0.2)

- `view.toml` loading and V = (F, fixed_V) construction
- `priors.toml` loading and T_V construction
- `camdl fit --method if2` using the experiment's model + params + view
- Posterior samples stored alongside runs in `analysis/inference/`
