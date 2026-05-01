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

// ── Hierarchical priors ───────────────────────────────────────────────────────

/// Distribution family for a hierarchical (pooled) prior leaf.
///
/// Mirrors the variants of `PriorDist` except `Fixed` (which has no
/// meaning in a hierarchical context). Serializes to/from the same
/// snake_case strings used in the IR JSON ("uniform", "normal",
/// "log_normal", …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HierarchicalKind {
    Uniform,
    Normal,
    LogNormal,
    HalfNormal,
    Beta,
    Gamma,
    Exponential,
}

impl HierarchicalKind {
    /// Returns the snake_case string used in IR JSON serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Uniform     => "uniform",
            Self::Normal      => "normal",
            Self::LogNormal   => "log_normal",
            Self::HalfNormal  => "half_normal",
            Self::Beta        => "beta",
            Self::Gamma       => "gamma",
            Self::Exponential => "exponential",
        }
    }
}

impl std::fmt::Display for HierarchicalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Hierarchical prior for a leaf parameter in a pooled group (wave 2 /
/// malaria #3).
///
/// A "leaf" is a parameter whose prior references *other parameters*
/// (hyperparameters) rather than being a pure-constant distribution.
/// At inference time the hyperparameters carry their own priors and are
/// sampled jointly with the leaves; at each log-posterior evaluation
/// the `args` expressions are resolved against the current
/// hyperparameter values.
///
/// - `kind` names the distribution family. Typed enum — rejected at
///   IR deserialisation time, not at inference time.
/// - `args` are keyword → expression pairs (e.g. `"mu" → Param("mu_h")`,
///   `"sigma" → Param("sigma_h")`).
/// - `pool_over` names the dimension over which shrinkage is applied
///   (from the DSL `| age` clause). Empty string for scalar leaves
///   with hyperparent references but no pooling dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HierarchicalPrior {
    pub kind:      HierarchicalKind,
    pub args:      std::collections::BTreeMap<String, crate::expr::Expr>,
    pub pool_over: String,
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
    /// Hierarchical prior for leaves in pooled groups; mutually exclusive
    /// with `prior`. Populated by the compiler when a prior's args
    /// reference other parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchical:  Option<HierarchicalPrior>,
    pub transform:     Option<Transform>,
    pub initial_value: Option<f64>,
    /// DSL parameter type: "rate", "probability", "positive", "count", "real".
    /// Used by inference to choose the default transform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_kind:    Option<String>,
    /// Explicit dimension annotation from the DSL `[dim]` syntax.
    /// Two-element array: `[P_exponent, T_exponent]`.
    /// E.g., `[0, -1]` = per-capita rate (T⁻¹), `[1, -1]` = population rate (P·T⁻¹).
    /// `None` = no annotation (dimension inferred by compiler).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_dim:     Option<(i32, i32)>,
}

