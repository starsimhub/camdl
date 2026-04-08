use crate::{
    compiled_model::CompiledModel,
    config::{ChainBinomialConfig, SimConfig},
    rng::StatefulRng,
    error::SimError,
    intervention::{all_intervention_times, apply_interventions_at},
    ode_integrator::rk4_step,
    output::output_times as get_output_times,
    propensity::{eval_propensities, eval_expr, EvalCtx},
    simulate::Simulate,
    state::{FlowVec, IntState, RealState, Snapshot, Trajectory},
};

pub struct ChainBinomialSim;

/// Pre-allocated scratch buffers for `step_one`, eliminating per-call heap
/// allocations. Allocate one per particle (or per thread) and reuse across
/// all time steps.
pub struct StepScratch {
    int_s: IntState,
    real_s: RealState,
    propensities: Vec<f64>,
    draws: Vec<ResolvedDraw>,
    pending_deltas: Vec<(usize, i64)>,
    handled: Vec<bool>,
    probs: Vec<(usize, f64)>,
    /// When set, overrides the next `gamma_multiplier()` call in `step_one`.
    /// Used by correlated pseudo-marginal MCMC to inject pre-drawn Gamma
    /// noise for correlation across MCMC steps.
    pub gamma_override: Option<f64>,
    /// When non-empty, provides standard normal z-values for the total-exit
    /// binomial draw in each source group. `step_one` transforms z to a
    /// binomial count via normal approximation (large np) or inverse CDF
    /// (small np). Consumed in source-group order.
    /// Used by CPM-MCMC for correlated binomial draws.
    pub binomial_z_values: Vec<f64>,
    /// Current index into binomial_z_values. Incremented as z-values are consumed.
    pub binomial_z_idx: usize,
    /// Gamma multipliers actually used during step_one, in source-group order.
    /// Populated by step_one for each overdispersed source group encountered.
    /// Used by PGAS to record the gamma drawn at each substep for transition
    /// density evaluation. Cleared before each step_one call by the caller.
    pub gamma_used: Vec<f64>,
}

/// How event counts are drawn — resolved from the IR at step start.
enum ResolvedDraw { Poisson, Deterministic, Overdispersed(f64) }

impl StepScratch {
    /// Create scratch buffers sized for `model`.
    pub fn new(model: &CompiledModel) -> Self {
        let n_int = model.int_local_to_global.len();
        let n_real = model.real_local_to_global.len();
        let n_tr = model.model.transitions.len();
        StepScratch {
            int_s: IntState::new(n_int),
            real_s: RealState::new(n_real),
            propensities: Vec::with_capacity(n_tr),
            draws: Vec::with_capacity(n_tr),
            pending_deltas: Vec::with_capacity(n_tr * 2),
            handled: vec![false; n_tr],
            gamma_override: None,
            binomial_z_values: Vec::new(),
            binomial_z_idx: 0,
            gamma_used: Vec::new(),
            probs: Vec::with_capacity(n_tr),
        }
    }
}

/// Chain-binomial process simulator for inference (particle filter, IF2).
/// Wraps a CompiledModel reference and implements ProcessSimulator.
pub struct ChainBinomialProcess<'a> {
    pub model: &'a CompiledModel,
}

impl<'a> crate::inference::ProcessSimulator for ChainBinomialProcess<'a> {
    fn step(
        &self,
        state: &mut crate::inference::ParticleState,
        params: &[f64],
        t: f64,
        dt: f64,
        rng: &mut crate::rng::StatefulRng,
    ) -> Result<(), crate::error::SimError> {
        // NOTE: this trait method can't use scratch buffers (signature is fixed).
        // Hot inference paths call step_one directly with scratch instead.
        let mut scratch = StepScratch::new(self.model);
        step_one(self.model, &mut state.counts, &mut state.flow_accumulators,
                 params, t, dt, rng, &mut scratch)
    }
}

