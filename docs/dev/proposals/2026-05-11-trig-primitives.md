---
title: Trig primitives — sin, cos, tanh + pi, e
date: 2026-05-11
issue: gh#58
status: drafted
---

# Trig primitives: `sin`, `cos`, `tanh` + `pi`, `e`

## TL;DR

The IR's `UnOp` enum lacks trig functions: `{Neg, Exp, Log, Sqrt,
Abs, Floor, Ceil}` only. There's no principled reason — the IR's
purity/totality/first-order invariants are preserved by trig. Add
`Sin`, `Cos`, `Tanh` as siblings of `Exp`/`Log`, and recognize `pi`
and `e` as reserved identifiers that desugar to `Ir.Const`. ~10
sites per language, fully mechanical.

This unblocks inline Fourier forcings:

```camdl
let seasonal = 1 + a1*cos(2*pi*t/period) + b1*sin(2*pi*t/period)
                 + a2*cos(4*pi*t/period) + b2*sin(4*pi*t/period)

transitions {
  infection : S --> I @ beta0 * seasonal * S * I / N
}
```

(With `a1, b1, a2, b2` estimable parameters, period a model constant.)
The motivating use case is the King 2008 cholera model comparison
chapter in camdl-book — 2-harmonic Fourier seasonality with
estimable amplitudes.

## Why this isn't already there

The original design pushed users toward the high-level `sinusoidal
{}` forcing block, which wraps one harmonic with a non-negative
baseline guarantee. That's fine for "one annual harmonic, simple
case" but doesn't compose: N-harmonic Fourier means N `sinusoidal`
forcings summed inline, which is verbose and hides the structure.

Trig is total over ℝ, so the only real constraint is the
rate-non-negativity invariant, which is already enforced by the
runtime (`rate < 0 → propensity = 0`) and is the user's
responsibility for any composite rate expression. `Sin`/`Cos`/`Tanh`
don't change that.

## Design (types-first)

### `UnOp` additions

```ocaml
(* ocaml/lib/ir/ir.ml *)
type un_op = Neg | Exp | Log | Sqrt | Abs | Floor | Ceil
           | Sin | Cos | Tanh
```

```rust
// rust/crates/ir/src/expr.rs
pub enum UnOp {
    Neg, Exp, Log, Sqrt, Abs, Floor, Ceil,
    Sin, Cos, Tanh,
}
```

### Dimensional rules

Argument must be dimensionless (`(P, T) = (0, 0)`); result is
dimensionless. Same rule as `Exp`, `Log`. Concretely:

| Expression                        | dim(arg) | accepted? |
|-----------------------------------|----------|-----------|
| `cos(t)`                          | T        | ✗ E300    |
| `cos(2*pi*t/period)`              | 0        | ✓         |
| `tanh(R0)`                        | 0        | ✓         |
| `tanh(beta)`                      | T⁻¹      | ✗ E300    |

The dim-check rule slots into the existing `infer_unop_dim` switch
in `dimcheck.ml` next to `Exp`/`Log`/`Sqrt`.

### Autodiff (autodiff.ml)

```
∂sin(x)/∂θ  = cos(x) · ∂x/∂θ
∂cos(x)/∂θ  = -sin(x) · ∂x/∂θ
∂tanh(x)/∂θ = (1 - tanh(x)²) · ∂x/∂θ
```

The `tanh` derivative uses the `1 - tanh²` form rather than `sech²`
to avoid introducing a new builtin. Compiler-emitted `rate_grad`
fields use these formulae.

### Const-folding (autodiff.ml simplify pass)

`Sin(Const c) → Const (sin c)`, same for Cos/Tanh. Mirrors existing
`Exp(Const c) → Const (exp c)` etc.

### Reserved identifiers: `pi`, `e`

`expander.ml` resolves these names at expansion time:

```ocaml
else if name = "pi" then Ir.Const Float.pi
else if name = "e"  then Ir.Const (Float.exp 1.0)
```

No new IR variants — they're just constants. Same pattern as
`t_start`, `t_end`, `dt`. Update `reserved_time_names` (or a
sibling list) so the expander doesn't try to look them up as
compartments/params.

### Runtime evaluator

```rust
UnOp::Sin  => a.sin(),
UnOp::Cos  => a.cos(),
UnOp::Tanh => a.tanh(),
```

In `eval_expr` and `eval_expr_deriv` (propensity.rs), and the
mirror in `resolved_expr.rs`. NaN-safe — `f64::sin`/`cos`/`tanh`
never return NaN for finite input.

## Implementation plan

| # | Step | Files | Estimate |
|---|------|-------|----------|
| 1 | Proposal + gh issue | this file, gh#58 | 10 min |
| 2 | OCaml: IR + serde + validate + autodiff + dimcheck + expander parser + pi/e + pp + inspect + ast | ~9 files | 40 min |
| 3 | Rust: UnOp + validate + propensity + resolved_expr + compiled_model + hierarchical | ~6 files | 30 min |
| 4 | Tests: serde, dimcheck, autodiff (vs finite diff), evaluator at known points, end-to-end Fourier smoke | test_compiler.ml, expr_eval.rs | 30 min |

Total: ~2 hours of mechanical work.

## Tests

### Unit

- **Serde round-trip**: `UnOp::Sin`, `Cos`, `Tanh` round-trip through
  JSON without loss.
- **Evaluator known points**: `sin(0) = 0`, `sin(pi/2) = 1`,
  `cos(0) = 1`, `cos(pi) = -1`, `tanh(0) = 0`, `tanh(∞) ≈ 1`.
- **Autodiff vs finite difference**: pick `f(beta) = sin(beta * 5)`,
  compare compiler-emitted `rate_grad` against `(f(β+h) - f(β-h))/2h`
  for several β values. Same for cos and tanh.
- **Const-folding**: `sin(0)` simplifies to `Const 0.0`.
- **`pi`, `e` resolution**: parse `pi`, get `Ir.Const ≈ 3.14159`;
  parse `e`, get `Ir.Const ≈ 2.71828`.

### Dim-check

- `cos(t)` with `t : time` → E300 with helpful hint.
- `cos(2*pi*t/period)` with `period : time` → accepted (ratio is
  dimensionless).
- `tanh(R0)` where R0 is unitless → accepted.

### End-to-end smoke

- 2-harmonic Fourier SEIR model compiles to IR, simulates with
  chain_binomial backend, produces non-pathological trajectory
  (no NaN, finite final counts).
- Same model, PGAS gradient evaluator runs without NaN.

## Out of scope (future)

- **Inverse trig** (`asin`, `acos`, `atan`, `atan2`). Add when a
  model needs them.
- **Hyperbolic** (`sinh`, `cosh`). `tanh` is by far the most useful
  one; the others can wait.
- **Numerical-stability variants** (`expm1`, `log1p`). Useful but
  not blocking anything; YAGNI for now.
- **Periodic-basis forcings** (`fourier`, `periodic_spline`).
  Separate proposal (PR B / gh follow-up); depends on this landing
  first because it'll use these primitives internally.

## v1 ship status

(filled in after each commit)

- [ ] OCaml IR `Sin`/`Cos`/`Tanh`
- [ ] OCaml `pi`/`e` reserved identifiers
- [ ] Rust IR `Sin`/`Cos`/`Tanh`
- [ ] Runtime evaluator + autodiff (both sides)
- [ ] Dim-check rules
- [ ] Tests
- [ ] End-to-end smoke (Fourier forcing)
