use std::collections::HashMap;
use std::sync::Arc;
use ir::{Model, model::CompartmentKind};
use crate::error::SimError;
use crate::state::{IntState, RealState};

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
            default_params.push(p.value);
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
        })
    }

    /// Build the initial state from model.initial_conditions + params.
    pub fn initial_state(
        &self,
        params: &[f64],
    ) -> Result<(IntState, RealState), SimError> {
        use ir::model::InitialConditions;
        use crate::propensity::eval_expr;

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
                for (name, expr) in map {
                    let global = self.comp_index.get(name.as_str())
                        .copied()
                        .ok_or_else(|| SimError::UnknownCompartment(name.clone()))?;
                    let v = eval_expr(expr, self, &zero_int, &zero_real, params, 0.0)?;
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
