---
title: Periodic B-spline forcing — proper algorithm, not the v1 hack
date: 2026-05-12
issue: gh#59 (follow-up)
supersedes: v1 PeriodicSpline evaluator shipped in commit ff7f8cc
status: drafted
---

# Periodic cubic B-spline forcing: proper algorithm

## Why this proposal exists

gh#59 shipped a v1 `PeriodicSpline` evaluator that wraps `t` into
`[knot0, knot0 + period)` and delegates to a natural cubic spline
of the period-extended knot/coef table. This was a shortcut.

The problems:

1. **It's not the same model**, not just a different numerical
   approximation. The natural cubic spline interprets `coefs` as
   *interpolated values at knots*; a true periodic B-spline basis
   interprets `coefs` as *basis weights*. These parameterize the
   same function space, but the same coefficient vector means
   different curves.
2. **It silently disagrees** with every standard implementation
   (de Boor's textbook, scipy's `BSpline`, R's `pbs::pbs`,
   pomp's `periodic.bspline.basis`) at the C² boundary. The
   discrepancy is small (~10⁻⁴ relative) but unprincipled.
3. **It cannot be cross-validated** against any external reference
   because no external reference computes what it computes.
4. **camdl's quality bar is "match the proper algorithm"** (see
   CLAUDE.md "Correct before clean"). The v1 was correct-ish, not
   correct.

Fix: implement the textbook algorithm, parameterize the surface to
match the canonical periodic-B-spline literature, validate against
two independent external oracles (scipy and pomp), commit reference
fixtures so CI catches drift.

## What we are *not* doing

