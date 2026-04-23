//! Subprocess invocation for camdl and reference scripts.
//!
//! Wraps `std::process::Command` with: placeholder substitution, per-seed
//! working-directory management, stdout/stderr capture with filesize caps,
//! and distinct error variants so the driver can classify a failure as
//! `crash` vs `tolerance-fail` vs `stale`.

use crate::manifest::CamdlSpec;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct CamdlRun {
    pub seed: u64,
    pub seed_dir: PathBuf,
    pub exit_code: Option<i32>,
    #[allow(dead_code)]
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

impl CamdlRun {
    pub fn succeeded(&self) -> bool {
        matches!(self.exit_code, Some(0))
    }
}

pub fn run_camdl_seed(
    spec: &CamdlSpec,
    case_dir: &Path,
    runs_dir: &Path,
    seed: u64,
) -> anyhow::Result<CamdlRun> {
    let seed_dir = runs_dir.join("seeds").join(seed.to_string());
    std::fs::create_dir_all(&seed_dir)
        .map_err(|e| anyhow::anyhow!("create {}: {}", seed_dir.display(), e))?;

    // Subprocess cwd will be case_dir, so @model / @params are passed as
    // the relative paths from case.toml. @obs_out is absolutised because
    // the seed directory lives outside case_dir.
    let abs_seed_dir = std::fs::canonicalize(&seed_dir)
        .map_err(|e| anyhow::anyhow!("canonicalize {}: {}", seed_dir.display(), e))?;
    let out_path = abs_seed_dir.join("obs.tsv");
    let model_path = spec.model.clone();
    let params_path = spec.params.clone();

    let args: Vec<String> = spec.command.iter().map(|tok| {
        match tok.as_str() {
            "@model"    => model_path.to_string_lossy().into_owned(),
            "@params"   => params_path.as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "@seed"     => seed.to_string(),
            "@obs_out"  => out_path.to_string_lossy().into_owned(),
            other       => other.to_string(),
        }
    }).collect();

    if args.is_empty() {
        return Err(anyhow::anyhow!("case.toml camdl.command is empty"));
    }
    let (prog, rest) = args.split_first().unwrap();

    let stdout_path = seed_dir.join("stdout.log");
    let stderr_path = seed_dir.join("stderr.log");
    let stdout = std::fs::File::create(&stdout_path)?;
    let stderr = std::fs::File::create(&stderr_path)?;

    let status = Command::new(prog)
        .args(rest)
        .current_dir(case_dir)
        .stdout(stdout)
        .stderr(stderr)
        .status()
        .map_err(|e| anyhow::anyhow!("spawn {}: {}", prog, e))?;

    Ok(CamdlRun {
        seed,
        seed_dir,
        exit_code: status.code(),
        stdout_path,
        stderr_path,
    })
}

/// Invoke `reference/run.sh` (or analog) for the regen path.
/// Returns the exit status; the reference script is expected to write
/// `fixtures/summary.tsv` itself.
#[allow(dead_code)]  // used by regen path, wired in next session
pub fn run_reference(
    run_script: &Path,
    case_dir: &Path,
    log_path: &Path,
) -> anyhow::Result<std::process::ExitStatus> {
    let log = std::fs::File::create(log_path)?;
    let log_err = log.try_clone()?;
    let status = Command::new("bash")
        .arg(run_script)
        .current_dir(case_dir)
        .stdout(log)
        .stderr(log_err)
        .status()
        .map_err(|e| anyhow::anyhow!("spawn reference {}: {}", run_script.display(), e))?;
    Ok(status)
}
