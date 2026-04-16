//! New fit.toml types (run-spec v0.4).
//!
//! Coexists with config.rs (FitToml) during migration. The old `camdl fit scout`
//! etc. commands use FitToml; the new `camdl fit run` uses FitConfigV2.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

// ─── Top-level ──────────────────────────────────────────────────────────────

/// A fit.toml v2 — single inference task with named stages.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FitConfigV2 {
    pub model: ModelRef,
    pub data: DataSpec,
    #[serde(default)]
    pub output_dir: Option<String>,

    /// The free parameters: what the inference algorithm estimates.
    pub estimate: IndexMap<String, EstimateSpecV2>,

    /// The fixed parameters: held constant during inference.
    /// estimate ∪ fixed must cover all model parameters.
    pub fixed: FixedParams,

    /// Inference pipeline stages, executed in declaration order.
    pub stages: IndexMap<String, Stage>,

    /// Backend and time step. Defaults: chain_binomial, dt=1.0.
    #[serde(default)]
    pub config: FitBackendConfig,

    /// Optional lineage metadata (not used by the runner).
    #[serde(default)]
    pub provenance: Option<FitProvenance>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelRef {
    pub camdl: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FitBackendConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_dt")]
    pub dt: f64,
}
fn default_backend() -> String { "chain_binomial".to_string() }
fn default_dt() -> f64 { 1.0 }
impl Default for FitBackendConfig {
    fn default() -> Self { FitBackendConfig { backend: default_backend(), dt: default_dt() } }
}

// ─── Data ───────────────────────────────────────────────────────────────────

/// Data file mapping. Keys in `observations` match observation stream names
/// declared in the .camdl file's `observations { }` block. The observation
/// model (likelihood family) and projection (which flow/compartment to
/// accumulate) are defined in the .camdl file — fit.toml only provides the
/// data file paths.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DataSpec {
    /// Map from observation stream name → data file path.
    pub observations: IndexMap<String, String>,

    /// Time threshold for temporal holdout: observations at t > this value
    /// are withheld from training. In model time units.
    /// Mutually exclusive with `holdout`.
    #[serde(default)]
    pub holdout_after: Option<f64>,

    /// Explicit holdout data files. Keys match observation stream names.
    /// Mutually exclusive with `holdout_after`.
    #[serde(default)]
    pub holdout: Option<IndexMap<String, String>>,
}

// ─── Estimate ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EstimateSpecV2 {
    /// Search bounds. Required.
    pub bounds: (f64, f64),

    /// Transform for inference. If omitted, inferred from the parameter's
    /// declared type in the .camdl file.
    #[serde(default)]
    pub transform: Option<Transform>,

    /// Prior distribution. Required for Bayesian methods (PGAS, PMMH).
    /// Optional for MLE (IF2 ignores priors).
    #[serde(default)]
    pub prior: Option<PriorSpec>,

    /// Initial value parameter: perturbed only at t=0 in IF2.
    #[serde(default)]
    pub ivp: bool,

    /// Per-parameter random walk SD for IF2. If omitted, auto-scaled from bounds.
    #[serde(default)]
    pub rw_sd: Option<f64>,

    /// Starting value override. If omitted, random from bounds (scout) or
    /// from starts_from (downstream stages).
    #[serde(default)]
    pub start: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    Log,
    Logit,
    Identity,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "dist")]
pub enum PriorSpec {
    #[serde(rename = "log_normal")]
    LogNormal { mu: f64, sigma: f64 },
    #[serde(rename = "normal")]
    Normal { mu: f64, sigma: f64 },
    #[serde(rename = "beta")]
    Beta { alpha: f64, beta: f64 },
    #[serde(rename = "uniform")]
    Uniform,
    #[serde(rename = "half_normal")]
    HalfNormal { sigma: f64 },
}

// ─── Fixed ──────────────────────────────────────────────────────────────────

