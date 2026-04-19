//! Unified run-metadata ADT for the `output/` tree.
//!
//! One `Run` type with a `kind: RunKind` discriminator covers every
//! result camdl produces — simulate runs, top-level fits, and
//! per-stage fits — under one schema. Replaces the parallel
//! `cas::RunMeta` and `fit::provenance::StageProvenance` structs that
//! had ~80 % field overlap with drifting names (version vs
//! camdl_version, created_at vs timestamp, etc.).
//!
//! See `docs/dev/proposals/2026-04-19-unified-output-tree.md` for the
//! full design. This module is introduced in commit 2/6 of the plan:
//! types live alongside the legacy structs and get filled at write
//! sites in commit 4, at which point the legacy types get deleted.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata written to `run.json` at the top of every content-hashed
/// run directory. Shared fields live at the top level; kind-specific
/// fields are inside `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Content hash for this run, full 64-char hex. Scope depends on
    /// `kind`:
    ///   - `Simulate`: hash of (sim_hash, scen_hash, seed).
    ///   - `Fit`: seed-independent content hash of
    ///     (fit.toml, model IR, data files).
    ///   - `FitStage`: stage-scope config hash from `fit_stage_hash`
    ///     (includes stage algorithm + seed).
    /// The 8-char prefix appears in the filesystem path.
    pub hash: String,
    /// camdl version at write time (e.g. "0.1.0+abc1234").
    pub version: String,
    /// ISO 8601 UTC timestamp at completion.
    pub created_at: String,
    /// Original argv that produced this run — `camdl show <hash>`
    /// prints it back for reproducibility.
    pub argv: Vec<String>,
    /// Total wall time for the run (seconds). Always set; cache hits
    /// record time-to-cache-hit-detection (typically < 0.1s).
    pub wall_time_seconds: f64,
    /// Kind-specific payload.
    pub kind: RunKind,
}

/// Tagged union over the three result shapes. `serde(tag = "kind")`
/// emits a `"kind": "simulate"` (etc.) field in the JSON, so
/// `camdl list` can discriminate without needing to know the directory
/// layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RunKind {
    /// One simulate invocation. The directory contains `traj.tsv` and
    /// optional `obs/<obs_hash>-<obs_seed>/` subdirectories.
    Simulate(SimulateMeta),
    /// A complete fit (potentially multi-stage). The directory
    /// contains per-stage subdirectories, each with its own
    /// stage-level `Run` whose kind is `FitStage`.
    Fit(FitMeta),
    /// One stage of a fit. The directory is a child of a `Fit` run,
    /// at `<fit_dir>/real/fit_<seed>/<stage>/` or
    /// `<fit_dir>/synthetic/ds_NN/fit_<seed>/<stage>/`.
    FitStage(FitStageMeta),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateMeta {
    /// Model file path or name (display only — not a hash input).
    pub model: String,
    /// Full model IR hash (64 hex chars).
    pub model_hash: String,
    /// Named scenario or "baseline".
    pub scenario: String,
    /// Simulation config hash: model + base params + backend + dt + version.
    pub sim_hash: String,
    /// Scenario delta hash: enable/disable/overrides.
    pub scen_hash: String,
    pub seed: u64,
    pub backend: String,
    pub dt: f64,
    /// Sweep-point param values (empty for single-run `--cas`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sweep_point: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitMeta {
    /// Model file path (display only — not a hash input).
    pub model: String,
    /// Structural model IR hash.
    pub model_hash: String,
    /// Path to the fit.toml that produced this fit.
    pub fit_toml_path: String,
    /// Hash of the fit.toml bytes. (Equals `fit_input_hash`'s fit-toml
    /// component for v1 FitToml, or a canonical-form hash for v2.)
    pub fit_toml_hash: String,
    /// Per-stream data file hashes.
    pub data_hashes: HashMap<String, String>,
    /// Names of parameters declared in `[estimate]`.
    pub estimated: Vec<String>,
    /// Resolved fixed params (name → numeric value).
    pub fixed: HashMap<String, f64>,
    /// Stage names declared in fit.toml, in execution order.
    pub stages_declared: Vec<String>,
    /// IC-free inference flag (see 2026-04-18 proposal).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ic_free: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitStageMeta {
    /// Hash of the parent fit (matches the `hash` on the enclosing
    /// `Run` of kind `Fit`). Enables walking from a stage back to its
    /// parent without relying on directory-layout inference.
    pub fit_hash: String,
    /// Stage name within the fit (e.g. "scout", "refine").
    pub stage: String,
    /// Stage method: "if2", "pgas", "pmmh", "pfilter".
    pub method: String,
    // NB: the stage's own content hash lives in the enclosing
    // `Run.hash` field (a FitStage run hashes exactly its stage-scope
    // inputs). Previously FitStageMeta carried a duplicate
    // `stage_hash: String` — removed to collapse the two-source-of-
    // truth smell.
    pub seed: u64,
    pub n_chains: usize,
    /// Stage-specific algorithm settings (chains, particles, cooling,
    /// etc.). A `serde_json::Value` keeps the shape open — each method
    /// (if2/pgas/pmmh/pfilter) has a different parameter set, and the
    /// human-readable record doesn't need a typed schema.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub algorithm: serde_json::Value,
    /// Best loglik across chains; `None` if the stage didn't compute
    /// one (e.g. a pure-diagnostic pass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_loglik: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_chain: Option<usize>,
    /// Reference to a parent stage this one started from, if any
    /// (e.g. refine → scout). Absent when the stage has no predecessor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_from: Option<StartsFromRef>,
    /// Path to the upstream fit dir this fit was derived from
    /// (`camdl fit derive` workflows). Free-form string — the consumer
    /// treats this as a display hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
}

