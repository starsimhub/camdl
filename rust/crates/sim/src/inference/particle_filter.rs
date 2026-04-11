//! Bootstrap particle filter (Gordon, Salmond & Smith 1993).
//!
//! Estimates log p(y_{1:T} | θ) via sequential importance sampling
//! with systematic resampling. Uses the ProcessModel trait to
//! advance particles — any simulation backend works (chain-binomial,
//! tau-leap, etc.).

use rayon::prelude::*;

use crate::rng::StatefulRng;
use crate::error::SimError;
use super::traits::{ProcessModel, ObservationModel, SMCConfig};
use super::types::{ParticleState, ParticleSwarm, log_sum_exp};
use super::resampling::systematic_resample;
use crate::chain_binomial::StepScratch;

/// Observation: one data point at a specific time.
#[derive(Clone)]
pub struct Observation {
    pub time: f64,
    pub value: f64,
}

/// One-step-ahead prediction diagnostics at a single observation time.
#[derive(Clone, Debug)]
pub struct PredictionDiag {
    /// Observation-space: E[y | projected] averaged across particles.
    pub obs_mean: f64,
    /// Observation-space quantiles (process + observation noise).
    pub obs_q05: f64,
    pub obs_q50: f64,
    pub obs_q95: f64,
    /// Latent state quantiles (process uncertainty only).
    pub state_mean: f64,
    pub state_q05: f64,
    pub state_q50: f64,
    pub state_q95: f64,
}

/// Result of a particle filter run.
pub struct PFilterResult {
    /// Estimated log p(y_{1:T} | θ).
    pub log_likelihood: f64,
    /// ESS at each observation time.
    pub ess_trace: Vec<f64>,
    /// Log-likelihood increment at each observation time.
    pub ll_increments: Vec<f64>,
    /// One-step-ahead prediction diagnostics at each observation time.
    /// Only populated when obs model supports sample/mean.
    pub predictions: Option<Vec<PredictionDiag>>,
    /// Final particle states after the last observation (post-resampling).
    /// Only populated when `save_final_state` is true.
    pub final_states: Option<Vec<ParticleState>>,
}

