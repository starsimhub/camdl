//! Compile observation model likelihoods from the IR into dmeasure closures.
//!
//! Evaluates the Expr fields in the IR's Likelihood using the expression
//! evaluator with `projected` set to the projected observation value.

use std::sync::Arc;
use crate::compiled_model::CompiledModel;
use crate::rng::StatefulRng;
use crate::propensity::{eval_expr, EvalCtx};
use crate::state::{IntState, RealState};
use crate::inference::obs_loglik::{negbin_logpmf, discretized_normal_logpmf_tol, poisson_logpmf, DEFAULT_TOL};
use ir::observation::{Likelihood, ObservationModel};
use rand::prelude::Distribution;
use rand_distr::{Gamma, Normal};

/// Build a dmeasure closure for IF2 (per-particle params).
/// Takes (projected, observed, params) → log-likelihood.
pub fn compile_dmeasure_if2(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
) -> Box<dyn Fn(f64, f64, &[f64]) -> f64 + Send + Sync> {
    let likelihood = obs_model.likelihood.clone();
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64, observed: f64, params: &[f64]| {
        eval_likelihood(&likelihood, projected, observed, params, &compiled, &int_s, &real_s)
    })
}

/// Build a dmeasure closure for pfilter (fixed params).
/// Takes (projected, observed) → log-likelihood.
pub fn compile_dmeasure_pf(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
    params: &[f64],
) -> Box<dyn Fn(f64, f64) -> f64> {
    let likelihood = obs_model.likelihood.clone();
    let params = params.to_vec();
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64, observed: f64| {
        eval_likelihood(&likelihood, projected, observed, &params, &compiled, &int_s, &real_s)
    })
}

/// Evaluate a likelihood at (projected, observed, params).
fn eval_likelihood(
    likelihood: &Likelihood,
    projected: f64,
    observed: f64,
    params: &[f64],
    compiled: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
) -> f64 {
    let ctx = |proj: f64| EvalCtx {
        model: compiled, int_s, real_s, params, t: 0.0, projected: Some(proj),
    };

    match likelihood {
        Likelihood::NegBinomial(nb) => {
            let mean = eval_expr(&nb.mean, &ctx(projected)).unwrap_or(projected);
            let k = eval_expr(&nb.dispersion, &ctx(projected)).unwrap_or(10.0);
            negbin_logpmf(observed, mean, k)
        }
        Likelihood::Normal(n) => {
            let mean = eval_expr(&n.mean, &ctx(projected)).unwrap_or(projected);
            let sd = eval_expr(&n.sd, &ctx(projected)).unwrap_or(1.0);
            // Discretized normal for count data
            discretized_normal_logpmf_tol(observed, mean, sd * sd, DEFAULT_TOL)
        }
        Likelihood::Poisson(p) => {
            let rate = eval_expr(&p.rate, &ctx(projected)).unwrap_or(projected);
            poisson_logpmf(observed, rate)
        }
        Likelihood::Binomial(b) => {
            let n_val = eval_expr(&b.n, &ctx(projected)).unwrap_or(projected);
            let p_val = eval_expr(&b.p, &ctx(projected)).unwrap_or(0.5);
            // Simple binomial logpmf
            let k = observed.round() as i64;
            let n = n_val.round() as i64;
            if k < 0 || k > n { return f64::NEG_INFINITY; }
            use crate::inference::obs_loglik::lgamma;
            lgamma((n + 1) as f64) - lgamma((k + 1) as f64) - lgamma((n - k + 1) as f64)
                + k as f64 * p_val.ln() + (n - k) as f64 * (1.0 - p_val).ln()
        }
        Likelihood::BetaBinomial(_) => {
            panic!("BetaBinomial dmeasure not yet implemented. Use neg_binomial or normal.");
        }
        Likelihood::Bernoulli(b) => {
            let p_val = eval_expr(&b.p, &ctx(projected)).unwrap_or(0.5);
            if observed > 0.5 { p_val.max(1e-300).ln() } else { (1.0 - p_val).max(1e-300).ln() }
        }
    }
}

// ── rmeasure: observation model sampler ─────────────────────────────────────

