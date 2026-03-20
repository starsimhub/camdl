use sha2::{Sha256, Digest};
use std::collections::HashMap;

const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// SHA-256 of the raw IR JSON bytes, hex-encoded.
pub fn model_hash(ir_json: &str) -> String {
    let mut h = Sha256::new();
    h.update(ir_json.as_bytes());
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
