//! Particle Marginal Metropolis-Hastings (PMMH) — Bayesian posterior
//! sampling via MCMC with particle filter likelihood estimation.
//!
//! Andrieu, Doucet & Holenstein (2010).
//!
//! PMMH wraps a standard Metropolis-Hastings sampler around the bootstrap
//! particle filter: at each MCMC step, propose θ' from a random walk on
//! the transformed scale, run the PF at θ' to get an unbiased estimate
//! of log p(y|θ'), then accept/reject via the MH ratio. The PF estimate
//! is noisy but unbiased, which is sufficient for the MCMC to target the
//! exact posterior (pseudo-marginal property).
//!
//! Key differences from IF2:
//! - No cooling schedule — each PF runs at fixed θ.
//! - No parameter perturbation inside the PF.
//! - Acceptance/rejection step gives valid posterior samples.
//! - Optional adaptive proposal covariance (Haario et al. 2001).

use serde::{Serialize, Deserialize};
use crate::rng::StatefulRng;
use super::if2::IF2Param;

// ── Prior ──────────────────────────────────────────────────────────

/// Prior distribution for one estimated parameter.
#[derive(Clone, Debug)]
pub enum Prior {
    /// Flat (improper) prior — log-density = 0 everywhere within transform bounds.
    Flat,
    /// Normal(mean, sd) on the natural scale.
    Normal { mean: f64, sd: f64 },
    /// Normal(mean, sd) on the transformed (log/logit) scale.
    TransformedNormal { mean: f64, sd: f64 },
    /// Beta(alpha, beta) on [0, 1]. For probability parameters.
    Beta { alpha: f64, beta: f64 },
}

impl Prior {
    /// Log-density of the prior at a natural-scale value.
    /// `transformed` is the value on the unconstrained scale (for TransformedNormal).
    pub fn log_density(&self, natural: f64, transformed: f64) -> f64 {
        match self {
            Prior::Flat => 0.0,
            Prior::Normal { mean, sd } => {
                let z = (natural - mean) / sd;
                -0.5 * z * z - sd.ln()
            }
            Prior::TransformedNormal { mean, sd } => {
                let z = (transformed - mean) / sd;
                -0.5 * z * z - sd.ln()
            }
            Prior::Beta { alpha, beta } => {
                // log Beta(x; a, b) = (a-1)ln(x) + (b-1)ln(1-x) - lnB(a,b)
                if natural <= 0.0 || natural >= 1.0 { return f64::NEG_INFINITY; }
                use crate::inference::obs_loglik::lgamma;
                (alpha - 1.0) * natural.ln() + (beta - 1.0) * (1.0 - natural).ln()
                    - (lgamma(*alpha) + lgamma(*beta) - lgamma(alpha + beta))
            }
        }
    }
}

// ── Configuration ──────────────────────────────────────────────────

/// PMMH configuration.
pub struct PMMHConfig {
    pub n_steps: usize,
    pub n_particles: usize,
    pub dt: f64,
    /// Initial proposal SD on the transformed scale (diagonal: one per estimated param).
    pub proposal_sd: Vec<f64>,
    /// Enable adaptive Metropolis (Haario et al. 2001).
    pub adapt: bool,
    /// Start adapting after this many steps.
    pub adapt_start: usize,
    /// Record every `thin`-th step.
    pub thin: usize,
    /// Discard first `burn_in` steps from output.
    pub burn_in: usize,
    /// Crank-Nicolson correlation for correlated pseudo-marginal.
    /// None = vanilla PMMH (independent PF evaluations).
    /// Some(0.99) = CPM with ρ=0.99 (recommended).
    pub rho: Option<f64>,
    /// Number of source groups in the model (for sizing binomial noise).
    /// Set by the CLI from model.source_groups.len().
    pub n_source_groups: usize,
    /// Number of observations (for sizing PFRandomState).
    pub n_obs: usize,
    /// Substeps per observation interval (= obs_spacing / dt).
    /// Used to size the CPM random state. Computed from actual observation times.
    pub steps_per_obs: usize,
}

// ── Output types ───────────────────────────────────────────────────

/// One recorded MCMC step.
#[derive(Clone, Debug)]
pub struct PMMHStep {
    pub step: usize,
    /// Parameter values on the natural scale (full param vector).
    pub params: Vec<f64>,
    /// PF log-likelihood estimate.
    pub log_likelihood: f64,
    /// Log prior density.
    pub log_prior: f64,
    pub accepted: bool,
}

