# camdl

**Compartmental Model Description Language** — a DSL for stochastic
compartmental epidemic models. An OCaml frontend compiles `.camdl` files to a
JSON intermediate representation; a Rust backend simulates them.

```
model.camdl  →  camdlc  →  model.ir.json  →  camdl-sim  →  trajectory (stdout)
                                                          →  diagnostics.tsv
```

## Build

```bash
make build       # build both OCaml and Rust
make install     # copy binaries to ~/.local/bin
```

`make install` puts `camdlc`, `camdl-sim`, and `camdl` in `~/.local/bin`.
Make sure that's on your `PATH`. (Dune names its output `camdlc.exe`
internally — `make install` strips the suffix.)

## Quick start

### Compile then simulate (two steps)

```bash
# 1. Compile to IR JSON
camdlc ocaml/golden/sir_basic.camdl > /tmp/sir.ir.json

# 2. Simulate (TSV on stdout)
camdl-sim simulate /tmp/sir.ir.json \
  --set beta=0.3 --set gamma=0.1 --set N0=1000 --set I0=10 \
  --seed 42
```

### Compile and simulate in one command

```bash
camdl simulate ocaml/golden/sir_basic.camdl \
  --set beta=0.3 --set gamma=0.1 --set N0=1000 --set I0=10
```

`camdl` is a thin wrapper in `bin/camdl` that routes `compile`/`check`/`inspect`
to `camdlc` and `simulate` to `camdl-sim`. Both must be on `PATH`, or override
via `$CAMDLC` and `$CAMDL_SIM`.

---

## The camdl language

A `.camdl` file defines model structure. Parameter _values_ are supplied
externally at simulation time — via `--set` flags — never hardcoded in the
model.

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

### Age-structured SEIR with contact matrix

Stratification expands compartments and transitions at compile time. The IR
contains only flat compartments and fully-resolved rate expressions — no
stratification shorthand survives serialization.

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

3 base transitions → 6 expanded IR transitions: `infection_child`,
`infection_adult`, `progression_child`, etc.

### Seasonal forcing

Time-varying rates use declared `functions` blocks. The function name is then
called as an expression in any rate:

```
functions {
  seasonal : sinusoidal {
    amplitude = 0.3
    period    = 365.0
    phase     = 0.0
    baseline  = 1.0
  }
}

transitions {
  infection : S --> I  @ seasonal(t) * beta * S * I / N
  recovery  : I --> R  @ gamma * I
}
```

Supported function kinds: `sinusoidal`, `piecewise`, `interpolated`, `periodic`.

### Scheduled interventions

```
interventions {
  sia : transfer(fraction = 0.8, from = S, to = V) at [30, 60]
}
```

Action kinds: `transfer(fraction=..., from=..., to=...)`,
`transfer(count=..., from=..., to=...)`, and direct compartment assignment
(`name = value`). Schedule kinds: `at [t1, t2, ...]` and
`every E from F to T`.

### Key language features

- **Stratification**: `stratify(by = dim, values = [...])` — adds index
  dimensions; partial stratification with `only = [COMP]`
- **Indexed transitions**: `[a in age]` binds index variables; one concrete
  transition per combination
- **Consecutive iterator**: `(a, a_next) in consecutive(age)` for aging chains
  and Erlang sub-staging
- **Guards**: `where src != dst` — compile-time filtering; self-loops are
  dropped before the IR is written
- **Coupling sugar**: `coupling(age) = C_age` expands contact-matrix mixing
  into explicit indexed sums; the compiler auto-generates per-stratum
  denominators
- **Let bindings**: `let name[indices] = expr` — inlined at every use site
- **Parameterized tables**: table entries can reference parameters,
  e.g. `[[0.0, beta_mf], [beta_fm, 0.0]]`
- **Comparison operators**: `==`, `!=`, `<`, `>`, `<=`, `>=` usable in rate
  expressions (evaluate to 1.0/0.0) for conditional rates

Full language reference: `camdl-language-spec.md`

---

## camdlc

### Compile

Compile a `.camdl` file to IR JSON on stdout. Parameter values are not
required — the IR stores symbolic `Param("beta")` references. Use `--set` to
resolve them before serialization:

```bash
# Symbolic IR
camdlc ocaml/golden/sir_basic.camdl > sir.ir.json

# With parameter values embedded in the IR
camdlc ocaml/golden/sir_basic.camdl \
  --set beta=0.3 --set gamma=0.1 --set N0=1000 --set I0=10 \
  > sir_concrete.ir.json
```

### Check

Validate and summarise a model. No simulation, no parameters required:

```
$ camdlc check ocaml/golden/seir_age.camdl

seir_age

  compartments   4 base × 2 age = 8 expanded
  transitions     3 base → 6 expanded (+ 0 filtered by where)
  parameters      3 declared (beta: rate, sigma: rate, gamma: rate)
  tables          1 (C_age: age × age)
  let bindings    1 (N_local[a in age])
  dimensions      age = [child, adult]
  observations    0 streams
  interventions   0

  ✓ no errors, 0 warnings
```

Errors show source locations with underlines:

```
error[E100]: undeclared name 'R'

  ┌─ model.camdl:7:37
  │
7 │   recovery  : I --> R @ gamma * I * R
  │                                    ^
  │
  = hint: check spelling, or add a declaration in compartments/parameters/let/tables
```

Multiple errors in the same file are all reported in one pass.

### inspect

Explore what the compiler produced. Useful for verifying expansion is correct
before running a simulation.

**List all expanded transitions:**

```
$ camdlc inspect ocaml/golden/seir_age.camdl --transitions

infection[a in age] -> 2 transitions
  │ infection_child : S[child] -> E[child]
  │   @ beta × S[child] × (C_age[0,0] × I[child]/N_local[child] + C_age[0,1] × I[adult]/N_local[adult])
  │ infection_adult : S[adult] -> E[adult]
  │   @ beta × S[adult] × (C_age[1,0] × I[child]/N_local[child] + C_age[1,1] × I[adult]/N_local[adult])
```

Filter by pattern:

```bash
camdlc inspect model.camdl --transitions "infection_*"
```

**Inspect a single transition:**

```
$ camdlc inspect ocaml/golden/seir_age.camdl --transition infection_child

infection_child
  stoichiometry:  S[child] (−1)  →  E[child] (+1)
  rate:  beta × S[child] × (C_age[0,0] × I[child] / N_local[child]
                           + C_age[0,1] × I[adult] / N_local[adult])
```

**Summary (compartment and transition counts):**

```
$ camdlc inspect ocaml/golden/sir_five_age.camdl --summary

sir_five_age
  compartments   3 base × 5 age = 15 expanded
  transitions     5 base → 38 expanded (+ 0 filtered by where)
  dimensions      age = [age_0_5, age_5_15, age_15_50, age_50_65, age_65p]
```

Other flags: `--let "N_local[child]"`, `--expansion infection`, `--ir`
(raw IR JSON dump).

---

## camdl-sim

```
camdl-sim simulate MODEL [OPTIONS]

MODEL may be an .ir.json file or a .camdl source file (compiled via $CAMDLC).

Options:
  --backend  gillespie|tau_leap|chain_binomial  (default: gillespie)
  --dt       DT     time step for tau_leap / chain_binomial
  --seed     N      RNG seed (default: 42)
  --set      NAME=VALUE  override a parameter value
```

Output is a TSV trajectory on stdout. A `diagnostics.tsv` is also written to
the current directory:

```
transition     total_firings  mean_propensity  ...
infection      14523          0.342
recovery       14201          0.100
```

Zero-firing transitions are reported as warnings on stderr with a hint to use
`camdlc inspect` to debug the rate expression.

### Simulation backends

| Backend | Command | Notes |
|---|---|---|
| Gillespie SSA | `--backend gillespie` | Exact; default |
| Tau-leap | `--backend tau_leap --dt 0.5` | Fast approximation; needs `--dt` |
| Chain-binomial | `--backend chain_binomial --dt 1.0` | Discrete-time; needs `--dt` |

All backends use a stateful PRNG seeded by `--seed`. Same seed → identical
trajectory (Common Random Numbers). Useful for counterfactual comparisons: run
baseline and intervention scenario with the same seed to isolate the effect.

### Examples

