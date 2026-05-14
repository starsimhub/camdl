//! Multi-source transition conservation tests (wave 1 / malaria #1).
//!
//! For the bimolecular reaction `A + B --> C @ k * A * B / N`, every
//! firing must atomically decrement A and B together and increment C.
//! If firing is NOT atomic (A decremented but B missed, or vice versa),
//! these invariants break. Exercises all three stochastic backends
//! because Gillespie, tau-leap, and chain-binomial each apply
//! stoichiometry through different code paths.
//!
//!   A(t) + C(t) = A(0)   (every A consumed became a C)
//!   B(t) + C(t) = B(0)   (every B consumed became a C)
//!   A(0) - A(t) = B(0) - B(t)   (co-decrement — the atomicity invariant)

use std::path::Path;
use sim::{
    compiled_model::CompiledModel,
    config::{ChainBinomialConfig, GillespieConfig, SimConfig, TauLeapConfig},
    simulate::Simulate,
    ChainBinomialSim, GillespieSim, TauLeapSim,
};

fn golden_path(name: &str) -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest)
        .join("../../../ir/golden")
        .join(format!("{}.ir.json", name))
        .to_string_lossy()
        .to_string()
}

fn load_bimolecular() -> (ir::Model, CompiledModel) {
    let contents = std::fs::read_to_string(golden_path("bimolecular"))
        .expect("read bimolecular.ir.json");
    let mut model: ir::Model = ir::from_str(&contents).unwrap();  // gh#audit-C8
    if let Some(preset) = model.presets.first() {
        for p in &mut model.parameters {
            if let Some(&v) = preset.params.get(&p.name) {
                p.value = Some(v);
            }
        }
    }
    let compiled = CompiledModel::new(model.clone()).unwrap();
    (model, compiled)
}

fn local_idx(compiled: &CompiledModel, name: &str) -> usize {
    let g = *compiled.comp_index.get(name).expect("compartment");
    compiled.global_to_int[g].expect("integer compartment")
}

fn assert_bimolecular_invariants<F>(
    compiled: &CompiledModel,
    params: &[f64],
    run_seed: F,
    backend: &str,
) where F: Fn(u64) -> sim::Trajectory {
    let idx_a = local_idx(compiled, "A");
    let idx_b = local_idx(compiled, "B");
    let idx_c = local_idx(compiled, "C");
    let _ = params;

    for seed in 0..10u64 {
        let traj = run_seed(seed);
        let a0 = traj.snapshots[0].int_state.counts[idx_a];
        let b0 = traj.snapshots[0].int_state.counts[idx_b];
        for snap in &traj.snapshots {
            let a = snap.int_state.counts[idx_a];
            let b = snap.int_state.counts[idx_b];
            let c = snap.int_state.counts[idx_c];
            assert_eq!(a + c, a0,
                "{}: A + C drift at t={} seed={}: {} != {}",
                backend, snap.t, seed, a + c, a0);
            assert_eq!(b + c, b0,
                "{}: B + C drift at t={} seed={}: {} != {}",
                backend, snap.t, seed, b + c, b0);
            assert_eq!(a0 - a, b0 - b,
                "{}: A and B not co-decremented at t={} seed={}: ΔA={} ΔB={}",
                backend, snap.t, seed, a0 - a, b0 - b);
        }
    }
}

#[test]
fn test_bimolecular_gillespie_conservation() {
    let (model, compiled) = load_bimolecular();
    let params = compiled.default_params.clone();
    let config = SimConfig::Gillespie(GillespieConfig {
        t_start: model.simulation.t_start,
        t_end: model.simulation.t_end,
        output_dt: None,
    });
    assert_bimolecular_invariants(&compiled, &params,
        |seed| GillespieSim.run(&compiled, &params, seed, &config).unwrap(),
        "gillespie");
}

#[test]
fn test_bimolecular_tau_leap_conservation() {
    let (model, compiled) = load_bimolecular();
    let params = compiled.default_params.clone();
    let config = SimConfig::TauLeap(TauLeapConfig {
        t_start: model.simulation.t_start,
        t_end: model.simulation.t_end,
        dt: 0.5,
    });
    assert_bimolecular_invariants(&compiled, &params,
        |seed| TauLeapSim.run(&compiled, &params, seed, &config).unwrap(),
        "tau_leap");
}

#[test]
fn test_bimolecular_chain_binomial_conservation() {
    let (model, compiled) = load_bimolecular();
    let params = compiled.default_params.clone();
    let config = SimConfig::ChainBinomial(ChainBinomialConfig {
        t_start: model.simulation.t_start,
        t_end: model.simulation.t_end,
        dt: 1.0,
    });
    assert_bimolecular_invariants(&compiled, &params,
        |seed| ChainBinomialSim.run(&compiled, &params, seed, &config).unwrap(),
        "chain_binomial");
}
