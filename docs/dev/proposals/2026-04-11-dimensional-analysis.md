---
status: proposal
date: 2026-04-11
note: Compile-time dimensional analysis pass for rate expressions. Implement after review/cleanup pass.
---

# camdl Dimensional Analysis: Design Document

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
4. **Incremental adoption.** Existing `.camdl` files should continue to compile
   without changes. The dimension checker runs as an optional pass initially,
   becoming mandatory in a future release.

## Dimension Representation

A dimension is a pair of integers `(p, t)` representing `P^p · T^t`:

```ocaml
type dim = { p: int; t: int }

let dimensionless = { p = 0; t = 0 }  (* probabilities, ratios *)
let population    = { p = 1; t = 0 }  (* compartment counts *)
let time          = { p = 0; t = 1 }  (* durations *)
let rate_percap   = { p = 0; t = -1 } (* per-capita rates: 1/T *)
let rate_total    = { p = 1; t = -1 } (* total rates: P/T *)
```

Arithmetic rules:

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
| `cond(p, a, b)`                 | `p` dimensionless; `dim(a) = dim(b)`                           |

## Ground Truth: Where Dimensions Are Known

The system bootstraps from a small set of known-dimension nodes:

| Expression                            | Dimension                                 | Source                           |
| ------------------------------------- | ----------------------------------------- | -------------------------------- |
| Compartment ref (`Pop`, `PopSum`)     | `P`                                       | Always — compartments are counts |
| `Time` (`t`)                          | `T`                                       | Always                           |
| `Const` with unit suffix (`14 'days`) | `T`                                       | Unit literal in AST              |
| Bare `Const` (no unit)                | **unknown** — inferred from context       | See below                        |
| Parameter with `kind = rate`          | `T⁻¹`                                     | `param_kind` field               |
| Parameter with `kind = probability`   | `1` (dimensionless)                       | `param_kind` field               |
| Parameter with `kind = count`         | `P`                                       | `param_kind` field               |
| Parameter with `kind = positive`      | **unknown**                               | Could be anything positive       |
| Parameter with `kind = real`          | **unknown**                               | Unconstrained                    |
| Time function (`TimeFunc`)            | Dimensionless by default                  | Override with annotation         |
| Table lookup                          | Dimensionless by default                  | Override with annotation         |
| `Projected`                           | Matches the observation's projection type | See below                        |

## Inference Algorithm

The checker walks the resolved IR expression tree bottom-up. At each node, it
either knows the dimension (from ground truth) or infers it from the arithmetic
rules. When it encounters an `Add`/`Sub` where one operand has a known dimension
and the other is unknown, it propagates. When both are unknown, it defers.

For bare constants: `Const 0.0` is compatible with any dimension (it's the
additive identity). All other bare constants are treated as dimensionless unless
context forces otherwise. This handles the common pattern `beta * S * I / N`
where `beta` is a rate parameter and `N` is a population — the compiler can
verify the whole expression has dimension `P/T`.

### Constraint-Based Approach

Rather than a single bottom-up pass, use lightweight constraint generation +
solving:

1. **Generate constraints.** Walk each transition's rate expression. At every
   `BinOp`, `UnOp`, etc., emit dimension constraints between the operands and
   result. Assign a fresh dimension variable to each `Param`/`Const` whose
   dimension isn't known from ground truth.

2. **Solve.** The constraint system is linear in the exponents `(p, t)` — it's
   two independent systems of linear equations over integers. This is trivially
   solvable by substitution (no need for a full unification engine).

3. **Check.** For each transition, verify that `dim(rate) = P · T⁻¹` (total
   propensity). If the transition has a source compartment (per-capita rate
   semantics), verify `dim(rate / Pop(source)) = T⁻¹` — i.e., the rate divided
   by the source population is a per-capita rate.

4. **Report.** On failure, show the user exactly which subexpression has the
   wrong dimension, with the inferred dimension of each operand.

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
have dimension `P · T⁻¹` (people per unit time) for transitions with a source
compartment, or just `T⁻¹` scaled appropriately for inflows.

The compiler already distinguishes these cases via `source_groups` (transitions
grouped by source compartment). The dimension checker uses the same information:

- **Source-grouped transition:** `dim(rate) = P · T⁻¹`
- **Inflow (no source):** `dim(rate) = P · T⁻¹` (Poisson mean = rate × dt must
  be dimensionless → rate is `P/T`)

So the universal rule is: **every transition rate expression must have dimension
`P · T⁻¹`**.

## Syntax Extensions

