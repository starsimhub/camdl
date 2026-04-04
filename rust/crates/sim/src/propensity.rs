use crate::{
    compiled_model::{CompiledModel, CompiledTimeFuncKind},
    error::SimError,
    state::{IntState, RealState},
};
use ir::expr::{BinOp, Expr, UnOp};

/// Evaluation context: bundles all read-only simulation state for a single time step.
/// Passed by reference to `eval_expr` and all callers, eliminating the repeated
/// `(model, int_s, real_s, params, t)` parameter list.
pub struct EvalCtx<'a> {
    pub model:  &'a CompiledModel,
    pub int_s:  &'a IntState,
    pub real_s: &'a RealState,
    pub params: &'a [f64],
    pub t:      f64,
    /// Projected observation value — only set when evaluating likelihood Exprs.
    /// `Expr::Projected` returns this value; errors if None.
    pub projected: Option<f64>,
}

/// Evaluate a single expression. No allocations in steady state.
pub fn eval_expr(expr: &Expr, ctx: &EvalCtx<'_>) -> Result<f64, SimError> {
    match expr {
        Expr::Const(c) => Ok(c.value),

        Expr::Param(p) => {
            let idx = ctx.model.param_index.get(p.param.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownParameter(p.param.clone()))?;
            Ok(ctx.params[idx])
        }

        Expr::Pop(p) => {
            let global = ctx.model.comp_index.get(p.pop.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownCompartment(p.pop.clone()))?;
            if let Some(local) = ctx.model.global_to_int[global] {
                Ok(ctx.int_s.counts[local] as f64)
            } else if let Some(local) = ctx.model.global_to_real[global] {
                Ok(ctx.real_s.values[local])
            } else {
                Err(SimError::UnknownCompartment(p.pop.clone()))
            }
        }

        Expr::PopSum(ps) => {
            let mut sum = 0.0;
            for name in &ps.pop_sum {
                let global = ctx.model.comp_index.get(name.as_str())
                    .copied()
                    .ok_or_else(|| SimError::UnknownCompartment(name.clone()))?;
                if let Some(local) = ctx.model.global_to_int[global] {
                    sum += ctx.int_s.counts[local] as f64;
                } else if let Some(local) = ctx.model.global_to_real[global] {
                    sum += ctx.real_s.values[local];
                }
            }
            Ok(sum)
        }

        Expr::Time(_) => Ok(ctx.t),

        Expr::BinOp(w) => {
            let a = eval_expr(&w.bin_op.left, ctx)?;
            let b = eval_expr(&w.bin_op.right, ctx)?;
            Ok(match w.bin_op.op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => {
                    if b == 0.0 {
                        log::debug!("eval_expr: Div by zero suppressed at t={}, returning 0.0", ctx.t);
                        0.0
                    } else {
                        a / b
                    }
                }
                BinOp::Pow => {
                    let r = a.powf(b);
                    if r.is_nan() || r.is_infinite() {
                        log::warn!("eval_expr: {}^{} = {} at t={}, returning 0.0", a, b, r, ctx.t);
                        0.0
                    } else { r }
                }
                BinOp::Mod => if b == 0.0 { 0.0 } else { a.rem_euclid(b) },
                BinOp::Min => a.min(b),
                BinOp::Max => a.max(b),
                BinOp::Eq  => if a == b { 1.0 } else { 0.0 },
                BinOp::Neq => if a != b { 1.0 } else { 0.0 },
                BinOp::Lt  => if a <  b { 1.0 } else { 0.0 },
                BinOp::Gt  => if a >  b { 1.0 } else { 0.0 },
                BinOp::Le  => if a <= b { 1.0 } else { 0.0 },
                BinOp::Ge  => if a >= b { 1.0 } else { 0.0 },
            })
        }

        Expr::UnOp(w) => {
            let a = eval_expr(&w.un_op.arg, ctx)?;
            let result = match w.un_op.op {
                UnOp::Neg   => -a,
                UnOp::Exp   => a.exp(),
                UnOp::Log   => if a > 0.0 { a.ln() } else { f64::NEG_INFINITY },
                UnOp::Sqrt  => if a >= 0.0 { a.sqrt() } else { 0.0 },
                UnOp::Abs   => a.abs(),
                UnOp::Floor => a.floor(),
                UnOp::Ceil  => a.ceil(),
            };
            if result.is_nan() {
                log::warn!("eval_expr: NaN from {:?}({}) at t={}", w.un_op.op, a, ctx.t);
                Ok(0.0)
            } else {
                Ok(result)
            }
        }

        Expr::Cond(w) => {
            let pred = eval_expr(&w.cond.pred, ctx)?;
            if pred > 0.0 {
                eval_expr(&w.cond.then, ctx)
            } else {
                eval_expr(&w.cond.else_, ctx)
            }
        }

        Expr::TimeFunc(w) => {
            let idx = ctx.model.time_func_index.get(w.time_func.name.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownTimeFunction(w.time_func.name.clone()))?;
            Ok(eval_time_func(&ctx.model.time_func_cache[idx].kind, ctx.t))
        }

        Expr::TableLookup(w) => {
            let idx = ctx.model.table_index.get(w.table_lookup.table.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownTable(w.table_lookup.table.clone()))?;
            let table = &ctx.model.model.tables[idx];
            let cached = &ctx.model.table_values_cache[idx];
            // Only single-index lookups supported (OCaml compiler pre-flattens multi-dim)
            if w.table_lookup.indices.len() != 1 {
                return Err(SimError::TableLookup(format!(
                    "table '{}' requires exactly 1 index, got {}",
                    w.table_lookup.table, w.table_lookup.indices.len()
                )));
            }
            let raw = eval_expr(&w.table_lookup.indices[0], ctx)?;
            let table_idx = raw.floor() as i64;
            table_lookup(table, cached, table_idx)
        }

        Expr::Projected(_) => {
            ctx.projected.ok_or_else(|| SimError::Validation(
                "Projected expression used outside observation likelihood context".into()
            ))
        }
    }
}

