---
status: approved (ship-now)
date: 2026-05-10
target: ship in ~1-2 days — tracks gh#54
related:
  - gh#53 (cohort fire-step misalignment — primary fix)
  - gh#54 (residual investigation — this proposal closes the residual)
  - gh#55 (per-site lint suppression — deferred)
  - gh#56 (CLI lint-policy knobs — deferred)
---

# `dt` primitive + L401 lint

## TL;DR

Add `dt` as a first-class expression in the camdl DSL (alongside the
existing `t`, `t_start`, `t_end` time primitives), so model authors
can write the dt-aware Euler-multinomial discretization that pomp's
csnippets use. Add a single lint pass (L401) that catches the common
misuse pattern — a fixed time literal where `dt` was meant.

This closes the gh#54 residual investigation: the +12-22 nat
camdl-vs-pomp loglik difference at sub-day dt traced to the He2010
model file's `let beta_base = R0 * (1 - exp(-(γ+μ) * 1 'days))`,
which pinned the discretization correction at dt=1 day. With
`dt` available as a primitive and the lint warning users away from
fixed-time-literal pinning, the same model becomes
`let beta_base = R0 * (1 - exp(-(γ+μ) * dt)) / dt` — exactly pomp's
formula, dt-invariant in effective R0.

## Motivation

The gh#53 cohort fire-step fix accounted for 99.5% of the
camdl-vs-pomp Richardson-ladder divergence on the canonical He et al.
2010 measles benchmark. A residual ~12-22 nat seed-stable bias
remained. Bisection at matched noise (gh#54 investigation, 2026-05-09
to 2026-05-10) traced it to the model file's discretization-pinning:

| dt | discrete-β (orig) | cts-β (continuous-time test) | pomp |
|---:|---:|---:|---:|
| 1.000 | -5815.82 | -5812.69 | -5811.52 |
| 0.500 | -5788.53 | -5799.82 | -5800.81 |
| 0.250 | -5788.91 | -5798.80 | -5805.25 |
| 0.125 | -5787.12 | -5795.48 | -5789.32 |

Original camdl: 14/14 sub-day cells positive (across two seed regimes),
seed-stable +12 to +22 nat bias. With continuous-time β (no
discretization pinning): mixed signs, all magnitudes ≤ 6 nats — within
PF SE. The discretization-pinning was the residual carrier.

Pomp's csnippet handles this correctly:
```c
beta = R0 * seas * (1.0 - exp(-(gamma+mu)*dt)) / dt;
```
The `dt` is the runtime integrator step — pomp's `rprocess` Csnippet
sees it via the API. Camdl's DSL has no equivalent; the He2010 model
author hardcoded `1 'days` as a workaround, valid only at dt=1 day.

This proposal adds the missing primitive and a lint to catch the
misuse pattern. Future model authors get the right tool; existing
models get a clean diagnostic pointing at the fix.

## Surface

### `dt` primitive

A bare identifier `dt` resolves to the runtime integrator step, in
the model's `time_unit`. Has dimension `T`. Available anywhere a
rate or time expression is valid (transitions, `let` bindings,
balance constraints, observation likelihoods, time-function
arguments).

```camdl
# Pomp's standard Euler-multinomial calibration:
let beta_base = R0 * (1.0 - exp(-(gamma + mu) * dt)) / dt

# Stochastic noise variance scaling:
let noise_increment = sigma * sqrt(dt)

# Diagnostic / debug output:
@ printf("integrator step: %.4f days", dt)  # if camdl ever gets printf
```

`dt` is a *runtime* value — its lowering is `Ir.Dt`, which the runtime
evaluator reads from the simulator config (`SMCConfig.dt` /
`ChainBinomialConfig.dt`). It does NOT participate in compile-time
constant folding — the IR stores `Dt`, not a baked numeric value.

### L401 lint

Fires when the AST contains the Euler-multinomial discretization-
correction pattern using a fixed time literal instead of `dt`:

```
warning[L401]: discretization-correction pattern uses fixed time literal `1 'days`
  ┌─ he2010_london.camdl:71:38
  │