### Explicit Parameter Annotations (Optional)

For parameters where `kind` doesn't fully specify the dimension (e.g.,
`kind = positive` could be a rate or a count), allow explicit annotation:

```
parameters {
    beta : rate            # T⁻¹ (already works via param_kind)
    gamma : rate           # T⁻¹
    N0 : count             # P (already works via param_kind)
    amplitude : real [1]   # explicitly dimensionless
    mu : real [P/T]        # explicit dimension
    contact_rate : real [1/(P*T)]  # per-capita per-time contact rate
}
```

The `[dim]` syntax is only needed for `kind = positive` or `kind = real`
parameters. For `kind = rate/probability/count`, the dimension is already
determined.

### Table and Time Function Annotations

```
tables {
    # Table values are populations (counts per patch)
    pop_table [P] { ... }
}

forcing {
    # Seasonal forcing is dimensionless (multiplier)
    seasonal [1] = sinusoidal(...)
    
    # Birth rate has dimension P/T
    birth_rate [P/T] = interpolated(...)
}
```

Default (no annotation) = dimensionless, which is correct for most time
functions (seasonal multipliers) and tables (indices, multipliers).

## Implementation Plan

### Phase 1: Core Inference Engine (OCaml)

Add a new file `lib/ir/dimcheck.ml` (~300 lines estimated):

```ocaml
(** Dimension of an IR expression node. *)
type dim = 
  | Known of { p: int; t: int }
  | Unknown of int  (* fresh variable ID *)
  | Any             (* compatible with anything: zero constants *)

(** Constraint: two dimensions must be equal. *)
type constraint_ = {
  lhs: dim;
  rhs: dim;
  loc: string;  (* human-readable location for error reporting *)
}

(** Check all transitions in a model. Returns a list of errors. *)
val check_model : Ir.model -> Diagnostics.t -> unit
```

The checker runs after `expand_detail` returns the IR model, before
serialization. It's a pure function over the IR — no mutation of the model.

### Phase 2: Error Messages

Good error messages are critical. Example:

```
error[E300]: dimension mismatch in transition 'infection' rate expression

  rate = beta * I / N
         ~~~~~~~~~~~
         dimension: T⁻¹ (per-capita rate)

  Expected: P·T⁻¹ (population-level rate)

  hint: This looks like a per-capita rate (T⁻¹). For a transition
        with source compartment S, the rate must be the TOTAL
        propensity (P·T⁻¹). Did you mean:

    rate = beta * S * I / N
           ~~~~~~~~~~~~~~~~
           dimension: P·T⁻¹ ✓

  note: beta has dimension T⁻¹ (declared as kind = rate)
        I has dimension P (compartment)
        N has dimension P (compartment)
        beta * I / N = T⁻¹ · P / P = T⁻¹
```

### Phase 3: Annotations & Ergonomics

- Add `[dim]` syntax to parameters, tables, time functions in the parser/AST
- Propagate to IR (new optional `dimension` field on `parameter`, `table`,
  `time_function`)
- Infer parameter dimensions from `param_kind` when no explicit annotation

### Phase 4: Let Bindings

Let bindings are inlined during expansion, so their dimensions are checked at
use sites. But it's useful to show the inferred dimension in
`camdl inspect --let`:

```
λ[a in age]  =  beta[a] * I[a] / N[a]
                dimension: T⁻¹  (per-capita rate)
```

## Test Plan

### Unit Tests (OCaml test suite)

These go in `test/test_dimcheck.ml`. Each test constructs an IR expression tree,
runs the checker, and asserts either success or a specific error.

**Basic arithmetic rules:**

```
test "add_same_dim"         → Pop("S") + Pop("I")             → OK (P)
test "add_mismatched_dim"   → Pop("S") + Time                 → Error: P ≠ T
test "mul_dims_add"         → Pop("S") * Param("beta":rate)   → OK (P·T⁻¹)
test "div_dims_subtract"    → Pop("S") / Pop("N")             → OK (dimensionless)
test "div_pop_by_time"      → Pop("S") / Time                 → OK (P·T⁻¹)
```

**Transition rate constraints:**

