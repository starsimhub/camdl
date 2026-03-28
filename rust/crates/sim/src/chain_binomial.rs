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

        // Binomial draws for each transition
        // p = 1 - exp(-rate * dt) converts continuous-time rate to per-step probability
        // Pre-evaluate overdispersion expressions before mutating int_s
        let od_values: Vec<Option<f64>> = {
            let ctx = EvalCtx { model, int_s: &int_s, real_s: &real_s, params, t };
            model.model.transitions.iter()
                .map(|tr| match &tr.overdispersion {
                    Some(od_expr) => eval_expr(od_expr, &ctx).map(Some),
                    None => Ok(None),
                })
                .collect::<Result<_, _>>()?
        };
        for (i, &rate) in propensities.iter().enumerate() {
            if rate <= 0.0 { continue; }

            // Source population (first compartment with negative stoichiometry).
            // For inflows (no source), n_src = 0 → use Poisson on total propensity.
            let n_src: i64 = model.transition_stoich[i].iter()
                .filter(|&&(_, d)| d < 0)
                .map(|&(local, _)| int_s.counts[local])
                .next()
                .unwrap_or(0)
                .max(0);

            if n_src == 0 {
                // Inflow: no source compartment, draw from total propensity directly
                let count = rng.poisson(rate * dt);
                for &(local, delta) in &model.transition_stoich[i] {
                    int_s.counts[local] += delta * count as i64;
                }
                current_flows.add(i, count);
                continue;
            }

            // Convert total propensity → per-capita rate, then to probability.
            // rate = per_capita * n_src, so per_capita = rate / n_src.
            let per_capita = rate / n_src as f64;

            // Apply overdispersion to the per-capita rate before probability conversion.
            let effective_per_capita = match od_values[i] {
                Some(sigma_sq) => per_capita * rng.gamma_multiplier(sigma_sq, dt),
                None => per_capita,
            };

            let p = 1.0 - (-effective_per_capita * dt).exp();
            let lambda = (n_src as f64) * p;
            let count = rng.poisson(lambda).min(n_src as u64);

            for &(local, delta) in &model.transition_stoich[i] {
                int_s.counts[local] += delta * count as i64;
            }
            current_flows.add(i, count);
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
