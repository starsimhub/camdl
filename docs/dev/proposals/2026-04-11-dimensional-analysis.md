# camdl Dimensional Analysis: Design Document

**Status:** Ready to implement\
**Author:** Vince Buffalo + Claude\
**Date:** 2026-04-12

## Motivation

The most common class of bug in compartmental epidemic models is a dimensional
error in a rate expression. Example: writing `beta * I / N` (units: 1/time, a
per-capita rate) when you meant `beta * S * I / N` (units: people/time, a
population-level rate). Today camdl silently compiles both — the Rust engine
sees floats. The user discovers the error only when calibration fails or
trajectories look wrong.

A compile-time dimensional analysis pass would catch these errors instantly,
before any simulation runs. For a DSL that aspires to make compartmental
modeling safe and expressive, this is table-stakes infrastructure.

## Design Principles

1. **Zero annotation burden for common models.** The compiler should infer
   dimensions from context (compartment references are `P`, parameters with
   `kind = rate` are `T⁻¹`, etc.). Explicit annotations are opt-in for
   disambiguation.
2. **Errors, not warnings.** A dimension mismatch is always a bug. The compiler
   should refuse to emit IR, same as an undeclared compartment.
3. **Small dimension space.** Compartmental epi models need exactly two base
   dimensions: **T** (time) and **P** (population). Everything is a product of
   integer powers of these: `P¹T⁰` for counts, `P⁰T⁻¹` for per-capita rates,
   `P¹T⁻¹` for population-level rates, `P⁰T⁰` for probabilities/dimensionless
   quantities.
4. **Graceful degradation.** When inference cannot determine a dimension
   (underconstrained system), emit an info-level diagnostic suggesting
   annotation — don't block compilation and don't silently skip.
5. **Global consistency.** A parameter used in multiple transitions must have a
   consistent dimension across all uses. Conflicting constraints are reported
   with both locations.

## Dimension Representation

A dimension is an integer vector indexed by base dimension. The initial base
dimensions are **P** (population, index 0) and **T** (time, index 1). The
representation uses `int array` rather than a fixed record, so adding a third
base dimension later (e.g., distinguishing host species in vector-borne models)
is a one-line registry change, not a type redesign.

```ocaml
(* Base dimension registry — extensible without changing the solver *)
let n_bases = 2
let base_P = 0
let base_T = 1

(* A dimension is an integer vector of exponents: dim.(base_P) = P exponent, etc. *)
type dim_vec = int array

let make p t = let d = Array.make n_bases 0 in d.(base_P) <- p; d.(base_T) <- t; d
let dimensionless = make 0 0    (* probabilities, ratios *)
let population    = make 1 0    (* compartment counts *)
let time          = make 0 1    (* durations *)
let rate_percap   = make 0 (-1) (* per-capita rates: 1/T *)
let rate_total    = make 1 (-1) (* total rates: P/T *)

(* Arithmetic: element-wise operations on the exponent vector *)
let dim_mul a b = Array.init n_bases (fun i -> a.(i) + b.(i))
let dim_div a b = Array.init n_bases (fun i -> a.(i) - b.(i))
let dim_eq  a b = Array.for_all2 (=) a b
```

The constraint solver operates identically — it's `n_bases` independent systems
of linear equations over integers, each solvable by substitution.

### Domain-Specific Display Names

Epidemiologists don't think in `P·T⁻¹` — they think in "total rate" and
"per-capita rate." Error messages always show BOTH the formal dimension and the
domain name:

| Dimension | Domain name                        | Examples                                 |
| --------- | ---------------------------------- | ---------------------------------------- |
| `(0, 0)`  | dimensionless (probability, ratio) | reporting probability ρ, R₀              |
| `(1, 0)`  | population count                   | compartment S, initial count N₀          |
| `(0, 1)`  | duration                           | infectious period, latent period         |
| `(0, -1)` | per-capita rate                    | recovery rate γ, mortality rate μ        |
| `(1, -1)` | population-level rate              | total propensity, force of infection × S |

