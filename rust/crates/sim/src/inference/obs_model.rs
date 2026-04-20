//! Compile observation model likelihoods from the IR into dmeasure closures.
//!
//! Evaluates the Expr fields in the IR's Likelihood using the expression
//! evaluator with `projected` set to the projected observation value.

use std::sync::Arc;
use crate::compiled_model::CompiledModel;
use crate::rng::StatefulRng;
use crate::propensity::EvalCtx;
use crate::resolved_expr::{ResolvedLikelihood, ResolveCtx, resolve_likelihood, eval_resolved};
use crate::state::{IntState, RealState};
use crate::inference::obs_loglik::{negbin_logpmf, discretized_normal_logpmf_tol, poisson_logpmf, DEFAULT_TOL};
use ir::observation::ObservationModel;
use rand::prelude::Distribution;
use rand_distr::{Gamma, Normal};

/// Build a dmeasure closure for IF2 (per-particle params).
/// Takes (projected, observed, params) → log-likelihood.
pub fn compile_obs_loglik_if2(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
) -> Box<dyn Fn(f64, f64, &[f64]) -> f64 + Send + Sync> {
    let resolved = resolve_likelihood_from_model(&obs_model.likelihood, &compiled);
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64, observed: f64, params: &[f64]| {
        eval_likelihood_resolved(&resolved, projected, observed, params, &compiled, &int_s, &real_s)
    })
}

/// Build a dmeasure closure for pfilter (fixed params).
/// Takes (projected, observed) → log-likelihood.
pub fn compile_obs_loglik_pf(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
    params: &[f64],
) -> Box<dyn Fn(f64, f64) -> f64 + Send + Sync> {
    let resolved = resolve_likelihood_from_model(&obs_model.likelihood, &compiled);
    let params = params.to_vec();
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64, observed: f64| {
        eval_likelihood_resolved(&resolved, projected, observed, &params, &compiled, &int_s, &real_s)
    })
}

/// Resolve a Likelihood using the compiled model's index maps.
pub(crate) fn resolve_likelihood_from_model(
    likelihood: &ir::observation::Likelihood,
    compiled: &CompiledModel,
) -> ResolvedLikelihood {
    use ir::table::OobPolicy;
    let table_meta: Vec<(OobPolicy, usize)> = compiled.model.tables.iter()
        .zip(&compiled.table_values_cache)
        .map(|(t, cached)| (t.out_of_bounds.clone(), cached.len()))
        .collect();
    let ctx = ResolveCtx {
        comp_index: &compiled.comp_index,
        param_index: &compiled.param_index,
        time_func_index: &compiled.time_func_index,
        table_index: &compiled.table_index,
        global_to_int: &compiled.global_to_int,
        global_to_real: &compiled.global_to_real,
        table_meta: &table_meta,
    };
    resolve_likelihood(likelihood, &ctx)
        .expect("observation likelihood resolution failed — this is a model construction bug")
}

/// Evaluate a resolved likelihood at (projected, observed, params).
pub(crate) fn eval_likelihood_resolved(
    likelihood: &ResolvedLikelihood,
    projected: f64,
    observed: f64,
    params: &[f64],
    compiled: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
) -> f64 {
    let ctx = |proj: f64| EvalCtx {
        model: compiled, int_s, real_s, params, t: 0.0, projected: Some(proj), int_float_override: None,
    };

    match likelihood {
        ResolvedLikelihood::NegBinomial { mean, dispersion } => {
            let m = eval_resolved(mean, &ctx(projected));
            let k = eval_resolved(dispersion, &ctx(projected));
            negbin_logpmf(observed, m, k)
        }
        ResolvedLikelihood::Normal { mean, sd } => {
            let m = eval_resolved(mean, &ctx(projected));
            let s = eval_resolved(sd, &ctx(projected));
            discretized_normal_logpmf_tol(observed, m, s * s, DEFAULT_TOL)
        }
        ResolvedLikelihood::Poisson { rate } => {
            let r = eval_resolved(rate, &ctx(projected));
            poisson_logpmf(observed, r)
        }
        ResolvedLikelihood::Binomial { n, p } => {
            let n_val = eval_resolved(n, &ctx(projected));
            let p_val = eval_resolved(p, &ctx(projected));
            let k = observed.round().max(0.0) as u64;
            let n_int = n_val.round().max(0.0) as u64;
            crate::inference::obs_loglik::binom_logpmf(k, n_int, p_val)
        }
        ResolvedLikelihood::BetaBinomial { n, alpha, beta } => {
            let n_val = eval_resolved(n, &ctx(projected));
            let alpha_val = eval_resolved(alpha, &ctx(projected));
            let beta_val = eval_resolved(beta, &ctx(projected));
            let k = observed.round().max(0.0) as u64;
            let n_int = n_val.round().max(0.0) as u64;
            crate::inference::obs_loglik::beta_binomial_logpmf(k, n_int, alpha_val, beta_val)
        }
        ResolvedLikelihood::Bernoulli { p } => {
            let p_val = eval_resolved(p, &ctx(projected));
            if observed > 0.5 { p_val.max(1e-300).ln() } else { (1.0 - p_val).max(1e-300).ln() }
        }
    }
}

// ── rmeasure: observation model sampler ─────────────────────────────────────

