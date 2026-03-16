# Project Structure: `camdl`

Monorepo. The IR schema is the shared contract — versioning OCaml and Rust
together prevents drift.

## Directory Layout

```
camdl/
│
├── README.md
├── Makefile                          # Top-level orchestration
├── CHANGELOG.md
│
├── ir/                               # ── The Contract ──
│   ├── schema.json                   # JSON Schema (source of truth for IR format)
│   ├── VERSION                       # Schema version string ("0.3")
│   │
│   ├── golden/                       # Golden IR files: the integration test surface
│   │   ├── sir_basic.ir.json         # Simplest possible model (3 compartments, 2 transitions)
│   │   ├── sir_closed.ir.json        # SIR closed — population conservation test
│   │   ├── sir_demography.ir.json    # SIR + birth/death (open model, mass balance)
│   │   ├── sir_tiny.ir.json          # N=10, extinction dynamics
│   │   ├── sir_large.ir.json         # N=10^8, overflow/perf test (ODE backend)
│   │   ├── sir_vaccination.ir.json   # SIR + scheduled intervention (SIA)
│   │   ├── sir_competing_hazards.ir.json  # Multiple outflows from same compartment
│   │   ├── sir_absorbing.ir.json     # Starts in absorbing state (I=0, S=0)
│   │   ├── sir_placebo_ekrng.ir.json # EKRNG placebo: mechanistically inert extra transition
│   │   ├── sir_scenario_pair.ir.json # Baseline + intervention pair for EKRNG coupling test
│   │   ├── seir_age.ir.json          # Age-stratified SEIR (the spec example)
│   │   ├── seir_seasonal.ir.json     # Seasonal forcing via TimeFunc
│   │   ├── pure_death.ir.json        # Single-compartment pure death (analytic solution)
│   │   ├── birth_death.ir.json       # Birth-death process (Poisson steady state)
│   │   ├── two_state.ir.json         # Reversible A↔B (analytic equilibrium)
│   │   ├── sir_discrete.ir.json      # Discrete-time chain binomial variant
│   │   └── cholera_siwr.ir.json      # SIWR with real-valued environmental reservoir
│   │
│   └── expected/                     # Expected simulation outputs for golden models
│       ├── sir_basic.traj.tsv        # Trajectory output (deterministic seed)
│       ├── sir_basic.obs.tsv         # Synthetic observations
│       └── ...                       # One pair per golden model
│
├── ocaml/                            # ── Frontend: DSL → IR ──
│   ├── dune-project
│   ├── camdl.opam
│   │
│   ├── lib/
│   │   ├── ir/                       # OCaml types mirroring the IR schema
│   │   │   ├── ir.ml                 # Core types: expr, transition, model, etc.
│   │   │   ├── ir.mli
│   │   │   ├── serialize.ml          # model → JSON (Yojson)
│   │   │   ├── deserialize.ml        # JSON → model (for round-trip testing)
│   │   │   └── validate.ml           # IR-level validation (well-formedness checks)
│   │   │
│   │   ├── dsl/                      # DSL surface + parser
│   │   │   ├── dsl.ml                # Embedded OCaml DSL (builder combinators)
│   │   │   ├── dsl.mli
│   │   │   ├── parser.ml             # Text DSL parser (if/when we want a file format)
│   │   │   └── parser.mli
│   │   │
│   │   └── expand/                   # Stratification expander
│   │       ├── expand.ml             # Base model × stratification → expanded IR
│   │       ├── expand.mli
│   │       ├── transition_kinds.ml   # Pre-expansion transition semantics
│   │       └── mixing.ml             # Contact matrix / FOI expansion logic
│   │
│   ├── bin/
│   │   └── compile.ml                # CLI: reads .dsl or .ml, writes .ir.json
│   │
│   └── test/
│       ├── test_ir_roundtrip.ml      # Serialize → deserialize → assert equal
│       ├── test_expand.ml            # Unit tests for stratification expansion
│       ├── test_golden.ml            # OCaml serializes known models → diff against golden/
│       └── test_expr.ml              # Expression construction + simplification
│
├── rust/                             # ── Backend: IR → simulate → output ──
│   ├── Cargo.toml                    # Workspace root
│   │
│   ├── crates/
│   │   ├── ir/                       # IR types + serde deserialization
│   │   │   ├── Cargo.toml
│   │   │   └── src/
│   │   │       ├── lib.rs
│   │   │       ├── model.rs          # Top-level Model struct
│   │   │       ├── expr.rs           # Expression AST + evaluator
│   │   │       ├── transition.rs     # Transition, stoichiometry
│   │   │       ├── ode_equation.rs   # ODE equations for real compartments
│   │   │       ├── observation.rs    # Observation model, likelihood, projection
│   │   │       ├── parameter.rs      # Parameter declarations
│   │   │       ├── intervention.rs   # Scheduled interventions
│   │   │       ├── time_func.rs      # Time-varying functions
│   │   │       ├── table.rs          # Table lookup
│   │   │       └── validate.rs       # Post-deserialization validation
│   │   │
│   │   ├── sim/                      # Simulation backends
│   │   │   ├── Cargo.toml            # depends on `ir`
│   │   │   └── src/
│   │   │       ├── lib.rs
│   │   │       ├── trait.rs          # The Model trait: simulate, sample_observations
│   │   │       ├── state.rs          # StateVec (integer), RealStateVec, FlowVec, Trajectory
│   │   │       ├── propensity.rs     # Compiled propensity evaluator (from IR expr)
│   │   │       ├── ode_integrator.rs # RK4 integrator for real compartments
│   │   │       ├── gillespie.rs      # Exact SSA (+ PDMP mode for real compartments)
│   │   │       ├── tau_leap.rs       # Tau-leaping (+ ODE step for real compartments)
│   │   │       ├── ode.rs            # Deterministic ODE backend [v0.2]
│   │   │       ├── chain_binomial.rs # Discrete-time chain binomial
│   │   │       ├── intervention.rs   # Intervention scheduler + applicator
│   │   │       └── ekrng.rs          # Event-keyed RNG (Philox/Threefry wrapper)
│   │   │
│   │   ├── observe/                  # Observation model: sample + score
│   │   │   ├── Cargo.toml            # depends on `ir`, `sim`
│   │   │   └── src/
│   │   │       ├── lib.rs
│   │   │       ├── projection.rs     # CumulativeFlow, CurrentPop, etc.
│   │   │       ├── likelihood.rs     # NegBinomial, Poisson, etc. (sample + logpdf)
│   │   │       └── sample.rs         # sample_observations implementation
│   │   │
│   │   ├── io/                       # Data I/O
│   │   │   ├── Cargo.toml            # depends on `ir`, `sim`, `observe`
│   │   │   └── src/
│   │   │       ├── lib.rs
│   │   │       ├── write_tsv.rs      # Trajectory + observations → TSV
│   │   │       └── read_tsv.rs       # Data input for inference [v0.2]
│   │   │
│   │   └── cli/                      # Command-line tool
│   │       ├── Cargo.toml            # depends on all above
│   │       └── src/
│   │           └── main.rs           # `camdl simulate model.ir.json -o output/`
│   │
│   └── tests/                        # Rust-side integration + statistical tests
│       ├── golden_deser.rs           # Deserialize all golden/*.ir.json, assert no errors
│       ├── golden_simulate.rs        # Simulate golden models, diff against expected/*.tsv
│       ├── expr_eval.rs              # Property tests for expression evaluator
│       ├── gillespie_invariants.rs   # Non-negativity, population conservation, mass balance
│       ├── ekrng_determinism.rs      # Determinism, order-independence, placebo, coupling
│       ├── intervention.rs           # FractionTransfer correctness, timing, ordering
│       ├── observation_sampling.rs   # Distribution correctness for all likelihood families
│       ├── pdmp_hybrid.rs            # PDMP coupling tests for real compartments (§A.2.7)
│       ├── statistical_distribution.rs  # Pure-death, birth-death, two-state, SIR final size
│       └── cross_backend.rs          # Gillespie vs ODE, vs tau-leap, vs chain-binomial
│
├── tests/                            # ── Integration tests (cross-language) ──
│   ├── run_integration.sh            # Master script
│   ├── test_ocaml_to_rust.sh         # OCaml compiles DSL → IR → Rust simulates → check
│   ├── test_golden_agreement.sh      # Both sides agree on golden IR files
│   ├── test_round_trip.sh            # OCaml serialize → Rust deser → Rust serialize → diff
│   ├── compare_tsv.py                # Approximate float comparison for trajectory diffs
│   │
│   ├── fixtures/                     # DSL source files for integration tests
│   │   ├── sir_basic.dsl
│   │   ├── seir_age.dsl
│   │   └── ...
│   │
│   └── snapshots/                    # Expected outputs for integration tests
│       └── ...                       # Generated by `make update-snapshots`
│
├── docs/                             # ── Documentation ──
│   ├── ir-spec.md                    # The IR specification
│   ├── dsl-guide.md                  # User-facing DSL documentation
│   ├── architecture.md               # System architecture overview
│   └── inference-interface.md        # Contract for external inference code [v0.2]
│
└── examples/                         # ── Worked examples ──
    ├── sir_basic/
    │   ├── model.dsl                 # DSL source
    │   ├── model.ir.json             # Compiled IR (committed for reference)
    │   ├── run.sh                    # compile + simulate
    │   └── plot.py                   # Quick matplotlib visualization
    ├── seir_nigeria_simple/
    │   ├── model.dsl
    │   ├── contact_matrix.tsv        # External data (table input)
    │   └── run.sh
    └── scenario_comparison/
        ├── baseline.dsl
        ├── sia_campaign.dsl
        └── compare.sh               # Paired simulation + diff
```