/// Fixed parameters. Supports bulk loading from a file + inline overrides.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FixedParams {
    /// Bulk load from a TOML file (all key=value pairs become fixed).
    #[serde(default)]
    pub from_file: Option<String>,

    /// Inline fixed values. Override from_file if both specify a key.
    #[serde(flatten)]
    pub values: IndexMap<String, f64>,
}

impl FixedParams {
    /// Resolve to a concrete map: load from_file, then overlay inline values.
    pub fn resolve(&self) -> Result<IndexMap<String, f64>, String> {
        let mut merged = match &self.from_file {
            Some(path) => {
                let contents = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read fixed params file '{}': {}", path, e))?;
                let table: HashMap<String, toml::Value> = toml::from_str(&contents)
                    .map_err(|e| format!("parse error in '{}': {}", path, e))?;
                let mut map = IndexMap::new();
                for (k, v) in table {
                    match v {
                        toml::Value::Float(f) => { map.insert(k, f); }
                        toml::Value::Integer(i) => { map.insert(k, i as f64); }
                        _ => return Err(format!(
                            "fixed param '{}' in '{}' must be a number, got {:?}",
                            k, path, v
                        )),
                    }
                }
                map
            }
            None => IndexMap::new(),
        };
        // Inline values override file values
        for (k, v) in &self.values {
            merged.insert(k.clone(), *v);
        }
        Ok(merged)
    }
}

// ─── Stages ─────────────────────────────────────────────────────────────────

/// A named inference stage. Tagged by method.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "method")]
pub enum Stage {
    #[serde(rename = "if2")]
    IF2 {
        chains: usize,
        particles: usize,
        iterations: usize,
        /// Fraction of initial perturbation magnitude remaining at the final
        /// iteration. 0.70 = perturbations shrink to 70% over the full run.
        cooling: f64,
        #[serde(default)]
        starts_from: StartsFrom,
    },

    #[serde(rename = "pgas")]
    PGAS {
        chains: usize,
        particles: usize,
        sweeps: usize,
        #[serde(default)]
        starts_from: StartsFrom,
        #[serde(default)]
        burn_in: Option<usize>,
        #[serde(default)]
        thin: Option<usize>,
    },

    #[serde(rename = "pmmh")]
    PMMH {
        chains: usize,
        particles: usize,
        iterations: usize,
        #[serde(default)]
        starts_from: StartsFrom,
        #[serde(default)]
        burn_in: Option<usize>,
        #[serde(default)]
        thin: Option<usize>,
    },

    #[serde(rename = "pfilter")]
    PFilter {
        particles: usize,
        #[serde(default)]
        replicates: Option<usize>,
        #[serde(default)]
        starts_from: StartsFrom,
    },
}

impl Stage {
    pub fn starts_from(&self) -> &StartsFrom {
        match self {
            Stage::IF2 { starts_from, .. }
            | Stage::PGAS { starts_from, .. }
            | Stage::PMMH { starts_from, .. }
            | Stage::PFilter { starts_from, .. } => starts_from,
        }
    }

    pub fn method_name(&self) -> &str {
        match self {
            Stage::IF2 { .. } => "if2",
            Stage::PGAS { .. } => "pgas",
            Stage::PMMH { .. } => "pmmh",
            Stage::PFilter { .. } => "pfilter",
        }
    }

    pub fn requires_priors(&self) -> bool {
        matches!(self, Stage::PGAS { .. } | Stage::PMMH { .. })
    }

    pub fn chains(&self) -> usize {
        match self {
            Stage::IF2 { chains, .. } => *chains,
            Stage::PGAS { chains, .. } => *chains,
            Stage::PMMH { chains, .. } => *chains,
            Stage::PFilter { .. } => 1,
        }
    }
}

/// Where a stage gets its initial parameter values.
/// Deserialized from a string. If the string contains `/` or `\`, it's a
/// directory path; if it equals "random", it's random starts; otherwise
/// it's a stage name reference.
#[derive(Debug, Clone, Serialize)]
pub enum StartsFrom {
    /// Name of a previous stage in this fit.toml (e.g., "mle").
    Stage(String),
    /// Path to an external results directory.
    Directory(PathBuf),
    /// Random starts from parameter bounds.
    Random,
}

