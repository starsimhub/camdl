# Testing camdl

Orientation for anyone (human or agent) writing or running tests in
this repo. camdl is dual-language (OCaml compiler + Rust runtime)
with cross-language integration, so "what test should I run?" has
several correct answers depending on what you're changing.

## TL;DR commands

```bash
# Before pushing — mirrors CI; ~30-60 s.
make test

# Just the fast layers:
make test-ocaml       # OCaml compiler + dimcheck + IR round-trip
make test-rust        # Rust workspace: cargo test --workspace

# Integration (slow — shells out to the built binary):
make test-integration

# Statistical tests (slow; skipped by default):
cd rust && cargo test --release --workspace -- --ignored

# A single Rust test file:
cd rust && cargo test --release -p sim --test erlang_distribution

# A single Rust test, with println! output visible:
cd rust && cargo test --release -p sim --test foo test_name -- --nocapture

# A single OCaml suite (Alcotest):
cd ocaml && dune runtest test/test_compiler.exe --force
```

If you're only changing one language, run just that language's layer
during iteration; run `make test` before committing.

## Architecture

Tests are organised by **layer**, not by file type. Each layer answers
a different question about the system; don't substitute one for
another.

```
┌───────────────────────────────────────────────────────────────┐
│  Layer                     Where                    When      │
├───────────────────────────────────────────────────────────────┤
│  L1  Parser + type check   ocaml/test/test_compiler.ml        │
│                            ocaml/test/test_dimcheck.ml        │
│                            + ocaml/test/errors/*.camdl        │
│                                                               │
│  L2  IR round-trip         ocaml/test/test_ir_roundtrip.ml    │
│                                                               │
│  L3  Rust unit tests       rust/crates/*/src/**  #[test] mods │
│                                                               │
│  L4  Rust integration      rust/crates/*/tests/*.rs           │
│      (fast)                                                   │
│                                                               │
│  L5  Rust integration      rust/crates/cli/tests/*.rs         │
│      (CLI shell-out)         — shell out to built binary      │
│                                                               │
│  L6  Statistical /         rust/crates/sim/tests/             │
│      distribution            erlang_distribution.rs,          │
│                              statistical_distribution.rs      │
│                              — #[ignore] by default           │
│                                                               │
│  L7  Cross-language        tests/test_ocaml_to_rust.sh        │
│      integration             — compile .camdl → simulate      │
│                                                               │
│  L8  Book build (prose)    docs/book/  (mdbook build)         │
└───────────────────────────────────────────────────────────────┘
```

### L1 — OCaml compiler + dimcheck

Runs via `dune runtest` in `ocaml/`. Fast (< 1 s). Three suites:

- **test_compiler.ml** (~120 tests): parsing, stratification expansion,
  scenario resolution, observations, interventions, spec-claim
  regression tests (`spec_claims_v1`, `table_unit_conversion`).
- **test_dimcheck.ml** (~73 tests): dimensional analysis checker.
  Uses qcheck for property-based tests alongside the fixture tests.
