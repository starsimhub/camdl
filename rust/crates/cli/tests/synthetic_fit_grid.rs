//! End-to-end tests for the replicate-grid machinery in
//! `camdl fit run`. Encodes the canonical modes from
//! docs/dev/proposals/2026-04-17-synthetic-fit-replicates.md.
//!
//! Shells out to the built `camdl-sim` binary; skipped silently when
//! the release binary or `camdlc.exe` isn't present so the suite
//! stays runnable in rust-only CI and when tests run before a build.

use std::path::{Path, PathBuf};
use std::process::Command;

fn camdl_sim() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../target/release/camdl-sim");
    if p.exists() { Some(p) } else { None }
}

fn camdlc() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../../ocaml/_build/default/bin/camdlc.exe");
    if p.exists() { Some(p) } else { None }
}

struct TempDir(PathBuf);
impl TempDir { fn path(&self) -> &Path { &self.0 } }
impl Drop for TempDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn tempdir(tag: &str) -> TempDir {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let base = std::env::temp_dir().join(format!(
        "camdl_gridtest_{}_{}_{}", tag, std::process::id(), ns));
    std::fs::create_dir_all(&base).unwrap();
    TempDir(base)
}

/// Minimal SIR with Poisson prevalence obs, fit config skeleton.
fn write_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let camdl = camdlc().expect("camdlc.exe");
    let src = r#"
time_unit = 'days
compartments { S, I, R }
parameters {
  beta  : rate  in [0.001, 5.0]
  gamma : rate  in [0.01, 1.0]
  N0    : count in [100, 10000]
}
transitions {
  infection : S --> I @ beta * S * I / N0
  recovery  : I --> R @ gamma * I
}
observations {
  cases : {
    projected  = prevalence(I)
    every      = 1 'days
    likelihood = poisson(rate = projected)
  }
}
init { S = 999  I = 1 }
simulate { from = 0 'days  to = 10 'days }
"#;
    let model_path = dir.join("sir.camdl");
    std::fs::write(&model_path, src).unwrap();
    let ir_path = dir.join("sir.ir.json");
    let output = Command::new(&camdl).arg(&model_path).output().unwrap();
    assert!(output.status.success(),
        "camdlc failed: {}", String::from_utf8_lossy(&output.stderr));
    std::fs::write(&ir_path, &output.stdout).unwrap();

    let truth_path = dir.join("truth.toml");
    std::fs::write(&truth_path, "beta = 0.8\ngamma = 0.3\nN0 = 1000\n").unwrap();

    (ir_path, truth_path)
}

fn stages_block() -> &'static str {
    // Deliberately cheap — we're testing the grid structure, not
    // convergence. One stage, very few iterations, tiny particle
    // count so a 2×2 grid finishes in seconds.
    r#"
[estimate]
beta  = { bounds = [0.01, 5.0], start = 1.0 }
gamma = { bounds = [0.01, 1.0], start = 0.3 }

[fixed]
N0 = 1000

[stages.mle]
method = "if2"
chains = 2
particles = 100
iterations = 5
cooling = 0.7
"#
}

fn run_fit(bin: &Path, fit_toml: &Path) {
    let status = Command::new(bin)
        .arg("fit").arg("run")
        .arg(fit_toml)
        .status()
        .expect("camdl-sim fit run must invoke");
    assert!(status.success(), "fit run failed for {}", fit_toml.display());
}

