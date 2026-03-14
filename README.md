# camdl

**Compartmental Model Description Language** — a DSL for stochastic compartmental
epidemic models. OCaml frontend compiles `.camdl` files to a JSON intermediate
representation; Rust backend simulates them.

```
model.camdl  →  camdlc compile  →  model.ir.json  →  compartmental simulate  →  trajectories.tsv
                                                                               →  diagnostics.tsv
```

## Quick start

```bash
# Build everything
cd ocaml && dune build
cd rust  && cargo build --release

# Compile a model to IR
cd ocaml
dune exec bin/camdlc.exe -- compile golden/sir_basic.camdl > /tmp/sir.ir.json

# Simulate from IR
cd rust
./target/release/compartmental simulate /tmp/sir.ir.json \
  --set beta=0.3 --set gamma=0.1 --set N0=1000 --set I0=10 \
  --traj /tmp/traj.tsv --obs /tmp/obs.tsv

# Validate and inspect a model without compiling
cd ocaml
dune exec bin/camdlc.exe -- check golden/seir_age.camdl
```

## The camdl language

A `.camdl` file defines model structure. Parameter *values* are always supplied
externally — via `--set` flags or a params file — never in the model definition.

### Minimal SIR

```
time_unit = 'days

compartments { S, I, R }
let N = S + I + R

parameters {
  beta  : rate
  gamma : rate
  N0    : count
  I0    : count
}

transitions {
  infection : S --> I  @ beta * S * (I / N)
  recovery  : I --> R  @ gamma * I
}

init {
  S = N0 - I0
  I = I0
}

simulate {
  from = 0 'days
  to   = 120 'days
}
```

### Age-structured SEIR

Stratification expands compartments and transitions at compile time. The
expanded IR sees only flat compartments and transitions — no stratification
shorthand survives serialization.

```
time_unit = 'days

compartments { S, E, I, R }
stratify(by = age, values = [child, adult])

let N_local[a in age] = S[a] + E[a] + I[a] + R[a]

parameters {
  beta  : rate
  sigma : rate
  gamma : rate
}

tables {
  C_age : age × age = [[12.0, 4.0], [4.0, 8.0]]
}

transitions {
  infection[a in age] : S[a] --> E[a]
    @ beta * S[a] * sum(b in age, C_age[a, b] * I[b] / N_local[b])

  progression[a in age] : E[a] --> I[a]  @ sigma * E[a]
  recovery[a in age]    : I[a] --> R[a]  @ gamma * I[a]
}

init {
  S[child] = 4990
  S[adult] = 5000
  I[child] = 10
}
```

3 base transitions → 6 expanded IR transitions. The compiler generates
`infection_child`, `infection_adult`, etc. with fully resolved rate expressions.

### Key language features

- **Stratification**: `stratify(by = dim, values = [...])` — adds index dimensions
  to compartments; partial stratification with `only = [COMP]`
- **Indexed transitions**: `[a in age]` binds index variables; the compiler
  produces one concrete transition per combination
- **Consecutive iterator**: `(a, a_next) in consecutive(age)` for aging chains,
  Erlang sub-staging
- **Compartment iteration**: `c in compartments` iterates all integer compartments
- **Guards**: `where src != dst` — compile-time filtering; self-loops and
  impossible combinations are dropped before the IR is written
- **Coupling sugar**: `coupling(age) = C_age` expands contact-matrix mixing
  into explicit indexed sums; the compiler auto-generates per-stratum denominators
- **Let bindings**: top-level `let name[indices] = expr` — inlined at every
  use site; names are unique regardless of arity
- **Tables**: `C_age : age × age = [[...]]` — shape-checked, unit-annotated,
  loaded at compile time and inlined into the IR

Full language reference: `camdl-language-spec.md`

## camdlc commands

### compile

Compile a `.camdl` file to IR JSON. Parameter values are not required — the
IR stores symbolic `Param("beta")` references. Use `--set` to resolve them:

```bash
# Symbolic IR (no parameter values)
dune exec bin/camdlc.exe -- golden/sir_basic.camdl > sir.ir.json

# With parameter overrides resolved in the IR
dune exec bin/camdlc.exe -- golden/sir_basic.camdl \
  --set beta=0.3 --set gamma=0.1 --set N0=1000 --set I0=10 \
  > sir_concrete.ir.json
```

### check

Validate a model and show a summary. No simulation, no parameters needed:

```
$ dune exec bin/camdlc.exe -- check golden/seir_age.camdl

seir_age

  compartments   4 base × 2 age = 8 expanded
  transitions     3 base → 6 expanded (+ 0 filtered by where)
  parameters      3 declared (beta: rate, sigma: rate, gamma: rate)
  tables          1 (C_age: age × age)
  let bindings    1 (N_local[a in age])
  dimensions      age = [child, adult]
  observations    0 streams
  interventions   0 (0 active by default)

  ✓ no errors, 0 warnings
```

Errors show source locations with underlines:

```
error[E100]: undeclared name 'R'

  ┌─ model.camdl:7:37
  │
7 │   recovery  : I --> R @ gamma * I * R   # R is undeclared
  │                                    ^
  │
  = hint: check spelling, or add a declaration in compartments/parameters/let/tables
```

Multiple errors in the same file are all reported in one pass.

### inspect

Explore what the compiler actually produced. The IR for a large spatial model
may have thousands of transitions — `inspect` lets you verify the expansion is
correct before running a simulation.

**List all expanded transitions, grouped by base transition:**

```
$ dune exec bin/camdlc.exe -- inspect golden/seir_age.camdl --transitions

infection[a in age] -> 2 transitions
  │ infection_child : S[child] -> E[child]
  │   @ beta × S[child] × (C_age[0,0] × I[child]/N_local[child] + C_age[0,1] × I[adult]/N_local[adult])
  │ infection_adult : S[adult] -> E[adult]
  │   @ beta × S[adult] × (C_age[1,0] × I[child]/N_local[child] + C_age[1,1] × I[adult]/N_local[adult])

progression[a in age] -> 2 transitions
  │ progression_child : E[child] -> I[child]   @ sigma × E[child]
  │ progression_adult : E[adult] -> I[adult]   @ sigma × E[adult]
```

Filter by pattern:

```bash
dune exec bin/camdlc.exe -- inspect model.camdl --transitions "infection_*"
```

**Inspect a single transition's rate and stoichiometry:**

```
$ dune exec bin/camdlc.exe -- inspect golden/seir_age.camdl --transition infection_child

infection_child
  stoichiometry:  S[child] (−1)  →  E[child] (+1)

  rate (total propensity):
    beta × S[child] × (C_age[0,0] × I[child] / N_local[child]
                     + C_age[0,1] × I[adult] / N_local[adult])

  where:
    N_local[child] = S[child] + E[child] + I[child] + R[child]
    N_local[adult] = S[adult] + E[adult] + I[adult] + R[adult]

  origin:     transmission
  event key:  infection_child:{firing_index}
```

**Inspect a let binding at specific indices:**

```bash
dune exec bin/camdlc.exe -- inspect model.camdl --let "N_local[child]"
```

**Show coupling sugar expansion (before → after):**

```bash
dune exec bin/camdlc.exe -- inspect model.camdl --expansion infection
```

**Transition counts by dimension (useful for large models):**

```bash
dune exec bin/camdlc.exe -- inspect golden/sir_five_age.camdl --summary

sir_five_age
  compartments   3 base × 5 age = 15 expanded
  transitions     5 base → 38 expanded (+ 0 filtered by where)
  dimensions      age = [age_0_5, age_5_15, age_15_50, age_50_65, age_65p]
```

### simulate (Rust CLI)

