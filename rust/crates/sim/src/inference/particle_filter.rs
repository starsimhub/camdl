//! Bootstrap particle filter (Gordon, Salmond & Smith 1993).
//!
//! Estimates log p(y_{1:T} | θ) via sequential importance sampling
//! with systematic resampling. Uses the ProcessSimulator trait to
//! advance particles — any simulation backend works (chain-binomial,
//! tau-leap, etc.).

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

/// Result of a particle filter run.
pub struct PFilterResult {
    /// Estimated log p(y_{1:T} | θ).
    pub log_likelihood: f64,
    /// ESS at each observation time.
    pub ess_trace: Vec<f64>,
    /// Log-likelihood increment at each observation time.
    pub ll_increments: Vec<f64>,
}

/// Signature for a single-step function that advances particle state.
/// This is what the ProcessSimulator trait provides, but we use a closure
/// for flexibility (allows capturing the CompiledModel and params).
pub type StepFn<'a> = dyn Fn(&mut ParticleState, f64, f64, &mut StatefulRng) -> Result<(), SimError> + 'a;

/// Signature for the observation log-likelihood (dmeasure).
/// Takes (projected_value, observed_value) → log p(y | projected, θ).
pub type DmeasureFn<'a> = dyn Fn(f64, f64) -> f64 + 'a;

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

    let mut total_loglik = 0.0;
    let mut ess_trace = Vec::with_capacity(observations.len());
    let mut ll_increments = Vec::with_capacity(observations.len());
    let mut t = model.model.simulation.t_start;

    // Resampling RNG (separate from particle RNGs)
    let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));

    for obs in observations {
        // Propagate all particles from t to obs.time using sub-steps
        while t < obs.time - 1e-10 {
            let step_dt = dt.min(obs.time - t);
            for (i, state) in swarm.states.iter_mut().enumerate() {
                step_fn(state, t, step_dt, &mut rngs[i])?;
            }
            t += step_dt;
        }

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

        // Resample
        let indices = systematic_resample(&swarm.log_weights, &mut resample_rng);
        let old_states: Vec<ParticleState> = swarm.states.clone();
        for (i, &src) in indices.iter().enumerate() {
            swarm.states[i] = old_states[src].clone();
        }

        // Reset flow accumulators for next observation interval
        for state in &mut swarm.states {
            state.reset_flows();
        }

        // Reset weights (after resampling, all particles are equally weighted)
        for lw in &mut swarm.log_weights { *lw = 0.0; }
    }

    Ok(PFilterResult {
        log_likelihood: total_loglik,
        ess_trace,
        ll_increments,
    })
}