/// Perform a table lookup using the table's OobPolicy and pre-evaluated cached values.
fn table_lookup(table: &ir::table::Table, cached: &[f64], idx: i64) -> Result<f64, SimError> {
    use ir::table::OobPolicy;
    let n = cached.len() as i64;
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
    Ok(cached[i as usize])
}

/// Evaluate a compiled time function kind at time `t`.
pub fn eval_time_func(kind: &CompiledTimeFuncKind, t: f64) -> f64 {
    match kind {
        CompiledTimeFuncKind::Sinusoidal { amplitude, period, phase, baseline } => {
            baseline + amplitude * (2.0 * std::f64::consts::PI * (t - phase) / period).sin()
        }
        CompiledTimeFuncKind::Piecewise { breakpoints, values } => {
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
        CompiledTimeFuncKind::Interpolated { times, values } => {
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
        CompiledTimeFuncKind::Constant { times, values } => {
            // Piecewise constant: return value at the largest grid point <= t.
            // Matches pomp's covariate_table(order = "constant").
            if times.is_empty() || values.is_empty() { return 0.0; }
            if t <= times[0] { return values[0]; }
            if t >= *times.last().unwrap() { return *values.last().unwrap(); }
            // Binary search for the last grid point <= t
            match times.binary_search_by(|x| x.partial_cmp(&t).unwrap()) {
                Ok(i) => values[i],
                Err(i) => values[i - 1], // i is insertion point; i-1 is last point <= t
            }
        }
        CompiledTimeFuncKind::CubicSpline(spline) => spline.eval(t),
        CompiledTimeFuncKind::Periodic { period, values } => {
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
    let ctx = EvalCtx { model, int_s, real_s, params, t , projected: None };
    out.clear();
    for tr in model.model.transitions.iter() {
        let p = eval_expr(&tr.rate, &ctx)?;
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
