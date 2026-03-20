use sha2::{Sha256, Digest};
use std::collections::HashMap;

const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Structural hash of the IR: only fields that affect simulation semantics.
/// Ignores t_end, output config, labels, and other non-structural fields.
/// serde_json's Map is backed by BTreeMap (sorted keys), so serialization is deterministic.
pub fn model_hash(ir_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(ir_json)
        .expect("model_hash: invalid JSON");
    let obj = v.as_object().expect("model_hash: expected object");

    let mut h = Sha256::new();
    let structural_keys = [
        "compartments", "transitions", "parameters", "tables",
        "time_functions", "interventions", "observations",
        "ode_equations", "initial_conditions",
    ];
    for key in &structural_keys {
        if let Some(val) = obj.get(*key) {
            h.update(key.as_bytes());
            h.update(b"\x00");
            h.update(serde_json::to_string(val).unwrap().as_bytes());
            h.update(b"\x00");
        }
    }
    if let Some(val) = obj.get("version") {
        h.update(b"version\x00");
        h.update(serde_json::to_string(val).unwrap().as_bytes());
    }
    hex::encode(h.finalize())
}

/// Hash of model_hash + canonical params string + backend + tool version.
pub fn config_hash(model_hash: &str, params_canonical: &str, backend: &str) -> String {
    let mut h = Sha256::new();
    h.update(model_hash.as_bytes());
    h.update(b"\x00");
    h.update(params_canonical.as_bytes());
    h.update(b"\x00");
    h.update(backend.as_bytes());
    h.update(b"\x00");
    h.update(TOOL_VERSION.as_bytes());
    hex::encode(h.finalize())
}

/// Hash of config_hash + scenario name + seed. This is the cache key for one run.
pub fn input_hash(config_hash: &str, scenario: &str, seed: u64) -> String {
    let mut h = Sha256::new();
    h.update(config_hash.as_bytes());
    h.update(b"\x00");
    h.update(scenario.as_bytes());
    h.update(b"\x00");
    h.update(seed.to_le_bytes());
    hex::encode(h.finalize())
}

/// Serialize a params map to a canonical string (sorted keys).
pub fn canonical_params(params: &HashMap<String, f64>) -> String {
    let mut pairs: Vec<(&String, &f64)> = params.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    pairs.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(";")
}
