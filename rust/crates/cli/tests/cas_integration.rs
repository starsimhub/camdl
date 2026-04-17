//! End-to-end tests for `camdl simulate --cas` and `camdl list/show/cat`.
//!
//! These shell out to the built `camdl-sim` binary in `target/release/`
//! and exercise the full pipeline: real hash computation, real cache
//! lookups, real directory writes.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the release-built binary. Tests assume a prior
/// `cargo build --release -p cli`, which happens automatically before
/// `cargo test` in CI. Skips the test if the binary is absent.
fn binary() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let bin = Path::new(&manifest)
        .join("../../target/release/camdl-sim");
    bin
}

/// A golden IR with a baseline scenario that sets beta/gamma/N0/I0 —
/// suitable for `--cas --scenario baseline --seed N`.
fn golden_sir_basic() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../../ocaml/golden/sir_basic.ir.json")
}

fn skip_if_missing_binary() -> Option<PathBuf> {
    let bin = binary();
    if !bin.exists() {
        eprintln!("skipping: camdl-sim binary not built at {}", bin.display());
        return None;
    }
    Some(bin)
}

#[test]
fn cas_first_run_writes_cache_and_metadata() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("output");

    let status = Command::new(&bin)
        .args(["simulate", &golden_sir_basic().to_string_lossy(),
               "--scenario", "baseline",
               "--seed", "42",
               "--cas",
               "--output-dir", &output.to_string_lossy(),
               "-o", &tmp.path().join("traj.tsv").to_string_lossy()])
        .status()
        .expect("spawn");
    assert!(status.success(), "first --cas run should succeed");

    // Exactly one CAS entry under runs/
    let runs = output.join("runs");
    assert!(runs.exists(), "runs/ directory should exist");
    let seed_dirs: Vec<_> = walkdir(&runs).into_iter()
        .filter(|p| p.join("run.json").exists())
        .collect();
    assert_eq!(seed_dirs.len(), 1, "should have exactly one run dir");

    let dir = &seed_dirs[0];
    assert!(dir.join("traj.tsv").exists(), "traj.tsv should be written");
    assert!(dir.join("run.json").exists(), "run.json should be written");

    // run.json should have full metadata including argv + version
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("run.json")).unwrap()
    ).unwrap();
    assert_eq!(meta["seed"], 42);
    assert_eq!(meta["scenario"], "baseline");
    assert!(meta["sim_hash"].as_str().unwrap().len() == 64);
    assert!(meta["argv"].as_array().unwrap().len() >= 4);
    assert!(meta["version"].as_str().unwrap().contains("+"),
        "version should include git hash suffix");
}

#[test]
fn cas_second_identical_run_is_cache_hit() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("output");

    let run_once = || {
        Command::new(&bin)
            .args(["simulate", &golden_sir_basic().to_string_lossy(),
                   "--scenario", "baseline",
                   "--seed", "42",
                   "--cas",
                   "--output-dir", &output.to_string_lossy(),
                   "-o", &tmp.path().join("traj.tsv").to_string_lossy()])
            .output()
            .expect("spawn")
    };

    let first = run_once();
    assert!(first.status.success());
    let stderr1 = String::from_utf8_lossy(&first.stderr);
    assert!(stderr1.contains("cached:"), "first run stderr should say 'cached:': {}", stderr1);

    // Wait long enough that the filesystem mtime would differ if rewritten
    let cache_path = walkdir(&output.join("runs")).into_iter()
        .find(|p| p.join("traj.tsv").exists()).unwrap()
        .join("traj.tsv");
    let mtime1 = std::fs::metadata(&cache_path).unwrap().modified().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(10));

    let second = run_once();
    assert!(second.status.success());
    let stderr2 = String::from_utf8_lossy(&second.stderr);
    assert!(stderr2.contains("cache hit:"),
        "second run stderr should say 'cache hit:': {}", stderr2);

    // mtime unchanged — second run must not have re-written the file
    let mtime2 = std::fs::metadata(&cache_path).unwrap().modified().unwrap();
    assert_eq!(mtime1, mtime2, "cache hit must not overwrite traj.tsv");
}

#[test]
fn cas_different_seed_new_cache_entry() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("output");

    for seed in ["42", "43"] {
        let st = Command::new(&bin)
            .args(["simulate", &golden_sir_basic().to_string_lossy(),
                   "--scenario", "baseline",
                   "--seed", seed,
                   "--cas",
                   "--output-dir", &output.to_string_lossy(),
                   "-o", &tmp.path().join("traj.tsv").to_string_lossy()])
            .status().expect("spawn");
        assert!(st.success());
    }

    let dirs: Vec<_> = walkdir(&output.join("runs")).into_iter()
        .filter(|p| p.join("run.json").exists()).collect();
    assert_eq!(dirs.len(), 2, "should have two separate seed dirs");

    let seeds: Vec<String> = dirs.iter()
        .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(seeds.iter().any(|n| n == "seed_42"));
    assert!(seeds.iter().any(|n| n == "seed_43"));
}