```bash
# Gillespie SSA (default)
compartmental simulate model.ir.json \
  --set beta=0.3 --set gamma=0.1 --set N0=1000 --set I0=10 \
  --traj traj.tsv --obs obs.tsv

# Tau-leaping
compartmental simulate model.ir.json --backend tau_leap ...

# Chain-binomial (discrete time)
compartmental simulate model.ir.json --backend chain_binomial ...
```

Every run writes a `diagnostics.tsv` alongside the trajectory:

```
transition_name    total_firings  mean_propensity  max_propensity  first_firing  last_firing
infection          14523          0.342            1.207           0.003         119.87
recovery           14201          0.100            0.100           2.841         119.99
```

Zero-firing transitions are reported as warnings on stderr with a hint to use
`camdlc inspect` to debug the rate expression.

## Testing

```bash
# OCaml compiler tests (golden IR round-trip)
cd ocaml && dune runtest

# Rust unit + integration tests
cd rust && cargo test

# Single test file
cd rust && cargo test --test golden_simulate
cd rust && cargo test --test expr_eval
cd rust && cargo test --test gillespie_invariants
```

Golden files in `ocaml/golden/` are the DSL→IR integration test surface.
Golden files in `ir/golden/` are the IR→simulation integration test surface.
Both sets are committed; changes to either must be intentional.

To regenerate golden IR from DSL fixtures:

```bash
make update-golden    # recompile DSL fixtures → ir/golden/*.ir.json
make update-expected  # re-simulate golden models → ir/expected/*.tsv
```

## Repository layout

```
ir/
  schema.json            IR schema (source of truth for both languages)
  golden/                committed IR files — Rust integration test surface
  expected/              expected simulation output — determinism test surface
ocaml/
  lib/ir/                IR types + Yojson serialization/deserialization
  lib/compiler/
    lexer.mll            Lexer (unit literals, keywords, Unicode ×)
    parser.mly           Menhir parser — full camdl grammar
    ast.ml               Surface AST types
    expander.ml          AST → flat IR (stratification, coupling, guards, etc.)
    diagnostics.ml       Structured error reporting with source locations
    inspect.ml           camdlc inspect subcommands
    pp_expr.ml           Human-readable rate expression printer
    validate.ml          Post-expansion IR validation
  bin/camdlc.ml          CLI (compile / check / inspect)
  golden/                camdl source fixtures + expected IR (compiler tests)
  test/
    test_compiler.ml     Golden compilation tests
    test_ir_roundtrip.ml IR serialization round-trip tests
    errors/              Fixtures for error message tests
rust/
  crates/ir/             IR types + serde deserialization
  crates/sim/            Simulation backends + propensity evaluator
    transition_diagnostics.rs  Per-transition firing statistics
  crates/observe/        Observation model (likelihood sampling/scoring)
  crates/io/             TSV output
  crates/cli/            compartmental simulate ...
```

## Architecture notes

The **IR is fully expanded**: no stratification shorthand survives serialization.
The OCaml compiler performs all expansion; the Rust backend sees only flat lists
of compartments, transitions, and expression ASTs.

The **expression language** is a pure first-order AST —
`Const | Param | Pop | PopSum | Time | BinOp | UnOp | Cond | TimeFunc | TableLookup`
— evaluated at each simulation step. No side effects, bounded evaluation time.
`Cond` guards against division-by-zero in propensity evaluation.

**EKRNG**: transitions carry an `event_key` used to seed per-event draws from a
counter-based PRNG (Philox/ChaCha). Same seed + same event key → same draw,
regardless of evaluation order. Enables valid counterfactual scenario comparison:
two model variants sharing a seed draw comparable randomness for the same events.

## Documentation

- `camdl-language-spec.md` — full DSL reference (syntax, expansion rules, error catalog)
- `compartmental-ir-spec.md` — IR schema and expression language reference
- `todo.md` — known gaps and follow-up tasks
