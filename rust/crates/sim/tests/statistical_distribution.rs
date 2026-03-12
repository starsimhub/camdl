//! Statistical distribution tests (§A.2). These are marked #[ignore] for nightly CI only.

use sim::{
    compiled_model::CompiledModel,
    config::{GillespieConfig, SimConfig},
    simulate::Simulate,
    GillespieSim,
};

fn load_golden(name: &str) -> ir::Model {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let path = std::path::PathBuf::from(&manifest)
        .join("../../../ir/golden")
        .join(format!("{}.ir.json", name));
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("could not read {:?}", path));
    serde_json::from_str(&contents).unwrap()
}

/// Pure death process: I(t=10) should follow Binomial(100, exp(-0.1*10)) = Binomial(100, exp(-1)).
/// Test: mean and variance of I(10) over 2000 seeds.
#[test]
#[ignore = "statistical test: run with --ignored in nightly CI"]
fn test_pure_death_distribution() {
    let model = load_golden("pure_death");
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();
    let config = SimConfig::Gillespie(GillespieConfig {
        t_start: 0.0,
        t_end: 10.0,
        output_dt: None,
    });

    let mut samples: Vec<f64> = Vec::new();
    for seed in 0..2000u64 {
        let traj = GillespieSim.run(&compiled, &params, seed, &config).unwrap();
        if let Some(last) = traj.snapshots.last() {
            samples.push(last.int_state.counts[0] as f64);
        }
    }

    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);

    // Theoretical: E[I(10)] = 100 * exp(-1) ≈ 36.79, Var ≈ 23.25
    let expected_mean = 100.0 * (-1.0f64).exp();
    let expected_var = expected_mean * (1.0 - (-1.0f64).exp());

    assert!(
        (mean - expected_mean).abs() < 2.0,
        "pure death mean wrong: got {:.2}, expected {:.2}", mean, expected_mean
    );
    assert!(
        (var - expected_var).abs() < 3.0,
        "pure death variance wrong: got {:.2}, expected {:.2}", var, expected_var
    );
}

/// Two-state equilibrium: E[A] = N * k2/(k1+k2) = 50 * 0.7 = 35.
#[test]
#[ignore = "statistical test: run with --ignored in nightly CI"]
fn test_two_state_equilibrium() {
    let model = load_golden("two_state");
    let compiled = CompiledModel::new(model.clone()).unwrap();
    let params = compiled.default_params.clone();
    let config = SimConfig::Gillespie(GillespieConfig {
        t_start: 0.0,
        t_end: 100.0, // run to equilibrium
        output_dt: None,
    });

    let mut a_samples: Vec<f64> = Vec::new();
    for seed in 0..5000u64 {
        let traj = GillespieSim.run(&compiled, &params, seed, &config).unwrap();
        if let Some(last) = traj.snapshots.last() {
            // A is first compartment
            a_samples.push(last.int_state.counts[0] as f64);
        }
    }

    let n = a_samples.len() as f64;
    let mean = a_samples.iter().sum::<f64>() / n;
    let expected_mean = 50.0 * 0.7 / (0.3 + 0.7); // = 35.0

    assert!(
        (mean - expected_mean).abs() < 1.5,
        "two-state equilibrium mean wrong: got {:.2}, expected {:.2}", mean, expected_mean
    );
}
