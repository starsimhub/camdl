//! Particle Gibbs with Ancestor Sampling (PGAS) — Bayesian posterior
//! sampling via Gibbs sweeps alternating θ|X (exact MH) and X|θ,y
//! (conditional SMC with ancestor sampling).
//!
//! Lindsten, Jordan & Schön (2014). "Particle Gibbs with ancestor
//! sampling." JMLR 15:2145–2184.
//!
//! PGAS avoids the particle filter variance problem that plagues PMMH:
//! with the full trajectory X known, the complete-data log-likelihood
//! is exact (no estimation noise). The latent trajectory is refreshed
//! via CSMC-AS, which conditions on a reference trajectory and uses
//! ancestor sampling to maintain diversity.

use crate::chain_binomial::{StepScratch, step_one};
use crate::compiled_model::CompiledModel;
use crate::rng::StatefulRng;
use crate::error::SimError;
use crate::inference::obs_loglik::{poisson_logpmf, binom_logpmf};
use crate::inference::particle_filter::{Observation, DmeasureFn};
use crate::inference::resampling::systematic_resample;
use crate::inference::pmmh::Prior;
use crate::inference::if2::{IF2Param, Transform};
use crate::propensity::{eval_propensities, eval_expr, EvalCtx};
use crate::state::{IntState, RealState};

// ═══════════════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════════════

/// PGAS configuration.
pub struct PGASConfig {
    pub n_particles: usize,
    pub n_sweeps: usize,
    pub burn_in: usize,
    pub thin: usize,
    pub dt: f64,
}

/// Per-substep record: minimal information for transition density
/// evaluation and trajectory reconstruction.
#[derive(Clone)]
pub struct SubstepRecord {
    /// Compartment counts AFTER this substep.
    pub counts: Vec<i64>,
    /// Per-transition flow counts FOR THIS SUBSTEP ONLY.
    pub flows: Vec<u64>,
    /// Gamma multipliers used at this substep (one per overdispersed
    /// source group, in source_groups order). Empty if no overdispersion.
    pub gammas: Vec<f64>,
}

/// Full trajectory stored at substep resolution.
#[derive(Clone)]
pub struct PGASTrajectory {
    /// Compartment counts at simulation start (before any substep).
    pub initial_counts: Vec<i64>,
    /// One record per substep, ordered chronologically.
    pub substeps: Vec<SubstepRecord>,
}

/// Diagnostics from one CSMC-AS sweep.
#[derive(Clone, Debug)]
pub struct CSMCDiagnostics {
    /// Fraction of traceback substeps from non-reference particles.
    /// Near 0% = path degeneracy (reference never replaced, CSMC broken).
    /// Near 50%+ = healthy trajectory renewal.
    pub trajectory_renewal: f64,
    /// Number of substeps where all ancestor weights were -inf.
    pub n_degenerate: usize,
    /// Total substeps.
    pub n_substeps: usize,
}

/// Result of one Gibbs sweep.
#[derive(Clone)]
pub struct PGASSweep {
    pub params: Vec<f64>,
    pub log_complete_data_ll: f64,
    pub accepted: Vec<bool>,
    pub csmc_diag: CSMCDiagnostics,
    pub proposal_sds: Vec<f64>,
}

/// Full PGAS result.
pub struct PGASResult {
    pub sweeps: Vec<PGASSweep>,
    pub final_trajectory: PGASTrajectory,
    pub acceptance_rates: Vec<f64>,
}

// ═══════════════════════════════════════════════════════════════════
// Transition density
// ═══════════════════════════════════════════════════════════════════

