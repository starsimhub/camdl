//! Core types for the particle filter and inference algorithms.
//!
//! Flat array layout for cache-friendly resampling — copying a particle's
//! state is one contiguous memcpy, not a pointer chase through Vec<Vec<...>>.
//!
//! Also owns the shared inference infrastructure types — `Transform`,
//! `EstimatedParam` — that all algorithms (IF2, PGAS, PMMH) use. These
//! live here rather than in `if2.rs` so that removing or replacing IF2
//! does not take the shared type contract with it.

use super::traits::Resettable;

// ── Parameter transform ───────────────────────────────────────────────────────

/// The unconstrained-space transform applied to an estimated parameter.
///
/// Matches Stan's lower/upper bounded parameter conventions. Used by IF2,
/// PGAS, and PMMH for all scale-management operations (to/from transformed,
/// Jacobian, gradient chain rule).
#[derive(Clone, Debug)]
pub enum Transform {
    /// Log transform with bounds clamping on the inverse.
    /// Correct for rates, positive quantities, counts.
    /// `from_transformed` clamps `z.exp()` to `[lo, hi]` — out-of-bounds
    /// particles get bad log-likelihood and are resampled away.
    /// This is what Stan does for lower-bounded parameters.
    Log { lo: f64, hi: f64 },
    /// Scaled logit mapping `[lo, hi]` to `(−∞, +∞)`.
    /// Correct for probabilities. Bounds enforced by the logistic function
    /// (output always in `(0, 1)`). For narrow bounds like `[0.01, 0.10]`
    /// the logit-scaled position can be extreme (|z| > 2), compressing the
    /// effective perturbation range; the preflight diagnostic warns about this.
    Logit { lo: f64, hi: f64 },
    /// No transform. For unconstrained real parameters.
    None,
}

/// One estimated parameter: its name, position in the full model parameter
/// vector, declared transform and bounds, and per-algorithm adaptation state.
///
/// Shared by IF2, PGAS, and PMMH. Constructed by the CLI from the fit config
/// and passed into algorithm entry points.
#[derive(Clone, Debug)]
pub struct EstimatedParam {
    /// Parameter name (for reporting and gradient lookup).
    pub name: String,
    /// Index into the full model `params` array.
    pub index: usize,
    /// Starting value for IF2 (or the current value on resume).
    pub initial: f64,
    /// Random walk standard deviation on the *transformed* scale.
    /// Shrinks by `cooling_fraction` each IF2 iteration.
    pub rw_sd: f64,
    /// Scale transform applied before perturbation / MH proposals.
    pub transform: Transform,
    /// Natural-scale lower bound (used for display and random-start sampling).
    pub lower: f64,
    /// Natural-scale upper bound (used for display and random-start sampling).
    pub upper: f64,
    /// Whether `rw_sd` was auto-computed from the data (for preflight reporting).
    #[allow(dead_code)]
    pub rw_sd_auto: bool,
    /// If true, perturb only at t=0 (initial-value parameter: S₀, E₀, I₀ …).
    /// Matches pomp's `ivp()` in `rw.sd`.
    pub ivp: bool,
}

impl EstimatedParam {
    /// Map a natural-scale value to the unconstrained (z) scale.
    pub fn to_transformed(&self, x: f64) -> f64 {
        match &self.transform {
            Transform::Log { lo, hi } => x.clamp(*lo, *hi).max(LOG_PROB_FLOOR).ln(),
            Transform::Logit { lo, hi } => {
                let p = ((x - lo) / (hi - lo)).clamp(1e-10, 1.0 - 1e-10);
                (p / (1.0 - p)).ln()
            }
            Transform::None => x,
        }
    }

    /// Map an unconstrained value back to the natural scale.
    pub fn from_transformed(&self, z: f64) -> f64 {
        match &self.transform {
            Transform::Log { lo, hi } => {
                // Clamp to declared bounds — prevents NaN/panic downstream.
                // Out-of-bounds particles get bad loglik and are resampled away.
                z.exp().clamp(*lo, *hi)
            }
            Transform::Logit { lo, hi } => {
                let p = 1.0 / (1.0 + (-z).exp());
                lo + p * (hi - lo)
                // Bounds enforced by construction — no clamp needed.
            }
            Transform::None => z,
        }
    }