/// Run the bootstrap particle filter.
///
/// # Arguments
/// * `process` — process model (advance state by dt)
/// * `obs_model` — observation model (log-likelihood, sample, mean)
/// * `params` — parameter values
/// * `config` — SMC config (n_particles, dt)
/// * `seed` — RNG seed
pub fn bootstrap_filter<P: ProcessModel<State = ParticleState, Scratch = StepScratch>>(
    process: &P,
    obs_model: &(dyn ObservationModel<ParticleState> + Sync),
    params: &[f64],
    config: &SMCConfig,
    seed: u64,
) -> Result<PFilterResult, SimError> {
    let n_particles = config.n_particles;
    let dt = config.dt;
    let n_obs = obs_model.n_observations();
    let n_int = process.n_compartments();
    let n_tr = process.n_transitions();

    // Initialize particles from model init
    let init = process.initial_state(params)?;
    let mut swarm = ParticleSwarm::new(n_particles, n_int, n_tr);
    for p in &mut swarm.states {
        p.counts.copy_from_slice(&init.counts);
    }

    // Per-particle RNG streams (deterministic, derived from seed)
    let mut rngs: Vec<StatefulRng> = (0..n_particles)
        .map(|i| StatefulRng::new(seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95)))
        .collect();

    // Separate RNG streams for diagnostic draws (rmeasure).
    // Process RNG streams must be identical whether or not predictions are computed.
    let mut diag_rngs: Vec<StatefulRng> = (0..n_particles)
        .map(|i| StatefulRng::new(seed ^ (i as u64).wrapping_mul(0xbaadf00dcafebabe)))
        .collect();

    // Double-buffer for resampling (avoids clone allocation)
    let mut states_buf: Vec<ParticleState> = (0..n_particles)
        .map(|_| ParticleState::new(n_int, n_tr))
        .collect();

    // Per-particle scratch buffers (allocated once, reused across all steps)
    let mut scratches: Vec<StepScratch> = (0..n_particles)
        .map(|_| process.new_scratch())
        .collect();

    let mut total_loglik = 0.0;
    let mut ess_trace = Vec::with_capacity(n_obs);
    let mut ll_increments = Vec::with_capacity(n_obs);
    let has_predictions = obs_model.n_streams() > 0 && !obs_model.mean(&init, 0, params).is_empty();
    let mut predictions: Vec<PredictionDiag> = if has_predictions {
        Vec::with_capacity(n_obs)
    } else {
        Vec::new()
    };

    let mut t = config.t_start;

    // Resampling RNG (separate from particle RNGs)
    let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));

    for obs_idx in 0..n_obs {
        let obs_time = obs_model.obs_time(obs_idx);

        // Propagate all particles from t to obs_time.
        let t_start_interval = t;
        let errors: Vec<Result<(), SimError>> = swarm.states.par_iter_mut()
            .zip(rngs.par_iter_mut())
            .zip(scratches.par_iter_mut())
            .map(|((state, rng), scratch)| {
                let mut t_local = t_start_interval;
                while t_local < obs_time - 1e-10 {
                    let step_dt = dt.min(obs_time - t_local);
                    process.step(state, params, t_local, step_dt, rng, scratch)?;
                    t_local += step_dt;
                }
                Ok(())
            })
            .collect();
        for r in errors { r?; }
        while t < obs_time - 1e-10 { t += dt.min(obs_time - t); }

        // Prediction diagnostics
        if has_predictions {
            let means: Vec<f64> = swarm.states.iter()
                .map(|s| obs_model.mean(s, obs_idx, params).into_iter().sum::<f64>())
                .collect();
            let equal_lw = vec![0.0_f64; n_particles];
            let (state_mean, state_q05, state_q50, state_q95) = weighted_quantiles(&means, &equal_lw);

            let obs_draws: Vec<f64> = swarm.states.iter().enumerate()
                .map(|(i, s)| obs_model.sample(s, obs_idx, params, &mut diag_rngs[i]).into_iter().sum())
                .collect();
            let (_, obs_q05, obs_q50, obs_q95) = weighted_quantiles(&obs_draws, &equal_lw);

            let obs_mean = means.iter().sum::<f64>() / n_particles as f64;
            predictions.push(PredictionDiag {
                obs_mean, obs_q05, obs_q50, obs_q95,
                state_mean, state_q05, state_q50, state_q95,
            });
        }

        // Compute log-weights via observation model
        for (i, state) in swarm.states.iter().enumerate() {
            swarm.log_weights[i] = obs_model.log_likelihood(state, obs_idx, params);
        }

        // Log-marginal increment
        let ll_increment = log_sum_exp(&swarm.log_weights) - (n_particles as f64).ln();
        total_loglik += ll_increment;
        ll_increments.push(ll_increment);
        ess_trace.push(swarm.ess());

        // Resample via double-buffer
        let indices = systematic_resample(&swarm.log_weights, &mut resample_rng);
        for (i, &src) in indices.iter().enumerate() {
            states_buf[i].counts.copy_from_slice(&swarm.states[src].counts);
            states_buf[i].flow_accumulators.copy_from_slice(&swarm.states[src].flow_accumulators);
        }
        std::mem::swap(&mut swarm.states, &mut states_buf);

        // Reset flow accumulators for next observation interval
        for state in &mut swarm.states {
            state.reset_flows();
        }

        // Reset weights
        for lw in &mut swarm.log_weights { *lw = 0.0; }
    }

    Ok(PFilterResult {
        log_likelihood: total_loglik,
        predictions: if has_predictions { Some(predictions) } else { None },
        ess_trace,
        ll_increments,
        final_states: Some(swarm.states),
    })
}

/// Weighted mean and quantiles from log-weighted samples.
/// Returns (mean, q05, q50, q95).
fn weighted_quantiles(values: &[f64], log_weights: &[f64]) -> (f64, f64, f64, f64) {
    let n = values.len();
    if n == 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }

    // Normalize weights
    let max_lw = log_weights.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = if max_lw.is_infinite() {
        vec![1.0 / n as f64; n]
    } else {
        let raw: Vec<f64> = log_weights.iter().map(|&lw| (lw - max_lw).exp()).collect();
        let sum: f64 = raw.iter().sum();
        if sum == 0.0 { vec![1.0 / n as f64; n] }
        else { raw.iter().map(|&w| w / sum).collect() }
    };

    // Weighted mean
    let mean: f64 = values.iter().zip(&weights).map(|(&v, &w)| v * w).sum();

    // Weighted quantiles: sort by value, walk cumulative weight
    let mut sorted: Vec<(f64, f64)> = values.iter().zip(&weights).map(|(&v, &w)| (v, w)).collect();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let quantile = |p: f64| -> f64 {
        let mut cumw = 0.0;
        for &(val, w) in &sorted {
            cumw += w;
            if cumw >= p { return val; }
        }
        sorted.last().map_or(0.0, |&(v, _)| v)
    };

    (mean, quantile(0.05), quantile(0.50), quantile(0.95))
}
