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

/// Hash of the shared simulation configuration: model + base params + backend + dt + tool version.
/// dt is always included even for backends that ignore it (gillespie) — keeps the logic
/// unconditional and avoids stale cache hits if someone switches backend while keeping dt.
pub fn sim_hash(model_hash: &str, params_canonical: &str, backend: &str, dt: f64) -> String {
    let mut h = Sha256::new();
    h.update(model_hash.as_bytes());
    h.update(b"\x00");
    h.update(params_canonical.as_bytes());
    h.update(b"\x00");
    h.update(backend.as_bytes());
    h.update(b"\x00");
    h.update(dt.to_bits().to_le_bytes());
    h.update(b"\x00");
    h.update(TOOL_VERSION.as_bytes());
    hex::encode(h.finalize())
}

/// Hash of a scenario's per-scenario delta: enable/disable lists and param overrides.
/// Does NOT include the scenario name — the name appears in the directory slug for navigation,
/// but two identically-specified scenarios (same enables/disables/params, different names)
/// correctly share a cache entry.
///
/// TODO(compose): when `compose = ["A", "B"]` is implemented (spec v0.4 §8.3),
/// this function must recursively incorporate each composed scenario's definition hash,
/// not just hash the compose list by name. Hashing names would break cache correctness
/// if a composed scenario's params change without the parent scenario changing.
pub fn scen_hash(enable: &[String], disable: &[String], params: &HashMap<String, f64>) -> String {
    let mut h = Sha256::new();

    // Sort enables/disables so order in TOML doesn't matter
    let mut enables = enable.to_vec();
    enables.sort();
    let mut disables = disable.to_vec();
    disables.sort();

    h.update(b"enable\x00");
    for e in &enables {
        h.update(e.as_bytes());
        h.update(b"\x00");
    }
    h.update(b"disable\x00");
    for d in &disables {
        h.update(d.as_bytes());
        h.update(b"\x00");
    }
    h.update(b"params\x00");
    h.update(canonical_params(params).as_bytes());
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

/// Convert a scenario name to a filesystem-safe slug: lowercase, non-[a-z0-9_] → '_'.
pub fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}
