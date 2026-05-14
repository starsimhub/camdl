#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("config variant does not match simulator: expected {expected}, got {got}")]
    ConfigMismatch { expected: &'static str, got: &'static str },

    #[error("unknown compartment '{0}'")]
    UnknownCompartment(String),

    #[error("unknown parameter '{0}'")]
    UnknownParameter(String),

    #[error("unknown time function '{0}'")]
    UnknownTimeFunction(String),

    #[error("unknown table '{0}'")]
    UnknownTable(String),

    #[error("table lookup error: {0}")]
    TableLookup(String),

    #[error("division by zero in expression at t={0}")]
    DivisionByZero(f64),

    #[error("negative propensity {value} for transition '{transition}' at t={t}")]
    NegativePropensity { transition: String, value: f64, t: f64 },

    #[error("op '{op}' requires {expected} args but got {got}")]
    WrongArgCount { op: String, expected: usize, got: usize },

    #[error("unknown op '{0}'")]
    UnknownOp(String),

    #[error("model validation error: {0}")]
    Validation(String),

    #[error("absorbing state: total propensity is zero at t={0}")]
    AbsorbingState(f64),

    /// gh#audit-C6 / S1. Expression evaluation hit a numerically
    /// degenerate path. Previously `eval_expr` silently returned 0.0
    /// (wrapped in Ok(_)), masking malformed rate expressions: a small
    /// patch that empties at runtime would produce silent zero
    /// force-of-infection rather than an error. The CLI surfaces this
    /// via `--allow-degenerate-rates` if the user has a defensible
    /// reason (e.g. force-of-infection legitimately undefined when
    /// N=0); default is hard error.
    #[error("numerical collapse ({kind:?}) in rate expression at t={t}")]
    NumericalCollapse { kind: CollapseKind, t: f64 },

    /// gh#audit-C5. Compartment count went below zero. Two distinct
    /// causes: BinomialOvershoot (rate·dt → 1 in chain-binomial split,
    /// transient under inference exploration) vs InterventionAddNegative
    /// (config bug: an Action::Add expression resolved to a negative
    /// value). Previously silently clamped to 0, making the population
    /// non-conservative. Inference layers catch BinomialOvershoot and
    /// convert to −Inf log-likelihood for the offending particle;
    /// forward-sim CLI propagates as a user-facing error.
    #[error("compartment '{compartment}' would go to {attempted_value} (cause: {cause:?}) at t={t}")]
    NegativeCount {
        compartment: String,
        attempted_value: i64,
        t: f64,
        cause: NegativeCountCause,
    },
}

/// gh#audit-C6 / S1. Specific numerical-degeneracy mode that
/// produced a NumericalCollapse. Lets the caller distinguish
/// "div by zero" (often a population-zero edge case) from
/// "Pow NaN" (often a domain bug — negative base to fractional
/// power) for actionable error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollapseKind {
    DivByZero,
    PowNanInf,
    UnOpNan,
    SqrtNegative,
    ModByZero,
}

/// gh#audit-C5. Cause discriminator for NegativeCount.
/// BinomialOvershoot is expected during inference exploration and
/// gets caught by the inference layer; InterventionAddNegative is
/// always a config bug and propagates regardless of caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegativeCountCause {
    BinomialOvershoot,
    InterventionAddNegative,
}

impl SimError {
    /// gh#audit-C5 / C6. True when the error represents a
    /// per-particle numerical degeneracy that the inference layer
    /// should catch and convert to a −Inf log-likelihood (killing the
    /// offending particle in resampling) rather than tearing down the
    /// whole filter run.
    ///
    /// Recoverable: NumericalCollapse (DivByZero, PowNanInf, UnOpNan,
    /// SqrtNegative, ModByZero) and NegativeCount with cause
    /// BinomialOvershoot — these arise from particles exploring
    /// extreme parameter regions.
    ///
    /// Not recoverable: structural errors (UnknownCompartment,
    /// UnknownParameter, ConfigMismatch, …), AbsorbingState (model-
    /// level absorbing condition, not particle-specific), and
    /// NegativeCount{InterventionAddNegative} (config bug).
    pub fn is_per_particle_recoverable(&self) -> bool {
        matches!(
            self,
            SimError::NumericalCollapse { .. }
            | SimError::NegativeCount { cause: NegativeCountCause::BinomialOvershoot, .. }
        )
    }
}
