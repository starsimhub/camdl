//! Inference module: particle filter, IF2, PMMH.
//!
//! All inference algorithms use the existing simulation backends as black-box
//! `ProcessSimulator` implementations. The observation model provides the
//! `dmeasure` function (log p(y | state, θ)).
//!
//! Architecture:
//!   ProcessSimulator  — advance state one dt (chain-binomial step)
//!   ObservationLikelihood — log p(y | projected, θ)
//!   ParticleFilter    — bootstrap filter using the above two
//!   IF2               — iterated filtering (MLE via perturbed PF)
//!   PMMH              — Bayesian posterior via MCMC with PF likelihood

pub mod obs_loglik;
pub mod resampling;
pub mod particle_filter;
pub mod if2;
pub mod types;
pub mod obs_model;
pub mod pmmh;
pub mod correlated_pf;
pub mod pgas;
pub mod pgas_grad;
pub mod nuts;

// Re-exports
pub use types::{ParticleState, ParticleSwarm, ObsStreamSpec, joint_obs_weight, joint_obs_weight_particle};
pub use obs_loglik::{negbin_logpmf, normal_logpdf, discretized_normal_logpmf, normal_cdf};
pub use particle_filter::bootstrap_filter;

/// Required for all inference. Every model can do this.
/// The "plug-and-play" property: simulate forward, don't need densities.
pub trait ProcessSimulator {
    /// Advance state from t to t+dt. Mutates state in place.
    /// Returns flows accumulated during this step.
    fn step(
        &self,
        state: &mut ParticleState,
        params: &[f64],
        t: f64,
        dt: f64,
        rng: &mut crate::rng::StatefulRng,
    ) -> Result<(), crate::error::SimError>;
}

/// Optional. Only for models with analytically tractable transition densities.
/// Placeholder for future methods (PGAS, exact marginal MH).
pub trait TransitionDensity {
    fn log_transition_density(
        &self,
        state_from: &ParticleState,
        state_to: &ParticleState,
        params: &[f64],
        t: f64,
        dt: f64,
    ) -> f64;
}
