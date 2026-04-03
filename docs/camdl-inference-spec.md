# camdl Inference Workflow Specification

**Version:** 0.1-draft
**Date:** 2026-03-30
**Depends on:** Experiment Spec v0.5 (output schemas), VOI Spec v0.1

---

## 1. Design Principles

**Files are the workflow.** Every inference step reads files and
writes files. The pipeline exists in the filesystem, not in the
researcher's head. Any step can be rerun. Any result can be traced
to its inputs.

**params.toml is the universal interface.** The inference pipeline's
final output is a params.toml. It feeds directly into `camdl simulate`,
`camdl experiment run`, `camdl voi run`. No format conversion. One
chain from raw data to decision.

**The fixed/free partition is explicit and exhaustive.** Every model
parameter must be declared as either estimated or fixed. No silent
defaults. Accidental fixation — the most common calibration error —
is a compile-time error, not a runtime surprise.

**Provenance is structural.** Every machine-generated file includes
a content hash and input references. Modified files are detectable.
The full chain from data to decision is auditable.

**TOML for humans, JSON for agents, TSV for data.** No file serves
two masters.

---

## 2. File Roles

| File | Format | Written by | Read by | Purpose |
|------|--------|-----------|---------|---------|
| `fit.toml` | TOML | Human | All fit commands | Inference configuration |
| `fit_state.toml` | TOML | Each stage | Next stage | Inter-stage handoff |
| `{stage}_summary.json` | JSON | Each stage | Agent / status | Machine-readable diagnostics |
| `mle_params.toml` | TOML | validate | experiment system | Final MLE (IS a params.toml) |
| `fit_record.json` | JSON | validate | Audit / archive | Self-contained provenance |
| `fit_report.txt` | Text | validate | Human / methods section | Human-readable summary |
| `parameter_traces.tsv` | TSV | Each stage | Plotting | Per-iteration param values |
| `diagnostics.tsv` | TSV | Each stage | Plotting / analysis | Per-iteration ESS, loglik |
| `profiles/*.tsv` | TSV | validate | Plotting / CI | Profile likelihood curves |

---

## 3. The Fit File

### 3.1 Structure

```toml
[fit]
model = "models/he2010_london.camdl"
output_dir = "fit/he2010"

# Data sources — one per observation block in the model.
# Keys must match observation block names in the .camdl file.
[data]
weekly_cases = "data/london_cases.tsv"

# For multi-stream models (polio):
# [data]
# afp_cases = "data/afp_by_district.tsv"
# es_positive = "data/es_results.tsv"

# Optional: out-of-sample holdout data.
# Keys must match [data] keys. Scout/refine never see holdout data.
# Validate runs PF on train + holdout and reports separate logliks.
# [holdout]
# weekly_cases = "data/london_cases_holdout.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

# ── Estimated parameters ──────────────────────────────
# Every parameter here will be perturbed during IF2.
# rw_sd is optional: if omitted, auto-computed from parameter bounds
# as (hi-lo)/6 on the transformed scale (tier 1 heuristic).
# If specified, rw_sd is on the NATURAL scale (converted internally
# via delta method).
[estimate]
R0        = { rw_sd = 5.0 }
sigma     = {}                  # auto rw_sd from bounds
gamma     = {}                  # auto rw_sd from bounds
rho       = { rw_sd = 0.02 }   # explicit override
amplitude = {}
S0        = { ivp = true }      # auto rw_sd, perturbed only at t=0
E0        = { ivp = true }
I0        = { ivp = true }

# Optional: override starting value (default: from model)
# sigma = { start = 0.1 }

# Optional: override transform (default: derived from parameter type)
# R0 = { rw_sd = 5.0, transform = "identity" }

# Optional: narrow the search region (default: full model bounds)
# R0 = { rw_sd = 5.0, bounds = [30.0, 80.0] }
# gamma = { bounds = [0.05, 0.15] }

# ── Fixed parameters ──────────────────────────────────
# Every parameter here is held constant at its model default.
# The user MUST explicitly assign every model parameter to
# either [estimate] or [fixed]. Missing parameters are an error.
[fixed]
N0     = true
mu     = true
k      = true
psi    = true
cohort = true
```

