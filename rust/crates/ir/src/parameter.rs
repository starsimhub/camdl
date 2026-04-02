use serde::{Deserialize, Serialize};

// ── Prior distributions ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UniformPrior   { pub lower: f64, pub upper: f64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalPrior    { pub mean: f64, pub sd: f64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogNormalPrior { pub mu: f64, pub sigma: f64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HalfNormalPrior { pub sigma: f64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BetaPrior      { pub alpha: f64, pub beta: f64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GammaPrior     { pub shape: f64, pub rate: f64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExponentialPrior { pub rate: f64 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorDist {
    Uniform(UniformPrior),
    Normal(NormalPrior),
    LogNormal(LogNormalPrior),
    HalfNormal(HalfNormalPrior),
    Beta(BetaPrior),
    Gamma(GammaPrior),
    Exponential(ExponentialPrior),
    Fixed(f64),
}

// ── Parameter transform ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    Log,
    Logit,
    Identity,
}

// ── Parameter declaration ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name:          String,
    /// `None` = must be supplied at runtime via --params / --set.
    /// `Some(v)` = value present (either from hand-crafted IR or applied override).
    pub value:         Option<f64>,
    /// Optional `[lo, hi]` constraint. Used by inference; simulation ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds:        Option<(f64, f64)>,
    pub prior:         Option<PriorDist>,
    pub transform:     Option<Transform>,
    pub initial_value: Option<f64>,
    /// DSL parameter type: "rate", "probability", "positive", "count", "real".
    /// Used by inference to choose the default transform (log vs logit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_kind:    Option<String>,
}
