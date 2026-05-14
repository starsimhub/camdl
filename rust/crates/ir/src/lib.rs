pub mod envelope;
pub mod expr;
pub mod intervention;
pub mod model;
pub mod observation;
pub mod ode_equation;
pub mod parameter;
pub mod table;
pub mod time_func;
pub mod transition;
pub mod validate;

pub use envelope::{IrEnvelope, IrError, IR_VERSION};
pub use model::Model;

/// Deserialise a `Model` from a JSON string. gh#audit-C8: enforces
/// the `IrEnvelope` wrapper with version handshake. Returns
/// `IrError::VersionMismatch` if the envelope's `ir_version` doesn't
/// match the `IR_VERSION` baked from `ir/VERSION` at compile time.
pub fn from_str(s: &str) -> Result<Model, IrError> {
    let env: IrEnvelope = serde_json::from_str(s)
        .map_err(|e| IrError::Parse(e.to_string()))?;
    env.into_model_checked()
}

/// Deserialise a `Model` from a JSON reader. gh#audit-C8: same
/// envelope check as `from_str`.
pub fn from_reader<R: std::io::Read>(r: R) -> Result<Model, IrError> {
    let env: IrEnvelope = serde_json::from_reader(r)
        .map_err(|e| IrError::Parse(e.to_string()))?;
    env.into_model_checked()
}

/// Serialise a `Model` to a pretty-printed JSON string. gh#audit-C8:
/// emits the envelope wrapper with the current `IR_VERSION` constant
/// (so Rust-emitted IR JSON round-trips through `from_str`).
pub fn to_string_pretty(model: &Model) -> Result<String, serde_json::Error> {
    let env = IrEnvelope::wrap(model.clone(), None);
    serde_json::to_string_pretty(&env)
}
