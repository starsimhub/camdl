use crate::{
    compiled_model::CompiledModel,
    config::{GillespieConfig, SimConfig},
    ekrng::StatefulRng,
    error::SimError,
    intervention::{all_intervention_times, apply_interventions_at},
    ode_integrator::rk4_step,
    output::output_times as get_output_times,
    propensity::eval_propensities,
    simulate::Simulate,
    state::{FlowVec, IntState, RealState, Snapshot, Trajectory},
    transition_diagnostics::TransitionDiagnostics,
};

pub struct GillespieSim;

impl Simulate for GillespieSim {
    fn run(
        &self,
        model: &CompiledModel,
        params: &[f64],
        seed: u64,
        config: &SimConfig,
    ) -> Result<Trajectory, SimError> {
        let cfg = match config {
            SimConfig::Gillespie(c) => c,
            _ => return Err(SimError::ConfigMismatch {
                expected: "Gillespie",
                got: config.variant_name(),
            }),
        };
        run_gillespie(model, params, seed, cfg)
    }
}

fn run_gillespie(
    model: &CompiledModel,
    params: &[f64],
    seed: u64,
    cfg: &GillespieConfig,
) -> Result<Trajectory, SimError> {
    let (mut int_s, mut real_s) = model.initial_state(params)?;

    let n_transitions = model.model.transitions.len();
    let n_real = real_s.values.len();

    // Per-transition firing diagnostics
    let mut diag_vec: Vec<TransitionDiagnostics> = model.model.transitions.iter()
        .map(|t| TransitionDiagnostics::new(t.name.clone()))
        .collect();

    // Propensity buffer — allocated once, reused
    let mut propensities: Vec<f64> = Vec::with_capacity(n_transitions);

    // Scenario coupling via Common Random Numbers (CRN): run baseline and intervention
    // with the same seed. Before the intervention time, states and propensities are
    // identical → sequential draws are identical → trajectories are identical.
    // After the intervention, trajectories diverge naturally.
    // EKRNG (per-transition keyed draws) would add per-event hash overhead with marginal
    // variance reduction for compartmental models — reserved for future ABM / conditional
    // SMC use cases (ekrng.rs is available if needed).
    let mut stateful_rng = StatefulRng::new(seed);

    // Sorted output times
    let output_times = get_output_times(&model.model.output.times);
    let mut output_idx = 0;

    // Sorted intervention times
    let iv_times = all_intervention_times(model);
    let mut iv_idx = 0;

    let mut t = cfg.t_start;
    let mut traj = Trajectory::new();
    let mut current_flows = FlowVec::new(n_transitions);

    // Record initial state
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

    loop {
        if t >= cfg.t_end { break; }

        // Evaluate propensities
        eval_propensities(model, &int_s, &real_s, params, t, &mut propensities)?;
        let lambda_total: f64 = propensities.iter().sum();

        if lambda_total <= 0.0 {
            // Absorbing state — advance to next output/intervention or end
            let next_special = next_time(t, cfg.t_end, output_idx, &output_times, iv_idx, &iv_times);
            // Flush outputs up to end
            flush_outputs(
                t, next_special, &mut output_idx, &output_times,
                &int_s, &real_s, &mut current_flows, &mut traj, n_transitions,
            );
            // If we hit t_end, break; if intervention, apply and continue
            if let Some(iv_t) = next_iv(t, iv_idx, &iv_times) {
                if iv_t <= cfg.t_end {
                    t = iv_t;
                    apply_interventions_at(t, model, &mut int_s, &mut real_s, params, 1e-10)?;
                    while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + 1e-10 {
                        iv_idx += 1;
                    }
                    // Propensities might become non-zero again after intervention
                    continue;
                }
            }
            break;
        }

        // Draw time to next event (stateful for global clock, but keyed per transition)
        // For Gillespie: draw total waiting time, then select transition
        let u1: f64 = stateful_rng.uniform();
        let dt = -(1.0 / lambda_total) * u1.ln();
        let t_next = t + dt;

        // Check for intervention or output boundary before this event
        let next_iv_t = next_iv(t, iv_idx, &iv_times);
        let next_out_t = output_times.get(output_idx).copied();

        let boundary = [Some(cfg.t_end), next_iv_t, next_out_t]
            .iter()
            .filter_map(|x| *x)
            .filter(|&b| b < t_next)
            .fold(f64::INFINITY, f64::min);

        if boundary < f64::INFINITY {
            // Advance to boundary without firing an event
            // TODO(v0.2): replace with PDMP thinning for real compartments
            // For v0.1: advance real state to boundary using RK4
            if n_real > 0 && (boundary - t) > 1e-15 {
                rk4_step(model, &int_s, &mut real_s, params, t, boundary - t)?;
                real_s.clamp_nonneg();
            }
            t = boundary;

            // Apply intervention if at intervention boundary
            if next_iv_t.map_or(false, |iv_t| (iv_t - t).abs() < 1e-10) {
                apply_interventions_at(t, model, &mut int_s, &mut real_s, params, 1e-10)?;
                while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + 1e-10 {
                    iv_idx += 1;
                }
            }

            // Record output if at output boundary
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

        // Fire an event: select transition proportional to propensity
        let u2: f64 = stateful_rng.uniform();
        let threshold = u2 * lambda_total;
        let mut cumulative = 0.0;
        let mut fired_idx = n_transitions - 1;
        for (i, &p) in propensities.iter().enumerate() {
            cumulative += p;
            if cumulative >= threshold {
                fired_idx = i;
                break;
            }
        }

        // Advance real state to event time
        // TODO(v0.2): replace with PDMP thinning
        if n_real > 0 && dt > 1e-15 {
            rk4_step(model, &int_s, &mut real_s, params, t, dt)?;
            real_s.clamp_nonneg();
        }
        t = t_next;

        // Record firing diagnostics
        diag_vec[fired_idx].record_firing(t, propensities[fired_idx]);

        // Apply stoichiometry
        for &(local, delta) in &model.transition_stoich[fired_idx] {
            int_s.counts[local] += delta;
        }

        // Clamp non-negativity
        let clamped = int_s.clamp_nonneg();
        if clamped > 0 {
            log::warn!("Gillespie: clamped {} negative integer compartments at t={}", clamped, t);
        }

        // Track flow
        current_flows.add(fired_idx, 1);

        debug_assert!(
            int_s.counts.iter().all(|&v| v >= 0),
            "non-negativity violated at t={}", t
        );

        // Record output at any output times we've passed
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

    // Ensure final output time is recorded
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

    traj.transition_diagnostics = diag_vec;
    Ok(traj)
}

fn next_iv(t: f64, iv_idx: usize, iv_times: &[f64]) -> Option<f64> {
    iv_times.get(iv_idx).copied().filter(|&iv| iv > t)
}

fn next_time(
    t: f64, t_end: f64,
    out_idx: usize, out_times: &[f64],
    iv_idx: usize, iv_times: &[f64],
) -> f64 {
    let out_t = out_times.get(out_idx).copied().unwrap_or(f64::INFINITY);
    let iv_t = iv_times.get(iv_idx).copied().unwrap_or(f64::INFINITY);
    t_end.min(out_t).min(iv_t)
}

#[allow(clippy::too_many_arguments)]
fn flush_outputs(
    _t_from: f64,
    t_to: f64,
    output_idx: &mut usize,
    output_times: &[f64],
    int_s: &IntState,
    real_s: &RealState,
    current_flows: &mut FlowVec,
    traj: &mut Trajectory,
    _n_transitions: usize,
) {
    while *output_idx < output_times.len() && output_times[*output_idx] <= t_to + 1e-12 {
        traj.push(Snapshot {
            t: output_times[*output_idx],
            int_state: int_s.clone(),
            real_state: real_s.clone(),
            flows: current_flows.clone(),
        });
        current_flows.reset();
        *output_idx += 1;
    }
}