```bash
# Gillespie (default)
camdl-sim simulate ir/golden/sir_basic.ir.json \
  --set beta=0.3 --set gamma=0.1 --set N0=1000 --set I0=10

# Tau-leap, daily steps
camdl-sim simulate ir/golden/sir_basic.ir.json \
  --backend tau_leap --dt 1.0 \
  --set beta=0.3 --set gamma=0.1 --set N0=10000 --set I0=100

# Directly from source (camdlc must be on PATH)
camdl-sim simulate ocaml/golden/seir_age.camdl \
  --set beta=0.4 --set sigma=0.2 --set gamma=0.1

# Reproducible pair (same seed, different beta)
camdl-sim simulate ir/golden/sir_basic.ir.json --set beta=0.3 ... --seed 1
camdl-sim simulate ir/golden/sir_basic.ir.json --set beta=0.5 ... --seed 1
```

---

## Testing

```bash
make test          # all OCaml + Rust tests
make test-ocaml    # OCaml compiler tests + IR round-trip only
make test-rust     # Rust unit + integration tests only
```

Individual Rust test files (from `rust/`):

```bash
cargo test --test golden_deser          # IR deserialisation
cargo test --test expr_eval             # expression evaluator
cargo test --test interventions         # FractionTransfer floor behaviour
cargo test --test gillespie_determinism # CRN seed reproducibility
```

### Golden files

| Directory | Contents | Tested by |
|---|---|---|
| `ocaml/golden/` | `.camdl` sources + compiled `.ir.json` | OCaml compiler tests |
| `ir/golden/` | Canonical `.ir.json` files | Rust deserialization tests |

Both sets are committed. Regenerate after compiler changes:

```bash
make update-golden   # recompile all DSL fixtures → ocaml/golden/*.ir.json
                     # and sync the shared models into ir/golden/
```

To regenerate a single file:

```bash
camdlc ocaml/golden/sir_basic.camdl > ocaml/golden/sir_basic.ir.json
```

---

## Repository layout

```
Makefile                 build / test / install / update-golden / sim
bin/
  camdl                  Wrapper script: routes compile/inspect → camdlc,
                         simulate → camdl-sim
ir/
  schema.json            IR schema (source of truth for both languages)
  VERSION                Schema version
  golden/                Canonical IR files — Rust deserialization test surface
ocaml/
  lib/ir/                IR types + Yojson serialization/deserialization
  lib/compiler/
    lexer.mll            Lexer (unit literals, keywords, Unicode ×)
    parser.mly           Menhir parser — full camdl grammar
    ast.ml               Surface AST types
    expander.ml          AST → flat IR (stratification, coupling, time funcs, etc.)
    diagnostics.ml       Structured error reporting with source locations
    inspect.ml           camdlc inspect subcommands
    pp_expr.ml           Human-readable rate expression printer
  bin/camdlc.ml          CLI (compile / check / inspect)
  golden/                .camdl fixtures + compiled .ir.json (compiler test surface)
  test/
    test_compiler.ml     Compiler unit + golden tests
    test_ir_roundtrip.ml IR serialization round-trip tests
rust/
  crates/ir/             IR types + serde
  crates/sim/            Gillespie / tau-leap / chain-binomial backends
                         + propensity evaluator + ekrng (library, unused in v0.1)
  crates/observe/        Observation model (likelihood sampling/scoring)
  crates/io/             TSV output
  crates/cli/            camdl-sim binary
```

---

## Architecture notes

The **IR is fully expanded**: the OCaml compiler performs all stratification,
coupling sugar expansion, and let-binding inlining. The Rust backend sees only
flat lists of compartments, transitions, and expression ASTs — no shorthand
survives serialization.

The **expression language** is a pure first-order AST:
`Const | Param | Pop | PopSum | Time | BinOp | UnOp | Cond | TimeFunc | TableLookup`.
Evaluated at each simulation step with bounded time. `Cond` guards against
division-by-zero. `TimeFunc` references a named time-varying function
(sinusoidal, piecewise, etc.) evaluated at the current simulation time.

**Common Random Numbers (CRN)**: all backends use a single stateful PRNG seeded
by `--seed`. Providing the same seed to two model variants (e.g. with and
without an intervention) draws the same sequence of random numbers, isolating
the causal effect. This is the standard scenario-comparison technique for
stochastic compartmental models.

---

## Documentation

- `camdl-language-spec.md` — full DSL reference (syntax, expansion rules, error catalog)
- `compartmental-ir-spec.md` — IR schema and expression language reference