- **errors/*.camdl + test_compiler's negative_golden suite**: one
  minimum-reproducer `.camdl` per error code. The test compiles each
  with `Diagnostics.json_errors_mode` on and asserts the emitted error
  code appears in the payload. Pattern described in the 2026-04-21
  spec-claims audit as the right way to grow error-code coverage.

**Running a subset:**

```bash
cd ocaml
dune runtest test/test_compiler.exe --force
dune exec test/test_compiler.exe -- test 'table_unit_conversion'
```

### L2 — IR round-trip

`test_ir_roundtrip.ml`: every `.camdl` in `ocaml/golden/` compiles to
IR, serialises, deserialises, and compares structurally. Catches
schema drift. Automatically exercises every golden fixture — if you
add a `.camdl` to `ocaml/golden/`, regenerate its `.ir.json` via
`make update-golden` and the round-trip test picks it up.

### L3 — Rust unit tests

`#[cfg(test)] mod tests { … }` inside each `src/` file. Fast;
compilation-coupled so they catch API mismatches at build time.
Currently concentrated in: `compiled_model.rs`, `hashing.rs`,
`inference/prequential.rs`, `inference/resampling.rs`, `rng.rs`.

### L4 — Rust integration (fast, in-process)

`rust/crates/sim/tests/*.rs`. Integration tests that import `sim` as a
library (not shell out). Fast — each file compiles once and runs
quickly. Highlights:

| File | What it tests |
|---|---|
| `cubic_spline.rs` | `CubicSpline` vs `scipy.interpolate.CubicSpline(bc_type='natural')` — 12 reference points |
| `interpolation.rs` | Linear + constant interp vs `np.interp` + `interp1d(kind="previous")` |
| `gillespie_determinism.rs` | Same seed → byte-identical trajectory (CRN) |
| `gillespie_invariants.rs` | Mass conservation, no-event dynamics, etc. |
| `chain_binomial_invariants.rs` | Same invariants for the chain-binomial backend |
| `ode.rs` | RK4 backend correctness |
| `particle_filter.rs` | Bootstrap filter log-likelihood consistency |
| `if2.rs` | IF2 convergence sanity |
| `pmmh.rs` / `pgas_resume.rs` / `pgas_tempering.rs` | PMMH / PGAS |
| `obs_level_params.rs` | Observation-model parameter plumbing |
| `interventions.rs` | Intervention timing + state effects |
| `periodic_forcing.rs` | Periodic bin lookup |
| `expr_eval.rs` | Pure expression evaluator |
| `smoke_all_golden.rs` | Every `.ir.json` in `ocaml/golden/` compiles + simulates under every backend — catches crate-level API drift but NOT dynamics bugs |

### L5 — Rust integration (CLI shell-out)

`rust/crates/cli/tests/*.rs`. Each test spawns the built
`target/release/camdl` binary against a `tempdir()` workspace. Slow
(each invocation pays the full binary startup) but tests the end-user
surface.

| File | What it tests |
|---|---|
| `backend_provenance.rs` | Simulate auto-matches fit's backend; warns on mismatch |
| `cas_integration.rs` | `camdl simulate --cas` + `camdl list/show/cat` |
| `intervention_event_defaults.rs` | Spec §14.4: events on, interventions off |
| `pfilter_trajectories.rs` | `pfilter --save-paths N` writes the right shape |
| `scenario_runtime_application.rs` | Spec §17.1: `set`/`scale` actually applied at runtime (closed audit gap P1.1/P1.2) |
| `synthetic_fit_grid.rs` | `fit run` replicate-grid end-to-end |

**Gotcha: the binary must be built first.** These tests `skip_if_missing_binary()` when `target/release/camdl` doesn't exist. They silently skip, not fail. Always run `cargo build --release -p cli` before a full integration pass, or use `make test-integration` which builds first.

**Gotcha: camdlc version check.** The binary checks that `camdlc` on
PATH matches its own git hash. When they diverge (you built `camdl`
but not `camdlc`), tests fail with *"camdlc version mismatch"*.
Options:
1. `make install` to resync both.
2. `CAMDL_SKIP_VERSION_CHECK=1 cargo test …` to bypass for this
   invocation.

**Gotcha: `camdl-sim` vs `camdl`.** The binary was renamed from
`camdl-sim` → `camdl` in the clap 4 migration (2026-04-20). Several
integration tests still reference `target/release/camdl-sim`. A
symlink keeps them working:

```bash
ln -sf camdl rust/target/release/camdl-sim
```

Created once after a fresh `cargo clean` or clone.

### L6 — Statistical / distribution tests

`rust/crates/sim/tests/statistical_distribution.rs`,
`rust/crates/sim/tests/erlang_distribution.rs`. Marked **`#[ignore]`**
because each test runs thousands of Gillespie seeds and takes ~3-30 s.

**Run them periodically, not every commit:**

```bash
cd rust && cargo test --release -p sim -- --ignored
```

**When to run:**
- Before a release.
- After touching `sim/src/gillespie.rs`, `chain_binomial.rs`,
  `propensity.rs`, or anything in `inference/`.
- After a compiler change to `expander.ml` that affects transition
  emission (e.g., the `consecutive()` staging or stoichiometry).
- Nightly in CI (not configured yet — see audit follow-ups).

**Pattern and tolerance design** → `docs/dev/runtime-simulation-tests.md`.
Key point: tolerance should be computed from Monte-Carlo SE with a 3σ
band, not tuned to pass today. A drift-within-tolerance regression
won't be visible otherwise.

### L7 — Cross-language integration

`tests/test_ocaml_to_rust.sh`. Compiles every `.camdl` fixture with
`camdlc`, feeds the IR to `camdl batch run`, checks exit status.
Invoked via `make test-integration`. Catches:

- OCaml emits IR that Rust can't deserialise (schema drift).
- Rust `batch run` rejects a shape the OCaml compiler happily emits.
- CLI surface renames (the `simulate batch` → `batch run` rename in
  2026-04-20 broke this script until we updated the invocation).

Fixtures live in `tests/fixtures/exp_*.toml`. Each is a batch sweep
config pointing at an `ocaml/golden/*.camdl`.

### L8 — Book build

`cd docs/book && mdbook build`. Catches broken symlinks and dangling
cross-refs in user-facing docs. Part of the pre-push hook.

## CI / pre-push

**Pre-push hook (`.githooks/pre-push`, installed via `core.hooksPath`).**
Mirrors CI — runs locally on every `git push`:

1. OCaml build + tests
2. Rust `cargo test --workspace --no-fail-fast`
3. `cargo clippy --all-targets -- -D warnings`
4. `make update-golden` + assert `ir/golden/` and `ocaml/golden/`
   unchanged (catches schema changes you forgot to regenerate goldens
   for)
5. `mdbook build` (skipped if mdbook isn't installed)
6. `make test-integration`

**Bypass only for documentation-only commits with `--no-verify`.**
Otherwise never — see the comment at the top of the hook about the
2026-04-17 commit that broke CI because `cargo check --tests`
compiled tests without running them.

**GitHub Actions (`.github/workflows/ci.yml`).** Runs on push to `main`
and on PRs:

- OCaml build + `dune runtest`
- Rust build + clippy + `cargo test --workspace`
- `make update-golden` + diff check
- `make test-integration`
- Build release artifacts (Linux / macOS / Windows)

Statistical `#[ignore]` tests are **not** in CI yet. Runs manually
before releases; nightly CI job planned.

## Writing tests

### Adding a spec-claim regression

The 2026-04-21 table-unit incident was a spec claim nothing tested.
Follow this discipline for any spec claim that the compiler /
runtime must uphold:

1. Write the test **before** the fix (TDD).
2. Confirm it fails against the unfixed code.
3. Fix.
4. Confirm the test now passes.
5. Commit both in the same change.

Example: `rust/crates/cli/tests/scenario_runtime_application.rs`,
`ocaml/test/test_compiler.ml::table_unit_conversion`.

The commit message should mention which spec section's claim the
test guards (§X.Y) so future drift has a breadcrumb.

### Adding an error-code fixture

For every `emit_error ctx ~code:"ENNN" …` in the compiler:

1. Create `ocaml/test/errors/ennn_<slug>.camdl` — a minimal model
   that triggers the error and nothing else.
2. Verify manually: `camdlc check ocaml/test/errors/ennn_<slug>.camdl`
   emits the expected code.
3. The `negative_golden` suite in `test_compiler.ml` picks up the
   fixture automatically — no glue code needed.

Coverage status (2026-04-21): 90 codes emitted, 26 tested. 64 have
no fixture — see `docs/dev/reviews/2026-04-21-spec-claims-vs-tests.md`
§P2 for the list.

### Adding a statistical test

See `docs/dev/runtime-simulation-tests.md` for the full pattern.
Skeleton:

```rust
#[test]
#[ignore = "statistical test: run with --ignored"]
fn my_distributional_claim() {
    let model = setup_isolated_fixture(...);
    let compiled = CompiledModel::new(model).unwrap();
    let mut samples = Vec::with_capacity(n_seeds);
    for seed in 0..n_seeds {
        let traj = GillespieSim.run(&compiled, &params, seed, &config).unwrap();
        samples.push(extract_summary(&traj));
    }
    let actual = mean(&samples);
    let tol = 3.0 * monte_carlo_se(n_seeds, /* sample variance */);
    assert!((actual - expected_from_reference).abs() < tol, "diagnostic …");
}
```

Always include a "distinguishable from the degenerate case" sanity
assertion alongside the quantitative match — a regression that
collapses to a different-but-similar-mean distribution might slip
through pointwise checks.

### Adding a golden fixture

1. Write the `.camdl` at `ocaml/golden/<name>.camdl`.
2. `make update-golden` — regenerates `<name>.ir.json`.
3. **Review the JSON diff before committing.** The golden is your
   ground truth; if the regeneration changed values you didn't
   expect, that's a compiler bug to investigate, not a "update and
   move on."
4. Optionally add `ocaml/golden/<name>.params.toml` for simulation
   tests that need parameter values.

The IR round-trip test (L2) and smoke-all-golden test (L4) will
automatically pick up the new fixture.

### Updating a golden intentionally

When a compiler change legitimately alters golden IR (e.g., the
2026-04-21 table-unit fix that changed `sir_five_age.ir.json`
values from `[5.0, 10.0, …]` to `[1826.2, 3652.4, …]`):

1. `make update-golden`
2. **Diff each regenerated file and check the values are the ones
   you intended.** The pre-push hook will now complain that the
   golden is dirty — that's working as designed.
3. Commit the compiler change and the golden update together.

## Gotchas

- **`#[ignore]`** on Rust tests means opt-in. Always run with
  `-- --ignored` before merging anything that touches the sim or
  inference code.
