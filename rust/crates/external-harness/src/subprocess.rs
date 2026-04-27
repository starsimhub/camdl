//! Subprocess invocation for camdl and reference scripts.
//!
//! Wraps `std::process::Command` with: placeholder substitution, per-seed
//! working-directory management, stdout/stderr capture with filesize caps,
//! and distinct error variants so the driver can classify a failure as
//! `crash` vs `tolerance-fail` vs `stale`.

use crate::manifest::CamdlSpec;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One batch-mode invocation's record — camdl runs once, writes a
/// long-format TSV containing all replicates, harness then summarises
/// via `summarise::summarise_long_tsv`.
#[derive(Debug, Clone)]
pub struct BatchRun {
    pub output_tsv: PathBuf,
    pub exit_code: Option<i32>,
    pub stderr_path: PathBuf,
}

impl BatchRun {
    pub fn succeeded(&self) -> bool { matches!(self.exit_code, Some(0)) }
}

pub fn run_camdl_batch(
    spec: &CamdlSpec,
    case_dir: &Path,
    runs_dir: &Path,
) -> anyhow::Result<BatchRun> {
    std::fs::create_dir_all(runs_dir)
        .map_err(|e| anyhow::anyhow!("create {}: {}", runs_dir.display(), e))?;
    let abs_runs_dir = std::fs::canonicalize(runs_dir)
        .map_err(|e| anyhow::anyhow!("canonicalize {}: {}", runs_dir.display(), e))?;
    let out_path = abs_runs_dir.join("batch_output.tsv");

    let args: Vec<String> = spec.command.iter().map(|tok| {
        match tok.as_str() {
            "@model"      => spec.model.to_string_lossy().into_owned(),
            "@params"     => spec.params.as_ref()
                               .map(|p| p.to_string_lossy().into_owned())
                               .unwrap_or_default(),
            "@n_seeds"    => spec.n_seeds.to_string(),
            "@seed_base"  => spec.seed_base.to_string(),
            "@batch_out"  => out_path.to_string_lossy().into_owned(),
            other         => other.to_string(),
        }
    }).collect();

    if args.is_empty() {
        return Err(anyhow::anyhow!("case.toml camdl.command is empty"));
    }
    let (prog, rest) = args.split_first().unwrap();

    let stdout_path = abs_runs_dir.join("stdout.log");
    let stderr_path = abs_runs_dir.join("stderr.log");
    let stdout = std::fs::File::create(&stdout_path)?;
    let stderr = std::fs::File::create(&stderr_path)?;

    let status = Command::new(prog)
        .args(rest)
        .current_dir(case_dir)
        .stdout(stdout)
        .stderr(stderr)
        .status()
        .map_err(|e| anyhow::anyhow!("spawn {}: {}", prog, e))?;

    Ok(BatchRun {
        output_tsv: out_path,
        exit_code: status.code(),
        stderr_path,
    })
}

#[derive(Debug, Clone)]
pub struct CamdlRun {
    pub seed: u64,
    pub seed_dir: PathBuf,
    pub exit_code: Option<i32>,
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
        stderr_path,
    })
}

/// Invoke `reference/run.sh` (or analog) for the regen path.
/// Returns the exit status; the reference script is expected to write
/// `fixtures/summary.tsv` itself.
pub fn run_reference(
    run_script: &Path,
    case_dir: &Path,
    log_path: &Path,
) -> anyhow::Result<std::process::ExitStatus> {
    // The subprocess cwd is case_dir, but run_script may be an absolute
    // path (when the caller already joined case_dir with the relative
    // script path). Canonicalise before handing to bash so the result
    // doesn't depend on how the caller assembled the path.
    let script_abs = std::fs::canonicalize(run_script)
        .map_err(|e| anyhow::anyhow!("canonicalize {}: {}", run_script.display(), e))?;
    let log = std::fs::File::create(log_path)?;
    let log_err = log.try_clone()?;
    let status = Command::new("bash")
        .arg(&script_abs)
        .current_dir(case_dir)
        .stdout(log)
        .stderr(log_err)
        .status()
        .map_err(|e| anyhow::anyhow!("spawn reference {}: {}", script_abs.display(), e))?;
    Ok(status)
}
