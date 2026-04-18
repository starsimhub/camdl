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
datasets    = 20                      # required, ≥ 1
sim_seeds   = "1:20"                  # required; string range or list
scenario    = "baseline"              # optional; default: no scenario
```

- `true_params` — a TOML file with `name = value` lines (same format
  as the existing `--params` file). The values are the ground truth
  used to generate data; downstream SBC statistics (bias, coverage)
  use these.
- `datasets` — number of synthetic realizations to produce. Must
  equal `len(sim_seeds)`; mismatch is a hard error.
- `sim_seeds` — either an integer-range string (`"1:20"`) or an
  explicit list (`[7, 42, 101, ...]`). Duplicates are rejected.
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
- `datasets ≠ len(sim_seeds)`: hard error.
- `sim_seeds` ∩ `fit_seeds` nonempty: warning, not error (different
  namespaces, but confusing to read in logs).

## Output layout

### Single fit (today, unchanged)

No `fit_seeds`, no `[synthetic]` block:

```
results/fits/<name>/
  scout/
  refine/
  validate/
  run.json
```

### Fit-seed sweep on real data

`[data]` + `fit_seeds = [101, 102, 103]`:

```
results/fits/<name>/
  fit_101/
    scout/
    refine/
    validate/
  fit_102/
    ...
  fit_103/
    ...
  summary.tsv          # one row per fit_seed
  run.json             # top-level provenance; references per-fit hashes
```

The `fit_NNN/` wrapping appears exactly when there is more than one
fit to disambiguate — never for a scalar `fit_seeds` and never when
the block is omitted.

### Synthetic-data fit

`[synthetic]` block present, any `fit_seeds` shape:

```
results/fits/<name>/
  synthetic/
    ds_01/
      fit_101/
        scout/
        refine/
        ...
      fit_102/
        ...
    ds_02/
      fit_101/
      ...
    ...
    summary.tsv        # one row per (ds, fit_seed)
    coverage.tsv       # per-parameter coverage + bias vs truth
    truth.toml         # copy of the ground-truth params (provenance)
    data/
      ds_01.tsv        # generated datasets, one per sim_seed
      ds_02.tsv
      ...
  run.json
```

The `synthetic/` subdirectory is the visual cue that everything under
it was fit against data generated from known truth. SBC-specific
statistics (bias vs. truth, coverage of truth by the MLE distribution)
are written only here; they are not meaningful for real-data fits.

When `fit_seeds` collapses to a scalar, the `fit_NNN/` wrapping under
each `ds_NN/` still disappears — a single fit per dataset lives
directly at `synthetic/ds_NN/scout/` etc. This keeps the directory
shape minimal for classical SBC (N datasets, one fit each).

## Canonical modes

| Mode                        | `[synthetic]` | `fit_seeds`  | Output           | Fits  |
|-----------------------------|---------------|--------------|------------------|-------|
| Single fit (today)          | —             | —            | flat             | 1     |
| Start-sensitivity           | —             | list, len M  | `fit_NNN/`       | M     |
| SBC (classical)             | N datasets    | — or scalar  | `synthetic/ds_NN/` | N     |
| SBC × start-sensitivity     | N datasets    | list, len M  | `synthetic/ds_NN/fit_NNN/` | N × M |

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

`--parallel P` parallelises across the outer loop: datasets first,
then fit_seeds within a dataset. IF2/PGAS internals stay
single-threaded per cell — avoids nested parallelism, avoids core
oversubscription. For a 20 × 4 matrix on 8 cores, the runtime is
`ceil(80 / 8) × per_cell_time`, which is the expected behavior.

Cache-hit cells (unchanged inputs, unchanged config) skip execution
and contribute to the summary from cached results — exactly as
single-fit runs do today.

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

- **`single_fit_unchanged`** — fit.toml with no `fit_seeds`, no
  `[synthetic]`. Assert output layout is exactly today's (flat
  `scout/ refine/ validate/`, no wrapping). Guards the "zero-impact
  when block absent" claim.
- **`fit_seeds_on_real_data_produces_per_seed_dirs`** — `[data]` +
  `fit_seeds = [1,2,3]`. Assert three `fit_1/`, `fit_2/`, `fit_3/`
  directories, one `summary.tsv` with three rows and distinct
  content hashes.
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
  rows, layout is `synthetic/ds_0N/fit_N/`.
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
- **Collapses orthogonally.** Each axis disappears independently
  when absent, with no wrapping directories, no extra files, no
  noise in the output layout. Single fits are indistinguishable from
  today's single fits.
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
