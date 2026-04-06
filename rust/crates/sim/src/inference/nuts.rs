//! No-U-Turn Sampler (NUTS) — Hoffman & Gelman (2014).
//!
//! Efficient HMC variant that automatically selects the number of leapfrog
//! steps via a tree-doubling procedure with a U-turn criterion. No manual
//! tuning of trajectory length.
//!
//! Used in PGAS as the θ|X update step, replacing one-at-a-time MH.
//! The target density is the complete-data log-posterior:
//!   log π(θ | X, y) = complete_data_loglik(θ, X, y) + log_prior(θ)

use crate::rng::StatefulRng;

/// Configuration for the NUTS sampler.
pub struct NUTSConfig {
    /// Maximum tree depth (number of doublings). Default 10 → up to 1024 leapfrog steps.
    pub max_tree_depth: usize,
    /// Step size for leapfrog integration. Adapted during warmup.
    pub step_size: f64,
    /// Diagonal mass matrix (inverse). One entry per parameter.
    /// M_inv[i] scales the momentum for parameter i.
    pub mass_matrix_inv: Vec<f64>,
}

/// Result of one NUTS step.
pub struct NUTSStepResult {
    /// Proposed parameter values (on transformed scale).
    pub params: Vec<f64>,
    /// Log-posterior at the proposed point.
    pub log_posterior: f64,
    /// Whether the proposal was accepted (MH correction).
    pub accepted: bool,
    /// Number of leapfrog steps taken.
    pub n_leapfrog: usize,
    /// Tree depth reached.
    pub tree_depth: usize,
    /// Whether a divergence was detected (numerical instability).
    pub divergent: bool,
}

/// One NUTS step: propose all parameters jointly using gradients.
///
/// `log_prob_and_grad` evaluates the target log-density AND its gradient
/// at a given parameter vector. Returns (log_p, gradient).
///
/// Parameters are on the TRANSFORMED (unconstrained) scale.
pub fn nuts_step(
    current_z: &[f64],
    current_log_p: f64,
    current_grad: &[f64],
    config: &NUTSConfig,
    log_prob_and_grad: &dyn Fn(&[f64]) -> (f64, Vec<f64>),
    rng: &mut StatefulRng,
) -> NUTSStepResult {
    let d = current_z.len();
    let eps = config.step_size;
    let max_depth = config.max_tree_depth;

    // Draw momentum: p ~ N(0, M), where M = diag(1/M_inv)
    let momentum: Vec<f64> = (0..d)
        .map(|i| rng.normal() / config.mass_matrix_inv[i].sqrt())
        .collect();

    // Initial Hamiltonian: H = -log_p + 0.5 * p^T M^{-1} p
    let kinetic = |p: &[f64]| -> f64 {
        p.iter().zip(&config.mass_matrix_inv)
            .map(|(&pi, &mi)| pi * pi * mi)
            .sum::<f64>() * 0.5
    };
    let h0 = -current_log_p + kinetic(&momentum);

    // Slice variable: log(u) ~ Uniform(0, exp(-H0))
    // Equivalent: log_u = -H0 - Exp(1)
    let log_slice = -h0 - rng.exp(1.0);

    // Initialize tree
    let mut z_minus = current_z.to_vec();
    let mut z_plus = current_z.to_vec();
    let mut p_minus = momentum.clone();
    let mut p_plus = momentum.clone();
    let mut grad_minus = current_grad.to_vec();
    let mut grad_plus = current_grad.to_vec();

    let mut z_proposal = current_z.to_vec();
    let mut log_p_proposal = current_log_p;
    let mut n_valid = 1usize; // number of valid states in the tree
    let mut n_leapfrog = 0usize;
    let mut tree_depth = 0usize;
    let mut divergent = false;
    let mut stop = false;

    // Divergence threshold: reject if energy error > 1000
    let delta_max = 1000.0;

    for depth in 0..max_depth {
        // Choose direction: forward or backward
        let direction: f64 = if rng.uniform() < 0.5 { 1.0 } else { -1.0 };

        // Build tree of depth `depth` in the chosen direction
        let (z_new, p_new, grad_new, z_prime, log_p_prime,
             n_prime, stop_prime, div_prime, n_lf) = if direction > 0.0 {
            build_tree(
                &z_plus, &p_plus, &grad_plus, direction, depth, eps,
                &config.mass_matrix_inv, log_slice, h0, delta_max,
                log_prob_and_grad,
            )
        } else {
            build_tree(
                &z_minus, &p_minus, &grad_minus, direction, depth, eps,
                &config.mass_matrix_inv, log_slice, h0, delta_max,
                log_prob_and_grad,
            )
        };

        n_leapfrog += n_lf;

        if !stop_prime && n_prime > 0 {
            // Metropolis: accept the subtree's proposal with probability n_prime / n_valid
            let accept_prob = n_prime as f64 / (n_valid + n_prime) as f64;
            if rng.uniform() < accept_prob {
                z_proposal = z_prime;
                log_p_proposal = log_p_prime;
            }
        }

        n_valid += n_prime;
        divergent = divergent || div_prime;

        // Update tree endpoints
        if direction > 0.0 {
            z_plus = z_new;
            p_plus = p_new;
            grad_plus = grad_new;
        } else {
            z_minus = z_new;
            p_minus = p_new;
            grad_minus = grad_new;
        }

        // U-turn check on the full tree
        stop = stop_prime || uturn(&z_minus, &z_plus, &p_minus, &p_plus,
                                    &config.mass_matrix_inv);
        tree_depth = depth + 1;
        if stop { break; }
    }

    let accepted = z_proposal != current_z;

    NUTSStepResult {
        params: z_proposal,
        log_posterior: log_p_proposal,
        accepted,
        n_leapfrog,
        tree_depth,
        divergent,
    }
}

