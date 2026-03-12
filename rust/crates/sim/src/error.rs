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
}
