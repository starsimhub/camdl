//! `fit.toml` parsing and validation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level fit.toml structure.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FitToml {
    pub fit: FitSection,
    pub data: HashMap<String, String>,
    /// Holdout data for out-of-sample validation. Keys must match [data] keys.
    /// Scout/refine never see holdout data. Validate runs PF on train + holdout
    /// and reports separate logliks.
    pub holdout: Option<HashMap<String, String>>,
    pub config: FitConfigSection,
    pub estimate: HashMap<String, EstimateSpec>,
    pub fixed: HashMap<String, toml::Value>,
    /// Optional per-stage configuration. Omitted sections use defaults.
    pub scout: Option<StageConfig>,
    pub refine: Option<StageConfig>,
    pub validate: Option<ValidateConfig>,
    /// PMMH posterior sampling configuration.
    pub pmmh: Option<PMMHSampleConfig>,
    /// PGAS posterior sampling configuration.
    pub pgas: Option<PGASSampleConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FitSection {
    pub model: String,
    pub output_dir: String,
    /// RNG seed. CLI --seed overrides this.
    pub seed: Option<u64>,

    /// Named scenario from the model's `scenarios { }` block. Applies the
    /// scenario's enable/disable lists and param overrides before the fit.
    /// Mutually exclusive with `enable`/`disable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,

    /// Ad-hoc enable list. Names of interventions (or their base_name
    /// families) to activate during inference. Default: no toggleable
    /// interventions fire, matching the spec's "off by default". Events
    /// (`events {}` block) always fire unless explicitly disabled.
    /// Wildcard `"*"` activates every toggleable intervention.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enable: Vec<String>,

    /// Ad-hoc disable list. Explicit disable wins over always_active, so
    /// this is the only way to silence an event during inference.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable: Vec<String>,

    /// IC-free inference: condition the likelihood on the first
    /// observation rather than on an initial-state commitment. The PF
    /// still weights and resamples at y₁ (that's how y₁ pins the
    /// initial state), but the log-likelihood accumulation starts from
    /// y₂. Requires at least one `[estimate.*]` entry with `ivp = true`
    /// to provide particle spread at t=0. See
    /// docs/dev/proposals/2026-04-18-ic-free-inference.md.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ic_free: bool,
}

/// Per-stage configuration for scout and refine.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StageConfig {
    pub chains: Option<usize>,
    pub particles: Option<usize>,
    pub iterations: Option<usize>,
    pub cooling: Option<f64>,
    /// Multiply all rw_sd values by this factor. Default 1.0.
    pub rw_sd_scale: Option<f64>,
    /// Number of chains seeded near start values (rest are random).
    /// Default 1 when any parameter has start, 0 otherwise.
    pub start_chains: Option<usize>,
}

