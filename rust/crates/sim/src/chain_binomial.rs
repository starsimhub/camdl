use crate::{
    compiled_model::CompiledModel,
    config::{ChainBinomialConfig, SimConfig},
    ekrng::StatefulRng,
    error::SimError,
    intervention::{all_intervention_times, apply_interventions_at},
    ode_integrator::rk4_step,
    output::output_times as get_output_times,
    propensity::{eval_propensities, eval_expr, EvalCtx},
    simulate::Simulate,
    state::{FlowVec, Snapshot, Trajectory},
};

pub struct ChainBinomialSim;

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
            let ctx = EvalCtx { model, int_s: &int_s, real_s: &real_s, params, t };
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

        // Multinomial draws for transitions sharing a source compartment.
        // Sequential conditional binomial decomposition:
        //   count_1 ~ Binom(n_remaining, p_1 / (1 - 0))
        //   count_2 ~ Binom(n_remaining - count_1, p_2 / (1 - p_1))
        // Exact for the multinomial. Guarantees Σ counts ≤ n_src.
        for &(src_local, ref group) in &model.source_groups {
            let n_src = int_s.counts[src_local].max(0);
            if n_src == 0 {
                for &tr_idx in group { handled[tr_idx] = true; }
                continue;
            }

            let mut probs: Vec<(usize, f64)> = Vec::with_capacity(group.len());
            for &tr_idx in group {
                let rate = propensities[tr_idx];
                if rate <= 0.0 {
                    handled[tr_idx] = true;
                    continue;
                }
                let per_capita = rate / n_src as f64;
                match &draws[tr_idx] {
                    ResolvedDraw::Deterministic => {
                        // Deterministic inflows handled separately below
                        // (shouldn't appear in source groups, but guard anyway)
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
                let p = 1.0 - (-effective * dt).exp();
                probs.push((tr_idx, p.clamp(0.0, 1.0)));
            }

            let mut n_remaining = n_src as u64;
            let mut p_consumed = 0.0_f64;

            for &(tr_idx, p_i) in &probs {
                if n_remaining == 0 {
                    handled[tr_idx] = true;
                    continue;
                }
                let p_budget = 1.0 - p_consumed;
                if p_budget <= 0.0 {
                    handled[tr_idx] = true;
                    continue;
                }
                let cond_p = (p_i / p_budget).clamp(0.0, 1.0);
                let count = rng.binomial(n_remaining, cond_p);
                n_remaining -= count;
                p_consumed += p_i;

                // Buffer the stoichiometry deltas — do NOT apply to int_s yet.
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

        // Apply all deltas simultaneously — the state transitions atomically
        // from start-of-step to end-of-step with no intermediate visibility.
        for (local, delta) in pending_deltas {
            int_s.counts[local] += delta;
        }

        let clamped = int_s.clamp_nonneg();
        if clamped > 0 {
            log::warn!("chain-binomial: clamped {} negative compartments at t={}", clamped, t);
        }
        debug_assert!(
            int_s.counts.iter().all(|&v| v >= 0),
            "non-negativity violated after chain-binomial step at t={}", t
        );

        t += dt;

        // Interventions
        if iv_times.get(iv_idx).copied().is_some_and(|iv| (iv - t).abs() < cfg.dt * 0.5) {
            apply_interventions_at(t, model, &mut int_s, &mut real_s, params, cfg.dt * 0.5)?;
            while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + cfg.dt * 0.5 { iv_idx += 1; }
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
