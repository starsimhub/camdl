//! Gradient evaluation for the PGAS complete-data log-likelihood.
//!
//! Uses compiler-emitted derivative expressions (`rate_grad` on each transition)
//! to compute ∂log p(y,X|θ)/∂θ analytically. No runtime autodiff or finite
//! differences — just evaluating pre-differentiated expression trees.
//!
//! The chain rule through p_total and binom_logpmf is hardcoded here:
//!   ∂/∂θ log Binom(k; n, p(θ)) = [k/p - (n-k)/(1-p)] × dp/dθ
//!   dp/dθ = dt × exp(-total_rate × dt) × d(total_rate)/dθ
//!
//! The d(rate)/dθ terms come from the OCaml compiler's symbolic differentiation.

use crate::compiled_model::CompiledModel;
use crate::error::SimError;
use crate::propensity::{eval_propensities, eval_expr, eval_expr_deriv, EvalCtx};
use crate::state::{IntState, RealState};
use crate::inference::obs_loglik::binom_logpmf;
use crate::inference::pgas::{PGASTrajectory, IVPMapping};
use crate::inference::particle_filter::Observation;

/// Evaluate log transition density AND its gradient w.r.t. estimated parameters
/// for a single substep.
///
/// Returns (log_p, grad) where grad[i] = ∂log_p/∂θ_i.
///
/// `param_names` are the names of estimated parameters (matching keys in rate_grad).
/// `param_indices` are their indices into the params array.
pub fn log_transition_density_grad(
    model: &CompiledModel,
    counts_before: &[i64],
    flows: &[u64],
    gammas: &[f64],
    params: &[f64],
    t: f64,
    dt: f64,
    param_names: &[String],
    _param_indices: &[usize],
) -> Result<(f64, Vec<f64>), SimError> {
    let d = param_names.len();
    let n_int = model.int_local_to_global.len();
    let n_tr = model.model.transitions.len();

    let mut int_s = IntState::new(n_int);
    int_s.counts.copy_from_slice(counts_before);
    let real_s = RealState::new(model.real_local_to_global.len());

    let mut propensities = vec![0.0; n_tr];
    eval_propensities(model, &int_s, &real_s, params, t, &mut propensities)?;

    let ctx = EvalCtx {
        model, int_s: &int_s, real_s: &real_s, params, t, projected: None,
    };

    let mut log_p = 0.0;
    let mut grad = vec![0.0; d];
    let mut handled = vec![false; n_tr];
    let mut gamma_idx = 0;

    // Source-grouped transitions (mirrors step_one + log_transition_density_substep)
    for &(src_local, ref group) in &model.source_groups {
        let n_src = counts_before[src_local].max(0);
        if n_src == 0 {
            for &tr_idx in group {
                if flows[tr_idx] > 0 { return Ok((f64::NEG_INFINITY, vec![0.0; d])); }
                handled[tr_idx] = true;
            }
            continue;
        }

        // Compute effective per-capita rates AND their gradients
        let mut probs: Vec<(usize, f64, Vec<f64>)> = Vec::new(); // (tr_idx, eff_rate, d_eff_rate/dθ)
        let mut total_rate = 0.0_f64;
        let mut total_rate_grad = vec![0.0; d];
        let mut group_has_overdispersion = false;

        for &tr_idx in group {
            let rate = propensities[tr_idx];
            if rate <= 0.0 || matches!(model.model.transitions[tr_idx].draw_method,
                ir::transition::DrawMethod::Deterministic) {
                handled[tr_idx] = true;
                continue;
            }
            let per_capita = rate / n_src as f64;

            // Compute d(rate)/dθ for each estimated parameter
            let tr = &model.model.transitions[tr_idx];
            let mut d_rate = vec![0.0; d];
            for (i, pname) in param_names.iter().enumerate() {
                if let Some(grad_expr) = tr.rate_grad.get(pname) {
                    d_rate[i] = eval_expr(grad_expr, &ctx).unwrap_or(0.0) / n_src as f64;
                }
            }

            let (effective, d_effective) = if let ir::transition::DrawMethod::Overdispersed(_) =
                &tr.draw_method
            {
                group_has_overdispersion = true;
                let g = if gamma_idx < gammas.len() { gammas[gamma_idx] } else { 1.0 };
                let eff = per_capita * g;
                let d_eff: Vec<f64> = d_rate.iter().map(|&dr| dr * g).collect();
                (eff, d_eff)
            } else {
                (per_capita, d_rate)
            };

            total_rate += effective;
            for i in 0..d { total_rate_grad[i] += d_effective[i]; }
            probs.push((tr_idx, effective, d_effective));
        }
        if group_has_overdispersion { gamma_idx += 1; }

        if total_rate <= 0.0 || probs.is_empty() { continue; }

        // Total exits: Binom(n_exit; n_src, p_total)
        let p_total = (1.0 - (-total_rate * dt).exp()).clamp(1e-15, 1.0 - 1e-15);
        let n_exit: u64 = probs.iter().map(|&(tr_idx, _, _)| flows[tr_idx]).sum();
        log_p += binom_logpmf(n_exit, n_src as u64, p_total);

        // Gradient of binom_logpmf w.r.t. p_total:
        //   d/dp [k*ln(p) + (n-k)*ln(1-p)] = k/p - (n-k)/(1-p)
        let dbinom_dp = n_exit as f64 / p_total - (n_src as u64 - n_exit) as f64 / (1.0 - p_total);

        // dp_total/d(total_rate) = dt * exp(-total_rate * dt)
        let dp_dtotalrate = dt * (-total_rate * dt).exp();

        // Chain rule: d(binom)/dθ = dbinom_dp * dp_dtotalrate * d(total_rate)/dθ
        for i in 0..d {
            grad[i] += dbinom_dp * dp_dtotalrate * total_rate_grad[i];
        }

        // Split density: Binom(flow_k; remaining, p_split)
        let n_competing = probs.len();
        let mut remaining = n_exit;
        let mut rate_remaining = total_rate;
        let mut rate_remaining_grad = total_rate_grad.clone();

        for (k, &(tr_idx, eff_rate, ref d_eff_rate)) in probs.iter().enumerate() {
            handled[tr_idx] = true;
            if k == n_competing - 1 {
                if flows[tr_idx] != remaining {
                    return Ok((f64::NEG_INFINITY, vec![0.0; d]));
                }
                // Last category: no density contribution (remainder)
            } else if remaining > 0 && rate_remaining > 0.0 {
                let p_split = (eff_rate / rate_remaining).clamp(1e-15, 1.0 - 1e-15);
                let flow_k = flows[tr_idx];
                log_p += binom_logpmf(flow_k, remaining, p_split);

                // Gradient of p_split = eff_rate / rate_remaining
                // d(p_split)/dθ = (d_eff * rate_rem - eff * d_rate_rem) / rate_rem²
                let dbinom_dp_split = flow_k as f64 / p_split
                    - (remaining - flow_k) as f64 / (1.0 - p_split);
                for i in 0..d {
                    let dp_split = (d_eff_rate[i] * rate_remaining
                        - eff_rate * rate_remaining_grad[i])
                        / (rate_remaining * rate_remaining);
                    grad[i] += dbinom_dp_split * dp_split;
                }

                remaining -= flow_k;
                rate_remaining -= eff_rate;
                for i in 0..d { rate_remaining_grad[i] -= d_eff_rate[i]; }
            } else if flows[tr_idx] > 0 {
                return Ok((f64::NEG_INFINITY, vec![0.0; d]));
            }
        }
    }

    // Ungrouped / inflow transitions (Poisson)
    for (tr_idx, &rate) in propensities.iter().enumerate() {
        if handled[tr_idx] || rate <= 0.0 { continue; }
        let mean = rate * dt;
        let flow = flows[tr_idx] as f64;

        // log Poisson(k; λ) = k*ln(λ) - λ - lgamma(k+1)
        // d/dλ = k/λ - 1
        // dλ/dθ = d(rate)/dθ * dt
        log_p += crate::inference::obs_loglik::poisson_logpmf(flow, mean);

        let tr = &model.model.transitions[tr_idx];
        for (i, pname) in param_names.iter().enumerate() {
            if let Some(grad_expr) = tr.rate_grad.get(pname) {
                let d_rate = eval_expr(grad_expr, &ctx).unwrap_or(0.0);
                let d_mean = d_rate * dt;
                if mean > 0.0 {
                    grad[i] += (flow / mean - 1.0) * d_mean;
                }
            }
        }
    }

    Ok((log_p, grad))
}