/// Validate stage configuration (includes pfilter settings).
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateConfig {
    pub chains: Option<usize>,
    pub particles: Option<usize>,
    pub iterations: Option<usize>,
    pub cooling: Option<f64>,
    pub rw_sd_scale: Option<f64>,
    /// Particle count for the final precise pfilter at the MLE.
    pub pfilter_particles: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FitConfigSection {
    pub backend: String,
    pub dt: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EstimateSpec {
    /// Random walk SD on natural scale. If omitted, auto-computed from bounds.
    pub rw_sd: Option<f64>,
    #[serde(default)]
    pub ivp: bool,
    pub start: Option<f64>,
    pub transform: Option<String>,
    pub bounds: Option<(f64, f64)>,
    /// Prior distribution string. Supported:
    ///   "lognormal(mu, sigma)" → TransformedNormal on log scale
    ///   "normal(mu, sigma)"    → Normal on natural scale
    ///   omitted                → Flat (improper uniform)
    pub prior: Option<String>,
}

/// PMMH posterior sampling configuration.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PMMHSampleConfig {
    pub chains: Option<usize>,
    pub steps: Option<usize>,
    pub particles: Option<usize>,
    pub burn_in: Option<usize>,
    pub thin: Option<usize>,
    pub adapt: Option<bool>,
    pub adapt_start: Option<usize>,
    /// Directory containing fit_state.toml from a prior IF2 run.
    /// Used to seed proposal covariance from scout chain spread.
    pub proposal_from: Option<String>,
    /// Crank-Nicolson correlation for correlated pseudo-marginal MCMC.
    /// Default: 0.99. Set to None or 0.0 for vanilla (independent) PMMH.
    pub rho: Option<f64>,
}

/// PGAS (Particle Gibbs with Ancestor Sampling) configuration.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PGASSampleConfig {
    pub chains: Option<usize>,
    pub sweeps: Option<usize>,
    pub particles: Option<usize>,
    pub burn_in: Option<usize>,
    pub thin: Option<usize>,
    /// Directory containing fit_state.toml from a prior stage.
    pub starts_from: Option<String>,
    /// Number of posterior trajectory samples to save per chain.
    /// Evenly spaced across post-burn-in sweeps. Default: 200.
    /// Set to 0 to disable trajectory output.
    pub n_trajectories: Option<usize>,
    /// Temperature ladder for parallel tempering (replica exchange).
    /// Each entry is a β value. First entry must be 1.0 (cold chain).
    /// Example: `[1.0, 0.7, 0.4, 0.15]` runs 4 rungs per chain.
    /// Default: no tempering (single rung).
    pub tempering: Option<Vec<f64>>,
    /// Maximum NUTS tree depth. Default: 10 (Stan default).
    /// Lower values (e.g., 6-8) speed up exploration at the cost of
    /// shorter trajectories. Useful for models with expensive gradients.
    pub max_treedepth: Option<usize>,
    /// Number of CSMC-only sweeps before parameter updates begin.
    /// During warm-up, the trajectory is refreshed via CSMC-AS but
    /// parameters are held fixed. Default: 0 (no warm-up).
    pub trajectory_warmup: Option<usize>,
    /// Number of CSMC trajectory updates per parameter update.
    /// Default: 1. Higher values (3-5) improve trajectory convergence
    /// on models with long time series.
    pub csmc_sweeps_per_nuts: Option<usize>,
}

