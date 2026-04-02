use crate::{
    compiled_model::CompiledModel,
    error::SimError,
    propensity::{eval_expr, EvalCtx},
    state::{IntState, RealState},
};
use ir::intervention::{Action, Intervention, InterventionSchedule, RecurringSchedule};

/// Convert an `InterventionSchedule` to a sorted list of fire times.
pub fn intervention_fire_times(sched: &InterventionSchedule) -> Vec<f64> {
    match sched {
        InterventionSchedule::AtTimes(times) => times.clone(),
        InterventionSchedule::Recurring(RecurringSchedule { start, period, end }) => {
            let mut times = Vec::new();
            let mut t = *start;
            while t <= end + period * 1e-9 {
                times.push(t);
                t += period;
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
    tolerance: f64,
) -> Result<bool, SimError> {
    let mut any_fired = false;
    for iv in &model.model.interventions {
        for fire_t in intervention_fire_times(&iv.schedule) {
            if (fire_t - t).abs() <= tolerance {
                apply_intervention(iv, model, int_s, real_s, params, t)?;
                any_fired = true;
                break;
            }
        }
    }
    Ok(any_fired)
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
    model: &CompiledModel,
    int_s: &mut IntState,
    real_s: &mut RealState,
    params: &[f64],
    t: f64,
) -> Result<(), SimError> {
    for action in &iv.actions {
        match action {
            Action::FractionTransfer(ft) => {
                // Eval at current state before mutation; ctx scoped to drop before the &mut borrows.
                let frac = eval_expr(&ft.fraction, &EvalCtx { model, int_s, real_s, params, t , projected: None })?.clamp(0.0, 1.0);
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
                let n = eval_expr(&at.count, &EvalCtx { model, int_s, real_s, params, t , projected: None })?;
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
                let v = eval_expr(&sa.value, &EvalCtx { model, int_s, real_s, params, t , projected: None })?;
                let global = *model.comp_index.get(sa.compartment.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(sa.compartment.clone()))?;
                if let Some(local) = model.global_to_int[global] {
                    int_s.counts[local] = v.round() as i64;
                } else if let Some(local) = model.global_to_real[global] {
                    real_s.values[local] = v;
                }
            }
        }
    }
    Ok(())
}
