# External Validation Harness: Design Proposal

**Status:** Proposed **Author:** Vince Buffalo + Claude **Date:** 2026-04-23
**Motivation:** GH #11 — 365× birth deficit + iota miscast were invisible to
every internal test; only a pomp replication surfaced them.

---

## Thesis

The test suites camdl currently runs — `cargo test`, `dune runtest`, golden-IR
equivalence, integration `test_ocaml_to_rust.sh`, synthetic fit replicates —
are all **self-consistency checks**. They verify that camdl does what camdl's
authors think camdl does. They cannot, by construction, catch bugs where
camdl behaves internally consistently but scientifically wrongly.

The 24-hour he2010 incident pair (2026-04-23) was exactly this class: both
bugs compiled cleanly, passed every golden, produced trajectories that looked
qualitatively correct in isolation, and were dimensionally valid. They were
only detectable against an external reference (pomp at the He et al. 2010
published MLE). Each incident cost about a day of pomp-vs-camdl cross-
validation plus a specialist's diagnosis, and the cost does not amortise —
each new reference model replicated is another multi-hour validation pass
whose pass/fail status is invisible to CI.

The proposal: promote external-reference comparison from a vignette-authoring
activity to a first-class regression test layer in `tests/external/`, with a
harness that caches reference output, detects staleness, and runs fast against
cached fixtures in CI without requiring the reference tooling to be installed.
For a simulator that aspires to inform public-health decisions, this
conversion — from day-per-incident diagnosis to one CI line — is directly
load-bearing.

## Motivation

The 2026-04-23 replication incidents
(`docs/dev/incidents/2026-04-23-{iota-toml-unit-silent-miscast,forcing-rescale-double-conversion}.md`)
crystallised the gap. Both bugs emerged from the same vignette (He et al. 2010
London measles) and were only surfaced because someone (a) noticed a
discrepancy against pomp, (b) had a pomp environment ready, and (c) was
willing to drive a multi-hour diagnosis. For a simulator that will host dozens
of reference models over time, that workflow cost amortises badly.

Today those external references live in `camdl-vignettes/bench/` as one-off
scripts run manually during vignette authoring. That workflow found both bugs
— evidence it works — but it is a manual, specialist activity, not a
continuously-green regression layer.

## Design Principles

1. **Cached fixtures are the default path.** The common case — running
   `cargo test --test external` — must not require R, Python, Stan, or any
   external language runtime. Cached reference output is the source of truth
   for the test; regeneration is an explicit, gated operation.
