# Nightly External-Validation Regen: Design Proposal

**Status:** Proposed (deferred; not built yet)
**Author:** Vince Buffalo + Claude
**Date:** 2026-04-23
**Related:** `docs/dev/proposals/2026-04-23-external-validation-harness.md`
(parent proposal; defines the fast-path/regen-path split this nightly implements).

---

## Problem

The L9 fast path (`cargo test --test external_validation`) catches
tolerance-fails against the **cached** reference fixtures. It does not and
cannot catch **reference-tool drift** — the case where pomp ships a new
version, or the runner's libm subtly changes, and fresh reference output
stops matching the committed `fixtures/summary.tsv`. Without an external
pulse, that drift accumulates silently. In six months someone may see a
500-nat shift and have no way to tell whether it's a camdl regression or
an unrelated reference-environment change from months ago.

## Goal

A scheduled CI job that runs the L9 regen path against the real
reference tooling (R + pomp, eventually NumPyro, Stan) and flags
meaningful drift via a reviewable PR. Never auto-merges — the maintainer
decides whether drift is "expected upstream update, accept" or "something
is actually wrong."

## Design Decisions

### 1. Daily, not on-PR

Fast path (cached fixtures) gates every PR; it's already in
`make test-rust`. The nightly is for the slower drift concern and should
not gate merges.

### 2. One case at first, expand later

Start with `he2010_forward`: richest covariate + event + noise coverage,
most likely to surface any pomp-side drift. Once the workflow is stable
and the review cadence is understood, add `boarding_school_sir` and
`he2010_pfilter_loglik`. `sir_analytical` doesn't benefit — no external
tooling.

### 3. Conservative drift gate (`2× MC SE`)

Bitwise fixture diff is useless — every run produces different last bits
thanks to BLAS/libm variance. The nightly should compare each summary
statistic against its cached value, normalised by the statistic's
committed SD:

```
drift_sigma = |new_mean - cached_mean| / (cached_sd / sqrt(cached_n))
```

Open a PR iff any stat has `drift_sigma > 2.0`. That threshold keeps
MC-noise-driven drift from opening PRs daily while catching real 3σ+
shifts within one or two runs. The threshold can be tightened later if
we see too many false positives, or loosened if we see too many false
negatives — for v1, be conservative (more false positives is fine; false
negatives defeat the purpose).

### 4. Linux runner, no Docker (yet)

- `ubuntu-latest` with `r-lib/actions/setup-r@v2` is the well-trodden
  path; pomp itself uses this for its CI.
- renv library cached between runs via `actions/cache` keyed on
  `renv.lock` hash. First run: ~5 min for CRAN pomp install. Steady
  state: ~1 min setup.
- Docker stays stubbed in `run.sh` as originally proposed. If the native
  path gets flaky, flip the default. Not a blocker for v1.

### 5. PR labels, no auto-merge

Opens a PR labelled `nightly-regen` + `L9-review`. Maintainer triages.
Merging it updates `fixtures/summary.tsv` + `MANIFEST.toml` — the
standard regen output, reviewed the normal way. If CI on that PR
fails (the fast path fails against the updated fixture), there is a
real problem worth investigating.

### 6. The nightly runs against `main`, always

Drift PRs land on the default branch. Feature branches don't get their
own nightly — they already got the fast-path check on every push, which
is sufficient until they merge.

## Workflow Skeleton (indicative, not committed)

```yaml
name: External validation — nightly regen
on:
  schedule: [{ cron: '0 7 * * *' }]     # daily 07:00 UTC
  workflow_dispatch:                     # manual trigger too

jobs:
  regen:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: r-lib/actions/setup-r@v2
      - uses: r-lib/actions/setup-renv@v2
        with:
          working-directory: tests/external/cases/he2010_forward/reference

      - name: Build harness + camdl
        run: |
          make -C . install
          cargo build -p external-harness --manifest-path rust/Cargo.toml

      - name: Regen he2010_forward
        run: ./rust/target/debug/external-harness regen tests/external/cases/he2010_forward

      - name: Compare new vs cached summary; open PR on meaningful drift
        run: ./scripts/external_nightly_gate.sh he2010_forward
        # Gate script decides exit 0 (meaningful drift → open PR)
        # vs exit 1 (no meaningful drift → workflow ends green, no PR).

      - uses: peter-evans/create-pull-request@v6
        if: success()       # i.e., gate script exited 0
        with:
          branch: nightly-regen/he2010_forward
          title: "L9 nightly: he2010_forward fixture drifted"
          labels: nightly-regen, L9-review
          body: |
            Daily regen of he2010_forward surfaced meaningful drift
            (>2σ on at least one summary stat) against the committed
            fixture. Attached diff in the PR shows each stat's
            old-vs-new numerics.

            Typical triage: (a) identify the upstream cause (pomp
            bump, renv lock change, runner libm update), (b) if
            scientifically benign, accept by merging; (c) if
            concerning, dig into which case parameter moved and why.
```

Plus a small `scripts/external_nightly_gate.sh` (~30 lines) that:
- Reads `fixtures/summary.tsv` (cached, pre-regen state — fetched from
  `main` rather than the regenerated copy)
- Reads the freshly-regenerated `fixtures/summary.tsv`
- Computes `drift_sigma` per stat
- Exits 0 if any stat exceeds 2.0; exits 1 otherwise

## What This Proposal Does Not Cover

- **Matrix across OS / R version**: a Linux runner with the pinned renv
  is the only axis for v1. Adding macOS or nightly-R as a matrix axis
  would catch more subtle drift but also open more false-positive PRs.
  Defer.
- **Publishing drift metrics to a dashboard**: the PR body is the
  reporting surface for v1. If a dashboard ever becomes useful, pipe
  the gate script's output to Grafana or similar; not now.
- **Handling the R library cache across the four-case expansion**: each
  case's `reference/renv.lock` is separate; the cache should key on the
  set of lockfiles in play, not just one. Refine when we expand past
  `he2010_forward`.
- **Python-side references** (NumPyro, Stan): will use `setup-python` +
  `uv` equivalent; same shape. Parallel track when those cases exist.

## Estimated Cost to Implement

- Workflow YAML + renv cache setup: ~30 min
- `external_nightly_gate.sh` + tests: ~1 h
- First-real-run debugging (pomp install, renv quirks, macOS → Linux
  fixture discrepancies surfacing and needing absorption into
  tolerances): ~1–2 h
- Docs + reviewer-onboarding note in `testing.md`: ~30 min

Total: half a focused day, probably spread across two sessions because
of CI-debug round-trip latency.

## When to Build

Trigger to prioritise:
- When we add a second R-based case (`boarding_school_sir` or
  `he2010_pfilter_loglik`) to main and want coverage on both
- When pomp or renv publishes a meaningful upstream change and we want
  early notification
- When someone (not Vince) starts contributing cases — they need the
  drift-detection loop to be automated for their contributions to be
  trustworthy

Not-triggers (don't build for these):
- "We've been meaning to wire more CI"
- "It feels incomplete without a nightly"

The fast-path gate is doing its job on every PR; the nightly is for
a specific slow-drift failure mode. Build it when that failure mode
becomes concrete.