## The Golden File Strategy

The golden files in `ir/golden/` are the **integration test contract**. They
serve multiple roles:

**1. Schema conformance.** Both OCaml and Rust must parse these files without
error. If either side changes how it reads/writes the IR, the golden tests catch
it immediately.

**2. Cross-language round-trip.** The test `test_round_trip.sh` does:

```
OCaml: deserialize golden.ir.json → OCaml model → serialize → golden_ocaml.ir.json
Rust:  deserialize golden.ir.json → Rust model  → serialize → golden_rust.ir.json
diff golden_ocaml.ir.json golden_rust.ir.json  # must be semantically equal
```

Semantic equality, not byte equality — JSON key ordering and whitespace may
differ. Use `jq --sort-keys` or a JSON-aware diff.

**3. Simulation determinism.** For each golden model with a fixed `rng_seed`,
the expected output in `ir/expected/` is committed. The Rust golden simulation
test runs each model and diffs against expected output. This catches propensity
evaluation bugs, EKRNG implementation errors, Gillespie algorithm bugs, and
observation sampling bugs.

**4. Regression.** When you change the expression evaluator, add a new backend,
or refactor the simulation loop, the golden tests are your safety net.

### Creating golden files

Start with hand-written IR JSON for the simplest models. As the OCaml DSL
matures, generate golden files from DSL source:

```makefile
golden-update:
    cd ocaml && dune exec bin/compile.exe -- \
        ../tests/fixtures/sir_basic.dsl -o ../ir/golden/sir_basic.ir.json
    cd rust && cargo run --bin camdl -- simulate \
        ../ir/golden/sir_basic.ir.json --seed 42 \
        --traj ../ir/expected/sir_basic.traj.tsv \
        --obs ../ir/expected/sir_basic.obs.tsv
```

## Rust Workspace Structure

The Rust side is a Cargo workspace with four crates. Dependency graph:

```
cli ──→ io ──→ observe ──→ sim ──→ ir
                                    ↑
                              (serde, serde_json)
```

- **`ir`**: Pure data types + serde. Zero simulation logic. Can be used by
  external tools that just need to read/write IR files.
- **`sim`**: Simulation backends. Depends on `ir`. Defines the `Model` trait.
  This is where the hot loop lives — perf-critical, allocation-conscious.
  Contains `ode_integrator.rs` for advancing real compartments in PDMP mode.
- **`observe`**: Observation model logic. Depends on `sim` for `Trajectory`.
  Sampling and scoring use the same distribution code, called differently.
- **`io`**: TSV reading/writing. Thin glue layer.
- **`cli`**: Binary entry point (`camdl`). Arg parsing, config, orchestration.