### 3.2 Search Bounds

The model declares structural bounds:
`R0 : positive in [1.0, 100.0]` — the physically valid range.

The fit.toml can declare narrower **search bounds** — "for this
data, focus the search here":

```toml
R0 = { rw_sd = 5.0, bounds = [30.0, 80.0] }
```

Search bounds are optional on every parameter. When omitted, the
full model bounds apply. Typically only 1-2 parameters need
narrowing — those where prior knowledge constrains the plausible
region.

**Rules:**

1. **Fit bounds must be within model bounds.** `bounds = [0.5, 80]`
   on a parameter declared `in [1.0, 100.0]` is an error:
   
   ```
   error: fit bound [0.5, 80] for 'R0' extends below model bound [1.0, 100.0].
     Fit search bounds must be within model structural bounds.
   ```

2. **Fit bounds control the transform.** When `bounds` is specified,
   the logit/scaled-logit transform maps to that interval instead of
   the model bounds. This concentrates the random walk in the region
   of interest rather than wasting perturbation on implausible values.

3. **Fit bounds control scout random starts.** Scout initializes
   chains from uniform random within the fit bounds, not the full
   model bounds.

4. **Boundary proximity warning.** If the MLE is within 1% of a
   fit bound, warn that the true MLE may be outside the search
   region:
   
   ```
   ⚠ R0 = 30.2 is within 1% of fit lower bound (30.0).
     The MLE may be outside the search region.
     Consider widening: R0 = { bounds = [15.0, 80.0] }
   ```
   
   This is reported in `{stage}_summary.json` and `camdl fit status`:
   
   ```
   R0 = 30.2   rw_sd=5.0  bounds=[30, 80]  ⚠ AT LOWER BOUND
   ```
   
   The warning is the safety net. Without it, narrowed bounds can
   hide the true MLE. With it, they're a useful search tool.

### 3.3 Exhaustive Partition Rule

On `camdl fit scout` (or any fit command), the tool checks:

```
model_params = set of all parameters declared in .camdl
estimated    = set of keys in [estimate]
fixed        = set of keys in [fixed]

if estimated ∪ fixed ≠ model_params:
    missing = model_params - estimated - fixed
    error:
      "Parameters not assigned in fit.toml: {missing}
       Every model parameter must appear in [estimate] or [fixed].
       
       Add to [estimate]:  {p} = {{}}
       Add to [fixed]:     {p} = true"
       
if estimated ∩ fixed ≠ ∅:
    overlap = estimated ∩ fixed
    error:
      "Parameters in both [estimate] and [fixed]: {overlap}
       Each parameter must appear in exactly one section."
```

This is a hard error. No warnings, no defaults. The user must
explicitly decide the fate of every parameter.

### 3.4 No `params` Field

Starting values come from the model's parameter defaults (declared
in the `.camdl` file or in params files referenced by the model's
`simulate {}` block). The fit.toml does not reference params files.

If the user wants a non-default starting value, they use `start`
in the `[estimate]` section:

```toml
sigma = { rw_sd = 0.005, start = 0.1 }
```

This keeps the fit.toml self-contained: it says what to estimate
and how, not what the values are.

### 3.5 Observation Model

The observation model is declared in the `.camdl` file's
`observations {}` block. The fit.toml references it by name:

```toml
[data]
weekly_cases = "data/london_cases.tsv"
```

The key `weekly_cases` must match an observation block name in the
model. The data file provides (time, value) pairs. The observation
model (likelihood family, projection, variance formula) comes from
the DSL — it's part of the model specification, not the fit
configuration.

For models with multiple observation streams:

```toml
[data]
afp_cases = "data/afp_by_district.tsv"
es_positive = "data/es_results.tsv"
```

The particle filter sums log-likelihoods across all streams at
each observation time.

