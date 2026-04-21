use std::collections::HashMap;
use std::sync::Arc;
use ir::{Model, model::CompartmentKind};
use ir::expr::{BinOp, Expr, UnOp};
use crate::error::SimError;
use crate::resolved_expr::{ResolvedExpr, ResolveCtx, resolve_expr};
use crate::state::{IntState, RealState};

/// A time function with all Expr fields resolved to concrete f64 values.
#[derive(Debug, Clone)]
pub enum CompiledTimeFuncKind {
    Sinusoidal { amplitude: f64, period: f64, phase: f64, baseline: f64 },
    Piecewise   { breakpoints: Vec<f64>, values: Vec<f64> },
    Interpolated { times: Vec<f64>, values: Vec<f64> },
    /// Piecewise constant: value holds until the next grid point.
    /// Matches pomp's `covariate_table(order = "constant")`.
    Constant { times: Vec<f64>, values: Vec<f64> },
    CubicSpline(CubicSpline),
    Periodic    { period: f64, values: Vec<f64> },
}

/// Natural cubic spline with precomputed coefficients.
/// S_i(x) = a_i + b_i(x - x_i) + c_i(x - x_i)² + d_i(x - x_i)³
#[derive(Debug, Clone)]
pub struct CubicSpline {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    pub b: Vec<f64>,
    pub c: Vec<f64>,
    pub d: Vec<f64>,
}

impl CubicSpline {
    /// Build a natural cubic spline (second derivative = 0 at endpoints).
    /// Thomas algorithm on the tridiagonal system, O(n).
    pub fn new(xs: &[f64], ys: &[f64]) -> Self {
        let n = xs.len();
        assert!(n >= 2 && n == ys.len());
        // Validate strictly increasing x-values
        for i in 0..n - 1 {
            assert!(xs[i] < xs[i + 1],
                "CubicSpline: x-values must be strictly increasing, but xs[{}]={} >= xs[{}]={}",
                i, xs[i], i + 1, xs[i + 1]);
        }
        if n == 2 {
            let slope = (ys[1] - ys[0]) / (xs[1] - xs[0]);
            return CubicSpline {
                xs: xs.to_vec(), ys: ys.to_vec(),
                b: vec![slope, slope], c: vec![0.0, 0.0], d: vec![0.0, 0.0],
            };
        }
        let nm1 = n - 1;
        let h: Vec<f64> = (0..nm1).map(|i| xs[i + 1] - xs[i]).collect();

        // Build tridiagonal system for c coefficients
        // Equations: h[i-1]*c[i-1] + 2*(h[i-1]+h[i])*c[i] + h[i]*c[i+1]
        //            = 3*((y[i+1]-y[i])/h[i] - (y[i]-y[i-1])/h[i-1])
        let mut alpha = vec![0.0; n];
        for i in 1..nm1 {
            alpha[i] = 3.0 * ((ys[i + 1] - ys[i]) / h[i] - (ys[i] - ys[i - 1]) / h[i - 1]);
        }

        // Thomas algorithm: forward sweep
        let mut l = vec![1.0; n];
        let mut mu = vec![0.0; n];
        let mut z = vec![0.0; n];
        for i in 1..nm1 {
            l[i] = 2.0 * (xs[i + 1] - xs[i - 1]) - h[i - 1] * mu[i - 1];
            mu[i] = h[i] / l[i];
            z[i] = (alpha[i] - h[i - 1] * z[i - 1]) / l[i];
        }

        // Back substitution
        let mut c = vec![0.0; n]; // natural: c[0] = c[n-1] = 0
        for j in (0..nm1).rev() {
            c[j] = z[j] - mu[j] * c[j + 1];
        }

        // Compute b, d from c
        let mut b = vec![0.0; n];
        let mut d = vec![0.0; n];
        for i in 0..nm1 {
            b[i] = (ys[i + 1] - ys[i]) / h[i] - h[i] * (c[i + 1] + 2.0 * c[i]) / 3.0;
            d[i] = (c[i + 1] - c[i]) / (3.0 * h[i]);
        }

        CubicSpline { xs: xs.to_vec(), ys: ys.to_vec(), b, c, d }
    }