```ocaml
let display_dim d =
  match (d.(base_P), d.(base_T)) with
  | (0, 0)  -> "dimensionless (probability, ratio)"
  | (1, 0)  -> "population count"
  | (0, 1)  -> "duration"
  | (0, -1) -> "per-capita rate"
  | (1, -1) -> "population-level rate"
  | (1, 1)  -> "population × duration"
  | (-1, 0) -> "inverse population (per-capita)"
  | _       -> Printf.sprintf "P^%d·T^%d" d.(base_P) d.(base_T)

let formal_dim d =
  match (d.(base_P), d.(base_T)) with
  | (0, 0)  -> "1"
  | (1, 0)  -> "P"
  | (0, 1)  -> "T"
  | (0, -1) -> "T⁻¹"
  | (1, -1) -> "P·T⁻¹"
  | _       -> Printf.sprintf "P^%d·T^%d" d.(base_P) d.(base_T)
```

Error messages render as: `dimension: T⁻¹ (per-capita rate)`.

### Arithmetic Rules

| Operation                       | Dimension rule                                                 |
| ------------------------------- | -------------------------------------------------------------- |
| `a + b`, `a - b`                | `dim(a) = dim(b)` (must match; result is the common dimension) |
| `a * b`                         | `dim(a) + dim(b)` (exponents add)                              |
| `a / b`                         | `dim(a) - dim(b)` (exponents subtract)                         |
| `a ^ n` (n constant)            | `n * dim(a)` (exponents scale)                                 |
| `a ^ b` (b non-const)           | Both must be dimensionless                                     |
| `exp(a)`, `log(a)`              | `a` must be dimensionless; result is dimensionless             |
| `sqrt(a)`                       | Exponents must be even; result is `dim(a) / 2`                 |
| `abs(a)`, `floor(a)`, `ceil(a)` | Result has same dimension as `a`                               |
| `min(a, b)`, `max(a, b)`        | `dim(a) = dim(b)`                                              |
| `a > b`, `a < b`, `a >= b`, etc | `dim(a) = dim(b)`; result is dimensionless                     |
| `cond(p, a, b)`                 | `p` dimensionless; `dim(a) = dim(b)`                           |

## Ground Truth: Where Dimensions Are Known

The system bootstraps from a small set of known-dimension nodes:

| Expression                              | Dimension                                 | Source                                |
| --------------------------------------- | ----------------------------------------- | ------------------------------------- |
| Compartment ref (`Pop`, `PopSum`)       | `P`                                       | Always — compartments are counts      |
| `Time` (`t`)                            | `T`                                       | Always                                |
| `Const` with unit suffix (`14 'days`)   | `T`                                       | Unit literal in AST                   |
| `Const` with rate unit (`0.1 'per_day`) | `T⁻¹`                                     | Unit literal in AST                   |
| `Const(0.0)` (bare zero)                | **any**                                   | Additive identity — adapts to context |
| Other bare `Const`                      | **dimensionless**                         | `3.14 * Pop("S")` has dim P           |
| Parameter with `kind = rate`            | `T⁻¹`                                     | `param_kind` field                    |
| Parameter with `kind = probability`     | `1` (dimensionless)                       | `param_kind` field                    |
| Parameter with `kind = count`           | `P`                                       | `param_kind` field                    |
| Parameter with `kind = positive`        | **unknown** — constrained by context      | Fresh variable                        |
| Parameter with `kind = real`            | **unknown** — constrained by context      | Fresh variable                        |
| Time function (`TimeFunc`)              | Dimensionless by default                  | Override with annotation              |
| Table lookup                            | Dimensionless by default                  | Override with annotation              |
| `Projected`                             | Matches the observation's projection type | CumulativeFlow → P, CurrentPop → P    |

## Inference Algorithm

### Whole-Model Constraint Solving

The checker operates globally across all transitions, not per-transition. This
is critical: when `beta` appears in multiple rate expressions, the constraints
from ALL uses must be consistent.

#### Phase 1: Generate Constraints

Walk every transition's rate expression. At every node, emit constraints:

- **Known node** (compartment, typed parameter, unit literal): assign its known
  dimension.
- **Unknown node** (untyped parameter, `kind = positive/real`): assign a fresh
  variable `?_i`.