/// Log transition density for ONE substep, mirroring step_one's
/// Euler-multinomial decomposition exactly.
///
/// Evaluates log p(flows | counts_before, params, gammas, t, dt).
///
/// CRITICAL: This must use the SAME rate computation, source grouping,
/// and split ordering as step_one. If this function computes p_split
/// differently from how step_one drew the split, ancestor weights will
/// be wrong and the sampler will degenerate silently.
pub fn log_transition_density_substep(
    model: &CompiledModel,
    counts_before: &[i64],
    flows: &[u64],
    gammas: &[f64],
    params: &[f64],
    t: f64,
    dt: f64,
) -> Result<f64, SimError> {
    let n_int = model.int_local_to_global.len();
    let n_tr = model.model.transitions.len();

    // Set up evaluation context (same as step_one)
    let mut int_s = IntState::new(n_int);
    int_s.counts.copy_from_slice(counts_before);
    let real_s = RealState::new(model.real_local_to_global.len());

    let mut propensities = vec![0.0; n_tr];
    eval_propensities(model, &int_s, &real_s, params, t, &mut propensities)?;

    // Pre-evaluate draw methods (same as step_one lines 374-385)
    let ctx = EvalCtx {
        model, int_s: &int_s, real_s: &real_s, params, t, projected: None,
    };
    let mut overdispersions: Vec<Option<f64>> = Vec::with_capacity(n_tr);
    let mut is_deterministic = vec![false; n_tr];
    for tr in &model.model.transitions {
        match &tr.draw_method {
            ir::transition::DrawMethod::Overdispersed(expr) => {
                overdispersions.push(Some(eval_expr(expr, &ctx)?));
            }
            ir::transition::DrawMethod::Deterministic => {
                overdispersions.push(None);
                is_deterministic[overdispersions.len() - 1] = true;
            }
            _ => { overdispersions.push(None); }
        }
    }
    // Fix: is_deterministic should be indexed by transition, not by overdispersions index
    let mut is_determ = vec![false; n_tr];
    for (i, tr) in model.model.transitions.iter().enumerate() {
        if matches!(tr.draw_method, ir::transition::DrawMethod::Deterministic) {
            is_determ[i] = true;
        }
    }
    // Re-evaluate overdispersion properly indexed by transition
    let mut sigma_sq_by_tr: Vec<Option<f64>> = vec![None; n_tr];
    for (i, tr) in model.model.transitions.iter().enumerate() {
        if let ir::transition::DrawMethod::Overdispersed(expr) = &tr.draw_method {
            sigma_sq_by_tr[i] = Some(eval_expr(expr, &ctx)?);
        }
    }

    let mut log_p = 0.0;
    let mut handled = vec![false; n_tr];
    let mut gamma_idx = 0;

    // Source-grouped transitions (mirrors step_one's Euler-multinomial)
    for &(src_local, ref group) in &model.source_groups {
        let n_src = counts_before[src_local].max(0);
        if n_src == 0 {
            for &tr_idx in group {
                if flows[tr_idx] > 0 { return Ok(f64::NEG_INFINITY); }
                handled[tr_idx] = true;
            }
            continue;
        }

        // Step 1: compute effective per-capita rates (same as step_one)
        let mut probs: Vec<(usize, f64)> = Vec::new();
        let mut total_rate = 0.0_f64;
        let mut group_has_overdispersion = false;
        for &tr_idx in group {
            let rate = propensities[tr_idx];
            if rate <= 0.0 || is_determ[tr_idx] {
                handled[tr_idx] = true;
                continue;
            }
            let per_capita = rate / n_src as f64;
            let effective = if let Some(_sigma_sq) = sigma_sq_by_tr[tr_idx] {
                group_has_overdispersion = true;
                let g = if gamma_idx < gammas.len() { gammas[gamma_idx] } else { 1.0 };
                // Don't increment gamma_idx yet — all transitions in the same
                // group share the same gamma (step_one only draws one gamma
                // per overdispersed group via .take())
                per_capita * g
            } else {
                per_capita
            };
            total_rate += effective;
            probs.push((tr_idx, effective));
        }
        if group_has_overdispersion {
            gamma_idx += 1;
        }

        if total_rate <= 0.0 || probs.is_empty() { continue; }

        // Step 2: evaluate total exits density
        let p_total = (1.0 - (-total_rate * dt).exp()).clamp(0.0, 1.0);
        let n_exit: u64 = probs.iter().map(|&(tr_idx, _)| flows[tr_idx]).sum();
        log_p += binom_logpmf(n_exit, n_src as u64, p_total);

        // Step 3: evaluate split density (mirrors step_one's proportional split)
        let n_competing = probs.len();
        let mut remaining = n_exit;
        let mut rate_remaining = total_rate;
        for (k, &(tr_idx, eff_rate)) in probs.iter().enumerate() {
            handled[tr_idx] = true;
            if k == n_competing - 1 {
                // Last category: remainder — check consistency
                if flows[tr_idx] != remaining {
                    return Ok(f64::NEG_INFINITY);
                }
            } else if remaining > 0 && rate_remaining > 0.0 {
                let p_split = (eff_rate / rate_remaining).clamp(0.0, 1.0);
                log_p += binom_logpmf(flows[tr_idx], remaining, p_split);
                remaining -= flows[tr_idx];
                rate_remaining -= eff_rate;
            } else if flows[tr_idx] > 0 {
                return Ok(f64::NEG_INFINITY);
            }
        }
    }

    // Ungrouped / inflow transitions
    for (i, &rate) in propensities.iter().enumerate() {
        if handled[i] || rate <= 0.0 { continue; }
        let mean = rate * dt;
        if is_determ[i] {
            if flows[i] != mean.round() as u64 {
                return Ok(f64::NEG_INFINITY);
            }
        } else {
            // Poisson density (or overdispersed — approximate as Poisson
            // since ungrouped overdispersed transitions are rare)
            log_p += poisson_logpmf(flows[i] as f64, mean);
        }
    }

    Ok(log_p)
}

