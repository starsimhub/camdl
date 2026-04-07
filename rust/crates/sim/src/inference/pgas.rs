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

use serde::{Serialize, Deserialize};

use crate::chain_binomial::{StepScratch, step_one};
use crate::compiled_model::CompiledModel;
use crate::rng::StatefulRng;
use crate::error::SimError;
use crate::inference::obs_loglik::{poisson_logpmf, binom_logpmf};
use crate::inference::particle_filter::{Observation, ObsLoglikFn};
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
    /// Use NUTS (gradient-based) for the θ|X step instead of MH-within-Gibbs.
    /// Requires rate_grad expressions in the IR (compiled with autodiff).
    /// Falls back to MH if gradients are not available.
    pub use_nuts: bool,
    /// Use dense (full covariance) mass matrix for NUTS. Default: true.
    /// Dense handles parameter correlations (e.g., R0-amplitude ridge).
    /// Set false for diagonal-only (handles scale but not correlations).
    pub dense_mass: bool,
}

/// Per-substep record: minimal information for transition density
/// evaluation and trajectory reconstruction.
#[derive(Clone, Serialize, Deserialize)]
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
#[derive(Clone, Serialize, Deserialize)]
pub struct PGASTrajectory {
    /// Compartment counts at simulation start (before any substep).
    pub initial_counts: Vec<i64>,
    /// One record per substep, ordered chronologically.
    pub substeps: Vec<SubstepRecord>,
}

/// Mapping from an IVP parameter to the compartment it controls.
/// Used to make the initial state stochastic in CSMC-AS and to add
/// the initial state density to the complete-data LL.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IVPMapping {
    /// Index into if2_params / priors vectors.
    pub param_idx: usize,
    /// Index into the model's param vector (if2_params[param_idx].index).
    pub model_param_idx: usize,
    /// Which compartment this IVP controls (local int index).
    pub compartment_idx: usize,
}

/// Diagnostics from one CSMC-AS sweep.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Serialize, Deserialize)]
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
    /// Resume state for chain continuation. Populated at end of every run.
    pub resume_state: ChainResumeState,
}

