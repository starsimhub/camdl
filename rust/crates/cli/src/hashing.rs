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

/// Content hash for a single simulate run. Covers the full identity of
/// what produced `traj.tsv`: the simulation config (`sim_hash`), the
/// scenario delta (`scen_hash`), and the seed. Stored in `run.json`
/// as `Run.hash` for cache-staleness checks that are independent of
/// the directory layout (so we can rename the tree without invalidating
/// the cache).
pub fn run_hash(sim_hash: &str, scen_hash: &str, seed: u64) -> String {
    let mut h = Sha256::new();
    h.update(b"run\x00");
    h.update(sim_hash.as_bytes());
    h.update(b"\x00");
    h.update(scen_hash.as_bytes());
    h.update(b"\x00");
    h.update(seed.to_le_bytes());
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

/// Full 64-char SHA-256 of a byte slice, hex-encoded. Used where the
/// caller wants a full content hash (e.g. fit_toml_hash in the top-level
/// fit run record); the 8-char truncated form is only appropriate when
/// the hash is paired with a richer identifier.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Hash the contents of a file (first 4 bytes of SHA256, 8 hex chars).
/// Returns `None` when the file can't be read — callers use this to
/// surface `<unreadable>` in provenance records rather than failing
/// the whole run.
///
/// Shared between simulate (data-file hashing for scen_hash / run
/// metadata) and fit (data-file hashing for fit_stage_hash /
/// per-stage provenance). Was `fit::provenance::file_content_hash`
/// before the 2026-04-19 unification.
pub fn file_hash(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(&Sha256::digest(&bytes)[..4]))
}

/// Content hash for a fit's *directory* (seed-independent). Keyed on
/// `(model IR, data files, fit.toml, version)` — deliberately omits
/// seed so re-running the same fit config with different seeds lands
/// in the same `output/fits/<stem>-<hash>/` directory, with seeds
/// differentiated via the `fit_<seed>` subdirectory.
///
/// Used by `FitConfigV2::fit_dir()` to produce the content-addressable
/// suffix on the fit-directory name. The proposal's "edit your
/// fit.toml and get a new dir" invariant falls out of this: any edit
/// to model, data, or fit.toml changes the hash; seed alone doesn't.
pub fn fit_content_hash(
    model_ir_bytes: &[u8],
    data_files: &mut [(String, Vec<u8>)],
    fit_toml_bytes: &[u8],
) -> String {
    let mut h = Sha256::new();
    h.update(b"model\x00");
    h.update(model_ir_bytes);
    h.update(b"\x00data\x00");
    data_files.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, bytes) in data_files.iter() {
        h.update(name.as_bytes());
        h.update(b"\x00");
        h.update(bytes);
        h.update(b"\x00");
    }
    h.update(b"fit\x00");
    h.update(fit_toml_bytes);
    h.update(b"\x00version\x00");
    h.update(version::VERSION_SHORT.as_bytes());
    // Full 64-char hex. Directory-name truncation happens at the
    // path layer via `run_paths::fit_run_dir`, not here —
    // decoupling storage key from display prefix. Prior versions
    // truncated to 8 chars here, which made `Run.hash`'s "full
    // 64-char hex" docstring a lie for FitMeta and created
    // collision risk at ~65k fits. Hardening proposal ship-now #1.
    hex::encode(h.finalize())
}

/// Fit-level input hash: `(model IR bytes, data files, fit.toml bytes,
/// seed, version)` → full 64-char hex. Identifies the whole fit as a
/// computation; written into `fit_state.toml.input_hash`. A change to
/// any of (model, data, fit.toml, seed, camdl version) invalidates
/// the cache.
///
/// Operates on raw byte slices (caller reads the files), matching the
/// v1 `FitToml` world where fit.toml is hashed whole. The v2
/// `FitConfigV2` world uses [`fit_stage_hash`] below, which
/// decomposes the config into structured fields.
///
/// Was `fit::provenance::compute_input_hash` before the 2026-04-19
/// unification.
pub fn fit_input_hash(
    model_ir_bytes: &[u8],
    data_files: &mut [(String, Vec<u8>)],
    fit_toml_bytes: &[u8],
    seed: u64,
) -> String {
    let mut h = Sha256::new();
    h.update(b"model\x00");
    h.update(model_ir_bytes);
    h.update(b"\x00data\x00");
    data_files.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, bytes) in data_files.iter() {
        h.update(name.as_bytes());
        h.update(b"\x00");
        h.update(bytes);
        h.update(b"\x00");
    }
    h.update(b"fit\x00");
    h.update(fit_toml_bytes);
    h.update(b"\x00seed\x00");
    h.update(seed.to_le_bytes());
    h.update(b"\x00version\x00");
    h.update(version::VERSION_SHORT.as_bytes());
    // Full 64-char hex, symmetric with fit_content_hash. v1
    // fit_state.toml readers consume the hash as opaque string —
    // no structural dependence on the 8-char width.
    hex::encode(h.finalize())
}

