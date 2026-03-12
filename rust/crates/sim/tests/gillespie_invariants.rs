//! Deterministic invariant tests for the Gillespie backend (§A.1).

use std::path::Path;
use sim::{
    compiled_model::CompiledModel,
    config::{GillespieConfig, SimConfig},
    simulate::Simulate,
    GillespieSim,
};

fn load_model(path: &str) -> ir::Model {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("could not read {}", path));
    serde_json::from_str(&contents)
        .unwrap_or_else(|e| panic!("failed to parse {}: {}", path, e))
}

fn golden_path(name: &str) -> String {
    // Relative to workspace root: ir/golden/<name>.ir.json
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // sim is at rust/crates/sim; golden files are at rust/../../ir/golden/
    Path::new(&manifest)
        .join("../../../ir/golden")
        .join(format!("{}.ir.json", name))
        .to_string_lossy()
        .to_string()
}

fn gillespie_config(model: &ir::Model) -> SimConfig {
    SimConfig::Gillespie(GillespieConfig {
        t_start: model.simulation.t_start,
        t_end: model.simulation.t_end,
        output_dt: None,
    })
}

#[test]
fn test_sir_basic_non_negativity() {
    let model = load_model(&golden_path("sir_basic"));
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = &compiled.default_params.clone();
    let config = gillespie_config(&model);

    for seed in 0..20u64 {
        let traj = GillespieSim.run(&compiled, params, seed, &config).unwrap();
        for snap in &traj.snapshots {
            for &c in &snap.int_state.counts {
                assert!(c >= 0, "negative integer compartment at t={}", snap.t);
            }
            for &v in &snap.real_state.values {
                assert!(v >= 0.0, "negative real compartment at t={}", snap.t);
            }
        }
    }
}

#[test]
fn test_sir_basic_population_conservation() {
    let model = load_model(&golden_path("sir_basic"));
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = &compiled.default_params.clone();
    let config = gillespie_config(&model);

    for seed in 0..10u64 {
        let traj = GillespieSim.run(&compiled, params, seed, &config).unwrap();
        if traj.snapshots.is_empty() { continue; }
        let n0: i64 = traj.snapshots[0].int_state.total();
        for snap in &traj.snapshots {
            let n = snap.int_state.total();
            assert_eq!(n, n0, "population not conserved at t={}: expected {}, got {}", snap.t, n0, n);
        }
    }
}

#[test]
fn test_two_state_population_conservation() {
    let model = load_model(&golden_path("two_state"));
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = &compiled.default_params.clone();
    let config = gillespie_config(&model);

    let traj = GillespieSim.run(&compiled, params, 42, &config).unwrap();
    let n0: i64 = traj.snapshots[0].int_state.total();
    for snap in &traj.snapshots {
        assert_eq!(snap.int_state.total(), n0, "population changed at t={}", snap.t);
    }
}

#[test]
fn test_pure_death_non_negativity_many_seeds() {
    let model = load_model(&golden_path("pure_death"));
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = &compiled.default_params.clone();
    let config = gillespie_config(&model);

    for seed in 0..100u64 {
        let traj = GillespieSim.run(&compiled, params, seed, &config).unwrap();
        for snap in &traj.snapshots {
            for &c in &snap.int_state.counts {
                assert!(c >= 0, "negative compartment at t={} seed={}", snap.t, seed);
            }
        }
    }
}

#[test]
fn test_propensity_non_negativity() {
    use sim::propensity::eval_propensities;
    use sim::state::{IntState, RealState};

    let model = load_model(&golden_path("sir_basic"));
    let compiled = CompiledModel::new(model).unwrap();
    let params = &compiled.default_params.clone();

    let mut propensities = Vec::new();
    // Test at several states
    let test_states = vec![
        vec![990i64, 10, 0],
        vec![500, 200, 300],
        vec![0, 0, 1000],   // absorbing state
        vec![1, 0, 999],    // I=0, propensity should be 0
    ];
    for counts in test_states {
        let int_s = IntState::from_vec(counts);
        let real_s = RealState::new(0);
        eval_propensities(&compiled, &int_s, &real_s, params, 0.0, &mut propensities).unwrap();
        for &p in &propensities {
            assert!(p >= 0.0, "negative propensity: {}", p);
        }
    }
}

#[test]
fn test_determinism_same_seed() {
    let model = load_model(&golden_path("sir_basic"));
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = &compiled.default_params.clone();
    let config = gillespie_config(&model);

    let traj1 = GillespieSim.run(&compiled, params, 42, &config).unwrap();
    let traj2 = GillespieSim.run(&compiled, params, 42, &config).unwrap();

    assert_eq!(traj1.snapshots.len(), traj2.snapshots.len());
    for (s1, s2) in traj1.snapshots.iter().zip(&traj2.snapshots) {
        assert_eq!(s1.int_state, s2.int_state, "trajectories differ at t={}", s1.t);
    }
}

#[test]
fn test_different_seeds_different_trajectories() {
    let model = load_model(&golden_path("sir_basic"));
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = &compiled.default_params.clone();
    let config = gillespie_config(&model);

    let traj1 = GillespieSim.run(&compiled, params, 1, &config).unwrap();
    let traj2 = GillespieSim.run(&compiled, params, 2, &config).unwrap();

    // Very unlikely to be identical
    let identical = traj1.snapshots.iter().zip(&traj2.snapshots)
        .all(|(s1, s2)| s1.int_state == s2.int_state);
    assert!(!identical, "different seeds produced identical trajectories");
}