71│ let beta_base = R0 * (1.0 - exp(-(gamma + mu) * 1 'days))
  │                                                ^^^^^^^^
  │
  = note: the `(1 - exp(-rate * τ))` shape is the Euler-multinomial
          per-step transition probability. Pinning τ to a fixed time
          literal makes the model correct only when the runtime
          integrator step (`config.dt`) equals that literal. Any
          other dt produces a discretization-pinned bias (gh#53/gh#54).
  = hint: use the `dt` primitive instead — `(1 - exp(-(γ+μ)*dt))/dt`
          gives the dt-aware β formulation that pomp's csnippet uses
          and is dt-invariant in effective R0.
```

False-positive minimization: the lint fires only when the AST shape
matches all of:
- Inside `exp(...)` argument
- On the negation side (matching `-RATE * TIME_LITERAL`)
- The TIME_LITERAL is a constant time-typed value (`EUnit (_, time_unit)` or `EConst _` with dimension T)
- The RATE is rate-typed (dimension `T^-1`)

This is the unambiguous Euler-correction template. Pure unit
conversions (e.g. `mu / 1 'years` for converting per-year rate to
per-day) don't match because they're outside `exp(...)`. Half-life
computations don't match because they don't have the `RATE *
TIME_LITERAL` shape inside exp.

## Architecture

### IR

**OCaml `ocaml/lib/ir/ir.ml`**: add `| Dt` to the `expr` ADT
alongside `Time`. ~3 LOC plus serde round-trip.

**Rust `rust/crates/ir/src/expr.rs`**: mirror with `Expr::Dt`. Update
the schema doc + serde derive. ~3 LOC plus golden file regeneration.

### Compiler (OCaml)

**`expander.ml`**:
- Add `"dt"` to `reserved_time_names` (line 382 currently
  `["t"; "t_start"; "t_end"]`).
- In the identifier-resolution branch (currently line 1235ish where
  `name = "t"` resolves to `Ir.Time`), add `name = "dt"` → `Ir.Dt`.
- Dimension: `T` (1 in time, 0 in population).
- L401 lint pass: a small AST visitor that walks all expressions in
  let bindings, transitions, balance constraints, etc., and matches
  the `exp(neg(mul(rate_expr, time_literal)))` shape. Emits the
  warning at the time-literal's source location.

### Runtime (Rust)

**`propensity.rs::EvalCtx`**: already carries `t: f64`. Add `dt:
f64`. Threaded through every call site that already constructs an
`EvalCtx` (chain_binomial, tau_leap, ode, gillespie, balance,
intervention). Each backend already has `cfg.dt` in scope when
constructing the ctx; just pass it.

**`propensity.rs::eval_expr`**: add `Expr::Dt => ctx.dt` arm. Same
shape as the existing `Expr::Time => ctx.t` arm.

### Lint catalog

New file `docs/dev/warning-catalog.md` listing every existing
`Wxxx`/`Lxxx` code with one-paragraph description + rationale. Seed
with the ~10 existing codes plus the new L401. Going forward, any
new warning emit-site must add a one-line entry to the catalog
(enforced by review, not the compiler).

## Implementation plan (commit order)

1. **Proposal + warning catalog skeleton** (this commit + the
   catalog markdown). Ship as a single planning commit.
2. **OCaml IR `Ir.Dt`** + serde + golden round-trip test.
3. **Rust IR `Expr::Dt`** + serde + schema docs + golden regen.
4. **OCaml expander resolves `dt`** + dim-check + golden compile test.
5. **Rust evaluator threads `dt` through `EvalCtx`** + unit test that
   `dt` evaluates to the runtime cfg.dt at substep level.
6. **L401 lint pass** + unit tests covering the bug shape, the
   fixed-form (cleared), and false-positive cases (unit conversions,
   half-life math).
7. **He2010 model file fix**: replace `1 'days` with `dt` in
   camdl-book's `models/he2010_london.camdl`. Cross-repo;
   coordinate with camdl-book agent. Refit at the new model to
   recalibrate θ̂; published lit MLE values get a small adjustment.

Total ~120 LOC across IR + OCaml + Rust + docs + tests. Plus the
camdl-book follow-up which is one model-file edit + a re-fit.

## Out of scope (filed as gh#55, gh#56)

**gh#55 — Per-site lint suppression syntax.** A `#[allow(L401)]`-
style mechanism so users can silence specific lints at specific
source locations. Design discussion: attribute-on-block vs comment-
prefix vs config-block. Defer until ≥ 3 lints have users wanting
suppression — currently we have ~10 warning codes total and zero
documented suppression requests.

**gh#56 — CLI lint-policy knobs (`--allow`, `--deny`, `-Werror`).**
Depends on gh#55's suppression syntax for `--allow` semantics. v2/v3
of this work.

Both follow-ups are documented in the warning-catalog markdown so
the path forward is discoverable.

## Tests worth adding

- **OCaml golden test**: a model with `dt` in a let binding compiles
  and produces an IR with the `Dt` variant in the right place.
- **Rust unit test**: at runtime, `Expr::Dt` evaluates to
  `ctx.dt`, and changing `cfg.dt` between sim runs gives different
  values without recompiling the IR.
- **OCaml lint test (positive)**: `let x = 1 - exp(-(gamma+mu)*1 'days)`
  triggers L401 with the right hint.
- **OCaml lint test (negative)**: `let mu_per_day = mu_per_year / 1 'years`
  does NOT trigger (unit conversion, outside exp).
- **OCaml lint test (negative)**: `let half_life = ln(2) / lambda` does
  NOT trigger (no time literal inside exp).
- **End-to-end**: He2010 model with `1 'days` triggers L401; same
  model with `dt` doesn't; the dt-aware version's Richardson ladder
  (post-fix) lands within ~5 nats of pomp at every dt.

## v1 ship status

- [x] OCaml IR `Ir.Dt`               — c5fb2d7
- [x] Rust IR `Expr::Dt`             — eb565f1
- [x] OCaml expander resolves `dt`   — c5fb2d7
- [x] Rust evaluator                 — eb565f1
- [x] L401 lint                      — 6b7b61c
- [x] Warning catalog                — 606f08b
- [x] Round-trip + runtime tests     — 5033f63
- [x] gh#55 (per-site suppression) filed
- [x] gh#56 (CLI lint-policy knobs) filed
- [ ] He2010 model file fix (cross-repo to camdl-book)
      — owned by the camdl-book agent. Four model files use the
        `let beta_base = R0 * (1.0 - exp(-(gamma + mu) * 1 'days))`
        pattern: `vignettes/he2010-paper`, `he2010-pomp`, `he2010`,
        `he2010-v0`. The L401 lint will fire on each. Replace with the
        canonical form (note the explicit `/dt` on the multiplier
        side and the post-correction continuous-time normalization;
        see warning-catalog.md §L401):
        `let beta_base = R0 * (gamma + mu)`
        and absorb `(1.0 - exp(-(gamma+mu)*dt)) / ((gamma+mu)*dt)`
        into the seasonal forcing or transition rate as needed.
        Pomp's csnippet uses the equivalent form internally.

## Empirical verification

End-to-end smoke test with chain_binomial backend (commit eb565f1):

| dt    | Final S | Final I | Final R |
|-------|---------|---------|---------|
| 1.0   | 45      | 16      | 939     |
| 0.5   | 63      | 16      | 921     |

Different trajectories at different dt is expected — the chain-binomial
discrete-time correction `(1 - exp(-γ·dt))/dt` evaluates to different
effective rates per substep at different dt. The point is not byte-
identity but that **the formula uses the runtime dt**, so paired-seed
CRN scenarios (and pomp comparisons) keep the discretization aligned.
Without the primitive, users had to hardcode a literal time, which only
works at one specific dt.

## Forward-looking notes

The deeper architectural question — *why doesn't the language already
have a built-in for the standard Euler-multinomial discretization?* —
is intentionally deferred. The math `(1 - exp(-rate*dt))/dt` is too
short to benefit from an abstracted name (no semantic compression,
hard to name well — `euler_correction`, `discretize_rate`,
`step_invariant` all have flaws). With the `dt` primitive available
and L401 catching the misuse pattern, the explicit form is what
users write; if a clear pattern emerges across multiple models in
6 months, we can revisit. Don't pre-abstract.
