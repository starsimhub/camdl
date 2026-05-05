---
status: proposal
date: 2026-04-17
---

# Synthetic-Data Fits and Fit-Seed Replicates

## Motivation

A downstream user validating the boarding-school SIR fit ran into the
standard stochastic-inference failure mode: generate one synthetic
dataset at known `(β, γ)`, fit it, and find the MLE several factors
away from truth. The likelihood surface on 14 noisy observations is
simply wide enough that a different parameter combination genuinely
explains this particular realization better. The textbook response —
simulation-based calibration (SBC; Talts et al. 2018) — is to generate
many synthetic datasets from truth, fit each one, and ask whether the
*distribution* of MLEs brackets the true values. That's the first-order
ask.

But the user's underlying need is broader than SBC as a named feature.
Two things routinely get tangled and they are genuinely different:

1. **Dataset variation.** "Given the same generative truth, how much
   does the MLE move when the stochastic realization changes?" —
   generate N datasets at one parameter point, fit each, look at the
   distribution. This is classical SBC when the datasets come from the
   prior; calibration-at-truth when they come from a point.
2. **Fitter variation.** "Given *one* dataset (real or synthetic), how
   much does the MLE move when the IF2/PGAS seed and the perturbation
   start change?" — fit the same data M times with different starts,
   look at whether the runs converge to the same mode or scatter into
   several.

Scripting either one is boilerplate. Scripting the full matrix — fit
each of N datasets M times — is enough boilerplate that nobody does
it, and the resulting diagnostic is one of the most informative things
you can run on a stochastic fit. camdl should do this natively, with a
single config block that collapses cleanly to each of the sub-cases.

## Design principle

The pattern is a small Cartesian grid over two orthogonal axes — the
data source, and the fit start — with the existing
`scout → refine → validate` pipeline running inside each cell. Make
those two axes first-class in the config; keep the pipeline untouched.

Both axes collapse independently:

- Omit both → one fit on one dataset, behaviour identical to today.
- Vary the fit axis only → same data, multiple IF2 seeds / starts.
- Vary the data axis only → one fit per synthetic dataset (classical
  SBC when the data is generated from truth).
- Vary both → the full matrix; disentangles fitter variance from
  data-realization variance.

## Config shape

### Fit-seed replicates

A scalar or list field on `[fit]`, available on any fit, real or
synthetic:

```toml
[fit]
model     = "sir.camdl"
# Absent or scalar: one fit per dataset. A list runs M fits per
# dataset, each with a different IF2/PGAS seed and (optionally)
# perturbation start.
fit_seeds = [101, 102, 103, 104]
```

Semantics: every listed seed runs the full stage pipeline
independently. Provenance hashes differ only in the seed input, so
the cache keys don't collide.

Optional: `fit_starts` controls how the initial parameter point is
chosen per run. Defaults match today's behaviour
(`model_default`). `"prior"` draws from declared priors; a
latin-hypercube mode is reserved but not part of the initial landing:

```toml
fit_starts = "prior"    # or "model_default" (default)
```

### Synthetic-data block

A new top-level block, mutually exclusive with `[data]`:

```toml
[synthetic]
true_params = "params/truth.toml"    # required
sim_seeds   = "1:20"                  # required; string range or list
datasets    = 20                      # optional; inferred from sim_seeds
scenario    = "baseline"              # optional; default: no scenario
```

- `true_params` — a TOML file with `name = value` lines (same format
  as the existing `--params` file). The values are the ground truth
  used to generate data; downstream SBC statistics (bias, coverage)
  use these.
- `sim_seeds` — either an integer-range string (`"1:20"`) or an
  explicit list (`[7, 42, 101, ...]`). Duplicates are rejected.
- `datasets` — *optional*. When omitted, inferred as
  `len(sim_seeds)`. When supplied, must equal that length; mismatch
  is a hard error. Present as a safety check for users who want the
  count stated explicitly.
- `scenario` — named scenario from the `.camdl` file applied during
  data generation (not during fitting). The fit runs against the
  scenario-free baseline; counterfactual fits are a separate
  composition.