impl Default for StartsFrom {
    fn default() -> Self { StartsFrom::Random }
}

impl StartsFrom {
    pub fn is_random(&self) -> bool { matches!(self, StartsFrom::Random) }
}

impl<'de> serde::Deserialize<'de> for StartsFrom {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        let s = String::deserialize(deserializer)?;
        // Contains path separator → directory path
        if s.contains('/') || s.contains('\\') {
            Ok(StartsFrom::Directory(PathBuf::from(s)))
        } else {
            // Bare name → stage reference (never interpreted as Random;
            // Random is only the Default when starts_from is omitted)
            Ok(StartsFrom::Stage(s))
        }
    }
}

// ─── Provenance ─────────────────────────────────────────────────────────────

/// Optional metadata linking this fit to a parent.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FitProvenance {
    pub derived_from: Option<String>,
    pub reason: Option<String>,
}

// ─── Legacy bridge ──────────────────────────────────────────────────────────

impl FitConfigV2 {
    /// Convert to a legacy FitToml for runner compatibility.
    /// This is a bridge during migration — the runners will eventually
    /// accept FitConfigV2 directly.
    pub fn to_legacy_toml(&self) -> Result<super::config::FitToml, String> {
        use super::config::*;

        let fixed_resolved = self.fixed.resolve()?;
        let fixed_legacy: HashMap<String, toml::Value> = fixed_resolved.iter()
            .map(|(k, v)| {
                let tv = if *v == (*v as i64) as f64 && v.abs() < 1e15 {
                    toml::Value::Integer(*v as i64)
                } else {
                    toml::Value::Float(*v)
                };
                (k.clone(), tv)
            })
            .collect();

        let estimate_legacy: HashMap<String, EstimateSpec> = self.estimate.iter()
            .map(|(name, spec)| {
                let prior_str = spec.prior.as_ref().map(|p| match p {
                    PriorSpec::LogNormal { mu, sigma } => format!("lognormal({}, {})", mu, sigma),
                    PriorSpec::Normal { mu, sigma } => format!("normal({}, {})", mu, sigma),
                    PriorSpec::Beta { alpha, beta } => format!("beta({}, {})", alpha, beta),
                    PriorSpec::Uniform => "uniform".to_string(),
                    PriorSpec::HalfNormal { sigma } => format!("halfnormal({})", sigma),
                });
                (name.clone(), EstimateSpec {
                    rw_sd: spec.rw_sd,
                    ivp: spec.ivp,
                    start: spec.start,
                    transform: spec.transform.as_ref().map(|t| match t {
                        Transform::Log => "log".to_string(),
                        Transform::Logit => "logit".to_string(),
                        Transform::Identity => "identity".to_string(),
                    }),
                    bounds: Some(spec.bounds),
                    prior: prior_str,
                })
            })
            .collect();

        // Extract IF2-specific stage configs for legacy sections
        let mut scout_config = None;
        let mut refine_config = None;
        let mut validate_config = None;
        let mut pgas_config = None;
        let mut pmmh_config = None;

        for (name, stage) in &self.stages {
            match stage {
                Stage::IF2 { chains, particles, iterations, cooling, .. } => {
                    let sc = StageConfig {
                        chains: Some(*chains),
                        particles: Some(*particles),
                        iterations: Some(*iterations),
                        cooling: Some(*cooling),
                        rw_sd_scale: None,
                        start_chains: None,
                    };
                    // Map to the closest legacy stage name
                    match name.as_str() {
                        "scout" | "mle" => scout_config = Some(sc),
                        "refine" => refine_config = Some(sc),
                        "validate" | "evaluate" => {
                            validate_config = Some(ValidateConfig {
                                chains: Some(*chains),
                                particles: Some(*particles),
                                iterations: Some(*iterations),
                                cooling: Some(*cooling),
                                rw_sd_scale: None,
                                pfilter_particles: None,
                            });
                        }
                        _ => {
                            // Custom IF2 stage name — use as scout config
                            if scout_config.is_none() { scout_config = Some(sc); }
                        }
                    }
                }
                Stage::PGAS { chains, sweeps, particles, burn_in, thin, .. } => {
                    pgas_config = Some(PGASSampleConfig {
                        chains: Some(*chains),
                        sweeps: Some(*sweeps),
                        particles: Some(*particles),
                        burn_in: *burn_in,
                        thin: *thin,
                        starts_from: None,
                        n_trajectories: None,
                        tempering: None,
                        max_treedepth: None,
                        trajectory_warmup: None,
                        csmc_sweeps_per_nuts: None,
                    });
                }
                Stage::PMMH { chains, particles, iterations, burn_in, thin, .. } => {
                    pmmh_config = Some(PMMHSampleConfig {
                        chains: Some(*chains),
                        steps: Some(*iterations),
                        particles: Some(*particles),
                        burn_in: *burn_in,
                        thin: *thin,
                        adapt: None,
                        adapt_start: None,
                        proposal_from: None,
                        rho: None,
                    });
                }
                Stage::PFilter { .. } => {}
            }
        }

        let data_legacy: HashMap<String, String> = self.data.observations.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let holdout_legacy = self.data.holdout.as_ref().map(|h| {
            h.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        });

        Ok(FitToml {
            fit: FitSection {
                model: self.model.camdl.clone(),
                output_dir: self.output_dir.clone().unwrap_or_else(|| "results".to_string()),
                seed: None,
            },
            data: data_legacy,
            holdout: holdout_legacy,
            config: FitConfigSection {
                backend: self.config.backend.clone(),
                dt: self.config.dt,
            },
            estimate: estimate_legacy,
            fixed: fixed_legacy,
            scout: scout_config,
            refine: refine_config,
            validate: validate_config,
            pmmh: pmmh_config,
            pgas: pgas_config,
        })
    }
}

