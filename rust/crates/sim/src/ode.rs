use crate::{
    compiled_model::CompiledModel,
    config::{OdeConfig, SimConfig},
    error::SimError,
    intervention::{all_intervention_times, apply_interventions_at},
    output::output_times as get_output_times,
    propensity::{eval_expr, eval_propensities, EvalCtx},
    simulate::Simulate,
    state::{FlowVec, IntState, RealState, Snapshot, Trajectory},
};

pub struct OdeSim;

impl Simulate for OdeSim {
    fn run(
        &self,
        model: &CompiledModel,
        params: &[f64],
        _seed: u64,
        config: &SimConfig,
    ) -> Result<Trajectory, SimError> {
        let cfg = match config {
            SimConfig::Ode(c) => c,
            _ => return Err(SimError::ConfigMismatch {
                expected: "Ode",
                got: config.variant_name(),
            }),
        };
        run_ode(model, params, cfg)
    }

    fn capabilities(&self) -> crate::Capabilities {
        crate::Capabilities::REAL_COMPARTMENTS
    }

    fn name(&self) -> &'static str { "ode" }
}

/// Evaluate ODE derivatives at the current (int_vals, real_vals) state.
///
/// Integer compartments are rounded to i64 when constructing the EvalCtx,
/// introducing O(1/N) relative error in propensity evaluation. This is
/// negligible for N > ~100 but can cause premature extinction for very small
/// compartment values (< ~10). See docs/runtimes.md for full discussion.
fn ode_derivs(
    model: &CompiledModel,
    int_vals: &[f64],
    real_vals: &[f64],
    params: &[f64],
    t: f64,
    d_int: &mut [f64],
    d_real: &mut [f64],
) -> Result<(), SimError> {
    let int_s = IntState::from_vec(
        int_vals.iter().map(|&x| x.max(0.0).round() as i64).collect()
    );
    let real_s = RealState::from_vec(real_vals.to_vec());

    // Integer compartment derivatives from transition stoichiometry × rate.
    let mut propensities = Vec::with_capacity(model.model.transitions.len());
    eval_propensities(model, &int_s, &real_s, params, t, &mut propensities)?;

    for v in d_int.iter_mut() { *v = 0.0; }
    for (tr_idx, stoich) in model.transition_stoich.iter().enumerate() {
        let rate = propensities[tr_idx];
        for &(local, delta) in stoich {
            d_int[local] += delta as f64 * rate;
        }
    }

    // Real compartment derivatives from explicit ODE equations.
    for v in d_real.iter_mut() { *v = 0.0; }
    let ctx = EvalCtx { model, int_s: &int_s, real_s: &real_s, params, t };
    for (eq_idx, eq) in model.model.ode_equations.iter().enumerate() {
        let local = model.ode_real_indices[eq_idx];
        d_real[local] = eval_expr(&eq.derivative, &ctx)?;
    }

    Ok(())
}

/// Single RK4 step over the combined (int_vals, real_vals) state.
fn rk4_step(
    model: &CompiledModel,
    int_vals: &mut Vec<f64>,
    real_vals: &mut Vec<f64>,
    params: &[f64],
    t: f64,
    dt: f64,
) -> Result<(), SimError> {
    let ni = int_vals.len();
    let nr = real_vals.len();

    let mut di = vec![0.0f64; ni];
    let mut dr = vec![0.0f64; nr];

    // k1
    ode_derivs(model, int_vals, real_vals, params, t, &mut di, &mut dr)?;
    let k1i: Vec<f64> = di.clone();
    let k1r: Vec<f64> = dr.clone();

    // k2
    let s2i: Vec<f64> = int_vals.iter().zip(&k1i).map(|(x, k)| x + 0.5 * dt * k).collect();
    let s2r: Vec<f64> = real_vals.iter().zip(&k1r).map(|(x, k)| x + 0.5 * dt * k).collect();
    ode_derivs(model, &s2i, &s2r, params, t + 0.5 * dt, &mut di, &mut dr)?;
    let k2i: Vec<f64> = di.clone();
    let k2r: Vec<f64> = dr.clone();

    // k3
    let s3i: Vec<f64> = int_vals.iter().zip(&k2i).map(|(x, k)| x + 0.5 * dt * k).collect();
    let s3r: Vec<f64> = real_vals.iter().zip(&k2r).map(|(x, k)| x + 0.5 * dt * k).collect();
    ode_derivs(model, &s3i, &s3r, params, t + 0.5 * dt, &mut di, &mut dr)?;
    let k3i: Vec<f64> = di.clone();
    let k3r: Vec<f64> = dr.clone();

    // k4
    let s4i: Vec<f64> = int_vals.iter().zip(&k3i).map(|(x, k)| x + dt * k).collect();
    let s4r: Vec<f64> = real_vals.iter().zip(&k3r).map(|(x, k)| x + dt * k).collect();
    ode_derivs(model, &s4i, &s4r, params, t + dt, &mut di, &mut dr)?;
    let k4i = &di;
    let k4r = &dr;

    // Combine
    for i in 0..ni {
        int_vals[i] += dt / 6.0 * (k1i[i] + 2.0 * k2i[i] + 2.0 * k3i[i] + k4i[i]);
        int_vals[i] = int_vals[i].max(0.0);
    }
    for i in 0..nr {
        real_vals[i] += dt / 6.0 * (k1r[i] + 2.0 * k2r[i] + 2.0 * k3r[i] + k4r[i]);
        real_vals[i] = real_vals[i].max(0.0);
    }

    Ok(())
}

