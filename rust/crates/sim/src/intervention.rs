use crate::{
    compiled_model::CompiledModel,
    error::SimError,
    propensity::EvalCtx,
    resolved_expr::eval_resolved,
    state::{IntState, RealState},
};
use ir::intervention::{Action, Intervention, InterventionSchedule};

/// Convert an `InterventionSchedule` to a sorted list of fire times.
pub fn intervention_fire_times(sched: &InterventionSchedule) -> Vec<f64> {
    match sched {
        InterventionSchedule::AtTimes(times) => times.clone(),
        InterventionSchedule::Recurring(rs) => {
            let mut times = Vec::new();
            if let Some(at_day) = rs.at_day {
                // Fire at at_day + k*period, for smallest k where target >= start
                let k0 = ((rs.start - at_day) / rs.period).ceil().max(0.0) as u64;
                let mut t = at_day + k0 as f64 * rs.period;
                while t <= rs.end + rs.period * 1e-9 {
                    times.push(t);
                    t += rs.period;
                }
            } else {
                let mut t = rs.start;
                while t <= rs.end + rs.period * 1e-9 {
                    times.push(t);
                    t += rs.period;
                }
            }
            times
        }
        InterventionSchedule::External(_) => vec![],
    }
}

/// Apply all interventions scheduled at time `t` (in document order).
pub fn apply_interventions_at(
    t: f64,
    model: &CompiledModel,
    int_s: &mut IntState,
    real_s: &mut RealState,
    params: &[f64],
    _tolerance: f64,
) -> Result<bool, SimError> {
    let dt = model.model.simulation.dt.unwrap_or(1.0);
    // Rm4 in 2026-04-19 engine review: guard against NaN t silently
    // rounding to step 0. NaN `as i64` is 0 on current rustc, which
    // would make every intervention match step 0 if an upstream bug
    // ever produced NaN.
    if !t.is_finite() {
        return Err(SimError::Validation(format!(
            "apply_interventions_at: non-finite t = {}", t
        )));
    }
    let current_step = (t / dt).round() as i64;
    let mut any_fired = false;
    for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
        if iv.always_active { continue; }
        if model.fire_steps[iv_idx].contains(&current_step) {
            apply_intervention(iv, iv_idx, model, int_s, real_s, params, t)?;
            any_fired = true;
        }
    }
    Ok(any_fired)
}

