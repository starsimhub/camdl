//! BUG-1 / Phase G: CRN determinism test.
//! Two independent runs with the same seed must produce identical trajectories.
//! This is the invariant that CRN scenario coupling depends on.

use std::path::PathBuf;
use sim::{
    compiled_model::CompiledModel,
    config::{GillespieConfig, SimConfig},
    simulate::Simulate,
    GillespieSim,
};

fn golden_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    PathBuf::from(&manifest).join("../../../ir/golden")
}

fn load_model(name: &str) -> ir::Model {
    let path = golden_dir().join(format!("{}.ir.json", name));
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("could not read {}", name));
    ir::from_str(&contents)
        .unwrap_or_else(|e| panic!("failed to parse {}: {}", name, e))
}

/// Same seed → identical trajectory snapshots (counts and times).
#[test]
fn test_gillespie_same_seed_identical_output() {
    let model = load_model("sir_basic");

    // Override parameters to non-zero values (golden file has value=0.0)
    let mut m1 = model.clone();
    for p in &mut m1.parameters {
        match p.name.as_str() {
            "beta"  => p.value = Some(0.3),
            "gamma" => p.value = Some(0.1),
            "N0"    => p.value = Some(1000.0),
            "I0"    => p.value = Some(10.0),
            _ => {}
        }
    }
    let m2 = m1.clone();

    let compiled1 = CompiledModel::new(m1).unwrap();
    let compiled2 = CompiledModel::new(m2).unwrap();
    let params1 = compiled1.default_params.clone();
    let params2 = compiled2.default_params.clone();

    let config = SimConfig::Gillespie(GillespieConfig {
        t_start: 0.0,
        t_end: 50.0,
        output_dt: None,
    });

    let traj1 = GillespieSim.run(&compiled1, &params1, 42, &config).unwrap();
    let traj2 = GillespieSim.run(&compiled2, &params2, 42, &config).unwrap();

    assert_eq!(
        traj1.snapshots.len(),
        traj2.snapshots.len(),
        "Trajectories have different lengths"
    );

    for (i, (s1, s2)) in traj1.snapshots.iter().zip(traj2.snapshots.iter()).enumerate() {
        assert_eq!(
            s1.int_state.counts, s2.int_state.counts,
            "Snapshot {} integer counts differ", i
        );
        assert!(
            (s1.t - s2.t).abs() < 1e-12,
            "Snapshot {} times differ: {} vs {}", i, s1.t, s2.t
        );
    }
}

/// Different seeds → different trajectories (sanity check that RNG is actually used).
#[test]
fn test_gillespie_different_seeds_different_output() {
    let model = load_model("sir_basic");

    let mut m = model;
    for p in &mut m.parameters {
        match p.name.as_str() {
            "beta"  => p.value = Some(0.3),
            "gamma" => p.value = Some(0.1),
            "N0"    => p.value = Some(1000.0),
            "I0"    => p.value = Some(10.0),
            _ => {}
        }
    }

    let compiled = CompiledModel::new(m).unwrap();
    let params = compiled.default_params.clone();

    let config = SimConfig::Gillespie(GillespieConfig {
        t_start: 0.0,
        t_end: 50.0,
        output_dt: None,
    });

    let traj1 = GillespieSim.run(&compiled, &params, 42, &config).unwrap();
    let traj2 = GillespieSim.run(&compiled, &params, 99, &config).unwrap();

    // Very likely to differ for a stochastic model with 1000 individuals over 50 days
    let any_differ = traj1.snapshots.iter().zip(traj2.snapshots.iter())
        .any(|(s1, s2)| s1.int_state.counts != s2.int_state.counts);
    assert!(any_differ, "Different seeds should produce different trajectories");
}

/// Regression: `source_groups` ordering inside CompiledModel must be
/// deterministic across recompilations of the same model. Prior to the
/// fix, `groups.into_iter().collect()` preserved HashMap randomized
/// iteration order, so the same .ir.json could produce two different
/// orderings — and since downstream chain_binomial/PGAS/PMMH consume the
/// RNG in source_groups order, the same seed produced different
/// trajectories across runs in the same process.
#[test]
fn test_source_groups_is_sorted_and_stable() {
    // Use a multi-transition model so several compartments act as sources,
    // giving the HashMap a chance to shuffle.
    let model = load_model("sir_basic");
    let mut m = model.clone();
    for p in &mut m.parameters {
        match p.name.as_str() {
            "beta"  => p.value = Some(0.3),
            "gamma" => p.value = Some(0.1),
            "N0"    => p.value = Some(1000.0),
            "I0"    => p.value = Some(10.0),
            _ => {}
        }
    }

    // Compile the same model 20 times. Every compilation must produce the
    // same source_groups order, and that order must be sorted by src_local.
    let first = CompiledModel::new(m.clone()).unwrap().source_groups.clone();
    assert!(
        first.windows(2).all(|w| w[0].0 < w[1].0),
        "source_groups must be sorted by src_local (got: {:?})",
        first.iter().map(|(s, _)| *s).collect::<Vec<_>>()
    );
    for _ in 0..20 {
        let next = CompiledModel::new(m.clone()).unwrap().source_groups.clone();
        assert_eq!(next, first,
            "source_groups order drifted between compilations — \
             HashMap randomization is leaking into simulation output");
    }
}

/// Regression: a seeded simulation run must produce byte-identical output
/// across 20 recompilations of the same model. This is the end-to-end
/// version of the test above — it would fail the same way if any
/// HashMap-ordered iteration made it into simulation code paths.
#[test]
fn test_recompile_preserves_trajectory() {
    let model = load_model("sir_basic");
    let mut m = model.clone();
    for p in &mut m.parameters {
        match p.name.as_str() {
            "beta"  => p.value = Some(0.3),
            "gamma" => p.value = Some(0.1),
            "N0"    => p.value = Some(1000.0),
            "I0"    => p.value = Some(10.0),
            _ => {}
        }
    }
    let config = SimConfig::Gillespie(GillespieConfig {
        t_start: 0.0, t_end: 50.0, output_dt: None,
    });
    let compiled0 = CompiledModel::new(m.clone()).unwrap();
    let traj0 = GillespieSim.run(&compiled0, &compiled0.default_params.clone(), 42, &config).unwrap();
    for _ in 0..20 {
        let compiled = CompiledModel::new(m.clone()).unwrap();
        let traj = GillespieSim.run(&compiled, &compiled.default_params.clone(), 42, &config).unwrap();
        assert_eq!(traj.snapshots.len(), traj0.snapshots.len(),
            "trajectory length differs across recompilations");
        for (i, (a, b)) in traj.snapshots.iter().zip(traj0.snapshots.iter()).enumerate() {
            assert_eq!(a.int_state.counts, b.int_state.counts,
                "snapshot {} counts differ across recompilations", i);
        }
    }
}