/// Convert (int_vals, real_vals) floats to the (IntState, RealState) used by
/// the intervention machinery and output snapshots.
fn to_states(int_vals: &[f64], real_vals: &[f64]) -> (IntState, RealState) {
    let int_s = IntState::from_vec(int_vals.iter().map(|&x| x.max(0.0).round() as i64).collect());
    let real_s = RealState::from_vec(real_vals.to_vec());
    (int_s, real_s)
}

fn run_ode(
    model: &CompiledModel,
    params: &[f64],
    cfg: &OdeConfig,
) -> Result<Trajectory, SimError> {
    let (int_s0, real_s0) = model.initial_state(params)?;
    let mut int_vals: Vec<f64> = int_s0.counts.iter().map(|&c| c as f64).collect();
    let mut real_vals: Vec<f64> = real_s0.values.clone();

    let n_transitions = model.model.transitions.len();
    let output_times = get_output_times(&model.model.output.times);
    let mut output_idx = 0;
    let iv_times = all_intervention_times(model);
    let mut iv_idx = 0;

    let mut traj = Trajectory::new();
    // Accumulated continuous flows (rate × dt); rounded to u64 at each snapshot.
    let mut flow_acc: Vec<f64> = vec![0.0; n_transitions];
    let mut t = cfg.t_start;

    // Record initial snapshot
    let snapshot_flows = |flow_acc: &[f64]| {
        FlowVec::from_vec(flow_acc.iter().map(|&x| x.round() as u64).collect())
    };

    if output_idx < output_times.len() && output_times[output_idx] <= t + 1e-12 {
        let (is, rs) = to_states(&int_vals, &real_vals);
        traj.push(Snapshot {
            t,
            int_state: is,
            real_state: rs,
            flows: snapshot_flows(&flow_acc),
        });
        for v in flow_acc.iter_mut() { *v = 0.0; }
        output_idx += 1;
    }

    while t < cfg.t_end {
        let next_boundary = {
            let out_t = output_times.get(output_idx).copied().unwrap_or(f64::INFINITY);
            let iv_t  = iv_times.get(iv_idx).copied().unwrap_or(f64::INFINITY);
            cfg.t_end.min(out_t).min(iv_t)
        };
        let dt = cfg.dt.min(next_boundary - t);

        if dt <= 1e-15 {
            // At a boundary — apply intervention or record output
            if iv_times.get(iv_idx).copied().is_some_and(|iv| (iv - t).abs() < 1e-10) {
                let (mut is, mut rs) = to_states(&int_vals, &real_vals);
                apply_interventions_at(t, model, &mut is, &mut rs, params, 1e-10)?;
                int_vals = is.counts.iter().map(|&c| c as f64).collect();
                real_vals = rs.values.clone();
                while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + 1e-10 { iv_idx += 1; }
            }
            while output_idx < output_times.len() && output_times[output_idx] <= t + 1e-12 {
                let (is, rs) = to_states(&int_vals, &real_vals);
                traj.push(Snapshot {
                    t: output_times[output_idx],
                    int_state: is,
                    real_state: rs,
                    flows: snapshot_flows(&flow_acc),
                });
                for v in flow_acc.iter_mut() { *v = 0.0; }
                output_idx += 1;
            }
            if t >= cfg.t_end { break; }
            continue;
        }

        // Accumulate flows before the step (propensities × dt approximation)
        {
            let (is, rs) = to_states(&int_vals, &real_vals);
            let mut propensities = Vec::with_capacity(n_transitions);
            eval_propensities(model, &is, &rs, params, t, &mut propensities)?;
            for (i, &p) in propensities.iter().enumerate() {
                flow_acc[i] += p * dt;
            }
        }

        rk4_step(model, &mut int_vals, &mut real_vals, params, t, dt)?;
        t += dt;

        // Apply intervention if now at that time
        if iv_times.get(iv_idx).copied().is_some_and(|iv| (iv - t).abs() < 1e-10) {
            let (mut is, mut rs) = to_states(&int_vals, &real_vals);
            apply_interventions_at(t, model, &mut is, &mut rs, params, 1e-10)?;
            int_vals = is.counts.iter().map(|&c| c as f64).collect();
            real_vals = rs.values.clone();
            while iv_idx < iv_times.len() && iv_times[iv_idx] <= t + 1e-10 { iv_idx += 1; }
        }

        // Record outputs
        while output_idx < output_times.len() && output_times[output_idx] <= t + 1e-12 {
            let (is, rs) = to_states(&int_vals, &real_vals);
            traj.push(Snapshot {
                t: output_times[output_idx],
                int_state: is,
                real_state: rs,
                flows: snapshot_flows(&flow_acc),
            });
            for v in flow_acc.iter_mut() { *v = 0.0; }
            output_idx += 1;
        }
    }

    // Flush any remaining output times
    while output_idx < output_times.len() {
        let (is, rs) = to_states(&int_vals, &real_vals);
        traj.push(Snapshot {
            t: output_times[output_idx],
            int_state: is,
            real_state: rs,
            flows: snapshot_flows(&flow_acc),
        });
        for v in flow_acc.iter_mut() { *v = 0.0; }
        output_idx += 1;
    }

    Ok(traj)
}
