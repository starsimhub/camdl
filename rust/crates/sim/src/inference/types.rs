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
        self.reset_accumulators();
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

/// Numerically stable log-sum-exp.
pub fn log_sum_exp(log_values: &[f64]) -> f64 {
    let max = log_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if max.is_infinite() { return f64::NEG_INFINITY; }
    max + log_values.iter().map(|&lv| (lv - max).exp()).sum::<f64>().ln()
}