- **BinOp(Mul, a, b)**: result = dim(a) + dim(b).
- **BinOp(Add, a, b)**: dim(a) = dim(b), result = dim(a).
- **BinOp(Div, a, b)**: result = dim(a) - dim(b).
- **UnOp(Exp, a)**: dim(a) = dimensionless, result = dimensionless.
- **Transition rate constraint**: dim(rate) = `P·T⁻¹`.

Each constraint carries a location string for error reporting:
`"transition 'infection', rate subexpression 'beta * I'"`.

#### Phase 2: Solve

The constraint system is `n_bases` independent systems of linear equations over
integers (one per base dimension). Solve by substitution:

1. Partition constraints into per-base-dimension systems.
2. For each system, propagate known values through equality constraints.
3. When a variable is determined, substitute into all remaining constraints.
4. If a constraint becomes `known ≠ known` → **dimension error**.
5. If a variable remains undetermined after propagation → **unable to infer**
   (see Phase 4).

No full unification engine needed — this is Gaussian elimination on a sparse
system with integer coefficients.

#### Phase 3: Check Cross-Transition Consistency

After solving, verify that each parameter has at most one inferred dimension
across all transitions. If transition A implies `dim(beta) = T⁻¹` and transition
B implies `dim(beta) = P`:

```
error[E303]: parameter 'beta' has conflicting dimensions

  In transition 'infection':
    rate = beta * S * I / N
    inferred dimension of beta: T⁻¹ (per-capita rate)

  In transition 'waning':
    rate = beta * R
    inferred dimension of beta: P⁻¹·T⁻¹

  These are incompatible. Check that 'beta' is used consistently,
  or use different parameters for different roles.
```

#### Phase 4: Handle Undetermined Variables

If a parameter or expression has `kind = positive` or `kind = real` and the
constraints don't fully determine its dimension:

- **If only one dimension is undetermined** (e.g., P exponent known but T
  unknown): try to resolve from the transition rate constraint.
- **If fully underdetermined** (two unknowns, not enough constraints): emit an
  **info-level diagnostic**, not an error:

```
info[I300]: dimension of parameter 'alpha' could not be determined

  alpha has kind = positive, which is compatible with any positive dimension.
  It appears in: transition 'progression', rate = alpha * beta * E

  hint: annotate with an explicit dimension:
    alpha : positive [T⁻¹]
  or use a more specific kind:
    alpha : rate
```

This is the right tradeoff: inference when it can, ask when it can't, never
silently wrong.

#### Phase 5: Report

On failure, show the user exactly which subexpression has the wrong dimension,
with the inferred dimension of each operand.

### The Transition Rate Constraint

This is the key domain-specific constraint that makes the whole system useful.
In a chain-binomial (Euler-multinomial) step, the engine computes:

```
per_capita_rate = propensity / n_source
p_exit = 1 - exp(-per_capita_rate * dt)
n_events ~ Binomial(n_source, p_exit)
```

For `exp(-x)` to be valid, `x` must be dimensionless. Since `dt` has dimension
`T`, `per_capita_rate` must have dimension `T⁻¹`. Therefore `propensity` must
have dimension `P · T⁻¹` (people per unit time).

The universal rule is: **every transition rate expression must have dimension
`P · T⁻¹`**.

## Known Edge Cases

### The Iota (Seeding) Pattern

The standard epi pattern for preventing stochastic extinction is:

```
rate = beta * (I + iota) * S / N
```

where `iota` is a small constant like `1e-6`. If written as a bare constant, the
checker sees `Pop(I) + Const(1e-6)` → `P + dimensionless` → **ERROR**.

This is technically correct — `iota` IS a population count (fractional people)
and should be declared as such:

```
parameters {
    iota : count = 1e-6
}
```

But this WILL surprise users. The error message must anticipate this:

```
error[E302]: dimension mismatch in addition

  rate = beta * (I + 1e-6) * S / N
                 ~~~~~~~~~
  Pop("I") has dimension P (population count)
  Const(1e-6) has dimension 1 (dimensionless)
  Cannot add P + 1 — dimensions must match.

  hint: If 1e-6 is a small population count (seeding/iota term),
        declare it as a parameter:
          iota : count = 1e-6
        Then write: beta * (I + iota) * S / N

  hint: If you intended this as a fraction, multiply instead:
          beta * (I + 1e-6 * N) * S / N
```

