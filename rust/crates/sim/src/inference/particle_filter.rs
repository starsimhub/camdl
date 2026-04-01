//! Bootstrap particle filter (Gordon, Salmond & Smith 1993).
//!
//! Estimates log p(y_{1:T} | θ) via sequential importance sampling
//! with systematic resampling. Uses the ProcessSimulator trait to
//! advance particles — any simulation backend works (chain-binomial,
//! tau-leap, etc.).

use rayon::prelude::*;

use crate::chain_binomial::StepScratch;
use crate::compiled_model::CompiledModel;
use crate::ekrng::StatefulRng;
use crate::error::SimError;
use super::types::{ParticleState, ParticleSwarm, log_sum_exp};
use super::resampling::systematic_resample;

/// Observation: one data point at a specific time.
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
    /// Weighted quantiles of the projected quantity (before resampling).
    pub predictions: Vec<PredictionDiag>,
    /// Final particle states after the last observation (post-resampling).
    /// Only populated when `save_final_state` is true.
    pub final_states: Option<Vec<ParticleState>>,
}

/// Signature for a single-step function that advances particle state.
/// This is what the ProcessSimulator trait provides, but we use a closure
/// for flexibility (allows capturing the CompiledModel and params).
/// Takes a `&mut StepScratch` to avoid per-call heap allocations.
/// `Send + Sync` required for rayon parallel particle propagation.
pub type StepFn<'a> = dyn Fn(&mut ParticleState, f64, f64, &mut StatefulRng, &mut StepScratch) -> Result<(), SimError> + Send + Sync + 'a;

/// Signature for the observation log-likelihood (dmeasure).
/// Takes (projected_value, observed_value) → log p(y | projected, θ).
pub type DmeasureFn<'a> = dyn Fn(f64, f64) -> f64 + 'a;

/// Signature for observation model sampler (rmeasure).
/// Takes (projected_value, rng) → observation draw.
pub type RmeasureFn<'a> = dyn Fn(f64, &mut StatefulRng) -> f64 + 'a;

/// Signature for observation model mean (no sampling).
/// Takes (projected_value) → E[y | projected, θ].
pub type ObsMeanFn<'a> = dyn Fn(f64) -> f64 + 'a;