impl FitToml {
    /// Seed-independent content hash for a v1 fit: hashes the fit.toml
    /// bytes, model IR, and every data file (train + holdout) referenced
    /// by the config. Used for the `<stem>-<hash[:8]>` suffix on the
    /// top-level fit directory. Mirrors `FitConfigV2::fit_content_hash`
    /// so v1 and v2 fits with the same inputs produce the same hash.
    pub fn fit_content_hash(&self, toml_path: &str) -> Result<String, String> {
        let fit_bytes = std::fs::read(toml_path)
            .map_err(|e| format!("cannot read fit.toml at '{}': {}", toml_path, e))?;
        let model_ir_bytes = std::fs::read(&self.fit.model)
            .map_err(|e| format!("cannot read model at '{}': {}", self.fit.model, e))?;
        let mut data_files: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, path) in &self.data {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("cannot read data file '{}' ({}): {}", name, path, e))?;
            data_files.push((name.clone(), bytes));
        }
        if let Some(ref holdout) = self.holdout {
            for (name, path) in holdout {
                let bytes = std::fs::read(path)
                    .map_err(|e| format!("cannot read holdout file '{}' ({}): {}", name, path, e))?;
                data_files.push((format!("holdout:{}", name), bytes));
            }
        }
        Ok(crate::hashing::fit_content_hash(&model_ir_bytes, &mut data_files, &fit_bytes))
    }

    /// The top-level fit directory under the unified output tree:
    /// `<output_root>/fits/<stem>-<fit_hash[:8]>/`. `output_root`
    /// takes the fit.toml's `[fit] output_dir` value (default
    /// `"output"`). This matches the v2 layout so `camdl list` sees
    /// v1 and v2 fits identically.
    pub fn fit_root(&self, toml_path: &str) -> Result<std::path::PathBuf, String> {
        let stem = crate::hashing::path_stem_slug(toml_path);
        let hash = self.fit_content_hash(toml_path)?;
        let root = crate::run_paths::output_root(None, Some(&self.fit.output_dir));
        Ok(crate::run_paths::fit_run_dir(&root, stem.as_deref(), &hash))
    }

    /// Per-fit cell directory: `<fit_root>/real/fit_<seed>/`. Stage
    /// subdirectories (`scout/`, `refine/`, `validate/`, `pmmh/`,
    /// `pgas/`) live immediately under this path. v1 doesn't have
    /// synthetic-data fits, so the source is always `Real`.
    pub fn cell_dir(&self, toml_path: &str, seed: u64) -> Result<std::path::PathBuf, String> {
        let fit_root = self.fit_root(toml_path)?;
        Ok(fit_root.join("real").join(format!("fit_{}", seed)))
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path, e))?;
        let fit: FitToml = toml::from_str(&contents)
            .map_err(|e| format!("parse error in {}: {}", path, e))?;

        // Validate holdout keys match data keys
        if let Some(ref holdout) = fit.holdout {
            for key in holdout.keys() {
                if !fit.data.contains_key(key) {
                    return Err(format!(
                        "[holdout] key '{}' does not match any [data] key.\n\
                         Available data streams: {}",
                        key, fit.data.keys().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
        }

        Ok(fit)
    }

    /// Exhaustive partition check: every model parameter must be in [estimate] or [fixed].
    #[allow(dead_code)]
    pub fn validate_partition(&self, model_params: &[String]) -> Result<(), String> {
        let estimated: std::collections::HashSet<&str> =
            self.estimate.keys().map(|s| s.as_str()).collect();
        let fixed: std::collections::HashSet<&str> =
            self.fixed.keys().map(|s| s.as_str()).collect();

        // Check overlap
        let overlap: Vec<&str> = estimated.intersection(&fixed).copied().collect();
        if !overlap.is_empty() {
            return Err(format!(
                "Parameters in both [estimate] and [fixed]: {}\n\
                 Each parameter must appear in exactly one section.",
                overlap.join(", ")
            ));
        }

        // Check missing
        let missing: Vec<&str> = model_params.iter()
            .map(|s| s.as_str())
            .filter(|name| !estimated.contains(name) && !fixed.contains(name))
            .collect();
        if !missing.is_empty() {
            let suggestions: Vec<String> = missing.iter().map(|name| {
                format!("  [estimate]: {} = {{ rw_sd = 0.01 }}\n  [fixed]:    {} = true", name, name)
            }).collect();
            return Err(format!(
                "Parameters not assigned in fit.toml: {}\n\
                 Every model parameter must appear in [estimate] or [fixed].\n\n{}",
                missing.join(", "),
                suggestions.join("\n")
            ));
        }

        // Check extra (in fit.toml but not in model)
        let model_set: std::collections::HashSet<&str> =
            model_params.iter().map(|s| s.as_str()).collect();
        let extra_est: Vec<&str> = estimated.iter()
            .filter(|n| !model_set.contains(**n)).copied().collect();
        let extra_fix: Vec<&str> = fixed.iter()
            .filter(|n| !model_set.contains(**n)).copied().collect();
        if !extra_est.is_empty() || !extra_fix.is_empty() {
            let mut extra = extra_est;
            extra.extend(extra_fix);
            return Err(format!(
                "Parameters in fit.toml but not in model: {}\n\
                 Check for typos.",
                extra.join(", ")
            ));
        }

        Ok(())
    }

    /// Check that fit bounds are within model bounds.
    #[allow(dead_code)]
    pub fn validate_bounds(&self, model: &ir::Model) -> Result<(), String> {
        for (name, spec) in &self.estimate {
            if let Some((fit_lo, fit_hi)) = spec.bounds {
                if let Some(ir_param) = model.parameters.iter().find(|p| p.name == *name) {
                    if let Some((model_lo, model_hi)) = ir_param.bounds {
                        if fit_lo < model_lo || fit_hi > model_hi {
                            return Err(format!(
                                "fit bound [{}, {}] for '{}' extends beyond model bound [{}, {}].\n\
                                 Fit search bounds must be within model structural bounds.",
                                fit_lo, fit_hi, name, model_lo, model_hi
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
