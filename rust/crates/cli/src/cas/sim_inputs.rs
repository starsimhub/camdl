//! Typed CAS inputs for `simulate --cas` and `batch run` (each sweep
//! point × seed is one simulate run).
//!
//! Single-realization shape: one seed per inputs value. Multi-seed
//! simulate isn't a feature today; if added, the runner would build N
//! `SimulateInputs` (one per seed) and group them via
//! `cas::typed::ReplicateSet` — same pattern as profile's `--seeds`.
//!
//! The path layout decomposes the content hash into `sim_hash` and
//! `scen_hash` for readable browsing
//! (`<root>/sims/<stem>-<sim_hash>/<scen-slug>-<scen_hash>/seed_N/`);
//! the trait's `content_hash()` is the authoritative cache key
//! independent of that decomposition.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cas::typed::{
    CasInputs, ContentHash, compose_with_replicate, hash_canonical,
};
use crate::hashing;
use crate::run_meta::{RunKind, SimulateMeta};
use crate::run_paths;

/// Typed CAS inputs for one simulate run.
pub struct SimulateInputs {
    /// Display-only model path (e.g. "models/sir.camdl").
    pub model_path: String,
    /// Slugified model stem for the path prefix.
    pub model_stem: Option<String>,
    /// Scenario name (or "baseline").
    pub scenario: String,
    /// Model IR hash (full 64 hex chars).
    pub model_hash: String,
    /// Canonical base-params string (used inside `sim_hash`).
    pub base_params_canonical: String,
    /// Backend name (gillespie / chain_binomial / tau_leap / ode).
    pub backend: crate::args::types::Backend,
    /// Step size.
    pub dt: f64,
    /// Scenario `enable` interventions.
    pub enable: Vec<String>,
    /// Scenario `disable` interventions.
    pub disable: Vec<String>,
    /// Merged scenario + sweep param overrides (the actual delta from
    /// base params). Hashed via `scen_hash`.
    pub scen_params: HashMap<String, f64>,
    /// Seed for this single realization.
    pub seed: u64,
    /// Lineage: hash of the parent fit when `--params` was a
    /// `mle_params.toml`. None for plain simulates.
    pub from_fit_hash: Option<String>,
    /// Sweep coordinates for display only — recorded in
    /// `SimulateMeta.sweep_point`. Already folded into `scen_params`
    /// for hashing; surfaced separately so `camdl list` can show
    /// "this run was at beta=2.5, gamma=0.4" without re-deriving.
    pub sweep_point: HashMap<String, f64>,
}

impl SimulateInputs {
    /// Sim-side hash component. Embedded in the path as the first
    /// directory under `<root>/sims/`.
    pub fn sim_hash_str(&self) -> String {
        hashing::sim_hash(
            &self.model_hash, &self.base_params_canonical,
            self.backend.as_str(), self.dt,
        )
    }
    /// Scenario-side hash component (enable + disable + scen_params).
    pub fn scen_hash_str(&self) -> String {
        hashing::scen_hash(&self.enable, &self.disable, &self.scen_params)
    }
}

impl CasInputs for SimulateInputs {
    fn content_hash(&self) -> ContentHash {
        // Inner: seed-free composition. Composed with seed via the
        // standard replicate form so a future multi-seed simulate
        // plugs in without re-deriving the hash function.
        let inner = hash_canonical(&[
            ("sim",  &self.sim_hash_str()),
            ("scen", &self.scen_hash_str()),
        ]);
        compose_with_replicate(&inner, "seed", &self.seed.to_string())
    }
    fn cas_path(&self, root: &Path) -> PathBuf {
        run_paths::sim_run_dir(
            root, self.model_stem.as_deref(),
            &self.sim_hash_str(), &self.scenario,
            &self.scen_hash_str(), self.seed,
        )
    }
    fn run_kind(&self) -> RunKind {
        RunKind::Simulate(SimulateMeta {
            model:        self.model_path.clone(),
            model_hash:   self.model_hash.clone(),
            scenario:     self.scenario.clone(),
            sim_hash:     self.sim_hash_str(),
            scen_hash:    self.scen_hash_str(),
            seed:         self.seed,
            backend:      self.backend.clone(),
            dt:           self.dt,
            sweep_point:  self.sweep_point.clone(),
            from_fit_hash: self.from_fit_hash.clone(),
        })
    }
}
