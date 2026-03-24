use serde::{Deserialize, Serialize};
use crate::expr::Expr;

// ── Projection ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    CumulativeFlow(String),
    CurrentPop(String),
    CurrentPopSum(Vec<String>),
    DerivedExpr(Expr),
}

// ── Likelihood ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoissonLikelihood {
    pub rate: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegBinomialLikelihood {
    pub mean:       Expr,
    pub dispersion: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalLikelihood {
    pub mean: Expr,
    pub sd:   Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinomialLikelihood {
    pub n: Expr,
    pub p: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BetaBinomialLikelihood {
    pub n:     Expr,
    pub alpha: Expr,
    pub beta:  Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BernoulliLikelihood {
    pub p: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Likelihood {
    Poisson(PoissonLikelihood),
    NegBinomial(NegBinomialLikelihood),
    Normal(NormalLikelihood),
    Binomial(BinomialLikelihood),
    BetaBinomial(BetaBinomialLikelihood),
    Bernoulli(BernoulliLikelihood),
}

// ── Observation schedule ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegularSchedule {
    pub start: f64,
    pub step:  f64,
    pub end:   f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSchedule {
    AtTimes(Vec<f64>),
    Regular(RegularSchedule),
    FromData,
}

// ── Observation model ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationModel {
    pub name:        String,
    pub data_stream: String,
    pub schedule:    ObservationSchedule,
    pub projection:  Projection,
    pub likelihood:  Likelihood,
}