---

## 4. The Fit State File

### 4.1 Purpose

`fit_state.toml` is the inter-stage handoff. Each stage reads the
previous stage's fit_state.toml and produces its own. The user
never needs to edit it (but can, for debugging).

### 4.2 Schema

```toml
# fit/he2010/scout/fit_state.toml
# Auto-generated by camdl fit scout.

stage = "scout"
seed = 1
timestamp = "2026-03-30T14:30:00Z"
best_loglik = -3891.2
initial_loglik = -4523.7
best_chain = 4
n_chains = 8
n_good_chains = 6

[start_values]
# Best chain's best-loglik parameters (not endpoint — the point
# during the chain's trajectory where loglik was highest)
R0 = 55.3
sigma = 0.081
gamma = 0.084
rho = 0.49
amplitude = 0.52
S0 = 71200
E0 = 140
I0 = 118

[rw_sd]
# Auto-calibrated via MAD-based convergence-gated procedure:
#
# 1. Collect each chain's best-loglik parameters
# 2. Per parameter: compute median and MAD across chains
# 3. A chain is "good" if ALL its params are within 3×MAD of median
# 4. If n_good < n_chains/2: ERROR — no consensus, cannot auto-calibrate
#    If n_good < n_chains: WARNING — excluded divergent chains
# 5. rw_sd = 0.5 × MAD of good chains (per parameter)
#
# MAD is breakdown-resistant: one chain in a secondary mode
# doesn't corrupt the scale estimate.
R0 = 8.5
sigma = 0.003
gamma = 0.004
rho = 0.025
amplitude = 0.02
S0 = 3200
E0 = 95
I0 = 45
```

### 4.3 How Stages Chain

```
fit.toml
  │
  ├─ camdl fit scout fit.toml
  │    reads: fit.toml (config + rw_sd)
  │    writes: scout/fit_state.toml
  │
  ├─ camdl fit refine fit.toml --starts-from scout/
  │    reads: fit.toml (config) + scout/fit_state.toml (start_values, rw_sd)
  │    writes: refine/fit_state.toml, refine/mle_params.toml
  │
  └─ camdl fit validate fit.toml --starts-from refine/
       reads: fit.toml (config) + refine/fit_state.toml (start_values, rw_sd)
       writes: validate/fit_state.toml, validate/mle_params.toml,
               validate/profiles/, validate/fit_record.json
```

When `--starts-from` is given:
- Starting values come from `fit_state.toml [start_values]`
  (overrides model defaults and fit.toml `start` fields)
- rw_sd comes from `fit_state.toml [rw_sd]`
  (overrides fit.toml rw_sd values)
- Explicit `--rw-sd` CLI flags override everything

When `--starts-from` is NOT given:
- Starting values come from model defaults + fit.toml `start` fields
- rw_sd comes from fit.toml `[estimate]` rw_sd values

---

## 5. MLE Output and Provenance

### 5.1 `mle_params.toml`

The final output of inference. A valid params.toml with provenance
in comments:

```toml
# camdl fit output
# Content hash: a3c1e890 (editing any value below invalidates this)
# Input hash: 7f2c1d3a (identifies the computation that produced this)
# Model: models/he2010_london.camdl (hash: 3a7f2c1d)
# Data: data/london_cases.tsv (hash: f9e2b047)
# Fit: fit.toml (hash: b2c4d6e8)
# Seed: 1
# Stage: validate, chain 2
# Log-likelihood: -3804.9 (sd: 5.2, N=10000)
# ESS at MLE: mean=3842, min=1205
# Timestamp: 2026-03-30T14:30:00Z
# camdl version: 0.3.0

R0 = 56.82
sigma = 0.0791
gamma = 0.0832
rho = 0.488
amplitude = 0.554
S0 = 73151
E0 = 127
I0 = 127
N0 = 2462500
mu = 0.0000548
k = 50.0
psi = 0.116
cohort = 0.0
```

### 5.2 Two Hashes for Two Purposes

