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
# Build the harness binary
cargo build --release --manifest-path rust/Cargo.toml -p external-harness

# Run a single case (fast path: cached fixtures only, no external tooling)
./rust/target/debug/external-harness run tests/external/cases/sir_analytical
```

Expected output on success:

```
PASS sir_analytical (1 checks)
  ok  final_R [mean]
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

Coming next (per the proposal):
- he2010_pfilter_loglik — pfilter log-lik at the MLE
