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
    /// If true, this parameter is only perturbed at t=0 (initial value
    /// parameter). Used for S₀, E₀, I₀ etc. that set the initial state
    /// but don't change during simulation. Matches pomp's ivp() in rw.sd.
    pub ivp: bool,
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

    /// Convert rw_sd from natural scale to transformed scale using the
    /// delta method at the current parameter value.
    ///
    /// For log:   sd_transformed ≈ rw_sd / current_value
    /// For logit: sd_transformed ≈ rw_sd / (current_value × (1 - current_value))
    ///            (scaled to the [lower, upper] interval)
    /// For none:  sd_transformed = rw_sd
    ///
    /// This matches pomp's convention: the user specifies rw.sd on the
    /// natural scale, and the perturbation happens on the transformed scale
    /// with the appropriate Jacobian correction.
    fn transformed_sd(&self, natural_sd: f64, current_value: f64) -> f64 {
        match self.transform {
            Transform::Log => {
                let v = current_value.max(1e-300);
                natural_sd / v
            }
            Transform::Logit => {
                let range = self.upper - self.lower;
                let p = ((current_value - self.lower) / range).clamp(1e-10, 1.0 - 1e-10);
                natural_sd / (range * p * (1.0 - p))
            }
            Transform::None => natural_sd,
        }
    }
}

/// IF2 configuration.
pub struct IF2Config {
    pub n_particles: usize,
    pub n_iterations: usize,
    /// Cooling schedule: after `cooling_target_iters` iterations,
    /// perturbation SD is `cooling_fraction` of initial.
    /// Matches pomp's cooling.fraction.50 semantics when
    /// cooling_target_iters = 50.
    ///
    /// Cooling factor per filtering step (observation):
    ///   c = cooling_fraction ^ (1 / (cooling_target_iters * n_obs))
    /// After m iterations × n_obs steps each:
    ///   effective_sd = rw_sd × c^(m * n_obs)
    pub cooling_fraction: f64,
    /// Number of iterations over which the cooling fraction applies.
    /// pomp default: 50 (cooling.fraction.50).
    pub cooling_target_iters: usize,
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
    /// MLE estimate: param means from the best-loglik iteration.
    pub mle: Vec<f64>,
    /// Best log-likelihood across all iterations.
    pub final_loglik: f64,
    /// Last iteration's log-likelihood (for diagnostics).
    pub last_loglik: f64,
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
/// Optional callback invoked after each IF2 iteration.
/// Arguments: (iteration_index, log_likelihood).
pub type ProgressCallback<'a> = Option<&'a dyn Fn(usize, f64)>;

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
    run_if2_with_progress(model, base_params, if2_params, observations, config,
        step_fn, project_fn, dmeasure_fn, seed, None)
}

pub fn run_if2_with_progress(
    model: &CompiledModel,
    base_params: &[f64],
    if2_params: &[IF2Param],
    observations: &[Observation],
    config: &IF2Config,
    step_fn: &dyn Fn(&mut ParticleState, &[f64], f64, f64, &mut StatefulRng) -> Result<(), SimError>,
    project_fn: &dyn Fn(&ParticleState) -> f64,
    dmeasure_fn: &dyn Fn(f64, f64, &[f64]) -> f64,
    seed: u64,
    on_iteration: ProgressCallback,
) -> Result<IF2Result, SimError> {
    let n = config.n_particles;
    let n_int = model.int_local_to_global.len();
    let n_tr = model.model.transitions.len();

    // Mutable copy of params — updated each iteration with the filter mean
    let mut current_params = base_params.to_vec();

    // Compute per-filtering-step cooling factor.
    // After cooling_target_iters × n_obs steps, SD is cooling_fraction of initial.
    // Per-step factor: c = cooling_fraction ^ (1 / (target_iters * n_obs))
    let n_obs = observations.len();
    let total_target_steps = config.cooling_target_iters as f64 * n_obs as f64;
    let per_step_cooling = config.cooling_fraction.powf(1.0 / total_target_steps);

    let mut iterations = Vec::with_capacity(config.n_iterations);
    let mut global_step: u64 = 0; // total filtering steps across all iterations

    for iter in 0..config.n_iterations {

        // Initialize particles: all start from model's initial state
        let (init_int, _) = model.initial_state(&current_params)?;
        let mut states: Vec<ParticleState> = (0..n)
            .map(|_| {
                let mut s = ParticleState::new(n_int, n_tr);
                s.counts.copy_from_slice(&init_int.counts);
                s
            })
            .collect();

        // Per-particle parameter vectors, initialized from current estimate
        let mut particle_params: Vec<Vec<f64>> = vec![current_params.clone(); n];

        // Per-particle RNGs
        let mut rngs: Vec<StatefulRng> = (0..n)
            .map(|i| StatefulRng::new(
                seed ^ ((iter as u64) << 32) ^ (i as u64).wrapping_mul(0x517cc1b727220a95)
            ))
            .collect();
        let mut resample_rng = StatefulRng::new(
            seed.wrapping_add(0xdeadbeef).wrapping_add(iter as u64)
        );

        // Initial parameter perturbation (at t=0)
        {
            let cooling_now = per_step_cooling.powi(global_step as i32);
            for i in 0..n {
                for spec in if2_params {
                    let current = particle_params[i][spec.index];
                    let sd = spec.transformed_sd(spec.rw_sd, current) * cooling_now;
                    let z = spec.to_transformed(current);
                    particle_params[i][spec.index] = spec.from_transformed(z + rngs[i].normal() * sd);
                }
            }
            global_step += 1;
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

            // Perturb parameters at observation time (per-step cooling).
            // IVP params are skipped here — they were only perturbed at t=0.
            let cooling_now = per_step_cooling.powi(global_step as i32);
            for i in 0..n {
                for spec in if2_params {
                    if spec.ivp { continue; } // IVP: perturbed at t=0 only
                    let current = particle_params[i][spec.index];
                    let sd = spec.transformed_sd(spec.rw_sd, current) * cooling_now;
                    let z = spec.to_transformed(current);
                    particle_params[i][spec.index] = spec.from_transformed(z + rngs[i].normal() * sd);
                }
            }

            global_step += 1;

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

        // Compute parameter means across particles → next iteration's starting point
        let mut param_means = current_params.clone();
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

        // Report progress
        if let Some(cb) = &on_iteration {
            cb(iter, total_loglik);
        }

        // Feed filter mean back as next iteration's starting params
        current_params = param_means;
    }

    let last_iter = iterations.last().unwrap();
    let best_iter = iterations.iter()
        .filter(|it| it.log_likelihood.is_finite())
        .max_by(|a, b| a.log_likelihood.partial_cmp(&b.log_likelihood).unwrap())
        .unwrap_or(last_iter);

    Ok(IF2Result {
        mle: best_iter.param_means.clone(),
        final_loglik: best_iter.log_likelihood,
        last_loglik: last_iter.log_likelihood,
        iterations,
    })
}

