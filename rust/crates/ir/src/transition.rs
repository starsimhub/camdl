use serde::{Deserialize, Serialize};
use crate::expr::Expr;

/// A single `(compartment_name, delta)` stoichiometry entry.
/// Serialises as a two-element JSON array: `["S", -1]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoichiometryEntry(pub String, pub i64);

/// Advisory metadata — the runtime ignores this; it exists for tooling and
/// human readers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionMetadata {
    pub origin_kind:        Option<String>,
    pub source_compartment: Option<String>,
    pub dest_compartment:   Option<String>,
}

/// How event counts are drawn for this transition.
///
/// Rate wrappers (`overdispersed`, `deterministic`) are compiler-recognized
/// forms in the DSL, not general-purpose functions. They are not composable
/// — `overdispersed(deterministic(rate), σ²)` is meaningless and rejected.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawMethod {
    /// Standard Poisson draw: count ~ Poisson(rate × dt).
    /// Default for all transitions.
    #[default]
    Poisson,
    /// Multiplicative Gamma-Poisson (He et al. 2010):
    /// G ~ Gamma(dt/σ², σ²/dt), count ~ Poisson(rate × G × dt).
    /// Var[count] = mean + mean² · σ²/dt (quadratic scaling).
    Overdispersed(Expr),
    /// Deterministic rounding: count = nearbyint(rate × dt).
    /// Used for demographic flows where Poisson noise is unphysical
    /// (e.g., constant immigration into a large population).
    Deterministic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub name:           String,
    pub stoichiometry:  Vec<StoichiometryEntry>,
    pub rate:           Expr,
    pub event_key:      Option<String>,
    pub metadata:       Option<TransitionMetadata>,
    /// How event counts are drawn. Defaults to Poisson.
    #[serde(default, skip_serializing_if = "is_poisson")]
    pub draw_method:    DrawMethod,
    /// ∂rate/∂param for each estimated parameter. Populated by the OCaml
    /// compiler's autodiff pass. Empty if not computed (backward compatible).
    /// Maps parameter name → derivative expression.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub rate_grad:      std::collections::HashMap<String, Expr>,
}

fn is_poisson(m: &DrawMethod) -> bool {
    matches!(m, DrawMethod::Poisson)
}
