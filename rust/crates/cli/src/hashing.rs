use sha2::{Sha256, Digest};
use std::collections::HashMap;

use crate::version;

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
    h.update(version::VERSION_SHORT.as_bytes());
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
    scen_hash_with_version(enable, disable, params, version::VERSION_SHORT)
}

/// Test-visible variant that allows injecting a synthetic version string.
/// Production code should go through [`scen_hash`], which pins the version
/// to `version::VERSION_SHORT` (semver + git hash). The runtime-version
/// component is load-bearing: without it, a code change that alters
/// scenario resolution (e.g. family-name expansion in
/// `resolve_enable_list`) would silently return stale cached results
/// under identical hashes.
pub(crate) fn scen_hash_with_version(
    enable: &[String], disable: &[String], params: &HashMap<String, f64>,
    version_short: &str,
) -> String {
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
    h.update(b"\x00");
    h.update(version_short.as_bytes());
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── sim_hash ─────────────────────────────────────────────────────────────

    #[test]
    fn sim_hash_stable() {
        assert_eq!(sim_hash("m", "p=1", "gillespie", 1.0), sim_hash("m", "p=1", "gillespie", 1.0));
    }

    #[test]
    fn sim_hash_dt_invalidates() {
        assert_ne!(sim_hash("m", "", "tau_leap", 1.0), sim_hash("m", "", "tau_leap", 0.5));
    }

    #[test]
    fn sim_hash_backend_invalidates() {
        assert_ne!(sim_hash("m", "", "gillespie", 1.0), sim_hash("m", "", "tau_leap", 1.0));
    }

    #[test]
    fn sim_hash_model_invalidates() {
        assert_ne!(sim_hash("model_a", "", "gillespie", 1.0), sim_hash("model_b", "", "gillespie", 1.0));
    }

    #[test]
    fn sim_hash_params_invalidates() {
        assert_ne!(sim_hash("m", "r0=2", "gillespie", 1.0), sim_hash("m", "r0=3", "gillespie", 1.0));
    }

    // ── scen_hash ────────────────────────────────────────────────────────────

    #[test]
    fn scen_hash_stable() {
        let p: HashMap<String, f64> = HashMap::new();
        assert_eq!(scen_hash(&["sia".to_string()], &[], &p), scen_hash(&["sia".to_string()], &[], &p));
    }

    #[test]
    fn scen_hash_enable_order_invariant() {
        let p: HashMap<String, f64> = HashMap::new();
        let ab = scen_hash(&["a".to_string(), "b".to_string()], &[], &p);
        let ba = scen_hash(&["b".to_string(), "a".to_string()], &[], &p);
        assert_eq!(ab, ba);
    }

    #[test]
    fn scen_hash_disable_order_invariant() {
        let p: HashMap<String, f64> = HashMap::new();
        let ab = scen_hash(&[], &["a".to_string(), "b".to_string()], &p);
        let ba = scen_hash(&[], &["b".to_string(), "a".to_string()], &p);
        assert_eq!(ab, ba);
    }

    #[test]
    fn scen_hash_enable_change_invalidates() {
        let p: HashMap<String, f64> = HashMap::new();
        assert_ne!(scen_hash(&["sia_r1".to_string()], &[], &p),
                   scen_hash(&["sia_r2".to_string()], &[], &p));
    }

    #[test]
    fn scen_hash_params_change_invalidates() {
        let mut p1: HashMap<String, f64> = HashMap::new(); p1.insert("vacc_frac".into(), 0.7);
        let mut p2: HashMap<String, f64> = HashMap::new(); p2.insert("vacc_frac".into(), 0.9);
        assert_ne!(scen_hash(&[], &[], &p1), scen_hash(&[], &[], &p2));
    }

    #[test]
    fn scen_hash_name_independent() {
        // Same enables/params, different name → same hash (name is navigation only)
        let p: HashMap<String, f64> = HashMap::new();
        // scen_hash doesn't take a name argument, so this is enforced by the API
        let h1 = scen_hash(&["sia".to_string()], &[], &p);
        let h2 = scen_hash(&["sia".to_string()], &[], &p);
        assert_eq!(h1, h2);
    }

    #[test]
    fn scen_hash_returns_64_hex_chars() {
        let p: HashMap<String, f64> = HashMap::new();
        assert_eq!(scen_hash(&[], &[], &p).len(), 64);
    }

    #[test]
    fn scen_hash_version_invalidates() {
        // Regression guard: a code change that alters scenario semantics
        // (e.g. resolve_enable_list family expansion) must invalidate the
        // cache. Version is pinned into scen_hash so two differing
        // versions produce different digests even with identical inputs.
        let p: HashMap<String, f64> = HashMap::new();
        let h_v1 = scen_hash_with_version(&["sia".into()], &[], &p, "0.1.0+aaaaaaa");
        let h_v2 = scen_hash_with_version(&["sia".into()], &[], &p, "0.1.0+bbbbbbb");
        assert_ne!(h_v1, h_v2, "scen_hash must invalidate on version change");
    }

    // ── slug ─────────────────────────────────────────────────────────────────

    #[test]
    fn slug_alphanumeric_passthrough() {
        assert_eq!(slug("baseline"), "baseline");
        assert_eq!(slug("with_sia"), "with_sia");
    }

    #[test]
    fn slug_lowercases() {
        assert_eq!(slug("WithSIA"), "withsia");
    }

    #[test]
    fn slug_replaces_spaces_and_specials() {
        assert_eq!(slug("with sia!"), "with_sia_");
        assert_eq!(slug("r0=3.0"), "r0_3_0");
    }

    // ── canonical_params ─────────────────────────────────────────────────────

    #[test]
    fn canonical_params_sorted_keys() {
        let mut p: HashMap<String, f64> = HashMap::new();
        p.insert("z".into(), 1.0);
        p.insert("a".into(), 2.0);
        // Regardless of insertion order, output is sorted
        assert_eq!(canonical_params(&p), "a=2;z=1");
    }

    #[test]
    fn canonical_params_empty() {
        assert_eq!(canonical_params(&HashMap::new()), "");
    }
}