/// Result of a PMMH run.
pub struct PMMHResult {
    pub steps: Vec<PMMHStep>,
    pub acceptance_rate: f64,
    pub n_steps: usize,
    /// MAP (highest posterior) parameter set.
    pub map_params: Vec<f64>,
    pub map_loglik: f64,
    pub map_log_posterior: f64,
    /// Resume state for chain continuation. Populated at end of every run.
    pub resume_state: PMMHResumeState,
}

/// Serializable chain state for `--resume`. Saved to `chain_N/resume_state.bin`
/// via bincode at end of every PMMH run, enabling continuation without
/// re-doing burn-in or adaptive proposal warm-up.
#[derive(Clone, Serialize, Deserialize)]
pub struct PMMHResumeState {
    /// Config hash — only resume if the statistical problem matches.
    pub config_hash: String,
    /// Number of MCMC steps completed (resume starts from here).
    pub completed_steps: usize,
    /// Current parameter values (natural scale, full model param vector).
    pub params: Vec<f64>,
    /// Current transformed parameters (unconstrained scale).
    pub transformed: Vec<f64>,
    /// Estimated parameter names (for reordering on resume).
    pub param_names: Vec<String>,
    /// Current PF log-likelihood estimate.
    pub current_ll: f64,
    /// Current log prior density.
    pub current_log_prior: f64,
    /// Number of accepted proposals so far.
    pub n_accepted: usize,
    /// Adaptive proposal state (None if adapt=false).
    pub adaptive: Option<AdaptiveProposal>,
    /// CPM random state (None if rho=None).
    pub current_randoms: Option<super::correlated_pf::PFRandomState>,
    /// MAP parameter values.
    pub map_params: Vec<f64>,
    /// MAP log-likelihood.
    pub map_loglik: f64,
    /// MAP log-posterior.
    pub map_log_posterior: f64,
}

// ── Adaptive proposal (Haario et al. 2001) ─────────────────────────

/// Running mean + covariance via Welford's online algorithm,
/// plus Cholesky factor for sampling N(0, Σ).
#[derive(Clone, Serialize, Deserialize)]
pub struct AdaptiveProposal {
    d: usize,
    n: usize,
    mean: Vec<f64>,
    /// Sum of outer products of deviations: M₂[i*d+j]
    m2: Vec<f64>,
    /// Cholesky factor L such that L Lᵀ = scaled covariance.
    /// Row-major, lower-triangular. Updated every `chol_interval` steps.
    chol: Vec<f64>,
    chol_interval: usize,
    steps_since_chol: usize,
    /// Whether Cholesky has been computed at least once.
    chol_valid: bool,
}

impl AdaptiveProposal {
    fn new(d: usize) -> Self {
        AdaptiveProposal {
            d,
            n: 0,
            mean: vec![0.0; d],
            m2: vec![0.0; d * d],
            chol: vec![0.0; d * d],
            chol_interval: 100,
            steps_since_chol: 0,
            chol_valid: false,
        }
    }

    /// Update running statistics with a new sample (on transformed scale).
    fn update(&mut self, x: &[f64]) {
        self.n += 1;
        let n = self.n as f64;
        let d = self.d;

        // Welford: delta = x - old_mean, update mean, delta2 = x - new_mean
        let mut delta = vec![0.0; d];
        for i in 0..d {
            delta[i] = x[i] - self.mean[i];
            self.mean[i] += delta[i] / n;
        }
        for i in 0..d {
            let delta2_i = x[i] - self.mean[i];
            for j in 0..d {
                self.m2[i * d + j] += delta[i] * (x[j] - self.mean[j]);
                // Symmetrize using the Welford identity for cross-terms:
                // We need M₂[i,j] = Σ (x_k - mean)(x_k - mean)ᵀ
                // The update above handles the rank-1 correction correctly
                // for both i,j simultaneously.
                let _ = delta2_i; // used implicitly via the outer product
            }
        }

        self.steps_since_chol += 1;
        if self.steps_since_chol >= self.chol_interval && self.n >= self.d + 1 {
            self.update_cholesky();
        }
    }

