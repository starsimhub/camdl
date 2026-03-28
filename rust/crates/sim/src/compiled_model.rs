use std::collections::HashMap;
use std::sync::Arc;
use ir::{Model, model::CompartmentKind};
use ir::expr::{BinOp, Expr, UnOp};
use ir::time_func::InterpMethod;
use crate::error::SimError;
use crate::state::{IntState, RealState};

/// A time function with all Expr fields resolved to concrete f64 values.
#[derive(Debug, Clone)]
pub enum CompiledTimeFuncKind {
    Sinusoidal { amplitude: f64, period: f64, phase: f64, baseline: f64 },
    Piecewise   { breakpoints: Vec<f64>, values: Vec<f64> },
    Interpolated { times: Vec<f64>, values: Vec<f64> },
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
        if n == 2 {
            // Degenerate: linear interpolation
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
/// Table values may only reference constants and parameters.
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
                BinOp::Pow => a.powf(b),
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

        // Group transitions by source compartment for multinomial draws
        let source_groups: Vec<(usize, Vec<usize>)> = {
            let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
            for (tr_idx, stoich) in transition_stoich.iter().enumerate() {
                if let Some(&(src_local, _)) = stoich.iter().find(|&&(_, d)| d < 0) {
                    groups.entry(src_local).or_default().push(tr_idx);
                }
            }
            groups.into_iter().collect()
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
                    // Placeholder — CLI must replace these before simulation.
                    // If still empty at simulation time, propensity eval will error.
                    let _ = external;
                    table_values_cache.push(vec![]);
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
        })
    }

    /// Features this model requires from a backend.
    pub fn required_capabilities(&self) -> crate::Capabilities {
        let mut caps = crate::Capabilities::empty();
        if self.model.transitions.iter().any(|t| t.overdispersion.is_some()) {
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
                let ctx = EvalCtx { model: self, int_s: &zero_int, real_s: &zero_real, params, t: 0.0 };
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
                // Not supported in sim at runtime; use default zeros
            }
        }

        Ok((IntState::from_vec(int_counts), RealState::from_vec(real_values)))
    }
}