/// Complete-data log-likelihood: sum of transition densities + observation
/// densities over the full trajectory.
///
/// log p(y, X | θ) = Σ_s log p(x_s | x_{s-1}, θ, g_s)
///                 + Σ_k log p(y_k | project(x_{obs_k}), θ)
pub fn complete_data_loglik(
    model: &CompiledModel,
    trajectory: &PGASTrajectory,
    params: &[f64],
    observations: &[Observation],
    dt: f64,
    dmeasure_fn: &DmeasureFn,
    flow_indices: &[usize],
) -> Result<f64, SimError> {
    let t_start = model.model.simulation.t_start;
    let n_substeps = trajectory.substeps.len();
    let n_tr = model.model.transitions.len();
    let mut log_p = 0.0;

    // Precompute observation substep indices
    let mut obs_at_substep = std::collections::HashMap::new();
    for (obs_idx, obs) in observations.iter().enumerate() {
        let s = ((obs.time - t_start) / dt).round() as usize;
        if s > 0 { obs_at_substep.insert(s - 1, obs_idx); }
    }

    // Cumulative flows since last observation (for projection)
    let mut cum_flows = vec![0u64; n_tr];

    for s in 0..n_substeps {
        let t = t_start + s as f64 * dt;
        let counts_before = if s == 0 {
            &trajectory.initial_counts
        } else {
            &trajectory.substeps[s - 1].counts
        };
        let rec = &trajectory.substeps[s];

        // Transition density
        let td = log_transition_density_substep(
            model, counts_before, &rec.flows, &rec.gammas, params, t, dt,
        )?;
        if !td.is_finite() {
            // Early exit: one impossible transition makes the whole trajectory invalid.
            // Log the first -inf for debugging (env CAMDL_TRACE_STEPS=1).
            if crate::chain_binomial::trace_enabled() {
                eprintln!("[pgas] -inf transition density at substep {} (t={:.1})", s, t);
                eprintln!("  counts_before: {:?}", counts_before);
                eprintln!("  flows: {:?}", &rec.flows);
                eprintln!("  gammas: {:?}", &rec.gammas);
            }
            return Ok(f64::NEG_INFINITY);
        }
        log_p += td;

        // Accumulate flows
        for (i, &f) in rec.flows.iter().enumerate() {
            cum_flows[i] += f;
        }

        // Observation density at observation times
        if let Some(&obs_idx) = obs_at_substep.get(&s) {
            let projected: f64 = flow_indices.iter()
                .map(|&i| cum_flows[i] as f64).sum();
            log_p += dmeasure_fn(projected, observations[obs_idx].value);
            // Reset cumulative flows
            for f in &mut cum_flows { *f = 0; }
        }
    }

    Ok(log_p)
}

// ═══════════════════════════════════════════════════════════════════
// Forward simulation (initial trajectory)
// ═══════════════════════════════════════════════════════════════════