/// Run the bootstrap particle filter.
///
/// # Arguments
/// * `model` — compiled model (for initial state, structure)
/// * `params` — parameter values
/// * `observations` — data, sorted by time
/// * `n_particles` — number of particles
/// * `dt` — sub-step size (e.g., 1.0 for daily steps)
/// * `step_fn` — advances one particle by dt
/// * `project_fn` — extracts the projected quantity from a particle (e.g., cumulative infection flow)
/// * `dmeasure_fn` — observation log-likelihood
/// * `seed` — RNG seed
pub fn bootstrap_filter(
    model: &CompiledModel,
    params: &[f64],
    observations: &[Observation],
    n_particles: usize,
    dt: f64,
    step_fn: &StepFn,
    project_fn: &dyn Fn(&ParticleState) -> f64,
    dmeasure_fn: &DmeasureFn,
    rmeasure_fn: Option<&RmeasureFn>,
    obs_mean_fn: Option<&ObsMeanFn>,
    seed: u64,
) -> Result<PFilterResult, SimError> {
    let n_int = model.int_local_to_global.len();
    let n_tr = model.model.transitions.len();

    // Initialize particles from model init
    let (init_int, _init_real) = model.initial_state(params)?;
    let mut swarm = ParticleSwarm::new(n_particles, n_int, n_tr);
    for p in &mut swarm.states {
        p.counts.copy_from_slice(&init_int.counts);
    }

    // Per-particle RNG streams (deterministic, derived from seed)
    let mut rngs: Vec<StatefulRng> = (0..n_particles)
        .map(|i| StatefulRng::new(seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95)))
        .collect();

    // Double-buffer for resampling (avoids clone allocation)
    let mut states_buf: Vec<ParticleState> = (0..n_particles)
        .map(|_| ParticleState::new(n_int, n_tr))
        .collect();

    // Per-particle scratch buffers (allocated once, reused across all steps)
    let mut scratches: Vec<StepScratch> = (0..n_particles)
        .map(|_| StepScratch::new(model))
        .collect();

    let mut total_loglik = 0.0;
    let mut ess_trace = Vec::with_capacity(observations.len());
    let mut ll_increments = Vec::with_capacity(observations.len());
    let mut predictions = Vec::with_capacity(observations.len());
    let mut t = model.model.simulation.t_start;

    // Resampling RNG (separate from particle RNGs)
    let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));

    for obs in observations {
        // Propagate all particles from t to obs.time.
        // Batched: one rayon dispatch per observation interval. Each thread
        // runs all sub-steps for its particles, keeping state in L1/L2.
        let obs_time = obs.time;
        let t_start = t;
        let errors: Vec<Result<(), SimError>> = swarm.states.par_iter_mut()
            .zip(rngs.par_iter_mut())
            .zip(scratches.par_iter_mut())
            .map(|((state, rng), scratch)| {
                let mut t_local = t_start;
                while t_local < obs_time - 1e-10 {
                    let step_dt = dt.min(obs_time - t_local);
                    step_fn(state, t_local, step_dt, rng, scratch)?;
                    t_local += step_dt;
                }
                Ok(())
            })
            .collect();
        // Check for errors from any particle
        for r in errors { r?; }
        // Advance shared time to match
        while t < obs.time - 1e-10 { t += dt.min(obs.time - t); }

        // Compute projections (before weighting — prior predictive)
        let projections: Vec<f64> = swarm.states.iter().map(|s| project_fn(s)).collect();

        // One-step-ahead prediction diagnostics (PRIOR predictive — equal weights)
        // State quantiles: process uncertainty only
        let equal_lw = vec![0.0_f64; n_particles]; // log(1/N) for all — equal weights
        let state_diag = weighted_prediction_diag(&projections, &equal_lw);

        // Observation-space predictions: process + observation noise
        let obs_diag = if let (Some(rmfn), Some(omfn)) = (rmeasure_fn, obs_mean_fn) {
            // Draw one obs sample per particle, compute quantiles
            let obs_draws: Vec<f64> = projections.iter().enumerate()
                .map(|(i, &proj)| rmfn(proj, &mut rngs[i]))
                .collect();
            let obs_q = weighted_prediction_diag(&obs_draws, &equal_lw);
            // obs_mean: analytic mean of observation model, averaged across particles
            let obs_mean = projections.iter().map(|&p| omfn(p)).sum::<f64>() / n_particles as f64;
            (obs_mean, obs_q.state_q05, obs_q.state_q50, obs_q.state_q95)
        } else {
            // Fallback: use state quantiles (no rmeasure available)
            (state_diag.state_mean, state_diag.state_q05, state_diag.state_q50, state_diag.state_q95)
        };

        predictions.push(PredictionDiag {
            obs_mean: obs_diag.0,
            obs_q05: obs_diag.1,
            obs_q50: obs_diag.2,
            obs_q95: obs_diag.3,
            state_mean: state_diag.state_mean,
            state_q05: state_diag.state_q05,
            state_q50: state_diag.state_q50,
            state_q95: state_diag.state_q95,
        });

        // Compute log-weights: log p(y_k | projected_i)
        for (i, state) in swarm.states.iter().enumerate() {
            let projected = project_fn(state);
            swarm.log_weights[i] = dmeasure_fn(projected, obs.value);
        }

        // Log-marginal increment: log(1/N × Σ exp(log_w))
        let ll_increment = log_sum_exp(&swarm.log_weights) - (n_particles as f64).ln();
        total_loglik += ll_increment;
        ll_increments.push(ll_increment);
        ess_trace.push(swarm.ess());

        // Resample via double-buffer (no allocation)
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

        // Reset weights (after resampling, all particles are equally weighted)
        for lw in &mut swarm.log_weights { *lw = 0.0; }
    }

    Ok(PFilterResult {
        log_likelihood: total_loglik,
        predictions,
        ess_trace,
        ll_increments,
        final_states: Some(swarm.states),
    })
}

/// Compute weighted mean and quantiles from log-weighted samples.
/// Used for one-step-ahead prediction diagnostics.
fn weighted_prediction_diag(values: &[f64], log_weights: &[f64]) -> PredictionDiag {
    let n = values.len();
    if n == 0 {
        return PredictionDiag {
            obs_mean: 0.0, obs_q05: 0.0, obs_q50: 0.0, obs_q95: 0.0,
            state_mean: 0.0, state_q05: 0.0, state_q50: 0.0, state_q95: 0.0,
        };
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

    PredictionDiag {
        obs_mean: mean, obs_q05: quantile(0.05), obs_q50: quantile(0.50), obs_q95: quantile(0.95),
        state_mean: mean, state_q05: quantile(0.05), state_q50: quantile(0.50), state_q95: quantile(0.95),
    }
}
