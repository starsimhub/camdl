//! Reproduction test: does log_transition_density_substep return -inf
//! on a trajectory simulated by step_one at the SAME params?
//!
//! If this fails, the density function doesn't mirror step_one.

use sim::compiled_model::CompiledModel;
use sim::inference::pgas::{simulate_reference, complete_data_loglik};
use sim::inference::ObsStreamSpec;
use sim::inference::pgas::IVPMapping;
use sim::inference::particle_filter::Observation;
use sim::rng::StatefulRng;

fn load_model(path: &str) -> ir::Model {
    let json = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", path, e));
    serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("cannot parse {}: {}", path, e))
}

/// Test: complete_data_loglik on the SIR basic model (single-patch, 2 transitions)
/// should be finite at its own params.
#[test]
fn test_density_matches_step_one_sir() {
    let model = load_model("../../../ocaml/golden/sir_basic.ir.json");
    let mut model = model;
    for p in &mut model.parameters {
        if p.value.is_none() {
            p.value = Some(match p.name.as_str() {
                "beta" => 0.4, "gamma" => 0.1, "mu" => 0.01, _ => 0.5,
            });
        }
    }
    let compiled = CompiledModel::new(model).unwrap();
    let mut params = vec![0.0; compiled.param_index.len()];
    for p in &compiled.model.parameters {
        if let Some(v) = p.value {
            params[compiled.param_index[p.name.as_str()]] = v;
        }
    }

    let mut rng = StatefulRng::new(42);
    let dt = compiled.model.simulation.dt.unwrap_or(1.0);
    let t_end = compiled.model.simulation.t_end;
    let trajectory = simulate_reference(&compiled, &params, t_end, dt, &mut rng).unwrap();

    let obs_streams: Vec<ObsStreamSpec> = vec![];
    let ivp_mappings: Vec<IVPMapping> = vec![];
    let observations: Vec<Observation> = vec![];

    let ll = complete_data_loglik(
        &compiled, &trajectory, &params, &observations, dt,
        &obs_streams, &ivp_mappings,
    ).unwrap();

    eprintln!("  SIR basic: complete-data LL = {:.4} ({} substeps, {} transitions, {} groups)",
        ll, trajectory.substeps.len(), compiled.model.transitions.len(), compiled.source_groups.len());
    assert!(ll.is_finite(), "SIR basic: LL should be finite at own params, got {}", ll);
}

/// Test: complete_data_loglik on the SIR demography model (3 transitions per S group)
/// should be finite at its own params.
#[test]
fn test_density_matches_step_one_sir_demography() {
    let model = load_model("../../../ocaml/golden/sir_demography.ir.json");
    let mut model = model;
    for p in &mut model.parameters {
        if p.value.is_none() {
            p.value = Some(match p.name.as_str() {
                "beta" => 0.4, "gamma" => 0.1, "mu" => 0.02, _ => 0.5,
            });
        }
    }
    let compiled = CompiledModel::new(model).unwrap();
    let mut params = vec![0.0; compiled.param_index.len()];
    for p in &compiled.model.parameters {
        if let Some(v) = p.value {
            params[compiled.param_index[p.name.as_str()]] = v;
        }
    }

    let mut rng = StatefulRng::new(42);
    let dt = compiled.model.simulation.dt.unwrap_or(1.0);
    let t_end = compiled.model.simulation.t_end;
    let trajectory = simulate_reference(&compiled, &params, t_end, dt, &mut rng).unwrap();

    let obs_streams: Vec<ObsStreamSpec> = vec![];
    let ivp_mappings: Vec<IVPMapping> = vec![];
    let observations: Vec<Observation> = vec![];

    let ll = complete_data_loglik(
        &compiled, &trajectory, &params, &observations, dt,
        &obs_streams, &ivp_mappings,
    ).unwrap();

    eprintln!("  SIR demography: complete-data LL = {:.4} ({} substeps, {} transitions, {} groups)",
        ll, trajectory.substeps.len(), compiled.model.transitions.len(), compiled.source_groups.len());
    assert!(ll.is_finite(), "SIR demography: LL should be finite at own params, got {}", ll);
}

