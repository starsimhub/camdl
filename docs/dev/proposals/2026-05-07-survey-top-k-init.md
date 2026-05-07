---
status: approved (ship-now)
date: 2026-05-07
target: ship in ~1 week — tracks gh#51
issue: https://github.com/vsbuffalo/camdl/issues/51
extends: gh#42 (init_method machinery), gh#43 (camdl survey)
---

# `init_method = "survey_top_k"` — seed multi-chain stages from a survey landscape

## TL;DR

A new `init_method` value that pulls per-chain starting points from
the top-K rows of a `camdl survey` landscape, with typed cross-checks
against the survey's `run.json` provenance and a `chain_starts.tsv`
audit sidecar. Honoured by every stage that already honours
`init_method` (IF2, PGAS, PMMH, NLopt, profile). v1 is strict
K=chains, filter-then-rank for bounds mismatch, SE-aware warn on
rank noise, single survey path, full-hash provenance.

This closes the gh#42 promise of `from_file`-style chain init by
shipping a *typed* version (the cross-check `from_file` couldn't
do against an untyped TSV) and unlocks the obvious next step in
the survey → fit pipeline. The 80,542-nat lever from gh#42
(LHS-spread starts vs single-point starts) is the same lever
here, just letting the global search seed the local search instead
of being a discarded artifact.

## Motivation