impl Simulate for ChainBinomialSim {
    fn run(
        &self,
        model: &CompiledModel,
        params: &[f64],
        seed: u64,
        config: &SimConfig,
    ) -> Result<Trajectory, SimError> {
        let cfg = match config {
            SimConfig::ChainBinomial(c) => c,
            _ => return Err(SimError::ConfigMismatch {
                expected: "ChainBinomial",
                got: config.variant_name(),
            }),
        };
        run_chain_binomial(model, params, seed, cfg)
    }

    fn capabilities(&self) -> crate::Capabilities {
        crate::Capabilities::OVERDISPERSION | crate::Capabilities::REAL_COMPARTMENTS
    }

    fn name(&self) -> &'static str { "chain_binomial" }
}

fn run_chain_binomial(
    model: &CompiledModel,
    params: &[f64],
    seed: u64,
    cfg: &ChainBinomialConfig,
) -> Result<Trajectory, SimError> {
    let (mut int_s, mut real_s) = model.initial_state(params)?;
    let n_transitions = model.model.transitions.len();
    let n_real = real_s.values.len();

    let mut rng = StatefulRng::new(seed);
    let mut propensities = Vec::with_capacity(n_transitions);

    let output_times = get_output_times(&model.model.output.times);
    let mut output_idx = 0;
    let iv_times = all_intervention_times(model);
    let mut iv_idx = 0;

    let mut traj = Trajectory::new();
    let mut current_flows = FlowVec::new(n_transitions);
    let mut t = cfg.t_start;

    if output_idx < output_times.len() && output_times[output_idx] <= t + 1e-12 {
        traj.push(Snapshot {
            t, int_state: int_s.clone(), real_state: real_s.clone(), flows: current_flows.clone(),
        });
        current_flows.reset();
        output_idx += 1;
    }

    while t < cfg.t_end {
        let dt = cfg.dt.min(cfg.t_end - t);
        if dt <= 1e-15 { break; }

        // Euler step for real compartments (before binomial draws)
        if n_real > 0 {
            rk4_step(model, &int_s, &mut real_s, params, t, dt)?;
            real_s.clamp_nonneg();
        }

        // Evaluate propensities
        eval_propensities(model, &int_s, &real_s, params, t, &mut propensities)?;

        // Pre-evaluate draw methods (resolves overdispersion σ² expressions
        // from start-of-step state before any mutations).
        enum ResolvedDraw { Poisson, Deterministic, Overdispersed(f64) }
        let draws: Vec<ResolvedDraw> = {
            let ctx = EvalCtx { model, int_s: &int_s, real_s: &real_s, params, t , projected: None };
            model.model.transitions.iter()
                .map(|tr| match &tr.draw_method {
                    ir::transition::DrawMethod::Poisson => Ok(ResolvedDraw::Poisson),
                    ir::transition::DrawMethod::Deterministic => Ok(ResolvedDraw::Deterministic),
                    ir::transition::DrawMethod::Overdispersed(expr) =>
                        eval_expr(expr, &ctx).map(ResolvedDraw::Overdispersed),
                })
                .collect::<Result<_, _>>()?
        };
        // ── CRITICAL: deferred state update ────────────────────────────────
        //
        // All draws must use the START-OF-STEP state. Updates are buffered
        // and applied simultaneously AFTER all draws complete.
        //
        // Why this matters: in an SEIR model, if infection (S→E) draws are
        // applied immediately, the E compartment grows mid-step. When the
        // progression (E→I) group is processed next, it reads the inflated E
        // and draws more progressions than should occur in one dt. This
        // creates a "pipeline acceleration" where individuals flow through
        // S→E→I→R within a single timestep — physically impossible for dt
        // shorter than the latent + infectious period.
        //
        // pomp's reulermultinom draws all multinomials from a frozen state
        // snapshot, then applies all deltas at once. We do the same by
        // accumulating (compartment_index, delta) pairs in a buffer.
        //
        // The propensities (line 82) are already evaluated from start-of-step
        // state, so those are correct. The n_src reads (line below) also need
        // the frozen state — we read from int_s which is NOT mutated during
        // the draw loop.
        // ──────────────────────────────────────────────────────────────────

        let mut pending_deltas: Vec<(usize, i64)> = Vec::new();
        let mut handled = vec![false; n_transitions];

        // Euler-multinomial draws for transitions sharing a source compartment.
        // Matches pomp's reulermultinom:
        //   1. Compute effective per-capita rates (with gamma noise if overdispersed)
        //   2. Draw total exits from Binom(n_src, 1-exp(-sum_rates * dt))
        //   3. Split total exits proportional to rates
        for &(src_local, ref group) in &model.source_groups {
            let n_src = int_s.counts[src_local].max(0);
            if n_src == 0 {
                for &tr_idx in group { handled[tr_idx] = true; }
                continue;
            }

            let mut rates: Vec<(usize, f64)> = Vec::with_capacity(group.len());
            let mut total_rate = 0.0_f64;
            for &tr_idx in group {
                let rate = propensities[tr_idx];
                if rate <= 0.0 {
                    handled[tr_idx] = true;
                    continue;
                }
                let per_capita = rate / n_src as f64;
                match &draws[tr_idx] {
                    ResolvedDraw::Deterministic => {
                        handled[tr_idx] = true;
                        continue;
                    }
                    _ => {}
                }
                let effective = match &draws[tr_idx] {
                    ResolvedDraw::Overdispersed(sigma_sq) =>
                        per_capita * rng.gamma_multiplier(*sigma_sq, dt),
                    _ => per_capita,
                };
                total_rate += effective;
                rates.push((tr_idx, effective));
            }

            if total_rate <= 0.0 || rates.is_empty() { continue; }

            // Step 1: draw total exits
            let p_total = (1.0 - (-total_rate * dt).exp()).clamp(0.0, 1.0);
            let mut n_events = rng.binomial(n_src as u64, p_total);

            // Step 2: split proportional to rates
            let n_competing = rates.len();
            let mut rate_remaining = total_rate;
            for (k, &(tr_idx, eff_rate)) in rates.iter().enumerate() {
                let count = if k == n_competing - 1 {
                    n_events
                } else if n_events > 0 && rate_remaining > 0.0 {
                    let p_split = (eff_rate / rate_remaining).clamp(0.0, 1.0);
                    let c = rng.binomial(n_events, p_split);
                    n_events -= c;
                    rate_remaining -= eff_rate;
                    c
                } else {
                    0
                };
                for &(local, delta) in &model.transition_stoich[tr_idx] {
                    pending_deltas.push((local, delta * count as i64));
                }
                current_flows.add(tr_idx, count);
                handled[tr_idx] = true;
            }
        }

        // Inflows and ungrouped transitions (no source compartment)
        for (i, &rate) in propensities.iter().enumerate() {
            if handled[i] || rate <= 0.0 { continue; }
            let mean = rate * dt;
            let count = match &draws[i] {
                ResolvedDraw::Poisson => rng.poisson(mean),
                ResolvedDraw::Deterministic => mean.round() as u64,
                ResolvedDraw::Overdispersed(sigma_sq) => rng.neg_binomial(mean, *sigma_sq, dt),
            };
            for &(local, delta) in &model.transition_stoich[i] {
                pending_deltas.push((local, delta * count as i64));
            }
            current_flows.add(i, count);
        }

        // Inject always_active event deltas (evaluated from snapshot)
        crate::intervention::inject_event_deltas(
            model, &int_s, &real_s, params, t, dt, &mut pending_deltas,
        )?;

        // Apply all deltas simultaneously — transitions + events atomically.
        for (local, delta) in pending_deltas {
            int_s.counts[local] += delta;
        }

        // Clamp non-negative (skip balance target)
        if let Some(ref bal) = model.balance {
            for (i, c) in int_s.counts.iter_mut().enumerate() {
                if i == bal.local_int_idx { continue; }
                if *c < 0 { *c = 0; }
            }
        } else {
            let clamped = int_s.clamp_nonneg();
            if clamped > 0 {
                log::warn!("chain-binomial: clamped {} negative compartments at t={}", clamped, t);
            }
        }

        t += dt;

        // Interventions
        if iv_times.get(iv_idx).copied().is_some_and(|iv| (iv - t).abs() < cfg.dt * 0.5) {
            apply_interventions_at(t, model, &mut int_s, &mut real_s, params, cfg.dt * 0.5)?;
            while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + cfg.dt * 0.5 { iv_idx += 1; }
        }

        // Apply balance constraint
        if let Some(ref bal) = model.balance {
            let ctx = EvalCtx {
                model, int_s: &int_s, real_s: &real_s, params, t, projected: None,
            };
            let val = eval_expr(&bal.expr, &ctx)?;
            int_s.counts[bal.local_int_idx] = val.round() as i64;
        }

        // Output
        while output_idx < output_times.len() && output_times[output_idx] <= t + 1e-12 {
            traj.push(Snapshot {
                t: output_times[output_idx],
                int_state: int_s.clone(),
                real_state: real_s.clone(),
                flows: current_flows.clone(),
            });
            current_flows.reset();
            output_idx += 1;
        }
    }

    while output_idx < output_times.len() {
        traj.push(Snapshot {
            t: output_times[output_idx],
            int_state: int_s.clone(),
            real_state: real_s.clone(),
            flows: current_flows.clone(),
        });
        current_flows.reset();
        output_idx += 1;
    }

    Ok(traj)
}

