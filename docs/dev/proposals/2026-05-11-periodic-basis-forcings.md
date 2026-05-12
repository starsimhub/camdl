---
title: Periodic-basis forcings — fourier + periodic_spline
date: 2026-05-11
issue: gh#59
depends_on: gh#58 (trig primitives)
status: drafted
---

# Periodic-basis forcings: `fourier` + `periodic_spline`

## TL;DR

Add two new `time_func_kind` variants for estimable periodic
forcings:

- **`fourier`** — finite Fourier series, N estimable cos/sin pairs.
- **`periodic_spline`** — periodic cubic B-spline basis with K
  estimable coefs over fixed knots.

This fills the underpopulated "estimable + periodic" cell in the
forcing-kinds taxonomy (see `docs/dev/forcing-kinds.md`) and
unblocks the King 2008 cholera comparison chapter in camdl-book,
which needs flexible seasonal forcing for bimodal Bengal cholera.

## Motivation

Two narrative beats from the camdl-book cholera chapter want this:

1. **2-harmonic Fourier seasonality** for the M0–M4 model
   comparison. With one harmonic (`sinusoidal {}`) you can't fit
   bimodal patterns; with N inline `sinusoidal {}` declarations
   summed in the rate, the model file becomes hard to read. A
   first-class `fourier {}` makes the spec match the math.
2. **6-coefficient periodic spline** is what King 2008 actually
   uses for β(t) and the reservoir ω(t). If we want to compare
   camdl fits against King's published numbers (not just refit
   from scratch), we need to express the same basis.

Both are estimable: each coefficient is an `expr` that can be a
`Param` reference, so IF2/PGAS/NUTS estimate them like any other
parameter.

## Design

### IR types

```ocaml
(* ocaml/lib/ir/ir.ml *)
type fourier = {
  period:    expr;
  harmonics: (expr * expr) list;  (* [(a_1, b_1); (a_2, b_2); ...] *)
}

type periodic_spline = {
  period: expr;
  knots:  expr list;  (* K values in [0, period); strictly increasing *)
  coefs:  expr list;  (* K values *)
}

type time_func_kind =
  | Sinusoidal     of sinusoidal
  | Piecewise      of piecewise
  | Interpolated   of interpolated
  | Periodic       of periodic
  | Fourier        of fourier
  | PeriodicSpline of periodic_spline
```

```rust
// rust/crates/ir/src/time_func.rs
pub struct FourierSpec {
    pub period:    Expr,
    pub harmonics: Vec<(Expr, Expr)>,
}

pub struct PeriodicSplineSpec {
    pub period: Expr,
    pub knots:  Vec<Expr>,
    pub coefs:  Vec<Expr>,
}

pub enum TimeFuncKind {
    Sinusoidal(SinusoidalSpec),
    Piecewise(PiecewiseSpec),
    Interpolated(InterpolatedSpec),
    Periodic(PeriodicSpec),
    Fourier(FourierSpec),
    PeriodicSpline(PeriodicSplineSpec),
}
```

### DSL surface

```camdl
forcing {
  # 2-harmonic Fourier
  beta_seas : fourier {
    period    = 365.25 'days
    harmonics = [(a1, b1), (a2, b2)]
  }

  # 6-coefficient periodic cubic B-spline
  beta_full : periodic_spline {
    period = 365.25 'days
    knots  = [0, 60, 120, 180, 240, 300]
    coefs  = [c1, c2, c3, c4, c5, c6]
  }
}
```

Coefficients can be Param refs (estimable), Const (fixed), or any
expression — same as other `forcing` field exprs.

### Dimchecking

- `period` has dim T (same as `sinusoidal.period`).
- `harmonics[k]` coefs are dimensionless (Fourier modulators of a
  dimensionless baseline).
- `knots[k]` has dim T.
- `coefs[k]` has the forcing's declared output dim (set by the
  tier-3 unit literal on the `forcing` block).

These slot into the existing `time_function.dim` machinery.

### Compiled-side representation