**Input hash (in `fit_record.json`):** identifies the computation.

```
input_hash = sha256(
    model_ir_bytes +
    sorted(data_file_bytes for each stream) +
    fit_toml_bytes +
    seed +
    camdl_version
)[:8]
```

Answers: "what computation produced this?" Includes the seed so
that two runs with different seeds have different input hashes.
When no `--seed` is given, one is generated, recorded, and included.

**Content hash (in `mle_params.toml` header):** detects manual edits.

```
content_hash = sha256(
    sorted(param_name + "=" + param_value for each parameter)
)[:8]
```

Answers: "have the output values been changed since the fit wrote
them?" Computed purely from the parameter values in the file.

If the user edits any parameter value, `camdl fit status` detects
the mismatch:

```
$ camdl fit status fit.toml

  validate/mle_params.toml: ⚠ MODIFIED
    R0: 56.82 → 60.0 (manual edit)
    Content hash mismatch: expected a3c1e890, computed d4f7b123
    This file has been hand-tuned. It is no longer an MLE output.
```

The hash doesn't prevent editing. It makes edits VISIBLE. The
distinction between "inference-derived" and "hand-tuned" is
auditable.

### 5.3 `fit_record.json`

Self-contained provenance record. Everything needed to understand
and reproduce the fit:

```json
{
  "model": {
    "path": "models/he2010_london.camdl",
    "hash": "3a7f2c1d"
  },
  "data": {
    "weekly_cases": {
      "path": "data/london_cases.tsv",
      "hash": "f9e2b047",
      "n_observations": 780,
      "time_range": [7.0, 5467.0]
    }
  },
  "fit_config": {
    "path": "fit.toml",
    "hash": "b2c4d6e8",
    "estimated": ["R0", "sigma", "gamma", "rho", "amplitude",
                   "S0", "E0", "I0"],
    "fixed": ["N0", "mu", "k", "psi", "cohort"]
  },
  "method": {
    "algorithm": "IF2",
    "backend": "chain_binomial",
    "dt": 1.0,
    "seed": 1,
    "stages": {
      "scout": { "chains": 8, "particles": 200, "iterations": 20 },
      "refine": { "chains": 4, "particles": 1000, "iterations": 50 },
      "validate": { "chains": 4, "particles": 5000, "iterations": 100 }
    }
  },
  "results": {
    "mle": {
      "R0": 56.82, "sigma": 0.0791, "gamma": 0.0832,
      "rho": 0.488, "amplitude": 0.554,
      "S0": 73151, "E0": 127, "I0": 127
    },
    "loglik": -3804.9,
    "loglik_sd": 5.2,
    "initial_loglik": -4523.7,
    "ess_at_mle": { "mean": 3842, "min": 1205 },
    "convergence": {
      "rhat_max": 1.03,
      "all_converged": true
    },
    "identifiability": {
      "R0": { "ci_95": [52.1, 62.3], "curvature": 0.23 },
      "sigma": { "ci_95": [0.073, 0.086], "curvature": 1.84 }
    }
  },
  "provenance": {
    "input_hash": "7f2c1d3a",
    "content_hash": "a3c1e890",
    "timestamp": "2026-03-30T14:30:00Z",
    "camdl_version": "0.3.0",
    "wall_time_seconds": 1847
  }
}
```

---

## 6. Stage Details

### 6.1 Scout

```bash
camdl fit scout fit.toml [--seed 1]
```

**Defaults:** 8 chains, 500 particles, 30 iterations,
`cooling_fraction = 0.5` (mild contraction — find basins, don't
converge), rw_sd from fit.toml or auto from bounds (`/20` for
log, `/6` for logit).

**Seeded chains:** When `[estimate]` parameters have `start` values,
chain 1 starts near those values (jittered by rw_sd). Remaining
chains start from random positions within bounds. Controlled by
`start_chains` in `[scout]` (default 1 when any start exists).

**Writes:**