#[test]
fn cas_rejects_multi_seeds() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();

    let out = Command::new(&bin)
        .args(["simulate", &golden_sir_basic().to_string_lossy(),
               "--scenario", "baseline",
               "--seeds", "1:3",
               "--cas",
               "--output-dir", &tmp.path().to_string_lossy()])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "multi-seed + --cas should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--cas supports single runs only"),
        "error should name the limitation: {}", stderr);
    assert!(stderr.contains("--batch"),
        "error should hint at --batch: {}", stderr);
}

#[test]
fn list_shows_cached_runs() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("output");

    // Cache two runs
    for seed in ["42", "99"] {
        Command::new(&bin)
            .args(["simulate", &golden_sir_basic().to_string_lossy(),
                   "--scenario", "baseline",
                   "--seed", seed,
                   "--cas",
                   "--output-dir", &output.to_string_lossy(),
                   "-o", &tmp.path().join("traj.tsv").to_string_lossy()])
            .status().expect("spawn");
    }

    // `camdl list` should find both
    let out = Command::new(&bin)
        .args(["list", &output.to_string_lossy()])
        .output().expect("spawn");
    assert!(out.status.success(), "list should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("seed_42"), "list should include seed_42: {}", stdout);
    assert!(stdout.contains("seed_99"), "list should include seed_99: {}", stdout);
    assert!(stdout.contains("baseline"), "list should show scenario name");
}

#[test]
fn cat_emits_cached_trajectory() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("output");

    Command::new(&bin)
        .args(["simulate", &golden_sir_basic().to_string_lossy(),
               "--scenario", "baseline",
               "--seed", "42",
               "--cas",
               "--output-dir", &output.to_string_lossy(),
               "-o", &tmp.path().join("traj.tsv").to_string_lossy()])
        .status().expect("spawn");

    // Find the cached dir, derive short hash
    let dir = walkdir(&output.join("runs")).into_iter()
        .find(|p| p.join("run.json").exists()).unwrap();
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("run.json")).unwrap()
    ).unwrap();
    let sim_hash_full = meta["sim_hash"].as_str().unwrap();
    let short = &sim_hash_full[..8];

    // `camdl cat <short>` uniquely resolves and emits the TSV
    let out = Command::new(&bin)
        .args(["cat", short, &output.to_string_lossy()])
        .output().expect("spawn");
    assert!(out.status.success(), "cat short-hash should resolve uniquely");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("# camdl"), "cat should emit trajectory header");
    assert!(stdout.contains("\tS\t") || stdout.contains("\tI\t"),
        "cat should include compartment columns");

    // Cached trajectory bytes should match the stdout of `cat`
    let cached = std::fs::read(dir.join("traj.tsv")).unwrap();
    assert_eq!(out.stdout, cached, "cat output must match cached bytes byte-for-byte");
}

#[test]
fn show_prints_metadata() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("output");

    Command::new(&bin)
        .args(["simulate", &golden_sir_basic().to_string_lossy(),
               "--scenario", "baseline",
               "--seed", "42",
               "--cas",
               "--output-dir", &output.to_string_lossy(),
               "-o", &tmp.path().join("traj.tsv").to_string_lossy()])
        .status().expect("spawn");

    let dir = walkdir(&output.join("runs")).into_iter()
        .find(|p| p.join("run.json").exists()).unwrap();
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("run.json")).unwrap()
    ).unwrap();
    let short = &meta["sim_hash"].as_str().unwrap()[..8];

    let out = Command::new(&bin)
        .args(["show", short, &output.to_string_lossy()])
        .output().expect("spawn");
    assert!(out.status.success(), "show should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Check that show emits the key fields
    assert!(stdout.contains("baseline"), "should show scenario");
    assert!(stdout.contains("42"), "should show seed");
    assert!(stdout.contains("gillespie"), "should show backend");
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Collect all directory paths under `root` (non-recursive children of
/// each level; bounded depth is fine for our 3-level CAS layout).
fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() { return out; }
    let Ok(entries) = std::fs::read_dir(root) else { return out; };
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() { continue; }
        // Recurse up to ~3 levels: sim_hash / scen-hash / seed_n
        if let Ok(entries2) = std::fs::read_dir(&p) {
            for e2 in entries2.flatten() {
                let p2 = e2.path();
                if !p2.is_dir() { continue; }
                if let Ok(entries3) = std::fs::read_dir(&p2) {
                    for e3 in entries3.flatten() {
                        let p3 = e3.path();
                        if p3.is_dir() { out.push(p3); }
                    }
                }
            }
        }
    }
    out
}