```
test "sir_correct"
  S→I rate = beta * S * I / N
  beta: rate (T⁻¹), S: P, I: P, N: P
  dim = T⁻¹ · P · P / P = P·T⁻¹ ✓

test "sir_missing_S"
  S→I rate = beta * I / N
  dim = T⁻¹ · P / P = T⁻¹
  Expected P·T⁻¹ → Error

test "sir_wrong_param_kind"
  S→I rate = p * S * I / N
  p: probability (dimensionless)
  dim = 1 · P · P / P = P
  Expected P·T⁻¹ → Error

test "recovery_correct"
  I→R rate = gamma * I
  gamma: rate (T⁻¹), I: P
  dim = T⁻¹ · P = P·T⁻¹ ✓

test "inflow_correct"
  ∅→S rate = mu * N
  mu: rate (T⁻¹), N: P
  dim = T⁻¹ · P = P·T⁻¹ ✓

test "inflow_bare_rate"
  ∅→S rate = mu
  mu: rate (T⁻¹)
  dim = T⁻¹
  Expected P·T⁻¹ → Error (missing population factor)
```

**Transcendental functions:**

```
test "exp_dimensionless_ok"  → exp(Param("p":probability))  → OK (dimensionless)
test "exp_dimensioned_fail"  → exp(Pop("S"))                 → Error: argument to exp() must be dimensionless
test "log_dimensionless_ok"  → log(Pop("S") / Pop("N"))      → OK (dimensionless)
test "log_dimensioned_fail"  → log(Pop("S"))                 → Error
test "sqrt_even_powers"      → sqrt(Pop("S") * Pop("I"))     → OK (P)
test "sqrt_odd_powers_fail"  → sqrt(Pop("S") * Time)         → Error: cannot take sqrt of P·T (odd exponents)
```

**Constants and zero:**

```
test "zero_compatible_with_pop"   → Pop("S") + Const(0.0)    → OK (P)
test "zero_compatible_with_rate"  → Param("beta":rate) + Const(0.0)  → OK (T⁻¹)
test "bare_const_is_dimensionless" → Const(3.14) * Pop("S")  → OK (P)
test "unit_const_is_time"         → Const(14.0, 'days)       → OK (T)
```

**Conditionals:**

```
test "cond_branches_match"
  cond(Pop("I") > Const(0), Param("beta":rate) * Pop("S"), Const(0.0))
  → OK (P·T⁻¹)  [zero adapts to branch dimension]

test "cond_branches_mismatch"
  cond(Pop("I") > Const(0), Pop("S"), Param("beta":rate))
  → Error: branches have different dimensions (P vs T⁻¹)

test "cond_pred_not_dimensioned"
  cond(Pop("S"), ...)  → Warning: predicate has dimension P, expected dimensionless
```

**PopSum:**

```
test "popsum_is_population"  → PopSum(["S"; "I"; "R"])  → OK (P)
test "popsum_in_rate"
  rate = beta * S * I / PopSum(["S"; "I"; "R"])
  → OK (P·T⁻¹)
```

**Overdispersion:**

```
test "overdispersed_sigma_sq_dimensionless"
  overdispersed(rate_expr, sigma_sq)
  sigma_sq must be dimensionless → OK

test "overdispersed_sigma_sq_dimensioned"
  overdispersed(rate_expr, Pop("S"))
  → Error: σ² must be dimensionless
```

**Time functions:**

```
test "timefunc_default_dimensionless"
  Param("beta":rate) * TimeFunc("seasonal")
  → OK (T⁻¹)

test "timefunc_annotated_rate"
  TimeFunc("birth_rate", dim=P·T⁻¹) + Param("gamma":rate) * Pop("I")
  → OK (P·T⁻¹)
```

**Table lookups:**

```
test "table_default_dimensionless"
  Param("beta":rate) * TableLookup("contact_matrix", [...])
  → OK (T⁻¹)

test "table_annotated_population"
  TableLookup("pop_table", [...], dim=P) / Pop("N")
  → OK (dimensionless)
```

**Unit conversions:**

```
test "unit_literal_days"    → Const(14, 'days) has dim T
test "unit_literal_per_day" → Const(0.3, 'per_day) has dim T⁻¹
test "rate_times_duration"  → Param("gamma":rate) * Const(14, 'days) → dimensionless ✓
```

**Let binding inlining:**

```
test "let_force_of_infection"
  let foi = beta * I / N   (* dim: T⁻¹ *)
  rate = foi * S            (* dim: T⁻¹ · P = P·T⁻¹ ✓ *)

test "let_wrong_usage"
  let x = Pop("S")         (* dim: P *)
  rate = x                  (* dim: P, expected P·T⁻¹ → Error *)
```

**Stratified models (spatial):**

