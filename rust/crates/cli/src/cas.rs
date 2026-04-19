//! Content-addressable storage helpers shared between `camdl simulate --cas`
//! (one-shot cache opt-in) and `camdl simulate --batch` (bulk experiments).
//!
//! Canonical layout (as of the 2026-04-19 output-tree unification):
//!
//! ```text
//! <root>/                                     # default: ./output
//!   sims/                                     # was `runs/` before 2026-04-19
//!     {sim_hash[:8]}/                         # model + base params + backend + dt + version
//!       {scenario_slug}-{scen_hash[:8]}/      # scenario delta (enable/disable/overrides)
//!         seed_{n}/
//!           traj.tsv                          # trajectory output (canonical)
//!           run.json                          # run metadata
//!           obs/                              # optional, one dir per (obs-model, obs-seed)
//!             {obs_hash[:8]}-{obs_seed}/
//!               <stream>.tsv                  # observation draws (wide or per-stream)
//!               obs.json                      # obs metadata
//!   fits/                                     # was `results/fits/` before 2026-04-19
//!     <config_stem>/
//!       real/fit_<seed>/<stage>/              # or synthetic/ds_NN/fit_<seed>/<stage>/
//! ```
//!
//! Browsed uniformly by `camdl list / show / cat`.
//!
//! The split between `traj.tsv` (outer run dir) and `obs/{obs_hash}-{obs_seed}/`
//! lets users iterate observation draws without recomputing the trajectory —
//! the trajectory cache key is `(sim_hash, scen_hash, seed)`; the obs cache
//! key is `(trajectory, obs_hash, obs_seed)`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};

use crate::hashing::{scen_hash, slug};

/// Build the canonical relative run path.
///
/// With `model_stem = None`:   `{sim_hash[:8]}/{scenario_slug}-{scen_hash[:8]}/seed_{n}`
/// With `model_stem = Some(s)`: `{s}-{sim_hash[:8]}/{scenario_slug}-{scen_hash[:8]}/seed_{n}`
///
/// The optional model stem (typically the basename of the `.camdl` file,
/// slugified via [`hashing::path_stem_slug`]) lets `ls output/sims/` surface
/// recognisable names alongside their content hashes. The hash still fully
/// discriminates — two models with the same stem but different content
/// land in different directories.
///
/// `sim_hash` must be the full 64-char hex string from `hashing::sim_hash`.
pub fn run_path_relative(
    sim_hash: &str,
    model_stem: Option<&str>,
    scenario_name: &str,
    enable: &[String],
    disable: &[String],
    params: &HashMap<String, f64>,
    seed: u64,
) -> String {
    let sh = scen_hash(enable, disable, params);
    let hash_prefix = &sim_hash[..8.min(sim_hash.len())];
    let head = match model_stem {
        Some(s) if !s.is_empty() => format!("{}-{}", s, hash_prefix),
        _ => hash_prefix.to_string(),
    };
    format!("{}/{}-{}/seed_{}", head, slug(scenario_name), &sh[..8], seed)
}

/// Absolute path to the simulate run directory for a given root and
/// relative path.
///
/// E.g. `run_dir(Path::new("./output"), "abc12345/baseline-def45678/seed_42")`
/// → `./output/sims/abc12345/baseline-def45678/seed_42`.
///
/// Before 2026-04-19 this was `./output/runs/…`; the rename unifies
/// the path vocabulary with fits which live at `./output/fits/…`.
pub fn run_dir(root: &Path, relative: &str) -> PathBuf {
    root.join("sims").join(relative)
}

/// Does this run have a cached trajectory?
pub fn has_cached_traj(run_dir: &Path) -> bool {
    run_dir.join("traj.tsv").exists()
}

/// Obs directory for a given (obs_hash, obs_seed) pair, relative to a run dir.
pub fn obs_dir(run_dir: &Path, obs_hash: &str, obs_seed: u64) -> PathBuf {
    run_dir.join("obs").join(format!("{}-{}", &obs_hash[..8], obs_seed))
}

/// Does this run have cached obs for the given (obs_hash, obs_seed)?
/// Checks for the presence of `obs.json` as the marker (obs streams may
/// or may not be present depending on the model).
pub fn has_cached_obs(run_dir: &Path, obs_hash: &str, obs_seed: u64) -> bool {
    obs_dir(run_dir, obs_hash, obs_seed).join("obs.json").exists()
}

// ─── Metadata types ──────────────────────────────────────────────────────────

/// Metadata written to `obs/{obs_hash}-{obs_seed}/obs.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsMeta {
    pub obs_hash: String,
    pub obs_seed: u64,
    pub streams: Vec<String>,
    pub layout: ObsLayout,
    pub version: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObsLayout {
    /// Single wide TSV with all streams as columns.
    Wide,
    /// One TSV per stream (`{stream}.tsv`).
    PerStream,
}

// ─── Run buffer: accumulator for --cas trajectory bytes ────────────────────

