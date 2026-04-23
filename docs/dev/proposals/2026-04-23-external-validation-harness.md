# External Validation Harness: Design Proposal

**Status:** Proposed
**Author:** Vince Buffalo + Claude
**Date:** 2026-04-23
**Motivation:** GH #11 — 365× birth deficit + iota miscast were invisible to every internal test; only a pomp replication surfaced them.

---

## Motivation

The 2026-04-23 He et al. (2010) replication incident (docs/dev/incidents/2026-04-23-{iota-toml-unit-silent-miscast,forcing-rescale-double-conversion}.md) crystallised a gap in the camdl test surface: **internal unit tests, golden-IR tests, and synthetic-fit tests cannot detect silent wrong-answer bugs that leave the system internally consistent.** Both incidents that day were dimensionally valid, compiled cleanly, simulated without panics, and produced trajectories that looked qualitatively right in isolation. They were only detectable against an external reference.

Today, those external references live in `camdl-vignettes/bench/` as one-off scripts that are run manually during vignette authoring. That workflow found both bugs — evidence it works — but only because someone (a) noticed a discrepancy, (b) had a pomp environment ready, and (c) was willing to drive a multi-hour diagnosis. For a simulator that will host dozens of reference models over time, this cost does not amortise. Each new model replicated is another multi-hour validation pass whose pass/fail status is invisible to CI.

The proposal: promote external-reference comparison from a vignette-authoring activity to a first-class regression test layer in `tests/external/`, with a harness that caches reference output, detects staleness, and runs fast against cached fixtures in CI without requiring the reference tooling to be installed.

## Design Principles