/// Stable reference to a parent stage. Uses the stage *name* plus its
/// content hash, not a filesystem path — so the reference survives any
/// tree reorganisation. The path is a cache-lookup concern the caller
/// reconstructs via `run_paths::fit_stage_dir`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartsFromRef {
    pub stage: String,
    pub stage_hash: String,
}

/// Unified cache status: result of comparing an expected content hash
/// against the `run.json` in a directory. Applies to both simulate
/// and fit-stage runs.
#[derive(Debug, Clone)]
pub enum CacheStatus {
    /// Run directory exists and its stored hash matches the expected
    /// hash; caller can read results from `run_dir`.
    Hit { stored_hash: String },
    /// Directory exists but the stored hash differs from the expected
    /// one. Typically triggers a re-run with a warning.
    Stale { stored: String, current: String },
    /// No `run.json` at the expected location; cache miss.
    Miss,
}

impl Run {
    /// Write `run.json` inside `dir`. Creates parent directories.
    pub fn write(&self, dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(dir.join("run.json"), json)
    }

    /// Read `run.json` from `dir`. Returns a serde error kinds in
    /// `ErrorKind::InvalidData` if the file exists but doesn't match
    /// the schema — a sign the directory was written by an older
    /// camdl version or a different tool.
    pub fn read(dir: &std::path::Path) -> std::io::Result<Run> {
        let contents = std::fs::read_to_string(dir.join("run.json"))?;
        serde_json::from_str(&contents)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Check whether `dir` has a `run.json` whose `hash` matches
    /// `expected_hash`. Replaces the sim-side `has_cached_traj` +
    /// `RunMeta.sim_hash` pair and the fit-side provenance.json check
    /// with one uniform code path.
    pub fn check_cache(dir: &std::path::Path, expected_hash: &str) -> CacheStatus {
        match Self::read(dir) {
            Ok(run) if run.hash == expected_hash =>
                CacheStatus::Hit { stored_hash: run.hash },
            Ok(run) => CacheStatus::Stale {
                stored: run.hash,
                current: expected_hash.to_string(),
            },
            Err(_) => CacheStatus::Miss,
        }
    }

    /// Short hash prefix used in filesystem paths. Always 8 chars.
    pub fn hash_prefix(&self) -> &str {
        &self.hash[..self.hash.len().min(8)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_simulate_run() -> Run {
        Run {
            hash: "abc12345def6789000000000000000000000000000000000000000000000000".into(),
            version: "0.1.0+test".into(),
            created_at: "2026-04-19T12:00:00Z".into(),
            argv: vec!["camdl".into(), "simulate".into(), "sir.camdl".into()],
            wall_time_seconds: 1.23,
            kind: RunKind::Simulate(SimulateMeta {
                model: "sir.camdl".into(),
                model_hash: "f00d".repeat(16),
                scenario: "baseline".into(),
                sim_hash: "abc12345".into(),
                scen_hash: "def67890".into(),
                seed: 42,
                backend: "gillespie".into(),
                dt: 1.0,
                sweep_point: HashMap::new(),
            }),
        }
    }

    fn sample_fit_run() -> Run {
        Run {
            hash: "deadbeef".repeat(8),
            version: "0.1.0+test".into(),
            created_at: "2026-04-19T12:00:00Z".into(),
            argv: vec!["camdl".into(), "fit".into(), "run".into(), "fit.toml".into()],
            wall_time_seconds: 42.0,
            kind: RunKind::Fit(FitMeta {
                model: "sir.camdl".into(),
                model_hash: "f00d".repeat(16),
                fit_toml_path: "fit.toml".into(),
                fit_toml_hash: "cafebabe".into(),
                data_hashes: {
                    let mut m = HashMap::new();
                    m.insert("cases".into(), "d4ta".repeat(2));
                    m
                },
                estimated: vec!["beta".into(), "gamma".into()],
                fixed: {
                    let mut m = HashMap::new();
                    m.insert("N0".into(), 1000.0);
                    m
                },
                stages_declared: vec!["scout".into(), "refine".into()],
                ic_free: false,
            }),
        }
    }

    fn sample_fit_stage_run() -> Run {
        Run {
            hash: "ae123456".repeat(8),
            version: "0.1.0+test".into(),
            created_at: "2026-04-19T12:00:00Z".into(),
            argv: vec!["camdl".into(), "fit".into(), "run".into(),
                       "fit.toml".into(), "--stage".into(), "refine".into()],
            wall_time_seconds: 10.0,
            kind: RunKind::FitStage(FitStageMeta {
                fit_hash: "deadbeef".repeat(8),
                stage: "refine".into(),
                method: "if2".into(),
                seed: 42,
                n_chains: 4,
                algorithm: serde_json::Value::Null,
                best_loglik: Some(-56.7),
                best_chain: Some(1),
                starts_from: Some(StartsFromRef {
                    stage: "scout".into(),
                    stage_hash: "beef1234".repeat(8),
                }),
                derived_from: None,
            }),
        }
    }

    #[test]
    fn simulate_run_roundtrip() {
        let r = sample_simulate_run();
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""kind":"simulate""#),
            "kind discriminator missing from JSON: {}", json);
        let parsed: Run = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.hash, r.hash);
        match parsed.kind {
            RunKind::Simulate(m) => assert_eq!(m.seed, 42),
            _ => panic!("expected Simulate"),
        }
    }

    #[test]
    fn fit_run_roundtrip() {
        let r = sample_fit_run();
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""kind":"fit""#));
        let parsed: Run = serde_json::from_str(&json).unwrap();
        match parsed.kind {
            RunKind::Fit(m) => {
                assert_eq!(m.estimated.len(), 2);
                assert_eq!(m.stages_declared, vec!["scout", "refine"]);
            }
            _ => panic!("expected Fit"),
        }
    }

    #[test]
    fn fit_stage_run_roundtrip() {
        let r = sample_fit_stage_run();
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""kind":"fit-stage""#));
        let parsed: Run = serde_json::from_str(&json).unwrap();
        match parsed.kind {
            RunKind::FitStage(m) => {
                assert_eq!(m.stage, "refine");
                assert_eq!(m.best_loglik, Some(-56.7));
                assert!(m.starts_from.is_some());
            }
            _ => panic!("expected FitStage"),
        }
    }

    #[test]
    fn write_read_roundtrip() {
        let tmp = std::env::temp_dir().join(format!(
            "camdl_run_meta_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&tmp).unwrap();
        let r = sample_fit_run();
        r.write(&tmp).unwrap();
        let read = Run::read(&tmp).unwrap();
        assert_eq!(read.hash, r.hash);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn check_cache_hit_stale_miss() {
        let tmp = std::env::temp_dir().join(format!(
            "camdl_cache_status_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&tmp).unwrap();
        let r = sample_simulate_run();
        let stored_hash = r.hash.clone();
        r.write(&tmp).unwrap();

        // Miss before write.
        let empty = tmp.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(matches!(Run::check_cache(&empty, &stored_hash), CacheStatus::Miss));

        // Hit with matching hash.
        match Run::check_cache(&tmp, &stored_hash) {
            CacheStatus::Hit { stored_hash: h } => assert_eq!(h, stored_hash),
            other => panic!("expected Hit, got {:?}", other),
        }

        // Stale when hash differs.
        match Run::check_cache(&tmp, "different_hash") {
            CacheStatus::Stale { stored, current } => {
                assert_eq!(stored, stored_hash);
                assert_eq!(current, "different_hash");
            }
            other => panic!("expected Stale, got {:?}", other),
        }

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn hash_prefix_is_8_chars() {
        let r = sample_simulate_run();
        assert_eq!(r.hash_prefix().len(), 8);
        assert_eq!(r.hash_prefix(), "abc12345");
    }

    #[test]
    fn optional_fields_skip_when_empty() {
        // Regression guard: serde(skip_serializing_if) on best_loglik,
        // best_chain, starts_from, sweep_point, ic_free. An empty
        // field shouldn't bloat the JSON.
        let r = Run {
            hash: "x".repeat(64), version: "v".into(),
            created_at: "t".into(), argv: vec![], wall_time_seconds: 0.0,
            kind: RunKind::FitStage(FitStageMeta {
                fit_hash: "f".repeat(64),
                stage: "mle".into(), method: "if2".into(),
                seed: 1, n_chains: 1,
                algorithm: serde_json::Value::Null,
                best_loglik: None, best_chain: None, starts_from: None,
                derived_from: None,
            }),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("best_loglik"));
        assert!(!json.contains("best_chain"));
        assert!(!json.contains("starts_from"));
    }
}