```toml
# rust/crates/ir/Cargo.toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# rust/crates/sim/Cargo.toml
[dependencies]
ir = { path = "../ir" }
rand = "0.8"
rand_chacha = "0.3" # or threefry crate for EKRNG

# rust/crates/observe/Cargo.toml
[dependencies]
ir = { path = "../ir" }
sim = { path = "../sim" }
rand = "0.8"
rand_distr = "0.4" # NegBinomial, Poisson, etc.

# rust/crates/io/Cargo.toml
[dependencies]
ir = { path = "../ir" }
sim = { path = "../sim" }
observe = { path = "../observe" }
csv = "1"

# rust/crates/cli/Cargo.toml
[dependencies]
ir = { path = "../ir" }
sim = { path = "../sim" }
observe = { path = "../observe" }
io = { path = "../io" }
clap = { version = "4", features = ["derive"] }
```

## OCaml Project Structure

```
# ocaml/dune-project
(lang dune 3.0)
(name camdl)

# ocaml/lib/ir/dune
(library (name ir) (libraries yojson))

# ocaml/lib/dsl/dune
(library (name dsl) (libraries ir))

# ocaml/lib/expand/dune
(library (name expand) (libraries ir dsl))

# ocaml/bin/dune
(executable (name compile) (libraries ir dsl expand))

# ocaml/test/dune
(tests (names test_ir_roundtrip test_expand test_golden test_expr)
       (libraries ir dsl expand alcotest))
```

- **`ir`**: OCaml types for the IR + Yojson serialization/deserialization.
  Mirror of `rust/crates/ir`.
- **`dsl`**: Embedded DSL (builder combinators, `E.( + )`, `M.param`, etc.).
  Produces pre-expansion IR types.
- **`expand`**: Stratification expander. Takes a model with transition kinds +
  stratification specs → expanded flat IR.

## Testing Strategy

The full testing specification is in the IR spec appendix (§A). Summary of
levels:

### Level 1: Unit tests (per-language, `cargo test` / `dune runtest`)

**OCaml:** `test_expr.ml`, `test_expand.ml`, `test_ir_roundtrip.ml`

**Rust:** `expr_eval.rs` (property-based), `gillespie_invariants.rs`
(conservation, non-negativity, mass balance), `ekrng_determinism.rs`
(determinism, order-independence), `intervention.rs`, `observation_sampling.rs`,
`pdmp_hybrid.rs` (real compartment coupling)

### Level 2: Golden tests (`make test-golden`)

- `golden_deser.rs`: all golden files deserialize without error
- `golden_simulate.rs`: simulate golden models, diff against `ir/expected/`
- `test_golden.ml`: OCaml compiles fixtures, diffs against golden JSON

### Level 3: Integration tests (`make test-integration`, CI on every push)

Cross-language: OCaml compiles DSL → IR → Rust simulates → check output.
Round-trip: OCaml and Rust serializations must be semantically equal.

### Level 4: Statistical and cross-backend tests (nightly CI, `--ignored`)

`statistical_distribution.rs`: pure-death KS test, birth-death steady state,
two-state equilibrium, SIR final size. `cross_backend.rs`: Gillespie vs ODE
(large N), Gillespie vs tau-leap (small τ), continuous vs discrete-time.

