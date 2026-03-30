# camdl Code Review & Nigeria Polio Roadmap

**Codebase:** OCaml compiler (4,053 LOC) + Rust runtime (2,641 LOC) **Date:**
2026-03-16

---

## Part 1: Architecture Assessment

The two-language split is well-executed. The OCaml compiler owns everything
structural (parsing, expansion, type checking, inspect, diagnostics). The Rust
runtime owns everything hot-path (propensity evaluation, Gillespie SSA, state
management, output). The IR JSON is the contract between them — the Rust side
never parses `.camdl` files and the OCaml side never simulates.

The compiler pipeline (parse → collect → desugar coupling → expand → serialize)
is clean and each phase is independently testable. The runtime's `CompiledModel`
pre-computes all index lookups at load time so the simulation inner loop is
tight.

**Lines of code by function:**

```
OCaml compiler:
  inspect.ml      848   (36% of inspect spec already implemented)
  expander.ml     744   (core compiler logic)
  parser.mly      464   (Menhir grammar)
  deserialize.ml  437   (IR JSON → OCaml types)
  serialize.ml    338   (OCaml types → IR JSON)
  ir.ml           232   (IR type definitions)
  ast.ml          191   (AST type definitions)
  pp_expr.ml      173   (expression pretty-printer)
  diagnostics.ml  147   (error collection/rendering)

Rust runtime:
  gillespie.rs    274   (SSA + PDMP hybrid)
  validate.rs     254   (IR validation)
  propensity.rs   218   (expression evaluator)
  compiled_model  204   (index precomputation)
  tau_leap.rs     176   (tau-leaping backend)
  chain_binomial  158   (discrete-time backend)
  intervention.rs 130   (FractionTransfer, Set)
  cli/main.rs     123   (CLI driver)
  state.rs        118   (IntState, RealState, FlowVec, Trajectory)
  ekrng.rs         90   (EKRNG primitives — not yet wired into Gillespie)
```

---

## Part 2: Bugs (Must Fix)

### BUG-1: EKRNG not wired into Gillespie (Critical)

**File:** `rust/crates/sim/src/gillespie.rs:55-59`

The Gillespie loop uses `StatefulRng` (sequential ChaCha8) for both the
inter-event time draw and the event selection. The `EkRng` struct exists and is
correctly implemented (hash-based, stateless per draw), but it's never called
during simulation.

**Impact:** Scenario coupling is completely broken. Running baseline and
with_sia with the same seed will produce different random streams as soon as any
divergence occurs. The Nigeria polio use case requires valid counterfactual
comparison — this is the #1 blocker.

**Fix:** Switch to the next-reaction method (Gibson & Bruck 2000). For each
transition, draw a per-transition waiting time via
`ekrng.exp_keyed(event_key, counter, propensity)`. Select the transition with
the smallest waiting time. This makes every random draw keyed to a specific
transition, guaranteeing order-independence. The transition's event_key is
already in the IR.