```rust
pub enum CompiledTimeFuncKind {
    // ... existing ...
    Fourier {
        period_inv: f64,
        // Pairs of (a_k, b_k) for k = 1..N.
        harmonics: Vec<(f64, f64)>,
    },
    PeriodicSpline {
        period_inv: f64,
        // De Boor coefficient table precomputed at compile time.
        knots: Vec<f64>,
        coefs: Vec<f64>,
    },
}
```

### Evaluator

`Fourier`: standard finite-sum. `t_phase = (t mod period) / period`,
then `sum_k (a_k * cos(2π k * t_phase) + b_k * sin(2π k * t_phase))`.
Hot path: cache `cos(2π t_phase)` and `sin(2π t_phase)` and use
recurrence for higher harmonics (Chebyshev-style).

`PeriodicSpline`: cubic B-spline evaluation at `t_phase` using De
Boor's algorithm. For periodic basis, the knot sequence wraps so
splines stay C² at `t_phase = 0`. Standard formula; we can adapt
`splines` crate or hand-roll the O(K) inner loop.

## Auxiliary cleanup

Bundled into the same schema bump (atomic IR-schema change):

1. **Rename `Interpolated.method_` → `method`.** OCaml's trailing
   underscore is a syntax artifact, not a semantic distinction. The
   serde key on the wire stays `method` (one-line fix; was already
   `method` in serde, only the OCaml record field is renamed).
2. **`docs/dev/forcing-kinds.md`** — documents the 2×2 taxonomy:

   |             | Estimable parametric                                   | Fixed / data-driven           |
   |-------------|--------------------------------------------------------|-------------------------------|
   | Periodic    | `sinusoidal`, `fourier`, `periodic_spline`             | `periodic`                    |
   | Aperiodic   | *(none — no obvious need)*                             | `interpolated`, `piecewise`   |

   Plus a short guide on when to pick each.

## Implementation plan

| # | Step | Files | Est |
|---|------|-------|-----|
| 1 | Proposal + gh issue + forcing-kinds.md | this file, gh#59 | 30 min |
| 2 | OCaml IR + serde + dimcheck + expander parser | ir.ml, serde.ml, dimcheck.ml, ast.ml, expander.ml | 1 hr |
| 3 | Rust IR + serde + compile + evaluator | time_func.rs, compiled_model.rs, propensity.rs | 1 hr |
| 4 | Rename method_ → method | ir.ml, serde.ml | 10 min |
| 5 | Tests + smoke fixtures | test_compiler.ml, expr_eval.rs | 30 min |
| 6 | Golden regen | ir/golden/, ir/expected/ | 10 min |

Total: ~3 hours of mostly-mechanical work.

## Tests

### Unit

- Serde round-trip for `Fourier` and `PeriodicSpline` with realistic
  values (period = 365.25, 2-4 harmonics, 6 knots/coefs).
- Evaluator known points:
  - Fourier with all-zero harmonics → 0 everywhere.
  - Fourier with a₁=1, b₁=0 at t=0 → 1; at t=period/4 → 0.
  - PeriodicSpline at t and t+period equal (periodicity).
- Dimcheck rejects `Fourier { period = R0 }` (R0 unitless, not T).

### End-to-end

- 2-harmonic Fourier SEIR with N=10⁵, 365-day sim: compiles,
  simulates, finishes without NaN, exhibits annual cycle.
- 4-knot periodic-spline SEIR: same.
- Both round-trip through golden TSV (run twice with same seed,
  byte-identical).

## Out of scope

- Aperiodic spline bases (use `interpolated { method = "spline" }`).
- Estimable knots (knots fixed at compile time; only coefs
  estimable). Adding estimable knots needs an outer loop, doesn't
  fit the IR's "evaluator reads fixed structure, params change
  values" pattern.
- Complex-valued Fourier coefs (the (a, b) real representation
  spans the same space).

## v1 ship status

(filled after each commit)

- [ ] Forcing-kinds taxonomy doc
- [ ] OCaml IR + serde + dimcheck + expander
- [ ] Rust IR + serde + evaluator
- [ ] `method_` → `method` rename
- [ ] Tests + smoke
- [ ] Golden regen