    /// Evaluate the spline at time t. Clamps to boundary values.
    pub fn eval(&self, t: f64) -> f64 {
        let n = self.xs.len();
        if t <= self.xs[0] { return self.ys[0]; }
        if t >= self.xs[n - 1] { return self.ys[n - 1]; }
        // Binary search for segment
        let mut lo = 0;
        let mut hi = n - 1;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if self.xs[mid] > t { hi = mid; } else { lo = mid; }
        }
        let dx = t - self.xs[lo];
        self.ys[lo] + self.b[lo] * dx + self.c[lo] * dx * dx + self.d[lo] * dx * dx * dx
    }
}

#[derive(Debug, Clone)]
pub struct CompiledTimeFunc {
    pub kind: CompiledTimeFuncKind,
}

/// Recursively collect integer compartment local indices referenced in an expression.
fn collect_int_comp_deps(
    expr: &Expr,
    comp_index: &HashMap<String, usize>,
    global_to_int: &[Option<usize>],
    deps: &mut std::collections::HashSet<usize>,
) {
    match expr {
        Expr::Pop(p) => {
            if let Some(&global) = comp_index.get(p.pop.as_str()) {
                if let Some(local) = global_to_int[global] {
                    deps.insert(local);
                }
            }
        }
        Expr::PopSum(ps) => {
            for name in &ps.pop_sum {
                if let Some(&global) = comp_index.get(name.as_str()) {
                    if let Some(local) = global_to_int[global] {
                        deps.insert(local);
                    }
                }
            }
        }
        Expr::BinOp(w) => {
            collect_int_comp_deps(&w.bin_op.left, comp_index, global_to_int, deps);
            collect_int_comp_deps(&w.bin_op.right, comp_index, global_to_int, deps);
        }
        Expr::UnOp(w) => {
            collect_int_comp_deps(&w.un_op.arg, comp_index, global_to_int, deps);
        }
        Expr::Cond(w) => {
            collect_int_comp_deps(&w.cond.pred, comp_index, global_to_int, deps);
            collect_int_comp_deps(&w.cond.then, comp_index, global_to_int, deps);
            collect_int_comp_deps(&w.cond.else_, comp_index, global_to_int, deps);
        }
        Expr::TableLookup(w) => {
            for idx_expr in &w.table_lookup.indices {
                collect_int_comp_deps(idx_expr, comp_index, global_to_int, deps);
            }
        }
        // Const, Param, Time, TimeFunc: no compartment dependencies
        _ => {}
    }
}

/// Returns true if the expression contains any time function reference.
fn expr_has_time_func(expr: &Expr) -> bool {
    match expr {
        Expr::TimeFunc(_) => true,
        Expr::BinOp(w) => {
            expr_has_time_func(&w.bin_op.left) || expr_has_time_func(&w.bin_op.right)
        }
        Expr::UnOp(w) => expr_has_time_func(&w.un_op.arg),
        Expr::Cond(w) => {
            expr_has_time_func(&w.cond.pred)
                || expr_has_time_func(&w.cond.then)
                || expr_has_time_func(&w.cond.else_)
        }
        Expr::TableLookup(w) => w.table_lookup.indices.iter().any(expr_has_time_func),
        _ => false,
    }
}