/// Simulate a forward trajectory recording per-substep detail.
/// Used to initialize the reference trajectory for PGAS.
pub fn simulate_reference(
    model: &CompiledModel,
    params: &[f64],
    t_end: f64,
    dt: f64,
    rng: &mut StatefulRng,
) -> Result<PGASTrajectory, SimError> {
    let (init_int, _) = model.initial_state(params)?;
    let n_tr = model.model.transitions.len();
    let t_start = model.model.simulation.t_start;
    let n_substeps = ((t_end - t_start) / dt).round() as usize;

    let mut counts = init_int.counts.clone();
    let mut scratch = StepScratch::new(model);
    let mut substeps = Vec::with_capacity(n_substeps);

    for s in 0..n_substeps {
        let t = t_start + s as f64 * dt;
        let mut flows = vec![0u64; n_tr];
        scratch.gamma_used.clear();

        step_one(model, &mut counts, &mut flows, params, t, dt, rng, &mut scratch)?;

        substeps.push(SubstepRecord {
            counts: counts.clone(),
            flows,
            gammas: scratch.gamma_used.clone(),
        });
    }

    Ok(PGASTrajectory {
        initial_counts: init_int.counts,
        substeps,
    })
}

// ═══════════════════════════════════════════════════════════════════
// Conditional SMC with Ancestor Sampling (CSMC-AS)
// ═══════════════════════════════════════════════════════════════════

