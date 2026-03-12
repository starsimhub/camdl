# Implementation Notes

## IR Layer — Decisions and Deviations

### Expr JSON encoding

The task description specifies a tagged-union encoding for `Expr` where the
discriminant is the sole key of a JSON object.  This diverges from the example
in `compartmental-ir-spec.md §11`, which uses a different n-ary `"op"`/`"args"`
schema.  **Decision**: follow the task description's encoding exactly (it is
more explicit and round-trips cleanly with serde's untagged enum).

### `table_lookup.indices` — 1D vs multi-D

The spec (§3.4) says tables are 1D and that multi-dimensional lookups are
flattened into a single index expression by the OCaml expander.  The task
description shows `"indices": [<expr>, <expr>]` (plural), suggesting multi-index
syntax.  **Decision**: implement `indices` as a list of expressions.  Validation
only warns (not errors) on multi-element lists in v0.1; the sim backend will
interpret a single flat index expression.  All golden files use singleton lists.

### Rust workspace root as a package

The project structure places integration tests at `rust/tests/`, which is
outside any crate.  To make `cargo test --test golden_deser` work, `rust/Cargo.toml`
declares both `[package]` (name `camdl-tests`) and `[workspace]`.  This is a
standard Cargo pattern for root-package workspaces.  A minimal `rust/src/lib.rs`
is included to satisfy Cargo's package requirements.

### OCaml library: `(wrapped false)`

The `ir` OCaml library uses `(wrapped false)` in its dune stanza.  With
`(wrapped true)` (the default), dune creates a single wrapper module `Ir` and
expects sub-modules (`Serialize`, `Deserialize`, `Validate`) to _not_ `open Ir`
since they are _part of_ `Ir`.  Removing wrapping makes all modules first-class
and lets them `open Ir` freely, which matches the natural module structure.

### OCaml `[@@@warning "-30"]`

Multiple record types share field names (`name`, `kind`, `op`, `compartment`).
OCaml warns (W30) about duplicate labels.  Adding `[@@@warning "-30"]` in
`ir.ml`/`ir.mli` suppresses this.  All field accesses in `validate.ml` and
`deserialize.ml` are annotated with explicit types to avoid resolution ambiguity.

### OCaml test golden-dir discovery

`dune runtest` may run the test binary from various working directories
depending on dune version and sandbox settings.  The test uses `find_up` to
walk up the directory tree from `Sys.getcwd()` until it locates `ir/golden/`,
making it robust to any invocation location.

### `Projected` Expr node

The spec mentions that likelihood expressions can reference the projection
output.  We add `Projected` as an `Expr` variant (serialised as
`{"projected": null}`) for use inside `likelihood` fields.  The validator does
not flag `Projected` in likelihood contexts.

### `data_contract` field

In v0.1, `data_contract` is always `null`.  Rather than defining a full
`DataContract` type now, the Rust type uses `Option<serde_json::Value>` and the
OCaml type uses `Yojson.Safe.t option`, deferring the schema to v0.2.

### Golden files provided

Eight golden files covering the required models:

| File | Description |
|------|-------------|
| `sir_basic` | Minimal SIR, frequency-dependent FOI |
| `sir_demography` | SIR + birth/death (open population) |
| `sir_vaccination` | SIR + SIA intervention (FractionTransfer) |
| `pure_death` | Single-compartment extinction (analytic) |
| `birth_death` | Linear birth-death (Poisson steady state) |
| `two_state` | Reversible A↔B (analytic equilibrium) |
| `cholera_siwr` | SIWR with real-valued water reservoir W (PDMP) |
| `seir_age` | Age-stratified SEIR, 2×2 contact matrix, TableLookup |

### Not implemented

- `ir/golden/` models listed in `project-structure.md` beyond the 8 required:
  `sir_closed`, `sir_tiny`, `sir_large`, `sir_competing_hazards`,
  `sir_absorbing`, `sir_placebo_ekrng`, `sir_scenario_pair`, `seir_seasonal`,
  `sir_discrete` — these are in the full spec but were not requested.
- Simulation backends (`sim`, `observe`, `io`, `cli`) — stub crates only.
- OCaml DSL (`lib/dsl/`) and expander (`lib/expand/`) — out of scope.
