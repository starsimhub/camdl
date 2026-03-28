# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Project Overview

`compartmental` is a monorepo for stochastic compartmental epidemic modelling.
It has two independent subsystems connected by a shared JSON IR (Intermediate
Representation):

- **OCaml frontend** (`ocaml/`): DSL → stratification expansion → IR
  serialization
- **Rust backend** (`rust/`): IR deserialization → simulation →
  trajectory/observation output

The IR schema (`ir/schema.json`) is the contract between them. Changes to the
schema must be reflected in both language implementations atomically.

## Build Commands

```bash
make build           # build both OCaml and Rust
make build-ocaml     # cd ocaml && dune build
make build-rust      # cd rust && cargo build --release
```

## Test Commands

```bash
make test            # all levels: unit + golden + integration
make test-unit       # fast, per-language unit tests only
make test-golden     # golden IR deserialization + simulation determinism

# OCaml only
cd ocaml && dune runtest

# Rust only
cd rust && cargo test

# Single Rust test file
cd rust && cargo test --test golden_simulate
cd rust && cargo test --test expr_eval

# Integration (cross-language, slow — CI only)
bash tests/test_ocaml_to_rust.sh
```

## Golden File Management

Golden files in `ir/golden/` are the integration test surface — committed IR
JSON that both sides must parse and agree on.

```bash
make update-golden    # recompile DSL fixtures → ir/golden/*.ir.json
make update-expected  # re-simulate golden models → ir/expected/*.tsv
```

When adding a new model: write DSL in `tests/fixtures/`, run `update-golden`,
review the JSON, run `update-expected`, review the TSV, commit all three
together.

## Quick Simulation

```bash
make sim MODEL=ir/golden/sir_basic.ir.json
# or directly:
rust/target/release/compartmental simulate <model.ir.json> --traj /tmp/traj.tsv --obs /tmp/obs.tsv
```

## Architecture

### The IR as contract

The IR is a **fully-expanded** declarative model — no stratification shorthand
survives serialization. The OCaml compiler performs stratification expansion;
what reaches Rust is a flat list of compartments, transitions (with
stoichiometry + rate expression), observation models, parameters, and initial
conditions.

The expression language (`expr`) is a pure, total, first-order AST over
`Const | Param | Pop | PopSum | Time | BinOp | UnOp | Cond | TimeFunc | TableLookup`.
No recursion, no binding — propensities evaluate in bounded time. `Cond` guards
against division-by-zero in Gillespie. `TableLookup` keeps stratified models
compact (contact matrices, age-specific rates).

### Rust crate dependency order

```
cli → io → observe → sim → ir
```

- `ir`: pure types + serde, no simulation logic
- `sim`: simulation backends (Gillespie, tau-leap, ODE, chain-binomial) +
  propensity evaluator; defines the `Model` trait
- `observe`: projection + likelihood sampling/scoring; depends on `sim` for
  `Trajectory`
- `io`: TSV read/write glue
- `cli`: arg parsing + orchestration

### OCaml library order

```
expand → dsl → ir
```

- `ir`: OCaml types mirroring the schema + Yojson serialization/deserialization
- `dsl`: embedded DSL builder combinators; produces pre-expansion IR
- `expand`: base model × stratification spec → flat expanded IR (the core
  compiler logic)

### RNG and CRN coupling

Scenario coupling uses Common Random Numbers (CRN): same seed → same sequential
RNG stream → identical trajectories as long as states and propensities match.
Pre-intervention trajectories are byte-identical for `enable`/`disable`
scenarios. For `set`/`scale` scenarios that modify propensities from t=0,
trajectories are correlated but not identical.

`rust/crates/sim/src/ekrng.rs` implements an event-keyed RNG (`EkRng`) for
potential future use (ABM support, conditional SMC). It is not wired into any
simulation backend — `StatefulRng` (seeded ChaCha8) is used exclusively. The
`event_key` field in the IR is populated by the compiler but ignored at runtime.

### Implementation phases

| Phase | Status                        | Scope                                                      |
| ----- | ----------------------------- | ---------------------------------------------------------- |
| v0.1  | Current target                | Forward simulation + synthetic data generation             |
| v0.2  | Designed, not yet implemented | Inference (PMCMC/IF2), real data input, priors             |
| v0.3  | Design sketch only            | Hierarchical priors, reporting pipelines, spatial coupling |

v0.2/v0.3 fields are present in the schema as nullable so the serialization
format never breaks between phases.

### Backend capabilities

Model features constrain which backends can run them. The `Capabilities`
bitflags in `rust/crates/sim/src/lib.rs` enforce this at dispatch time:

- `OVERDISPERSION`: transitions using `overdispersed(rate, σ²)` require
  tau-leap or chain-binomial (NegBinomial draws). Gillespie and ODE reject
  these models with a hard error.
- `REAL_COMPARTMENTS`: real-valued compartments with ODE equations.

The `CompiledModel::required_capabilities()` scans the IR; each backend's
`Simulate::capabilities()` declares what it supports. Mismatch → error before
simulation starts.

### Scheduled interventions and simulation backends

Interventions are deterministic state modifications (not stochastic events).
Each backend handles them differently and the interaction is non-trivial — see
§2.3.1 of `compartmental-ir-spec.md` for the
Gillespie/tau-leap/ODE/discrete-time specifics. The key constraint: after a
Gillespie intervention, propensities must be fully recomputed from the modified
state; do not resume with remaining exponential time.

### Changing the IR schema

1. Update `ir/schema.json` + bump `ir/VERSION`
2. Update OCaml types in `ocaml/lib/ir/` (ir.ml, serialize.ml, deserialize.ml)
3. Update Rust types in `rust/crates/ir/src/`
4. `make test-unit` — fix type errors
5. `make update-golden && make update-expected` — regenerate all golden files
6. Commit schema + both language changes + updated golden files in one atomic
   commit

## Design Principles

### No loose semantics

Never silently accept invalid input. If a construct looks like it means
something, it must either mean exactly that or produce a clear error.
Examples: `_args` patterns that discard function arguments, optional
fields that default to "works but wrong." If the compiler accepts it,
the behavior must be fully specified and intentional.

### Backwards compatibility is a non-goal

This is unreleased software. Do not add backwards-compatibility shims,
`alias` attributes, fallback deserialization paths, or deprecated field
names. When a field is renamed, rename it everywhere atomically. When a
format changes, update all golden files. Clean design beats legacy support.
