use serde::{Deserialize, Serialize};
use crate::expr::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sinusoidal {
    pub amplitude: Expr,
    pub period:    Expr,
    pub phase:     Expr,
    pub baseline:  Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Piecewise {
    pub breakpoints: Vec<Expr>,
    pub values:      Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interpolated {
    pub times:  Vec<Expr>,
    pub values: Vec<Expr>,
    pub method: InterpMethod,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterpMethod {
    Linear,
    Constant,
    Spline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Periodic {
    pub period: Expr,
    pub values: Vec<Expr>,
}

/// gh#59: finite Fourier series with N estimable cos/sin pairs.
/// `harmonics[k]` is the (a_k, b_k) pair for harmonic k = 1, 2, …
/// (k=0 baseline is the caller's responsibility: `1 + sum_k a_k cos + b_k sin`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fourier {
    pub period:    Expr,
    pub harmonics: Vec<(Expr, Expr)>,
}

/// gh#59: periodic cubic B-spline with K estimable coefficients
/// over fixed knots in [0, period).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodicSpline {
    pub period: Expr,
    pub knots:  Vec<Expr>,
    pub coefs:  Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeFuncKind {
    Sinusoidal(Sinusoidal),
    Piecewise(Piecewise),
    Interpolated(Interpolated),
    Periodic(Periodic),
    Fourier(Fourier),                  // gh#59
    PeriodicSpline(PeriodicSpline),    // gh#59
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeFunction {
    pub name: String,
    pub kind: TimeFuncKind,
    /// Declared dimension from the forcing's tier-3 unit literal
    /// (GH #8): `(P_exp, T_exp)`. E.g. `(0, -1)` for `'per_day`,
    /// `(1, 0)` for `'count`, `(0, 0)` for `'ratio`. Always present —
    /// the parser requires a unit literal on every forcing
    /// declaration, so the dim-checker can use this authoritatively
    /// without falling back on value-based inference.
    pub dim: (i32, i32),
}