```
test "spatial_coupling_correct"
  (* S_i → I_i at rate = beta * S_i * sum_j(c_ij * I_j) / N_i *)
  (* c_ij is dimensionless (contact matrix), I_j is P, sum is P *)
  (* beta * P * P / P = P·T⁻¹ ✓ *)

test "spatial_sum_preserves_dim"
  sum(j in patches, contact[i,j] * Pop("I", j))
  → each term is 1 · P = P, sum is P ✓
```

**Real-world golden file tests:**

```
test "sir_basic_golden"        → compile sir_basic.camdl, run dimcheck → 0 errors
test "seir_vaccine_golden"     → compile seir_vaccine.camdl → 0 errors
test "sir_overdispersion"      → compile sir_overdispersion.camdl → 0 errors
test "polio_spatial_5"         → compile polio_spatial_5.camdl → 0 errors
test "seir_seasonal_patch"     → compile seir_seasonal_patch.camdl → 0 errors
test "malaria_two_species"     → compile malaria_two_species.camdl → 0 errors
```

These golden tests are the most important — they verify that the dimension
checker doesn't reject correct models.

**Negative golden tests (intentionally broken models):**

Create small `.camdl` files in `test/errors/` with known dimensional errors:

```
test "e300_missing_susceptible.camdl"
  (* S→I with rate = beta * I / N — missing S *)
  → Error E300

test "e300_rate_is_probability.camdl"
  (* recovery rate = p (probability, not rate) * I *)
  → Error E300

test "e301_exp_of_count.camdl"
  (* rate = exp(S) — S is a count, not dimensionless *)
  → Error E301

test "e302_add_count_and_rate.camdl"
  (* rate = beta + S — adding T⁻¹ and P *)
  → Error E302
```

### Integration Tests

Run `camdlc --check` on every golden `.camdl` file with the dimension checker
enabled:

```bash
# All golden files must pass dimension checking
for f in golden/*.camdl; do
  camdlc --check "$f" || exit 1
done
```

### Property-Based Tests

Using QCheck or similar:

```
property "mul_then_div_preserves_dim"
  ∀ dim d, e with dim(e) = d:
    dim(e * x / x) = d

property "add_requires_matching_dims"
  ∀ e1 e2 where dim(e1) ≠ dim(e2):
    check(e1 + e2) = Error

property "zero_is_universal_identity"
  ∀ e:
    check(e + Const(0)) = OK
    check(Const(0) + e) = OK
```

## Migration Path

1. **v0.3.x (initial):** Dimension checker runs as `--warn-dimensions` flag.
   Existing models not affected. All golden tests pass. Users can opt in.
2. **v0.4.0:** Dimension checker runs by default but errors are downgraded to
   warnings with `--no-dim-check` escape hatch.
3. **v0.5.0:** Dimension errors are hard errors. `--no-dim-check` removed.

## Estimated Effort

| Component                                            | Est. LOC  | Notes                         |
| ---------------------------------------------------- | --------- | ----------------------------- |
| `dimcheck.ml` (inference + constraint solving)       | 300       | Core algorithm                |
| `dimcheck.ml` (error messages)                       | 150       | Formatting, hints             |
| Parser/AST changes (`[dim]` annotation syntax)       | 50        | Optional phase 3              |
| IR changes (dimension field on param/table/timefunc) | 30        |                               |
| Serialize/deserialize for dimension field            | 20        |                               |
| Test suite (`test_dimcheck.ml`)                      | 500+      | Extensive, as specified above |
| Golden negative test files                           | 100       | ~10 small .camdl files        |
| Integration in `camdlc` CLI                          | 20        | Flag handling                 |
| **Total**                                            | **~1200** |                               |

## Open Questions

1. **Should `Const(1.0)` be dimensionless or universal?** Currently proposed:
   dimensionless. But `1.0 * Pop("S")` should work (scaling by 1). Answer: bare
   numeric constants are dimensionless; multiplication by dimensionless
   preserves dimension. This is consistent — `3.14 * Pop("S")` has dimension P.

2. **What about `mod(a, b)`?** Proposed: `dim(a) = dim(b)`, result has same
   dimension. This handles `mod(t, 365 'days)` correctly.

3. **Should the checker run on observation likelihood expressions?** Yes —
   `Projected` gets its dimension from the observation's projection type
   (CumulativeFlow → P, CurrentPop → P). The likelihood parameters (mean, sd,
   dispersion) should be checked for consistency with the projected quantity.

4. **What about ODE equations?** `d(compartment)/dt` has dimension `P/T`. The
   derivative expression must match. Same constraint as transition rates.

5. **Interaction with `balance` expressions.** The balance expression computes a
   population count, so it must have dimension P. This is checkable.