/// Inject always_active event actions as deltas into `pending_deltas`.
///
/// All action types are expressed as deltas from the snapshot state:
///   Add(n)        → (+n, target)
///   Transfer(f)   → (-delta, src), (+delta, dst) where delta = floor(src * f)
///   Set(v)        → (v - old, target) where old is from snapshot
///
/// Called from both `step_one` and `run_chain_binomial` to ensure events
/// are applied atomically with transitions, matching pomp's ordering.
pub fn inject_event_deltas(
    model: &CompiledModel,
    snapshot: &IntState,
    real_s: &RealState,
    params: &[f64],
    t: f64,
    dt: f64,
    pending_deltas: &mut Vec<(usize, i64)>,
) -> Result<(), SimError> {
    let t_end = t + dt;
    let ctx = EvalCtx {
        model, int_s: snapshot, real_s, params, t: t_end, projected: None, int_float_override: None,
    };
    let current_step = (t_end / dt).round() as i64;
    for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
        if !iv.always_active { continue; }
        if !model.fire_steps[iv_idx].contains(&current_step) { continue; }
        for (action_idx, action) in iv.actions.iter().enumerate() {
            let resolved_val = eval_resolved(&model.resolved.intervention_exprs[iv_idx][action_idx], &ctx);
            match action {
                Action::Add(aa) => {
                    let raw = resolved_val;
                    let n = raw.round() as i64;
                    {
                        use std::sync::OnceLock;
                        static TRACE: OnceLock<bool> = OnceLock::new();
                        if *TRACE.get_or_init(|| std::env::var("CAMDL_TRACE_STEPS").is_ok_and(|v| v == "1")) {
                            eprintln!("EVENT '{}' at t={:.1}: add {} += {} (raw={:.2})",
                                iv.name, t_end, aa.compartment, n, raw);
                        }
                    }
                    if let Some(&global) = model.comp_index.get(aa.compartment.as_str()) {
                        if let Some(local) = model.global_to_int[global] {
                            pending_deltas.push((local, n));
                        }
                    }
                }
                Action::FractionTransfer(ft) => {
                    let frac = resolved_val.clamp(0.0, 1.0);
                    if let (Some(&sg), Some(&dg)) = (
                        model.comp_index.get(ft.src.as_str()),
                        model.comp_index.get(ft.dst.as_str()),
                    ) {
                        if let (Some(sl), Some(dl)) = (model.global_to_int[sg], model.global_to_int[dg]) {
                            let delta = (snapshot.counts[sl] as f64 * frac).floor() as i64;
                            pending_deltas.push((sl, -delta));
                            pending_deltas.push((dl, delta));
                        }
                    }
                }
                Action::AbsoluteTransfer(at) => {
                    let n = resolved_val.round() as i64;
                    if let (Some(&sg), Some(&dg)) = (
                        model.comp_index.get(at.src.as_str()),
                        model.comp_index.get(at.dst.as_str()),
                    ) {
                        if let (Some(sl), Some(dl)) = (model.global_to_int[sg], model.global_to_int[dg]) {
                            let transfer = n.min(snapshot.counts[sl]);
                            pending_deltas.push((sl, -transfer));
                            pending_deltas.push((dl, transfer));
                        }
                    }
                }
                Action::Set(sa) => {
                    let new_val = resolved_val.round() as i64;
                    if let Some(&global) = model.comp_index.get(sa.compartment.as_str()) {
                        if let Some(local) = model.global_to_int[global] {
                            let old_val = snapshot.counts[local];
                            pending_deltas.push((local, new_val - old_val));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Collect sorted, deduplicated intervention times.
pub fn all_intervention_times(model: &CompiledModel) -> Vec<f64> {
    let mut times: Vec<f64> = model.model.interventions.iter()
        .flat_map(|iv| intervention_fire_times(&iv.schedule))
        .collect();
    times.sort_by(|a, b| a.total_cmp(b));
    times.dedup();
    times
}

fn apply_intervention(
    iv: &Intervention,
    iv_idx: usize,
    model: &CompiledModel,
    int_s: &mut IntState,
    real_s: &mut RealState,
    params: &[f64],
    t: f64,
) -> Result<(), SimError> {
    for (action_idx, action) in iv.actions.iter().enumerate() {
        let resolved_val = eval_resolved(
            &model.resolved.intervention_exprs[iv_idx][action_idx],
            &EvalCtx { model, int_s, real_s, params, t, projected: None, int_float_override: None },
        );
        match action {
            Action::FractionTransfer(ft) => {
                let frac = resolved_val.clamp(0.0, 1.0);
                let src_global = *model.comp_index.get(ft.src.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(ft.src.clone()))?;
                let dst_global = *model.comp_index.get(ft.dst.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(ft.dst.clone()))?;

                if let (Some(s_local), Some(d_local)) = (
                    model.global_to_int[src_global],
                    model.global_to_int[dst_global],
                ) {
                    let transfer = ((int_s.counts[s_local] as f64) * frac).floor() as i64;
                    int_s.counts[s_local] -= transfer;
                    int_s.counts[d_local] += transfer;
                } else if let (Some(s_local), Some(d_local)) = (
                    model.global_to_real[src_global],
                    model.global_to_real[dst_global],
                ) {
                    let transfer = real_s.values[s_local] * frac;
                    real_s.values[s_local] -= transfer;
                    real_s.values[d_local] += transfer;
                }
            }

            Action::AbsoluteTransfer(at) => {
                let n = resolved_val;
                let src_global = *model.comp_index.get(at.src.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(at.src.clone()))?;
                let dst_global = *model.comp_index.get(at.dst.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(at.dst.clone()))?;

                if let (Some(s_local), Some(d_local)) = (
                    model.global_to_int[src_global],
                    model.global_to_int[dst_global],
                ) {
                    let transfer = (n.round() as i64).min(int_s.counts[s_local]);
                    int_s.counts[s_local] -= transfer;
                    int_s.counts[d_local] += transfer;
                } else if let (Some(s_local), Some(d_local)) = (
                    model.global_to_real[src_global],
                    model.global_to_real[dst_global],
                ) {
                    let transfer = n.min(real_s.values[s_local]);
                    real_s.values[s_local] -= transfer;
                    real_s.values[d_local] += transfer;
                }
            }

            Action::Set(sa) => {
                let v = resolved_val;
                let global = *model.comp_index.get(sa.compartment.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(sa.compartment.clone()))?;
                if let Some(local) = model.global_to_int[global] {
                    int_s.counts[local] = v.round() as i64;
                } else if let Some(local) = model.global_to_real[global] {
                    real_s.values[local] = v;
                }
            }

            Action::Add(aa) => {
                let n = resolved_val;
                let count = n.round() as i64;
                if count < 0 {
                    log::warn!("event '{}' adding negative count ({}) to '{}'",
                        iv.name, count, aa.compartment);
                }
                let global = *model.comp_index.get(aa.compartment.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(aa.compartment.clone()))?;
                if let Some(local) = model.global_to_int[global] {
                    int_s.counts[local] += count;
                } else if let Some(local) = model.global_to_real[global] {
                    real_s.values[local] += n;
                }
            }
        }
    }
    Ok(())
}