The model's existing `observations { }` block supplies the projection
and likelihood used for data generation — the same `--obs` path that
`camdl simulate` already uses. No new observation semantics.

### Mutual exclusivity and validation

- `[data]` + `[synthetic]`: hard error. They are alternate data
  sources.
- `[synthetic]` without `true_params`, `datasets`, or `sim_seeds`:
  hard error; each is required.
- `fit_seeds` containing duplicates: hard error (provenance hashes
  would collide).
- `datasets` supplied and `≠ len(sim_seeds)`: hard error. (When
  `datasets` is omitted, it is set to `len(sim_seeds)` and no check
  runs.)
- `sim_seeds` ∩ `fit_seeds` nonempty: warning, not error (different
  namespaces, but confusing to read in logs).

## Output layout

One shape, always. Every fit has a seed regardless of whether it's
the only fit or one of M; the leaf is therefore always
`fit_<seed>/`. This removes the classic "single-run exception" that
forces downstream result-processing code to handle two layouts.
Breaking change vs. today — acceptable per the project's
backwards-compatibility stance (unreleased software; clean design
wins).

Every fit lives under a data-source subdirectory — `real/` or
`synthetic/`. The two are analogues: a reader of the output tree
cannot mistake a fit against generated data for a fit against
observed data, because the distinction is in the path.

### Real-data fit

`[data]` + `fit_seeds = 101` (scalar) or `fit_seeds = [101, 102, 103]`
(list) — same shape either way:

```
results/fits/<name>/
  real/
    fit_101/
      scout/
      refine/
      validate/
    fit_102/        # present only if fit_seeds included 102
      ...
    summary.tsv     # one row per fit (one row if scalar seed)
  run.json
```

### Synthetic-data fit

`[synthetic]` block present, any `fit_seeds` shape:

```
results/fits/<name>/
  synthetic/
    ds_01/
      fit_101/
        scout/
        refine/
        validate/
      fit_102/      # present only if fit_seeds is a list
        ...
    ds_02/
      fit_101/
      ...
    summary.tsv     # one row per (ds, fit_seed)
    coverage.tsv    # per-parameter coverage + bias vs truth
    truth.toml      # copy of the ground-truth params (provenance)
    data/
      ds_01.tsv     # generated datasets, one per sim_seed
      ds_02.tsv
      ...
  run.json
```

The `real/` vs. `synthetic/` split is the visual cue for what the fit
consumed. SBC-specific statistics (bias vs. truth, coverage of truth
by the MLE distribution) are written only under `synthetic/`; they
are not meaningful for real-data fits. The one remaining asymmetry is
genuine: `synthetic/` has a `ds_NN/` level between the root and
`fit_<seed>/` because there are N datasets; `real/` has one dataset
and skips that level. If multi-file real-data fitting is ever added,
`real/` gains a `<data_basename>/` level and the shapes become
identical.

### What `fit_<seed>` means

Every fit has exactly one IF2/PGAS seed. For `fit_seeds = 101`, the
directory is `fit_101/` — a single directory, not a bare pipeline.
For `fit_seeds = [101, 102]`, there are two directories, `fit_101/`
and `fit_102/`. The seed is in the name because it's the thing that
disambiguates fits, not because there are several — so the layout is
uniform and result-processing scripts read one shape.

### Migration

Existing `results/fits/<name>/scout/…` becomes
`results/fits/<name>/real/fit_<seed>/scout/…` where `<seed>` is the
seed currently supplied (in fit.toml or via `--seed`). Scripts that
walk `results/fits/<name>/scout/` directly must be updated. The full
path is deterministic from config, so the new location is knowable
without runtime inspection.

## Canonical modes

| Mode                    | `[synthetic]` | `fit_seeds`  | Output                              | Fits  |
|-------------------------|---------------|--------------|-------------------------------------|-------|
| Single fit              | —             | scalar       | `real/fit_<seed>/`                  | 1     |
| Start-sensitivity       | —             | list, len M  | `real/fit_<seed>/` × M              | M     |
| SBC (classical)         | N datasets    | scalar       | `synthetic/ds_NN/fit_<seed>/`       | N     |
| SBC × start-sensitivity | N datasets    | list, len M  | `synthetic/ds_NN/fit_<seed>/` × M   | N × M |