```
fit/{name}/scout/
  chain_{1..8}/parameter_traces.tsv
  scout_best_params.toml     ← best chain's params (all estimated + fixed)
  scout_summary.json
  fit_state.toml
  diagnostics.tsv
```

**scout_summary.json includes:**
- `status`: "ok" | "warning" | "error"
- `best_loglik`, `initial_loglik` (pfilter at starting params)
- `ess_at_best`: mean and min ESS at best chain's parameters
- Per-parameter: `rhat`, `range`, `recommended_rw_sd`,
  `boundary_fraction`
- `n_good_chains`: chains within 3×MAD of median (see §4.2)
- `warnings`: list of diagnostic messages
- `next_step`: "refine" | "fix_model" | "widen_bounds"

### 6.2 Refine

```bash
camdl fit refine fit.toml --starts-from fit/{name}/scout/ [--seed 1]
```

**Defaults:** 4 chains, 1000 particles, 50 iterations,
cooling_fraction=0.95 over 50 iterations.

**Reads:** `scout/fit_state.toml` for start values and rw_sd.

**Writes:**

```
fit/{name}/refine/
  chain_{1..4}/
    parameter_traces.tsv
    final_params.toml
  refine_summary.json
  fit_state.toml
  mle_params.toml           ← best chain MLE
  diagnostics.tsv
```

**refine_summary.json includes:**
- `rhat` per parameter (across chains, last half of iterations)
- `loglik_spread` across chains
- `converged`: true if all Rhat < 1.1 and spread < 10
- Per-parameter: `estimate`, `sd`, `cv`, `drift`

### 6.3 Validate

```bash
camdl fit validate fit.toml --starts-from fit/{name}/refine/ [--seed 1]
```

**Defaults:** 4 chains, 5000 particles, 100 iterations,
cooling_fraction=0.95. Profiles for ALL estimated parameters
(embarrassingly parallel). Final pfilter at MLE with N=10000 for
precise loglik and ESS measurement.

**ESS at MLE:** The most informative single diagnostic. Run a clean
pfilter at the MLE and report mean and min ESS across observation
times. High ESS (>N/2) means the model generates trajectories that
track the data. Low ESS (<N/4) means the model can't produce
trajectories consistent with the data even at its best parameters:

```
ESS at MLE: mean=3842, min=1205  ✓ filter is healthy
```

or:

```
ESS at MLE: mean=312, min=8  ✗ filter is degenerate
  Possible causes:
    - Observation model too tight (estimate psi, or increase it)
    - Process noise too low (estimate sigma_se, or increase it)
    - Model structure cannot reproduce observed dynamics
```

**Reads:** `refine/fit_state.toml`.

**Writes:**

```
fit/{name}/validate/
  chain_{1..4}/
    parameter_traces.tsv
    final_params.toml
  profiles/
    {param}_profile.tsv       ← one per estimated parameter
  validate_summary.json
  fit_state.toml
  mle_params.toml           ← final MLE (provenance-hashed)
  fit_record.json           ← self-contained provenance
  fit_report.txt            ← human-readable summary
  pfilter_loglik.txt        ← precise loglik at MLE
  ess_at_mle.tsv            ← per-observation ESS trace at MLE
```

---

## 7. Status Command

```bash
camdl fit status fit.toml
```

Reads the output_dir, checks which stages have completed, reports
convergence and identifiability:

```
fit/he2010/ — He et al. 2010 London measles

  scout:     ✓ complete (8 chains, best loglik -3891.2, 6/8 good)
  refine:    ✓ complete (4 chains, Rhat < 1.05, loglik -3804.9)
  validate:  ✓ complete (profiles clean, loglik -3804.9 ± 5.2)

  ESS at MLE: mean=3842, min=1205  ✓ filter is healthy

  Estimated (8 parameters):
    R0        = 56.82   rw_sd=5.0    ✓ identified  CI: [52.1, 62.3]
    sigma     = 0.0791  rw_sd=0.005  ✓ identified  CI: [0.073, 0.086]
    gamma     = 0.0832  rw_sd=0.005  ✓ identified  CI: [0.077, 0.090]
    rho       = 0.488   rw_sd=0.02   ✓ identified  CI: [0.45, 0.53]
    amplitude = 0.554   rw_sd=0.03   ✓ identified  CI: [0.49, 0.61]
    S0        = 73151   rw_sd=5000   ✓ identified  (ivp)
    E0        = 127     rw_sd=100    ✓ identified  (ivp)
    I0        = 127     rw_sd=50     ✓ identified  (ivp)

  Fixed (5 parameters):
    N0     = 2462500
    mu     = 0.0000548
    k      = 50.0
    psi    = 0.116
    cohort = 0.0

  Provenance:
    mle_params.toml: ✓ content hash matches (a3c1e890)
    fit_record.json: ✓ input hash 7f2c1d3a

  Next: camdl experiment run experiment.toml \
          --params fit/he2010/validate/mle_params.toml
```

---

## 8. Connection to Experiment System

The inference pipeline's output is the experiment system's input.
One shared format: params.toml.

```toml
# experiment.toml
[experiment]
model = "models/he2010_london.camdl"
params = ["fit/he2010/validate/mle_params.toml"]   # ← from inference

[config]
backend = "chain_binomial"
seeds = { n = 200 }
# ...
```

The experiment system doesn't know or care that the params came from
IF2. It's just a params.toml. But the provenance hash in the
comments lets anyone trace it back to the fit.

### 8.1 The Complete Pipeline

```
model.camdl + data.tsv + fit.toml
    │
    ├── camdl fit scout    → scout/fit_state.toml
    ├── camdl fit refine   → refine/mle_params.toml
    └── camdl fit validate → validate/mle_params.toml
                                      │
                    ┌─────────────────┤
                    ▼                  ▼
            experiment.toml      voi.toml
            (params = [mle])     (experiment = ...)
                    │                  │
                    ▼                  ▼
            camdl experiment     camdl voi run
            run + analyze            │
                    │                  ▼
                    ▼              evsi.tsv
            sobol_indices.tsv          │
                    │                  ▼
                    └────────┬─────────┘
                             ▼
                         DECISION
```

### 8.2 What the Experiment System Gains

With inference-derived parameters:

- **Sensitivity analysis at the MLE.** Sobol indices tell you which
  parameter uncertainties matter most for the decision — but only
  if you're centered on the right parameter values. Sensitivity
  analysis at the prior mean is less useful than at the MLE.

- **VOI with informative priors.** The IF2 profiles give approximate
  posterior marginals. These feed into the VOI spec's `prior`
  fields as Beta/LogNormal fits to the profile curves. EVSI with
  data-informed priors is more realistic than with vague priors.

- **Predictive simulation.** `camdl simulate` at the MLE produces
  the "best guess" trajectory. The experiment system's seed
  ensemble around the MLE produces prediction intervals.

---

## 9. Provenance Design

### 9.1 Two Hashes

**Input hash:** identifies the computation. Stored in `fit_record.json`.

```
input_hash = sha256(
    model_ir_bytes +
    sorted(data_file_bytes for each stream) +
    fit_toml_bytes +
    seed +
    camdl_version_string
)[:8]
```

Covers all inputs including the RNG seed. Two runs with different
seeds have different input hashes — the computation is fully
specified and reproducible.

**Content hash:** detects manual edits. Stored in `mle_params.toml`
comment header.

```
content_hash = sha256(
    sorted(param_name + "=" + format(param_value) for each parameter)
)[:8]
```

Computed purely from the parameter values in the file. If any value
is changed, the content hash no longer matches.

### 9.2 Overwrite and Cache Semantics

When a fit command runs, it computes the input hash and checks
whether the output directory already contains results with a
matching input hash:

```
$ camdl fit scout fit.toml --seed 1

  scout already complete for these inputs (input_hash: 7f2c1d3a).
  Use --force to re-run.
```

