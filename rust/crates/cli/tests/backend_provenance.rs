//! End-to-end tests for the backend-provenance guardrail.
//!
//! See `docs/dev/proposals/2026-04-19-backend-provenance-guardrail.md`
//! and the originating incident
//! `docs/dev/incidents/2026-04-19-backend-default-mismatch.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../target/release/camdl-sim")
}

fn skip_if_missing() -> Option<PathBuf> {
    let b = binary();
    if !b.exists() {
        eprintln!("skipping: binary not built at {}", b.display());
        return None;
    }
    Some(b)
}

fn golden_pure_death() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../../ir/golden/pure_death.ir.json")
}

/// Write a minimal mle_params.toml with a `[provenance]` block claiming
/// the fit used `backend` at `dt=1.0`. Top-level params come first
/// (required by TOML so they stay at the file's top level, not scoped
/// under the [provenance] table) — matches what `write_mle_params`
/// produces in production.
fn write_fake_mle(path: &Path, backend: &str, dt: f64) {
    let contents = format!(r#"mu = 0.05

[provenance]
camdl_version = "test"
timestamp = "2026-04-19T00:00:00Z"
content_hash = "deadbeef"
backend = "{}"
dt = {}
model = "pure_death.ir.json"
model_hash = "f00d"
seed = 1
stage = "refine"
chain = 1
log_likelihood = -22.0
loglik_sd = 0.0
n_particles = 500
"#, backend, dt);
    std::fs::write(path, contents).unwrap();
}

#[test]
fn simulate_auto_matches_fit_backend_when_backend_not_passed() {
    let Some(bin) = skip_if_missing() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let mle = tmp.path().join("mle.toml");
    write_fake_mle(&mle, "chain_binomial", 1.0);

    let out = Command::new(&bin)
        .args(["simulate", &golden_pure_death().to_string_lossy(),
               "--params", &mle.to_string_lossy(),
               "--replicates", "2", "--seed", "1",
               "-o", &tmp.path().join("t.tsv").to_string_lossy()])
        .output().expect("spawn");
    assert!(out.status.success(), "expected success; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("backend auto-matched"),
        "expected auto-match info log, got: {}", stderr);
    assert!(stderr.contains("chain_binomial"),
        "info log should name the fit's backend: {}", stderr);
}

#[test]
fn simulate_warns_on_explicit_backend_mismatch() {
    let Some(bin) = skip_if_missing() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let mle = tmp.path().join("mle.toml");
    write_fake_mle(&mle, "chain_binomial", 1.0);

    let out = Command::new(&bin)
        .args(["simulate", &golden_pure_death().to_string_lossy(),
               "--params", &mle.to_string_lossy(),
               "--replicates", "2", "--seed", "1",
               "--backend", "gillespie",
               "-o", &tmp.path().join("t.tsv").to_string_lossy()])
        .output().expect("spawn");
    assert!(out.status.success(),
        "simulate should proceed despite mismatch warning; stderr: {}",
        String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("backend mismatch"),
        "expected warning on mismatch, got: {}", stderr);
    assert!(stderr.contains("chain_binomial") && stderr.contains("gillespie"),
        "warning should name both backends: {}", stderr);
    assert!(stderr.contains("incidents/2026-04-19-backend-default-mismatch"),
        "warning should cite the incident file: {}", stderr);
}

#[test]
fn simulate_silent_on_matching_explicit_backend() {
    let Some(bin) = skip_if_missing() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let mle = tmp.path().join("mle.toml");
    write_fake_mle(&mle, "chain_binomial", 1.0);

    let out = Command::new(&bin)
        .args(["simulate", &golden_pure_death().to_string_lossy(),
               "--params", &mle.to_string_lossy(),
               "--replicates", "2", "--seed", "1",
               "--backend", "chain_binomial", "--dt", "1.0",
               "-o", &tmp.path().join("t.tsv").to_string_lossy()])
        .output().expect("spawn");
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("backend auto-matched"),
        "matching explicit backend should not emit auto-match log: {}", stderr);
    assert!(!stderr.contains("backend mismatch"),
        "matching explicit backend should not emit warning: {}", stderr);
}

#[test]
fn simulate_unchanged_on_standalone_params_file() {
    let Some(bin) = skip_if_missing() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let standalone = tmp.path().join("p.toml");
    // No [provenance] block — should behave as pre-guardrail.
    std::fs::write(&standalone, "mu = 0.05\n").unwrap();

    let out = Command::new(&bin)
        .args(["simulate", &golden_pure_death().to_string_lossy(),
               "--params", &standalone.to_string_lossy(),
               "--replicates", "2", "--seed", "1",
               "-o", &tmp.path().join("t.tsv").to_string_lossy()])
        .output().expect("spawn");
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("backend auto-matched"),
        "standalone params should not trigger auto-match log");
    assert!(!stderr.contains("backend mismatch"),
        "standalone params should not trigger warning");
}

#[test]
fn simulate_records_from_fit_hash_in_run_json() {
    // When simulate runs with --params pointing at a fit MLE, and --cas
    // is active, the resulting run.json should record `from_fit_hash`
    // matching the fit's hash. Closes the sim → fit provenance edge.
    let Some(bin) = skip_if_missing() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let mle = tmp.path().join("mle.toml");
    // Use a distinctive fit_hash so we can grep for it.
    let fit_hash = "abc12345deadbeef00000000000000000000000000000000000000000000abcd";
    let contents = format!(r#"mu = 0.05

[provenance]
camdl_version = "test"
timestamp = "2026-04-19T00:00:00Z"
content_hash = "deadbeef"
fit_hash = "{}"
backend = "chain_binomial"
dt = 1.0
model = "pure_death.ir.json"
model_hash = "f00d"
seed = 1
stage = "refine"
chain = 1
log_likelihood = -22.0
loglik_sd = 0.0
n_particles = 500
"#, fit_hash);
    std::fs::write(&mle, contents).unwrap();

    let output = tmp.path().join("out");
    let status = Command::new(&bin)
        .args(["simulate", &golden_pure_death().to_string_lossy(),
               "--params", &mle.to_string_lossy(),
               "--seed", "1", "--cas",
               "--output-dir", &output.to_string_lossy(),
               "-o", &tmp.path().join("t.tsv").to_string_lossy()])
        .status().expect("spawn");
    assert!(status.success());

    // Locate the run.json under output/sims/...
    let run_jsons: Vec<_> = walkdir(&output.join("sims")).into_iter()
        .filter(|p| p.file_name().map(|s| s == "run.json").unwrap_or(false))
        .collect();
    assert_eq!(run_jsons.len(), 1, "expected one sim run, got {}",
        run_jsons.len());
    let body = std::fs::read_to_string(&run_jsons[0]).unwrap();
    assert!(body.contains(fit_hash),
        "run.json must record from_fit_hash; got: {}", body);
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p.clone()); }
                else { out.push(p); }
            }
        }
    }
    out
}