All four run the same `scout → refine → validate` pipeline inside
each cell. No new stage verb; `--stage scout` etc. still selects
which stages execute per cell.

## Summary tables

`summary.tsv` columns (always):

```
dataset  fit_seed  stage  <param1>  <param2>  ...  loglik  ess_mean  n_iterations  wall_time_s  content_hash
```

`dataset` is `ds_01`, `ds_02`, … when synthetic; `real` when fitting
`[data]`. One row per (dataset, fit_seed, terminal_stage). The
`content_hash` column is the per-fit provenance hash, matching the
directory's `run.json`.

`coverage.tsv` (synthetic mode only):

```
param  truth  mean_mle  bias  sd_mle  q05  q95  covers_truth  n_datasets
```

`covers_truth` is `1` when the per-dataset MLE central 90% window
brackets `truth`, `0` otherwise. `n_datasets` is the number of
datasets that ran to completion (failed fits excluded). Writing this
is unconditional in synthetic mode; a modeler doing careful work
always wants it.

## Parallelism

`--parallel P` covers **both phases** — synthetic data generation
and the fit grid — using the same outer-loop worker pool. For
cheap models data generation finishes in seconds either way, but
for expensive models (spatial SEIR with 100K population, polio
spatial 5-patch) the data-generation phase itself takes real time,
and parallelising it costs nothing to wire up since the N simulate
calls are independent.

Inside each phase, parallelism is outer-loop only: datasets first,
then fit_seeds within a dataset. IF2/PGAS internals stay
single-threaded per cell — avoids nested parallelism, avoids core
oversubscription. For a 20 × 4 matrix on 8 cores, the fit-phase
runtime is `ceil(80 / 8) × per_cell_time`, which is the expected
behavior.

Cache-hit cells (unchanged inputs, unchanged config) skip execution
and contribute to the summary from cached results — exactly as
single-fit runs do today. Generated datasets are cached by
`(true_params_hash, sim_seed, scenario_name)` so re-runs don't
regenerate data that hasn't changed.

## Provenance

Each cell gets a content hash derived from `(model_ir, fit_config,
data_source, fit_seed)` where `data_source` is either the real data
file hash or `(true_params_hash, sim_seed, scenario_name)` for
synthetic. The top-level `run.json` lists:

- The fit config hash.
- The grid dimensions `(D, M)`.
- Per-cell `(dataset, fit_seed) → content_hash` mapping.

Cache invalidation is per-cell: editing `true_params` invalidates all
synthetic cells; editing `[fit]` invalidates everything; editing
`fit_seeds` only adds new cells without touching existing ones.

## Implementation surface

| File | Change |
|------|--------|
| `rust/crates/cli/src/fit/config.rs` + `config_v2.rs` | Add `fit_seeds: Option<SeedsSpec>`, `fit_starts: Option<FitStarts>`, `[synthetic] SyntheticSpec`. Validate mutual exclusivity. |
| `rust/crates/cli/src/fit/runner.rs` | Accept a `Vec<FitCell>` (one per (dataset, fit_seed) pair). Keep the per-cell path identical to today's single-fit path. |
| `rust/crates/cli/src/fit/synthetic.rs` *(new)* | Generate datasets from `[synthetic]` — thin wrapper over the existing `simulate --obs` pipeline, reusing compiled model + observation block. Write `synthetic/data/ds_NN.tsv`. |
| `rust/crates/cli/src/fit/replicate_grid.rs` *(new)* | Build the (dataset, fit_seed) grid, dispatch to runner, collect results into `summary.tsv` and `coverage.tsv`. |
| `rust/crates/cli/src/fit/summary.rs` *(new or extend)* | Write `summary.tsv` after all cells complete. For synthetic mode, compute and write `coverage.tsv`. |
| `rust/crates/cli/src/fit/main.rs` | `fit run` dispatches to the grid layer when either axis is non-trivial; to the existing single-fit path otherwise. |
| `docs/camdl-inference-spec.md` | New §… "Replicate fits and synthetic-data calibration" — config shape, output layout, coverage semantics. |
| `docs/book/src/guide/fitting-to-data.qmd` | Worked example: SBC on the boarding-school SIR, showing that the MLE distribution brackets truth across 20 datasets even when individual realizations are noisy. |

