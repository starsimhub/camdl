//! Core types for the particle filter and inference algorithms.
//!
//! Flat array layout for cache-friendly resampling — copying a particle's
//! state is one contiguous memcpy, not a pointer chase through Vec<Vec<...>>.

use super::traits::Resettable;

/// State of one particle: compartment counts + flow accumulators.
#[derive(Clone, Debug)]
pub struct ParticleState {
    /// Integer compartment values (local int indices, same layout as IntState).
    pub counts: Vec<i64>,
    /// Cumulative transition flows since last observation.
    /// Reset after each observation time (used for incidence projections).
    pub flow_accumulators: Vec<u64>,
}

impl ParticleState {
    pub fn new(n_compartments: usize, n_transitions: usize) -> Self {
        ParticleState {
            counts: vec![0; n_compartments],
            flow_accumulators: vec![0; n_transitions],
        }
    }

    /// Reset flow accumulators to zero (called after each observation).
    pub fn reset_flows(&mut self) {
        for f in &mut self.flow_accumulators { *f = 0; }
    }

}

impl Resettable for ParticleState {
    fn reset_accumulators(&mut self) {
        for f in &mut self.flow_accumulators { *f = 0; }
    }
}

impl ParticleState {
    /// Clamp negative compartment values to zero.
    pub fn clamp_nonneg(&mut self) {
        for c in &mut self.counts {
            if *c < 0 { *c = 0; }
        }
    }
}

/// Storage for N particles with log-weights.
pub struct ParticleSwarm {
    pub n_particles: usize,
    pub states: Vec<ParticleState>,
    pub log_weights: Vec<f64>,
}

impl ParticleSwarm {
    pub fn new(n_particles: usize, n_compartments: usize, n_transitions: usize) -> Self {
        ParticleSwarm {
            n_particles,
            states: (0..n_particles)
                .map(|_| ParticleState::new(n_compartments, n_transitions))
                .collect(),
            log_weights: vec![0.0; n_particles],
        }
    }

    /// Effective sample size: ESS = 1 / Σ(w_normalized²).
    /// Returns N when all weights are equal, 1 when one particle dominates.
    pub fn ess(&self) -> f64 {
        let max_lw = self.log_weights.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if max_lw.is_infinite() { return 0.0; }
        let sum_w: f64 = self.log_weights.iter().map(|&lw| (lw - max_lw).exp()).sum();
        let sum_w2: f64 = self.log_weights.iter().map(|&lw| (2.0 * (lw - max_lw)).exp()).sum();
        if sum_w2 == 0.0 { return 0.0; }
        (sum_w * sum_w) / sum_w2
    }
}

/// One observation stream: how to project from state, what data to compare to,
/// and how to evaluate the likelihood.
///
/// The joint observation weight at obs_idx is:
///   Σ_stream obs_loglik_fn(project(stream, state), observations[obs_idx])
///
/// This is the SINGLE shared type used by PF, PGAS, CSMC, and gradient evaluation.
/// All observation accumulation goes through `joint_obs_weight` — no inline
/// projection+likelihood code paths.
pub struct ObsStreamSpec {
    /// Indices into flow_accumulators for this stream's projection.
    pub flow_indices: Vec<usize>,
    /// Observation log-likelihood: (projected, observed) → loglik.
    pub obs_loglik_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
    /// Observed values indexed by observation time index.
    pub observations: Vec<f64>,
}

/// Joint observation weight across all streams at one observation time.
///
/// Called by PF (from ParticleState), PGAS (from cumulative flows),
/// and gradient evaluation. ONE function, ONE code path, audited ONCE.
pub fn joint_obs_weight(
    streams: &[ObsStreamSpec],
    cum_flows: &[u64],
    obs_idx: usize,
) -> f64 {
    streams.iter().map(|s| {
        let projected: f64 = s.flow_indices.iter()
            .map(|&i| cum_flows[i] as f64).sum();
        (s.obs_loglik_fn)(projected, s.observations[obs_idx])
    }).sum()
}

/// Joint observation weight from a ParticleState (convenience for PF).
pub fn joint_obs_weight_particle(
    streams: &[ObsStreamSpec],
    state: &ParticleState,
    obs_idx: usize,
) -> f64 {
    streams.iter().map(|s| {
        let projected: f64 = s.flow_indices.iter()
            .map(|&i| state.flow_accumulators[i] as f64).sum();
        (s.obs_loglik_fn)(projected, s.observations[obs_idx])
    }).sum()
}

/// Numerically stable log-sum-exp.
pub fn log_sum_exp(log_values: &[f64]) -> f64 {
    let max = log_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if max.is_infinite() { return f64::NEG_INFINITY; }
    max + log_values.iter().map(|&lv| (lv - max).exp()).sum::<f64>().ln()
}
