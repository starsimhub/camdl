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
    /// DSL parameter type: "rate", "probability", "positive", "count", "real",
    /// "simplex_member". Used by inference to choose the default transform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_kind:    Option<String>,
    /// Explicit dimension annotation from the DSL `[dim]` syntax.
    /// Two-element array: `[P_exponent, T_exponent]`.
    /// E.g., `[0, -1]` = per-capita rate (T⁻¹), `[1, -1]` = population rate (P·T⁻¹).
    /// `None` = no annotation (dimension inferred by compiler).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_dim:     Option<(i32, i32)>,
}

/// A group of parameters with a joint constraint.
/// Currently only "simplex" (softmax/barycentric, sum-to-1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterGroup {
    pub kind: String,
    pub members: Vec<String>,
}