2. **Comparison is on summary statistics, not trajectories.** Given RNG
   differences (ChaCha8 vs Mersenne-Twister vs Stan's Xoshiro256++) camdl and
   the reference tool cannot produce byte-identical paths. Both produce
   ensembles; both are summarised identically; the ensemble summaries are
   compared within per-case tolerances. This is also what scientists care
   about — not path identity.
3. **Reference regeneration is reproducible in distribution, not byte-for-
   byte.** pomp internals have platform-dependent C, R uses the system libm
   for transcendentals, and floating-point summation over 200-seed ensembles
   amplifies last-bit differences into quantile jitter. Cross-platform
   identity is not a goal. *Distributional* reproducibility — that summary
   statistics converge within MC error across architectures — is. Stronger
   reproducibility (for CI regen PRs, where diffs must be meaningful) is
   achieved via Docker, not via floating-point faith.
4. **Manifest-driven staleness detection.** Every fixture carries a MANIFEST
   that records what produced it. The harness refuses to trust a fixture
   whose MANIFEST doesn't match the current reference script, model, or
   parameters — no silent drift.
5. **Tolerances are per-case, set by the case author with a documented
   rationale, not tuned to pass.** The `rationale` field on each check in
   `expected.toml` is load-bearing — it must include a Monte Carlo power
   statement ("this tolerance catches systematic biases of X% or larger with
   power ≥0.95 at N seeds"). Checks without a rationale fail code review.
6. **External ≠ vignette.** The vignette repo documents replications
   narratively for human readers. The external harness gates regressions
   machine-readably. They share models and data but serve different
   audiences; don't merge them.

## Implementation Language — Rust

The harness is a test orchestrator, not a scientific-computing tool. Its
responsibilities are: (a) shell out to a reference script, (b) shell out to
camdl N times with different seeds, (c) read two TSVs, (d) compute per-column
summary statistics, (e) compare against tolerances, (f) read/write a
MANIFEST. Rust handles all of this without new dependencies:
`std::process::Command` for subprocess driving, `serde` + `toml` for the
manifests, a hand-rolled summariser (quantiles over a `Vec<f64>` is ten
lines), and cargo's existing test machinery for invocation.

The alternative — Python — was rejected because it would add a second
test-running idiom (`cargo test` for most things, a Python invocation for
external), a second lint/format config, a second CI caching layer, a "which
Python?" resolution problem on contributor machines, and onboarding docs
covering both. The scientific-computing ergonomics Python would give the
harness aren't needed: quantile and summary-stat computation on a few hundred
seeds of TSV data is trivial in Rust and doesn't need polars.

**Reference scripts remain any-language.** The harness treats
`reference/run.sh` as opaque shell-out, so per-case reference implementations
can be R, Python, Stan, or whatever the reference tooling is — only the
harness itself is Rust.

Shape: a small crate at `rust/crates/external-harness` exposing a binary
invoked via `cargo test --test external`. The binary parses case manifests,
drives subprocess execution, summarises outputs, runs tolerance checks, and
emits structured pass/fail reports.

## Repository Layout

```
tests/external/
  README.md
  cases/
    sir_analytical/                # zero-external-tooling dogfood
      case.toml
      model.camdl
      reference/
        derivation.md              # closed-form final-size solution + citation
      fixtures/
        summary.tsv                # precomputed, committed; edited manually
        MANIFEST.toml              # reference_sha = sha256(derivation.md)
      expected.toml

    he2010_forward/
      case.toml
      model.camdl                  # or path reference into vignettes/
      params.toml
      reference/
        reference.R
        renv.lock
        Dockerfile                 # used by CI regen
        run.sh                     # runs reference.R in local R or in docker
      fixtures/
        summary.tsv
        MANIFEST.toml
      expected.toml

    he2010_pfilter_loglik/ ...
    boarding_school_sir/     ...

rust/crates/external-harness/      # the Rust driver
  src/
    main.rs
    manifest.rs        # MANIFEST + case.toml + expected.toml types
    summarise.rs       # shared summary-stat computation
    compare.rs         # tolerance-aware comparison + crash detection
    subprocess.rs      # camdl + reference invocation with timeout/error capture
  Cargo.toml
```

## Case Manifest (`case.toml`)

```toml
name        = "he2010_forward"
description = "He et al. 2010 London measles forward simulation at published MLE"
category    = "forward-simulation"    # forward-simulation | pfilter | if2 | pmmh | analytical

[camdl]
model   = "model.camdl"
params  = "params.toml"
command = [
    "camdl", "simulate", "@model", "--params", "@params",
    "--backend", "chain_binomial", "--dt", "1",
]
n_seeds   = 200
seed_base = 42

[reference]
kind     = "r-pomp"                   # r-pomp | py-numpyro | stan | analytical
run      = "reference/run.sh"
n_seeds  = 200

[summary]
kind  = "ensemble-stats"
stats = [
    { name = "total_cases",      aggregate = "sum",  over = "weekly_cases", scope = "per-seed" },
    { name = "last52_cases",     aggregate = "sum",  over = "weekly_cases", scope = "last-year-per-seed" },
    { name = "peak_I",           aggregate = "max",  over = "I",            scope = "per-seed" },
    { name = "persistence_rate", aggregate = "frac", over = "last52_cases", threshold = 50 },
]
```

## Expected-Output Manifest (`expected.toml`)

Every check carries a required `rationale` field. The rationale must include
a Monte Carlo power statement.

```toml
[checks.total_cases]
compare   = "mean"
tol_rel   = 0.05
rationale = """
At n=200 seeds the MC SE of the mean is ~0.15% (empirical sd/mean ≈ 0.02 on
pomp 200-seed ensembles). Tol 5% catches systematic biases ≥3.3% at >95%
power. Motivating bug (GH #11 iota miscast) was ~97% bias — surfaces at >600σ.
"""

[checks.total_cases_distribution]
compare   = "quantiles"
q         = [0.025, 0.5, 0.975]
tol_rel   = 0.10
rationale = """
Tail quantiles have larger MC SE than the mean (~1–2% at n=200 for q025/q975).
Tol 10% catches systematic distribution-level biases ≥5% at >90% power.
Catches "mean looks right but variance/skew is off" bugs (e.g., overdispersion
misspecification).
"""

[checks.persistence_rate]
compare   = "proportion-test"
alpha     = 0.01
rationale = """
Binomial SE on a rate scales with √(p(1-p)/n); fixed absolute tolerance would
be too tight at p=0.5 and too loose at p=0.99. Two-sample proportion test at
α=0.01 gives consistent sensitivity across the full [0,1] range. Motivating
bug: he2010 camdl 0/20000 vs pomp 200/200, rejected at p<10^-300.
"""

[checks.peak_I_distribution]
compare   = "quantiles"
q         = [0.5, 0.95]
tol_rel   = 0.15
rationale = """
Peak height varies strongly across seeds (measles dynamics are bimodal in
some years). Wider tolerance acknowledges this while still catching the
"amplitude systematically wrong" class of bug. At n=200 the median MC SE is
~3%, so tol 15% gives ~5× MC SE coverage.
"""
```

Comparison kinds the harness supports:

- `compare = "mean"`: camdl mean vs reference mean, `tol_abs` and/or `tol_rel`.
- `compare = "quantiles"`: per-quantile comparison, `tol_abs` and/or `tol_rel`.
- `compare = "value"`: scalar comparison, for reference outputs with no
  ensemble (e.g., analytical cases).
- `compare = "proportion-test"`: two-sample z-test for proportions, configured
  by `alpha`.
- `compare = "ks-test"`: Kolmogorov–Smirnov test on a marginal distribution,
  configured by `alpha`. For "are these two samples drawn from the same
  distribution?" checks.

## Fixture MANIFEST

Auto-maintained. The harness enforces three hashes:

```toml
# Enforced strictly. Computed from the reference script directory.
reference_sha    = "a3f891..."       # sha256 over reference/**/*

# Enforced strictly. Computed from the case's model + parameters + manifests.
# A model or param change without a reference regen is a test bug.
case_sha         = "e72c1b..."       # sha256(model.camdl + params.toml + case.toml + expected.toml)

# Enforced strictly. Detects summariser changes between fixture generation
# and consumption.
harness_version  = "0.1.0"

# Informational only. Byte-level reproducibility is not a design goal
# (principle #3). Useful for spotting truncation / corruption, not for
# detecting drift.
fixture_sha      = "b90fee..."       # sha256(summary.tsv)

# Provenance metadata (informational).
pomp_version       = "6.4"
r_version          = "4.5.3"
generated_at       = "2026-04-23T12:00:00Z"
generated_on       = "darwin-arm64"
generated_command  = "bash reference/run.sh"
generated_in_docker = false
n_seeds_reference  = 200
seed_base          = 42
```

**Run-record** (separate from MANIFEST; written to
`target/test-external/runs/<case>/<timestamp>/run.toml`, not committed):

```toml
camdl_git_sha     = "6028f74..."
camdl_version     = "0.1.0+6028f74"
host              = "darwin-arm64"
started_at        = "2026-04-23T13:45:02Z"
duration_ms       = 3721
camdl_exit_codes  = [0, 0, 0, ..., 0]    # per-seed
status            = "pass"                # pass | tolerance-fail | crash | stale
```

The distinction matters: the MANIFEST describes *what produced the fixture*;
the run-record describes *what this particular test run did*. Failure
messages can read both to distinguish "reference fixture is stale" from
"camdl has drifted since the fixture was generated" from "camdl crashed on
seed 42".

## Harness Behaviour

### Fast path (default, CI)

```
$ cargo test --test external
```

For each case:

1. Compute `current_reference_sha = sha256 of reference/**/*` and
   `current_case_sha = sha256 of model + params + manifests`.
2. Read `MANIFEST.reference_sha`, `MANIFEST.case_sha`,
   `MANIFEST.harness_version`.
3. If any mismatch → **fail** with a tailored message:
   ```
   FIXTURE STALE: he2010_forward
     MANIFEST.reference_sha  = a3f891... (fixture generated 2026-04-23)
     current reference_sha   = d17a22... (reference/ has been modified since)

   Run:
     CAMDL_REGEN_EXTERNAL=1 cargo test --test external -- he2010_forward
   to regenerate the fixture from the updated reference script.
   ```
4. If all hashes match:
   - Invoke the camdl command `n_seeds` times with seeds
     `seed_base + 0 .. seed_base + n_seeds - 1`.
   - Collect per-seed output TSVs in
     `target/test-external/runs/<case>/<timestamp>/seeds/<i>/`.
   - **Crash detection (see below).** Any seed with a non-zero exit is
     reported as a distinct failure class; the run does not attempt to
     average crashes into the summary.
   - Summarise camdl output into a parallel `summary.tsv`.
   - Apply each check from `expected.toml`. Any failed check → test fails
     with a detailed diff table.

### Regen path (explicit)

```
$ CAMDL_REGEN_EXTERNAL=1 cargo test --test external -- he2010_forward
```

- Invokes `reference/run.sh`, which either:
  - runs `reference.R` in local R (`renv::restore()` first if lockfile newer),
    or
  - runs it in `docker build reference/ && docker run ...` if
    `$CAMDL_EXTERNAL_USE_DOCKER=1` or if local R is absent.
- Rewrites `fixtures/summary.tsv` and `fixtures/MANIFEST.toml`.
- Proceeds with the fast path against the fresh fixture.
- Emits a note; does **not** auto-commit. The fixture diff is the reviewer's
  signal in the subsequent PR.

### CI tiers

- **Fast** (every PR): `cargo test --test external`. No R, no Python. Cached
  fixtures only. Any staleness → CI fails with the regen instruction.
- **Weekly / on-demand** (gated CI job): `CAMDL_REGEN_EXTERNAL=1
  CAMDL_EXTERNAL_USE_DOCKER=1 ...`. Full reference rerun in docker. Compares
  the freshly-regenerated summary to the previously-cached summary via the
  same tolerance machinery, not bitwise — if any stat drifts by more than
  2× its MC SE, opens a PR to update the fixture; otherwise reports "no
  meaningful drift" and does nothing. This catches upstream reference bugs
  (pomp version change, R package drift, reproducibility loss) without
  spurious weekly PRs from last-bit jitter.

### Crash vs tolerance distinction

Harness failure modes are explicitly typed:

- **`stale`**: MANIFEST hash mismatch. Never ran camdl. Message: "fixture
  stale; regen with ...".
- **`crash`**: one or more camdl invocations exited non-zero, OR the
  reference script did. Message: "camdl exited 101 on seed=42; see
  `target/test-external/runs/.../seeds/42/stderr.txt`". No tolerance check
  is attempted — a crashed run cannot be meaningfully averaged into a
  summary.
- **`tolerance-fail`**: all runs completed, summary computed, one or more
  checks exceeded their tolerance. Message includes per-check diff table.
- **`pass`**.

These map to cargo-test exit codes + structured stderr. A tolerance failure
should never look like a crash, and vice versa — silent wrong-answer bugs
manifest as `tolerance-fail`, engineering bugs manifest as `crash`, and the
developer's first five seconds of triage should tell them which.

## Summary Format

A single wide-format TSV, shared between reference and camdl sides:

```
stat_name         mean        sd          q025        q500        q975        n
total_cases       538418.2    11273.4     517903      538105      560772      200
last52_cases      2194.1      14853.7     0           42          54376       200
peak_I            5012.7      1623.8      2184        4876        8441        200
persistence_rate  1.0         0.0         1.0         1.0         1.0         200
```

Small (≤ a few KB per case), diffable, version-control-friendly. Full
trajectories are kept in the run-record directory during test invocation for
debuggability but don't enter the committed fixture.

## `sir_analytical` — the Dogfood Case

The proposal's claim is that the harness's cached-fixture path must work
without external tooling. `sir_analytical` proves this. No R, no Python, no
`renv.lock`, no Dockerfile:

```
cases/sir_analytical/
  case.toml
  model.camdl                        # SIR at R0 = 2
  reference/
    derivation.md                    # closed form: R_∞ = 1 - exp(-R0 · R_∞)
                                     # solved: R_∞ ≈ 0.7968...
                                     # final-size expected values tabulated here
  fixtures/
    summary.tsv                      # hand-computed, committed
    MANIFEST.toml                    # reference_sha = sha256(derivation.md),
                                     # generated_command = "manual: see derivation.md",
                                     # generated_in_docker = false
  expected.toml
```

Staleness works identically: if `derivation.md` ever changes (a bug in the
derivation, a reformulation, a new R0 value), its sha changes, the fixture
becomes stale, and a developer must manually update `summary.tsv` to match
the new derivation before CI passes. That manual regeneration is the
reference tool for this case.

This is a feature, not a workaround. It validates that the harness
infrastructure works end-to-end before any external runtime enters the
picture — a precondition for trusting the harness on harder cases.

## Case Catalogue (v1)

Ordered by implementation priority:

1. **`sir_analytical`** — SIR at R0 = 2, deterministic ODE backend vs
   closed-form final-size equation. Zero external runtime. Proves the
   harness works.
2. **`he2010_forward`** — this proposal's motivating case. R+pomp reference,
   200 seeds, total-cases + persistence-rate + peak-I checks. Locks in the
   issue #11 fix.
3. **`boarding_school_sir`** — pomp's canonical SIR tutorial example.
   Small, fast, widely-known. Good regression coverage for the SIR core.
4. **`he2010_pfilter_loglik`** — pomp's `pfilter` log-likelihood at the MLE
   vs camdl's `particle_filter` log-likelihood, within MC error. Validates
   the observation layer + likelihood machinery, not just the simulator.

## Case Catalogue (future)

- **`he2010_if2`** — pomp's `mif2` MLE vs camdl's `if2` MLE within tolerance.
- **`sir_stan_posterior`** — Stan HMC posterior vs camdl PGAS posterior for
  a small SIR; KS test on marginal posteriors.
- **`seir_numpyro`** — Python-side reference via NumPyro + scipy ODEs.
- **`spatial_sir_pomp2`** — pomp2's spatial SIR; stretch target for the
  spatial-coupling work.

Any new reference model replicated in `camdl-vignettes` should graduate to
`tests/external/cases/` once it stabilises. The vignette is the narrative;
the case is the regression lock.

## Docker Fallback for Reference Regeneration

Each case's `reference/Dockerfile` pins the reference tooling. The CI regen
tier uses Docker by default (`$CAMDL_EXTERNAL_USE_DOCKER=1` is set in the
nightly workflow). Local developer regeneration defaults to native execution
for iteration speed; setting `$CAMDL_EXTERNAL_USE_DOCKER=1` locally is
supported for reproducing CI behaviour.

The MANIFEST records `generated_in_docker` so reviewers of a regen PR can see
which path produced the new fixture.

## What This Proposal Does Not Cover

- **Coverage metrics.** "Does this test exercise `pgas_grad.rs`?" is a
  different question — out of scope.
- **Property-based testing against reference invariants.** E.g., "for any
  R0 in [1, 10], camdl and pomp should agree on final size within 5%".
  Interesting future layer; needs its own design.
- **Performance regression tests.** Wall-time benchmarks vs reference
  tooling — possible but orthogonal; a different harness could read the
  same fixture infrastructure for timing metadata.
- **GUI diff tooling.** A `camdl external diff he2010_forward` that shows
  per-stat comparisons graphically is a nice-to-have; defer until the
  text-mode harness is working.

## Open Questions

1. **Single vs multi-`seed_base`.** Proposal uses one `seed_base` per case.
   Should the nightly regen tier additionally run a second `seed_base` and
   require that *both* pass their checks? This would catch tolerances that
   were inadvertently tuned to a lucky seed. My weak vote: add it to the
   weekly tier, keep the fast tier single-seed for speed.
2. **Partial regen.** If the reference script produces 5 summary stats and
   only 1 changes after a reference-tooling upgrade, do we regen everything
   or just the affected stats? Regen everything is simpler and the fixture
   cost is tiny; stick with that.
3. **Reference-tool version bumps.** Example: pomp 7.0 breaks the he2010
   reference script. Does the camdl PR that bumps `pomp_version` land
   together with the fixture regen? Yes, and CI enforces it: if
   `reference_sha` changed, `fixture_sha` must also have changed.
4. **What about inference runs with stochastic parameter draws?** IF2 MLE
   outputs have their own MC variance on top of per-seed ensemble variance.
   The tolerance machinery accommodates this (wider rel tolerances, ks-test
   instead of mean-comparison), but case authors need guidance; write up
   an "inference-case cookbook" section as cases 4+ go in.

## Cost Estimate

- **Harness MVP (`sir_analytical` + framework):** ~2 days. Analytical case
  needs no external tooling and exercises the full pipeline; includes the
  MANIFEST/staleness machinery which always takes longer than the happy path
  implies.
- **`he2010_forward` case:** ~1 day. Reference script already exists in
  `camdl-vignettes/bench/he2010-forward/`; port, pin renv, generate docker
  image, commit fixture.
- **`boarding_school_sir`:** ~0.5 day.
- **`he2010_pfilter_loglik`:** ~1 day.
- **CI integration + docs + one pass through "the CI environment doesn't
  have X":** ~1 day.

Total: ~5–7 days wall-time for a working v1 covering the class of bugs we
currently can't catch. Subsequent cases: a day each once the template is
stable.
