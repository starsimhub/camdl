//! Canonical output-path construction for the unified tree.
//!
//! Every filesystem path that camdl writes results into goes through
//! one of the helpers in this module. Centralising construction
//! prevents the `format!("{}/refine", fit.fit.output_dir)`
//! scattered-hand-rolling that let the fit and simulate trees drift
//! apart historically.
//!
//! Introduced in commit 3/6 of the output-tree unification
//! (docs/dev/proposals/2026-04-19-unified-output-tree.md).

use std::path::{Path, PathBuf};

use crate::hashing::slug;

/// Default output root: `./output`. Overridden by explicit CLI
/// `--output-dir`, fit.toml `output_dir`, or batch.toml `output_dir`
/// — in that precedence order. Callers should resolve via
/// [`output_root`] so the three entry points can't drift.
pub const DEFAULT_OUTPUT_ROOT: &str = "output";

/// Resolve the output root from the three places a user can set it.
/// CLI override wins; then config-file value; else default.
pub fn output_root(cli: Option<&str>, config: Option<&str>) -> PathBuf {
    cli.map(PathBuf::from)
        .or_else(|| config.map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT_ROOT))
}

/// Directory for one simulate run. Matches the layout established
/// for `--cas` and `simulate batch`:
///
/// ```text
/// <root>/sims/<sim_hash[:8]>/<scenario-slug>-<scen_hash[:8]>/seed_<N>/
/// ```
///
/// Once commit 4 lands, this replaces the hard-coded
/// `<root>/runs/...` construction in `cas::run_dir` and
/// `batch.rs`'s sweep-point path assembly.
pub fn sim_run_dir(
    root: &Path,
    sim_hash: &str,
    scenario: &str,
    scen_hash: &str,
    seed: u64,
) -> PathBuf {
    root.join("sims")
        .join(&sim_hash[..8.min(sim_hash.len())])
        .join(format!("{}-{}", slug(scenario), &scen_hash[..8.min(scen_hash.len())]))
        .join(format!("seed_{}", seed))
}

/// Directory for obs draws derived from a simulate run. Obs is a
/// simulate-specific concept (draws from the observation model on
/// top of a trajectory) and doesn't apply to fits — lives under
/// the per-run directory, not as a sibling.
pub fn sim_obs_dir(run_dir: &Path, obs_hash: &str, obs_seed: u64) -> PathBuf {
    run_dir.join("obs").join(format!("{}-{}", &obs_hash[..8.min(obs_hash.len())], obs_seed))
}

/// Top-level directory for a fit. Commit 4 will switch writes to
/// this path (replacing today's `results/fits/<config-stem>/`).
///
/// ```text
/// <root>/fits/<fit_hash[:8]>/
/// ```
pub fn fit_run_dir(root: &Path, fit_hash: &str) -> PathBuf {
    root.join("fits").join(&fit_hash[..8.min(fit_hash.len())])
}

/// Where a fit's dataset sits. `Real` fits have a single dataset,
/// `Synthetic` fits have N with 1-based indices. Mirrors the layout
/// from the synthetic-fit-replicates proposal
/// (docs/dev/proposals/2026-04-17-synthetic-fit-replicates.md):
///
/// ```text
/// real/                    (single dataset — the user's data files)
/// synthetic/ds_NN/         (one per sim_seed)
/// ```
#[derive(Debug, Clone, Copy)]
pub enum FitSource {
    Real,
    Synthetic { dataset_idx: usize },
}

/// The per-fit subdirectory under [`fit_run_dir`]. Each fit cell
/// (one (dataset, fit_seed) pair) owns its own tree of stage
/// subdirectories inside this path.
///
/// ```text
/// real/fit_<seed>/                              # real data fit
/// synthetic/ds_NN/fit_<seed>/                   # SBC-style cell
/// ```
pub fn fit_cell_dir(root: &Path, fit_hash: &str, source: FitSource, fit_seed: u64) -> PathBuf {
    let base = fit_run_dir(root, fit_hash);
    match source {
        FitSource::Real =>
            base.join("real").join(format!("fit_{}", fit_seed)),
        FitSource::Synthetic { dataset_idx } =>
            base.join("synthetic")
                .join(format_dataset_dir(dataset_idx))
                .join(format!("fit_{}", fit_seed)),
    }
}

