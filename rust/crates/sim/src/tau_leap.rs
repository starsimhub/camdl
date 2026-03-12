use crate::{
    compiled_model::CompiledModel,
    config::{SimConfig, TauLeapConfig},
    ekrng::EkRng,
    error::SimError,
    intervention::{all_intervention_times, apply_interventions_at},
    ode_integrator::rk4_step,
    output::output_times as get_output_times,
    propensity::eval_propensities,
    simulate::Simulate,
    state::{FlowVec, Snapshot, Trajectory},
};

pub struct TauLeapSim;

impl Simulate for TauLeapSim {
    fn run(
        &self,
        model: &CompiledModel,
        params: &[f64],
        seed: u64,
        config: &SimConfig,
    ) -> Result<Trajectory, SimError> {
        let cfg = match config {
            SimConfig::TauLeap(c) => c,
            _ => return Err(SimError::ConfigMismatch {
                expected: "TauLeap",
                got: config.variant_name(),
            }),
        };
        run_tau_leap(model, params, seed, cfg)
    }
}

fn run_tau_leap(
    model: &CompiledModel,
    params: &[f64],
    seed: u64,
    cfg: &TauLeapConfig,
) -> Result<Trajectory, SimError> {
    let (mut int_s, mut real_s) = model.initial_state(params)?;
    let n_transitions = model.model.transitions.len();
    let n_real = real_s.values.len();

    let ekrng = EkRng::new(seed);
    let mut step_counts: Vec<u64> = vec![0; n_transitions];
    let mut propensities = Vec::with_capacity(n_transitions);

    let output_times = get_output_times(&model.model.output.times);
    let mut output_idx = 0;
    let iv_times = all_intervention_times(model);
    let mut iv_idx = 0;

    let mut traj = Trajectory::new();
    let mut current_flows = FlowVec::new(n_transitions);
    let mut t = cfg.t_start;

    // Initial snapshot
    if output_idx < output_times.len() && output_times[output_idx] <= t + 1e-12 {
        traj.push(Snapshot {
            t,
            int_state: int_s.clone(),
            real_state: real_s.clone(),
            flows: current_flows.clone(),
        });
        current_flows.reset();
        output_idx += 1;
    }

    let mut global_step: u64 = 0;

    while t < cfg.t_end {
        // Determine actual step (might be truncated by boundary)
        let next_boundary = {
            let out_t = output_times.get(output_idx).copied().unwrap_or(f64::INFINITY);
            let iv_t = iv_times.get(iv_idx).copied().unwrap_or(f64::INFINITY);
            cfg.t_end.min(out_t).min(iv_t)
        };
        let dt = cfg.dt.min(next_boundary - t);
        if dt <= 0.0 {
            // At a boundary — handle it
            // Apply intervention if due
            if iv_times.get(iv_idx).copied().map_or(false, |iv| (iv - t).abs() < 1e-10) {
                apply_interventions_at(t, model, &mut int_s, &mut real_s, params, 1e-10)?;
                while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + 1e-10 { iv_idx += 1; }
            }
            // Record output if due
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
            if t >= cfg.t_end { break; }
            continue;
        }

        // Evaluate propensities at current state
        eval_propensities(model, &int_s, &real_s, params, t, &mut propensities)?;

        // Draw Poisson counts for each transition (EKRNG-keyed by transition + step)
        for (i, &lambda) in propensities.iter().enumerate() {
            let event_key = &model.model.transitions[i].event_key;
            let count = if let Some(key) = event_key {
                // Replace {firing_index} template with global step
                let resolved = key.replace("{firing_index}", &global_step.to_string());
                ekrng.poisson_keyed(&resolved, step_counts[i], lambda * dt)
            } else {
                // No event key — should use stateful, but EkRng can substitute
                ekrng.poisson_keyed(&format!("__stateful_{}_{}", i, global_step), step_counts[i], lambda * dt)
            };

            // Apply stoichiometry
            for &(local, delta) in &model.transition_stoich[i] {
                int_s.counts[local] += delta * count as i64;
            }
            current_flows.add(i, count);
            step_counts[i] += 1;
        }

        // Clamp
        let clamped = int_s.clamp_nonneg();
        if clamped > 0 {
            log::warn!("tau-leap: clamped {} negative compartments at t={}", clamped, t);
        }
        debug_assert!(
            int_s.counts.iter().all(|&v| v >= 0),
            "non-negativity violated after tau-leap step at t={}", t
        );

        // RK4 for real compartments (integer state now at end-of-step)
        if n_real > 0 {
            rk4_step(model, &int_s, &mut real_s, params, t, dt)?;
            real_s.clamp_nonneg();
        }

        t += dt;
        global_step += 1;

        // Apply intervention if now at that time
        if iv_times.get(iv_idx).copied().map_or(false, |iv| (iv - t).abs() < 1e-10) {
            apply_interventions_at(t, model, &mut int_s, &mut real_s, params, 1e-10)?;
            while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + 1e-10 { iv_idx += 1; }
        }

        // Record outputs
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

    // Flush remaining outputs
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
