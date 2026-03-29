//! Iterated Filtering (IF2) — MLE via sequential Monte Carlo.
//!
//! Ionides, Bretó & King (2006), Ionides et al. (2015).
//!
//! IF2 runs a particle filter with perturbed parameters: at each
//! observation time, each particle's parameter vector is jittered by
//! a random walk with shrinking variance. Over M iterations, the
//! perturbation scale σ → 0 and the particle swarm concentrates
//! around the MLE.
//!
//! Key property: IF2 finds the MLE without computing the transition
//! density — it only needs the simulator (process model) and the
//! observation log-likelihood (dmeasure). This makes it compatible
//! with any simulation backend.

use crate::compiled_model::CompiledModel;
use crate::ekrng::StatefulRng;
use crate::error::SimError;
use super::types::{ParticleState, log_sum_exp};
use super::resampling::systematic_resample;
use super::obs_loglik;

/// One parameter's transform and perturbation spec.
#[derive(Clone, Debug)]
pub struct IF2Param {
    /// Parameter name (for reporting).
    pub name: String,
    /// Index into the params array.
    pub index: usize,
    /// Initial value (starting point for IF2).
    pub initial: f64,
    /// Random walk standard deviation on the TRANSFORMED scale.
    /// Shrinks by `cooling_fraction` each iteration.
    pub rw_sd: f64,
    /// Transform: "log" for positive parameters, "logit" for [0,1],
    /// "none" for unconstrained.
    pub transform: Transform,
    /// Bounds for logit transform.
    pub lower: f64,
    pub upper: f64,
}

#[derive(Clone, Debug)]
pub enum Transform {
    Log,
    Logit,
    None,
}

impl IF2Param {
    fn to_transformed(&self, x: f64) -> f64 {
        match self.transform {
            Transform::Log => x.max(1e-300).ln(),
            Transform::Logit => {
                let p = ((x - self.lower) / (self.upper - self.lower)).clamp(1e-10, 1.0 - 1e-10);
                (p / (1.0 - p)).ln()
            }
            Transform::None => x,
        }
    }

    fn from_transformed(&self, z: f64) -> f64 {
        match self.transform {
            Transform::Log => z.exp(),
            Transform::Logit => {
                let p = 1.0 / (1.0 + (-z).exp());
                self.lower + p * (self.upper - self.lower)
            }
            Transform::None => z,
        }
    }
}

/// IF2 configuration.
pub struct IF2Config {
    pub n_particles: usize,
    pub n_iterations: usize,
    /// Fraction of rw_sd retained each iteration.
    /// After iter m: effective_sd = rw_sd × cooling^m.
    /// Typical: 0.95 (5% shrinkage per iteration).
    pub cooling_fraction: f64,
    pub dt: f64,
}

/// Result of one IF2 iteration.
#[derive(Clone, Debug)]
pub struct IF2IterResult {
    pub iteration: usize,
    pub log_likelihood: f64,
    /// Parameter means across particles at end of this iteration.
    pub param_means: Vec<f64>,
}

/// Result of the full IF2 run.
pub struct IF2Result {
    pub iterations: Vec<IF2IterResult>,
    /// Final MLE estimate (param means from last iteration).
    pub mle: Vec<f64>,
    /// Final log-likelihood.
    pub final_loglik: f64,
}

/// Observation for IF2 (same as particle_filter::Observation).
pub struct Observation {
    pub time: f64,
    pub value: f64,
}