// ─── Loading + Validation ───────────────────────────────────────────────────

impl FitConfigV2 {
    pub fn load(path: &str) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path, e))?;
        let config: FitConfigV2 = toml::from_str(&contents)
            .map_err(|e| format!("parse error in {}: {}", path, e))?;

        Ok(config)
    }

    /// Output directory for this fit, derived from the config file name.
    pub fn fit_dir(&self, config_path: &str) -> PathBuf {
        let name = Path::new(config_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("fit");
        let output_root = self.output_dir.as_deref().unwrap_or("results");
        PathBuf::from(output_root).join("fits").join(name)
    }

    /// Output directory for a specific stage.
    pub fn stage_dir(&self, config_path: &str, stage_name: &str) -> PathBuf {
        self.fit_dir(config_path).join(stage_name)
    }

    /// Exhaustive partition check + stage DAG validation + data consistency.
    pub fn validate(&self, model_params: &[String]) -> Result<(), String> {
        // holdout_after and holdout are mutually exclusive
        if self.data.holdout_after.is_some() && self.data.holdout.is_some() {
            return Err("data.holdout_after and data.holdout are mutually exclusive.\n  \
                        Use holdout_after for temporal splits, holdout for explicit files."
                .to_string());
        }

        let model_set: BTreeSet<&str> = model_params.iter()
            .map(|s| s.as_str()).collect();
        let estimated: BTreeSet<&str> = self.estimate.keys()
            .map(|s| s.as_str()).collect();

        let fixed_resolved = self.fixed.resolve()?;
        let fixed: BTreeSet<&str> = fixed_resolved.keys()
            .map(|s| s.as_str()).collect();

        // estimate ∩ fixed = ∅
        let overlap: Vec<&&str> = estimated.intersection(&fixed).collect();
        if !overlap.is_empty() {
            return Err(format!(
                "parameters in both [estimate] and [fixed]: {}\n  \
                 Each parameter must be in exactly one section.",
                overlap.iter().map(|s| **s).collect::<Vec<_>>().join(", ")
            ));
        }

        // estimate ∪ fixed = model_params
        let covered: BTreeSet<&str> = estimated.union(&fixed).cloned().collect();
        let missing: Vec<&&str> = model_set.difference(&covered).collect();
        if !missing.is_empty() {
            return Err(format!(
                "parameters neither estimated nor fixed: {}\n  \
                 Every model parameter must appear in [estimate] or [fixed].",
                missing.iter().map(|s| **s).collect::<Vec<_>>().join(", ")
            ));
        }

        let extra: Vec<&&str> = covered.difference(&model_set).collect();
        if !extra.is_empty() {
            return Err(format!(
                "parameters not in model: {}",
                extra.iter().map(|s| **s).collect::<Vec<_>>().join(", ")
            ));
        }

        // Bayesian stages require priors on all estimated params
        for (stage_name, stage) in &self.stages {
            if stage.requires_priors() {
                let missing_priors: Vec<&str> = self.estimate.iter()
                    .filter(|(_, spec)| spec.prior.is_none())
                    .map(|(name, _)| name.as_str())
                    .collect();
                if !missing_priors.is_empty() {
                    return Err(format!(
                        "stage '{}' (method={}) requires priors, but missing for: {}\n  \
                         Add prior = {{ dist = \"...\", ... }} to each [estimate.*] entry.",
                        stage_name, stage.method_name(), missing_priors.join(", ")
                    ));
                }
            }
        }

        // Validate backend
        let valid_backends = ["gillespie", "tau_leap", "chain_binomial", "ode"];
        if !valid_backends.contains(&self.config.backend.as_str()) {
            return Err(format!(
                "unknown backend '{}'. Valid backends: {}",
                self.config.backend, valid_backends.join(", ")
            ));
        }

        // Validate stage DAG: starts_from references must be valid
        self.validate_stage_dag()?;

        // Validate bounds
        for (name, spec) in &self.estimate {
            let (lo, hi) = spec.bounds;
            if lo >= hi {
                return Err(format!(
                    "estimate.{}: bounds [{}, {}] are empty (lo must be < hi)",
                    name, lo, hi
                ));
            }
        }

        Ok(())
    }

    /// Check that starts_from references point to valid stages or "random".
    fn validate_stage_dag(&self) -> Result<(), String> {
        let stage_names: BTreeSet<&str> = self.stages.keys()
            .map(|s| s.as_str()).collect();

        // Build execution order (declaration order) and check dependencies
        let stage_order: Vec<&str> = self.stages.keys()
            .map(|s| s.as_str()).collect();

        for (i, (name, stage)) in self.stages.iter().enumerate() {
            match stage.starts_from() {
                StartsFrom::Random => continue,
                StartsFrom::Stage(ref dep) => {
                    if !stage_names.contains(dep.as_str()) {
                        return Err(format!(
                            "stage '{}': starts_from = \"{}\" does not match any stage.\n  \
                             Available stages: {}",
                            name, dep, stage_order.join(", ")
                        ));
                    }
                    // Check ordering: dependency must come before this stage
                    let dep_idx = stage_order.iter().position(|s| *s == dep.as_str());
                    if let Some(di) = dep_idx {
                        if di >= i {
                            return Err(format!(
                                "stage '{}': starts_from = \"{}\" but '{}' is declared after '{}'.\n  \
                                 Stages execute in declaration order; dependencies must come first.",
                                name, dep, dep, name
                            ));
                        }
                    }
                }
                StartsFrom::Directory(_) => {
                    // External directory — no DAG check needed
                }
            }
        }
        Ok(())
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_str: &str) -> Result<FitConfigV2, String> {
        toml::from_str(toml_str)
            .map_err(|e| format!("parse error: {}", e))
    }

    #[test]
    fn parse_simple_mle() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta  = { bounds = [0.01, 2.0] }
gamma = { bounds = [0.05, 1.0] }
rho   = { bounds = [0.001, 1.0] }
k     = { bounds = [0.1, 100.0] }

[fixed]
N0 = 1000000
I0 = 10

[stages.mle]
method = "if2"
chains = 8
particles = 1000
iterations = 80
cooling = 0.70
        "#).unwrap();

        assert_eq!(config.estimate.len(), 4);
        assert_eq!(config.fixed.values.len(), 2);
        assert_eq!(config.stages.len(), 1);
        assert!(config.stages.contains_key("mle"));

        match &config.stages["mle"] {
            Stage::IF2 { chains, particles, iterations, cooling, .. } => {
                assert_eq!(*chains, 8);
                assert_eq!(*particles, 1000);
                assert_eq!(*iterations, 80);
                assert!((cooling - 0.70).abs() < 1e-10);
            }
            _ => panic!("expected IF2 stage"),
        }
    }

    #[test]
    fn parse_mle_plus_posterior() {
        let config = parse(r#"
[provenance]
derived_from = "fits/01_all_free.toml"
reason = "beta mixing poor in PGAS"

[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
gamma = { bounds = [0.05, 1.0], prior = { dist = "log_normal", mu = -2.0, sigma = 1.0 } }
rho   = { bounds = [0.001, 1.0], prior = { dist = "beta", alpha = 2.0, beta = 5.0 } }
k     = { bounds = [0.1, 100.0], prior = { dist = "half_normal", sigma = 10.0 } }

[fixed]
beta = 0.34
N0 = 1000000
I0 = 10

[stages.mle]
method = "if2"
chains = 4
particles = 2000
iterations = 60
cooling = 0.95
starts_from = "results/fits/01_all_free/mle"

[stages.posterior]
method = "pgas"
chains = 4
particles = 50
sweeps = 5000
starts_from = "mle"

[stages.evaluate]
method = "pfilter"
particles = 10000
replicates = 100
starts_from = "mle"
        "#).unwrap();

        assert_eq!(config.stages.len(), 3);
        let stage_names: Vec<&str> = config.stages.keys().map(|s| s.as_str()).collect();
        assert_eq!(stage_names, vec!["mle", "posterior", "evaluate"]);

        // mle starts from external directory
        match config.stages["mle"].starts_from() {
            StartsFrom::Directory(p) => assert_eq!(p, Path::new("results/fits/01_all_free/mle")),
            other => panic!("expected Directory, got {:?}", other),
        }

        // posterior starts from mle (stage reference)
        match config.stages["posterior"].starts_from() {
            StartsFrom::Stage(s) => assert_eq!(s, "mle"),
            other => panic!("expected Stage, got {:?}", other),
        }

        // All estimated params have priors (needed for PGAS)
        for (_, spec) in &config.estimate {
            assert!(spec.prior.is_some());
        }

        assert!(config.provenance.is_some());
        assert_eq!(config.provenance.as_ref().unwrap().derived_from.as_deref(),
                   Some("fits/01_all_free.toml"));
    }

    #[test]
    fn parse_with_from_file() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 5.0] }

