//! Compile observation model likelihoods from the IR into dmeasure closures.
//!
//! Evaluates the Expr fields in the IR's Likelihood using the expression
//! evaluator with `projected` set to the projected observation value.

use std::sync::Arc;
use crate::compiled_model::CompiledModel;
use crate::propensity::{eval_expr, EvalCtx};
use crate::state::{IntState, RealState};
use crate::inference::obs_loglik::{negbin_logpmf, discretized_normal_logpmf_tol, poisson_logpmf, DEFAULT_TOL};
use ir::observation::{Likelihood, ObservationModel};

/// Build a dmeasure closure for IF2 (per-particle params).
/// Takes (projected, observed, params) → log-likelihood.
pub fn compile_dmeasure_if2(
    obs_model: &ObservationModel,
    compiled: Arc<CompiledModel>,
) -> Box<dyn Fn(f64, f64, &[f64]) -> f64 + Send + Sync> {
    let likelihood = obs_model.likelihood.clone();
    let n_int = compiled.int_local_to_global.len();
    let n_real = compiled.real_local_to_global.len();

    Box::new(move |projected: f64, observed: f64, params: &[f64]| {
        let int_s = IntState::new(n_int);
        let real_s = RealState::new(n_real);
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
    let n_int = compiled.int_local_to_global.len();
    let n_real = compiled.real_local_to_global.len();

    Box::new(move |projected: f64, observed: f64| {
        let int_s = IntState::new(n_int);
        let real_s = RealState::new(n_real);
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
            // Not yet implemented
            f64::NEG_INFINITY
        }
        Likelihood::Bernoulli(b) => {
            let p_val = eval_expr(&b.p, &ctx(projected)).unwrap_or(0.5);
            if observed > 0.5 { p_val.max(1e-300).ln() } else { (1.0 - p_val).max(1e-300).ln() }
        }
    }
}