// ── mode 1: single fit lives under real/fit_<seed>/ ────────────────────
#[test]
fn single_fit_lives_under_real_fit_seed_dir() {
    let Some(bin) = camdl_sim() else { return; };
    if camdlc().is_none() { return; }
    let tmp = tempdir("single");
    let (ir, _) = write_fixture(tmp.path());
    let out = tmp.path().join("out");
    let data_tsv = tmp.path().join("cases.tsv");
    std::fs::write(&data_tsv, "time\tcases\n1\t5\n2\t7\n3\t12\n4\t18\n5\t25\n6\t30\n7\t28\n8\t22\n9\t15\n10\t10\n").unwrap();
    let fit_toml = tmp.path().join("fit.toml");
    std::fs::write(&fit_toml, format!(r#"
output_dir = "{}"

[model]
camdl = "{}"

[data.observations]
cases = "{}"
{}
"#, out.display(), ir.display(), data_tsv.display(), stages_block())).unwrap();

    run_fit(&bin, &fit_toml);

    let expected = out.join("fits").join("fit").join("real").join("fit_1").join("mle");
    assert!(expected.exists(),
        "single fit must land at real/fit_1/mle/, not found at {}", expected.display());
    // No flat stage dir at the top level.
    assert!(!out.join("fits").join("fit").join("mle").exists(),
        "flat top-level stage dir must NOT exist");
}

// ── mode 2: fit_seeds list → one dir per seed under real/ ──────────────
#[test]
fn fit_seeds_list_produces_per_seed_dirs() {
    let Some(bin) = camdl_sim() else { return; };
    if camdlc().is_none() { return; }
    let tmp = tempdir("list");
    let (ir, _) = write_fixture(tmp.path());
    let out = tmp.path().join("out");
    let data_tsv = tmp.path().join("cases.tsv");
    std::fs::write(&data_tsv, "time\tcases\n1\t5\n2\t7\n3\t12\n4\t18\n5\t25\n6\t30\n7\t28\n8\t22\n9\t15\n10\t10\n").unwrap();
    let fit_toml = tmp.path().join("fit.toml");
    std::fs::write(&fit_toml, format!(r#"
output_dir = "{}"
fit_seeds = [11, 22, 33]

[model]
camdl = "{}"

[data.observations]
cases = "{}"
{}
"#, out.display(), ir.display(), data_tsv.display(), stages_block())).unwrap();

    run_fit(&bin, &fit_toml);

    let base = out.join("fits").join("fit").join("real");
    for s in [11u64, 22, 33] {
        let dir = base.join(format!("fit_{}", s)).join("mle");
        assert!(dir.exists(), "fit_{}/mle/ must exist at {}", s, dir.display());
    }
    let summary = out.join("fits").join("fit").join("real").join("summary.tsv");
    assert!(summary.exists(), "real/summary.tsv must be written");
    let text = std::fs::read_to_string(&summary).unwrap();
    // Header + 3 rows = 4 lines.
    assert!(text.lines().count() >= 4,
        "summary.tsv should have at least 3 data rows: {}", text);
}

// ── mode 3: synthetic generation — N datasets, synthetic/ prefix ───────
#[test]
fn synthetic_generates_n_datasets_and_fits() {
    let Some(bin) = camdl_sim() else { return; };
    if camdlc().is_none() { return; }
    let tmp = tempdir("syn");
    let (ir, truth) = write_fixture(tmp.path());
    let out = tmp.path().join("out");
    let fit_toml = tmp.path().join("fit.toml");
    std::fs::write(&fit_toml, format!(r#"
output_dir = "{}"

[model]
camdl = "{}"

[synthetic]
true_params = "{}"
sim_seeds = [1, 2, 3]
{}
"#, out.display(), ir.display(), truth.display(), stages_block())).unwrap();

    run_fit(&bin, &fit_toml);

    let syn = out.join("fits").join("fit").join("synthetic");
    for i in 1..=3 {
        let ds_tsv = syn.join("data").join(format!("ds_{:02}.tsv", i));
        assert!(ds_tsv.exists(), "ds_{:02}.tsv must exist", i);
        let fit_dir = syn.join(format!("ds_{:02}", i)).join("fit_1").join("mle");
        assert!(fit_dir.exists(), "synthetic/ds_{:02}/fit_1/mle/ must exist", i);
    }
    assert!(syn.join("truth.toml").exists(),
        "truth.toml must be copied for provenance");
    assert!(syn.join("summary.tsv").exists());
    assert!(syn.join("coverage.tsv").exists());
}

// ── mode 4: synthetic × fit_seeds full matrix ─────────────────────────
#[test]
fn synthetic_and_fit_seeds_full_matrix() {
    let Some(bin) = camdl_sim() else { return; };
    if camdlc().is_none() { return; }
    let tmp = tempdir("matrix");
    let (ir, truth) = write_fixture(tmp.path());
    let out = tmp.path().join("out");
    let fit_toml = tmp.path().join("fit.toml");
    std::fs::write(&fit_toml, format!(r#"
output_dir = "{}"
fit_seeds = [1, 2]

[model]
camdl = "{}"

[synthetic]
true_params = "{}"
sim_seeds = [10, 20]
{}
"#, out.display(), ir.display(), truth.display(), stages_block())).unwrap();

    run_fit(&bin, &fit_toml);

    let syn = out.join("fits").join("fit").join("synthetic");
    for ds in 1..=2 {
        for fs in [1u64, 2] {
            let p = syn.join(format!("ds_{:02}", ds))
                .join(format!("fit_{}", fs))
                .join("mle");
            assert!(p.exists(), "cell ds_{:02} × fit_{} must exist at {}",
                ds, fs, p.display());
        }
    }
    let summary = std::fs::read_to_string(syn.join("summary.tsv")).unwrap();
    // Header + 4 data rows.
    assert!(summary.lines().count() >= 5,
        "summary.tsv should have 4 rows for 2×2 grid: {}", summary);
}

// ── mode 5: [data] + [synthetic] errors cleanly ───────────────────────
#[test]
fn data_and_synthetic_errors_cleanly() {
    let Some(bin) = camdl_sim() else { return; };
    if camdlc().is_none() { return; }
    let tmp = tempdir("mutex");
    let (ir, truth) = write_fixture(tmp.path());
    let out = tmp.path().join("out");
    let data_tsv = tmp.path().join("cases.tsv");
    std::fs::write(&data_tsv, "time\tcases\n1\t5\n").unwrap();
    let fit_toml = tmp.path().join("fit.toml");
    std::fs::write(&fit_toml, format!(r#"
output_dir = "{}"

[model]
camdl = "{}"

[data.observations]
cases = "{}"

[synthetic]
true_params = "{}"
sim_seeds = [1]
{}
"#, out.display(), ir.display(), data_tsv.display(), truth.display(), stages_block())).unwrap();

    let output = Command::new(&bin).arg("fit").arg("run")
        .arg(&fit_toml).output().unwrap();
    assert!(!output.status.success(),
        "[data]+[synthetic] must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[data]") && stderr.contains("[synthetic]"),
        "error must name both blocks: {}", stderr);
}