## Test plan

Six tests, all in `rust/crates/cli/tests/`:

- **`single_fit_lives_under_real_fit_seed_dir`** — fit.toml with
  `fit_seeds = 42`, no `[synthetic]`. Assert output is
  `results/fits/<name>/real/fit_42/scout,refine,validate/`, with no
  bare `scout/` at `real/` or at the top level. Guards "one shape,
  always" and the mandatory `real/` wrapping.
- **`fit_seeds_list_produces_per_seed_dirs`** — `[data]` +
  `fit_seeds = [1,2,3]`. Assert three `real/fit_1/`, `real/fit_2/`,
  `real/fit_3/` directories, one `real/summary.tsv` with three rows
  and distinct content hashes.
- **`synthetic_generates_n_datasets`** — `[synthetic]` with
  `datasets = 5`. Assert five `ds_0N.tsv` files in
  `synthetic/data/`, each distinct, each generated with the declared
  sim seed.
- **`synthetic_recovers_truth_on_well_identified_toy`** *(the real
  check)* — toy SIR with long enough duration to be well-identified
  (`t_end = 100`, daily observations), 20 synthetic datasets from
  truth, one fit each. Assert `coverage.tsv` shows `covers_truth = 1`
  for both β and γ, and that the mean MLE is within 5% of truth on
  each. Guards the statistical correctness of the SBC summary.
- **`synthetic_and_fit_seeds_full_matrix`** — `datasets = 3`,
  `fit_seeds = [1,2]`. Assert six cells ran, `summary.tsv` has six
  rows, layout is `synthetic/ds_0N/fit_<seed>/`.
- **`data_and_synthetic_errors_cleanly`** — both blocks present.
  Assert hard error with a message naming both blocks and pointing
  to "choose one."

## Out of scope

- **Latin-hypercube `fit_starts` mode.** Reserved as a syntactic
  option in config_v2 (for forward-compat) but not implemented in
  the initial landing. `model_default` and `prior` are enough to
  validate the machinery.
- **Hierarchical SBC** (datasets drawn from the prior rather than
  from a fixed truth). Talts et al. prescribe this for true rank
  statistics; we cover the point-truth version first, which is the
  variant the book's teaching chapter needs. Prior-draw SBC is a
  clean follow-up — replace `true_params` with a draw from
  declared priors, everything else stays the same.
- **Per-dataset fit-config overrides.** A future user might want
  different `rw_sd` or `n_iterations` per dataset for diagnostic
  purposes; not worth designing for until we see the request.
- **Automatic parallel tuning** across the two levels of nesting
  (cells × IF2 internal). Outer-loop parallelism is enough for the
  dataset sizes this feature targets.

## Why this design is clean

- **The config shape maps 1:1 to the science.** `[synthetic]` is
  "data generation from truth" and nothing else; `fit_seeds` is
  "fitter variation" and nothing else. A reader of the fit.toml can
  tell what's being varied without reading docs.
- **One output shape, always.** Every fit lives at
  `{real|synthetic}/[ds_NN/]fit_<seed>/<stage>/`. The data-source
  subdirectory is mandatory and makes it impossible to confuse a
  real-data fit with a synthetic-data fit when browsing results or
  composing downstream processing. No single-fit exception, no
  conditional wrapping.
- **No new pipeline.** `scout → refine → validate` runs per cell,
  unchanged. The grid is a sweep layer on top, not a new fit mode.
- **Provenance falls out.** Per-cell content hashes mean the cache
  and the summary table agree on what ran, and editing any piece of
  the grid only invalidates affected cells.
- **Covers the three real use cases in one abstraction.**
  Start-sensitivity on real data, classical SBC, and the full
  dataset × fitter matrix are the same feature viewed from three
  angles. No user needs to learn a second config shape to move
  between them.

---

## Appendix: LOC comparison with the scripted workaround