### Let Binding Source Locations

The checker runs on post-expansion IR, so let bindings are already inlined.
Error messages show the expanded expression, not the original `foi * S`. Where
possible, error messages include the let binding origin:

```
error[E300]: dimension mismatch in transition 'infection'

  rate = beta * I / N * S
         ~~~~~~~~~~~
         expanded from let binding 'foi'
         dimension: T⁻¹ (per-capita rate)
```

This requires carrying source-origin metadata through expansion. If not feasible
in phase 1, add it as a follow-up.

### Overdispersion σ²

The `overdispersed(rate, σ²)` wrapper has a separate constraint:
`dim(σ²) = dimensionless`. σ² is a variance parameter of the Gamma multiplier,
which is dimensionless by construction.

### Balance Expressions

The balance expression (e.g., `R = N - S - E - I`) computes a population count,
so `dim(balance_expr) = P`.

### ODE Equations

`d(compartment)/dt` has dimension `P/T`. The derivative expression must match:
`dim(derivative) = P·T⁻¹`.

### Observation Likelihood Expressions

`Projected` gets its dimension from the observation's projection type:

- `CumulativeFlow` → `P`
- `CurrentPop` → `P`
- `DerivedExpr` → inferred from the expression

Likelihood parameters (mean, sd, dispersion) should be checked for consistency
with the projected quantity.

## Syntax Extensions

### Explicit Parameter Annotations (Optional, Phase 2)

For parameters where `kind` doesn't fully specify the dimension:

```
parameters {
    beta : rate                    # T⁻¹ (inferred from param_kind)
    gamma : rate                   # T⁻¹
    N0 : count                     # P (inferred from param_kind)
    amplitude : real [1]           # explicitly dimensionless
    mu : real [P/T]                # explicit dimension
    contact_rate : real [1/(P*T)]  # per-capita per-time contact rate
}
```

The `[dim]` syntax is only needed for `kind = positive` or `kind = real`
parameters where inference can't determine the dimension.

### Table and Time Function Annotations (Optional, Phase 2)

```
tables {
    pop_table [P] { ... }
}

forcing {
    seasonal [1] = sinusoidal(...)
    birth_rate [P/T] = interpolated(...)
}
```

Default (no annotation) = dimensionless, correct for most time functions and
tables.

### Unit Literal Extensions (Future, Phase 3)

The current unit system handles simple time conversions. Compound rates like
"2.3 per 100,000 person-years" are expressed with arithmetic:

```
parameters {
    incidence_rate = 2.3 'per_year / 100000
}
```

A richer unit literal syntax is deferred to phase 3. The arithmetic approach
works and is explicit.

## Error Message Catalog

| Code | Condition                                                     | Severity |
| ---- | ------------------------------------------------------------- | -------- |
| E300 | Transition rate has wrong dimension                           | Error    |
| E301 | Argument to exp/log/pow has non-dimensionless dimension       | Error    |
| E302 | Addition/subtraction of mismatched dimensions                 | Error    |
| E303 | Parameter used with conflicting dimensions across transitions | Error    |
| E304 | sqrt of odd-exponent dimension                                | Error    |
| E305 | Balance expression has non-population dimension               | Error    |
| E306 | ODE derivative has wrong dimension                            | Error    |
| E307 | Observation model parameter has wrong dimension               | Error    |
| E308 | Overdispersion σ² has non-dimensionless dimension             | Error    |
| I300 | Parameter dimension could not be inferred                     | Info     |
| I301 | Dimension check skipped (--no-dim-check)                      | Info     |

## Implementation Plan

### Phase 1: Core Checker (OCaml)

Add `lib/ir/dimcheck.ml` (~500 lines):