Alternatively, for v0.1, use the direct method but draw per-transition uniform
variates via EkRng for the event selection step. The inter-event time can remain
a global draw (it's the sum propensity, not per-transition). This is simpler but
doesn't give full EKRNG guarantees — a middle ground.

### BUG-2: Parameterized table entries silently dropped

**File:** `ocaml/lib/compiler/expander.ml:607-625`

`flatten_elist` only extracts `EConst` and `EUnit` values. Any expression that
isn't a literal (including `EIdent` for parameter references) returns `[]`. This
means:

```
tables {
  B_sex : sex × sex = [[0.0, beta_mf], [beta_fm, 0.0]]
}
```

produces an empty table (`flat_vals = []`) and is silently filtered out by
`expand_tables`. The STI model (§23.4) and any model with inferrable contact
matrix entries is broken.

**Fix:** The IR `table` type needs to support `expr` values, not just `float`.
Two options:

(a) Add `Ir.table.expr_values: expr list option` alongside `values: float list`.
When parameter expressions are present, store them in `expr_values`. The Rust
runtime evaluates `expr_values` at simulation time using the parameter vector.
Tables with only literals use `values` (fast path).

(b) Simpler: always store table values as `Ir.expr list`. The serializer writes
`{"const": 0.0}` for literals and `{"param": "beta_mf"}` for parameters. The
Rust runtime evaluates each entry once at model load time (when params are
known). This adds negligible overhead — table values are evaluated once, not
per-event.

Option (b) is cleaner. It requires:

- `Ir.table.values` becomes `Ir.expr list` (OCaml side)
- `Table.values` becomes `Vec<Expr>` (Rust side)
- `CompiledModel::new()` evaluates table exprs into a `Vec<Vec<f64>>` cache,
  used by `table_lookup`
- `flatten_elist` is replaced by a recursive `flatten_expr_list` that preserves
  `EIdent` as `Ir.Param`

### BUG-3: Comparison operators crash the compiler

**File:** `ocaml/lib/compiler/expander.ml:176-177`

```ocaml
let ir_bin_op = function
  | Ast.Eq | Ast.Neq | Ast.Lt | Ast.Gt | Ast.Le | Ast.Ge ->
    failwith "comparison operators not supported in rates"
```

But the spec explicitly allows `if I > 0 then beta * S * I / N else 0.0` in rate
expressions (§9.7). The `ECond` is parsed correctly, but when its predicate
contains a comparison, the compiler crashes.

**Root cause:** The OCaml IR `bin_op` type and the Rust IR `BinOp` enum both
lack comparison operators. They only have `Add|Sub|Mul|Div|Pow|Min|Max`.

**Fix:**

1. Add `Eq|Neq|Lt|Gt|Le|Ge` to `Ir.bin_op` (OCaml) and `ir::expr::BinOp` (Rust)
2. Update `ir_bin_op` in the expander to map them through
3. Update `eval_expr` in the Rust propensity evaluator to handle them (return
   1.0 for true, 0.0 for false — consistent with the `Cond` predicate semantics
   where `pred > 0.0` means true)
4. Update the JSON serializer/deserializer for both sides

### BUG-4: Time functions resolve to zero

**File:** `ocaml/lib/compiler/expander.ml:281`

```ocaml
| EFuncCall _ -> Ir.Const 0.0
```

Any function call in a rate expression becomes `Const 0.0`. This means
`@ beta * seasonal * S * I / N` where `seasonal` is a declared sinusoidal
function will have `seasonal = 0.0` — killing all infection.

**The problem is two-fold:**

(a) When `seasonal` appears as a bare `EIdent` in the rate, `resolve_ident_name`
doesn't check `func_decls`. It falls through to the "undeclared name" error path
— or worse, if there's a parameter named `seasonal`, it maps to that.

(b) When `seasonal(...)` appears as `EFuncCall`, it's explicitly mapped to
`Const 0.0`.

**Fix:**

1. In `resolve_ident_name`: after checking let bindings, compartments, and
   parameters, check `ctx.func_decls`. If the name matches a declared function,
   emit `Ir.TimeFunc name`.
2. In `resolve_expr` for `EFuncCall`: check if the function name is a declared
   time function. If so, emit `Ir.TimeFunc name` (the arguments are already
   encoded in the function declaration, not in the call site).
3. Add `expand_time_functions` to the expander that converts `Ast.func_decl` to
   `Ir.time_function` with resolved argument values. Currently
   `model.time_functions` is always `[]`.

### BUG-5: FractionTransfer uses `round` instead of `floor`

**File:** `rust/crates/sim/src/intervention.rs:80`

```rust
let transfer = ((int_s.counts[s_local] as f64) * frac).round() as i64;
```

The spec (§14.1) says: "delta = floor(source * fraction)". `round` can transfer
more individuals than intended when the fractional part is ≥ 0.5. For
`fraction = 0.8` and `source = 3`, `round(2.4) = 2` (OK), but for `source = 5`,
`round(4.0) = 4` (OK). The difference matters at small population sizes —
`round(1.5) = 2` but `floor(1.5) = 1`.

**Fix:** Change `.round()` to `.floor()`.

### BUG-6: Output schedule from DSL is ignored

**File:** `ocaml/lib/compiler/expander.ml:688-697`

```ocaml
let expand_output ctx =
  ...
  { Ir.times = Ir.OutRegular { Ir.start = 0.0; Ir.step = 1.0; Ir.end_ = t_end };
```

This always outputs at step=1.0 regardless of the user's
`output { trajectories { every = 7 'days } }`. The output block is parsed and
stored in `ctx.output_decl` but never read during IR generation.

**Fix:** Read `ctx.output_decl`, extract the `every` expression, resolve it, and
use it as the step. Fall back to 1.0 if no output block is present.

---

## Part 3: Design Issues (Should Fix)

### DESIGN-1: No standalone ODE backend

The RK4 integrator exists for PDMP real compartments but there's no backend that
treats all compartments as continuous. Adding one is ~150 lines of Rust:

```rust
fn run_ode(model: &CompiledModel, params: &[f64], cfg: &OdeConfig)
  -> Result<Trajectory, SimError>
{
    let (int_s, real_s) = model.initial_state(params)?;
    // Treat int compartments as f64 for ODE
    let mut state: Vec<f64> = int_s.counts.iter().map(|&c| c as f64).collect();
    state.extend_from_slice(&real_s.values);

    // RHS: dx/dt = stoichiometry^T * rates(x, t)
    let rhs = |state: &[f64], t: f64| -> Vec<f64> {
        // ... evaluate all propensities, multiply by stoichiometry
    };

    // Adaptive RK45 or fixed-step RK4 with output at scheduled times
    // ...
}
```

High value for: parameter exploration (100-1000x faster than Gillespie),
Gillespie validation (mean of N runs ≈ ODE), IF2 warm-starting.

### DESIGN-2: Intervention expansion not implemented in compiler

The expander generates `model.interventions = []` (line 720). Intervention
declarations in the DSL are parsed and stored in `ctx.interv_decls` but never
expanded to IR interventions. The Rust intervention runtime is fully implemented
and tested against hand-crafted IR — the gap is only in the compiler.

### DESIGN-3: Observation model expansion not implemented

Same gap: `model.observations = []` (line 722). The observation model
declarations are parsed but not expanded. This blocks the scoring primitive
needed for inference.

### DESIGN-4: Timepoints block ignored

Line 79: `| DTimepoints _ -> ()`. User-defined timepoints (§15) are silently
discarded. The `t_start`/`t_end` built-ins from the simulate block work, but
custom timepoints for intervention timing or summary expressions don't.

### DESIGN-5: CLI is minimal

The Rust CLI accepts only `simulate` with an IR JSON file. There's no
`camdl compile` → `camdl simulate` pipeline, no `--params` file loading, no
`--output-dir`, no file output. The OCaml binary (`camdlc`) handles compilation
and inspection but doesn't invoke the Rust runtime.

Need: a unified `camdl` binary (or shell wrapper) that chains
`camdlc compile → camdl-sim simulate` in a single command.

### DESIGN-6: RK4 is fixed-step, no error control

The ODE integrator uses fixed-step RK4. For stiff systems (fast recovery + slow
demography), this can be unstable or require tiny steps. Not a problem for PDMP
(short intervals between stochastic events), but a problem for the standalone
ODE backend.

For v0.1: adaptive RK45 (Dormand-Prince) is sufficient and straightforward. For
production: bind to SUNDIALS CVODE via FFI (handles stiffness).

---

## Part 4: Code Smells

### SMELL-1: Mutable context with repeated list appends

`Expander.context` is a record of mutable lists, and `collect_declarations`
appends with `@` (O(n) per append). For large models this is quadratic. Not a
problem at current scale but will be for 774-patch models with many transitions.

Fix: use `Buffer`-style accumulators or reverse-cons + `List.rev` at the end.

### SMELL-2: `failwith` in parser actions

Several parser actions use `failwith` for unknown identifiers (line 103:
`failwith ("unknown unit: " ^ s)`, line 285:
`failwith ("unknown likelihood: " ^ s)`). These produce unhelpful stack traces
instead of the nice error display system. Should emit diagnostics with source
locations.

### SMELL-3: Hardcoded epsilon comparisons

The Gillespie loop uses `1e-12` and `1e-10` as tolerance values in multiple
places (lines 74, 104, 142, 150, 209, 264). These should be a named constant or
a config field.

### SMELL-4: `ignore` of dead code in inspect.ml

Line 58: `ignore (v, d)` and line 77: `ignore pp_one` — dead code from an
earlier iteration. The `pp_one` function is shadowed by `pp_one_correct`. Delete
the dead code.

### SMELL-5: `all_expanded_compartments` recomputed on every ident resolution

`resolve_ident_name` calls `all_expanded_compartments ctx` (line 291) which
recomputes the full Cartesian product of compartments × strata every time an
identifier is resolved. For a 774-patch model with 5 compartments and 2 age
groups, this is 7,740 strings generated on every expression node. Cache this
once in the context.

### SMELL-6: Serde untagged enum for IR expressions is fragile

The Rust `Expr` enum uses `#[serde(untagged)]` (line 131) which relies on each
variant having a uniquely-named field to disambiguate during deserialization.
This works but is brittle — adding a new variant with a field name that overlaps
an existing one will silently break deserialization. Consider
`#[serde(tag = "type")]` (internally tagged) for robustness, at the cost of
slightly more verbose JSON.

---

## Part 5: Nigeria Polio Roadmap

### The model

Nigeria cVDPV2 transmission: 774 LGAs, 2 age groups (0-5, 5+), SEIR+V with SIA
campaigns. Seasonal forcing, gravity-model spatial coupling, importation, OPV
campaign interventions at specific times. Calibration to AFP surveillance data
via IF2 or PMCMC. Scenario comparison: baseline vs SIA campaigns with EKRNG
coupling.

### What's needed, in dependency order

**Phase 1: Correctness (bugs that break the model)**

```
Week 1:
  [ ] BUG-3: Add comparison operators to IR + evaluator
  [ ] BUG-4: Wire time functions through compiler
  [ ] BUG-2: Parameterized table values (expr, not float)
  [ ] BUG-6: Wire output schedule from DSL
  [ ] BUG-5: floor not round in FractionTransfer

Week 2:
  [ ] BUG-1: EKRNG in Gillespie (next-reaction method)
  [ ] DESIGN-2: Intervention expansion in compiler
  [ ] DESIGN-3: Observation model expansion in compiler
  [ ] SMELL-5: Cache expanded compartment names
```

**Phase 2: Usability (run the Nigeria model end-to-end)**

```
Week 3:
  [ ] Unified CLI: camdl compile + simulate in one command
  [ ] --params FILE loading (TOML)
  [ ] File output (TSV to --output-dir, not stdout)
  [ ] Wire output schedule from DSL to IR
  [ ] Test: compile + run sir_basic through full pipeline

Week 4:
  [ ] ODE backend (standalone, all compartments continuous)
  [ ] Spatial scale test: 774-patch × 2-age model
  [ ] Profile: is propensity evaluation the bottleneck?
  [ ] If yes: consider sparse propensity updates (dependency graph)
  [ ] diagnostics.tsv for all backends (not just Gillespie)
```

**Phase 3: Inference (calibrate to AFP data)**

```
Week 5-6:
  [ ] Scoring primitive: score_observation in runtime
  [ ] Particle filter (bootstrap SMC)
  [ ] camdl score: evaluate log p(data|theta) at a point
  [ ] IF2: iterated filtering with parameter perturbation
  [ ] camdl fit --method if2

Week 7-8:
  [ ] PMCMC (particle MCMC with random walk proposal)
  [ ] camdl fit --method pmcmc
  [ ] Profile likelihood for key parameters (beta, rho)
  [ ] ODE warm-start for IF2 initialization
```

**Phase 4: Production (Nigeria analysis)**

```
Week 9-10:
  [ ] Content-addressable output directories
  [ ] Scenario comparison with EKRNG coupling
  [ ] Ensemble summary across seeds
  [ ] camdl compare for paired scenario analysis
  [ ] Write the actual Nigeria SEIR+V .camdl model file
  [ ] Calibrate to POLIS AFP data
  [ ] Run baseline vs SIA scenarios
```

### Scale considerations

774 patches × 2 age groups × 5 compartments = 7,740 expanded compartments. With
infection (age-mixing + spatial coupling), recovery, death, aging, migration,
birth, importation: roughly 50,000-100,000 expanded transitions depending on
migration kernel sparsity.

**Gillespie performance concern:** With 100K transitions, computing all
propensities per event is O(100K) per event. For a model with ~1M events per
year of simulated time, this is ~10^11 operations for a 2-year run. That's
minutes, not seconds.

**Mitigation options (in order of effort):**

1. Sparse propensity updates — only recompute propensities for transitions whose
   source compartment changed. Requires a dependency graph from compartments to
   transitions. The CompiledModel already has stoichiometry; the inverse map
   (compartment → affected transitions) is straightforward.
2. Tau-leaping for large populations — approximate, O(N_transitions) per step
   rather than per event. Already implemented.
3. ODE for deterministic dynamics — O(N_transitions) per step with adaptive
   stepping. Use for parameter exploration, switch to Gillespie for final
   stochastic runs.
4. Spatial decomposition — if patches are weakly coupled, run each patch
   independently with migration as external forcing. This is model-specific and
   probably v0.3.

The sparse propensity update (option 1) is the highest-value optimization. It
reduces per-event cost from O(N_transitions) to O(N_affected) where N_affected
is typically 10-50 transitions (the ones sharing a compartment with the fired
transition). This is a 1000x speedup for the 774-patch model.

**Implementation sketch:**

```rust
// In CompiledModel:
// For each compartment, which transition indices reference it?
pub comp_to_transitions: Vec<Vec<usize>>,

// In Gillespie loop, after firing event:
// Only recompute propensities for affected transitions
let affected = &model.comp_to_transitions[changed_comp];
for &tr_idx in affected {
    let old = propensities[tr_idx];
    propensities[tr_idx] = eval_expr(...)?;
    lambda_total += propensities[tr_idx] - old;
}
```

This requires maintaining `lambda_total` incrementally rather than summing the
full vector each time. It's the standard Gillespie optimization and should be
implemented before the Nigeria model is run at scale.