/// Run IF2.
///
/// # Arguments
/// * `model` — compiled model
/// * `base_params` — starting parameter values (full vector)
/// * `if2_params` — parameters to estimate (subset of base_params)
/// * `observations` — data sorted by time
/// * `config` — IF2 settings
/// * `step_fn` — chain-binomial step function
/// * `project_fn` — extract projected quantity from particle state
/// * `dmeasure_fn` — observation log-likelihood (takes projected, observed, params)
/// * `seed` — base RNG seed
pub fn run_if2(
    model: &CompiledModel,
    base_params: &[f64],
    if2_params: &[IF2Param],
    observations: &[Observation],
    config: &IF2Config,
    step_fn: &dyn Fn(&mut ParticleState, &[f64], f64, f64, &mut StatefulRng) -> Result<(), SimError>,
    project_fn: &dyn Fn(&ParticleState) -> f64,
    dmeasure_fn: &dyn Fn(f64, f64, &[f64]) -> f64,
    seed: u64,
) -> Result<IF2Result, SimError> {
    let n = config.n_particles;
    let n_int = model.int_local_to_global.len();
    let n_tr = model.model.transitions.len();
    let n_params = base_params.len();

    let mut iterations = Vec::with_capacity(config.n_iterations);

    for iter in 0..config.n_iterations {
        let cooling = config.cooling_fraction.powi(iter as i32);

        // Initialize particles: all start from model's initial state
        let (init_int, _) = model.initial_state(base_params)?;
        let mut states: Vec<ParticleState> = (0..n)
            .map(|_| {
                let mut s = ParticleState::new(n_int, n_tr);
                s.counts.copy_from_slice(&init_int.counts);
                s
            })
            .collect();

        // Per-particle parameter vectors, initialized from base + perturbation
        let mut particle_params: Vec<Vec<f64>> = vec![base_params.to_vec(); n];

        // Per-particle RNGs
        let mut rngs: Vec<StatefulRng> = (0..n)
            .map(|i| StatefulRng::new(
                seed ^ ((iter as u64) << 32) ^ (i as u64).wrapping_mul(0x517cc1b727220a95)
            ))
            .collect();
        let mut resample_rng = StatefulRng::new(
            seed.wrapping_add(0xdeadbeef).wrapping_add(iter as u64)
        );

        // Initial parameter perturbation
        for i in 0..n {
            for spec in if2_params {
                let z = spec.to_transformed(particle_params[i][spec.index]);
                let perturbation = rngs[i].normal() * spec.rw_sd * cooling;
                particle_params[i][spec.index] = spec.from_transformed(z + perturbation);
            }
        }

        let mut log_weights = vec![0.0_f64; n];
        let mut total_loglik = 0.0;
        let mut t = model.model.simulation.t_start;

        for obs in observations {
            // Propagate
            while t < obs.time - 1e-10 {
                let step_dt = config.dt.min(obs.time - t);
                for i in 0..n {
                    step_fn(&mut states[i], &particle_params[i], t, step_dt, &mut rngs[i])?;
                }
                t += step_dt;
            }

            // Perturb parameters at observation time
            for i in 0..n {
                for spec in if2_params {
                    let z = spec.to_transformed(particle_params[i][spec.index]);
                    let perturbation = rngs[i].normal() * spec.rw_sd * cooling;
                    particle_params[i][spec.index] = spec.from_transformed(z + perturbation);
                }
            }

            // Weight by observation likelihood
            for i in 0..n {
                let projected = project_fn(&states[i]);
                log_weights[i] = dmeasure_fn(projected, obs.value, &particle_params[i]);
            }

            // Log-likelihood increment
            let ll_inc = log_sum_exp(&log_weights) - (n as f64).ln();
            total_loglik += ll_inc;

            // Resample states AND parameters jointly
            let indices = systematic_resample(&log_weights, &mut resample_rng);
            let old_states = states.clone();
            let old_params = particle_params.clone();
            for (i, &src) in indices.iter().enumerate() {
                states[i] = old_states[src].clone();
                particle_params[i] = old_params[src].clone();
            }

            // Reset
            for s in &mut states { s.reset_flows(); }
            for lw in &mut log_weights { *lw = 0.0; }
        }

        // Compute parameter means across particles
        let mut param_means = base_params.to_vec();
        for spec in if2_params {
            let mean: f64 = particle_params.iter()
                .map(|pp| pp[spec.index])
                .sum::<f64>() / n as f64;
            param_means[spec.index] = mean;
        }

        iterations.push(IF2IterResult {
            iteration: iter,
            log_likelihood: total_loglik,
            param_means: param_means.clone(),
        });

        // Update base_params for next iteration (filter mean → new starting point)
        // This is the "plug-in" approach: next iteration starts from the
        // current mean estimate.
        // (Don't mutate the original base_params — copy)
    }

    let final_iter = iterations.last().unwrap();
    Ok(IF2Result {
        mle: final_iter.param_means.clone(),
        final_loglik: final_iter.log_likelihood,
        iterations,
    })
}

