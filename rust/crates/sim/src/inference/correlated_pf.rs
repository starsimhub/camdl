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

    /// Binomial total-exit draws per source group per substep per particle.
    /// binomial_noise[obs_idx][particle * steps_per_obs * n_groups + step * n_groups + group]
    /// Transformed to binomial counts via normal approximation (large np)
    /// or inverse CDF (small np). This is the dominant variance source
    /// that the broken to_bits() seeding failed to correlate.
    pub binomial_noise: Vec<Vec<f64>>,

    /// Number of source groups (for indexing into binomial_noise).
    pub n_source_groups: usize,
}

impl PFRandomState {
    /// Draw a fresh random state for one PF evaluation.
    pub fn draw_fresh(
        n_particles: usize,
        n_obs: usize,
        steps_per_obs: usize,
        n_source_groups: usize,
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
        let binomial_noise = (0..n_obs)
            .map(|_| (0..n_particles * steps_per_obs * n_source_groups)
                .map(|_| rng.normal())
                .collect())
            .collect();
        PFRandomState { gamma_noise, resample_noise, binomial_noise, n_source_groups }
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
        let binomial_noise = self.binomial_noise.iter()
            .map(|row| row.iter()
                .map(|&x| rho * x + scale * rng.normal())
                .collect())
            .collect();
        PFRandomState { gamma_noise, resample_noise, binomial_noise,
                        n_source_groups: self.n_source_groups }
    }
}

/// Transform a standard normal to a binomial draw via normal approximation
/// (large np) or inverse CDF (small np).
///
/// For large np (>20): Binom(n,p) ≈ Normal(np, np(1-p)), so
///   count = round(np + sqrt(np(1-p)) * z)
/// Nearly continuous in z → excellent Crank-Nicolson correlation.
///
/// For small np: use Φ(z) as a uniform, then walk the binomial CDF.
/// Step function in z → partial correlation, but these draws contribute
/// negligible variance to the total log-likelihood.
fn correlated_binomial(z: f64, n: u64, p: f64) -> u64 {
    if n == 0 || p <= 0.0 { return 0; }
    if p >= 1.0 { return n; }
    let np = n as f64 * p;
    let nq = n as f64 * (1.0 - p);
    if np > 20.0 && nq > 20.0 {
        // Normal approximation (exact for large np)
        let sd = (np * (1.0 - p)).sqrt();
        let x = (np + sd * z).round().clamp(0.0, n as f64);
        x as u64
    } else {
        // Small np: inverse CDF via uniform
        let u = phi(z).clamp(1e-15, 1.0 - 1e-15);
        binomial_quantile(n, p, u)
    }
}

/// Inverse binomial CDF: find smallest k such that P(X <= k) >= u.
pub fn binomial_quantile(n: u64, p: f64, u: f64) -> u64 {
    // Walk the CDF from 0. For small np this is fast (< 50 iterations).
    let mut cdf = 0.0;
    let q = 1.0 - p;
    let mut binom_prob = q.powi(n as i32); // P(X=0) = (1-p)^n
    for k in 0..=n {
        cdf += binom_prob;
        if cdf >= u { return k; }
        // P(X=k+1) = P(X=k) * (n-k)/(k+1) * p/(1-p)
        binom_prob *= (n - k) as f64 / (k + 1) as f64 * p / q;
        if binom_prob < 1e-300 { break; } // underflow guard
    }
    n // fallback
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
pub fn phi(x: f64) -> f64 {
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

    // Track which original particle identity each current slot descends from.
    // After resampling, particle_identity[i] = ancestor's original index.
    // z-values are indexed by this identity, not by current slot position.
    let mut particle_identity: Vec<usize> = (0..n_particles).collect();

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

        // Propagate particles with pre-drawn correlated noise (parallel).
        // z-values are indexed by particle_identity (original particle index),
        // NOT by current slot position. This ensures that after resampling,
        // a particle's z-values follow its ancestor through the lineage.
        let gamma_row = &randoms.gamma_noise[obs_idx];
        let binom_row = &randoms.binomial_noise[obs_idx];
        let n_groups = randoms.n_source_groups;
        let identities = &particle_identity;
        let errors: Vec<Result<(), SimError>> = swarm.states.par_iter_mut()
            .zip(rngs.par_iter_mut())
            .zip(scratches.par_iter_mut())
            .enumerate()
            .map(|(i, ((state, rng), scratch))| {
                let pid = identities[i]; // original particle identity
                let mut t_local = t_start;
                let mut substep = 0;
                while t_local < obs_time - 1e-10 {
                    let step_dt = dt.min(obs_time - t_local);

                    // Inject pre-drawn Gamma multiplier (indexed by identity)
                    let noise_idx = pid * steps_per_obs + substep;
                    if noise_idx < gamma_row.len() {
                        let z = gamma_row[noise_idx];
                        let g = normal_to_gamma(z, gamma_shape, gamma_scale);
                        scratch.gamma_override = Some(g);
                    }

                    // Inject pre-drawn binomial z-values per source group
                    scratch.binomial_z_values.clear();
                    for group in 0..n_groups {
                        let binom_idx = pid * steps_per_obs * n_groups + substep * n_groups + group;
                        if binom_idx < binom_row.len() {
                            scratch.binomial_z_values.push(binom_row[binom_idx]);
                        }
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

        // Map sorted indices back to original particle indices and update identity
        let mut new_identity = vec![0usize; n_particles];
        for (i, &sorted_idx) in indices.iter().enumerate() {
            let orig_idx = sort_order[sorted_idx];
            states_buf[i].counts.copy_from_slice(&swarm.states[orig_idx].counts);
            states_buf[i].flow_accumulators.copy_from_slice(&swarm.states[orig_idx].flow_accumulators);
            // Particle i now descends from whatever particle orig_idx was
            new_identity[i] = particle_identity[orig_idx];
        }
        std::mem::swap(&mut swarm.states, &mut states_buf);
        particle_identity = new_identity;

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