/// Build an rmeasure closure for pfilter (fixed params).
/// Takes (projected, rng) → observation draw.
pub fn compile_obs_sample_pf(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
    params: &[f64],
) -> Box<dyn Fn(f64, &mut crate::rng::StatefulRng) -> f64> {
    let resolved = resolve_likelihood_from_model(&obs_model.likelihood, &compiled);
    let params = params.to_vec();
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64, rng: &mut StatefulRng| {
        sample_obs_resolved(&resolved, projected, &params, &compiled, &int_s, &real_s, rng)
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
    let resolved = resolve_likelihood_from_model(&obs_model.likelihood, &compiled);
    let params = params.to_vec();
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    Box::new(move |projected: f64| {
        eval_obs_mean_resolved(&resolved, projected, &params, &compiled, &int_s, &real_s)
    })
}

/// Draw one sample from the resolved observation model.
pub(crate) fn sample_obs_resolved(
    likelihood: &ResolvedLikelihood,
    projected: f64,
    params: &[f64],
    compiled: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
    rng: &mut StatefulRng,
) -> f64 {
    let ctx = |proj: f64| EvalCtx {
        model: compiled, int_s, real_s, params, t: 0.0, projected: Some(proj), int_float_override: None,
    };

    match likelihood {
        ResolvedLikelihood::NegBinomial { mean, dispersion } => {
            let m = eval_resolved(mean, &ctx(projected));
            let k = eval_resolved(dispersion, &ctx(projected));
            if m <= 0.0 || k <= 0.0 { return 0.0; }
            let g = Gamma::new(k, m / k).unwrap().sample(rng.inner_mut());
            rng.poisson(g) as f64
        }
        ResolvedLikelihood::Normal { mean, sd } => {
            let m = eval_resolved(mean, &ctx(projected));
            let s = eval_resolved(sd, &ctx(projected));
            let draw = Normal::new(m, s.max(1e-10)).unwrap().sample(rng.inner_mut());
            draw.round().max(0.0)
        }
        ResolvedLikelihood::Poisson { rate } => {
            let r = eval_resolved(rate, &ctx(projected));
            rng.poisson(r) as f64
        }
        ResolvedLikelihood::Binomial { n, p } => {
            let n_val = eval_resolved(n, &ctx(projected));
            let p_val = eval_resolved(p, &ctx(projected));
            rng.binomial(n_val.round().max(0.0) as u64, p_val.clamp(0.0, 1.0)) as f64
        }
        ResolvedLikelihood::BetaBinomial { n, alpha, beta } => {
            // Draw BetaBinomial(n, alpha, beta): p ~ Beta(alpha, beta),
            // then k ~ Binomial(n, p). Uses the inner RNG directly for
            // the Beta draw (Gamma(a,1)/(Gamma(a,1)+Gamma(b,1))).
            let n_val = eval_resolved(n, &ctx(projected));
            let alpha_val = eval_resolved(alpha, &ctx(projected)).max(1e-300);
            let beta_val  = eval_resolved(beta,  &ctx(projected)).max(1e-300);
            let n_int = n_val.round().max(0.0) as u64;
            use rand_distr::{Gamma, Distribution};
            let inner = rng.inner_mut();
            let a = Gamma::new(alpha_val, 1.0).map(|d| d.sample(inner)).unwrap_or(1.0);
            let b = Gamma::new(beta_val,  1.0).map(|d| d.sample(inner)).unwrap_or(1.0);
            let p = a / (a + b);
            rng.binomial(n_int, p.clamp(0.0, 1.0)) as f64
        }
        ResolvedLikelihood::Bernoulli { p } => {
            let p_val = eval_resolved(p, &ctx(projected));
            if rng.uniform() < p_val { 1.0 } else { 0.0 }
        }
    }
}

/// Compute E[y | projected, params] — the observation model mean, no sampling.
pub(crate) fn eval_obs_mean_resolved(
    likelihood: &ResolvedLikelihood,
    projected: f64,
    params: &[f64],
    compiled: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
) -> f64 {
    let ctx = |proj: f64| EvalCtx {
        model: compiled, int_s, real_s, params, t: 0.0, projected: Some(proj), int_float_override: None,
    };

    match likelihood {
        ResolvedLikelihood::NegBinomial { mean, .. } => {
            eval_resolved(mean, &ctx(projected))
        }
        ResolvedLikelihood::Normal { mean, .. } => {
            eval_resolved(mean, &ctx(projected))
        }
        ResolvedLikelihood::Poisson { rate } => {
            eval_resolved(rate, &ctx(projected))
        }
        ResolvedLikelihood::Binomial { n, p } => {
            let n_val = eval_resolved(n, &ctx(projected));
            let p_val = eval_resolved(p, &ctx(projected));
            n_val * p_val
        }
        ResolvedLikelihood::BetaBinomial { n, alpha, beta } => {
            // E[BetaBinomial(n, α, β)] = n · α / (α + β)
            let n_val = eval_resolved(n, &ctx(projected));
            let alpha_val = eval_resolved(alpha, &ctx(projected));
            let beta_val  = eval_resolved(beta,  &ctx(projected));
            let denom = (alpha_val + beta_val).max(1e-300);
            n_val * (alpha_val / denom)
        }
        ResolvedLikelihood::Bernoulli { p } => {
            eval_resolved(p, &ctx(projected))
        }
    }
}

// NOTE: The old compile_joint_obs_loglik was replaced by
// types::joint_obs_weight + types::ObsStreamSpec. The join now happens
// in ONE shared function used by PF, PGAS, CSMC, and gradient evaluation.