/// Leapfrog integrator: one step of Störmer-Verlet.
fn leapfrog(
    z: &[f64], p: &[f64], grad: &[f64],
    eps: f64, direction: f64, m_inv: &[f64],
    log_prob_and_grad: &dyn Fn(&[f64]) -> (f64, Vec<f64>),
) -> (Vec<f64>, Vec<f64>, f64, Vec<f64>) {
    let d = z.len();
    let dt = eps * direction;

    // Half-step momentum
    let mut p_half: Vec<f64> = (0..d).map(|i| p[i] + 0.5 * dt * grad[i]).collect();

    // Full-step position
    let z_new: Vec<f64> = (0..d).map(|i| z[i] + dt * m_inv[i] * p_half[i]).collect();

    // Evaluate gradient at new position
    let (log_p_new, grad_new) = log_prob_and_grad(&z_new);

    // Half-step momentum
    for i in 0..d {
        p_half[i] += 0.5 * dt * grad_new[i];
    }

    (z_new, p_half, log_p_new, grad_new)
}

/// Recursively build a balanced binary tree of leapfrog states.
/// Returns: (z_end, p_end, grad_end, z_proposal, log_p_proposal, n_valid, stop, divergent, n_leapfrog)
#[allow(clippy::too_many_arguments)]
fn build_tree(
    z: &[f64], p: &[f64], grad: &[f64],
    direction: f64, depth: usize, eps: f64,
    m_inv: &[f64], log_slice: f64, h0: f64, delta_max: f64,
    log_prob_and_grad: &dyn Fn(&[f64]) -> (f64, Vec<f64>),
) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, f64, usize, bool, bool, usize) {
    if depth == 0 {
        // Base case: one leapfrog step
        let (z_new, p_new, log_p_new, grad_new) =
            leapfrog(z, p, grad, eps, direction, m_inv, log_prob_and_grad);

        let kinetic: f64 = p_new.iter().zip(m_inv)
            .map(|(&pi, &mi)| pi * pi * mi).sum::<f64>() * 0.5;
        let h_new = -log_p_new + kinetic;

        // Valid if within slice
        let n_valid = if log_slice <= -h_new { 1 } else { 0 };
        // Divergent if energy error too large
        let divergent = (h_new - h0).abs() > delta_max;
        let stop = divergent;

        return (z_new.clone(), p_new, grad_new, z_new, log_p_new,
                n_valid, stop, divergent, 1);
    }

    // Recursive case: build left subtree, then right subtree
    let (z_inner, p_inner, grad_inner, z_prime, log_p_prime,
         n_prime, stop_prime, div_prime, n_lf1) =
        build_tree(z, p, grad, direction, depth - 1, eps, m_inv,
                   log_slice, h0, delta_max, log_prob_and_grad);

    if stop_prime {
        return (z_inner, p_inner, grad_inner, z_prime, log_p_prime,
                n_prime, true, div_prime, n_lf1);
    }

    let (z_outer, p_outer, grad_outer, z_dprime, log_p_dprime,
         n_dprime, stop_dprime, div_dprime, n_lf2) =
        build_tree(&z_inner, &p_inner, &grad_inner, direction, depth - 1, eps, m_inv,
                   log_slice, h0, delta_max, log_prob_and_grad);

    // Choose between subtree proposals
    let (z_proposal, log_p_proposal) = if n_dprime > 0 && n_prime + n_dprime > 0 {
        let accept = n_dprime as f64 / (n_prime + n_dprime) as f64;
        // Use a deterministic choice based on counts (no RNG in recursive tree)
        if accept > 0.5 { (z_dprime, log_p_dprime) } else { (z_prime, log_p_prime) }
    } else {
        (z_prime, log_p_prime)
    };

    let n_valid = n_prime + n_dprime;
    let divergent = div_prime || div_dprime;

    // U-turn check across the full subtree
    let z_minus = if direction > 0.0 { z.to_vec() } else { z_outer.clone() };
    let z_plus = if direction > 0.0 { z_outer.clone() } else { z.to_vec() };
    let p_minus = if direction > 0.0 { p.to_vec() } else { p_outer.clone() };
    let p_plus = if direction > 0.0 { p_outer.clone() } else { p.to_vec() };
    let stop = stop_dprime || uturn(&z_minus, &z_plus, &p_minus, &p_plus, m_inv);

    (z_outer, p_outer, grad_outer, z_proposal, log_p_proposal,
     n_valid, stop, divergent, n_lf1 + n_lf2)
}