```ocaml
(** Dimension of an IR expression node. *)
type dim =
  | Known of dim_vec          (* fully determined *)
  | Unknown of int            (* fresh variable ID *)
  | Any                       (* compatible with anything: Const 0.0 *)

(** A constraint on two dimensions. *)
type constraint_ = {
  lhs: dim;
  rhs: dim;
  loc: string;
  transition: string;
}

(** Result of checking one parameter's global consistency. *)
type param_check = {
  name: string;
  inferred: dim option;
  constraints: (string * dim) list;
}

(** Check all transitions in a model. Pushes diagnostics. *)
val check_model : Ir.model -> Diagnostics.t -> unit
```

The checker:

1. Generates constraints from all transition rate expressions.
2. Solves the global constraint system (two independent integer linear systems).
3. Checks cross-transition parameter consistency.
4. Emits errors for mismatches, infos for undetermined variables.
5. Verifies every transition rate has dimension `P·T⁻¹`.
6. Checks balance expressions (dim = P), ODE derivatives (dim = P·T⁻¹),
   overdispersion (dim = 1).

Enable with `--dim-check` initially. Make mandatory once golden tests pass. Ship
with `--no-dim-check` escape hatch.

### Phase 2: Annotations (Parser + IR)

- Add `[dim]` syntax to parameter, table, time function declarations.
- Propagate to IR (new optional `dimension` field).
- Serialize/deserialize.

### Phase 3: Unit Literal Extensions (Future)

- Richer unit literal syntax if demand warrants.

## Test Plan

### Unit Tests (`test/test_dimcheck.ml`)

Each test constructs an IR expression tree, runs the checker, and asserts either
success or a specific error code.

**Basic arithmetic rules:**

```
test "add_same_dim"         → Pop("S") + Pop("I")             → OK (P)
test "add_mismatched_dim"   → Pop("S") + Time                 → Error E302: P ≠ T
test "mul_dims_add"         → Pop("S") * Param("beta":rate)   → OK (P·T⁻¹)
test "div_dims_subtract"    → Pop("S") / Pop("N")             → OK (1)
test "div_pop_by_time"      → Pop("S") / Time                 → OK (P·T⁻¹)
```

**Transition rate constraints:**

```
test "sir_correct"          → beta * S * I / N                 → OK (P·T⁻¹)
test "sir_missing_S"        → beta * I / N                     → Error E300 (T⁻¹)
test "sir_wrong_param_kind" → p:probability * S * I / N        → Error E300 (P)
test "recovery_correct"     → gamma * I                        → OK (P·T⁻¹)
test "inflow_correct"       → mu * N                           → OK (P·T⁻¹)
test "inflow_bare_rate"     → mu                               → Error E300 (T⁻¹)
```

**Iota / seeding pattern:**

```
test "iota_bare_const_rejected"    → beta * (I + 1e-6) * S / N   → Error E302
test "iota_typed_param_ok"         → beta * (I + iota:count) * S / N → OK
test "iota_error_message_has_hint" → verify E302 message contains "seeding" hint
```

**Cross-transition consistency:**

```
test "param_consistent_across_transitions"     → alpha used as T⁻¹ in both → OK
test "param_inconsistent_across_transitions"   → alpha as T⁻¹ in A, P·T⁻¹ in B → Error E303
test "unknown_param_inferred_globally"         → alpha:positive in alpha*I → inferred T⁻¹ ✓
```

**Undetermined parameters:**

```
test "underdetermined_emits_info"   → alpha:pos * beta:pos * I → Info I300 for both
test "partially_determined"         → alpha:pos * beta:rate * I → dim(alpha) = 1 ✓
```

**Transcendental functions:**

```
test "exp_dimensionless_ok"   → exp(p:probability)  → OK (1)
test "exp_dimensioned_fail"   → exp(Pop("S"))        → Error E301
test "log_dimensionless_ok"   → log(S / N)           → OK (1)
test "log_dimensioned_fail"   → log(S)               → Error E301
test "sqrt_even_powers"       → sqrt(S * I)          → OK (P)
test "sqrt_odd_powers_fail"   → sqrt(S * t)          → Error E304
```

**Constants and zero:**

