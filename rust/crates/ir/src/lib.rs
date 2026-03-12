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

pub use model::Model;

/// Deserialise a `Model` from a JSON string.
pub fn from_str(s: &str) -> Result<Model, serde_json::Error> {
    serde_json::from_str(s)
}

/// Deserialise a `Model` from a JSON reader.
pub fn from_reader<R: std::io::Read>(r: R) -> Result<Model, serde_json::Error> {
    serde_json::from_reader(r)
}

/// Serialise a `Model` to a pretty-printed JSON string.
pub fn to_string_pretty(model: &Model) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(model)
}
