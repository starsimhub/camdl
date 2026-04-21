# Runtime-simulation regression tests

Short guide to the "forward-simulate many replicates, compare summary
statistics to an analytical or external reference" pattern. This is
distinct from the other test shapes we use (compile-level shape
assertions, golden IR round-trips, small-fixture unit tests) and is
the right tool when:

- The compiled IR looks correct but the simulated dynamics might not
  be (the Erlang-k staging case was the motivating example).
- A claim in the spec is about a *distribution* rather than a single
  scalar output.
- Multiple backends (Gillespie vs chain-binomial vs tau-leap) should
  produce the same distribution and we want a cross-check.

## Pattern

```rust
#[test]
#[ignore = "statistical test: run with --ignored in nightly CI"]
fn my_dynamical_claim() {
    // 1. Load or construct a model isolating the phenomenon.
    let model = load_golden("my_fixture");
    let compiled = CompiledModel::new(model).unwrap();
    let params = compiled.default_params.clone();

    // 2. Forward-simulate many seeds.
    let mut samples = Vec::new();
    for seed in 0..n_seeds {
        let traj = GillespieSim.run(&compiled, &params, seed, &config).unwrap();
        samples.push(extract_summary(&traj));
    }

    // 3. Compare to a reference generated externally (scipy, analytic,
    //    another backend) with a tolerance scaled by the MC noise.
    let actual = mean(&samples);
    let tol   = 3.0 * monte_carlo_se(n_seeds, /* variance per sample */);
    assert!((actual - expected).abs() < tol, "diagnostic message …");
}
```

Two living examples in the tree:

- **`rust/crates/sim/tests/statistical_distribution.rs`** — pure-death
  binomial, two-state equilibrium. Older; uses `#[ignore]` to run
  opt-in.
- **`rust/crates/sim/tests/erlang_distribution.rs`** — Erlang-k latent
  period via `consecutive()` (audit gap P1.3). Two tests: one
  numerical match against scipy's `gamma.cdf`, one "distinguishably
  different from exponential" sanity check.

## Design choices

### Isolate the phenomenon

Statistical tests have statistical noise, so the signal-to-noise ratio
is what determines whether the test is useful. Strip out every
dynamic that *isn't* the thing under test. In the Erlang case,
`setup_pure_erlang_decay` zeroes out `β` and `γ` and puts all 10000
individuals in `E_e1` — so the only dynamics that can fire are the
`E_e1 → E_e2 → E_e3 → I` chain. The measured E-total decay is then
pure Erlang survival, unpolluted by new infections or recoveries.

### Use Gillespie for ground truth

Exact CTMC → no dt-related bias. Reserve chain-binomial / tau-leap
tests for when you're specifically comparing approximations against
the exact backend, not against an external analytical reference.

### Tolerance from first principles, not tuning

```rust
let tol = 3.0 * (n0 as f64 * p * (1.0 - p) / n_seeds as f64).sqrt() + 5.0;
```

3σ band of the binomial-indicator mean per seed. The `+ 5.0` absorbs
quantisation (integer counts) and rounding. When this fails at 3σ
you have ≈ 0.3% chance of a false positive — acceptable for a
CI test, and the failure message tells you exactly how the actual
and expected diverge.

Avoid "pick a tolerance that makes the test pass today." That's a
regression trap: any future slow drift within the tolerance accrues
silently.

### Two-tier check: quantitative + distinguishable

The Erlang test has two tests:

1. `erlang_3_latent_matches_analytical_survival` — quantitative, six
   reference points from scipy.
2. `erlang_3_distinguishably_tighter_than_exponential` — coarse
   "degenerate-to-exponential would fail this."

The second is the safety net. If someone refactors `consecutive()` and
the k-sub-stage machinery silently collapses to a single transition
(same mean, wrong variance), the quantitative test might pass
fortuitously at one time point but the distinguishable test — which
checks the actual tail shape — fails.

### `#[ignore]` by default, run in nightly CI

Each test takes ≈ 1-3 s × n_seeds seeds. A full statistical suite can
add minutes to `cargo test`. Mark `#[ignore]`, run opt-in:

```bash
cargo test --release -p sim --test erlang_distribution -- --ignored
```

Wire a nightly CI job to run `cargo test --release -- --ignored` so
regressions still get caught daily without blowing the CI budget for
every PR.

## What other claims warrant this treatment

From the 2026-04-21 spec-claims-vs-tests audit:

- **Overdispersion variance.** `overdispersed(rate, σ²)` should produce
  a transition-count distribution with `var = rate · Δt · (1 + σ²·rate·Δt)`.
  Same shape as the Erlang test: run chain-binomial, measure empirical
  variance over seeds, compare to analytic formula.
- **Chain-binomial vs Gillespie agreement at small `dt`.** For a simple
  model, both backends should converge on the same mean trajectory as
  `dt → 0`. Pick a coarse and fine dt, assert convergence.
- **ODE steady state.** For a birth-death model, ODE trajectory should
  converge to `β/μ · N`. A two-line test.
- **Conservation.** Sum of compartments equals `N` throughout (no
  demography models). Exact assertion per trajectory, not statistical.
  Good for every closed model in the golden set.
- **Intervention timing.** `transfer(fraction=0.5, from=S, to=V) at [t=50]`
  must produce a discontinuous jump in S(t) at exactly t=50, not
  t=50±dt. Test for each backend.

Each of these is 30-80 lines and reuses the existing fixture loader.
A half-day's work would double the statistical-regression coverage.

## Anti-pattern: tests that assert nothing about dynamics

Golden-IR round-trip tests (e.g., `smoke_all_golden.rs`) check that
simulation doesn't panic on every golden file. Useful for catching
Rust-side type errors in the compiled-model pipeline. **Not a
substitute for statistical tests**: a broken `consecutive()`
expansion would still produce a running (wrong) simulation, and the
smoke test passes. The 2026-04-21 audit calls out exactly this
class of gap.
