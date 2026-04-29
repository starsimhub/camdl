//! Typed CAS inputs for `fit run`.
//!
//! Two levels:
//!
//! - [`FitInputs`] — the umbrella that contains all stages. Its
//!   `content_hash` is the existing `fit_content_hash`
//!   (model IR + data files + fit.toml bytes, seed-free). Its
//!   `cas_path` is `<root>/fits/<stem>-<hash[:8]>/`.
//! - [`StageInputs`] — one fit stage (cell × stage). Its `content_hash`
//!   is the existing `fit_stage_hash` (fit content + stage config + seed,
//!   seed-inclusive). Its `cas_path` is the runner-computed
//!   `<fit>/{cell}/{stage_name}/` directory; the trait's `root` argument
//!   is ignored because cell layout (real / synthetic ds_NN /
//!   fit_<seed>) is too rich to derive from `<root>` alone.
//!
//! Both wrap the existing hashing helpers (`fit_content_hash`,
//! `fit_stage_hash`) — those continue to be the load-bearing
//! implementations; the trait gives a uniform consumer-facing API.

use std::path::{Path, PathBuf};

use crate::cas::typed::{CasInputs, ContentHash};
use crate::run_meta::{FitMeta, FitStageMeta, RunKind};
use crate::run_paths;

/// Top-level fit run (the umbrella over a fit's stages).
#[derive(Clone)]
pub struct FitInputs {
    /// Pre-computed `fit_content_hash` (model IR + data + fit.toml bytes).
    /// Caller invokes `FitConfigV2::fit_content_hash` once and stashes
    /// the result here.
    pub fit_content_hash: String,
    /// Slugified stem from the fit.toml path (or model basename).
    pub stem: Option<String>,
    /// `FitMeta` payload for the umbrella's `run.json`.
    pub meta: FitMeta,
}

impl CasInputs for FitInputs {
    fn content_hash(&self) -> ContentHash {
        ContentHash::from_hex(self.fit_content_hash.clone())
    }
    fn cas_path(&self, root: &Path) -> PathBuf {
        run_paths::fit_run_dir(root, self.stem.as_deref(), &self.fit_content_hash)
    }
    fn run_kind(&self) -> RunKind {
        RunKind::Fit(self.meta.clone())
    }
}

/// One fit stage (cell × stage) — the leaf of a fit's CAS tree.
///
/// `stage_dir` is pre-computed by the runner because the cell layout
/// (`real/fit_<seed>/...` vs. `synthetic/ds_NN/fit_<seed>/...`, plus
/// optional sweep slug) is too compositional to derive from a bare
/// `<root>` argument. The `cas_path` impl returns it unchanged.
pub struct StageInputs {
    /// Pre-computed `fit_stage_hash`. Includes seed, so each cell
    /// produces a distinct StageInputs even across the same stage.
    pub fit_stage_hash: String,
    /// Absolute path of the stage's directory under the fit tree.
    pub stage_dir: PathBuf,
    /// `FitStageMeta` payload for the stage's `run.json`.
    pub meta: FitStageMeta,
}

impl CasInputs for StageInputs {
    fn content_hash(&self) -> ContentHash {
        ContentHash::from_hex(self.fit_stage_hash.clone())
    }
    fn cas_path(&self, _root: &Path) -> PathBuf {
        // Stage dirs depend on the fit's cell composition (real vs
        // synthetic ds_NN, fit_seed, sweep slug). Runner pre-computes
        // and we surface it here unchanged. Reader code can still
        // recover the relative position via Run.kind backrefs
        // (FitStageMeta.fit_hash etc.).
        self.stage_dir.clone()
    }
    fn run_kind(&self) -> RunKind {
        RunKind::FitStage(self.meta.clone())
    }
}