/// Build an rmeasure closure for pfilter (fixed params).
/// Takes (projected, rng) → observation draw.
pub fn compile_rmeasure_pf(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
    params: &[f64],
) -> Box<dyn Fn(f64, &mut StatefulRng) -> f64> {
    let likelihood = obs_model.likelihood.clone();
    let params = params.to_vec();
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64, rng: &mut StatefulRng| {
        sample_obs(&likelihood, projected, &params, &compiled, &int_s, &real_s, rng)
    })
}

/// Build a function that computes the observation model MEAN (no sampling).
/// Takes (projected) → E[y | projected, params].
/// Used for obs_mean in prediction diagnostics.
pub fn compile_obs_mean_pf(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
    params: &[f64],
) -> Box<dyn Fn(f64) -> f64> {
    let likelihood = obs_model.likelihood.clone();
    let params = params.to_vec();
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64| {
        eval_obs_mean(&likelihood, projected, &params, &compiled, &int_s, &real_s)
    })
}

/// Draw one sample from the observation model at a given projected value.
fn sample_obs(
    likelihood: &Likelihood,
    projected: f64,
    params: &[f64],
    compiled: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
    rng: &mut StatefulRng,
) -> f64 {
    let ctx = |proj: f64| EvalCtx {
        model: compiled, int_s, real_s, params, t: 0.0, projected: Some(proj),
    };

    match likelihood {
        Likelihood::NegBinomial(nb) => {
            let mean = eval_expr(&nb.mean, &ctx(projected)).unwrap_or(projected);
            let k = eval_expr(&nb.dispersion, &ctx(projected)).unwrap_or(10.0);
            if mean <= 0.0 || k <= 0.0 { return 0.0; }
            // NegBin via Gamma-Poisson: G ~ Gamma(k, mean/k), Y ~ Poisson(G)
            let g = Gamma::new(k, mean / k).unwrap().sample(rng.inner_mut());
            rng.poisson(g) as f64
        }
        Likelihood::Normal(n) => {
            let mean = eval_expr(&n.mean, &ctx(projected)).unwrap_or(projected);
            let sd = eval_expr(&n.sd, &ctx(projected)).unwrap_or(1.0);
            let draw = Normal::new(mean, sd.max(1e-10)).unwrap().sample(rng.inner_mut());
            draw.round().max(0.0) // discretized, non-negative
        }
        Likelihood::Poisson(p) => {
            let rate = eval_expr(&p.rate, &ctx(projected)).unwrap_or(projected);
            rng.poisson(rate) as f64
        }
        Likelihood::Binomial(b) => {
            let n_val = eval_expr(&b.n, &ctx(projected)).unwrap_or(projected);
            let p_val = eval_expr(&b.p, &ctx(projected)).unwrap_or(0.5);
            rng.binomial(n_val.round().max(0.0) as u64, p_val.clamp(0.0, 1.0)) as f64
        }
        Likelihood::BetaBinomial(_) => {
            panic!("BetaBinomial rmeasure not yet implemented.");
        }
        Likelihood::Bernoulli(b) => {
            let p_val = eval_expr(&b.p, &ctx(projected)).unwrap_or(0.5);
            if rng.uniform() < p_val { 1.0 } else { 0.0 }
        }
    }
}

/// Compute E[y | projected, params] — the observation model mean, no sampling.
fn eval_obs_mean(
    likelihood: &Likelihood,
    projected: f64,
    params: &[f64],
    compiled: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
) -> f64 {
    let ctx = |proj: f64| EvalCtx {
        model: compiled, int_s, real_s, params, t: 0.0, projected: Some(proj),
    };

    match likelihood {
        Likelihood::NegBinomial(nb) => {
            eval_expr(&nb.mean, &ctx(projected)).unwrap_or(projected)
        }
        Likelihood::Normal(n) => {
            eval_expr(&n.mean, &ctx(projected)).unwrap_or(projected)
        }
        Likelihood::Poisson(p) => {
            eval_expr(&p.rate, &ctx(projected)).unwrap_or(projected)
        }
        Likelihood::Binomial(b) => {
            let n_val = eval_expr(&b.n, &ctx(projected)).unwrap_or(projected);
            let p_val = eval_expr(&b.p, &ctx(projected)).unwrap_or(0.5);
            n_val * p_val
        }
        Likelihood::BetaBinomial(_) => {
            panic!("BetaBinomial obs_mean not yet implemented.");
        }
        Likelihood::Bernoulli(b) => {
            eval_expr(&b.p, &ctx(projected)).unwrap_or(0.5)
        }
    }
}