/// Run one CSMC-AS sweep: draw X' ~ p(X | θ, y) conditioned on
/// the reference trajectory.
///
/// Returns a new trajectory + diagnostics.
pub fn csmc_as(
    model: &CompiledModel,
    params: &[f64],
    observations: &[Observation],
    reference: &PGASTrajectory,
    n_particles: usize,
    dt: f64,
    dmeasure_fn: &DmeasureFn,
    flow_indices: &[usize],
    seed: u64,
) -> Result<(PGASTrajectory, CSMCDiagnostics), SimError> {
    let t_start = model.model.simulation.t_start;
    let n_substeps = reference.substeps.len();
    let n_tr = model.model.transitions.len();
    let j_ref = n_particles - 1; // reference particle is the last slot

    // Precompute observation substep indices
    let mut obs_at_substep = std::collections::HashMap::new();
    for (obs_idx, obs) in observations.iter().enumerate() {
        let s = ((obs.time - t_start) / dt).round() as usize;
        if s > 0 { obs_at_substep.insert(s - 1, obs_idx); }
    }

    // Initialize particles
    let (init_int, _) = model.initial_state(params)?;

    // Current particle states: counts[j] and flows[j]
    let mut counts: Vec<Vec<i64>> = (0..n_particles)
        .map(|j| {
            if j == j_ref {
                reference.initial_counts.clone()
            } else {
                init_int.counts.clone()
            }
        })
        .collect();

    // Per-particle per-substep flows (reset each substep)
    let mut substep_flows: Vec<Vec<u64>> = (0..n_particles)
        .map(|_| vec![0u64; n_tr])
        .collect();
    let mut substep_gammas: Vec<Vec<f64>> = (0..n_particles)
        .map(|_| Vec::new())
        .collect();

    // Cumulative flows since last observation (for projection)
    let mut cum_flows: Vec<Vec<u64>> = (0..n_particles)
        .map(|_| vec![0u64; n_tr])
        .collect();

    // History for traceback
    let mut history_counts: Vec<Vec<Vec<i64>>> = Vec::with_capacity(n_substeps);
    let mut history_flows: Vec<Vec<Vec<u64>>> = Vec::with_capacity(n_substeps);
    let mut history_gammas: Vec<Vec<Vec<f64>>> = Vec::with_capacity(n_substeps);
    let mut ancestors: Vec<Vec<usize>> = Vec::with_capacity(n_substeps);

    // Weights (log-space)
    let mut log_weights = vec![0.0f64; n_particles];

    // Per-particle RNGs
    let mut rngs: Vec<StatefulRng> = (0..n_particles)
        .map(|i| StatefulRng::new(seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95)))
        .collect();
    let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));

    // Per-particle scratch buffers
    let mut scratches: Vec<StepScratch> = (0..n_particles)
        .map(|_| StepScratch::new(model))
        .collect();

    // Previous states (for ancestor sampling: need state before propagation)
    let mut prev_counts: Vec<Vec<i64>> = counts.clone();

    // Diagnostic: count substeps where ancestor sampling is degenerate
    // (no particle can reach the reference state → reference stays self-connected)
    let mut n_degenerate: usize = 0;

    for s in 0..n_substeps {
        let t = t_start + s as f64 * dt;

        // ── 1. Resample free particles (ancestor selection from prev weights) ──
        // On non-observation substeps, weights are uniform → systematic
        // resampling is identity. Skip resampling in that case.
        let substep_ancestors: Vec<usize>;
        let weights_are_uniform = log_weights.iter().all(|&w| (w - log_weights[0]).abs() < 1e-10);

        if weights_are_uniform {
            // Identity: each particle is its own ancestor
            substep_ancestors = (0..n_particles).collect();
        } else {
            // Resample from previous weights
            let indices = systematic_resample(&log_weights, &mut resample_rng);
            // Apply resampling to free particles (not reference)
            let mut new_counts = Vec::with_capacity(n_particles);
            let mut new_cum_flows = Vec::with_capacity(n_particles);
            for j in 0..n_particles {
                if j == j_ref {
                    new_counts.push(counts[j_ref].clone());
                    new_cum_flows.push(cum_flows[j_ref].clone());
                } else {
                    new_counts.push(counts[indices[j]].clone());
                    new_cum_flows.push(cum_flows[indices[j]].clone());
                }
            }
            counts = new_counts;
            cum_flows = new_cum_flows;
            substep_ancestors = indices;
        }

        // Save pre-propagation states for ancestor sampling
        for j in 0..n_particles {
            prev_counts[j].copy_from_slice(&counts[j]);
        }

        // ── 2. Propagate free particles ──
        for j in 0..n_particles {
            if j == j_ref { continue; }
            // Reset substep flows
            for f in &mut substep_flows[j] { *f = 0; }
            scratches[j].gamma_used.clear();

            step_one(
                model, &mut counts[j], &mut substep_flows[j],
                params, t, dt, &mut rngs[j], &mut scratches[j],
            )?;

            substep_gammas[j] = scratches[j].gamma_used.clone();
        }

        // ── 3. Clamp reference particle ──
        let ref_rec = &reference.substeps[s];
        counts[j_ref].copy_from_slice(&ref_rec.counts);
        substep_flows[j_ref].copy_from_slice(&ref_rec.flows);
        substep_gammas[j_ref] = ref_rec.gammas.clone();

        // ── 4. Ancestor sampling for reference particle ──
        // ã_j = w_{s-1}^j + log f(X_ref_s | x_{s-1}^j, θ, gamma_ref_s)
        // The gamma from the reference is used because we're asking:
        // "given this gamma noise, what's P(reaching ref state from particle j?)"
        {
            let mut ancestor_log_w = vec![f64::NEG_INFINITY; n_particles];
            for j in 0..n_particles {
                let td = log_transition_density_substep(
                    model,
                    &prev_counts[j],
                    &ref_rec.flows,
                    &ref_rec.gammas,
                    params,
                    t,
                    dt,
                )?;
                ancestor_log_w[j] = log_weights[j] + td;
            }

            // Sample from categorical(softmax(ancestor_log_w))
            let max_w = ancestor_log_w.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let ref_ancestor = if max_w.is_finite() {
                let weights: Vec<f64> = ancestor_log_w.iter()
                    .map(|&w| (w - max_w).exp())
                    .collect();
                let sum: f64 = weights.iter().sum();
                if sum > 0.0 {
                    let u = resample_rng.uniform() * sum;
                    let mut cum = 0.0;
                    let mut selected = 0;
                    for (j, &w) in weights.iter().enumerate() {
                        cum += w;
                        if cum >= u { selected = j; break; }
                    }
                    selected
                } else {
                    // Degenerate: no particle can reach the reference state.
                    // Keep the reference's own history to maintain internal
                    // consistency (the reference's flows at substep s were
                    // produced from the reference's state at substep s-1).
                    // Picking a random particle here causes splice-point
                    // inconsistencies → -inf in complete_data_loglik.
                    n_degenerate += 1;
                    j_ref
                }
            } else {
                // All ancestor weights are -inf: reference unreachable
                n_degenerate += 1;
                j_ref
            };

            // Record ancestor for reference particle
            let mut step_ancestors = substep_ancestors;
            step_ancestors[j_ref] = ref_ancestor;
            ancestors.push(step_ancestors);
        }

        // Accumulate cumulative flows
        for j in 0..n_particles {
            for (i, &f) in substep_flows[j].iter().enumerate() {
                cum_flows[j][i] += f;
            }
        }

        // ── 5. Compute weights ──
        if let Some(&obs_idx) = obs_at_substep.get(&s) {
            let obs_value = observations[obs_idx].value;
            for j in 0..n_particles {
                let projected: f64 = flow_indices.iter()
                    .map(|&i| cum_flows[j][i] as f64).sum();
                log_weights[j] = dmeasure_fn(projected, obs_value);
            }
            // Reset cumulative flows
            for j in 0..n_particles {
                for f in &mut cum_flows[j] { *f = 0; }
            }
        } else {
            // Non-observation substep: uniform weights
            for w in &mut log_weights { *w = 0.0; }
        }

        // ── 6. Store history ──
        history_counts.push(counts.iter().map(|c| c.clone()).collect());
        history_flows.push(substep_flows.iter().map(|f| f.clone()).collect());
        history_gammas.push(substep_gammas.iter().map(|g| g.clone()).collect());
    }

    // Diagnostic: warn if many substeps had degenerate ancestor sampling
    if n_degenerate > 0 {
        let pct = n_degenerate as f64 / n_substeps as f64 * 100.0;
        if pct > 10.0 {
            log::warn!("CSMC-AS: {}/{} substeps ({:.0}%) had degenerate ancestor sampling — \
                        reference trajectory is too far from particle cloud. \
                        Consider more particles or smaller parameter proposals.",
                        n_degenerate, n_substeps, pct);
        }
    }

    // ── Select final trajectory ──
    // Sample from final weights
    let k = {
        let max_w = log_weights.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if max_w.is_finite() {
            let weights: Vec<f64> = log_weights.iter()
                .map(|&w| (w - max_w).exp())
                .collect();
            let sum: f64 = weights.iter().sum();
            let u = resample_rng.uniform() * sum;
            let mut cum = 0.0;
            let mut selected = 0;
            for (j, &w) in weights.iter().enumerate() {
                cum += w;
                if cum >= u { selected = j; break; }
            }
            selected
        } else {
            j_ref // fallback to reference
        }
    };

    // Trace back through ancestry and compute trajectory renewal
    let mut trajectory_substeps = Vec::with_capacity(n_substeps);
    let mut particle = k;
    let mut n_from_ref = 0usize;
    for s in (0..n_substeps).rev() {
        if particle == j_ref { n_from_ref += 1; }
        trajectory_substeps.push(SubstepRecord {
            counts: history_counts[s][particle].clone(),
            flows: history_flows[s][particle].clone(),
            gammas: history_gammas[s][particle].clone(),
        });
        particle = ancestors[s][particle];
    }
    trajectory_substeps.reverse();

    let trajectory_renewal = 1.0 - n_from_ref as f64 / n_substeps as f64;

    // Initial counts: the ancestor at step 0 determines whose initial state we use
    let initial_counts = if particle == j_ref {
        reference.initial_counts.clone()
    } else {
        let (init_int, _) = model.initial_state(params)?;
        init_int.counts
    };

    let diag = CSMCDiagnostics {
        trajectory_renewal,
        n_degenerate,
        n_substeps,
    };

    Ok((PGASTrajectory {
        initial_counts,
        substeps: trajectory_substeps,
    }, diag))
}