[fixed]
from_file = "params/fixed.toml"
vacc_frac = 0.80

[stages.mle]
method = "if2"
chains = 8
particles = 2000
iterations = 100
cooling = 0.70
        "#).unwrap();

        assert_eq!(config.fixed.from_file.as_deref(), Some("params/fixed.toml"));
        assert_eq!(config.fixed.values["vacc_frac"], 0.80);
    }

    #[test]
    fn parse_holdout_after() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data]
holdout_after = 5474.0

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 2.0] }

[fixed]
N0 = 1000000

[stages.mle]
method = "if2"
chains = 4
particles = 1000
iterations = 50
cooling = 0.70
        "#).unwrap();

        assert_eq!(config.data.holdout_after, Some(5474.0));
        assert!(config.data.holdout.is_none());
    }

    #[test]
    fn validate_complete_partition() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta  = { bounds = [0.01, 2.0] }
gamma = { bounds = [0.05, 1.0] }

[fixed]
N0 = 1000000
I0 = 10

[stages.mle]
method = "if2"
chains = 4
particles = 1000
iterations = 50
cooling = 0.70
        "#).unwrap();

        // All params present → OK
        let model_params = vec![
            "beta".to_string(), "gamma".to_string(),
            "N0".to_string(), "I0".to_string(),
        ];
        assert!(config.validate(&model_params).is_ok());
    }

    #[test]
    fn validate_missing_param() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 2.0] }