- **Deterministic backend for scenario / value tests.** Use
  `--backend ode` in shell-out tests whose assertion is on a scalar
  output. Stochastic backends (Gillespie, chain-binomial) introduce
  seed-dependent noise that can mask an off-by-one or a no-op.
- **`--release`.** Almost every test runs under release. Debug-build
  runs of the sim tests are ~10× slower; the statistical tests
  become unbearable. Exception: rapid iteration on a single test
  while you're debugging — `cargo test <name>` without `--release`
  is fine for a minute.
- **Parallel test execution.** `cargo test` uses threads by default.
  Tests that touch shared filesystem state (`tempdir()` is fine;
  `/tmp` is not) can race. If you see flakiness, pass
  `-- --test-threads=1`.
- **Golden-file drift.** The pre-push hook runs `make update-golden`
  and checks for dirty working tree. If a schema change requires
  updates, update + commit in the same branch.
- **camdlc version pin.** The `camdl` binary refuses to run against
  a mismatched `camdlc`. Fix: `make install` after a pull, or
  `CAMDL_SKIP_VERSION_CHECK=1` for throwaway runs.

## When tests disagree with each other

If L4 (Rust unit) passes but L5 (CLI shell-out) fails, the bug is in
the CLI glue — arg parsing, path resolution, or the util-layer
model mutation (params application, scenario filter). If L5 passes
but L7 fails, the bug is cross-language — OCaml emits something
Rust doesn't understand, or vice versa. If L1-L4 pass but L6 fails,
the compiler produces correct-looking IR whose runtime dynamics are
wrong (the 2026-04-21 table-unit bug was exactly this shape —
compiler test passed, no runtime check existed). Use the layer
disagreement to triangulate.