/// U-turn criterion: stop if the trajectory is turning back on itself.
/// Check: (z_plus - z_minus) · M^{-1} p_minus >= 0 AND
///        (z_plus - z_minus) · M^{-1} p_plus  >= 0
fn uturn(z_minus: &[f64], z_plus: &[f64], p_minus: &[f64], p_plus: &[f64],
         m_inv: &[f64]) -> bool {
    let d = z_minus.len();
    let mut dot_minus = 0.0;
    let mut dot_plus = 0.0;
    for i in 0..d {
        let dz = z_plus[i] - z_minus[i];
        dot_minus += dz * m_inv[i] * p_minus[i];
        dot_plus += dz * m_inv[i] * p_plus[i];
    }
    dot_minus < 0.0 || dot_plus < 0.0
}

/// Dual averaging for step size adaptation (Nesterov 2009).
/// Targets a specific acceptance rate (typically 0.80 for NUTS).
pub struct DualAveraging {
    target_accept: f64,
    gamma: f64,       // shrinkage toward log_eps_bar
    t0: f64,          // stabilization offset
    kappa: f64,       // decay rate
    mu: f64,          // log(10 * initial_eps) — shrinkage target
    log_eps_bar: f64,  // smoothed log step size
    h_bar: f64,       // smoothed acceptance statistic
    count: usize,
}

impl DualAveraging {
    pub fn new(initial_eps: f64, target_accept: f64) -> Self {
        DualAveraging {
            target_accept,
            gamma: 0.05,
            t0: 10.0,
            kappa: 0.75,
            mu: (10.0 * initial_eps).ln(),
            log_eps_bar: 0.0,
            h_bar: 0.0,
            count: 0,
        }
    }

    /// Update with the acceptance probability from one NUTS step.
    /// Returns the adapted step size for the next step.
    pub fn update(&mut self, accept_prob: f64) -> f64 {
        self.count += 1;
        let m = self.count as f64;
        let w = 1.0 / (m + self.t0);

        self.h_bar = (1.0 - w) * self.h_bar + w * (self.target_accept - accept_prob);

        let log_eps = self.mu - self.h_bar * m.sqrt() / self.gamma;
        let eta = m.powf(-self.kappa);
        self.log_eps_bar = (1.0 - eta) * self.log_eps_bar + eta * log_eps;

        log_eps.exp()
    }

    /// Final adapted step size (smoothed, for post-warmup use).
    pub fn final_step_size(&self) -> f64 {
        self.log_eps_bar.exp()
    }
}
