use crate::{
    compiled_model::CompiledModel,
    error::SimError,
    propensity::{eval_expr, EvalCtx},
    state::{IntState, RealState},
};

/// RK4 step for all real compartments.
/// Integer state is held fixed throughout (PDMP semantics).
pub fn rk4_step(
    model: &CompiledModel,
    int_s: &IntState,
    real_s: &mut RealState,
    params: &[f64],
    t: f64,
    dt: f64,
) -> Result<(), SimError> {
    let n = real_s.values.len();
    if n == 0 { return Ok(()); }

    // k1
    let k1 = eval_ode_derivs(model, int_s, real_s, params, t)?;

    // k2
    let mut s2 = RealState::from_vec(
        real_s.values.iter().zip(&k1).map(|(x, k)| x + 0.5 * dt * k).collect()
    );
    s2.clamp_nonneg();
    let k2 = eval_ode_derivs(model, int_s, &s2, params, t + 0.5 * dt)?;

    // k3
    let mut s3 = RealState::from_vec(
        real_s.values.iter().zip(&k2).map(|(x, k)| x + 0.5 * dt * k).collect()
    );
    s3.clamp_nonneg();
    let k3 = eval_ode_derivs(model, int_s, &s3, params, t + 0.5 * dt)?;

    // k4
    let mut s4 = RealState::from_vec(
        real_s.values.iter().zip(&k3).map(|(x, k)| x + dt * k).collect()
    );
    s4.clamp_nonneg();
    let k4 = eval_ode_derivs(model, int_s, &s4, params, t + dt)?;

    // Combine
    for i in 0..n {
        real_s.values[i] += dt / 6.0 * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]);
    }
    Ok(())
}

fn eval_ode_derivs(
    model: &CompiledModel,
    int_s: &IntState,
    real_s: &RealState,
    params: &[f64],
    t: f64,
) -> Result<Vec<f64>, SimError> {
    let ctx = EvalCtx { model, int_s, real_s, params, t };
    let mut derivs = vec![0.0; model.ode_real_indices.len()];
    for (i, eq) in model.model.ode_equations.iter().enumerate() {
        derivs[i] = eval_expr(&eq.derivative, &ctx)?;
    }
    Ok(derivs)
}