/// Directory for one stage of a fit cell.
///
/// ```text
/// real/fit_<seed>/<stage>/
/// synthetic/ds_NN/fit_<seed>/<stage>/
/// ```
pub fn fit_stage_dir(
    root: &Path,
    fit_hash: &str,
    source: FitSource,
    fit_seed: u64,
    stage: &str,
) -> PathBuf {
    fit_cell_dir(root, fit_hash, source, fit_seed).join(stage)
}

/// Format a synthetic-dataset index as `ds_01`, `ds_02`, … The single
/// authoritative formatter, shared between path construction, summary
/// table writers, and TSV filenames. Zero-padded to 2 digits; grids
/// of > 99 datasets render as `ds_100`, `ds_101`, etc.
pub fn format_dataset_dir(idx: usize) -> String {
    format!("ds_{:02}", idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_root_precedence() {
        assert_eq!(output_root(None, None), PathBuf::from("output"));
        assert_eq!(output_root(None, Some("results")), PathBuf::from("results"));
        assert_eq!(output_root(Some("/tmp/abc"), None), PathBuf::from("/tmp/abc"));
        // CLI wins over config.
        assert_eq!(output_root(Some("/cli"), Some("/config")), PathBuf::from("/cli"));
    }

    #[test]
    fn sim_run_dir_layout() {
        let p = sim_run_dir(Path::new("/out"), "abcdef1234567890", "baseline",
            "deadbeef1234", 42);
        assert_eq!(p, Path::new("/out/sims/abcdef12/baseline-deadbeef/seed_42"));
    }

    #[test]
    fn sim_run_dir_slugifies_scenario() {
        let p = sim_run_dir(Path::new("/out"), "aaaaaaaa", "With SIA!",
            "bbbbbbbb", 1);
        assert!(p.to_str().unwrap().contains("/with_sia_-"),
            "scenario must be slugified: {}", p.display());
    }

    #[test]
    fn sim_obs_dir_nested_under_run() {
        let run = Path::new("/out/sims/aaaa/base-bbbb/seed_1");
        let p = sim_obs_dir(run, "ccccdddd", 99);
        assert_eq!(p, run.join("obs").join("ccccdddd-99"));
    }

    #[test]
    fn fit_run_dir_uses_hash_prefix() {
        let p = fit_run_dir(Path::new("/out"), "deadbeef00000000");
        assert_eq!(p, Path::new("/out/fits/deadbeef"));
    }

    #[test]
    fn fit_cell_dir_real_vs_synthetic() {
        let r = fit_cell_dir(Path::new("/out"), "deadbeef00", FitSource::Real, 42);
        assert_eq!(r, Path::new("/out/fits/deadbeef/real/fit_42"));
        let s = fit_cell_dir(Path::new("/out"), "deadbeef00",
            FitSource::Synthetic { dataset_idx: 3 }, 101);
        assert_eq!(s, Path::new("/out/fits/deadbeef/synthetic/ds_03/fit_101"));
    }

    #[test]
    fn fit_stage_dir_composes_cell_plus_stage() {
        let p = fit_stage_dir(Path::new("/out"), "deadbeef00",
            FitSource::Real, 42, "refine");
        assert_eq!(p, Path::new("/out/fits/deadbeef/real/fit_42/refine"));
    }

    #[test]
    fn dataset_dir_zero_pads_to_2_digits() {
        assert_eq!(format_dataset_dir(1),   "ds_01");
        assert_eq!(format_dataset_dir(9),   "ds_09");
        assert_eq!(format_dataset_dir(10),  "ds_10");
        assert_eq!(format_dataset_dir(100), "ds_100");
    }

    #[test]
    fn hash_prefix_tolerates_short_hashes() {
        // Defensive: helpers slice [..8] on the hash. If a caller
        // accidentally passes a short hash, the slice should still
        // work (not panic) so we don't turn a hash-bug into a
        // crash-bug.
        let p = fit_run_dir(Path::new("/out"), "abc");
        assert_eq!(p, Path::new("/out/fits/abc"));
    }
}
