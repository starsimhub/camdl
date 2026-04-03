//! Provenance hashing: input_hash (computation identity) and content_hash (tamper detection).

use sha2::{Sha256, Digest};
use std::collections::HashMap;

use crate::version;

/// Input hash: identifies the computation (model + data + config + seed + version).
pub fn compute_input_hash(
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
    hex::encode(&h.finalize()[..4]) // 8 hex chars
}

/// Content hash: detects manual edits to mle_params.toml.
pub fn compute_content_hash(params: &HashMap<String, f64>) -> String {
    let mut pairs: Vec<(&String, &f64)> = params.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    let mut h = Sha256::new();
    for (name, value) in pairs {
        h.update(format!("{}={:.12}\x00", name, value).as_bytes());
    }
    hex::encode(&h.finalize()[..4]) // 8 hex chars
}

/// Write mle_params.toml with provenance comment header.
pub fn write_mle_params(
    path: &str,
    all_params: &HashMap<String, f64>,
    metadata: &MleMetadata,
) -> Result<(), String> {
    use std::io::Write;
    let content_hash = compute_content_hash(all_params);
    let mut f = std::fs::File::create(path)
        .map_err(|e| format!("cannot write {}: {}", path, e))?;

    writeln!(f, "# camdl fit output").unwrap();
    writeln!(f, "# Content hash: {} (editing any value below invalidates this)", content_hash).unwrap();
    writeln!(f, "# Input hash: {}", metadata.input_hash).unwrap();
    writeln!(f, "# Model: {} (hash: {})", metadata.model_path, &metadata.model_hash[..8]).unwrap();
    for (name, hash) in &metadata.data_hashes {
        writeln!(f, "# Data: {} (hash: {})", name, &hash[..8.min(hash.len())]).unwrap();
    }
    writeln!(f, "# Seed: {}", metadata.seed).unwrap();
    writeln!(f, "# Stage: {}, chain {}", metadata.stage, metadata.best_chain + 1).unwrap();
    writeln!(f, "# Log-likelihood: {:.1} (sd: {:.1}, N={})",
        metadata.loglik, metadata.loglik_sd, metadata.n_particles).unwrap();
    if let Some((ess_mean, ess_min)) = metadata.ess_at_mle {
        writeln!(f, "# ESS at MLE: mean={:.0}, min={:.0}", ess_mean, ess_min).unwrap();
    }
    writeln!(f, "# Timestamp: {}", metadata.timestamp).unwrap();
    writeln!(f, "# camdl version: {}", version::VERSION_SHORT).unwrap();
    writeln!(f).unwrap();

    // Write params in sorted order for deterministic output
    let mut pairs: Vec<(&String, &f64)> = all_params.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    for (name, value) in pairs {
        writeln!(f, "{} = {}", name, crate::fit::runner::format_param_value(*value)).unwrap();
    }
    Ok(())
}

/// Check cache: does the stage directory already have results with matching input hash?
/// Reads input_hash directly from fit_state.toml — one TOML parse, one string comparison.
pub fn check_cache(stage_dir: &str, input_hash: &str) -> CacheStatus {
    use crate::fit::state::FitState;
    match FitState::load(stage_dir) {
        Ok(state) => {
            match state.input_hash {
                Some(ref h) if h == input_hash => CacheStatus::Match,
                Some(_) => CacheStatus::Mismatch,
                None => CacheStatus::Mismatch, // old format without input_hash
            }
        }
        Err(_) => CacheStatus::NotFound,
    }
}

/// Hash the contents of a file (first 4 bytes of SHA256, 8 hex chars).
pub fn file_content_hash(path: &str) -> Option<String> {
    use sha2::Digest;
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(&sha2::Sha256::digest(&bytes)[..4]))
}

/// Collect content hashes of all primary output files in a stage directory.
/// Returns (relative_path, hash) pairs for files that exist.
pub fn collect_output_hashes(stage_dir: &str, primary_only: bool) -> Vec<(String, String)> {
    let mut outputs = Vec::new();

    // Primary outputs (always verified by `camdl fit status`)
    let primary_files = [
        "mle_params.toml",
        "fit_state.toml",
        "pfilter_trace.tsv",
        "ess_at_mle.tsv",
        "pfilter_loglik.txt",
        "fit_report.txt",
    ];
    for name in &primary_files {
        let path = format!("{}/{}", stage_dir, name);
        if let Some(hash) = file_content_hash(&path) {
            outputs.push((name.to_string(), hash));
        }
    }

    // Profile files
    let profile_dir = format!("{}/profiles", stage_dir);
    if let Ok(entries) = std::fs::read_dir(&profile_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with("_profile.tsv") {
                let path = entry.path().to_string_lossy().to_string();
                if let Some(hash) = file_content_hash(&path) {
                    outputs.push((format!("profiles/{}", name), hash));
                }
            }
        }
    }

    if primary_only {
        return outputs;
    }

    // Chain-level files (verified only with --audit)
    for i in 1..=20 {
        let chain_dir = format!("{}/chain_{}", stage_dir, i);
        if !std::path::Path::new(&chain_dir).exists() { break; }
        for name in &["parameter_traces.tsv", "final_params.toml"] {
            let path = format!("{}/{}", chain_dir, name);
            if let Some(hash) = file_content_hash(&path) {
                outputs.push((format!("chain_{}/{}", i, name), hash));
            }
        }
    }

    outputs
}

pub enum CacheStatus {
    Match,
    Mismatch,
    NotFound,
}

pub struct MleMetadata {
    pub input_hash: String,
    pub model_path: String,
    pub model_hash: String,
    pub data_hashes: Vec<(String, String)>,
    pub seed: u64,
    pub stage: String,
    pub best_chain: usize,
    pub loglik: f64,
    pub loglik_sd: f64,
    pub n_particles: usize,
    pub ess_at_mle: Option<(f64, f64)>,
    pub timestamp: String,
}

/// Verify content hash of an mle_params.toml file.
pub fn verify_content_hash(path: &str) -> Result<ContentVerification, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path, e))?;

    // Extract declared hash from comment
    let declared = contents.lines()
        .find(|l| l.starts_with("# Content hash:"))
        .and_then(|l| l.split_whitespace().nth(3))
        .map(|s| s.to_string());

    // Parse param values
    let params: HashMap<String, f64> = contents.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut parts = l.splitn(2, '=');
            let k = parts.next()?.trim().to_string();
            let v: f64 = parts.next()?.trim().parse().ok()?;
            Some((k, v))
        })
        .collect();

    let computed = compute_content_hash(&params);

    match declared {
        Some(ref d) if *d == computed => Ok(ContentVerification::Valid),
        Some(d) => Ok(ContentVerification::Modified { declared: d, computed }),
        None => Ok(ContentVerification::NoHash),
    }
}

pub enum ContentVerification {
    Valid,
    Modified {
        declared: String,
        computed: String,
    },
    NoHash,
}
