# Incident: `[synthetic]` and `--obs-only` diverge at the same seed

**Date:** 2026-04-18
**Status:** Fixed.
**Severity:** Medium. Same nominal seed produced different synthetic
observations across two supposed-equivalent CLI paths; silently wrong
rather than catastrophically wrong. No inference result was
mathematically incorrect, but SBC batches generated via the two paths
could not be reconciled, which is a reproducibility bug.

## Fundamental vs. implementation

**Fundamental:** nothing. The process RNG was seeded identically in
both paths (`backend.run(seed = sim_seed)`) — trajectories at the same
seed are bit-identical. Only the *observation* RNG diverged, because
the two entry points mixed the seed with different decorrelation masks
before constructing the obs `StatefulRng`.

**Implementation:** two copies of the mix constant existed in two
files. `main.rs:386` declared `SEED_MIX_OBS = 0xa5a5a5a5a5a5` for the
`camdl simulate --obs` / `--obs-only` path;
`rust/crates/cli/src/fit/synthetic.rs` had a locally-scoped constant
`0xa7c1_e890_7f2c_1d3a`. Whoever landed the `[synthetic]` data
generator wrote the local constant without consulting the existing
one. Everything downstream — same `StatefulRng::new(...)`, same
sampler pipeline — was otherwise identical.

## Summary

Downstream ran two SBC batches on the same model with the same true
parameters:

- Batch A: 20 datasets generated with `camdl simulate --obs-only
  --seed $k` (k = 1..20). Posterior β showed +59 % bias vs truth.
- Batch B: 30 datasets generated with `camdl fit run` using
  `[synthetic] sim_seeds = "1:30"`. Posterior β showed –0.4 % bias.

Both fits used the same binary, same model, same likelihood. The
bias was entirely in *which stochastic realizations reached the
fitter*. The `--obs-only` path at seeds 1..20 happened to produce an
unrepresentative batch of obs draws on top of the (identical)
underlying trajectories; `[synthetic]` at seeds 1..30 produced a more
representative batch. The +59 % figure looked like a bug in IF2 or
the likelihood, but was an artefact of the two seed-mix constants.

## Blast radius

- **Any user comparing an `--obs-only` SBC to a `[synthetic]` SBC
  on the same model + seeds** would get non-matching results. The
  discrepancy is deterministic given the mix constants, so the same
  user hitting the same paths twice got the same discrepancy
  reliably — not flaky, just reproducibly wrong.
- **`camdl simulate --obs` with real model runs** (single-use
  synthetic data, not part of an SBC comparison): unaffected. The
  obs draws are still a valid Poisson/NegBin/etc sample from the
  likelihood. Just not reproducible via the other path.
- **No impact on trajectories.** Both paths seed the process RNG
  identically, so compartment counts and transition flows were
  always bit-identical at the same seed. Only the obs noise on top
  differed.
- **No impact on `camdl fit run` with real `[data]`.** The obs mix
  constant isn't on that path at all — the fit reads data files and
  evaluates the likelihood; it doesn't sample obs noise.

## How we found it

Downstream (the book agent) set up two SBC batches during validation
work for the "Fitting to Data" chapter and noticed one showed +59 %
bias and the other –0.4 %. Both were rerun and the numbers
reproduced. Because the fitting code was identical in both runs
(same binary, same fit.toml templates), suspicion landed on the data
side. Byte-comparing the synthetic TSVs at nominally equivalent seeds
showed they differed from row 1. That plus the observation that day-0
values were identical (obs noise from a near-deterministic starting
state) pointed at "obs RNG diverges immediately but process RNG
doesn't" — which fingered the mix constant.

Fix was a one-line change: replace the local constant in
`synthetic.rs` with `crate::util::SEED_MIX_OBS`, promoted from
main.rs to `util.rs` as the canonical home.

## The fix

`rust/crates/cli/src/util.rs`: add `pub const SEED_MIX_OBS: u64 =
0xa5a5a5a5a5a5` with a doc comment explaining the invariant that any
code path sampling synthetic observations must use this constant.

`rust/crates/cli/src/main.rs`: drop the local `const SEED_MIX_OBS`,
replace with `use util::SEED_MIX_OBS`.

`rust/crates/cli/src/fit/synthetic.rs`: drop the local (differently-
valued) constant, reference `crate::util::SEED_MIX_OBS`.

## Regression test

`rust/crates/cli/tests/synthetic_fit_grid.rs::
obs_only_and_synthetic_agree_byte_for_byte_at_same_seed` — generates
one dataset via each path at the same seed and asserts
`cli_bytes == syn_bytes`. Fails red against the old code (local
constants diverge → file bytes diverge), passes against the fix.

Verified by hand: `--obs-only --seed 10` and `[synthetic] sim_seeds =
[10]` now produce identical `ds_01.tsv`:

```
time  cases
0     1
1     1
2     5
3     6
4     6
5     8
6     14
7     18
8     33
9     49
10    93
```

## How did this escape

The `[synthetic]` data generator was landed recently (step 3 of the
synthetic-fit-replicates proposal, 2026-04-17) and its generation
tests (colocated in `fit::synthetic::tests`) exercised the
`[synthetic]` path in isolation — determinism *within* the path was
asserted (`same_seed_produces_identical_content_hash`), but parity
*across* paths was not. A cross-path parity test was obvious in
retrospect: two supposedly-equivalent CLI entry points for the same
operation must agree, bit-for-bit, at the same seed.

The decorrelation mask was also never documented as a public API
contract. Both constants looked like random bit salt to a reader,
neither was tied to a particular source of truth; writing a second
one did not trigger the author's "don't duplicate this" reflex.

## Followup actions

- **Done in this fix:** one canonical `SEED_MIX_OBS` in `util.rs`;
  regression test; this report; docstring explaining why it's
  public.
- **Not done; worth considering:**
  - Audit other seed-mix constants in main.rs (`SEED_MIX_DRAW`,
    `SEED_MIX_REP`, `SEED_MIX_UNIFORM`, `SEED_MIX_PRIOR`) for
    duplicated-or-likely-to-duplicate patterns as the CLI grows.
    They look fine today but the same class of bug applies.
  - Add the "both CLI paths produce identical bytes at same seed"
    invariant as a general property-check in CI — would catch the
    next copy of this bug before a downstream user hits it.

## Lessons

- **Decorrelation masks are API, not implementation.** Any constant
  that feeds into an external-facing RNG stream is part of the
  contract between the user and the tool. Two CLI paths that agree
  on their user-facing shape (same seed, same model, same
  parameters) must agree on the bits too. Putting the constant in a
  private module invites the exact duplication that happened here.
- **"Looks different" from "is wrong" can take hours to
  disentangle.** The downstream team initially suspected a fitter
  bug (biased MLE), but the problem was in the data-generation
  side. The time lost to this was proportional to how plausible
  "broken fitter" was as a hypothesis, which was high because IF2
  is the more complex component. When two results that should agree
  don't, check data parity first — it's cheaper to rule out.
- **A test that a feature works in isolation is not a test that it
  composes.** The `[synthetic]` generator had a
  `same_seed_produces_identical_content_hash` test. That test
  passed. The bug was *between* that feature and another feature.
  The missing test was the cross-feature parity check.
