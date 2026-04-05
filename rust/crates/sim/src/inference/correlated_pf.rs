//! Correlated pseudo-marginal particle filter.
//!
//! Stores all random draws as standard normals and transforms them at
//! evaluation time. The Crank-Nicolson update `u' = ρu + √(1-ρ²)z`
//! correlates successive PF evaluations so the likelihood RATIO has
//! low variance even when individual estimates are noisy.
//!
//! Reference: Deligiannidis, Doucet & Pitt (2018), JRSSB.

use rayon::prelude::*;
use crate::chain_binomial::StepScratch;
use crate::compiled_model::CompiledModel;
use crate::rng::StatefulRng;
use crate::error::SimError;
use super::types::{ParticleState, ParticleSwarm, log_sum_exp};
use super::particle_filter::{StepFn, DmeasureFn, Observation, PFilterResult};

/// Pre-drawn random state for one PF evaluation.
///
/// All values are standard normals. Transformed to the target distribution
/// (Gamma, Uniform) at consumption time.
#[derive(Clone)]
pub struct PFRandomState {
    /// Gamma multiplier draws: gamma_noise[obs_idx][particle_idx * steps_per_obs + step]
    /// One normal per overdispersed transition per substep per particle.
    /// Transformed to Gamma(shape, scale) via inverse CDF.
    pub gamma_noise: Vec<Vec<f64>>,

    /// Resampling draws: one normal per observation.
    /// Transformed to Uniform(0,1) via Phi(·) for systematic resampling.
    pub resample_noise: Vec<f64>,
}

impl PFRandomState {
    /// Draw a fresh random state for one PF evaluation.
    pub fn draw_fresh(
        n_particles: usize,
        n_obs: usize,
        steps_per_obs: usize,
        rng: &mut StatefulRng,
    ) -> Self {
        let gamma_noise = (0..n_obs)
            .map(|_| (0..n_particles * steps_per_obs)
                .map(|_| rng.normal())
                .collect())
            .collect();
        let resample_noise = (0..n_obs)
            .map(|_| rng.normal())
            .collect();
        PFRandomState { gamma_noise, resample_noise }
    }

    /// Crank-Nicolson update: u' = ρu + √(1-ρ²)z, z ~ N(0,1).
    /// Returns a new PFRandomState correlated with self.
    pub fn correlate(&self, rho: f64, rng: &mut StatefulRng) -> Self {
        let scale = (1.0 - rho * rho).sqrt();
        let gamma_noise = self.gamma_noise.iter()
            .map(|row| row.iter()
                .map(|&x| rho * x + scale * rng.normal())
                .collect())
            .collect();
        let resample_noise = self.resample_noise.iter()
            .map(|&x| rho * x + scale * rng.normal())
            .collect();
        PFRandomState { gamma_noise, resample_noise }
    }
}

/// Transform a standard normal to a Gamma(shape, scale) draw via inverse CDF.
/// Uses the Wilson-Hilferty approximation for the inverse Gamma CDF.
fn normal_to_gamma(z: f64, shape: f64, scale: f64) -> f64 {
    if shape < 1e-6 { return 1.0; } // degenerate: no overdispersion
    // Wilson-Hilferty: if X ~ Gamma(shape, 1), then
    //   ((X/shape)^(1/3) - (1 - 1/(9*shape))) / sqrt(1/(9*shape)) ≈ N(0,1)
    // Invert: X = shape * (1 - 1/(9*shape) + z/sqrt(9*shape))^3
    let c = 1.0 / (9.0 * shape);
    let cube = 1.0 - c + z * c.sqrt();
    let x = if cube > 0.0 { shape * cube * cube * cube } else { 0.0 };
    x * scale // scale from Gamma(shape, 1) to Gamma(shape, scale)
}

/// Standard normal CDF (same as obs_loglik::normal_cdf).
fn phi(x: f64) -> f64 {
    let z = x / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.3275911 * z.abs());
    let poly = t * (0.254829592
        + t * (-0.284496736
        + t * (1.421413741
        + t * (-1.453152027
        + t * 1.061405429))));
    let erf_abs = 1.0 - poly * (-z * z).exp();
    let erf_val = if z >= 0.0 { erf_abs } else { -erf_abs };
    0.5 * (1.0 + erf_val)
}