// ═══════════════════════════════════════════════════════════════════
// Jacobian for transforms (shared with PMMH)
// ═══════════════════════════════════════════════════════════════════

fn log_jacobian(param: &IF2Param, z: f64) -> f64 {
    match &param.transform {
        Transform::Log { .. } => z, // d/dz exp(z) = exp(z), log|exp(z)| = z
        Transform::Logit { lo, hi } => {
            let p = 1.0 / (1.0 + (-z).exp());
            ((hi - lo) * p * (1.0 - p)).ln()
        }
        Transform::None => 0.0,
    }
}

// ═══════════════════════════════════════════════════════════════════
// Main PGAS loop
// ═══════════════════════════════════════════════════════════════════

/// Run the PGAS Gibbs sampler.
///
/// Alternates between:
/// 1. θ | y — MH updates using PF marginal log-likelihood (smooth surface)
/// 2. X | θ, y — CSMC-AS to refresh the latent trajectory
///
/// Step 1 uses a particle filter to estimate log p(y|θ), NOT the complete-data
/// log p(y,X|θ). This gives a smooth likelihood surface that allows large
/// proposals. The PF variance is handled by the pseudo-marginal property
/// (Andrieu & Roberts 2009). Step 2 then produces a trajectory sample
/// conditioned on the accepted θ.
pub fn run_pgas(
    model: &CompiledModel,
    if2_params: &[IF2Param],
    priors: &[Prior],
    base_params: &[f64],
    config: &PGASConfig,
    observations: &[Observation],
    dmeasure_fn: &DmeasureFn,
    flow_indices: &[usize],
    seed: u64,
    on_sweep: Option<&dyn Fn(usize, &PGASSweep, &PGASTrajectory)>,
) -> Result<PGASResult, SimError> {
    let d = if2_params.len();
    assert_eq!(d, priors.len(), "priors must match if2_params length");

    let mut rng = StatefulRng::new(seed);
    let mut current_params = base_params.to_vec();
    let t_end = observations.last().map_or(
        model.model.simulation.t_start,
        |o| o.time,
    );

    // Initialize: forward simulation to get the first reference trajectory
    eprintln!("  initializing reference trajectory...");
    let mut trajectory = simulate_reference(
        model, &current_params, t_end, config.dt, &mut rng,
    )?;
    eprintln!("  reference: {} substeps, initial S={}",
        trajectory.substeps.len(),
        trajectory.initial_counts.get(0).copied().unwrap_or(0));

    // Current transformed parameters
    let mut current_transformed: Vec<f64> = if2_params.iter()
        .map(|p| p.to_transformed(current_params[p.index]))
        .collect();

    // Adaptive proposal SDs via Robbins-Monro stochastic approximation.
    // Each parameter's log(proposal_sd) is nudged after every MH attempt
    // to target 44% acceptance (optimal for 1D MH, Roberts & Rosenthal 2001).
    // The adaptation rate c/√sweep decays to zero, so the proposal stabilizes.
    //
    // Initial scale: (upper - lower) / 10 on the TRANSFORMED scale, giving
    // the chain room to explore broadly during early burn-in. The Robbins-Monro
    // then narrows it to the right scale for each parameter. Starting too
    // small (e.g., rw_sd × 0.1) causes the chain to get stuck near its
    // starting values — the adaptation sees ~44% acceptance (because steps
    // are tiny) and never discovers that larger steps are needed.
    const TARGET_ACCEPTANCE: f64 = 0.44;
    const ADAPT_C: f64 = 2.0; // adaptation speed (higher = faster convergence)
    let adapt_end = config.burn_in; // stop adapting at end of burn-in

    let mut log_proposal_sd: Vec<f64> = if2_params.iter()
        .map(|p| {
            let lo = p.to_transformed(p.lower.max(1e-10));
            let hi = p.to_transformed(p.upper.min(1e10));
            let range = (hi - lo).abs();
            // 10% of the transformed-scale range: broad enough to explore,
            // Robbins-Monro will shrink to the right scale within ~200 sweeps
            (range / 10.0).max(0.01).ln()
        })
        .collect();

    // Helper: run a PF at given params and return marginal log-likelihood.
    // This is the smooth surface we use for parameter updates.
    let n_pf = config.n_particles; // same particle count for PF and CSMC
    let run_pf = |params: &[f64], pf_seed: u64| -> f64 {
        let step_fn = |state: &mut crate::inference::types::ParticleState,
                       t: f64, step_dt: f64,
                       rng: &mut StatefulRng,
                       scratch: &mut StepScratch| {
            step_one(model, &mut state.counts, &mut state.flow_accumulators,
                     params, t, step_dt, rng, scratch)
        };
        let project_fn = |state: &crate::inference::types::ParticleState| -> f64 {
            flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
        };
        match crate::inference::particle_filter::bootstrap_filter(
            model, params, observations, n_pf, config.dt,
            &step_fn, &project_fn, dmeasure_fn, None, None, pf_seed,
        ) {
            Ok(r) => r.log_likelihood,
            Err(_) => f64::NEG_INFINITY,
        }
    };

    // Initial PF loglik at starting params (pseudo-marginal baseline)
    let mut current_ll = run_pf(&current_params, seed.wrapping_add(0));
    eprintln!("  initial PF loglik: {:.1} ({} particles)", current_ll, n_pf);

    let mut sweeps = Vec::new();
    let mut total_accepted = vec![0usize; d];

    for sweep in 0..config.n_sweeps {
        let mut accepted = vec![false; d];

        // Current proposal SDs (from adaptive log scale)
        let proposal_sd: Vec<f64> = log_proposal_sd.iter()
            .map(|&ls| ls.exp())
            .collect();

        // ── Step 1: Update θ | y via PF marginal likelihood ──
        // One-at-a-time MH with pseudo-marginal PF loglik (smooth surface).
        // Each proposal requires one PF evaluation (~0.1s with 100 particles).
        for i in 0..d {
            let spec = &if2_params[i];
            let z_old = current_transformed[i];
            let z_new = z_old + proposal_sd[i] * rng.normal();
            let theta_new = spec.from_transformed(z_new);

            let mut proposed_params = current_params.clone();
            proposed_params[spec.index] = theta_new;

            // PF marginal loglik at proposed params
            let pf_seed = seed.wrapping_add(sweep as u64 * d as u64 + i as u64 + 1);
            let proposed_ll = run_pf(&proposed_params, pf_seed);

            let proposed_log_prior_i = priors[i].log_density(theta_new, z_new);
            let current_log_prior_i = priors[i].log_density(
                current_params[spec.index], z_old,
            );
            let proposed_log_jac_i = log_jacobian(spec, z_new);
            let current_log_jac_i = log_jacobian(spec, z_old);

            let log_alpha = (proposed_ll + proposed_log_prior_i + proposed_log_jac_i)
                          - (current_ll + current_log_prior_i + current_log_jac_i);

            if log_alpha.is_finite() && rng.uniform().ln() < log_alpha {
                current_params[spec.index] = theta_new;
                current_transformed[i] = z_new;
                current_ll = proposed_ll;
                accepted[i] = true;
                total_accepted[i] += 1;
            }

            // Robbins-Monro: adapt proposal SD during burn-in
            if sweep < adapt_end {
                let gamma = ADAPT_C / (1.0 + sweep as f64).sqrt();
                let acc_indicator = if accepted[i] { 1.0 } else { 0.0 };
                log_proposal_sd[i] += gamma * (acc_indicator - TARGET_ACCEPTANCE);
                log_proposal_sd[i] = log_proposal_sd[i].clamp(-20.0, 5.0);
            }
        }

        // ── Step 2: Update X | θ, y via CSMC-AS ──
        // The trajectory update is independent of the PF loglik used in step 1.
        // CSMC-AS conditions on the reference trajectory and produces a new
        // trajectory sample from p(X | θ, y).
        let csmc_seed = seed ^ ((sweep as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15));
        let (new_trajectory, csmc_diag) = csmc_as(
            model, &current_params, observations, &trajectory,
            config.n_particles, config.dt, dmeasure_fn, flow_indices,
            csmc_seed,
        )?;
        trajectory = new_trajectory;

        // Log adapted proposal SDs at end of burn-in
        if sweep + 1 == adapt_end {
            eprintln!("  proposal SD adapted (end of burn-in):");
            for (i, spec) in if2_params.iter().enumerate() {
                let acc_rate = total_accepted[i] as f64 / (sweep + 1) as f64;
                eprintln!("    {:12} sd={:.6} acc={:.0}%",
                    spec.name, log_proposal_sd[i].exp(), acc_rate * 100.0);
            }
            eprintln!("  trajectory renewal: {:.1}%", csmc_diag.trajectory_renewal * 100.0);
        }

        let sweep_result = PGASSweep {
            params: current_params.clone(),
            log_complete_data_ll: current_ll,
            accepted,
            csmc_diag: csmc_diag.clone(),
            proposal_sds: proposal_sd.clone(),
        };

        if let Some(cb) = on_sweep {
            cb(sweep, &sweep_result, &trajectory);
        }

        // Record (respecting burn-in and thinning)
        if sweep >= config.burn_in && (sweep - config.burn_in) % config.thin == 0 {
            sweeps.push(sweep_result);
        }
    }

    let acceptance_rates: Vec<f64> = total_accepted.iter()
        .map(|&n| n as f64 / config.n_sweeps as f64)
        .collect();

    Ok(PGASResult {
        sweeps,
        final_trajectory: trajectory,
        acceptance_rates,
    })
}