Before this feature existed, the downstream team ran SBC by hand
with a bash loop that generated N datasets, templated N fit.toml
files, ran each fit, and extracted the MLE columns with `grep` +
`awk`. A side-by-side comparison was recorded while this proposal
was under review; it is reproduced here verbatim as a record of why
the feature was worth building.

### With the proposal

```toml
[model]
camdl    = "boarding_school_sir_poisson.camdl"
scenario = "baseline"

[synthetic]
true_params = "data/sbc/true_params_nok.toml"
sim_seeds   = "1:20"

[estimate]
beta  = { bounds = [0.5, 5.0], start = 1.5 }
gamma = { bounds = [0.1, 1.0], start = 0.4 }

[fixed]
N0 = 763
I0 = 5

[stages.scout]
algorithm     = "if2"
backend     = "chain_binomial"
chains     = 24
particles  = 1000
iterations = 100
cooling    = 0.9

[stages.refine]
algorithm     = "if2"
backend     = "chain_binomial"
chains     = 8
particles  = 2000
iterations = 120
cooling    = 0.95
starts_from = "scout"
```

```
camdl fit run fit_sir_poisson_sbc.toml --seed 42 --parallel 8
```

One file, one command. Output lands at
`results/fits/.../synthetic/` with `summary.tsv` and `coverage.tsv`
pre-computed.

### What was actually written in bash

```bash
# Generate 20 datasets (loop)
for dseed in $(seq 1 20); do
    camdl simulate boarding_school_sir_poisson.camdl \
        --params data/sbc/true_params_nok.toml \
        --scenario baseline --seed $dseed \
        --obs-only data/sbc/syn_poisson_${dseed}.tsv
done

# Create 20 fit configs (loop with heredoc)
for dseed in $(seq 1 20); do
    cat > /tmp/fit_pois_${dseed}.toml << TOML
[model]
camdl = "boarding_school_sir_poisson.camdl"
scenario = "baseline"
[data.observations]
in_bed = "data/sbc/syn_poisson_${dseed}.tsv"
[estimate]
beta  = { bounds = [0.5, 5.0], start = 1.5 }
gamma = { bounds = [0.1, 1.0], start = 0.4 }
[fixed]
N0 = 763
I0 = 5
[stages.scout]
algorithm = "if2"
backend = "chain_binomial"
chains = 24
particles = 1000
iterations = 100
cooling = 0.9
[stages.refine]
algorithm = "if2"
backend = "chain_binomial"
chains = 8
particles = 2000
iterations = 120
cooling = 0.95
starts_from = "scout"
TOML
    camdl fit run /tmp/fit_pois_${dseed}.toml --seed 42
done

# Collect results (another loop)
echo "dseed\tbeta\tgamma\tll" > results/sbc_poisson.tsv
for dseed in $(seq 1 20); do
    mle="results/fits/fit_pois_${dseed}/refine/mle_params.toml"
    beta=$(grep "^beta " $mle | awk '{print $3}')
    gamma=$(grep "^gamma " $mle | awk '{print $3}')
    ll=$(grep "Log-likelihood" $mle | awk '{print $3}')
    echo "$dseed\t$beta\t$gamma\t$ll" >> results/sbc_poisson.tsv
done
```

~45 lines per observation model, three loops, temp file
management, manual result extraction, no provenance, no
parallelism, no coverage stats.

### Counts

| | Lines | Commands | Loops |
|---|---|---|---|
| Proposal (single fit.toml + one `fit run`) | ~21 (TOML) + 1 | 1 | 0 |
| Bash workaround, per observation model | ~45 | 3 (simulate + fit + collect) | 3 |
| Bash workaround, 3 observation models (the actual book chapter) | ~135 + error handling + temp cleanup | 9 | 9 |

Net saving on the book's boarding-school chapter alone: ~115 lines
of boilerplate eliminated, plus correctness gains (provenance
hashes, per-cell cache invalidation, parallel execution, standard
coverage table). The manual-extraction `grep | awk` step is where
the bash workaround was most fragile — any change to the MLE file
format silently breaks the summary.

This comparison motivates the design: not a new algorithm, just
taking a ubiquitous scripted pattern and making it a first-class
operation so the correctness and parallelism concerns get solved
once.