```
test "zero_compatible_with_pop"     → S + Const(0.0)           → OK (P)
test "zero_compatible_with_rate"    → beta:rate + Const(0.0)   → OK (T⁻¹)
test "bare_const_is_dimensionless"  → Const(3.14) * S          → OK (P)
test "unit_const_is_time"           → Const(14, 'days)         → OK (T)
test "unit_const_is_inv_time"       → Const(0.3, 'per_day)     → OK (T⁻¹)
```

**Conditionals:**

```
test "cond_branches_match"     → cond(I > 0, beta*S, 0.0) → OK (P·T⁻¹)
test "cond_branches_mismatch"  → cond(I > 0, S, beta)     → Error E302
```

**Balance and ODE:**

```
test "balance_population_ok"   → R = N - S - E - I        → OK (P)
test "balance_wrong_dim"       → R = gamma                 → Error E305
test "ode_derivative_correct"  → d(V)/dt = -decay * V      → OK (P·T⁻¹)
test "ode_derivative_wrong"    → d(V)/dt = V               → Error E306
```

**Golden file tests (most important — no false positives):**

```
test "sir_basic_golden"         → 0 errors
test "seir_vaccine_golden"      → 0 errors
test "sir_overdispersion"       → 0 errors
test "polio_spatial_5"          → 0 errors
test "seir_seasonal_patch"      → 0 errors
test "malaria_two_species"      → 0 errors
```

**Negative golden tests (intentionally broken `.camdl` files):**

```
test "e300_missing_susceptible.camdl"    → Error E300
test "e300_rate_is_probability.camdl"    → Error E300
test "e301_exp_of_count.camdl"           → Error E301
test "e302_add_count_and_rate.camdl"     → Error E302
test "e302_iota_bare_const.camdl"        → Error E302 (with iota hint)
test "e303_param_inconsistent.camdl"     → Error E303
```

**Property-based tests (QCheck):**

```
property "mul_then_div_preserves_dim"    → dim(e * x / x) = dim(e)
property "add_requires_matching_dims"    → mismatched non-zero → Error
property "zero_is_universal_identity"    → e + Const(0) always OK
```

### Integration Test

```bash
for f in golden/*.camdl; do
  camdlc --dim-check "$f" || exit 1
done
```

## Migration Path

Unreleased code — ship mandatory with escape hatch:

- Default: dimension checker runs on every compilation.
- `--no-dim-check`: disable for models where false positives occur.
- Users who hit false positives use the flag and file a bug.
- Fix the checker. Once stable, deprecate `--no-dim-check`.

## Estimated Effort

| Component                                    | Est. LOC  | Notes                          |
| -------------------------------------------- | --------- | ------------------------------ |
| `dimcheck.ml` (constraint gen + solving)     | 250       | Core algorithm                 |
| `dimcheck.ml` (global consistency)           | 100       | Cross-transition checks        |
| `dimcheck.ml` (error messages + display)     | 150       | Domain names, hints, iota hint |
| Parser/AST changes (`[dim]` syntax, phase 2) | 50        | Optional annotations           |
| IR changes (dimension field)                 | 30        |                                |
| Serialize/deserialize for dimension field    | 20        |                                |
| Test suite (`test_dimcheck.ml`)              | 600+      | Extensive, as specified above  |
| Golden negative test files                   | 120       | ~12 small .camdl files         |
| Integration in `camdlc` CLI                  | 20        | Flag handling                  |
| **Total**                                    | **~1350** |                                |

## Resolved Design Questions

1. **`Const(1.0)` — dimensionless or universal?** → Dimensionless. Only
   `Const(0.0)` is universal (additive identity).

2. **`mod(a, b)`?** → `dim(a) = dim(b)`, result has same dimension.

3. **Observation likelihood expressions?** → Yes, checked. `Projected` dimension
   comes from projection type.

4. **ODE equations?** → `dim(derivative) = P·T⁻¹`. Same constraint as transition
   rates.

5. **Balance expressions?** → `dim(balance_expr) = P`.

6. **Iota pattern?** → Error with specific hint. Users declare iota as
   `kind = count`.

7. **Per-transition or whole-model?** → Whole-model. Parameters must be globally
   consistent.

8. **Undetermined dimensions?** → Info diagnostic suggesting annotation.
   Compilation continues.