/// Evaluate a table value expression using only params (no compartment state).
///
/// This is a construction-time evaluator used before `CompiledModel` is fully
/// built — `eval_expr` cannot be used here because it requires an `EvalCtx`
/// with a completed model. Table value expressions are guaranteed to contain
/// only `Const`, `Param`, `BinOp`, and `UnOp` nodes (no `Pop`, `PopSum`,
/// `Time`, `TimeFunc`, or `TableLookup`). The `BinOp`/`UnOp` arms MUST match
/// the semantics in `eval_expr` — if a new operator is added there, it must
/// be added here too.
fn eval_table_expr(
    expr: &Expr,
    param_index: &HashMap<String, usize>,
    params: &[f64],
) -> Result<f64, SimError> {
    match expr {
        Expr::Const(c) => Ok(c.value),
        Expr::Param(p) => {
            let idx = param_index.get(p.param.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownParameter(p.param.clone()))?;
            Ok(params[idx])
        }
        Expr::BinOp(w) => {
            let a = eval_table_expr(&w.bin_op.left, param_index, params)?;
            let b = eval_table_expr(&w.bin_op.right, param_index, params)?;
            Ok(match w.bin_op.op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => if b == 0.0 { 0.0 } else { a / b },
                BinOp::Pow => {
                    // RM6 in 2026-04-19 engine review: align with
                    // eval_expr / eval_resolved, which both guard
                    // NaN/Inf Pow results. Inline tables with Pow
                    // expressions previously could cache NaN values
                    // that fed the hot path as silent wrong answers.
                    let r = a.powf(b);
                    if r.is_nan() || r.is_infinite() { 0.0 } else { r }
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
            let a = eval_table_expr(&w.un_op.arg, param_index, params)?;
            let r = match w.un_op.op {
                UnOp::Neg   => -a,
                UnOp::Exp   => a.exp(),
                UnOp::Log   => if a > 0.0 { a.ln() } else { f64::NEG_INFINITY },
                UnOp::Sqrt  => if a >= 0.0 { a.sqrt() } else { 0.0 },
                UnOp::Abs   => a.abs(),
                UnOp::Floor => a.floor(),
                UnOp::Ceil  => a.ceil(),
            };
            Ok(if r.is_nan() { 0.0 } else { r })
        }
        _ => Err(SimError::Validation(
            "unsupported expression type in table values (only Const and Param are valid)".to_string()
        )),
    }
}

pub struct CompiledModel {
    pub model: Arc<Model>,

    /// compartment name → index in the *combined* compartment list
    pub comp_index: HashMap<String, usize>,

    /// parameter name → index in the params slice passed to simulate
    pub param_index: HashMap<String, usize>,

    /// time_function name → index in model.time_functions
    pub time_func_index: HashMap<String, usize>,

    /// table name → index in model.tables
    pub table_index: HashMap<String, usize>,

    /// Indices (in the combined compartment list) of integer compartments,
    /// in model order.
    pub int_comp_indices: Vec<usize>,

    /// Indices (in the combined compartment list) of real compartments,
    /// in model order.
    pub real_comp_indices: Vec<usize>,

    /// For each integer compartment (by its local int-index), its global comp index.
    pub int_local_to_global: Vec<usize>,

    /// For each real compartment (by its local real-index), its global comp index.
    pub real_local_to_global: Vec<usize>,

    /// For a global compartment index: Some(local_int_idx) or None.
    pub global_to_int: Vec<Option<usize>>,

    /// For a global compartment index: Some(local_real_idx) or None.
    pub global_to_real: Vec<Option<usize>>,

    /// Default parameter values extracted from model.parameters, in param_index order.
    pub default_params: Vec<f64>,

    /// For each transition, pre-computed stoichiometry as (int_local_idx, delta).
    /// Real compartments cannot appear in stoichiometry (validator enforces this).
    pub transition_stoich: Vec<Vec<(usize, i64)>>,

    /// For each ODE equation, the local real-compartment index.
    pub ode_real_indices: Vec<usize>,

    /// Per-table evaluated values (params resolved at load time).
    /// Indexed in the same order as model.tables / table_index.
    pub table_values_cache: Vec<Vec<f64>>,

    /// Per-time-function resolved values (Expr fields evaluated at load time).
    /// Indexed in the same order as model.time_functions / time_func_index.
    pub time_func_cache: Vec<CompiledTimeFunc>,

    /// For each integer compartment (local index), the list of transition indices
    /// whose rate expression references that compartment.
    /// Used for sparse incremental propensity updates after stoichiometry changes.
    pub comp_to_transitions: Vec<Vec<usize>>,

    /// Indices of transitions whose rate expression contains a time function.
    /// These must be re-evaluated whenever simulation time advances.
    pub time_dep_transitions: Vec<usize>,

    /// For chain-binomial multinomial draws: transitions grouped by source
    /// compartment. Key = local int index of source compartment, value = list
    /// of transition indices that draw from it. Transitions with no source
    /// (inflows) are not included — they use Poisson draws directly.
    pub source_groups: Vec<(usize, Vec<usize>)>,

    /// Balance constraint: one compartment is overwritten at each substep
    /// to satisfy a population conservation expression.
    pub balance: Option<ResolvedBalance>,

    /// Precomputed fire steps for each intervention/event, snapped to
    /// the integer timestep grid. Key = intervention index, value = set
    /// of step numbers where the event fires. Eliminates floating-point
    /// tolerance issues (double-fires, zero-fires) by rounding fire times
    /// to the nearest integer step at model init.
    pub fire_steps: Vec<std::collections::BTreeSet<i64>>,

    /// Pre-resolved expression trees for all hot-path evaluations.
    pub resolved: ResolvedModel,
}

/// Pre-resolved balance constraint.
#[derive(Debug, Clone)]
pub struct ResolvedBalance {
    /// Local integer compartment index of the target (e.g., R).
    pub local_int_idx: usize,
    /// Pre-resolved expression (e.g., pop(t) - S - E - I).
    pub expr: ResolvedExpr,
}

/// All pre-resolved expression trees for hot-path evaluation.
/// Populated once during `CompiledModel::new()`, used by all simulation
/// backends and inference algorithms.
pub struct ResolvedModel {
    /// Per-transition resolved rate expression.
    pub rates: Vec<ResolvedExpr>,
    /// Per-transition resolved overdispersion σ² (None for Poisson/Deterministic).
    pub overdispersion: Vec<Option<ResolvedExpr>>,
    /// Per-transition resolved rate gradients: Vec of (param_name, resolved_expr).
    pub rate_grads: Vec<Vec<(String, ResolvedExpr)>>,
    /// Like rate_grads but with param names replaced by model param indices
    /// (indices into the `params` slice). Populated at construction time via
    /// `param_index`. Gradient terms whose name is not found in `param_index`
    /// are silently dropped — this only happens if the IR is malformed.
    pub rate_grads_indexed: Vec<Vec<(usize, ResolvedExpr)>>,
    /// Per-ODE-equation resolved derivative expression.
    pub ode_derivatives: Vec<ResolvedExpr>,
    /// Per-intervention, per-action resolved expression (count/fraction/value).
    pub intervention_exprs: Vec<Vec<ResolvedExpr>>,
}

impl CompiledModel {
    pub fn new(model: Model) -> Result<Self, SimError> {
        let n_comps = model.compartments.len();

        let mut comp_index = HashMap::with_capacity(n_comps);
        let mut int_local_to_global = Vec::new();
        let mut real_local_to_global = Vec::new();
        let mut global_to_int = vec![None; n_comps];
        let mut global_to_real = vec![None; n_comps];
        let mut int_comp_indices = Vec::new();
        let mut real_comp_indices = Vec::new();

        for (global, comp) in model.compartments.iter().enumerate() {
            comp_index.insert(comp.name.clone(), global);
            match comp.kind {
                CompartmentKind::Integer => {
                    let local = int_local_to_global.len();
                    int_local_to_global.push(global);
                    global_to_int[global] = Some(local);
                    int_comp_indices.push(global);
                }
                CompartmentKind::Real => {
                    let local = real_local_to_global.len();
                    real_local_to_global.push(global);
                    global_to_real[global] = Some(local);
                    real_comp_indices.push(global);
                }
            }
        }

        let mut param_index = HashMap::with_capacity(model.parameters.len());
        let mut default_params = Vec::with_capacity(model.parameters.len());
        for (i, p) in model.parameters.iter().enumerate() {
            param_index.insert(p.name.clone(), i);
            let v = p.value.ok_or_else(|| SimError::Validation(
                format!("parameter '{}' has no value; supply it via --params or --param", p.name)
            ))?;
            default_params.push(v);
        }

        let mut time_func_index = HashMap::with_capacity(model.time_functions.len());
        for (i, tf) in model.time_functions.iter().enumerate() {
            time_func_index.insert(tf.name.clone(), i);
        }

        let mut table_index = HashMap::with_capacity(model.tables.len());
        for (i, t) in model.tables.iter().enumerate() {
            table_index.insert(t.name.clone(), i);
        }

        // Pre-compute stoichiometry for integer compartments only.
        // Real compartments cannot appear in stoichiometry (IR validator enforces this).
        let mut transition_stoich = Vec::with_capacity(model.transitions.len());
        for t in &model.transitions {
            let mut stoich = Vec::new();
            for entry in &t.stoichiometry {
                let comp_name = &entry.0;
                let delta = entry.1;
                let global = comp_index.get(comp_name.as_str())
                    .copied()
                    .ok_or_else(|| SimError::UnknownCompartment(comp_name.clone()))?;
                if let Some(local) = global_to_int[global] {
                    stoich.push((local, delta));
                } else if global_to_real[global].is_some() {
                    // Real compartments cannot appear in stoichiometry
                    return Err(SimError::Validation(format!(
                        "real compartment '{}' cannot appear in stoichiometry", comp_name
                    )));
                }
            }
            transition_stoich.push(stoich);
        }

        // Build dependency graph for sparse propensity updates.
        // comp_to_transitions[local_int_idx] = [transition indices that reference it]
        // time_dep_transitions = [transition indices with TimeFunc in rate expression]
        let n_int_comps = int_local_to_global.len();
        let mut comp_to_transitions: Vec<Vec<usize>> = vec![vec![]; n_int_comps];
        let mut time_dep_transitions: Vec<usize> = Vec::new();
        for (tr_idx, tr) in model.transitions.iter().enumerate() {
            let mut deps = std::collections::HashSet::new();
            collect_int_comp_deps(&tr.rate, &comp_index, &global_to_int, &mut deps);
            for local_idx in deps {
                comp_to_transitions[local_idx].push(tr_idx);
            }
            if expr_has_time_func(&tr.rate) {
                time_dep_transitions.push(tr_idx);
            }
        }

        // Group transitions by source compartment for multinomial draws.
        //
        // Iteration order of `source_groups` drives RNG consumption in the
        // chain-binomial/PGAS/PMMH paths. HashMap::into_iter() is
        // nondeterministic, so we sort by src_local after collecting — same
        // seed + same model must always produce the same trajectory.
        let source_groups: Vec<(usize, Vec<usize>)> = {
            let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
            for (tr_idx, stoich) in transition_stoich.iter().enumerate() {
                if let Some(&(src_local, _)) = stoich.iter().find(|&&(_, d)| d < 0) {
                    groups.entry(src_local).or_default().push(tr_idx);
                }
            }
            let mut out: Vec<(usize, Vec<usize>)> = groups.into_iter().collect();
            out.sort_by_key(|(src, _)| *src);
            out
        };

        // Pre-compute ODE equation → real local index
        let mut ode_real_indices = Vec::with_capacity(model.ode_equations.len());
        for eq in &model.ode_equations {
            let global = comp_index.get(eq.compartment.as_str())
                .copied()
                .ok_or_else(|| SimError::UnknownCompartment(eq.compartment.clone()))?;
            let local = global_to_real[global]
                .ok_or_else(|| SimError::Validation(
                    format!("ODE equation references non-real compartment '{}'", eq.compartment)
                ))?;
            ode_real_indices.push(local);
        }

        // Evaluate table value expressions at load time using default params.
        // External tables (TableSource::External) are left empty here; the CLI
        // fills them in before calling CompiledModel::new() via --table flags.
        let mut table_values_cache: Vec<Vec<f64>> = Vec::with_capacity(model.tables.len());
        for table in &model.tables {
            match &table.source {
                ir::table::TableSource::Inline { values } => {
                    let vals: Result<Vec<f64>, SimError> = values.iter()
                        .map(|expr| eval_table_expr(expr, &param_index, &default_params))
                        .collect();
                    table_values_cache.push(vals?);
                }
                ir::table::TableSource::External { external } => {
                    // Rm5 in 2026-04-19 engine review: the CLI is
                    // responsible for replacing External with Inline
                    // before calling CompiledModel::new — unreplaced
                    // externals caused a panic when the empty cached
                    // vec was indexed during propensity eval. Fail
                    // loud at construction instead.
                    return Err(SimError::Validation(format!(
                        "table '{}' is declared external() but was not replaced \
                         before CompiledModel::new; populate TableSource::Inline \
                         from the runtime input first",
                        external
                    )));
                }
            }
        }

        // Evaluate time function Expr fields at load time using default params.
        let mut time_func_cache: Vec<CompiledTimeFunc> = Vec::with_capacity(model.time_functions.len());
        for tf in &model.time_functions {
            use ir::time_func::TimeFuncKind;
            let kind = match &tf.kind {
                TimeFuncKind::Sinusoidal(s) => CompiledTimeFuncKind::Sinusoidal {
                    amplitude: eval_table_expr(&s.amplitude, &param_index, &default_params)?,
                    period:    eval_table_expr(&s.period,    &param_index, &default_params)?,
                    phase:     eval_table_expr(&s.phase,     &param_index, &default_params)?,
                    baseline:  eval_table_expr(&s.baseline,  &param_index, &default_params)?,
                },
                TimeFuncKind::Piecewise(p) => {
                    let bps: Result<Vec<f64>, SimError> = p.breakpoints.iter()
                        .map(|e| eval_table_expr(e, &param_index, &default_params))
                        .collect();
                    let vals: Result<Vec<f64>, SimError> = p.values.iter()
                        .map(|e| eval_table_expr(e, &param_index, &default_params))
                        .collect();
                    CompiledTimeFuncKind::Piecewise { breakpoints: bps?, values: vals? }
                }
                TimeFuncKind::Interpolated(i) => {
                    let times: Result<Vec<f64>, SimError> = i.times.iter()
                        .map(|e| eval_table_expr(e, &param_index, &default_params))
                        .collect();
                    let vals: Result<Vec<f64>, SimError> = i.values.iter()
                        .map(|e| eval_table_expr(e, &param_index, &default_params))
                        .collect();
                    let ts = times?;
                    let vs = vals?;
                    match i.method {
                        ir::time_func::InterpMethod::Spline =>
                            CompiledTimeFuncKind::CubicSpline(CubicSpline::new(&ts, &vs)),
                        ir::time_func::InterpMethod::Linear =>
                            CompiledTimeFuncKind::Interpolated { times: ts, values: vs },
                        ir::time_func::InterpMethod::Constant =>
                            CompiledTimeFuncKind::Constant { times: ts, values: vs },
                    }
                }
                TimeFuncKind::Periodic(p) => {
                    let period = eval_table_expr(&p.period, &param_index, &default_params)?;
                    let vals: Result<Vec<f64>, SimError> = p.values.iter()
                        .map(|e| eval_table_expr(e, &param_index, &default_params))
                        .collect();
                    CompiledTimeFuncKind::Periodic { period, values: vals? }
                }
            };
            time_func_cache.push(CompiledTimeFunc { kind });
        }

        // Precompute fire steps (integer grid) for all interventions/events
        let fire_steps: Vec<std::collections::BTreeSet<i64>> = {
            use crate::intervention::intervention_fire_times;
            let dt = model.simulation.dt.unwrap_or(1.0);
            model.interventions.iter().map(|iv| {
                let times = intervention_fire_times(&iv.schedule);
                times.iter()
                    .map(|&ft| (ft / dt).round() as i64)
                    .collect()
            }).collect()
        };

        // ── Pre-resolve all expression trees ─────────────────────────────
        // Build ResolveCtx from the index maps we just constructed.
        let table_meta: Vec<(ir::table::OobPolicy, usize)> = model.tables.iter()
            .zip(&table_values_cache)
            .map(|(t, cached)| (t.out_of_bounds.clone(), cached.len()))
            .collect();

        let resolve_ctx = ResolveCtx {
            comp_index: &comp_index,
            param_index: &param_index,
            time_func_index: &time_func_index,
            table_index: &table_index,
            global_to_int: &global_to_int,
            global_to_real: &global_to_real,
            table_meta: &table_meta,
        };

        // Resolve balance constraint
        let balance = if let Some(ref bs) = model.balance {
            let global = *comp_index.get(bs.target.as_str())
                .ok_or_else(|| SimError::UnknownCompartment(bs.target.clone()))?;
            let local = global_to_int[global]
                .ok_or_else(|| SimError::Validation(
                    format!("balance target '{}' must be an integer compartment", bs.target)
                ))?;
            let resolved_expr = resolve_expr(&bs.expr, &resolve_ctx)?;
            Some(ResolvedBalance { local_int_idx: local, expr: resolved_expr })
        } else {
            None
        };

        // Resolve transition rates + overdispersion + rate_grad
        let rates: Vec<ResolvedExpr> = model.transitions.iter()
            .map(|tr| resolve_expr(&tr.rate, &resolve_ctx))
            .collect::<Result<_, _>>()?;

        let overdispersion: Vec<Option<ResolvedExpr>> = model.transitions.iter()
            .map(|tr| match &tr.draw_method {
                ir::transition::DrawMethod::Overdispersed(expr) =>
                    resolve_expr(expr, &resolve_ctx).map(Some),
                _ => Ok(None),
            })
            .collect::<Result<_, _>>()?;

        let rate_grads: Vec<Vec<(String, ResolvedExpr)>> = model.transitions.iter()
            .map(|tr| {
                tr.rate_grad.iter()
                    .map(|(name, expr)| {
                        resolve_expr(expr, &resolve_ctx).map(|r| (name.clone(), r))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<_, _>>()?;

        // Index-keyed form: resolve String keys to model param indices once at
        // construction time. Gradient terms with unknown names are dropped (they
        // indicate a malformed IR — all known names are in param_index).
        let rate_grads_indexed: Vec<Vec<(usize, ResolvedExpr)>> = rate_grads.iter()
            .map(|tr_grads| {
                tr_grads.iter()
                    .filter_map(|(name, expr)| {
                        param_index.get(name.as_str()).map(|&idx| (idx, expr.clone()))
                    })
                    .collect()
            })
            .collect();

        // Resolve ODE derivatives
        let ode_derivatives: Vec<ResolvedExpr> = model.ode_equations.iter()
            .map(|eq| resolve_expr(&eq.derivative, &resolve_ctx))
            .collect::<Result<_, _>>()?;

        // Resolve intervention action expressions
        let intervention_exprs: Vec<Vec<ResolvedExpr>> = model.interventions.iter()
            .map(|iv| {
                iv.actions.iter().map(|action| {
                    let expr = match action {
                        ir::intervention::Action::Add(a) => &a.count,
                        ir::intervention::Action::Set(s) => &s.value,
                        ir::intervention::Action::FractionTransfer(ft) => &ft.fraction,
                        ir::intervention::Action::AbsoluteTransfer(at) => &at.count,
                    };
                    resolve_expr(expr, &resolve_ctx)
                }).collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<_, _>>()?;

        let resolved = ResolvedModel {
            rates,
            overdispersion,
            rate_grads,
            rate_grads_indexed,
            ode_derivatives,
            intervention_exprs,
        };

        Ok(CompiledModel {
            model: Arc::new(model),
            comp_index,
            param_index,
            time_func_index,
            table_index,
            int_comp_indices,
            real_comp_indices,
            int_local_to_global,
            real_local_to_global,
            global_to_int,
            global_to_real,
            default_params,
            transition_stoich,
            ode_real_indices,
            table_values_cache,
            time_func_cache,
            source_groups,
            comp_to_transitions,
            time_dep_transitions,
            balance,
            fire_steps,
            resolved,
        })
    }

    /// Features this model requires from a backend.
    pub fn required_capabilities(&self) -> crate::Capabilities {
        let mut caps = crate::Capabilities::empty();
        if self.model.transitions.iter().any(|t| matches!(t.draw_method, ir::transition::DrawMethod::Overdispersed(_))) {
            caps |= crate::Capabilities::OVERDISPERSION;
        }
        if !self.real_comp_indices.is_empty() {
            caps |= crate::Capabilities::REAL_COMPARTMENTS;
        }
        caps
    }

    /// Build the initial state from model.initial_conditions + params.
    pub fn initial_state(
        &self,
        params: &[f64],
    ) -> Result<(IntState, RealState), SimError> {
        use ir::model::InitialConditions;
        use crate::propensity::{eval_expr, EvalCtx};

        let n_int = self.int_local_to_global.len();
        let n_real = self.real_local_to_global.len();
        let mut int_counts = vec![0i64; n_int];
        let mut real_values = vec![0.0f64; n_real];

        // Temporary zero state for evaluating parameterized ICs
        let zero_int = IntState::new(n_int);
        let zero_real = RealState::new(n_real);

        match &self.model.initial_conditions {
            InitialConditions::Explicit(map) => {
                for (name, val) in map {
                    let global = self.comp_index.get(name.as_str())
                        .copied()
                        .ok_or_else(|| SimError::UnknownCompartment(name.clone()))?;
                    if let Some(local) = self.global_to_int[global] {
                        int_counts[local] = *val as i64;
                    } else if let Some(local) = self.global_to_real[global] {
                        real_values[local] = *val;
                    }
                }
            }
            InitialConditions::Parameterized(map) => {
                let ctx = EvalCtx { model: self, int_s: &zero_int, real_s: &zero_real, params, t: 0.0 , projected: None, int_float_override: None };
                for (name, expr) in map {
                    let global = self.comp_index.get(name.as_str())
                        .copied()
                        .ok_or_else(|| SimError::UnknownCompartment(name.clone()))?;
                    let v = eval_expr(expr, &ctx)?;
                    if let Some(local) = self.global_to_int[global] {
                        int_counts[local] = v.round() as i64;
                    } else if let Some(local) = self.global_to_real[global] {
                        real_values[local] = v;
                    }
                }
            }
            InitialConditions::FromDistribution(_) => {
                // RC3 in 2026-04-19 engine review: this was a silent
                // fall-through to "all zeros," which would start every
                // compartment at 0 and not tell anyone. Hard-fail until
                // the inference-side prior sampling path is wired in.
                return Err(SimError::Validation(
                    "initial_conditions::from_distribution is not yet \
                     supported at the sim layer; draw initial values \
                     via the inference pipeline and pass them in as \
                     explicit initial_conditions instead".to_string()
                ));
            }
        }

        Ok((IntState::from_vec(int_counts), RealState::from_vec(real_values)))
    }
}