    /// Recompute Cholesky of the scaled proposal covariance:
    /// Σ_prop = (2.38² / d) × Cov + ε × I
    fn update_cholesky(&mut self) {
        let d = self.d;
        let n = self.n as f64;
        let scale = 2.38_f64.powi(2) / d as f64;
        let eps = 1e-6;

        // Build scaled covariance + regularization
        let mut a = vec![0.0; d * d];
        for i in 0..d {
            for j in 0..d {
                a[i * d + j] = scale * self.m2[i * d + j] / (n - 1.0);
            }
            a[i * d + i] += eps;
        }

        // Cholesky decomposition (in-place, lower triangular)
        if cholesky_lower(&mut a, d) {
            self.chol.copy_from_slice(&a);
            self.chol_valid = true;
        }
        // If Cholesky fails (shouldn't with eps regularization), keep old factor.

        self.steps_since_chol = 0;
    }

    /// Sample a proposal perturbation: Δθ ~ N(0, Σ_prop).
    /// Returns the perturbation vector (on transformed scale).
    /// Falls back to diagonal if Cholesky isn't ready yet.
    fn sample_perturbation(&self, rng: &mut StatefulRng, fallback_sd: &[f64]) -> Vec<f64> {
        let d = self.d;
        let z: Vec<f64> = (0..d).map(|_| rng.normal()).collect();

        if self.chol_valid {
            // Δ = L × z
            let mut delta = vec![0.0; d];
            for i in 0..d {
                for j in 0..=i {
                    delta[i] += self.chol[i * d + j] * z[j];
                }
            }
            delta
        } else {
            // Diagonal fallback
            z.iter().zip(fallback_sd).map(|(&zi, &sd)| zi * sd).collect()
        }
    }
}

/// In-place Cholesky decomposition of a symmetric positive-definite matrix.
/// Overwrites `a` with the lower-triangular factor L (row-major).
/// Returns false if the matrix is not positive definite.
fn cholesky_lower(a: &mut [f64], d: usize) -> bool {
    for i in 0..d {
        for j in 0..=i {
            let mut sum = a[i * d + j];
            for k in 0..j {
                sum -= a[i * d + k] * a[j * d + k];
            }
            if i == j {
                if sum <= 0.0 { return false; }
                a[i * d + j] = sum.sqrt();
            } else {
                a[i * d + j] = sum / a[j * d + j];
            }
        }
        // Zero out upper triangle
        for j in (i + 1)..d {
            a[i * d + j] = 0.0;
        }
    }
    true
}

// ── Core PMMH algorithm ────────────────────────────────────────────

/// Run PMMH.
///
/// `eval_loglik` runs a particle filter at the given params and returns log L̂(θ).
/// Built in the CLI layer from `run_quick_pfilter`. Takes `(full_params, pf_seed) → log L̂`.
///
/// `on_step` optional progress callback: `(step, loglik, accepted)`.
/// Correlated PF evaluator for CPM-MCMC.
/// Takes (params, randoms) → (loglik, randoms_used).
pub type CorrelatedEvalFn<'a> = dyn Fn(&[f64], &super::correlated_pf::PFRandomState)
    -> f64 + 'a;