[fixed]
N0 = 1000000

[stages.mle]
method = "if2"
chains = 4
particles = 1000
iterations = 50
cooling = 0.70
        "#).unwrap();

        let model_params = vec![
            "beta".to_string(), "gamma".to_string(),
            "N0".to_string(), "I0".to_string(),
        ];
        let err = config.validate(&model_params).unwrap_err();
        assert!(err.contains("neither estimated nor fixed"));
        assert!(err.contains("gamma"));
        assert!(err.contains("I0"));
    }

    #[test]
    fn validate_overlap() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 2.0] }

[fixed]
beta = 0.5
N0 = 1000000

[stages.mle]
method = "if2"
chains = 4
particles = 1000
iterations = 50
cooling = 0.70
        "#).unwrap();

        let model_params = vec!["beta".to_string(), "N0".to_string()];
        let err = config.validate(&model_params).unwrap_err();
        assert!(err.contains("both [estimate] and [fixed]"));
        assert!(err.contains("beta"));
    }

    #[test]
    fn validate_extra_param() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 2.0] }
typo_param = { bounds = [0.0, 1.0] }

[fixed]
N0 = 1000000

[stages.mle]
method = "if2"
chains = 4
particles = 1000
iterations = 50
cooling = 0.70
        "#).unwrap();

        let model_params = vec!["beta".to_string(), "N0".to_string()];
        let err = config.validate(&model_params).unwrap_err();
        assert!(err.contains("not in model"));
        assert!(err.contains("typo_param"));
    }

    #[test]
    fn validate_pgas_requires_priors() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 2.0] }

