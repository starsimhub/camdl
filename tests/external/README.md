# External Validation Harness

Regression tests that compare camdl's output against cached results from
external reference implementations (pomp, NumPyro, Stan, analytical).

Design: [docs/dev/proposals/2026-04-23-external-validation-harness.md](../../docs/dev/proposals/2026-04-23-external-validation-harness.md)

## Layout

```
tests/external/
  cases/                          # one directory per test case
    sir_analytical/               # dogfood case — zero external tooling
      case.toml                   # what to run
      model.camdl                 # the camdl model
      params.toml
      reference/
        derivation.md             # closed-form source of truth
      fixtures/
        summary.tsv               # reference summary stats (committed)
        MANIFEST.toml             # hashes + provenance
      expected.toml               # tolerances + MC power rationale
  runs/                           # per-run artifacts (gitignored)
    <case>/<timestamp>/
      seeds/<seed>/{obs.tsv, stdout.log, stderr.log}
      camdl_summary.tsv
```

## Running

From the repo root:

```bash
# All cases via cargo test (the pre-push/CI path — no external tooling)
cargo test --test external_validation --manifest-path rust/Cargo.toml -- --nocapture

# Or via the harness binary directly (built by cargo when needed)
cd rust && cargo build -p external-harness
./target/debug/external-harness run-all

# Single case
./target/debug/external-harness run ../tests/external/cases/sir_analytical

# Regen (rebuilds cached fixture from the reference tool; requires R/etc.)
CAMDL_REGEN_EXTERNAL=1 ./target/debug/external-harness run-all
./target/debug/external-harness regen ../tests/external/cases/he2010_forward
```

Expected `run-all` output on success:

```
running 3 external-validation cases under tests/external/cases

  run    boarding_school_sir            pass (2 checks, 0.2s)
  run    he2010_forward                 pass (3 checks, 0.8s)
  run    sir_analytical                 pass (1 checks, 0.3s)

── summary ──
3 passed, 0 failed, 0 stale, 0 crashed  in 1.3s
```

## Adding a new case

1. Create `tests/external/cases/<name>/` with `case.toml`, `expected.toml`,
   `model.camdl`, and `params.toml`.
2. For analytical cases: write `reference/derivation.md` and
   hand-author `fixtures/summary.tsv`.
3. For external-tool cases: write `reference/run.sh` (+ `renv.lock`,
   `Dockerfile`, etc.) — harness regen will invoke it.
4. Bootstrap the fixture MANIFEST:
   ```bash
   external-harness bootstrap tests/external/cases/<name> --write
   ```
5. Run the case:
   ```bash
   external-harness run tests/external/cases/<name>
   ```

## Adding a check

Every check in `expected.toml` must carry a `rationale` field that
includes a Monte Carlo power statement. Example:

```toml
[checks.some_stat]
compare   = "mean"
tol_rel   = 0.01
rationale = """
At n=200 seeds and typical variance, MC SE of the mean is ~0.15%. Tol
1% catches systematic biases ≥ 0.9% with power ≥ 0.95. Motivating bug
class: ...
"""
```

The harness rejects checks with empty rationales at load time.

## Current cases

- **sir_analytical** — bare SIR at R0=3, chain_binomial backend, compared
  to the closed-form final-size `r_∞ ≈ 0.9405`. Runs in <1s with 20
  seeds. Requires no external tooling; regeneration of its fixture is
  manual (edit `derivation.md` and `fixtures/summary.tsv` together).
- **he2010_forward** — He et al. 2010 London measles at the published
  MLE vs pomp 6.4. Environmental Gamma noise, interpolated pop +
  birthrate covariates, cohort school-entry pulse, term-time
  seasonality. Regression lock for GH #11 (iota + forcing-rescale
  double-conversion bugs). 20 camdl seeds vs 200 pomp seeds; total
  21-year cases within ~0.3% of pomp's ensemble mean, persistence
  rate 20/20 vs 200/200.
- **boarding_school_sir** — Anderson & May (1991) boarding-school flu
  narrative via pomp's canonical SIR: closed population of 763, R0=3,
  14-day window. Structurally simplest pomp case; validates
  chain_binomial vs reulermultinom on a bare SIR without covariates,
  events, or inhomogeneous mixing. Total infections agree to ~0.1%,
  peak daily infections to ~0.4%.
- **he2010_pfilter_loglik** — particle-filter log-likelihood of He et al.
  2010 London measles at the published MLE. Sibling to he2010_forward
  that validates a different slice: where he2010_forward checks the
  process model + cohort/birth pipeline, this case checks the pfilter
  algorithm, observation-likelihood evaluation, and resampling.
  Uses `batch-replicated` mode (one camdl invocation with
  `--replicates N`). Agreement: camdl −5828.7 vs pomp −5827.4 (N=10
  vs N=20, 2000 particles both sides), difference 1.4 nats, well
  inside the 35-nat tolerance.

Coming later: IF2 MLE cross-check, spatial SIR, NumPyro / Stan
references for Bayesian-posterior cases.