1. **Cached fixtures are the default path.** The common case — running `cargo test --test external` — must not require R, Python, Stan, or any external language runtime. Cached reference output is the source of truth for the test; regeneration is an explicit, gated operation.
2. **Comparison is on summary statistics, not trajectories.** Given RNG differences (ChaCha8 vs Mersenne-Twister vs Stan's Xoshiro256++) camdl and the reference tool cannot produce byte-identical paths. Both produce ensembles; both are summarised identically; the ensemble summaries are compared within per-case tolerances. This is also what scientists care about — not path identity.
3. **Reference regeneration is reproducible.** Pinned dependency lockfiles (`renv.lock`, `uv.lock`, `conda-lock.yml`), optional Docker/Nix image, stored commit hash of the reference script. Regenerating a fixture on Vince's M4 Max should produce the same summary as regenerating it in CI a year later.
4. **Manifest-driven staleness detection.** Every fixture carries a MANIFEST that records what produced it. The harness refuses to trust a fixture whose MANIFEST doesn't match the current reference script — no silent drift.
5. **Tolerances are per-case, not global.** Some cases need 1% tolerance on total cases; some need 20% tolerance on peak heights; some need byte-equal analytical solutions. The harness reads tolerances from each case's `expected.toml`; no global fuzz factor.
6. **External ≠ vignette.** The vignette repo exists to document replications narratively for human readers. The external harness exists to gate regressions machine-readably. They share models and data but serve different audiences; don't merge them.

## Repository Layout

```
tests/external/
  README.md
  lib/
    harness.py              # driver — or .rs, see "Implementation language"
    summarise.py            # shared summary-stat computation
    compare.py              # tolerance-aware comparison
    manifest.py             # MANIFEST.toml read/write
  cases/
    he2010_forward/
      case.toml             # case manifest
      model.camdl           # symlinks or relative paths to vignettes/ where possible
      params.toml
      reference/
        reference.R         # executable that produces summary.tsv
        renv.lock
        Dockerfile          # reproducibility fallback
        run.sh              # "run reference.R in local R or in docker"
      fixtures/
        summary.tsv         # cached reference output (committed)
        MANIFEST.toml       # what produced summary.tsv, when, how
      expected.toml         # acceptance criteria + tolerances
    he2010_pfilter_loglik/
      ...
    boarding_school_sir/
      ...
    sir_analytical/         # reference kind = "analytical"; no external runtime
      case.toml
      model.camdl
      fixtures/summary.tsv
      expected.toml
```

## Case Manifest (`case.toml`)

Minimal required fields; extensible.

```toml
name        = "he2010_forward"
description = "He et al. 2010 London measles forward simulation at published MLE"
category    = "forward-simulation"    # forward-simulation | pfilter | if2 | pmmh | analytical

# What camdl runs
[camdl]
model   = "model.camdl"
params  = "params.toml"
command = ["camdl", "simulate", "@model", "--params", "@params", "--backend", "chain_binomial", "--dt", "1"]
n_seeds = 200
dt      = 1

# What the reference runs (absent for kind = "analytical")
[reference]
kind     = "r-pomp"                   # r-pomp | py-numpyro | stan | analytical
run      = "reference/run.sh"
n_seeds  = 200

# How to summarise each side's output before comparison
[summary]
kind = "ensemble-stats"
stats = [
    { name = "total_cases",      aggregate = "sum",  over = "weekly_cases", scope = "per-seed" },
    { name = "last52_cases",     aggregate = "sum",  over = "weekly_cases", scope = "last-year-per-seed" },
    { name = "peak_I",           aggregate = "max",  over = "I",            scope = "per-seed" },
    { name = "persistence_rate", aggregate = "frac", over = "last52_cases", threshold = 50 },
]
```

## Expected-Output Manifest (`expected.toml`)

Per-case tolerances and test assertions.

```toml
# Each entry: compare the named summary stat's mean/quantiles between
# camdl and reference. Tolerances can be absolute, relative, or both.
# If both are set, either must pass (inclusive).

[checks.total_cases]
compare  = "mean"
tol_rel  = 0.05    # camdl mean within 5% of reference mean

[checks.total_cases_distribution]
compare  = "quantiles"
q        = [0.025, 0.5, 0.975]
tol_rel  = 0.10    # each quantile within 10%

[checks.persistence_rate]
compare  = "value"
tol_abs  = 0.02    # camdl rate within 2 percentage points of reference rate

[checks.peak_I_distribution]
compare  = "quantiles"
q        = [0.5, 0.95]
tol_rel  = 0.15
```

## Fixture MANIFEST

Auto-maintained. Harness refuses fixtures whose MANIFEST doesn't hash-match.

```toml
reference_sha      = "a3f891..."                # sha256(reference.R + renv.lock + run.sh)
pomp_version       = "6.4"
r_version          = "4.5.3"
generated_at       = "2026-04-23T12:00:00Z"
generated_on       = "darwin-arm64"
generated_command  = "bash reference/run.sh"
fixture_sha        = "b90fee..."                # sha256(summary.tsv)
harness_version    = "0.1.0"
n_seeds_reference  = 200
seed_base          = 42
```

## Harness Behaviour

### Fast path (default, CI)

```
$ cargo test --test external
```

For each case:
1. Compute `current_sha = sha256(reference.R + renv.lock + run.sh)`.
2. Read `MANIFEST.reference_sha`.
3. If mismatch → **fail** with:
   ```
   FIXTURE STALE: he2010_forward
     MANIFEST.reference_sha = a3f891... (generated 2026-04-23)
     current_sha            = d17a22... (reference script modified since)
   Run `CAMDL_REGEN_EXTERNAL=1 cargo test --test external -- he2010_forward` to regenerate.
   ```
4. If match:
   - Invoke the camdl command (`case.toml [camdl] command`) `n_seeds` times with deterministic seeds.
   - Summarise camdl output into a parallel `summary.tsv` using the shared summariser.
   - Apply each check from `expected.toml`. Any failed check → test fails with a detailed diff table.

### Regen path (explicit)

```
$ CAMDL_REGEN_EXTERNAL=1 cargo test --test external -- he2010_forward
```

- Invokes `reference/run.sh`, which either:
  - runs `reference.R` in local R (`renv::restore()` first if lockfile newer), or
  - runs it in `docker build reference/ && docker run ...` if `$CAMDL_EXTERNAL_USE_DOCKER=1` or local R is absent.
- Rewrites `fixtures/summary.tsv` and `fixtures/MANIFEST.toml`.
- Proceeds with the fast path against the fresh fixture.
- Stages the fixture changes (harness emits a note; doesn't auto-commit).

### CI tiers

- **Fast** (every PR): `cargo test --test external`. No R. Cached fixtures only. Staleness → fail.
- **Weekly / on-demand**: nightly job with `CAMDL_REGEN_EXTERNAL=1`, full reference rerun. Opens a PR if fixtures drift. Catches upstream reference bugs (pomp version change, R package drift, reproducibility loss).

## Summary Format

A single TSV format, used by both the reference side and the camdl side:

```
stat_name         mean        sd          q025        q500        q975        n
total_cases       538418.2    11273.4     517903      538105      560772      200
last52_cases      2194.1      14853.7     0           42          54376       200
peak_I            5012.7      1623.8      2184        4876        8441        200
persistence_rate  1.0         0           1.0         1.0         1.0         200
```

Small, diffable, trivially version-controlled. Full trajectories stay on disk during active debugging but don't enter the committed fixture.

## Implementation Language

Two workable choices:

**Option A: Python harness (`lib/harness.py`)**
- Pros: easier to write ensemble statistics (pandas/polars/numpy), easy to call arbitrary external tools (R via subprocess, Python reference code directly), easy docker/conda integration.
- Cons: a second language in the repo; adds a Python dependency (uv) even for users who don't run external tests.

**Option B: Rust harness** (integrated into `rust/crates/cli/tests/external_*.rs`)
- Pros: no new language; uses polars via the existing Rust build; summaries can share code with the CLI's existing TSV handling.
- Cons: driving R subprocesses and container regeneration from Rust is awkward; ensemble stats libs are less ergonomic than Python's.

**Recommended: Python harness.** Vince already uses uv + polars heavily; the harness is fundamentally a scientific-tooling-driver job, which is Python's comfort zone; the "external" label already implies a willingness to depend on multiple language runtimes. The Python dependency can be pinned in `tests/external/pyproject.toml` (uv-managed) and is not required for `cargo test` unless the `external` harness is explicitly invoked.

## Case Catalogue (v1)

Ordered by implementation priority:

1. **`sir_analytical`** — SIR at R0=2, deterministic ODE backend vs closed-form final-size equation `R_∞ = 1 - exp(-R0 · R_∞)`. Zero external runtime. Good end-to-end exercise of the harness pipeline.
2. **`he2010_forward`** — this PR's motivating case. R+pomp reference, 200 seeds, persistence-rate + total-cases + peak-I checks. Once in, issue #11 cannot regress.
3. **`boarding_school_sir`** — pomp's canonical SIR tutorial example. Small data, fast simulation, widely-known reference values. Good regression coverage for the SIR core.
4. **`he2010_pfilter_loglik`** — pomp's `pfilter` log-likelihood at the MLE, compared to camdl's `particle_filter` log-likelihood, within Monte Carlo error. Validates the observation layer + likelihood machinery, not just the simulator.

## Case Catalogue (future)

- **`he2010_if2`** — pomp's `mif2` MLE vs camdl's `if2` MLE within tolerance. Validates the inference engine against a published result.
- **`sir_stan_posterior`** — Stan HMC posterior for a small SIR vs camdl PGAS posterior; KS test on marginal posteriors. Validates the Bayesian machinery.
- **`seir_numpyro`** — Python-side reference via NumPyro + scipy ODEs.
- **`spatial_sir_pomp2`** — pomp2's spatial SIR as a stretch target for the spatial-coupling work.

Any new reference model replicated in `camdl-vignettes` should graduate to `tests/external/cases/` once it stabilises. The vignette is the narrative; the case is the regression lock.

## Docker / Nix Fallback for Reference Regeneration

Each case's `reference/Dockerfile` pins the reference tooling. Opt-in via `$CAMDL_EXTERNAL_USE_DOCKER=1`. The MANIFEST records whether the fixture was generated locally or in docker; fixture regeneration is deterministic either way because `renv.lock` pins all R packages and the `Dockerfile` pins R itself.

Nix variant possible but lower priority — docker is ubiquitous in CI, nix is Vince-only for now.

## What This Proposal Does Not Cover

- **Coverage metrics.** "Does this test exercise `pgas_grad.rs`?" is a different question — out of scope.
- **Property-based testing against reference invariants.** E.g., "for any R0 in [1, 10], camdl and pomp should agree on final size within 5%". Interesting future layer, but needs its own design — statistical power analysis, tolerance design, not just cached fixtures.
- **Performance regression tests.** Wall-time benchmarks vs reference tooling — possible but orthogonal; a different harness could read the same fixture infrastructure for timing metadata.
- **GUI diff tooling.** Nice-to-have `camdl external diff he2010_forward` command that shows per-stat comparisons graphically; defer until the text-mode harness is working.

## Open Questions

1. **Where does random seeding enter the case manifest?** Proposal above uses a `seed_base` in MANIFEST plus per-run seeds `seed_base + i`. Should tolerance-checks be tight enough that one deterministic `seed_base` suffices, or do we want the harness to run multiple `seed_base` values to sanity-check that tolerances aren't tuned to a lucky seed? Err on the side of running a second seed_base nightly as part of the regen tier.
3. **Partial regen.** If the reference script produces 5 summary stats and only 1 changes after a reference-tooling upgrade, do we regen everything or just the affected stats? Regen everything is simpler and the fixture cost is tiny; stick with that.
4. **Who owns a case when the reference tool changes upstream?** Example: pomp 7.0 breaks the he2010 reference script. Does the camdl PR that bumps `pomp_version` land together with the fixture regen, or separately? Together, and enforce in CI: "if reference_sha changed, fixture_sha must also have changed."

## Cost Estimate

- **Harness MVP (`sir_analytical` + framework):** ~1 day. Analytical case needs no external tooling, exercises the full pipeline.
- **`he2010_forward` case:** ~1 day. Reference script already exists in `camdl-vignettes/bench/he2010-forward/`; port, pin renv, commit fixture.
- **`boarding_school_sir`:** ~0.5 day.
- **`he2010_pfilter_loglik`:** ~1 day.
- **CI integration + docs:** ~0.5 day.

Total: ~4 days wall-time for a working v1 covering the bugs-that-got-past-us class directly. Subsequent cases: a day each once the template is stable.

## Closing Note

The broader claim: the test suites camdl currently runs (cargo test, ocaml dune runtest, golden IR equivalence, integration test_ocaml_to_rust.sh) are all **self-consistency checks**. They verify that camdl does what camdl's authors think camdl does. They cannot — by construction — catch bugs where camdl does something internally consistent but scientifically wrong. The external validation harness closes that gap for the specific class of bugs that matters most: silent divergence from a published, peer-reviewed reference implementation at known parameters.

The 24-hour he2010 incident pair cost about a day of diagnosis each. The harness, once built, converts that cost into a CI line. For a simulator that aspires to inform public-health decisions, this conversion is directly load-bearing.
