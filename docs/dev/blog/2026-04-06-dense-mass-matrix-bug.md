---
date: 2026-04-06
status: fixed
commit: f0272a8
---

# Dense mass matrix momentum draw: L^{-T} not L^{-1}

## The bug

The NUTS dense mass matrix implementation drew momentum from the wrong
distribution. For a target with empirical covariance Σ, the mass matrix
is M = Σ^{-1}. Momentum should be p ~ N(0, M) = N(0, Σ^{-1}).

Given L_Σ = Cholesky(Σ), the correct draw is p = L_Σ^{-T} z where
z ~ N(0, I). This gives Cov(p) = L^{-T} L^{-1} = Σ^{-1} = M. ✓

The buggy code used forward substitution: p = L_Σ^{-1} z. This gives
Cov(p) = L^{-1} L^{-T}. For non-diagonal L (i.e., correlated
parameters — the whole point of dense mass matrix), L^{-1} L^{-T} ≠
L^{-T} L^{-1} because matrix multiplication is not commutative.

## Impact

On a 2D Gaussian with correlation r=0.95:

| Metric | Buggy | Fixed |
|--------|-------|-------|
| var[0] | 7.55  | 1.02  |
| var[1] | 8.58  | 1.05  |
| correlation | 1.000 | 0.954 |

The broken implementation inflated variance 8× and locked the
correlation to 1.0, effectively making NUTS move along only one axis
of the rotated coordinate system. This made "dense" mass matrix
WORSE than identity — explaining why downstream testing showed
"no change" from enabling dense mode.

## Why it was subtle

- The diagonal case (L = diag) is unaffected: L^{-1} L^{-T} =
  L^{-T} L^{-1} when L is diagonal. So all diagonal mass matrix
  tests passed.
- The dense case only fails when there are actual correlations.
  The existing NUTS Gaussian test used identity mass matrix, which
  is diagonal. The bug was invisible to existing tests.
- The difference between forward and back substitution is one
  loop direction and one index transposition. Easy to get wrong,
  hard to spot in review.

## The fix

Replace `solve_lower_triangular(L, z)` (forward: L x = z → x = L^{-1} z)
with `solve_upper_triangular_from_lower(L, z)` (back: L^T x = z → x = L^{-T} z).

## Test added

`test_nuts_dense_mass_matrix_correlated`: NUTS on a 2D Gaussian with
r=0.95. Verifies mean, variance, AND correlation of posterior samples
match the target when using a dense mass matrix built from the true
covariance. This test would have caught the bug immediately.

## Lesson

Test the mass matrix on a correlated target, not just a scaled one.
Diagonal-only tests can't catch dense-specific bugs because the
commutative property holds for diagonal matrices. The same principle
applies to any coordinate transform: always test with off-diagonal
structure.
