//! Tests for the ODE (RK4) simulation backend.

use std::path::PathBuf;
use sim::{
    compiled_model::CompiledModel,
    config::{OdeConfig, SimConfig},
    simulate::Simulate,
    OdeSim,
};

fn golden_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    PathBuf::from(&manifest).join("../../../ir/golden")
}

fn load_model(name: &str) -> ir::Model {
    let path = golden_dir().join(format!("{}.ir.json", name));
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("could not read {}", name));
    serde_json::from_str(&contents)
        .unwrap_or_else(|e| panic!("failed to parse {}: {}", name, e))
}

fn set_params(model: &mut ir::Model, vals: &[(&str, f64)]) {
    for p in &mut model.parameters {
        for &(name, v) in vals {
            if p.name == name { p.value = Some(v); }
        }
    }
}

fn ode_config(model: &ir::Model, dt: f64) -> SimConfig {
    SimConfig::Ode(OdeConfig {
        t_start: model.simulation.t_start,
        t_end:   model.simulation.t_end,
        dt,
    })
}

// ── Determinism ───────────────────────────────────────────────────────────────

/// ODE is deterministic: same inputs always produce the same trajectory,
/// regardless of seed.
#[test]
fn ode_deterministic_regardless_of_seed() {
    let mut model = load_model("sir_basic");
    set_params(&mut model, &[("beta", 0.3), ("gamma", 0.1), ("N0", 1000.0), ("I0", 10.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();
    let config = ode_config(&model, 1.0);

    let traj1 = OdeSim.run(&compiled, &params, 1,  &config).unwrap();
    let traj2 = OdeSim.run(&compiled, &params, 42, &config).unwrap();
    let traj3 = OdeSim.run(&compiled, &params, 99, &config).unwrap();

    assert_eq!(traj1.snapshots.len(), traj2.snapshots.len());
    for (s1, s2) in traj1.snapshots.iter().zip(&traj2.snapshots) {
        assert_eq!(s1.int_state.counts, s2.int_state.counts,
            "ODE outputs differ between seeds at t={}", s1.t);
    }
    for (s1, s3) in traj1.snapshots.iter().zip(&traj3.snapshots) {
        assert_eq!(s1.int_state.counts, s3.int_state.counts,
            "ODE outputs differ between seeds at t={}", s1.t);
    }
}

// ── Population conservation ───────────────────────────────────────────────────

/// For a closed SIR (no births/deaths), S + I + R must remain constant.
/// The ODE backend should conserve population to floating-point precision.
#[test]
fn ode_sir_population_conserved() {
    let mut model = load_model("sir_basic");
    set_params(&mut model, &[("beta", 0.3), ("gamma", 0.1), ("N0", 10000.0), ("I0", 100.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();
    let config = ode_config(&model, 0.5);

    let traj = OdeSim.run(&compiled, &params, 0, &config).unwrap();
    assert!(!traj.snapshots.is_empty());

    let n0: i64 = traj.snapshots[0].int_state.total();
    for snap in &traj.snapshots {
        let n = snap.int_state.total();
        // Allow rounding slack of 1 because int state is rounded from floats at output
        assert!((n - n0).abs() <= 1,
            "population not conserved at t={}: expected ~{}, got {}", snap.t, n0, n);
    }
}

// ── Non-negativity ────────────────────────────────────────────────────────────

#[test]
fn ode_non_negativity() {
    let mut model = load_model("sir_basic");
    set_params(&mut model, &[("beta", 0.5), ("gamma", 0.1), ("N0", 1000.0), ("I0", 50.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();
    let config = ode_config(&model, 1.0);

    let traj = OdeSim.run(&compiled, &params, 0, &config).unwrap();
    for snap in &traj.snapshots {
        for &c in &snap.int_state.counts {
            assert!(c >= 0, "negative integer compartment at t={}: {}", snap.t, c);
        }
        for &v in &snap.real_state.values {
            assert!(v >= 0.0, "negative real compartment at t={}: {}", snap.t, v);
        }
    }
}

// ── Epidemic dynamics ─────────────────────────────────────────────────────────

/// For R₀ > 1, infected compartment must rise above initial value before falling.
/// Recovered must be monotonically non-decreasing.
/// Susceptible must be monotonically non-increasing.
#[test]
fn ode_sir_epidemic_shape() {
    let mut model = load_model("sir_basic");
    // R0 = beta/gamma = 3 — well above threshold
    set_params(&mut model, &[("beta", 0.3), ("gamma", 0.1), ("N0", 10000.0), ("I0", 10.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();
    let config = ode_config(&model, 0.1);

    let traj = OdeSim.run(&compiled, &params, 0, &config).unwrap();
    assert!(traj.snapshots.len() >= 2);

    // Identify S, I, R by position (model order: S=0, I=1, R=2)
    let s_idx = 0usize;
    let i_idx = 1usize;
    let r_idx = 2usize;

    let i_initial = traj.snapshots[0].int_state.counts[i_idx];
    let i_peak = traj.snapshots.iter()
        .map(|s| s.int_state.counts[i_idx])
        .max()
        .unwrap();
    assert!(i_peak > i_initial, "I never grew above initial value — epidemic didn't take off");

    // R must be non-decreasing
    let mut prev_r = 0i64;
    for snap in &traj.snapshots {
        let r = snap.int_state.counts[r_idx];
        assert!(r >= prev_r, "R decreased at t={}: {} < {}", snap.t, r, prev_r);
        prev_r = r;
    }

    // S must be non-increasing
    let s0 = traj.snapshots[0].int_state.counts[s_idx];
    let mut prev_s = s0;
    for snap in &traj.snapshots {
        let s = snap.int_state.counts[s_idx];
        assert!(s <= prev_s + 1, "S increased at t={}: {} > {}", snap.t, s, prev_s);
        prev_s = s;
    }

    // Some infection must have occurred by end
    let r_final = traj.snapshots.last().unwrap().int_state.counts[r_idx];
    assert!(r_final > 1000, "too few recoveries by end: R={} (expected >1000 for R0=3)", r_final);
}

/// For R₀ < 1, the epidemic must fade out — I should be lower at the end
/// than at the start (no exponential growth).
#[test]
fn ode_sir_subcritical_fades() {
    let mut model = load_model("sir_basic");
    // R0 = 0.3/0.5 = 0.6 — below threshold
    set_params(&mut model, &[("beta", 0.3), ("gamma", 0.5), ("N0", 10000.0), ("I0", 100.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();
    let config = ode_config(&model, 0.1);

    let traj = OdeSim.run(&compiled, &params, 0, &config).unwrap();
    let i_initial = traj.snapshots[0].int_state.counts[1];
    let i_final   = traj.snapshots.last().unwrap().int_state.counts[1];
    assert!(i_final < i_initial,
        "subcritical epidemic grew: I went from {} to {}", i_initial, i_final);
}

// ── dt convergence ────────────────────────────────────────────────────────────

/// Smaller dt should give a trajectory closer to the dt=0.01 reference.
/// Tests that RK4 error decreases as dt shrinks (order-of-magnitude check).
#[test]
fn ode_dt_convergence() {
    let mut model = load_model("sir_basic");
    set_params(&mut model, &[("beta", 0.3), ("gamma", 0.1), ("N0", 10000.0), ("I0", 100.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();

    // Reference: very fine dt
    let traj_ref = OdeSim.run(&compiled, &params, 0, &ode_config(&model, 0.01)).unwrap();
    let traj_med = OdeSim.run(&compiled, &params, 0, &ode_config(&model, 0.5)).unwrap();
    let traj_crs = OdeSim.run(&compiled, &params, 0, &ode_config(&model, 2.0)).unwrap();

    // Compare final I value
    let i_ref = traj_ref.snapshots.last().unwrap().int_state.counts[1] as f64;
    let i_med = traj_med.snapshots.last().unwrap().int_state.counts[1] as f64;
    let i_crs = traj_crs.snapshots.last().unwrap().int_state.counts[1] as f64;

    let err_med = (i_med - i_ref).abs();
    let err_crs = (i_crs - i_ref).abs();

    // Coarser dt should have larger error (unless both are already at machine precision)
    // Allow for the possibility that both are very accurate; just require medium ≤ coarse
    assert!(err_med <= err_crs + 2.0,
        "finer dt not more accurate: err(dt=0.5)={}, err(dt=2.0)={}", err_med, err_crs);
}

// ── Output count ─────────────────────────────────────────────────────────────

/// The number of output snapshots should match the model's output schedule.
#[test]
fn ode_output_count_matches_schedule() {
    let mut model = load_model("sir_basic");
    set_params(&mut model, &[("beta", 0.3), ("gamma", 0.1), ("N0", 1000.0), ("I0", 10.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();

    let traj = OdeSim.run(&compiled, &params, 0, &ode_config(&model, 1.0)).unwrap();

    // sir_basic runs from 0 to 80 days, output every 1 day → 81 snapshots (0..=80)
    assert_eq!(traj.snapshots.len(), 81,
        "expected 81 snapshots (days 0..=80), got {}", traj.snapshots.len());
}

// ── ODE vs Gillespie agreement at large N ─────────────────────────────────────

/// At large N, ODE and Gillespie means should agree to within ~1%.
/// We run a single Gillespie trajectory (not an ensemble mean) so we allow
/// wider tolerance for stochastic noise.
#[test]
fn ode_agrees_with_gillespie_large_n() {
    use sim::{config::{GillespieConfig, SimConfig as SC}, GillespieSim};

    let mut model = load_model("sir_basic");
    set_params(&mut model, &[("beta", 0.3), ("gamma", 0.1), ("N0", 100_000.0), ("I0", 1000.0)]);
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();

    let ode_traj = OdeSim.run(&compiled, &params, 0, &ode_config(&model, 0.1)).unwrap();
    let ssa_traj = GillespieSim.run(
        &compiled, &params, 42,
        &SC::Gillespie(GillespieConfig { t_start: model.simulation.t_start, t_end: model.simulation.t_end, output_dt: None }),
    ).unwrap();

    // Compare final S values: within 5% of N (stochastic noise at N=100k is ~0.3%)
    let s_ode = ode_traj.snapshots.last().unwrap().int_state.counts[0] as f64;
    let s_ssa = ssa_traj.snapshots.last().unwrap().int_state.counts[0] as f64;
    let n = 100_000.0f64;
    let rel_diff = (s_ode - s_ssa).abs() / n;
    assert!(rel_diff < 0.05,
        "ODE and Gillespie final S differ by {:.1}%: ODE={}, SSA={}", rel_diff * 100.0, s_ode, s_ssa);
}