pub fn run_pmmh(
    if2_params: &[IF2Param],
    priors: &[Prior],
    base_params: &[f64],
    config: &PMMHConfig,
    eval_loglik: &dyn Fn(&[f64], u64) -> f64,
    eval_loglik_correlated: Option<&CorrelatedEvalFn>,
    seed: u64,
    on_step: Option<&dyn Fn(usize, f64, bool, &[f64])>,
    resume_from: Option<PMMHResumeState>,
    config_hash: String,
) -> PMMHResult {
    let d = if2_params.len();
    assert_eq!(d, priors.len(), "priors must match if2_params length");
    assert_eq!(d, config.proposal_sd.len(), "proposal_sd must match if2_params length");

    use super::correlated_pf::PFRandomState;

    let start_step;
    let mut current_params: Vec<f64>;
    let mut current_transformed: Vec<f64>;
    let mut current_ll: f64;
    let mut current_log_prior: f64;
    let mut current_randoms: Option<PFRandomState>;
    let mut map_log_posterior: f64;
    let mut map_params: Vec<f64>;
    let mut map_loglik: f64;
    let mut adaptive: Option<AdaptiveProposal>;
    let mut n_accepted: usize;
    let mut rng: StatefulRng;

    if let Some(state) = resume_from {
        eprintln!("  resuming from step {}...", state.completed_steps);
        start_step = state.completed_steps;
        current_params = state.params;
        n_accepted = state.n_accepted;
        current_ll = state.current_ll;
        current_log_prior = state.current_log_prior;
        current_randoms = state.current_randoms;
        adaptive = state.adaptive;
        map_params = state.map_params;
        map_loglik = state.map_loglik;
        map_log_posterior = state.map_log_posterior;

        // Restore z-values with name-based reordering (same pattern as PGAS)
        if !state.param_names.is_empty() && state.param_names.len() == state.transformed.len() {
            let saved_z: std::collections::HashMap<&str, f64> = state.param_names.iter()
                .zip(state.transformed.iter())
                .map(|(name, &z)| (name.as_str(), z))
                .collect();
            current_transformed = if2_params.iter().map(|spec| {
                if let Some(&z) = saved_z.get(spec.name.as_str()) {
                    z
                } else {
                    eprintln!("  warning: param '{}' not found in resume state, computing from theta", spec.name);
                    spec.to_transformed(current_params[spec.index])
                }
            }).collect();
        } else {
            eprintln!("  warning: resume state lacks param_names — recomputing z from params.");
            current_transformed = if2_params.iter()
                .map(|spec| spec.to_transformed(current_params[spec.index]))
                .collect();
        }

        // Enforce bounds on restored params
        for (i, spec) in if2_params.iter().enumerate() {
            let clamped = spec.from_transformed(current_transformed[i]);
            current_params[spec.index] = clamped;
        }

        // Derive RNG from seed ^ completed_steps for continuation
        rng = StatefulRng::new(seed ^ start_step as u64);
    } else {
        start_step = 0;
        rng = StatefulRng::new(seed);
        current_params = base_params.to_vec();

        // Current state on transformed scale
        current_transformed = if2_params.iter()
            .map(|p| p.to_transformed(current_params[p.index]))
            .collect();

        // CPM random state (if correlated mode)
        let steps_per_obs = config.steps_per_obs;
        current_randoms = config.rho.map(|_| {
            PFRandomState::draw_fresh(
                config.n_particles, config.n_obs, steps_per_obs,
                config.n_source_groups, &mut rng,
            )
        });

        // Initial PF evaluation
        current_ll = if let (Some(ref randoms), Some(eval_corr)) =
            (&current_randoms, &eval_loglik_correlated)
        {
            eval_corr(&current_params, randoms)
        } else {
            eval_loglik(&current_params, seed.wrapping_add(0))
        };
        current_log_prior = if2_params.iter().zip(priors.iter())
            .zip(current_transformed.iter())
            .map(|((p, prior), &z)| prior.log_density(current_params[p.index], z))
            .sum();

        // Track MAP
        map_log_posterior = current_ll + current_log_prior;
        map_params = current_params.clone();
        map_loglik = current_ll;

        // Adaptive proposal
        adaptive = if config.adapt {
            let mut ap = AdaptiveProposal::new(d);
            ap.update(&current_transformed);
            Some(ap)
        } else {
            None
        };

        n_accepted = 0;
    }

    let mut current_log_jacobian: f64 = if2_params.iter()
        .zip(current_transformed.iter())
        .map(|(p, &z)| p.log_jacobian(z))
        .sum();

    let mut steps = Vec::new();

    if start_step >= config.n_steps {
        eprintln!("  warning: chain already completed {} steps (requested {}). \
                   Increase steps in fit.toml to continue.", start_step, config.n_steps);
    }

    for step in start_step..config.n_steps {
        // Propose: θ' = θ + Δ on transformed scale
        let delta = if let Some(ref ap) = adaptive {
            if config.adapt && step >= config.adapt_start {
                ap.sample_perturbation(&mut rng, &config.proposal_sd)
            } else {
                (0..d).map(|i| rng.normal() * config.proposal_sd[i]).collect()
            }
        } else {
            (0..d).map(|i| rng.normal() * config.proposal_sd[i]).collect::<Vec<f64>>()
        };

        let proposed_transformed: Vec<f64> = current_transformed.iter()
            .zip(delta.iter())
            .map(|(&z, &dz)| z + dz)
            .collect();

        // Back to natural scale
        let mut proposed_params = current_params.clone();
        for (i, spec) in if2_params.iter().enumerate() {
            proposed_params[spec.index] = spec.from_transformed(proposed_transformed[i]);
        }

        // Log prior at proposed params
        let proposed_log_prior: f64 = if2_params.iter().zip(priors.iter())
            .zip(proposed_transformed.iter())
            .map(|((p, prior), &z)| prior.log_density(proposed_params[p.index], z))
            .sum();

        // Evaluate PF: correlated or independent
        let proposed_randoms: Option<PFRandomState>;
        let proposed_ll;
        if let (Some(rho), Some(ref cur_rand), Some(eval_corr)) =
            (config.rho, &current_randoms, &eval_loglik_correlated)
        {
            let pr = cur_rand.correlate(rho, &mut rng);
            proposed_ll = eval_corr(&proposed_params, &pr);
            proposed_randoms = Some(pr);
        } else {
            let pf_seed = seed.wrapping_add(step as u64 + 1);
            proposed_ll = eval_loglik(&proposed_params, pf_seed);
            proposed_randoms = None;
        };

        // Jacobian correction
        let proposed_log_jacobian: f64 = if2_params.iter()
            .zip(proposed_transformed.iter())
            .map(|(p, &z)| p.log_jacobian(z))
            .sum();

        // MH acceptance ratio (log scale)
        let log_alpha = (proposed_ll + proposed_log_prior + proposed_log_jacobian)
                      - (current_ll + current_log_prior + current_log_jacobian);

        let accepted = log_alpha.is_finite() && rng.uniform().ln() < log_alpha;

        if accepted {
            current_params.copy_from_slice(&proposed_params);
            current_transformed.copy_from_slice(&proposed_transformed);
            current_ll = proposed_ll;
            current_log_prior = proposed_log_prior;
            current_log_jacobian = proposed_log_jacobian;
            if proposed_randoms.is_some() {
                current_randoms = proposed_randoms;
            }
            n_accepted += 1;

            let log_posterior = current_ll + current_log_prior;
            if log_posterior > map_log_posterior {
                map_log_posterior = log_posterior;
                map_params.copy_from_slice(&current_params);
                map_loglik = current_ll;
            }
        }

        // Update adaptive proposal with current position (whether accepted or not
        // is debated — we include all steps, matching the original Haario algorithm)
        if let Some(ref mut ap) = adaptive {
            ap.update(&current_transformed);
        }

        // Record step (respecting burn-in and thinning)
        if step >= config.burn_in && (step - config.burn_in) % config.thin == 0 {
            steps.push(PMMHStep {
                step,
                params: current_params.clone(),
                log_likelihood: current_ll,
                log_prior: current_log_prior,
                accepted,
            });
        }

        if let Some(cb) = on_step {
            cb(step, current_ll, accepted, &current_params);
        }
    }

    let total_steps = config.n_steps - start_step;
    let acceptance_rate = if total_steps > 0 {
        n_accepted as f64 / config.n_steps as f64
    } else {
        0.0
    };

    let resume_state = PMMHResumeState {
        config_hash,
        completed_steps: config.n_steps,
        params: current_params.clone(),
        transformed: current_transformed,
        param_names: if2_params.iter().map(|p| p.name.clone()).collect(),
        current_ll,
        current_log_prior,
        n_accepted,
        adaptive,
        current_randoms,
        map_params: map_params.clone(),
        map_loglik,
        map_log_posterior,
    };

    PMMHResult {
        steps,
        acceptance_rate,
        n_steps: config.n_steps,
        map_params,
        map_loglik,
        map_log_posterior,
        resume_state,
    }
}

// ── MCMC diagnostics ───────────────────────────────────────────────

/// Effective sample size from chain autocorrelation (Geyer 1992 initial
/// positive sequence estimator).
pub fn mcmc_ess(chain: &[f64]) -> f64 {
    let n = chain.len();
    if n < 4 { return n as f64; }

    let mean = chain.iter().sum::<f64>() / n as f64;
    let var = chain.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    if var < 1e-30 { return 1.0; }

    let mut sum_rho = 0.0;
    let mut lag = 1;
    while lag < n {
        let rho: f64 = chain.iter().zip(chain.iter().skip(lag))
            .map(|(&a, &b)| (a - mean) * (b - mean))
            .sum::<f64>() / (n as f64 * var);
        // Initial positive sequence: stop when autocorrelation goes negative
        // (Geyer recommends stopping at the first negative *pair* sum, but
        // single-lag cutoff is standard and simpler)
        if lag >= 2 && rho < 0.0 { break; }
        sum_rho += rho;
        lag += 1;
    }
    (n as f64 / (1.0 + 2.0 * sum_rho)).max(1.0)
}