[fixed]
N0 = 1000000

[stages.posterior]
method = "pgas"
chains = 4
particles = 50
sweeps = 5000
        "#).unwrap();

        let model_params = vec!["beta".to_string(), "N0".to_string()];
        let err = config.validate(&model_params).unwrap_err();
        assert!(err.contains("requires priors"));
        assert!(err.contains("beta"));
    }

    #[test]
    fn validate_bad_stage_dag() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 2.0] }

[fixed]
N0 = 1000000

[stages.refine]
method = "if2"
chains = 4
particles = 2000
iterations = 50
cooling = 0.95
starts_from = "mle"

[stages.mle]
method = "if2"
chains = 8
particles = 1000
iterations = 80
cooling = 0.70
        "#).unwrap();

        let model_params = vec!["beta".to_string(), "N0".to_string()];
        let err = config.validate(&model_params).unwrap_err();
        assert!(err.contains("declared after"));
    }

    #[test]
    fn validate_bad_stage_ref() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [0.01, 2.0] }

[fixed]
N0 = 1000000

[stages.mle]
method = "if2"
chains = 4
particles = 1000
iterations = 50
cooling = 0.70
starts_from = "nonexistent"
        "#).unwrap();

        let model_params = vec!["beta".to_string(), "N0".to_string()];
        let err = config.validate(&model_params).unwrap_err();
        assert!(err.contains("does not match any stage"));
    }

    #[test]
    fn validate_empty_bounds() {
        let config = parse(r#"
[model]
camdl = "models/sir.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

[config]
backend = "chain_binomial"
dt = 1.0

[estimate]
beta = { bounds = [2.0, 0.01] }

[fixed]
N0 = 1000000

[stages.mle]
method = "if2"
chains = 4
particles = 1000
iterations = 50
cooling = 0.70
        "#).unwrap();

        let model_params = vec!["beta".to_string(), "N0".to_string()];
        let err = config.validate(&model_params).unwrap_err();
        assert!(err.contains("bounds"));
        assert!(err.contains("empty"));
    }
}