/// Serializable chain state for `--resume`. Saved to `chain_N/resume_state.bin`
/// via bincode at end of every PGAS run, enabling continuation without
/// re-doing burn-in or mass matrix adaptation.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChainResumeState {
    /// Config hash — only resume if the statistical problem matches.
    pub config_hash: String,
    /// Number of sweeps completed (resume starts from here).
    pub completed_sweeps: usize,
    /// Current parameter values (natural scale).
    pub params: Vec<f64>,
    /// Current transformed parameters (z-scale for NUTS).
    pub transformed: Vec<f64>,
    /// Reference trajectory from the last CSMC sweep.
    pub trajectory: PGASTrajectory,
    /// Adapted mass matrix (NUTS).
    pub mass_matrix: super::nuts::MassMatrix,
    /// Adapted step size (NUTS).
    pub nuts_step_size: f64,
    /// Adapted proposal SDs on log scale (MH-within-Gibbs).
    pub log_proposal_sd: Vec<f64>,
    /// Running acceptance counts per parameter.
    pub total_accepted: Vec<usize>,
    /// Current complete-data log-likelihood.
    pub current_ll: f64,
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

    let ctx = EvalCtx {
        model, int_s: &int_s, real_s: &real_s, params, t, projected: None,
    };

    // Per-transition: is it deterministic? What's its sigma_sq?
    let mut is_determ = vec![false; n_tr];
    let mut sigma_sq_by_tr: Vec<Option<f64>> = vec![None; n_tr];
    for (i, tr) in model.model.transitions.iter().enumerate() {
        match &tr.draw_method {
            ir::transition::DrawMethod::Deterministic => { is_determ[i] = true; }
            ir::transition::DrawMethod::Overdispersed(expr) => {
                sigma_sq_by_tr[i] = Some(eval_expr(expr, &ctx)?);
            }
            _ => {}
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
/// log p(y, X | θ) = log p(x₀ | θ)
///                 + Σ_s log p(x_s | x_{s-1}, θ, g_s)
///                 + Σ_k log p(y_k | project(x_{obs_k}), θ)
///
/// The initial state density log p(x₀ | θ) is included for IVP parameters
/// (e.g., S₀ ~ Binom(N₀, s0)). Without it, IVPs are invisible to the MH step.
pub fn complete_data_loglik(
    model: &CompiledModel,
    trajectory: &PGASTrajectory,
    params: &[f64],
    observations: &[Observation],
    dt: f64,
    obs_loglik_fn: &ObsLoglikFn,
    flow_indices: &[usize],
    ivp_mappings: &[IVPMapping],
) -> Result<f64, SimError> {
    let t_start = model.model.simulation.t_start;
    let n_substeps = trajectory.substeps.len();
    let n_tr = model.model.transitions.len();
    let mut log_p = 0.0;

    // Initial state density: log p(x₀ | θ) for IVP-controlled compartments.
    // S₀ ~ Binom(N₀, s0) → log Binom(S₀; N₀, s0) constrains s0.
    if !ivp_mappings.is_empty() {
        let total_pop: i64 = trajectory.initial_counts.iter().sum();
        for ivp in ivp_mappings {
            let count = trajectory.initial_counts[ivp.compartment_idx] as u64;
            let frac = params[ivp.model_param_idx].clamp(1e-10, 1.0 - 1e-10);
            log_p += binom_logpmf(count, total_pop as u64, frac);
        }
    }

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

        // Gamma multiplier density (for sigma_se estimation)
        if !rec.gammas.is_empty() {
            let n_int = model.int_local_to_global.len();
            let mut int_s = IntState::new(n_int);
            int_s.counts.copy_from_slice(counts_before);
            let real_s = RealState::new(model.real_local_to_global.len());
            let ctx = EvalCtx {
                model, int_s: &int_s, real_s: &real_s, params, t, projected: None,
            };
            let mut gamma_idx = 0;
            for &(_, ref group) in &model.source_groups {
                for &tr_idx in group {
                    if let ir::transition::DrawMethod::Overdispersed(ref expr)
                        = model.model.transitions[tr_idx].draw_method
                    {
                        if gamma_idx < rec.gammas.len() {
                            let g = rec.gammas[gamma_idx];
                            let sigma_sq = eval_expr(expr, &ctx).unwrap_or(1.0);
                            if g > 0.0 && sigma_sq > 0.0 {
                                let shape = dt / sigma_sq;
                                let scale = sigma_sq / dt;
                                log_p += crate::inference::obs_loglik::log_gamma_density(g, shape, scale);
                            }
                            gamma_idx += 1;
                        }
                        break;
                    }
                }
            }
        }

        // Accumulate flows
        for (i, &f) in rec.flows.iter().enumerate() {
            cum_flows[i] += f;
        }

        // Observation density at observation times
        if let Some(&obs_idx) = obs_at_substep.get(&s) {
            let projected: f64 = flow_indices.iter()
                .map(|&i| cum_flows[i] as f64).sum();
            log_p += obs_loglik_fn(projected, observations[obs_idx].value);
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
    obs_loglik_fn: &ObsLoglikFn,
    flow_indices: &[usize],
    ivp_mappings: &[IVPMapping],
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

    // Initialize particles with stochastic initial states for IVP compartments.
    // Each free particle draws S₀ ~ Binom(N₀, s0) independently, giving the
    // CSMC diverse initial states to select among. This is what enables
    // posterior sampling of IVP parameters like s0.
    let (init_int, _) = model.initial_state(params)?;
    let total_pop = init_int.counts.iter().sum::<i64>();

    // Per-particle RNGs (needed early for stochastic init)
    let mut rngs: Vec<StatefulRng> = (0..n_particles)
        .map(|i| StatefulRng::new(seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95)))
        .collect();

    let mut counts: Vec<Vec<i64>> = (0..n_particles)
        .map(|j| {
            if j == j_ref {
                reference.initial_counts.clone()
            } else {
                let mut c = init_int.counts.clone();
                // Draw stochastic initial state for IVP compartments
                for ivp in ivp_mappings {
                    let frac = params[ivp.model_param_idx].clamp(1e-10, 1.0 - 1e-10);
                    c[ivp.compartment_idx] = rngs[j].binomial(total_pop as u64, frac) as i64;
                }
                // Reapply balance constraint if present
                if let Some(ref bal) = model.balance {
                    let bal_val: i64 = total_pop - c.iter().enumerate()
                        .filter(|&(i, _)| i != bal.local_int_idx)
                        .map(|(_, &v)| v)
                        .sum::<i64>();
                    c[bal.local_int_idx] = bal_val;
                }
                c
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

    // Store initial counts per particle BEFORE propagation (for traceback).
    // Needed because free particles have stochastic initial states (Binom draw)
    // that differ from the deterministic initial_state(params).
    let initial_counts_per_particle: Vec<Vec<i64>> = counts.iter()
        .map(|c| c.clone())
        .collect();

    // History for traceback
    let mut history_counts: Vec<Vec<Vec<i64>>> = Vec::with_capacity(n_substeps);
    let mut history_flows: Vec<Vec<Vec<u64>>> = Vec::with_capacity(n_substeps);
    let mut history_gammas: Vec<Vec<Vec<f64>>> = Vec::with_capacity(n_substeps);
    let mut ancestors: Vec<Vec<usize>> = Vec::with_capacity(n_substeps);

    // Weights (log-space)
    let mut log_weights = vec![0.0f64; n_particles];

    // Resampling RNG (particle RNGs already created above for stochastic init)
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
                log_weights[j] = obs_loglik_fn(projected, obs_value);
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

    // Initial counts: use the stored per-particle initial state (which
    // includes stochastic Binom draws for IVP compartments).
    let initial_counts = initial_counts_per_particle[particle].clone();

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

/// Derivative of the transform θ(z) w.r.t. z.
/// dθ/dz for chain rule: d(f(θ))/dz = d(f)/dθ × dθ/dz.
fn transform_deriv(param: &IF2Param, z: f64) -> f64 {
    match &param.transform {
        Transform::Log { .. } => z.exp(), // θ = exp(z), dθ/dz = exp(z)
        Transform::Logit { lo, hi } => {
            let p = 1.0 / (1.0 + (-z).exp());
            (hi - lo) * p * (1.0 - p) // θ = lo + (hi-lo)*σ(z), dθ/dz = (hi-lo)*σ(z)*(1-σ(z))
        }
        Transform::None => 1.0,
    }
}

/// Prior log-density AND its gradient on the z (unconstrained) scale.
/// Each variant handles its own chain rule — the caller just sums.
fn prior_log_density_and_grad_z(
    prior: &Prior, param: &IF2Param, theta: f64, z: f64,
) -> (f64, f64) {
    match prior {
        Prior::Flat => (0.0, 0.0),
        Prior::Normal { mean, sd } => {
            let lp = -0.5 * ((theta - mean) / sd).powi(2) - sd.ln();
            let dlp_dtheta = -(theta - mean) / (sd * sd);
            let dlp_dz = dlp_dtheta * transform_deriv(param, z);
            (lp, dlp_dz)
        }
        Prior::TransformedNormal { mean, sd } => {
            let lp = -0.5 * ((z - mean) / sd).powi(2) - sd.ln();
            let dlp_dz = -(z - mean) / (sd * sd);
            (lp, dlp_dz)
        }
        Prior::Beta { alpha, beta } => {
            // log Beta(θ; a, b) = (a-1)ln(θ) + (b-1)ln(1-θ) - lnB(a,b)
            // d/dθ = (a-1)/θ - (b-1)/(1-θ)
            // d/dz = d/dθ × dθ/dz
            if theta <= 0.0 || theta >= 1.0 { return (f64::NEG_INFINITY, 0.0); }
            use crate::inference::obs_loglik::lgamma;
            let lp = (alpha - 1.0) * theta.ln() + (beta - 1.0) * (1.0 - theta).ln()
                - (lgamma(*alpha) + lgamma(*beta) - lgamma(alpha + beta));
            let dlp_dtheta = (alpha - 1.0) / theta - (beta - 1.0) / (1.0 - theta);
            let dlp_dz = dlp_dtheta * transform_deriv(param, z);
            (lp, dlp_dz)
        }
    }
}

/// Derivative of log|Jacobian| w.r.t. z.
/// d/dz log|dθ/dz|.
fn jacobian_grad(param: &IF2Param, z: f64) -> f64 {
    match &param.transform {
        Transform::Log { .. } => 1.0, // d/dz z = 1
        Transform::Logit { .. } => {
            let p = 1.0 / (1.0 + (-z).exp());
            1.0 - 2.0 * p // d/dz log(p*(1-p)) = (1 - 2p)
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
/// 1. θ | X, y — MH updates using exact complete-data log-likelihood
/// 2. X | θ, y — CSMC-AS to refresh the latent trajectory
///
/// Step 1 evaluates the exact log p(y,X|θ) — no PF, no estimation noise.
/// The surface is sharp (46K transition terms), so proposals are small, but
/// the CSMC-AS in Step 2 shifts the mode by renewing the trajectory X. The
/// Gibbs alternation provides mixing: small θ steps track the shifting mode.
pub fn run_pgas(
    model: &CompiledModel,
    if2_params: &[IF2Param],
    priors: &[Prior],
    base_params: &[f64],
    config: &PGASConfig,
    observations: &[Observation],
    obs_loglik_fn: &ObsLoglikFn,
    flow_indices: &[usize],
    seed: u64,
    on_sweep: Option<&dyn Fn(usize, &PGASSweep, &PGASTrajectory)>,
    resume_from: Option<ChainResumeState>,
    config_hash: String,
) -> Result<PGASResult, SimError> {
    let d = if2_params.len();
    assert_eq!(d, priors.len(), "priors must match if2_params length");

    let mut rng = StatefulRng::new(seed);
    let mut current_params = base_params.to_vec();
    let t_end = observations.last().map_or(
        model.model.simulation.t_start,
        |o| o.time,
    );

    // Resume or fresh start
    let start_sweep;
    let mut trajectory;
    let mut current_transformed: Vec<f64>;

    // Extract resume adaptation state (consumed separately from trajectory/params)
    let resume_nuts = resume_from.as_ref().map(|s| (
        s.mass_matrix.clone(), s.nuts_step_size,
        s.log_proposal_sd.clone(), s.total_accepted.clone(), s.current_ll,
    ));

    if let Some(state) = resume_from {
        eprintln!("  resuming from sweep {}...", state.completed_sweeps);
        current_params.copy_from_slice(&state.params);
        current_transformed = state.transformed;
        trajectory = state.trajectory;
        start_sweep = state.completed_sweeps;
    } else {
        eprintln!("  initializing reference trajectory...");
        trajectory = simulate_reference(
            model, &current_params, t_end, config.dt, &mut rng,
        )?;
        eprintln!("  reference: {} substeps, initial S={}",
            trajectory.substeps.len(),
            trajectory.initial_counts.get(0).copied().unwrap_or(0));
        current_transformed = if2_params.iter()
            .map(|p| p.to_transformed(current_params[p.index]))
            .collect();
        start_sweep = 0;
    }

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

    let log_proposal_sd: Vec<f64> = if2_params.iter()
        .map(|p| {
            let lo = p.to_transformed(p.lower.max(1e-10));
            let hi = p.to_transformed(p.upper.min(1e10));
            let range = (hi - lo).abs();
            // 10% of the transformed-scale range: broad enough to explore,
            // Robbins-Monro will shrink to the right scale within ~200 sweeps
            (range / 10.0).max(0.01).ln()
        })
        .collect();

    // Detect IVP parameters: parameters that affect initial_state but not
    // propensities. These get stochastic initial states in CSMC and a
    // Binomial density term in the complete-data LL, enabling posterior
    // sampling through the Gibbs structure.
    let ivp_mappings: Vec<IVPMapping> = {
        let (init_base, _) = model.initial_state(&current_params)?;
        let mut mappings = Vec::new();
        for (i, spec) in if2_params.iter().enumerate() {
            let mut perturbed = current_params.clone();
            let delta = (spec.upper - spec.lower).min(1.0) * 0.01;
            perturbed[spec.index] = (perturbed[spec.index] + delta).min(spec.upper);
            let (init_pert, _) = model.initial_state(&perturbed)?;
            // Find which compartment changed
            for (c, (&base_c, &pert_c)) in init_base.counts.iter()
                .zip(init_pert.counts.iter()).enumerate()
            {
                // Skip balance compartment (it changes as a consequence)
                if model.balance.as_ref().map_or(false, |b| b.local_int_idx == c) {
                    continue;
                }
                if base_c != pert_c {
                    eprintln!("  {} detected as IVP → compartment {} \
                              (stochastic init, Binom density in LL)", spec.name, c);
                    mappings.push(IVPMapping {
                        param_idx: i,
                        model_param_idx: spec.index,
                        compartment_idx: c,
                    });
                    break;
                }
            }
        }
        mappings
    };

    // Initial complete-data log-likelihood (now includes initial state density)
    let mut current_ll = complete_data_loglik(
        model, &trajectory, &current_params, observations,
        config.dt, obs_loglik_fn, flow_indices, &ivp_mappings,
    )?;
    eprintln!("  initial complete-data ll: {:.1}", current_ll);

    // Check if gradients are available (compiler emitted rate_grad)
    let has_gradients = config.use_nuts && model.model.transitions.iter()
        .any(|t| !t.rate_grad.is_empty());
    if has_gradients {
        eprintln!("  NUTS enabled (gradient expressions found in IR)");
    }

    // NUTS state — restored from resume or initialized fresh
    let (mut nuts_mass, mut nuts_step_size, log_proposal_sd_restored,
         mut total_accepted, current_ll_restored) = if let Some((mass, ss, lpsd, ta, ll)) = resume_nuts {
        (mass, ss, lpsd, ta, Some(ll))
    } else {
        (super::nuts::MassMatrix::identity(d), 0.1, log_proposal_sd, vec![0usize; d], None)
    };
    let mut log_proposal_sd = log_proposal_sd_restored;
    let mut nuts_dual_avg = super::nuts::DualAveraging::new(nuts_step_size, 0.80);

    // Collect z-scale samples during burn-in for mass matrix adaptation.
    let mut welford_n = 0.0_f64;
    let mut welford_mean = vec![0.0; d];
    let mut welford_m2 = vec![0.0; d];
    let mut welford_cov = vec![0.0; d * d];

    let mut sweeps = Vec::new();

    // Override current_ll if we have a resumed value
    if let Some(ll) = current_ll_restored {
        current_ll = ll;
    }

    for sweep in start_sweep..config.n_sweeps {
        let mut accepted = vec![false; d];

        // Current proposal SDs (from adaptive log scale, MH only)
        let proposal_sd: Vec<f64> = log_proposal_sd.iter()
            .map(|&ls| ls.exp())
            .collect();

        // ── Step 1: Update θ | X, y ──
        // The complete-data log-likelihood is exact (no PF noise).
        // Two strategies: NUTS (gradient-based, joint proposal) or
        // MH-within-Gibbs (1D random walk, one-at-a-time).
        if has_gradients {
            // NUTS: propose all parameters jointly using gradients.
            // The closure evaluates the FULL target on z scale. Each component
            // (LL, prior, Jacobian) returns its own (value, d/dz) pair — no
            // cross-scale gradient mixing, no manual chain rule at the call site.
            let param_names: Vec<String> = if2_params.iter().map(|p| p.name.clone()).collect();
            let param_model_indices: Vec<usize> = if2_params.iter().map(|p| p.index).collect();

            let log_prob_and_grad = |z: &[f64]| -> (f64, Vec<f64>) {
                // Transform z → θ (natural scale)
                let mut params = current_params.clone();
                for (i, spec) in if2_params.iter().enumerate() {
                    params[spec.index] = spec.from_transformed(z[i]);
                }

                // LL + gradient in NATURAL scale (d/dθ)
                let (ll, ll_grad_theta) = match super::pgas_grad::complete_data_loglik_grad(
                    model, &trajectory, &params, observations,
                    config.dt, obs_loglik_fn, flow_indices, &ivp_mappings,
                    &param_names, &param_model_indices,
                ) {
                    Ok(r) => r,
                    Err(_) => return (f64::NEG_INFINITY, vec![0.0; d]),
                };

                // Assemble full target on z scale: each component produces (value, d/dz)
                let mut log_p = ll;
                let mut grad_z = vec![0.0; d];

                for i in 0..d {
                    let theta = params[if2_params[i].index];
                    let dtheta_dz = transform_deriv(&if2_params[i], z[i]);

                    // LL: chain rule θ → z
                    grad_z[i] += ll_grad_theta[i] * dtheta_dz;

                    // Prior: (value, d/dz) — each variant handles its own scale
                    let (prior_val, prior_grad_z) = prior_log_density_and_grad_z(
                        &priors[i], &if2_params[i], theta, z[i],
                    );
                    log_p += prior_val;
                    grad_z[i] += prior_grad_z;

                    // Jacobian: (value, d/dz) — already on z scale
                    log_p += log_jacobian(&if2_params[i], z[i]);
                    grad_z[i] += jacobian_grad(&if2_params[i], z[i]);
                }

                (log_p, grad_z)
            };

            let (init_log_p, init_grad) = log_prob_and_grad(&current_transformed);

            let nuts_config = super::nuts::NUTSConfig {
                max_tree_depth: 10,
                step_size: nuts_step_size,
                mass_matrix: nuts_mass.clone(),
            };

            let result = super::nuts::nuts_step(
                &current_transformed, init_log_p, &init_grad,
                &nuts_config, &log_prob_and_grad, &mut rng,
            );

            if result.accepted {
                current_transformed.copy_from_slice(&result.params);
                for (i, spec) in if2_params.iter().enumerate() {
                    current_params[spec.index] = spec.from_transformed(current_transformed[i]);
                }
                // current_ll is recomputed after CSMC (trajectory changes)
                for a in &mut accepted { *a = true; }
                for t in &mut total_accepted { *t += 1; }
            }

            // Two-phase adaptation (matching Stan's warmup schedule):
            //   Phase 1 (sweeps 0..mass_adapt_end): adapt step size with identity
            //     mass matrix + collect Welford statistics for mass matrix.
            //   Phase 2 (sweeps mass_adapt_end..adapt_end): re-adapt step size
            //     WITH the estimated mass matrix. This is critical — the optimal
            //     step size changes when the mass matrix changes.
            let mass_adapt_end = (adapt_end as f64 * 0.7) as usize;

            if sweep < mass_adapt_end {
                // Phase 1: step size adaptation + covariance collection
                nuts_step_size = nuts_dual_avg.update(result.mean_accept_prob);

                // Online covariance (Welford for diagonal + cross-products for dense)
                welford_n += 1.0;
                let old_mean = welford_mean.clone();
                for i in 0..d {
                    let delta = current_transformed[i] - welford_mean[i];
                    welford_mean[i] += delta / welford_n;
                    let delta2 = current_transformed[i] - welford_mean[i];
                    welford_m2[i] += delta * delta2;
                }
                // Cross-products for full covariance: C_{ij} += (x_i - mean_old_i)(x_j - mean_new_j)
                for i in 0..d {
                    for j in 0..d {
                        welford_cov[i * d + j] += (current_transformed[i] - old_mean[i])
                            * (current_transformed[j] - welford_mean[j]);
                    }
                }
            } else if sweep == mass_adapt_end {
                // Compute mass matrix from burn-in covariance
                if welford_n > 10.0 {
                    if config.dense_mass {
                        // Dense: full empirical covariance → Cholesky
                        let mut cov = vec![0.0; d * d];
                        for i in 0..d {
                            for j in 0..d {
                                cov[i * d + j] = welford_cov[i * d + j] / (welford_n - 1.0);
                            }
                        }
                        nuts_mass = super::nuts::MassMatrix::dense_from_covariance(&cov, d);
                        eprintln!("  dense mass matrix estimated (sweep {}):", sweep);
                        for (i, spec) in if2_params.iter().enumerate() {
                            let sd = (cov[i * d + i]).max(1e-10).sqrt();
                            eprintln!("    {:12} sd={:.6}", spec.name, sd);
                        }
                        // Print correlations
                        eprint!("    correlations:");
                        for i in 0..d {
                            for j in (i+1)..d {
                                let r = cov[i * d + j]
                                    / (cov[i * d + i].max(1e-10).sqrt() * cov[j * d + j].max(1e-10).sqrt());
                                eprint!(" {}-{}={:.2}", &if2_params[i].name[..3.min(if2_params[i].name.len())],
                                    &if2_params[j].name[..3.min(if2_params[j].name.len())], r);
                            }
                        }
                        eprintln!();
                    } else {
                        // Diagonal: variances only
                        let variances: Vec<f64> = (0..d).map(|i|
                            (welford_m2[i] / (welford_n - 1.0)).max(1e-10)
                        ).collect();
                        eprintln!("  diagonal mass matrix estimated (sweep {}):", sweep);
                        for (i, spec) in if2_params.iter().enumerate() {
                            eprintln!("    {:12} sd={:.6}", spec.name, variances[i].sqrt());
                        }
                        nuts_mass = super::nuts::MassMatrix::diagonal(variances);
                    }
                }
                // Reset dual averaging — step size must be re-tuned for new mass matrix
                nuts_step_size = 0.1;
                nuts_dual_avg = super::nuts::DualAveraging::new(nuts_step_size, 0.80);
            } else if sweep < adapt_end {
                // Phase 2: re-adapt step size WITH the mass matrix
                nuts_step_size = nuts_dual_avg.update(result.mean_accept_prob);
            } else if sweep == adapt_end {
                nuts_step_size = nuts_dual_avg.final_step_size();
                eprintln!("  NUTS fully adapted (sweep {}):", sweep);
                eprintln!("    final step_size: {:.6}", nuts_step_size);
            }
        } else {
            // MH-within-Gibbs: one-at-a-time random walk proposals
            for i in 0..d {
                let spec = &if2_params[i];
                let z_old = current_transformed[i];
                let z_new = z_old + proposal_sd[i] * rng.normal();
                let theta_new = spec.from_transformed(z_new);

                let mut proposed_params = current_params.clone();
                proposed_params[spec.index] = theta_new;

                let proposed_ll = complete_data_loglik(
                    model, &trajectory, &proposed_params, observations,
                    config.dt, obs_loglik_fn, flow_indices, &ivp_mappings,
                )?;

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
                    let gamma_rm = ADAPT_C / (1.0 + sweep as f64).sqrt();
                    let acc_indicator = if accepted[i] { 1.0 } else { 0.0 };
                    log_proposal_sd[i] += gamma_rm * (acc_indicator - TARGET_ACCEPTANCE);
                    log_proposal_sd[i] = log_proposal_sd[i].clamp(-20.0, 5.0);
                }
            }
        }

        // ── Step 2: Update X | θ, y via CSMC-AS ──
        // The trajectory update is independent of the PF loglik used in step 1.
        // CSMC-AS conditions on the reference trajectory and produces a new
        // trajectory sample from p(X | θ, y).
        let csmc_seed = seed ^ ((sweep as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15));
        let (new_trajectory, csmc_diag) = csmc_as(
            model, &current_params, observations, &trajectory,
            config.n_particles, config.dt, obs_loglik_fn, flow_indices,
            &ivp_mappings, csmc_seed,
        )?;
        trajectory = new_trajectory;

        // Recompute complete-data LL with the new trajectory
        // (the CSMC changed X, so log p(y,X|θ) changes even at the same θ)
        current_ll = complete_data_loglik(
            model, &trajectory, &current_params, observations,
            config.dt, obs_loglik_fn, flow_indices, &ivp_mappings,
        )?;

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

    let resume_state = ChainResumeState {
        config_hash,
        completed_sweeps: config.n_sweeps,
        params: current_params,
        transformed: current_transformed,
        trajectory: trajectory.clone(),
        mass_matrix: nuts_mass,
        nuts_step_size,
        log_proposal_sd,
        total_accepted: total_accepted.clone(),
        current_ll,
    };

    Ok(PGASResult {
        sweeps,
        final_trajectory: trajectory,
        acceptance_rates,
        resume_state,
    })
}