/// Extract a directory-safe stem from a file path: basename without
/// extension(s), slugified. Used to label fit / sim output
/// directories so `ls output/fits/` shows recognisable names alongside
/// their content hashes. Multi-dot extensions (`foo.ir.json` →
/// `foo`) collapse by stripping from the first dot.
pub fn path_stem_slug(path: &str) -> Option<String> {
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    let stem = name.split('.').next().filter(|s| !s.is_empty())?;
    Some(slug(stem))
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

    // ── Frozen golden hashes (regression guard) ──────────────────────────────
    //
    // These assertions lock each primary hash helper to a known byte-
    // for-byte output for a fixed input. If someone refactors the
    // hashing code and the bytes move, CI fails with a crisp diff —
    // forcing the refactor to either justify the break (and update
    // this test as a conscious decision) or preserve the hash.
    //
    // The inputs are chosen to be minimal-but-not-trivial so they
    // exercise the main codepaths (params canonicalisation, enable/
    // disable sort, version injection via scen_hash_with_version).

    #[test]
    fn golden_hash_model_hash() {
        let ir = r#"{"compartments":["S","I"],"parameters":[{"name":"beta"}]}"#;
        assert_eq!(model_hash(ir),
            "53b7d24e97c71b0fb35e58a95d21ccd8b7178a22317e3115df5770c856d9180b");
    }

    #[test]
    fn golden_hash_sim_hash() {
        // scen_hash_with_version's test-friendly form is used by the
        // scen_hash tests; here sim_hash folds in VERSION_SHORT which
        // bumps every commit — so we hash it with a synthetic model
        // hash that stays fixed. We pin the model side only.
        let mh = "abc".repeat(16); // 48 chars, stable across commits
        // Two calls in the same process must equal.
        assert_eq!(sim_hash(&mh, "beta=0.3", "gillespie", 1.0),
                   sim_hash(&mh, "beta=0.3", "gillespie", 1.0));
    }

    #[test]
    fn golden_hash_fit_content_hash_is_full_64_chars() {
        // Hardening #1: fit_content_hash used to truncate to 8 hex
        // chars, contradicting the Run.hash docstring. Lock the new
        // full-width output here so a future truncation regression
        // fails CI instead of silently reinstating collision risk.
        let model_ir = r#"{"compartments":["S","I"],"parameters":[]}"#;
        let fit_toml = b"[estimate]\nbeta = { bounds = [0.1, 2.0] }\n";
        let mut data: Vec<(String, Vec<u8>)> = vec![
            ("cases".into(), b"time\tvalue\n1\t5\n2\t7\n".to_vec()),
        ];
        let h = fit_content_hash(model_ir.as_bytes(), &mut data, fit_toml);
        assert_eq!(h.len(), 64,
            "fit_content_hash must return full 64-char hex (was truncated \
             to 8 pre-hardening; see hardening-proposal §ship-now/#1)");
        // Call it twice — must be deterministic.
        let mut data2: Vec<(String, Vec<u8>)> = vec![
            ("cases".into(), b"time\tvalue\n1\t5\n2\t7\n".to_vec()),
        ];
        let h2 = fit_content_hash(model_ir.as_bytes(), &mut data2, fit_toml);
        assert_eq!(h, h2);
    }

    #[test]
    fn golden_hash_scen_hash_with_version() {
        // scen_hash_with_version pins the version so the golden bytes
        // remain stable across commits. This guards the sort-enables,
        // param-canonicalisation, and domain-separator logic.
        let mut params = HashMap::new();
        params.insert("rho".to_string(), 0.5);
        let h = scen_hash_with_version(
            &["sia".to_string(), "school_close".to_string()],
            &[],
            &params,
            "0.0.0+frozen",
        );
        assert_eq!(h, "3d19534d546efd26118d6983fcd8a58a559c9791477db4316d3edfc357dadc78");
    }

    // ── run_hash ─────────────────────────────────────────────────────────────

    #[test]
    fn run_hash_stable() {
        let a = run_hash("aaa", "bbb", 42);
        let b = run_hash("aaa", "bbb", 42);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64, "run_hash returns full SHA-256 hex (64 chars)");
    }

    #[test]
    fn run_hash_known_bytes() {
        // Regression guard: if anyone changes the domain separator or
        // field order in `run_hash`, existing caches become invisible.
        // The known value locks the current function to its current
        // output. Updating this test is a conscious break.
        let h = run_hash("sim123", "scen456", 7);
        assert_eq!(h, "74559eaeb35e55fe88042fb428009a47df00be22b1627d87d4171b9688b77443");
    }

    #[test]
    fn run_hash_seed_invalidates() {
        assert_ne!(run_hash("aaa", "bbb", 1), run_hash("aaa", "bbb", 2));
    }

    #[test]
    fn run_hash_sim_invalidates() {
        assert_ne!(run_hash("aaa", "bbb", 1), run_hash("ccc", "bbb", 1));
    }

    #[test]
    fn run_hash_scen_invalidates() {
        assert_ne!(run_hash("aaa", "bbb", 1), run_hash("aaa", "ddd", 1));
    }

    #[test]
    fn run_hash_domain_separator_disambiguates() {
        // "aa" + "bb" + 0  must differ from "aab" + "b" + 0  even
        // though the concatenated bytes are similar. Domain separators
        // and length-delimited field framing guard against that
        // ambiguity. Guards against the domain-separator being dropped
        // during a refactor.
        assert_ne!(run_hash("aa", "bb", 0), run_hash("aab", "b", 0));
        assert_ne!(run_hash("aa", "bb", 0), run_hash("a", "abb", 0));
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

    // ── file_hash / fit_input_hash (relocated from fit::provenance) ─────────

    #[test]
    fn file_hash_returns_8_hex() {
        let tmp = std::env::temp_dir().join(format!(
            "camdl_hash_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::write(&tmp, b"hello world").unwrap();
        let h = file_hash(tmp.to_str().unwrap()).unwrap();
        assert_eq!(h.len(), 8, "file_hash should return 8 hex chars");
        // SHA256("hello world")[..4] is b94d27b9 in hex.
        assert_eq!(h, "b94d27b9");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn file_hash_returns_none_for_missing() {
        assert!(file_hash("/does/not/exist/at/all").is_none());
    }

    #[test]
    fn fit_input_hash_deterministic() {
        let model = b"ir:{}";
        let mut data = vec![("cases".to_string(), b"t\ty\n1\t2\n".to_vec())];
        let fit = b"[fit]\nmodel = \"x\"";
        let h1 = fit_input_hash(model, &mut data.clone(), fit, 42);
        let h2 = fit_input_hash(model, &mut data, fit, 42);
        assert_eq!(h1, h2, "same inputs → same hash");
        assert_eq!(h1.len(), 64,
            "fit_input_hash returns full 64-char hex (widened in hardening \
             ship-now #1; was 8 pre-hardening)");
    }

    #[test]
    fn fit_input_hash_sensitivity() {
        let model = b"ir:{}";
        let data = vec![("cases".to_string(), b"a".to_vec())];
        let fit = b"[fit]";
        let base = fit_input_hash(model, &mut data.clone(), fit, 1);
        // Change each input independently; hash must differ every time.
        let diff_model = fit_input_hash(b"ir:{changed}", &mut data.clone(), fit, 1);
        let diff_data  = fit_input_hash(model, &mut vec![("cases".into(), b"b".to_vec())], fit, 1);
        let diff_fit   = fit_input_hash(model, &mut data.clone(), b"[fit]\nseed=1", 1);
        let diff_seed  = fit_input_hash(model, &mut data.clone(), fit, 2);
        assert_ne!(base, diff_model, "model change must invalidate");
        assert_ne!(base, diff_data,  "data change must invalidate");
        assert_ne!(base, diff_fit,   "fit.toml change must invalidate");
        assert_ne!(base, diff_seed,  "seed change must invalidate");
    }

    #[test]
    fn fit_input_hash_data_order_invariant() {
        // Multi-stream fits can register streams in any order; hash must
        // not depend on that. Regression guard on the sort-before-hash.
        let model = b"ir";
        let mut order_a = vec![
            ("a".to_string(), b"1".to_vec()),
            ("b".to_string(), b"2".to_vec()),
        ];
        let mut order_b = vec![
            ("b".to_string(), b"2".to_vec()),
            ("a".to_string(), b"1".to_vec()),
        ];
        let h_a = fit_input_hash(model, &mut order_a, b"", 1);
        let h_b = fit_input_hash(model, &mut order_b, b"", 1);
        assert_eq!(h_a, h_b, "stream order must not affect hash");
    }
}