/// Check if step tracing is enabled via CAMDL_TRACE_STEPS=1.
pub fn trace_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("CAMDL_TRACE_STEPS").map_or(false, |v| v == "1"))
}

/// Advance integer compartment state by one chain-binomial step.
///
/// This is the core Euler-multinomial step, extracted for use by the
/// particle filter and other inference algorithms. It operates on raw
/// slices to avoid coupling to IntState/FlowVec/ParticleState.
///
/// `scratch` holds pre-allocated buffers to avoid heap allocation per call.
/// Create one `StepScratch` per particle and reuse across all time steps.
pub fn step_one(
    model: &CompiledModel,
    counts: &mut [i64],
    flows: &mut [u64],
    params: &[f64],
    t: f64,
    dt: f64,
    rng: &mut StatefulRng,
    scratch: &mut StepScratch,
) -> Result<(), SimError> {
    // Copy current counts into scratch IntState for propensity evaluation.
    // This is a memcpy into pre-allocated memory, not a heap allocation.
    scratch.int_s.counts.copy_from_slice(counts);

    eval_propensities(model, &scratch.int_s, &scratch.real_s, params, t,
                      &mut scratch.propensities)?;

    // Pre-evaluate draw methods from start-of-step state
    scratch.draws.clear();
    {
        let ctx = EvalCtx { model, int_s: &scratch.int_s, real_s: &scratch.real_s, params, t , projected: None };
        for tr in &model.model.transitions {
            scratch.draws.push(match &tr.draw_method {
                ir::transition::DrawMethod::Poisson => ResolvedDraw::Poisson,
                ir::transition::DrawMethod::Deterministic => ResolvedDraw::Deterministic,
                ir::transition::DrawMethod::Overdispersed(expr) =>
                    ResolvedDraw::Overdispersed(eval_expr(expr, &ctx)?),
            });
        }
    }

    // ── Deferred state update (see run_chain_binomial for full explanation) ──
    scratch.pending_deltas.clear();
    scratch.handled.fill(false);

    // Euler-multinomial draws for transitions sharing a source compartment.
    //
    // Matches pomp's reulermultinom exactly:
    //   1. Compute effective per-capita rates (with gamma noise if overdispersed)
    //   2. Draw TOTAL exits from Binom(n_src, 1-exp(-sum_rates * dt))
    //   3. Split total exits across transitions proportional to their rates
    //
    // This is NOT equivalent to sequential conditional binomials with
    // individual probabilities p_i = 1-exp(-r_i*dt), because
    // Σ(1-exp(-r_i*dt)) > 1-exp(-Σr_i*dt) (subadditivity of 1-exp).
    // The old algorithm systematically over-counted total exits, causing
    // particle trajectories to drift and ESS to degrade over long runs.
    for &(src_local, ref group) in &model.source_groups {
        let n_src = counts[src_local].max(0);
        if n_src == 0 {
            for &tr_idx in group { scratch.handled[tr_idx] = true; }
            continue;
        }

        // Step 1: compute effective per-capita rates
        scratch.probs.clear(); // reuse as (tr_idx, effective_rate) pairs
        let mut total_rate = 0.0_f64;
        for &tr_idx in group {
            let rate = scratch.propensities[tr_idx];
            // Epsilon threshold: rates below this are treated as zero.
            // Must match log_transition_density_substep in pgas.rs to avoid
            // the floating-point mismatch where step_one sees 1e-300 (positive,
            // enters split, occasionally draws 1 event) but the density sees
            // 0.0 exactly (skipped, flow=1 → -inf). See spatial-pgas-inf-bug.md.
            if rate <= 1e-15 { scratch.handled[tr_idx] = true; continue; }
            let per_capita = rate / n_src as f64;
            match &scratch.draws[tr_idx] {
                ResolvedDraw::Deterministic => { scratch.handled[tr_idx] = true; continue; }
                _ => {}
            }
            let effective = match &scratch.draws[tr_idx] {
                ResolvedDraw::Overdispersed(sigma_sq) => {
                    let g = scratch.gamma_override.take()
                        .unwrap_or_else(|| rng.gamma_multiplier(*sigma_sq, dt));
                    scratch.gamma_used.push(g);
                    per_capita * g
                }
                _ => per_capita,
            };
            total_rate += effective;
            scratch.probs.push((tr_idx, effective));
        }

        if total_rate <= 0.0 || scratch.probs.is_empty() { continue; }

        // Step 2: draw total exits (pomp's first rbinom)
        let p_total = (1.0 - (-total_rate * dt).exp()).clamp(0.0, 1.0);
        let mut n_events = if scratch.binomial_z_idx < scratch.binomial_z_values.len() {
            // CPM: use pre-drawn z-value for correlated binomial
            let z = scratch.binomial_z_values[scratch.binomial_z_idx];
            scratch.binomial_z_idx += 1;
            let n = n_src as u64;
            let np = n as f64 * p_total;
            let nq = n as f64 * (1.0 - p_total);
            if np > 20.0 && nq > 20.0 {
                let sd = (np * (1.0 - p_total)).sqrt();
                (np + sd * z).round().clamp(0.0, n as f64) as u64
            } else if np > 0.0 {
                // Small np: inverse CDF via Phi(z)
                let u = crate::inference::correlated_pf::phi(z).clamp(1e-15, 1.0 - 1e-15);
                crate::inference::correlated_pf::binomial_quantile(n, p_total, u)
            } else {
                0
            }
        } else {
            rng.binomial(n_src as u64, p_total)
        };

        // Step 3: split total exits proportional to rates (pomp's inner loop)
        let n_competing = scratch.probs.len();
        let mut rate_remaining = total_rate;
        for (k, &(tr_idx, eff_rate)) in scratch.probs.iter().enumerate() {
            let count = if k == n_competing - 1 {
                // Last category gets the remainder (avoids rounding drift)
                n_events
            } else if n_events > 0 && rate_remaining > 0.0 {
                // pomp: if (rate[k] > p) p = rate[k]; trans[k] = rbinom(size, rate[k]/p)
                let p_split = (eff_rate / rate_remaining).clamp(0.0, 1.0);
                let c = rng.binomial(n_events, p_split);
                n_events -= c;
                rate_remaining -= eff_rate;
                c
            } else {
                0
            };
            for &(local, delta) in &model.transition_stoich[tr_idx] {
                scratch.pending_deltas.push((local, delta * count as i64));
            }
            flows[tr_idx] += count;
            scratch.handled[tr_idx] = true;
        }
    }

    // Inflows and ungrouped transitions
    for (i, &rate) in scratch.propensities.iter().enumerate() {
        if scratch.handled[i] || rate <= 1e-15 { continue; }
        let mean = rate * dt;
        let count = match &scratch.draws[i] {
            ResolvedDraw::Poisson => rng.poisson(mean),
            ResolvedDraw::Deterministic => mean.round() as u64,
            ResolvedDraw::Overdispersed(sigma_sq) => rng.neg_binomial(mean, *sigma_sq, dt),
        };
        for &(local, delta) in &model.transition_stoich[i] {
            scratch.pending_deltas.push((local, delta * count as i64));
        }
        flows[i] += count;
    }

    // Inject always_active event deltas (evaluated from snapshot, applied atomically)
    crate::intervention::inject_event_deltas(
        model, &scratch.int_s, &scratch.real_s, params, t, dt,
        &mut scratch.pending_deltas,
    )?;

    // Apply all deltas atomically (transitions + events)
    for &(local, delta) in &scratch.pending_deltas {
        counts[local] += delta;
    }

    // Per-substep trace (CAMDL_TRACE_STEPS=1)
    if trace_enabled() {
        // Header on first call
        use std::sync::OnceLock;
        static HEADER: OnceLock<bool> = OnceLock::new();
        if *HEADER.get_or_init(|| {
            eprint!("t");
            for c in &model.model.compartments { eprint!("\t{}", c.name); }
            for tr in &model.model.transitions { eprint!("\tflow_{}", tr.name); }
            eprint!("\ttotal_pop");
            for (_i, tr) in model.model.transitions.iter().enumerate() {
                eprint!("\trate_{}", tr.name);
            }
            eprintln!();
            true
        }) {}
        eprint!("{:.1}", t + dt);
        for &c in counts.iter() { eprint!("\t{}", c); }
        for &f in flows.iter() { eprint!("\t{}", f); }
        let total: i64 = counts.iter().sum();
        eprint!("\t{}", total);
        for &p in scratch.propensities.iter() { eprint!("\t{:.4}", p); }
        eprintln!();
    }

    // Clamp non-negative (skip balance target — it may legitimately go negative
    // when the constraint expression yields a negative value, signaling a broken
    // model that the particle filter should penalize via bad trajectories).
    if let Some(ref bal) = model.balance {
        for (i, c) in counts.iter_mut().enumerate() {
            if i == bal.local_int_idx { continue; }
            if *c < 0 { *c = 0; }
        }
    } else {
        for c in counts.iter_mut() {
            if *c < 0 { *c = 0; }
        }
    }

    // Apply interventions that fire at t + dt (within tolerance dt/2).
    if !model.model.interventions.is_empty() {
        let t_end = t + dt;
        scratch.int_s.counts.copy_from_slice(counts);
        let fired = apply_interventions_at(
            t_end, model, &mut scratch.int_s, &mut scratch.real_s, params, dt * 0.5,
        )?;
        if fired {
            counts.copy_from_slice(&scratch.int_s.counts);
        }
    }

    // Apply balance constraint: overwrite target compartment so the population
    // budget holds. All other compartments are finalized at this point.
    if let Some(ref bal) = model.balance {
        scratch.int_s.counts.copy_from_slice(counts);
        let t_end = t + dt;
        let ctx = EvalCtx {
            model, int_s: &scratch.int_s, real_s: &scratch.real_s,
            params, t: t_end, projected: None,
        };
        let val = eval_expr(&bal.expr, &ctx)?;
        let bal_count = val.round() as i64;
        if bal_count < 0 {
            log::warn!("balance compartment went negative ({}) at t={:.1} — \
                        model may be inconsistent at these parameters", bal_count, t_end);
        }
        counts[bal.local_int_idx] = bal_count;
    }

    Ok(())
}
