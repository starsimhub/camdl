use crate::{
    compiled_model::CompiledModel,
    config::{ChainBinomialConfig, SimConfig},
    ekrng::StatefulRng,
    error::SimError,
    intervention::{all_intervention_times, apply_interventions_at},
    ode_integrator::rk4_step,
    output::output_times as get_output_times,
    propensity::eval_propensities,
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
        for (i, &rate) in propensities.iter().enumerate() {
            let p = 1.0 - (-rate * dt).exp();
            if p <= 0.0 { continue; }

            // Source population for this transition (first compartment with negative delta)
            let n_src: i64 = model.transition_stoich[i].iter()
                .filter(|&&(_, d)| d < 0)
                .map(|&(local, _)| int_s.counts[local])
                .next()
                .unwrap_or(0)
                .max(0);

            if n_src == 0 { continue; }

            // Poisson(n*p) approximates Binomial(n, p); CRN via stateful RNG
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
        if iv_times.get(iv_idx).copied().map_or(false, |iv| (iv - t).abs() < cfg.dt * 0.5) {
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