/// Log-density of gamma multipliers AND gradient w.r.t. estimated params.
///
/// For each overdispersed source group, evaluates
/// log Gamma(g; dt/σ², σ²/dt) and its gradient through σ².
fn log_gamma_density_grad_substep(
    model: &CompiledModel,
    counts_before: &[i64],
    gammas: &[f64],
    params: &[f64],
    t: f64,
    dt: f64,
    param_indices: &[usize],
) -> Result<(f64, Vec<f64>), SimError> {
    use crate::inference::obs_loglik::{log_gamma_density, digamma};

    let d = param_indices.len();
    let n_int = model.int_local_to_global.len();
    let mut int_s = IntState::new(n_int);
    int_s.counts.copy_from_slice(counts_before);
    let real_s = RealState::new(model.real_local_to_global.len());
    let ctx = EvalCtx {
        model, int_s: &int_s, real_s: &real_s, params, t, projected: None,
    };

    let mut log_p = 0.0;
    let mut grad = vec![0.0; d];
    let mut gamma_idx = 0;

    for &(_, ref group) in &model.source_groups {
        let mut sigma_sq_expr: Option<&ir::expr::Expr> = None;
        let mut sigma_sq = 1.0;

        for &tr_idx in group {
            if let ir::transition::DrawMethod::Overdispersed(ref expr)
                = model.model.transitions[tr_idx].draw_method
            {
                sigma_sq = eval_expr(expr, &ctx).unwrap_or(1.0);
                sigma_sq_expr = Some(expr);
                break;
            }
        }
        if let Some(expr) = sigma_sq_expr {
            if gamma_idx < gammas.len() {
                let g = gammas[gamma_idx];
                if g > 0.0 && sigma_sq > 0.0 {
                    let shape = dt / sigma_sq;
                    let scale = sigma_sq / dt;
                    log_p += log_gamma_density(g, shape, scale);

                    // d(log Gamma)/d(shape) = ln(g) - ln(scale) - ψ(shape)
                    let dlg_dshape = g.ln() - scale.ln() - digamma(shape);
                    // d(log Gamma)/d(scale) = g/scale² - shape/scale
                    let dlg_dscale = g / (scale * scale) - shape / scale;
                    // d(shape)/d(σ²) = -dt/σ⁴, d(scale)/d(σ²) = 1/dt
                    let dshape_dsq = -dt / (sigma_sq * sigma_sq);
                    let dscale_dsq = 1.0 / dt;
                    let dlg_dsq = dlg_dshape * dshape_dsq + dlg_dscale * dscale_dsq;

                    // Chain rule through σ² expression
                    for i in 0..d {
                        let d_sq = eval_expr_deriv(expr, param_indices[i], &ctx);
                        grad[i] += dlg_dsq * d_sq;
                    }
                }
                gamma_idx += 1;
            }
        }
    }
    Ok((log_p, grad))
}