/// Test: complete_data_loglik on a 2-patch model.
#[test]
fn test_density_matches_step_one_two_patch() {
    let path = "../../../ocaml/golden/sir_two_patch.ir.json";
    let model = match std::fs::read_to_string(path) {
        Ok(json) => match serde_json::from_str::<ir::Model>(&json) {
            Ok(m) => m, Err(e) => { eprintln!("  skip: {}", e); return; }
        },
        Err(_) => { eprintln!("  skip: not found"); return; }
    };
    let mut model = model;
    for p in &mut model.parameters { if p.value.is_none() { p.value = Some(0.1); } }
    let compiled = CompiledModel::new(model).unwrap();
    let mut params = vec![0.0; compiled.param_index.len()];
    for p in &compiled.model.parameters {
        if let Some(v) = p.value { params[compiled.param_index[p.name.as_str()]] = v; }
    }
    let mut rng = StatefulRng::new(42);
    let dt = compiled.model.simulation.dt.unwrap_or(1.0);
    let t_end = compiled.model.simulation.t_end;
    let trajectory = simulate_reference(&compiled, &params, t_end, dt, &mut rng).unwrap();
    let ll = complete_data_loglik(
        &compiled, &trajectory, &params, &[], dt, &[], &[],
    ).unwrap();
    eprintln!("  two_patch: LL={:.4} ({} substeps, {} tr, {} groups)",
        ll, trajectory.substeps.len(), compiled.model.transitions.len(), compiled.source_groups.len());
    assert!(ll.is_finite(), "two_patch LL should be finite, got {}", ll);
}

/// Test: complete_data_loglik on polio_spatial_5 (5 patches, 5 transitions per S group)
/// This is the exact pattern that causes -inf on the downstream agent's model.
#[test]
fn test_density_matches_step_one_polio_spatial_5() {
    let path = "../../../ocaml/golden/polio_spatial_5.ir.json";
    let model = match std::fs::read_to_string(path) {
        Ok(json) => match serde_json::from_str::<ir::Model>(&json) {
            Ok(m) => m,
            Err(e) => { eprintln!("  skipping: cannot parse {}: {}", path, e); return; }
        },
        Err(_) => { eprintln!("  skipping: {} not found", path); return; }
    };

    let mut model = model;
    for p in &mut model.parameters {
        if p.value.is_none() {
            p.value = Some(0.1); // default for any missing param
        }
    }
    let compiled = CompiledModel::new(model).unwrap();
    let mut params = vec![0.0; compiled.param_index.len()];
    for p in &compiled.model.parameters {
        if let Some(v) = p.value {
            params[compiled.param_index[p.name.as_str()]] = v;
        }
    }

    eprintln!("  spatial model: {} transitions, {} source groups",
        compiled.model.transitions.len(), compiled.source_groups.len());
    for (i, (src, group)) in compiled.source_groups.iter().enumerate() {
        eprintln!("    group {}: src={}, {} transitions", i, src, group.len());
    }

    let mut rng = StatefulRng::new(42);
    let dt = compiled.model.simulation.dt.unwrap_or(1.0);
    let t_end = compiled.model.simulation.t_end;
    let trajectory = simulate_reference(&compiled, &params, t_end, dt, &mut rng).unwrap();

    let obs_streams: Vec<ObsStreamSpec> = vec![];
    let ivp_mappings: Vec<IVPMapping> = vec![];
    let observations: Vec<Observation> = vec![];

    let ll = complete_data_loglik(
        &compiled, &trajectory, &params, &observations, dt,
        &obs_streams, &ivp_mappings,
    ).unwrap();

    eprintln!("  spatial: complete-data LL = {:.4} ({} substeps)",
        ll, trajectory.substeps.len());
    assert!(ll.is_finite(),
        "spatial: LL should be finite at own params, got {}. \
         Run with CAMDL_TRACE_STEPS=1 for diagnostics.", ll);
}
