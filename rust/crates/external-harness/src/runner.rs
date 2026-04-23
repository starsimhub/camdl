//! End-to-end driver: load a case, run camdl, summarise, compare.

use crate::compare::{self, CheckResult, Outcome};
use crate::hashing;
use crate::manifest::{CaseManifest, ExpectedManifest, FixtureManifest, ReferenceSpec, SummarySpec};
use crate::subprocess;
use crate::summarise;
use crate::summary::Summary;
use std::path::{Path, PathBuf};

pub const HARNESS_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub struct CaseOutcome {
    pub name: String,
    pub status: Status,
    pub checks: Vec<CheckResult>,
    /// Where per-seed stdout/stderr and summary.tsv were written.
    pub run_dir: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Stale(String),
    Crash(String),
    ToleranceFail,
}

impl CaseOutcome {
    #[allow(dead_code)]
    pub fn is_pass(&self) -> bool { self.status == Status::Pass }
}

/// Run a single case's fast-path. Returns without invoking reference
/// scripts — staleness checks come first, then camdl runs, then compare.
pub fn run_case(case_dir: &Path) -> anyhow::Result<CaseOutcome> {
    let case: CaseManifest = load_toml(&case_dir.join("case.toml"))?;
    let expected: ExpectedManifest = load_toml(&case_dir.join("expected.toml"))?;
    validate_expected(&expected)?;

    let fixtures_dir = case_dir.join("fixtures");
    let fixture_manifest_path = fixtures_dir.join("MANIFEST.toml");
    let fixture_summary_path = fixtures_dir.join("summary.tsv");

    // Prepare run directory.
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let run_dir = case_dir.join("..").join("..").join("runs").join(&case.name).join(&ts);
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| anyhow::anyhow!("create {}: {}", run_dir.display(), e))?;

    // Staleness checks.
    let fixture: FixtureManifest = load_toml(&fixture_manifest_path)?;
    if let Some(msg) = check_staleness(&case, case_dir, &fixture)? {
        return Ok(CaseOutcome {
            name: case.name.clone(),
            status: Status::Stale(msg),
            checks: vec![],
            run_dir,
        });
    }

    // Camdl runs (skipped entirely for analytical cases where we only
    // want to test the reference fixture itself; analytical cases still
    // need a "camdl side" though, since the whole point is to compare
    // camdl's simulation to the closed form).
    let mut runs = Vec::with_capacity(case.camdl.n_seeds);
    for i in 0..case.camdl.n_seeds {
        let seed = case.camdl.seed_base + i as u64;
        let run = subprocess::run_camdl_seed(&case.camdl, case_dir, &run_dir, seed)?;
        if !run.succeeded() {
            return Ok(CaseOutcome {
                name: case.name.clone(),
                status: Status::Crash(format!(
                    "camdl exit={:?} on seed={} (see {})",
                    run.exit_code, run.seed, run.stderr_path.display())),
                checks: vec![],
                run_dir,
            });
        }
        runs.push(run);
    }

    // Summarise camdl output.
    let camdl_summary = match &case.summary {
        SummarySpec::Prebaked => {
            return Err(anyhow::anyhow!(
                "summary.kind = 'prebaked' is for the reference side only; \
                 case.toml must declare a summary spec for camdl's output"));
        }
        SummarySpec::EnsembleStats { .. } => summarise::summarise_runs(&case.summary, &runs)?,
    };
    camdl_summary.write_tsv(&run_dir.join("camdl_summary.tsv"))?;

    let reference_summary = Summary::read_tsv(&fixture_summary_path)?;

    // Compare.
    let checks = compare::run_all(&expected, &camdl_summary, &reference_summary);
    let any_fail = checks.iter().any(|c| c.outcome == Outcome::Fail);
    let status = if any_fail { Status::ToleranceFail } else { Status::Pass };

    Ok(CaseOutcome { name: case.name.clone(), status, checks, run_dir })
}

fn load_toml<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {}", path.display(), e))?;
    toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("parse {}: {}", path.display(), e))
}

fn validate_expected(m: &ExpectedManifest) -> anyhow::Result<()> {
    // Principle #5: rationale required. Catch at load time so a case
    // author can't merge a check with an empty rationale.
    for (stat, check) in &m.checks {
        if check.rationale.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "expected.toml check '{}' has empty rationale — rationale is required \
                 and must include a Monte Carlo power statement (see proposal #5)",
                stat));
        }
    }
    Ok(())
}

fn check_staleness(
    case: &CaseManifest,
    case_dir: &Path,
    fixture: &FixtureManifest,
) -> anyhow::Result<Option<String>> {
    // 1. harness_version
    if fixture.harness_version != HARNESS_VERSION {
        return Ok(Some(format!(
            "harness version changed: fixture generated by {}, current {}",
            fixture.harness_version, HARNESS_VERSION)));
    }

    // 2. reference_sha
    let current_ref_sha = compute_reference_sha(&case.reference, case_dir)?;
    if current_ref_sha != fixture.reference_sha {
        return Ok(Some(format!(
            "reference script/directory changed:\n  \
             MANIFEST.reference_sha = {} (fixture)\n  \
             current                = {}\n  \
             Re-run with CAMDL_REGEN_EXTERNAL=1 to regenerate.",
            fixture.reference_sha, current_ref_sha)));
    }

    // 3. case_sha (model + params + case.toml + expected.toml)
    let current_case_sha = compute_case_sha(case, case_dir)?;
    if current_case_sha != fixture.case_sha {
        return Ok(Some(format!(
            "model/params/manifest changed since fixture generation:\n  \
             MANIFEST.case_sha = {} (fixture)\n  \
             current           = {}\n  \
             Re-run with CAMDL_REGEN_EXTERNAL=1 to regenerate.",
            fixture.case_sha, current_case_sha)));
    }

    Ok(None)
}

pub fn compute_reference_sha(spec: &ReferenceSpec, case_dir: &Path) -> anyhow::Result<String> {
    match spec {
        ReferenceSpec::Analytical { derivation } => {
            hashing::sha256_file(&case_dir.join(derivation))
        }
        ReferenceSpec::RPomp { run, fingerprint_dir }
        | ReferenceSpec::PyNumpyro { run, fingerprint_dir }
        | ReferenceSpec::Stan { run, fingerprint_dir } => {
            let target = fingerprint_dir
                .clone()
                .unwrap_or_else(|| run.parent().unwrap_or_else(|| Path::new(".")).to_path_buf());
            hashing::sha256_dir(&case_dir.join(target))
        }
    }
}

pub fn compute_case_sha(case: &CaseManifest, case_dir: &Path) -> anyhow::Result<String> {
    let model_path = case_dir.join(&case.camdl.model);
    let case_toml_path = case_dir.join("case.toml");
    let expected_toml_path = case_dir.join("expected.toml");
    let params_path = case.camdl.params.as_ref().map(|p| case_dir.join(p));

    let mut paths: Vec<PathBuf> = vec![model_path, case_toml_path, expected_toml_path];
    if let Some(p) = params_path {
        if p.exists() { paths.push(p); }
    }
    let refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
    hashing::sha256_files(&refs)
}
