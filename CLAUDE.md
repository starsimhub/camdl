# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Implementation standard

This software is used to inform major public health decisions. Errors
in inference, simulation, or data handling are not just bugs — they can
mislead policy. Every implementation must be:

- **Correct before clean**: verify logic against the mathematical
  derivation or spec before refactoring for style.
- **Tested at every step**: run `cargo test` before and after each
  change; do not batch multiple semantic changes into one commit without
  an intermediate green test run.
- **Reviewed against the proposal**: when implementing from a proposal
  in `docs/dev/proposals/`, follow it exactly unless a concrete reason
  to deviate is documented inline. Do not improvise design changes
  mid-implementation.
- **Conservatively scoped**: if a change touches inference math
  (`pgas.rs`, `pgas_grad.rs`, `obs_loglik.rs`, `obs_model.rs`,
  `if2.rs`, `particle_filter.rs`), treat it as high-risk regardless of
  how mechanical it looks. Read the full function before editing any
  part of it.

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

## Debugging a diverging simulation

When a simulation's dynamics don't match a reference implementation (pomp,
Stan, a paper's published trajectory), the first tool is the per-substep
tracer built into the chain-binomial backend:

```bash
CAMDL_TRACE_STEPS=1 camdl simulate model.camdl --params p.toml \
    --backend chain_binomial --dt 1 --seed 1 --obs-only /tmp/obs.tsv \
    2> /tmp/trace.tsv 1>/dev/null
```

The trace dumps one TSV row per substep to **stderr** with columns:
`t`, all compartment counts, all `flow_<name>` (counts per substep), all
`rate_<name>` (total per-source rates evaluated this substep), and
`total_pop`. Redirect stderr to a file — stdout carries the normal TSV
simulation output, so keep them separate.

Workflow: pick a few diagnostic times (t=1, after seasonal onset, at
peak, post-epidemic trough) and compare the rate/flow columns against
hand-computed values from the reference implementation's rate
expressions. A mismatch at t=1 localizes to init or rate construction; a
mismatch that grows over time localizes to dynamics (noise, forcing
interaction, event ordering).

Other logging channels worth knowing about:
- `log::debug!` in `pgas.rs`, `particle_filter.rs`, `if2.rs`: inference
  diagnostics (-inf logliks, skipped observations, density mismatches).
  Enable with `RUST_LOG=camdl_sim=debug` or similar.
- `CAMDL_TRACE_STEPS=1` also activates in `intervention.rs` — logs
  intervention firings alongside the substep trace.

Before inventing new logging, check the existing paths above. They
already cover most per-step/per-iteration diagnostics.

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

### RNG and paired-seed coupling

The runtime uses a plain ChaCha8 `StatefulRng`. Paired scenarios with
the same seed produce identical trajectories only while the RNG is
consumed in the same order on both sides: pre-intervention
trajectories are byte-identical for `enable`/`disable` scenarios,
and correlated-but-not-identical for `set`/`scale` scenarios that
modify propensities from t=0. Any structural change that reorders
draws also breaks the coupling — this is paired-seed CRN, NOT
event-keyed RNG.

### Implementation phases

| Phase | Status    | Scope                                                      |
| ----- | --------- | ---------------------------------------------------------- |
| v0.1  | Complete  | Forward simulation + synthetic data generation             |
| v0.2  | Complete  | Inference: IF2 (MLE), PGAS+NUTS (Bayesian), particle filter, priors, real data input |
| v0.3  | In design | Hierarchical priors, reporting pipelines, spatial coupling |

### Inference algorithms

The inference stack lives in `rust/crates/sim/src/inference/`:

- `if2.rs` — Iterated filtering for maximum likelihood estimation
- `pgas.rs` — Particle Gibbs with Ancestor Sampling (production Bayesian method)
- `pgas_grad.rs` — Gradient evaluation for PGAS (uses compiler-emitted `rate_grad`)
- `nuts.rs` — No-U-Turn Sampler for gradient-based parameter proposals within PGAS
- `pmmh.rs` — Particle Marginal Metropolis-Hastings (experimental, gated)
- `particle_filter.rs` — Bootstrap particle filter
- `dmeasure.rs` — Observation likelihood compilation
- `obs_loglik.rs` — Distribution log-PMFs + analytical gradients (incl. digamma)

The OCaml compiler (`ocaml/lib/ir/autodiff.ml`) performs source-to-source
symbolic differentiation of rate expressions, emitting `rate_grad` fields
in the IR. The Rust backend evaluates these derivative expressions via
`eval_expr` — no runtime autodiff, no finite differences.

### DSL features for inference

- `events {}` — Scheduled discrete state modifications (cohort entry,
  importation). Sister construct to `interventions {}` but fires every
  substep. Uses `add()`, `transfer()`, `set()` actions.
- `balance {}` — Population conservation constraint. Applied last in each
  substep after transitions and events.
- `ivp: true` — Parameter type for initial value parameters (s0, e0).
  PGAS draws stochastic initial states via Binomial(N, param).

### Backend capabilities

Model features constrain which backends can run them. The `Capabilities`
bitflags in `rust/crates/sim/src/lib.rs` enforce this at dispatch time:

- `OVERDISPERSION`: transitions using `overdispersed(rate, σ²)` require tau-leap
  or chain-binomial (NegBinomial draws). Gillespie and ODE reject these models
  with a hard error.
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
something, it must either mean exactly that or produce a clear error. Examples:
`_args` patterns that discard function arguments, optional fields that default
to "works but wrong." If the compiler accepts it, the behavior must be fully
specified and intentional.

### Error messages are a feature, not polish

Error quality is a first-class design goal. A bad error message is a bug —
it means the compiler detected a problem but failed to help the user fix it.

Every diagnostic should:
- Show what went wrong (the mismatch, the constraint violation)
- Show where (source location, transition name, parameter name)
- Show why (the expected vs actual value, with domain-specific names)
- Suggest a fix when possible (hint text, corrected code)

When two possible error codes could fire for the same root cause, prefer the
one that points closest to the actual mistake. E.g., a parameter used
inconsistently across transitions should produce E303 ("conflicting
dimensions in transition A vs B") not E302 ("dimension mismatch in
addition") — even though E302 is technically correct, E303 gives the user
the cross-transition context they need.

Never use `failwith` or `assert false` for user-facing errors. These produce
stack traces instead of diagnostics. Use the Diagnostics module with error
codes, source locations, and hint text.

### Backwards compatibility is a non-goal

This is unreleased software. Do not add backwards-compatibility shims, `alias`
attributes, fallback deserialization paths, or deprecated field names. When a
field is renamed, rename it everywhere atomically. When a format changes, update
all golden files. Clean design beats legacy support.
