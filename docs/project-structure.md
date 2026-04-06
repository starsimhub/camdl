# Project Structure

Monorepo for stochastic compartmental epidemic modelling. Two independent
subsystems connected by a shared JSON IR (Intermediate Representation).

## Directory Layout

```
camdl/
├── README.md
├── CLAUDE.md                        # Claude Code project instructions
├── Makefile                         # Top-level: build, test, update-golden
│
├── ir/                              # ── The Contract ──
│   ├── schema.json                  # JSON Schema for the IR format
│   ├── VERSION                      # Schema version ("0.3")
│   └── golden/                      # Golden IR files (integration test surface)
│
├── ocaml/                           # ── Frontend: DSL → IR ──
│   ├── lib/
│   │   ├── compiler/                # Parser, lexer, AST, expander
│   │   │   ├── parser.mly           # Menhir grammar
│   │   │   ├── lexer.mll            # Ocamllex tokenizer
│   │   │   ├── ast.ml               # AST types
│   │   │   ├── expander.ml          # Stratification expansion
│   │   │   └── compiler.ml          # Top-level: source → IR model
│   │   └── ir/                      # IR types + serialization
│   │       ├── ir.ml / ir.mli       # Type definitions (mirrors Rust ir crate)
│   │       ├── serialize.ml         # Model → JSON
│   │       ├── deserialize.ml       # JSON → Model
│   │       ├── autodiff.ml          # Symbolic differentiation of expr trees
│   │       └── validate.ml          # IR validation
│   ├── bin/camdlc.ml                # Compiler CLI
│   ├── golden/                      # Golden .camdl → .ir.json pairs
│   └── test/                        # Alcotest suite (48 tests)
│
├── rust/                            # ── Backend: IR → Simulation + Inference ──
│   ├── crates/
│   │   ├── ir/                      # IR types + serde (mirrors OCaml ir/)
│   │   │   └── src/
│   │   │       ├── expr.rs          # Expression AST
│   │   │       ├── transition.rs    # Transitions + rate_grad
│   │   │       ├── model.rs         # Top-level Model type
│   │   │       └── ...              # parameter, observation, intervention, etc.
│   │   │
│   │   ├── sim/                     # Simulation + inference engine
│   │   │   └── src/
│   │   │       ├── compiled_model.rs    # IR → optimized runtime model
│   │   │       ├── propensity.rs        # eval_expr + eval_expr_deriv
│   │   │       ├── chain_binomial.rs    # Euler-multinomial simulation
│   │   │       ├── gillespie.rs         # Gillespie SSA
│   │   │       ├── tau_leap.rs          # Tau-leap
│   │   │       ├── ode.rs               # ODE (RK4)
│   │   │       ├── intervention.rs      # Scheduled events + interventions
│   │   │       └── inference/
│   │   │           ├── particle_filter.rs   # Bootstrap PF
│   │   │           ├── if2.rs               # Iterated filtering (MLE)
│   │   │           ├── pgas.rs              # Particle Gibbs with Ancestor Sampling
│   │   │           ├── pgas_grad.rs         # PGAS gradient evaluation
│   │   │           ├── nuts.rs              # No-U-Turn Sampler (HMC)
│   │   │           ├── pmmh.rs              # Particle Marginal MH (experimental)
│   │   │           ├── dmeasure.rs          # Observation likelihood compilation
│   │   │           ├── obs_loglik.rs        # Distribution log-PMFs + gradients
│   │   │           └── resampling.rs        # Systematic resampling
│   │   │
│   │   ├── cli/                     # CLI: camdl simulate/pfilter/if2/fit/profile
│   │   │   └── src/
│   │   │       ├── main.rs          # Command dispatch
│   │   │       └── fit/             # Multi-stage inference workflow
│   │   │           ├── scout.rs     # Landscape discovery
│   │   │           ├── refine.rs    # MLE refinement
│   │   │           ├── validate.rs  # Out-of-sample validation
│   │   │           ├── pmmh.rs      # PMMH posterior sampling
│   │   │           └── pgas.rs      # PGAS posterior sampling (production)
│   │   │
│   │   ├── io/                      # TSV read/write
│   │   ├── observe/                 # Observation projection + sampling
│   │   └── wasm/                    # WebAssembly bindings
│   │
│   └── tests/                       # Integration tests (golden deser + simulate)
│
├── docs/                            # ── Documentation ──
│   ├── camdl-language-spec.md       # DSL language specification
│   ├── compartmental-ir-spec.md     # IR JSON specification
│   ├── camdl-inference-spec.md      # Inference algorithms
│   ├── camdl-data-spec.md           # Data contract
│   ├── camdl-experiment-spec.md     # Experiment system
│   └── dev/                         # Developer docs
│       ├── proposals/               # Design proposals (isodate naming)
│       ├── reviews/                 # Code reviews (closed, isodate naming)
│       └── blog/                    # Dev blog posts
│
└── tests/                           # Cross-language integration tests
    └── test_ocaml_to_rust.sh
```

## Crate Dependency Order (Rust)

```
cli → io → observe → sim → ir
```

- **ir**: Pure types + serde. No simulation logic.
- **sim**: Simulation backends + propensity evaluator + inference algorithms.
- **observe**: Observation projection + likelihood sampling.
- **io**: TSV read/write.
- **cli**: Argument parsing + orchestration.

## OCaml Library Order

```
compiler → ir
```

- **ir**: Types + JSON serialization/deserialization + autodiff.
- **compiler**: Parser → AST → expander → IR model.
