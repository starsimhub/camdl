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

/// Build the canonical relative run path: `{sim_hash[:8]}/{scenario_slug}-{scen_hash[:8]}/seed_{n}`.
///
/// `sim_hash` must be the full 64-char hex string from `hashing::sim_hash`.
/// The enable/disable/params trio is hashed here so the caller doesn't have
/// to precompute scen_hash separately.
pub fn run_path_relative(
    sim_hash: &str,
    scenario_name: &str,
    enable: &[String],
    disable: &[String],
    params: &HashMap<String, f64>,
    seed: u64,
) -> String {
    let sh = scen_hash(enable, disable, params);
    format!("{}/{}-{}/seed_{}", &sim_hash[..8], slug(scenario_name), &sh[..8], seed)
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

/// Metadata written to `run.json` inside each `seed_{n}/` directory.
///
/// `argv` records the full original command line for reproducibility —
/// `camdl show <path>` prints it back so the user knows exactly what
/// produced the cached output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    /// Model file path or name (for display; not a hash input).
    pub model: String,
    pub model_hash: String,
    pub scenario: String,
    pub sim_hash: String,
    pub scen_hash: String,
    pub seed: u64,
    pub backend: String,
    pub dt: f64,
    /// Runtime version at write time (`version::VERSION_SHORT`).
    pub version: String,
    /// ISO 8601 UTC timestamp at completion.
    pub created_at: String,
    /// Original argv that produced this run.
    pub argv: Vec<String>,
    /// Sweep parameter values applied to this run (empty for non-sweep
    /// runs, including all single-run `--cas` invocations). Populated by
    /// `--batch` when a `[sweep]` block is active. Without this, the
    /// scen_hash is opaque — analysis can't recover which param values
    /// produced which trajectory.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sweep_point: HashMap<String, f64>,
}

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

pub fn write_run_meta(run_dir: &Path, meta: &RunMeta) -> std::io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(run_dir.join("run.json"), json)
}

pub fn read_run_meta(run_dir: &Path) -> std::io::Result<RunMeta> {
    let contents = std::fs::read_to_string(run_dir.join("run.json"))?;
    serde_json::from_str(&contents)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
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
        let path = run_path_relative(&sim, "With SIA!", &[], &[], &params, 1);
        assert!(path.contains("/with_sia_-"), "got: {}", path);
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
    fn run_meta_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = RunMeta {
            model: "sir.camdl".into(),
            model_hash: "abc".into(),
            scenario: "baseline".into(),
            sim_hash: "def".into(),
            scen_hash: "123".into(),
            seed: 42,
            backend: "gillespie".into(),
            dt: 1.0,
            version: "0.1.0+aaa".into(),
            created_at: "2026-04-16T00:00:00Z".into(),
            argv: vec!["camdl".into(), "simulate".into(), "sir.camdl".into(), "--seed".into(), "42".into(), "--cas".into()],
            sweep_point: HashMap::new(),
        };
        write_run_meta(tmp.path(), &meta).unwrap();
        let read = read_run_meta(tmp.path()).unwrap();
        assert_eq!(read.seed, 42);
        assert_eq!(read.scenario, "baseline");
        assert_eq!(read.argv.len(), 6);
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