/// Run the bootstrap particle filter with pre-drawn correlated randoms.
///
/// The Gamma multiplier for overdispersed transitions is drawn from
/// `randoms.gamma_noise` (transformed from normal to Gamma via inverse CDF).
/// Systematic resampling uses `randoms.resample_noise` (transformed to
/// uniform via Phi). All other draws (binomial in reulermultinom) use
/// per-particle RNGs seeded from the gamma noise for partial correlation.
pub fn bootstrap_filter_correlated(
    model: &CompiledModel,
    params: &[f64],
    observations: &[Observation],
    n_particles: usize,
    dt: f64,
    step_fn: &StepFn,
    project_fn: &dyn Fn(&ParticleState) -> f64,
    dmeasure_fn: &DmeasureFn,
    randoms: &PFRandomState,
    seed: u64,
) -> Result<PFilterResult, SimError> {
    let n_int = model.int_local_to_global.len();
    let n_tr = model.model.transitions.len();

    let (init_int, _init_real) = model.initial_state(params)?;
    let mut swarm = ParticleSwarm::new(n_particles, n_int, n_tr);
    for p in &mut swarm.states {
        p.counts.copy_from_slice(&init_int.counts);
    }

    // Per-particle RNGs — used for binomial draws (not correlated).
    // Seeded from the base seed for reproducibility.
    let mut rngs: Vec<StatefulRng> = (0..n_particles)
        .map(|i| StatefulRng::new(seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95)))
        .collect();

    let mut states_buf: Vec<ParticleState> = (0..n_particles)
        .map(|_| ParticleState::new(n_int, n_tr))
        .collect();

    let mut scratches: Vec<StepScratch> = (0..n_particles)
        .map(|_| StepScratch::new(model))
        .collect();

    let mut total_loglik = 0.0;
    let mut ess_trace = Vec::with_capacity(observations.len());
    let mut ll_increments = Vec::with_capacity(observations.len());
    let mut t = model.model.simulation.t_start;

    // Compute steps per observation interval
    let obs_dt = if observations.len() > 1 {
        observations[1].time - observations[0].time
    } else {
        observations.first().map_or(1.0, |o| o.time - model.model.simulation.t_start)
    };
    let steps_per_obs = (obs_dt / dt).round() as usize;

    // Gamma shape/scale for the overdispersed transition (precompute)
    let sigma_sq = model.model.transitions.iter()
        .find_map(|tr| match &tr.draw_method {
            ir::transition::DrawMethod::Overdispersed(_) => {
                // Get sigma_sq from params — this is model-specific
                // For now, evaluate the expression at t=0
                let int_s = crate::state::IntState::new(n_int);
                let real_s = crate::state::RealState::new(model.real_local_to_global.len());
                let ctx = crate::propensity::EvalCtx {
                    model, int_s: &int_s, real_s: &real_s, params,
                    t: 0.0, projected: None,
                };
                crate::propensity::eval_expr(
                    match &tr.draw_method {
                        ir::transition::DrawMethod::Overdispersed(e) => e,
                        _ => unreachable!(),
                    },
                    &ctx,
                ).ok()
            }
            _ => None,
        })
        .unwrap_or(1.0);

    let gamma_shape = dt / sigma_sq;
    let gamma_scale = sigma_sq / dt;

    for (obs_idx, obs) in observations.iter().enumerate() {
        let obs_time = obs.time;
        let t_start = t;

        // Propagate particles with pre-drawn Gamma noise (parallel)
        let gamma_row = &randoms.gamma_noise[obs_idx];
        let errors: Vec<Result<(), SimError>> = swarm.states.par_iter_mut()
            .zip(rngs.par_iter_mut())
            .zip(scratches.par_iter_mut())
            .enumerate()
            .map(|(i, ((state, rng), scratch))| {
                // Seed particle RNG from correlated gamma noise for partial
                // binomial correlation: same gamma draw → same RNG seed →
                // correlated binomial sequence
                let noise_base = gamma_row.get(i * steps_per_obs)
                    .map(|&z| z.to_bits()).unwrap_or(0);
                *rng = StatefulRng::new(seed ^ noise_base ^ (obs_idx as u64 * 0x9e3779b9));

                let mut t_local = t_start;
                let mut substep = 0;
                while t_local < obs_time - 1e-10 {
                    let step_dt = dt.min(obs_time - t_local);

                    let noise_idx = i * steps_per_obs + substep;
                    if noise_idx < gamma_row.len() {
                        let z = gamma_row[noise_idx];
                        let g = normal_to_gamma(z, gamma_shape, gamma_scale);
                        scratch.gamma_override = Some(g);
                    }

                    step_fn(state, t_local, step_dt, rng, scratch)?;
                    t_local += step_dt;
                    substep += 1;
                }
                Ok(())
            })
            .collect();
        for r in errors { r?; }
        while t < obs.time - 1e-10 { t += dt.min(obs.time - t); }

        // Compute log-weights
        for (i, state) in swarm.states.iter().enumerate() {
            let projected = project_fn(state);
            swarm.log_weights[i] = dmeasure_fn(projected, obs.value);
        }

        let ll_increment = log_sum_exp(&swarm.log_weights) - (n_particles as f64).ln();
        total_loglik += ll_increment;
        ll_increments.push(ll_increment);
        ess_trace.push(swarm.ess());

        // Sorted systematic resampling with correlated uniform
        // Sort particles by projected value for correlation preservation
        let mut sort_order: Vec<usize> = (0..n_particles).collect();
        {
            let projections: Vec<f64> = swarm.states.iter().map(|s| project_fn(s)).collect();
            sort_order.sort_by(|&a, &b| projections[a].total_cmp(&projections[b]));
        }

        // Resampling using correlated uniform
        let base_uniform = phi(randoms.resample_noise[obs_idx]).clamp(1e-10, 1.0 - 1e-10);

        // Build sorted weights for resampling
        let sorted_weights: Vec<f64> = sort_order.iter()
            .map(|&i| swarm.log_weights[i])
            .collect();

        // Systematic resample with sorted weights and correlated uniform
        let mut resample_rng = StatefulRng::new(
            seed.wrapping_add(0xdeadbeef).wrapping_add(obs_idx as u64)
        );
        // Override the uniform in systematic_resample — we need a custom version
        let indices = sorted_systematic_resample(&sorted_weights, base_uniform);

        // Map sorted indices back to original particle indices
        for (i, &sorted_idx) in indices.iter().enumerate() {
            let orig_idx = sort_order[sorted_idx];
            states_buf[i].counts.copy_from_slice(&swarm.states[orig_idx].counts);
            states_buf[i].flow_accumulators.copy_from_slice(&swarm.states[orig_idx].flow_accumulators);
        }
        std::mem::swap(&mut swarm.states, &mut states_buf);

        // Reset flow accumulators
        for state in &mut swarm.states {
            state.reset_flows();
        }
        for lw in &mut swarm.log_weights { *lw = 0.0; }
    }

    Ok(PFilterResult {
        log_likelihood: total_loglik,
        ess_trace,
        ll_increments,
        predictions: None,
        final_states: Some(swarm.states),
    })
}

/// Systematic resampling with a fixed base uniform (for correlation).
fn sorted_systematic_resample(log_weights: &[f64], base_uniform: f64) -> Vec<usize> {
    let n = log_weights.len();
    if n == 0 { return vec![]; }

    let max_lw = log_weights.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = if max_lw.is_infinite() {
        vec![1.0 / n as f64; n]
    } else {
        let raw: Vec<f64> = log_weights.iter().map(|&lw| (lw - max_lw).exp()).collect();
        let sum: f64 = raw.iter().sum();
        if sum == 0.0 { vec![1.0 / n as f64; n] }
        else { raw.iter().map(|&w| w / sum).collect() }
    };

    let u = base_uniform / n as f64;
    let mut indices = Vec::with_capacity(n);
    let mut cumsum = 0.0;
    let mut j = 0;

    for i in 0..n {
        let threshold = u + i as f64 / n as f64;
        while j < n - 1 && cumsum + weights[j] < threshold {
            cumsum += weights[j];
            j += 1;
        }
        indices.push(j);
    }

    indices
}