We are not copying pomp's C code. pomp is GPL-3+; copying its
implementation would force camdl's license to GPL. The algorithm
(de Boor's recurrence + periodic wrap-fold + a centering shift) is
not novel to pomp — it's in primary sources predating pomp by
decades. We implement from those primary sources, never with
pomp's source open. We compare *numerical outputs* against pomp's
matrix (and scipy's) as a sanity oracle; that's standard
black-box validation, not copying.

## Primary sources (algorithm, not code)

- **de Boor (1978) *A Practical Guide to Splines* (Springer)** §X.
  The B-spline recurrence (eq. X.5) and the periodic wrap-fold
  construction (§X.5 "Periodic splines").
- **Press, Teukolsky, Vetterling, Flannery (2007) *Numerical
  Recipes* 3rd ed §3.3.1** "Cubic B-spline interpolation". De Boor's
  recurrence written in NR-style C.
- **Eilers & Marx (1996) "Flexible smoothing with B-splines and
  penalties" Statistical Science 11(2):89–121.** §2 covers the
  periodic-spline construction with the same wrap-fold this proposal
  uses.
- **Wand & Ormerod (2008) "On semiparametric regression with
  O'Sullivan penalised splines" Australian & N.Z. J. Statistics
  50(2):179–198.** §3 documents the `(degree-1)/2` centering shift
  used here, motivated by interpretability of basis function indices.

## Algorithm

Given `period > 0`, `degree ≥ 0` (default 3 = cubic), `n_basis > degree`,
and a coefficient vector `c = (c_0, …, c_{n_basis − 1})`, the periodic
B-spline forcing evaluates as:

```
def eval(t):
    # Uniform knots over [0, period) with `degree` wrap-pad on each side
    dx = period / n_basis
    knots = [k * dx for k in range(-degree, n_basis + degree + 1)]

    # Wrap t into [0, period)
    x = (t mod period)

    # Standard de Boor: evaluate nbasis + 2*degree + 1 non-periodic basis
    # functions at x using the recurrence
    #   B_i^0(x) = 1{knots[i] ≤ x < knots[i+1]}
    #   B_i^k(x) = ((x − knots[i]) / (knots[i+k] − knots[i])) · B_i^{k-1}(x)
    #            + ((knots[i+k+1] − x) / (knots[i+k+1] − knots[i+1])) · B_{i+1}^{k-1}(x)
    n_eval = n_basis + 2 * degree + 1
    yy = [bspline_recurrence(x, i, degree, knots) for i in range(n_eval)]

    # Periodic wrap-fold: the last `degree` basis functions extend past
    # `period`; fold them back onto the first `degree`. This makes the
    # basis truly periodic and preserves partition-of-unity (∑_i B_i(t) = 1).
    for k in range(degree):
        yy[k] += yy[n_basis + k]

    # Centering shift: rotate output so basis 0 is centered at t=0
    # rather than at t = −(degree+1)/2 · dx (which is where it lives
    # in the unshifted index convention). For cubic, shift = 1.
    shift = (degree - 1) // 2
    basis = [yy[(shift + k) % n_basis] for k in range(n_basis)]

    # Linear combination
    return sum(c[k] * basis[k] for k in range(n_basis))
```

This is the standard construction from the references above. The
wrap-fold step is in de Boor §X.5 and Eilers & Marx §2; the
centering shift is in Wand & Ormerod §3.

## Why uniform knots (not user-specified)

Three reasons:

1. **King 2008 uses uniform knots.** The cholera comparison
   chapter needs to reproduce his published coefficients; arbitrary
   knot placement would require an additional knot vector in the
   `fit.toml`, which King doesn't publish.
2. **Standard P-spline practice** (Eilers & Marx 1996, Wood 2017)
   uses uniform knots; non-uniform placement is rare and usually
   requires penalty-matrix tuning that's outside our scope.
3. **`docs/CLAUDE.md` "don't design for hypothetical future
   requirements"** — no user has asked for non-uniform knots. Add
   them when someone does.

This is strictly less general than my v1 IR surface, which had
`knots: expr list`. We replace with `n_basis: int`.

## IR shape (replaces gh#59 v1)

```ocaml
(* ocaml/lib/ir/ir.ml *)
type periodic_spline = {
  period:  expr;
  n_basis: int;       (* number of basis functions; must be > degree *)
  degree:  int;       (* default 3 (cubic) *)
  coefs:   expr list; (* length must equal n_basis *)
}
```

```rust
// rust/crates/ir/src/time_func.rs
pub struct PeriodicSpline {
    pub period:  Expr,
    pub n_basis: u32,
    pub degree:  u32,   // default 3
    pub coefs:   Vec<Expr>,
}
```

DSL surface:

```camdl
forcing {
  seas : periodic_spline 'ratio {
    period  = 365.25 'days
    n_basis = 6
    degree  = 3
    coefs   = [c1, c2, c3, c4, c5, c6]
  }
}
```

If `n_basis ≤ degree` or `coefs.len() ≠ n_basis`, compile fails
with a specific diagnostic.

## Basis-indexing convention

We adopt the `(degree-1)/2` centering shift, which means:
- For cubic (`degree = 3`), basis 0 is centered at `t = 0`.
- Coefficient `c_0` controls the curve value near the start of
  each period.
- This matches the convention King 2008 implicitly uses (via pomp).

scipy's `BSpline` with a periodic knot vector does *not* apply this
shift; the same coefficient vector would produce a curve shifted
by `(degree-1)/2` knot positions. We document this in the
cross-validation test (we apply an explicit `roll` to scipy's
output before comparing).

This is a labeling choice, not an algorithmic one. The function
space spanned is identical.

## Validation strategy

Two independent external oracles, both via reference TSVs committed
to the repo so CI is offline-safe:

### 1. scipy oracle

```python
# scripts/gen_periodic_bspline_scipy_fixture.py
import numpy as np
from scipy.interpolate import BSpline

period, n_basis, degree = 365.25, 6, 3
dx = period / n_basis
knots = np.arange(-degree, n_basis + degree + 1) * dx
coefs = np.array([0.7, 1.2, 0.9, 0.5, 1.1, 0.8])

# Extend coefs cyclically and roll for centering-shift convention
shift = (degree - 1) // 2
coefs_extended = np.concatenate([coefs, coefs[:degree]])
coefs_rolled = np.roll(coefs_extended, -shift)

spline = BSpline(knots, coefs_rolled, degree)
ts = np.linspace(0, period, 1000, endpoint=False)
ys = spline(ts % period)

np.savetxt("tests/fixtures/periodic_bspline_scipy.tsv",
           np.column_stack([ts, ys]),
           header="t\ty", comments="", delimiter="\t")
```

Camdl's Rust evaluator loads this TSV and asserts `|camdl - scipy| <
1e-12` for every row.

### 2. pomp oracle

```r
# scripts/gen_periodic_bspline_pomp_fixture.R
library(pomp)
period <- 365.25
n_basis <- 6
ts <- seq(0, period, length.out = 1001)[-1001]
coefs <- c(0.7, 1.2, 0.9, 0.5, 1.1, 0.8)

# basis is 1000 x n_basis
basis <- periodic.bspline.basis(ts, nbasis = n_basis,
                                 degree = 3, period = period)
ys <- as.vector(basis %*% coefs)

write.table(data.frame(t = ts, y = ys),
            "tests/fixtures/periodic_bspline_pomp.tsv",
            sep = "\t", quote = FALSE, row.names = FALSE)
```

Same Rust test asserts `|camdl - pomp| < 1e-12`.

**If both fixtures agree with camdl, we have independent verification
from two ecosystems that don't share code paths.** If they ever drift
apart from each other, we'll learn about it from CI — either we
introduced a bug, or one of the libraries changed convention.

The fixture-generation scripts live in `scripts/` and run only when
regenerating; CI doesn't depend on R or scipy being installed.

## Fourier — same treatment, simpler math

The Fourier evaluator from gh#59 is mathematically direct
(`Σ_k a_k cos(2π k t/T) + b_k sin(...)`) but I never cross-validated
against an external oracle. Adding:

```python
# scripts/gen_fourier_numpy_fixture.py
import numpy as np
period = 365.25
ts = np.linspace(0, period, 1000, endpoint=False)
harmonics = [(0.2, 0.1), (0.05, 0.05)]
ys = sum(a * np.cos(2*np.pi*(k+1)*ts/period)
       + b * np.sin(2*np.pi*(k+1)*ts/period)
         for k, (a, b) in enumerate(harmonics))
np.savetxt("tests/fixtures/fourier_numpy.tsv",
           np.column_stack([ts, ys]),
           header="t\ty", comments="", delimiter="\t")
```

Test asserts `|camdl - numpy| < 1e-12`.

## Implementation plan

1. **Schema change**: `PeriodicSpline { knots; coefs }` →
   `PeriodicSpline { n_basis; degree; coefs }`. OCaml + Rust IR
   atomically; serde; expander parser; dimcheck (no change — coefs
   still carry forcing dim). ~30 min.
2. **Rust evaluator**: de Boor recurrence + periodic wrap-fold +
   centering shift. ~60 LOC in a new `crates/sim/src/periodic_bspline.rs`
   module, with the recurrence as a pure function on `(t, period,
   n_basis, degree, coefs) → f64`. Replace the v1 hack call site in
   `propensity.rs::eval_time_func`. ~1 hour.
3. **Compile-time evaluation**: `CompiledTimeFuncKind::PeriodicSpline
   { period, n_basis, degree, coefs }` (no precomputed spline table;
   per-call evaluation cost is O(n_basis · degree) ≈ 18 flops for
   K=6 cubic, dwarfed by the surrounding propensity evaluation).
   ~10 min.
4. **Fixture-gen scripts** (`scripts/gen_*_fixture.{py,R}`) +
   committed TSV fixtures. ~30 min including script writing.
5. **Cross-validation tests** loading TSVs + asserting agreement.
   ~30 min.
6. **Remove the v1 hack**: delete the period-extended natural cubic
   spline path; it's dead code after step 2.

Total: ~3 hours.

## Acceptance

- [ ] IR schema updated atomically across OCaml + Rust
- [ ] de Boor + wrap-fold + centering-shift evaluator in
      `crates/sim/src/periodic_bspline.rs`
- [ ] Reference TSVs from scipy and pomp committed under
      `tests/fixtures/`
- [ ] Rust tests assert `|camdl - reference| < 1e-12` for both
      oracles on a 1000-point grid
- [ ] Fourier likewise cross-validated against numpy
- [ ] v1 hack (`CompiledTimeFuncKind::PeriodicSpline` with embedded
      `CubicSpline`) removed entirely
- [ ] DSL smoke (King 2008-style 6-coef forcing) compiles and
      simulates a sensible trajectory

## Out of scope

- Non-uniform knots. Add when a user asks.
- Derivatives of the periodic spline (we never read its
  derivative in the simulator — covariates appear in rates as
  values, not d-values).
- Penalty matrices for P-spline smoothing. The penalty lives at
  the *inference* layer (a prior on coef differences), not the
  evaluation layer.