**Rules:**
- Input hash match → skip (results are deterministic given inputs)
- Input hash mismatch → run (different inputs = different computation)
- `--force` → always run (bypass cache, overwrite existing results)
- No `--seed` given → generate a seed, record it, include in hash

This ensures the cache is reliable: same inputs always produce the
same hash, and different inputs always produce different hashes.
The seed in the hash is what makes this work — without it, two
runs with different seeds would look identical to the cache.

### 9.3 Content Verification

`camdl fit status` checks whether `mle_params.toml` has been
modified since the fit produced it:

```rust
fn verify_mle_params(path: &str) -> ProvenanceStatus {
    let content = read_file(path);
    let declared_hash = extract_content_hash(&content);
    let current_hash = compute_content_hash(&parse_toml_values(&content));
    
    if declared_hash == current_hash {
        ProvenanceStatus::Valid
    } else {
        ProvenanceStatus::Modified {
            changes: diff_params(declared_hash, current_hash)
        }
    }
}
```

### 9.4 When Provenance Breaks

If the user edits `mle_params.toml`, the content hash is broken.
This is not an error — it's information:

```
mle_params.toml: ⚠ MODIFIED (content hash mismatch)
  R0: 56.82 → 60.0 (manual edit)
  All other parameters: unchanged
  
  This file has been hand-tuned. Inference provenance no longer applies.
  To restore: camdl fit validate fit.toml --starts-from refine/
```

The modified file still works as a params.toml everywhere. The
provenance status is advisory, not blocking.

---

## 10. Data File Format

### 10.1 Single-Stream

```tsv
time	cases
7	23
14	67
21	134
28	201
```

`time` in model time units (days for He et al.). One value column
named to match the observation projection output.

**Observation times must be exact multiples of dt.** A time that
does not fall on a step boundary is a hard error:

```
error: observation at t=7.5 is not a multiple of dt=1.0.
  The chain-binomial state only exists at step boundaries.
  Adjust observation times or dt to align.
```

Silent snapping would change which flows accumulate into the
projected quantity, altering the likelihood. This is a user error
that must be caught, not papered over.

### 10.2 Multi-Stream

Each observation stream has its own file. The `[data]` section in
fit.toml maps stream names to files:

```toml
[data]
afp_cases = "data/afp_by_district.tsv"
es_positive = "data/es_results.tsv"
```

### 10.3 Missing Data

Rows with `NA` or blank values are skipped (no contribution to
log-likelihood at that time). Missing entire time points: omit the
row.

### 10.4 Spatially Indexed Data

For spatial models with per-patch observations:

```tsv
time	patch	cases
30	kano	3
30	lagos	0
30	sokoto	1
60	kano	2
60	lagos	1
```

The `patch` column matches the model's spatial dimension. The
particle filter sums log-likelihoods across patches at each time.

---

## 11. Relationship to Other Specs

```
┌──────────────────────────┐
│  .camdl model            │
│  observations {} block   │ structure, obs model
└────────────┬─────────────┘
             │
             ▼
┌──────────────────────────┐
│  fit.toml                │ what to estimate, what data
│  Inference Workflow Spec  │
└────────────┬─────────────┘
             │ camdl fit scout → refine → validate
             ▼
┌──────────────────────────┐
│  mle_params.toml         │ IS a params.toml
│  fit_record.json         │ provenance
└────────────┬─────────────┘
             │ referenced by
             ▼
┌──────────────────────────┐
│  experiment.toml         │ designs, sweeps, comparisons
│  Experiment Spec v0.5    │ → outputs.tsv
└────────────┬─────────────┘
             │ consumed by
             ▼
┌──────────────────────────┐
│  voi.toml                │ decisions, studies, EVSI
│  VOI Spec v0.1           │ → evsi.tsv
└──────────────────────────┘
```

The dependency chain is one-way. Each spec consumes the outputs of
the one above it. No circular dependencies. The observation model
lives in the .camdl file and is shared by both the fit pipeline
(for dmeasure) and the experiment pipeline (for synthetic
observations).
