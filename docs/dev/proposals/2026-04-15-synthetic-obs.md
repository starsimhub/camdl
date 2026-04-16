---
status: proposal
date: 2026-04-15
---

# Synthetic Observations for `camdl simulate`

## Problem

`camdl simulate` outputs trajectories (compartment counts + flow accumulators)
but cannot generate synthetic observations from the `observations` block. Users
must reimplement the observation model externally to produce test data for
fitting pipelines or to visualize what surveillance data looks like.

## Design

### Two flags, no magic

```bash
# Simple: one file (single-stream or wide multi-stream)
camdl simulate sir.camdl --params p.toml --seed 42 --obs cases.tsv

# Advanced: one file per stream in a directory
camdl simulate spatial.camdl --params p.toml --seed 42 --obs-dir obs/
```

**`--obs FILE`** always produces one file. **`--obs-dir DIR`** always produces
one file per observation stream. No path sniffing, no auto-detection.

They can coexist:

```bash
camdl simulate spatial.camdl --obs quick_look.tsv --obs-dir obs/
```

### `--obs FILE`

Single observation stream:

```
time    weekly_cases
7    42
14    87
21    156
```

Multiple streams on the same schedule (wide format):

```
time    cases_p1    cases_p2    cases_p3    cases_p4    cases_p5
7    42    38    12    8    55
14    87    71    28    19    102
```

Mixed schedules (streams with different `every` values) → error:

```
error: observation streams have different schedules (weekly_cases: 7d,
       monthly_sero: 30d). Use --obs-dir to produce one file per stream.
```

The wide-format file is directly usable as `[data]` input for `camdl fit`
when the data loader selects columns by stream name:

```toml
[data]
cases_p1 = "synthetic.tsv"   # reads the cases_p1 column
cases_p2 = "synthetic.tsv"   # reads the cases_p2 column
```

### `--obs-dir DIR`

Produces one file per observation stream, named by the observation block:

```
obs/weekly_cases.tsv
obs/seroprevalence.tsv
```

Each file has two columns (`time` + value), matching the current `[data]`
input format exactly:

```
time    weekly_cases
7    42
14    87
```

Works for all models regardless of schedule alignment. The power user flag
for SBC pipelines and multi-stream spatial models.

### Seeding policy

**Requirement:** Adding `--obs` or `--obs-dir` must not change the trajectory.
A user who runs with and without observation output must get identical
compartment trajectories for the same `--seed`.

**Implementation:** Two independent RNG streams derived from the base seed:

```
process_rng = StatefulRng::new(seed)                    # drives simulation
obs_rng     = StatefulRng::new(seed ^ 0xa5a5a5a5a5a5)  # drives obs sampling
```

The process RNG is identical whether or not observations are generated. The
observation RNG is only consumed when `--obs` or `--obs-dir` is specified.

### `--replicates N`

```bash
camdl simulate sir.camdl --params p.toml --seed 42 --replicates 100 --obs cases.tsv
```

Runs N independent forward simulations, each with its own trajectory and
observation draw. Adds a `replicate` column to all output:

**Trajectory output** (stdout or `--output`):
```
replicate    t    S    I    R    flow_infection
1    0    9990    10    0    0
1    1    9985    15    0    5
...
2    0    9990    10    0    0
```

**Observation output** (`--obs`):
```
replicate    time    weekly_cases
1    7    42
1    14    87
...
2    7    38
```

**Observation directory** (`--obs-dir`):
```
obs/weekly_cases.tsv    (with replicate column)
obs/seroprevalence.tsv  (with replicate column)
```

The `replicate` column is only present when `--replicates` is used. Without
it, output matches the current single-run format (backward compatible).

**Replicate seeding:**

```
replicate i:
  process_rng_i = StatefulRng::new(seed ^ (i * 0x517cc1b727220a95))
  obs_rng_i     = StatefulRng::new(seed ^ (i * 0x517cc1b727220a95) ^ 0xa5a5a5a5a5a5)
```

Guarantees: reproducibility (same seed = same output), independence (process
and obs noise uncorrelated), stability (adding obs flags doesn't change
trajectories), no cross-replicate correlation.

