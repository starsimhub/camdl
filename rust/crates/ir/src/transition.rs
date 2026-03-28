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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub name:           String,
    pub stoichiometry:  Vec<StoichiometryEntry>,
    pub rate:           Expr,
    pub event_key:      Option<String>,
    pub metadata:       Option<TransitionMetadata>,
    /// Extra-demographic stochasticity: σ²_SE for Gamma-Poisson (NegBinomial)
    /// draws in tau-leap / chain-binomial backends.  `None` → standard Poisson.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overdispersion: Option<Expr>,
}