    /// log |dθ/dz| for the transform θ = f(z).
    /// Needed for the MH acceptance ratio when proposing on the transformed scale.
    ///
    /// Log-transform:   θ = exp(z)              → log-Jacobian = z
    /// Logit-transform: θ = lo + (hi−lo)·σ(z)   → log-Jacobian = log((hi−lo)·p·(1−p))
    /// No transform:    Jacobian = 1             → log-Jacobian = 0
    pub fn log_jacobian(&self, z: f64) -> f64 {
        match &self.transform {
            Transform::Log { .. } => z,
            Transform::Logit { lo, hi } => {
                let p = 1.0 / (1.0 + (-z).exp());
                ((hi - lo) * p * (1.0 - p)).ln()
            }
            Transform::None => 0.0,
        }
    }

    /// d/dz log|dθ/dz| — derivative of the log-Jacobian w.r.t. z.
    pub fn jacobian_grad(&self, z: f64) -> f64 {
        match &self.transform {
            Transform::Log { .. } => 1.0,
            Transform::Logit { .. } => {
                let p = 1.0 / (1.0 + (-z).exp());
                1.0 - 2.0 * p
            }
            Transform::None => 0.0,
        }
    }

    /// dθ/dz — derivative of the natural-scale value with respect to z.
    /// Used in the chain rule: d(f(θ))/dz = d(f)/dθ × dθ/dz.
    pub fn transform_deriv(&self, z: f64) -> f64 {
        match &self.transform {
            Transform::Log { .. } => z.exp(),
            Transform::Logit { lo, hi } => {
                let p = 1.0 / (1.0 + (-z).exp());
                (hi - lo) * p * (1.0 - p)
            }
            Transform::None => 1.0,
        }
    }

    /// Delta method: convert a natural-scale `rw_sd` to the transformed scale.
    /// Matches pomp's convention: the user specifies `rw.sd` on the natural scale.
    pub fn transformed_sd(&self, natural_sd: f64, current_value: f64) -> f64 {
        match &self.transform {
            Transform::Log { .. } => natural_sd / current_value.max(LOG_PROB_FLOOR),
            Transform::Logit { lo, hi } => {
                let range = hi - lo;
                let p = ((current_value - lo) / range).clamp(1e-10, 1.0 - 1e-10);
                natural_sd / (range * p * (1.0 - p))
            }
            Transform::None => natural_sd,
        }
    }
}

// ── Numeric constants ─────────────────────────────────────────────────────────

/// Minimum argument for `ln()` in log-weight computations.
///
/// Chosen so that `ln(LOG_PROB_FLOOR) ≈ −690`, well above the
/// underflow threshold for any realistic particle count: even at
/// N = 10_000 particles, a weight of 1e-300 contributes roughly −690 to
/// `log_sum_exp`, which rounds to −∞ for that particle but does not
/// corrupt the normaliser.
///
/// Do NOT reduce below `f64::MIN_POSITIVE` (≈ 5×10⁻³²⁴), which would
/// produce −∞ and defeat the purpose.
pub const LOG_PROB_FLOOR: f64 = 1e-300;

/// Reserved stream index for the per-algorithm resampling RNG.
///
/// Per-particle streams use indices `[0, n_particles)`. This constant
/// is set high enough (2^48) to never collide with any realistic particle
/// count, making it safe to pass to `StatefulRng::new_stream` alongside
/// per-particle streams from the same base seed.
pub const RESAMPLE_RNG_STREAM: u64 = 1u64 << 48;

// ── RNG helpers ───────────────────────────────────────────────────────────────

/// Allocate `n` per-particle RNG streams derived from `seed`.
///
/// `stream_offset` separates particles from different callers or iterations:
/// - Particle filter and PGAS pass `0` (particles are differentiated by
///   index alone).
/// - IF2 passes `(iter as u64) << 32` so each iteration's particle streams
///   are disjoint from all other iterations (top 32 bits = iteration index,
///   bottom 32 bits = particle index).
pub fn init_particle_rngs(
    seed: u64,
    n: usize,
    stream_offset: u64,
) -> Vec<crate::rng::StatefulRng> {
    (0..n)
        .map(|i| crate::rng::StatefulRng::new_stream(seed, stream_offset | (i as u64)))
        .collect()
}