/// `Rc<RefCell<Vec<u8>>>`-backed `Write` target for --cas mode. The
/// trajectory-emission code writes to a `Box<dyn Write>` target; using
/// `RunBuffer` lets the caller hold a reference to the underlying bytes
/// while the main loop writes through the trait object.
#[derive(Clone)]
pub struct RunBuffer {
    inner: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
}

impl RunBuffer {
    pub fn new() -> Self {
        RunBuffer { inner: std::rc::Rc::new(std::cell::RefCell::new(Vec::with_capacity(64 * 1024))) }
    }

    /// Snapshot the buffered bytes. Call after all writes complete.
    pub fn bytes(&self) -> Vec<u8> {
        self.inner.borrow().clone()
    }
}

impl std::io::Write for RunBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

// ─── Read/write helpers ──────────────────────────────────────────────────────

pub fn write_traj(run_dir: &Path, content: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    std::fs::write(run_dir.join("traj.tsv"), content)
}

pub fn read_traj(run_dir: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(run_dir.join("traj.tsv"))
}

pub fn write_obs_meta(obs_dir: &Path, meta: &ObsMeta) -> std::io::Result<()> {
    std::fs::create_dir_all(obs_dir)?;
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(obs_dir.join("obs.json"), json)
}

// ─── ISO-8601 timestamp helper ───────────────────────────────────────────────

/// Format a SystemTime as ISO 8601 UTC (e.g. "2026-04-16T14:23:11Z").
/// Pure stdlib, no external crate — keeps supply-chain surface zero for
/// this shared module.
pub fn iso8601_utc(t: std::time::SystemTime) -> String {
    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    // Days since 1970-01-01 (epoch day 0).
    let (year, month, day, hour, minute, second) = civil_from_secs(secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, hour, minute, second)
}

/// Convert a unix timestamp to civil date using the proleptic Gregorian
/// calendar. Adapted from Howard Hinnant's date algorithms
/// (https://howardhinnant.github.io/date_algorithms.html), public domain.
fn civil_from_secs(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let time = secs.rem_euclid(86400) as u32;
    let hour = time / 3600;
    let minute = (time % 3600) / 60;
    let second = time % 60;

    // days_from_civil inverse
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe/4 - yoe/100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d, hour, minute, second)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_path_format_is_stable() {
        let params: HashMap<String, f64> = HashMap::new();
        let path = run_path_relative(
            "abc12345000000000000000000000000000000000000000000000000",
            None,
            "baseline",
            &[],
            &[],
            &params,
            42,
        );
        // sim_hash 8-char prefix + scen_slug + scen_hash 8-char prefix + seed
        assert!(path.starts_with("abc12345/baseline-"));
        assert!(path.ends_with("/seed_42"));
    }

    #[test]
    fn run_path_slugifies_scenario() {
        let params: HashMap<String, f64> = HashMap::new();
        let sim = "a".repeat(64);
        let path = run_path_relative(&sim, None, "With SIA!", &[], &[], &params, 1);
        assert!(path.contains("/with_sia_-"), "got: {}", path);
    }

    #[test]
    fn run_path_uses_model_stem_prefix() {
        let params: HashMap<String, f64> = HashMap::new();
        let sim = "a".repeat(64);
        let path = run_path_relative(&sim, Some("sir_basic"), "baseline", &[], &[], &params, 1);
        assert!(path.starts_with("sir_basic-aaaaaaaa/"), "got: {}", path);
    }

    #[test]
    fn run_path_empty_stem_falls_back_to_hash_only() {
        let params: HashMap<String, f64> = HashMap::new();
        let sim = "b".repeat(64);
        let path = run_path_relative(&sim, Some(""), "baseline", &[], &[], &params, 1);
        assert!(path.starts_with("bbbbbbbb/"), "got: {}", path);
    }

    #[test]
    fn has_cached_traj_detects_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sims").join("abc/baseline-def/seed_1");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!has_cached_traj(&dir));
        std::fs::write(dir.join("traj.tsv"), "t\tS\n0\t100\n").unwrap();
        assert!(has_cached_traj(&dir));
    }

    #[test]
    fn iso8601_epoch() {
        let epoch = std::time::UNIX_EPOCH;
        assert_eq!(iso8601_utc(epoch), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_dates() {
        // 2026-04-16T00:00:00Z → 1776297600 unix seconds
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1776297600);
        assert_eq!(iso8601_utc(t), "2026-04-16T00:00:00Z");
        // 2000-01-01T00:00:00Z → 946684800 unix seconds
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(946684800);
        assert_eq!(iso8601_utc(t), "2000-01-01T00:00:00Z");
        // A leap-day: 2024-02-29T12:34:56Z
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1709210096);
        assert_eq!(iso8601_utc(t), "2024-02-29T12:34:56Z");
    }

    #[test]
    fn obs_dir_layout() {
        let run = Path::new("/tmp/sims/abc/baseline-def/seed_1");
        let od = obs_dir(run, "obsaaaa11111111000000000000000000000000000000000000000000000000", 99);
        assert_eq!(od, Path::new("/tmp/sims/abc/baseline-def/seed_1/obs/obsaaaa1-99"));
    }
}