## Makefile

```makefile
.PHONY: all build test test-unit test-golden test-integration clean

all: build test

build: build-ocaml build-rust

build-ocaml:
	cd ocaml && dune build

build-rust:
	cd rust && cargo build --release

test: test-unit test-golden test-integration

test-unit: test-unit-ocaml test-unit-rust

test-unit-ocaml:
	cd ocaml && dune runtest

test-unit-rust:
	cd rust && cargo test

test-golden: build
	cd rust && cargo test --test golden_deser --test golden_simulate

test-integration: build
	bash tests/test_ocaml_to_rust.sh
	bash tests/test_golden_agreement.sh
	bash tests/test_round_trip.sh

update-golden: build
	@echo "Regenerating golden IR files from DSL fixtures..."
	@for f in tests/fixtures/*.dsl; do \
		name=$$(basename $$f .dsl); \
		ocaml/_build/default/bin/compile.exe $$f -o ir/golden/$$name.ir.json; \
	done

update-expected: build
	@echo "Regenerating expected outputs from golden IR files..."
	@for f in ir/golden/*.ir.json; do \
		name=$$(basename $$f .ir.json); \
		rust/target/release/camdl simulate $$f \
			--traj ir/expected/$$name.traj.tsv \
			--obs ir/expected/$$name.obs.tsv; \
	done

sim: build-rust
	rust/target/release/camdl simulate $(MODEL) \
		--traj /tmp/traj.tsv --obs /tmp/obs.tsv
	@echo "Output: /tmp/traj.tsv, /tmp/obs.tsv"

clean:
	cd ocaml && dune clean
	cd rust && cargo clean
```

## CI Pipeline

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Setup OCaml
        uses: ocaml/setup-ocaml@v2
        with:
          ocaml-compiler: "5.1"

      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install OCaml deps
        run: |
          opam install . --deps-only --with-test -y
          eval $(opam env)

      - name: Build
        run: make build

      - name: Unit tests
        run: make test-unit

      - name: Golden tests
        run: make test-golden

      - name: Integration tests
        run: make test-integration

  property-tests:
    runs-on: ubuntu-latest
    if: github.event_name == 'schedule'
    steps:
      - uses: actions/checkout@v4
      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable
      - name: Statistical + cross-backend tests
        run: cd rust && cargo test --test statistical_distribution --test cross_backend -- --ignored
```

## Development Workflow

### Adding a new model

1. Write DSL in `tests/fixtures/new_model.dsl`
2. `make update-golden` → generates `ir/golden/new_model.ir.json`
3. Review the golden JSON (is the expansion correct?)
4. `make update-expected` → generates expected simulation output
5. Review the expected TSV (does the trajectory make sense?)
6. Commit all of: DSL source, golden IR, expected output
7. CI runs integration tests → green

### Changing the IR schema

1. Update `ir/schema.json` + bump `ir/VERSION`
2. Update `ocaml/lib/ir/ir.ml` (types + serialize/deserialize)
3. Update `rust/crates/ir/src/` (types + serde)
4. `make test-unit` → fix any type errors
5. `make update-golden` → regenerate golden files from DSL
6. `make update-expected` → regenerate expected outputs
7. `make test` → everything green
8. Commit schema + both language changes + updated golden files atomically

### Working on Rust simulation only

```bash
cd rust
cargo test
cargo test --test golden_simulate
cargo run --bin camdl -- simulate ../ir/golden/sir_basic.ir.json \
    --traj /tmp/traj.tsv --obs /tmp/obs.tsv
```

No OCaml needed — the golden IR files are committed and always available.

### Working on OCaml DSL only

```bash
cd ocaml
dune runtest
dune exec bin/compile.exe -- ../tests/fixtures/sir_basic.dsl -o /tmp/test.ir.json
```

No Rust needed — you're just verifying the compiler output looks right.
