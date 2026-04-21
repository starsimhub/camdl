---
status: proposed
date: 2026-04-20
scope: ocaml/bin/camdlc.ml, rust/crates/cli/src/
priority: low — quality of life / learning tool
---

# Proposal: camdl Interactive Inspector (REPL)

## Motivation

A read-only interactive mode would accelerate model development and onboarding:

- **Learning tool.** New users can explore models interactively — list compartments,
  trace how a rate expression evaluates at specific parameter values, inspect what
  stratification produced — without reading IR JSON by hand.

- **Dimension/stratum visualization.** Inspecting `model_structure` fields
  interactively gives immediate feedback on what the expander did: which compartments
  got which dimensions, how transitions expanded, what the contact matrix looks like
  flattened.

- **Autodiff transparency.** `grad infection beta` shows the symbolic derivative
  expression the Rust backend will evaluate — useful for catching wrong-zero
  gradients and verifying parametrization decisions.

## Non-goal: True Incremental REPL

The DSL is file-oriented: `compartments {}` has no meaning without `transitions {}`,
stratification depends on declarations that may precede the current statement, etc.
Incremental parsing of partial models would require significant parser changes and
a new "fragment evaluation" mode in the expander.

This proposal scopes to an **IR inspector** — the compiled IR is fixed at load time,
and the interactive session only reads it. No mutation, no re-compilation.

## Interface Sketch

```
$ camdl inspect sir_basic.camdl
compiled in 12ms: 3 compartments · 2 transitions · 4 parameters

camdl> help
  show compartments          list compartments and kinds
  show transitions           list transitions with rate expressions
  show parameters            list parameters with bounds and priors
  show dims                  show model_structure dimensions
  show strata                show compartment→dimension mapping
  eval <expr>                evaluate expression at current params
  grad <transition> <param>  show symbolic ∂rate/∂param
  sim [--steps N] [--seed S] run a short simulation
  set <param>=<value>        override a parameter value
  params <file.toml>         load parameter values
  quit

camdl> show dims
  age: [child, adult]
  patch: [north, south, east, west, center]

camdl> show strata
  S → [age, patch]
  E → [age, patch]
  ...

camdl> eval "beta * S * I / N" --beta 0.3 --S 900 --I 10 --N 1000
2.7

camdl> grad infection beta
  ∂/∂beta = S * I / PopSum(S, I, R)
```

## Implementation Path

The plumbing already exists:

1. **Load step** — `camdlc compile` already produces an `Ir.model`. The
   `compile_detail_result` function gives access to `ctx` and `summary` for
   dimension/structure metadata.

2. **Eval** — the `eval` subcommand in the Rust CLI already evaluates expressions
   against an IR. Factor it into a library function callable per-command.

3. **Grad** — `Autodiff.differentiate_rate` is already callable and `rate_grad`
   is already in the IR for compiled models.

4. **Sim** — the existing `simulate` backend can be driven with `--steps N`.

5. **Interactive loop** — `rustyline` (already in Rust ecosystem) for history +
   line editing. Command dispatch is ~200 lines.

The OCaml side can optionally expose `camdlc inspect <file>` that compiles and
immediately enters the Rust-side interactive loop by exec-ing `compartmental inspect`.

## Dimension / Strata Visualization

The most immediately useful feature for stratified models: show how the expander
expanded compartments and which dimension indices each compartment carries.
`model_structure.compartment_dims` already stores this; the inspector just renders it.

For contact mixing patterns specifically, `show table C_age` could print the matrix
in human-readable form — useful when verifying that a WAIFW matrix loaded from a
TSV looks right before running inference.

## Scope for Initial Version

- `show compartments`, `show transitions`, `show parameters`, `show dims`, `show strata`
- `eval <expr>` with `--param name=value` overrides
- `grad <transition> <param>` to render the symbolic derivative
- `params <file.toml>` to load a full parameter set
- `sim --steps N --seed S` for quick sanity checks

Deferred:
- Tab completion of compartment/parameter names
- Syntax highlighting
- Web/notebook interface
- Streaming output for long simulations