/// Gradient of the complete-data log-likelihood over all substeps.
///
/// Returns (log_p, grad) summed over transition densities + observation densities.
/// Observation model gradient is zero when obs model params (rho, psi) are fixed.
pub fn complete_data_loglik_grad(
    model: &CompiledModel,
    trajectory: &PGASTrajectory,
    params: &[f64],
    observations: &[Observation],
    dt: f64,
    obs_streams: &[super::types::ObsStreamSpec],
    
    ivp_mappings: &[IVPMapping],
    param_names: &[String],
    param_indices: &[usize],
) -> Result<(f64, Vec<f64>), SimError> {
    let t_start = model.model.simulation.t_start;
    let n_substeps = trajectory.substeps.len();
    let n_tr = model.model.transitions.len();
    let d = param_names.len();
    let mut log_p = 0.0;
    let mut grad = vec![0.0; d];

    // Initial state density gradient: d/dθ log Binom(S₀; N₀, s0)
    if !ivp_mappings.is_empty() {
        let total_pop: i64 = trajectory.initial_counts.iter().sum();
        for ivp in ivp_mappings {
            let count = trajectory.initial_counts[ivp.compartment_idx] as u64;
            let frac = params[ivp.model_param_idx].clamp(1e-10, 1.0 - 1e-10);
            log_p += binom_logpmf(count, total_pop as u64, frac);

            // d/d(frac) log Binom(count; N, frac) = count/frac - (N-count)/(1-frac)
            let dbinom_dfrac = count as f64 / frac
                - (total_pop as u64 - count) as f64 / (1.0 - frac);
            grad[ivp.param_idx] += dbinom_dfrac;
        }
    }

    // Precompute observation substep indices
    let mut obs_at_substep = std::collections::HashMap::new();
    for (obs_idx, obs) in observations.iter().enumerate() {
        let s = ((obs.time - t_start) / dt).round() as usize;
        if s > 0 { obs_at_substep.insert(s - 1, obs_idx); }
    }

    let mut cum_flows = vec![0u64; n_tr];

    for s in 0..n_substeps {
        let t = t_start + s as f64 * dt;
        let counts_before = if s == 0 {
            &trajectory.initial_counts
        } else {
            &trajectory.substeps[s - 1].counts
        };
        let rec = &trajectory.substeps[s];

        let (td, td_grad) = log_transition_density_grad(
            model, counts_before, &rec.flows, &rec.gammas,
            params, t, dt, param_names, param_indices,
        )?;

        if !td.is_finite() {
            return Ok((f64::NEG_INFINITY, vec![0.0; d]));
        }
        log_p += td;
        for i in 0..d { grad[i] += td_grad[i]; }

        // Gamma multiplier density gradient (for sigma_se estimation)
        if !rec.gammas.is_empty() {
            let (gd, gd_grad) = log_gamma_density_grad_substep(
                model, counts_before, &rec.gammas, params, t, dt, param_indices,
            )?;
            log_p += gd;
            for i in 0..d { grad[i] += gd_grad[i]; }
        }

        // Accumulate flows
        for (i, &f) in rec.flows.iter().enumerate() {
            cum_flows[i] += f;
        }

        // Observation density (gradient is zero when obs params are fixed)
        if let Some(&obs_idx) = obs_at_substep.get(&s) {
            log_p += super::types::joint_obs_weight(obs_streams, &cum_flows, obs_idx);
            for f in &mut cum_flows { *f = 0; }
        }
    }

    Ok((log_p, grad))
}