`camdl survey` (gh#43) routinely pays for thousands of
likelihood evaluations across declared bounds and writes a clean
CAS-keyed `landscape.tsv` with full `run.json` provenance. The
natural next step — using the top-K of that landscape as IF2 chain
starts in `camdl fit run` — is currently impossible without manual
TOML stitching. Today the only honest options are:

1. **Re-LHS at fit time** → discards every survey evaluation paid for.
2. **Hand-write per-chain TOML / TSV** → no provenance link, won't
   survive a reproducibility audit.

The immediate downstream consumer is
`camdl-book/vignettes/he2010-pomp/`, where a 2,450-point survey
(~2 hours wall on M4 Max) over the seven sbied-profile.R dynamic
parameters has its top-5 cluster on the known γ–σ ridge. The right
next move per the sbied teaching workflow is IF2 starting from
those top points.

## Surface

```toml
[stages.scout]
algorithm      = "if2"
backend        = "chain_binomial"
chains         = 20
init_method    = "survey_top_k"
survey_path    = "results/surveys/he2010_london-eb935038"   # CAS dir
survey_top_k_n = 20                                          # default = chains
```

CLI override:

```
camdl fit run fit.toml --init survey_top_k \
    --survey-path results/surveys/he2010_london-eb935038 \
    [--survey-top-k 20]
```

`survey_path` points at the **CAS directory** (containing `run.json`
and `landscape.tsv`), not the bare TSV. The bare TSV alone is
insufficient — without `run.json` we can't validate that the survey
matches the fit, which is the whole point of preferring this over a
generic `from_file`.

CLI errors out cleanly when `--init survey_top_k` is passed without
`--survey-path` (rather than silently falling back to LHS).

## Stage scope — v1 vs v2

**v1: IF2 only.** The immediate downstream consumer
(`camdl-book/vignettes/he2010-pomp/`) is IF2-context, the 80,542-nat
gh#42 lever was measured on IF2, and the IF2 dispatch site already
has the fit-level cross-check inputs (model_json, effective_obs,
fixed_resolved, estimate names) in scope at the per-stage loop. v1
ships `init_method = "survey_top_k"` working end-to-end on `Stage::IF2`
and refuses cleanly with a "v2 work" diagnostic at the four other
dispatch sites.

**v2: PGAS / PMMH / NLopt / profile.** These are mechanically the same
shape but require plumbing the cross-check context through each stage
launcher (`pgas::run_stage`, `pmmh::run_stage`, `run_nlopt_stage`,
`profile::run_profile`) — a multi-file refactor that the
inference-touch risk profile (CLAUDE.md "Conservatively scoped")
argues for splitting into a follow-up issue once v1 has settled.
The marginal value is genuine but smaller: PGAS/PMMH posteriors mix
past burn-in regardless of seed (the chain's stationary distribution
is set by the prior, not the start), and NLopt's existing
deterministic LHS multi-start already covers the same multi-start
use-case for Sbplx/BOBYQA.

Until v2 lands, every non-IF2 stage with `init_method = "survey_top_k"`
fails fast with `error: init_method = "survey_top_k" is not yet
supported on <stage> stages; v1 supports it on IF2 only — see gh#51`.
Silent fallback to LHS would be the wrong behaviour: the user's
fit.toml asks for survey-seeded chains, and getting LHS instead is a
correctness regression masked as success.

## Validation: the `run.json` cross-check

Before reading a single landscape row, the runner loads
`<survey_path>/run.json` and validates against the resolved fit
inputs:

| Field | Rule |
|---|---|
| `model_hash` | Must match the fit's model_hash exactly. Mismatch → refuse with both hashes printed. |
| `data_hashes` | Must match the fit's data_hashes for any data file the fit consumes. (Survey may reference *more* data files than the fit if it fixed parameters the fit estimates; that's fine.) |
| `[fixed]` | Survey's `[fixed]` block must be a **superset** of fit's. Extra-fixed in survey is fine (fit estimates what survey held fixed → fall back to fit's `[estimate].start`). Differing-fixed-value at any shared key → refuse. |
| `estimated` | Fit's estimated-param set must be a **subset** of survey's estimated-param set. Fit-estimated parameter absent from survey → refuse with the missing name. |
| `bounds` | See "Bounds mismatch: filter-then-rank" below — *not* a hard run.json check. |

Refusal is loud, with a clear diagnostic naming exactly which field
mismatched and showing both values. No `--allow-mismatch` opt-out for
model/data/fixed mismatches; those signal a different inference
problem and should not be papered over.

## Bounds mismatch: filter-then-rank

The naive policy ("clip survey rows into fit bounds, then rank") has
a silent failure mode: a high-loglik survey row gets pinned at a
bound and becomes a worse-than-uniform start. Instead:

1. Read the full landscape TSV.
2. **Filter**: keep only rows whose every parameter value lies
   within fit's bounds for that parameter. No clipping.
3. **Rank** the filtered set by `loglik` desc.
4. Take top-K.

Failure modes:

- Filtered set has fewer than `chains` rows → **refuse** with both
  the original and filtered counts ("survey has 2,450 rows; only 12
  fall within fit bounds, but `chains = 20`").
- Filtered set discards >50% of original rows → **warn**: the user
  is throwing away most of their survey work; they may want to widen
  fit bounds or re-run the survey on the narrower box.

This composes naturally with the survey-wide-fit-narrow workflow
(legitimate, common) without the silent-clip-to-bound failure mode.

## Rank-noise warning: SE-aware threshold

He2010-pomp survey shows median `loglik_se ≈ 1.8 nats` — top-5
ordering is genuinely uncertain at single-nat resolution. Borrowing
the convergence-gate's SE-aware floor rather than picking a
hand-tuned cutoff:

```
threshold_dB = max(decibans_thresh, 8 · σ_max · NATS_TO_DB)
```

where `σ_max` is the largest `loglik_se` among the top-K rows and
`decibans_thresh = 30.0` (matching the scout convergence gate).

Warn — don't refuse — when the top-K's decibans-spread is below
this threshold. The fit will still work; the seeding is just
noisier than rank-1-vs-rank-K's nominal ordering suggests.
Recommended remediations are surfaced in the warning text:
re-run the survey at higher `--eval-replicates`, or widen K beyond
`chains` (v2).

Refusal would punish the user for the survey's measurement budget
rather than helping them; warn is the right pattern, mirroring
camdl's "fail loud, but don't fail spurious" stance.

## Provenance: full-hash, two surfaces

Two artifacts capture the survey → fit linkage, with different
audiences:

### `fit_state.toml` — canonical, machine-readable, summary-input

A single new field:

```toml
chain_init_source = "survey:eb935038c8a4b1f9...:top-20"
```

Format: `survey:<full-hash>:top-<K>`. Full hash, not short — short
hashes collide and the entire point of CAS provenance is
audit-survivable links. For non-survey init methods, the field
holds `"lhs"`, `"single"`, `"uniform"`, etc. — same field, every
fit, no special-casing in the summary surface.

`camdl fit summary` reads this field and renders a one-line header:

```
seeded from: survey:eb935038c8a4b1f9 (top-20)
```

Summary does not parse the sidecar TSV. Source string in
`fit_state.toml` is the trusted artifact; the TSV is auxiliary.

### `chain_starts.tsv` — sidecar, audit-only

Per-chain rows with provenance:

```
chain_id  source                              R0        rho       sigma_se   ...
0         survey:eb935038c8a4b1f9:rank-1     31.42     0.4831    0.1126     ...
1         survey:eb935038c8a4b1f9:rank-2     30.87     0.4914    0.1093     ...
...
```

Lives at the stage root next to the existing `chain_evaluations.tsv`.
Written by every stage with `init_method != "single"` regardless of
source — `lhs` and `uniform` get rows too with `source =
"lhs:rank-N"` etc. This means an auditor can re-derive any chain's
exact start from a single TSV without reading engine internals.

The `source` column is the only camdl-specific one; the rest mirror
`landscape.tsv`'s parameter columns 1:1. Reproducibility audits, not
summary display.

## Mapping survey columns to chain starts

`EstimatedParam.initial` is natural-scale; `Transform` (`Log`,
`Logit`, `None`) applies only to IF2's random-walk perturbations,
not to seeds. Survey TSV columns are also natural-scale. So the
mapping is direct: column → `initial`, no transform-aware conversion.

A unit test asserts this end-to-end: feed an LHS-survey row through
`survey_top_k` and check the resulting `EstimatedParam.initial` is
bit-identical to the survey TSV value (modulo the parameter-name
sort that the fit-side specs impose).

For estimated parameters present in the fit but absent from the
survey (fit estimates ρ, survey held it fixed): fall back to fit's
`[estimate].start`, or (per gh#34) the Transform-aware uniform
fallback if `start` was omitted. This branch is well-tested already.

## Out of scope for v1

Three known extensions, deferred until a real consumer pushes:

1. **K > chains with stratified sub-sampling.** Rank-noise mitigation
   beyond the SE-aware warning. Right algorithm is max-min greedy on
   Transform-scaled parameter distance (spatial spread, not loglik
   spread — IF2 disperses locally; loglik-spread starts redo each
   other's work). Deferred until field reports of chain-clumping at
   strict K=chains.
2. **Multi-survey input.** sbied splits dynamics and IVP surveys
   (`survey_dynamic.toml` + future `survey_ivp.toml`). Outer-product
   of top-K dynamics × top-K IVP is the natural composition.
   Deferred until the dynamics+IVP split pattern is concrete enough
   to type properly.
3. **`init_method = "fit_top_k"`.** Refine seeded from scout's
   `chain_evaluations.tsv` rather than starts_from-style scout-MLE
   handoff. Same surface shape (`<artifact>_path` + run.json
   cross-check + `chain_starts.tsv` provenance), different reader.
   v1's surface is designed with v2's `fit_top_k` in mind so the
   field naming generalises (`survey_path` → `fit_path` would
   collide with TOML `[fit]` idiom; the v2 issue should pick a
   non-colliding name — `from_fit` or similar).

## Implementation sketch

Estimated 150–250 LOC + tests, distributed roughly:

| File | Change | LOC |
|---|---|---|
| `rust/crates/cli/src/fit/init.rs` | New `InitMethod::SurveyTopK` variant; `build_chain_starts_from_survey` reader | ~80 |
| `rust/crates/cli/src/fit/config_v2.rs` | New stage fields `survey_path`, `survey_top_k_n`; CLI parse | ~30 |
| `rust/crates/cli/src/fit/runner.rs` | `run.json` cross-check, filter-then-rank, SE-aware warn, fallback handling | ~60 |
| `rust/crates/cli/src/fit/state.rs` | `chain_init_source: String` field on `FitState`; `chain_starts.tsv` writer | ~30 |
| `rust/crates/cli/src/main.rs` | CLI flag plumbing (`--survey-path`, `--survey-top-k`) | ~15 |
| Tests | Unit tests on filter-then-rank, run.json cross-check refusals, end-to-end he2010-pomp survey → fit | ~80 |

No new dependencies. Reuses the existing
`run_chains_with_per_chain_params` plumbing from gh#42.

## Tests worth adding

- **Cross-check refusals**: model_hash mismatch, data_hashes
  mismatch, estimate-param-not-in-survey, fixed-value-disagreement.
  Each should produce a clean diagnostic naming the offending field.
- **Filter-then-rank**: survey with bounds wider than fit; assert
  filtered count is correct, top-K comes from filtered set, refusal
  fires when filtered count < chains.
- **SE-aware warn**: synthetic survey with controlled
  `loglik_se` per row; assert warn fires when decibans-spread is
  below SE-aware threshold and not when above.
- **End-to-end provenance**: he2010-pomp survey artifact (or a
  miniature version) → fit run → `fit_state.toml` carries
  `chain_init_source` with full hash; `chain_starts.tsv` rows match
  the top-K rows of `landscape.tsv` byte-for-byte on the parameter
  columns.
- **Fallback for fit-estimates-survey-fixed**: fit estimates a
  parameter the survey held fixed → that parameter's chain `initial`
  comes from fit's `[estimate].start` (or gh#34 uniform fallback),
  not from the survey row.

## Open question — chain_init_source on existing fits

Existing `fit_state.toml` files (pre-this-proposal) have no
`chain_init_source` field. Proposed handling: `serde(default)` on
the field, defaulting to `"unknown"`. Summary surface shows
`seeded from: unknown` for old fits, populated correctly for new
fits. No migration; the field is metadata, not load-bearing.