### `--obs-only`

For large replicate counts, suppress trajectory output entirely:

```bash
camdl simulate sir.camdl --seed 42 --replicates 1000 --obs-only cases.tsv
```

Produces only the observation file — no trajectory to stdout or `--output`.
Faster and smaller output for SBC workflows.

`--obs-only` implies `--obs` (takes a file path). Can also be combined with
`--obs-dir`:

```bash
camdl simulate spatial.camdl --replicates 1000 --obs-only obs/
```

In this form, `--obs-only` implies `--obs-dir`.

### Interaction with scenarios

```bash
camdl simulate sir.camdl --scenario vaccination --seed 42 --obs obs_vacc.tsv
```

Scenarios set params and toggle interventions as before. The observation model
evaluates at the scenario's parameter values (e.g., if `rho` differs between
scenarios, the observation noise changes accordingly).

## Data loader: wide-format support

To close the round-trip (`simulate --obs` → `fit`), the data loader in
`FitRunConfig::build` needs to handle wide-format TSV files where multiple
`[data]` entries point to the same file:

```toml
[data]
cases_p1 = "synthetic.tsv"
cases_p2 = "synthetic.tsv"
```

The loader reads `synthetic.tsv`, finds columns `cases_p1` and `cases_p2`,
and extracts each as a separate observation stream. Current behavior (one
`time` + one `value` column) continues to work unchanged.

**Column matching rule:** if the file has exactly 2 columns (`time` + one
value), use the value column regardless of its name (backward compatible).
If the file has 3+ columns, match the stream name to a column header. Missing
column → error.

## Implementation

### Files to modify

1. **`cli/src/main.rs`** — add `--obs`, `--obs-dir`, `--obs-only`,
   `--replicates` flags to simulate command parsing.

2. **`cli/src/main.rs` or new `cli/src/simulate_obs.rs`** — observation
   sampling loop: accumulate flows between observation times, sample from
   obs model at each observation time, write TSV.

3. **`sim/src/inference/obs_model.rs`** — `compile_obs_sample_pf` already
   exists. The simulate command calls it.

4. **`cli/src/fit/runner.rs`** — extend `load_observations` to handle
   wide-format TSV (column selection by stream name).

### Estimated effort

| Component | Lines | Notes |
|-----------|------:|-------|
| CLI arg parsing | 30 | `--obs`, `--obs-dir`, `--obs-only`, `--replicates` |
| Obs model construction from IR | 20 | Reuse `compile_obs_sample_pf` |
| Observation sampling loop | 60 | Flow accumulation + sample at obs times |
| Replicate loop + seed derivation | 30 | Independent RNG pairs per replicate |
| `--obs` TSV writing (single file) | 30 | Wide format for multi-stream |
| `--obs-dir` TSV writing (per stream) | 25 | One file per obs block |
| Wide-format data loader | 40 | Column selection in `load_observations` |
| **Total** | **~235** | |

### Test plan

1. **Determinism:** same `--seed` + `--obs` produces identical output
2. **Stability:** adding `--obs` doesn't change trajectory output
3. **Round-trip (single stream):** `simulate --obs` → `fit` with same file
4. **Round-trip (multi-stream):** `simulate --obs` → `fit` with wide file
5. **Round-trip (obs-dir):** `simulate --obs-dir` → `fit` with per-stream files
6. **Replicate independence:** different replicates produce different values
7. **Mixed schedule error:** `--obs` with different-schedule streams → clear error
8. **`--obs-only`:** no trajectory output, obs file correct
9. **Scenario interaction:** different scenario → different obs noise

### What this enables

1. **Tutorials:** full pipeline (`model → data → fit`) without leaving camdl
2. **Fitting validation:** generate synthetic data at known params, fit, check
   parameter recovery
3. **Simulation-based calibration:** `--replicates 1000 --obs-only` for SBC
4. **Posterior predictive checks:** simulate from posterior draws, compare
   synthetic obs to real data
5. **Multi-stream spatial workflows:** 5-patch model generates 5-column obs
   file, directly usable for spatial fitting
