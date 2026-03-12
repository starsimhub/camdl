use crate::{
    compiled_model::CompiledModel,
    error::SimError,
    state::{IntState, RealState},
};
use ir::expr::{BinOp, Expr, UnOp};
use ir::time_func::{TimeFuncKind, Sinusoidal, Piecewise, Interpolated, Periodic};

/// Evaluate a single expression. No allocations in steady state.
pub fn eval_expr(
    expr: &Expr,
    model: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
    params: &[f64],
    t: f64,
) -> Result<f64, SimError> {
    match expr {
        Expr::Const(c) => Ok(c.value),

        Expr::Param(p) => {
            let idx = model.param_index.get(p.param.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownParameter(p.param.clone()))?;
            Ok(params[idx])
        }

        Expr::Pop(p) => {
            let global = model.comp_index.get(p.pop.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownCompartment(p.pop.clone()))?;
            if let Some(local) = model.global_to_int[global] {
                Ok(int_s.counts[local] as f64)
            } else if let Some(local) = model.global_to_real[global] {
                Ok(real_s.values[local])
            } else {
                Err(SimError::UnknownCompartment(p.pop.clone()))
            }
        }

        Expr::PopSum(ps) => {
            let mut sum = 0.0;
            for name in &ps.pop_sum {
                let global = model.comp_index.get(name.as_str())
                    .copied()
                    .ok_or_else(|| SimError::UnknownCompartment(name.clone()))?;
                if let Some(local) = model.global_to_int[global] {
                    sum += int_s.counts[local] as f64;
                } else if let Some(local) = model.global_to_real[global] {
                    sum += real_s.values[local];
                }
            }
            Ok(sum)
        }

        Expr::Time(_) => Ok(t),

        Expr::BinOp(w) => {
            let a = eval_expr(&w.bin_op.left, model, int_s, real_s, params, t)?;
            let b = eval_expr(&w.bin_op.right, model, int_s, real_s, params, t)?;
            Ok(match w.bin_op.op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => {
                    if b == 0.0 {
                        log::debug!("eval_expr: Div by zero suppressed at t={t}, returning 0.0");
                        0.0
                    } else {
                        a / b
                    }
                }
                BinOp::Pow => a.powf(b),
                BinOp::Min => a.min(b),
                BinOp::Max => a.max(b),
            })
        }

        Expr::UnOp(w) => {
            let a = eval_expr(&w.un_op.arg, model, int_s, real_s, params, t)?;
            Ok(match w.un_op.op {
                UnOp::Neg   => -a,
                UnOp::Exp   => a.exp(),
                UnOp::Log   => a.ln(),
                UnOp::Sqrt  => a.sqrt(),
                UnOp::Abs   => a.abs(),
                UnOp::Floor => a.floor(),
                UnOp::Ceil  => a.ceil(),
            })
        }

        Expr::Cond(w) => {
            let pred = eval_expr(&w.cond.pred, model, int_s, real_s, params, t)?;
            if pred > 0.0 {
                eval_expr(&w.cond.then, model, int_s, real_s, params, t)
            } else {
                eval_expr(&w.cond.else_, model, int_s, real_s, params, t)
            }
        }

        Expr::TimeFunc(w) => {
            let idx = model.time_func_index.get(w.time_func.name.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownTimeFunction(w.time_func.name.clone()))?;
            Ok(eval_time_func(&model.model.time_functions[idx].kind, t))
        }

        Expr::TableLookup(w) => {
            let idx = model.table_index.get(w.table_lookup.table.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownTable(w.table_lookup.table.clone()))?;
            let table = &model.model.tables[idx];
            // Only single-index lookups supported
            if w.table_lookup.indices.len() != 1 {
                return Err(SimError::TableLookup(format!(
                    "table '{}' requires exactly 1 index, got {}",
                    w.table_lookup.table, w.table_lookup.indices.len()
                )));
            }
            let raw = eval_expr(&w.table_lookup.indices[0], model, int_s, real_s, params, t)?;
            let table_idx = raw.floor() as i64;
            table_lookup(table, table_idx)
        }

        Expr::Projected(_) => {
            Err(SimError::Validation("Projected expression is only valid in observation likelihood context".into()))
        }
    }
}

/// Perform a table lookup using the table's OobPolicy.
fn table_lookup(table: &ir::table::Table, idx: i64) -> Result<f64, SimError> {
    use ir::table::OobPolicy;
    let n = table.values.len() as i64;
    let i = match table.out_of_bounds {
        OobPolicy::Clamp => idx.clamp(0, n - 1),
        OobPolicy::Wrap  => {
            if n == 0 { return Err(SimError::TableLookup(format!("table '{}' is empty", table.name))); }
            idx.rem_euclid(n)
        }
        OobPolicy::Error => {
            if idx < 0 || idx >= n {
                return Err(SimError::TableLookup(format!(
                    "table '{}': index {} out of bounds [0, {})", table.name, idx, n
                )));
            }
            idx
        }
    };
    Ok(table.values[i as usize])
}

/// Evaluate a time function kind at time `t`.
pub fn eval_time_func(kind: &TimeFuncKind, t: f64) -> f64 {
    match kind {
        TimeFuncKind::Sinusoidal(Sinusoidal { amplitude, period, phase, baseline }) => {
            baseline + amplitude * (2.0 * std::f64::consts::PI * (t - phase) / period).sin()
        }
        TimeFuncKind::Piecewise(Piecewise { breakpoints, values }) => {
            // Constant on each interval: values[i] applies for t in [breakpoints[i-1], breakpoints[i])
            // values[0] applies before breakpoints[0]; values[last] applies after breakpoints[last-1]
            if values.is_empty() { return 0.0; }
            let mut result = values[0];
            for (i, &bp) in breakpoints.iter().enumerate() {
                if t >= bp && i + 1 < values.len() {
                    result = values[i + 1];
                }
            }
            result
        }
        TimeFuncKind::Interpolated(Interpolated { times, values, .. }) => {
            // Linear interpolation
            if times.is_empty() || values.is_empty() { return 0.0; }
            if t <= times[0] { return values[0]; }
            if t >= *times.last().unwrap() { return *values.last().unwrap(); }
            for i in 0..times.len() - 1 {
                if t >= times[i] && t <= times[i + 1] {
                    let frac = (t - times[i]) / (times[i + 1] - times[i]);
                    return values[i] + frac * (values[i + 1] - values[i]);
                }
            }
            *values.last().unwrap()
        }
        TimeFuncKind::Periodic(Periodic { period, values }) => {
            if values.is_empty() || *period <= 0.0 { return 0.0; }
            let phase = t.rem_euclid(*period);
            let n = values.len();
            let step = period / n as f64;
            let i = (phase / step).floor() as usize;
            values[i.min(n - 1)]
        }
    }
}

/// Evaluate all propensities into `out` (cleared and refilled in-place).
/// No allocation if `out` is already the right size.
pub fn eval_propensities(
    model: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
    params: &[f64],
    t: f64,
    out: &mut Vec<f64>,
) -> Result<(), SimError> {
    out.clear();
    for tr in model.model.transitions.iter() {
        let p = eval_expr(&tr.rate, model, int_s, real_s, params, t)?;
        if p < 0.0 {
            return Err(SimError::NegativePropensity {
                transition: tr.name.clone(),
                value: p,
                t,
            });
        }
        out.push(p);
    }
    Ok(())
}