/// Restore the unconstrained z-values from a saved resume state, reordering
/// them to match the current `if2_params` ordering.
///
/// The resume state stores `param_names` alongside `transformed` z-values.
/// Because HashMap iteration order is non-deterministic, the current run's
/// `if2_params` may be in a different order than when the state was saved.
/// Parameters missing from the saved state are recomputed from `current_params`
/// with a warning. If `saved_names` is empty (legacy state before param_names
/// was added), all z-values are recomputed from `current_params`.
pub fn restore_z_values(
    saved_names: &[String],
    saved_z: &[f64],
    if2_params: &[EstimatedParam],
    current_params: &[f64],
) -> Vec<f64> {
    if saved_names.is_empty() || saved_names.len() != saved_z.len() {
        eprintln!("  warning: resume state lacks param_names — recomputing z from params.");
        return if2_params.iter()
            .map(|spec| spec.to_transformed(current_params[spec.index]))
            .collect();
    }

    let saved: std::collections::HashMap<&str, f64> = saved_names.iter()
        .zip(saved_z.iter())
        .map(|(name, &z)| (name.as_str(), z))
        .collect();

    if2_params.iter().map(|spec| {
        if let Some(&z) = saved.get(spec.name.as_str()) {
            z
        } else {
            eprintln!("  warning: param '{}' not found in resume state, computing from theta", spec.name);
            spec.to_transformed(current_params[spec.index])
        }
    }).collect()
}

/// State of one particle: compartment counts + flow accumulators.
#[derive(Clone, Debug)]
pub struct ParticleState {
    /// Integer compartment values (local int indices, same layout as IntState).
    pub counts: Vec<i64>,
    /// Cumulative transition flows since last observation.
    /// Reset after each observation time (used for incidence projections).
    pub flow_accumulators: Vec<u64>,
}

impl ParticleState {
    pub fn new(n_compartments: usize, n_transitions: usize) -> Self {
        ParticleState {
            counts: vec![0; n_compartments],
            flow_accumulators: vec![0; n_transitions],
        }
    }

    /// Reset flow accumulators to zero (called after each observation).
    pub fn reset_flows(&mut self) {
        self.reset_accumulators();
    }

}

impl Resettable for ParticleState {
    fn reset_accumulators(&mut self) {
        for f in &mut self.flow_accumulators { *f = 0; }
    }
}

impl ParticleState {
    /// Clamp negative compartment values to zero.
    pub fn clamp_nonneg(&mut self) {
        for c in &mut self.counts {
            if *c < 0 { *c = 0; }
        }
    }
}

/// Storage for N particles with log-weights.
pub struct ParticleSwarm {
    pub n_particles: usize,
    pub states: Vec<ParticleState>,
    pub log_weights: Vec<f64>,
}

impl ParticleSwarm {
    pub fn new(n_particles: usize, n_compartments: usize, n_transitions: usize) -> Self {
        ParticleSwarm {
            n_particles,
            states: (0..n_particles)
                .map(|_| ParticleState::new(n_compartments, n_transitions))
                .collect(),
            log_weights: vec![0.0; n_particles],
        }
    }

    /// Effective sample size: ESS = 1 / Σ(w_normalized²).
    /// Returns N when all weights are equal, 1 when one particle dominates.
    pub fn ess(&self) -> f64 {
        let max_lw = self.log_weights.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if max_lw.is_infinite() { return 0.0; }
        let sum_w: f64 = self.log_weights.iter().map(|&lw| (lw - max_lw).exp()).sum();
        let sum_w2: f64 = self.log_weights.iter().map(|&lw| (2.0 * (lw - max_lw)).exp()).sum();
        if sum_w2 == 0.0 { return 0.0; }
        (sum_w * sum_w) / sum_w2
    }
}

/// Numerically stable log-sum-exp.
///
/// Im2 in the 2026-04-19 inference review batch 1: distinguish
/// +∞ vs −∞. If `max = +∞`, at least one entry is +∞ and the
/// result is also +∞ (not −∞ as the old bulk-check produced).
/// If `max = −∞`, every entry is −∞ and the result is −∞.
pub fn log_sum_exp(log_values: &[f64]) -> f64 {
    let max = log_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if max == f64::NEG_INFINITY { return f64::NEG_INFINITY; }
    if max == f64::INFINITY { return f64::INFINITY; }
    max + log_values.iter().map(|&lv| (lv - max).exp()).sum::<f64>().ln()
}
