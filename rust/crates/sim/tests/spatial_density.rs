//! Reproduction test: does log_transition_density_substep return -inf
//! on a trajectory simulated by step_one at the SAME params?
//!
//! If this fails, the density function doesn't mirror step_one.

use sim::compiled_model::CompiledModel;
use sim::inference::pgas::{simulate_reference, complete_data_loglik, log_transition_density_substep};
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

/// Test: downstream agent's SEIR spatial 5-patch model.
/// This model has waning immunity (R→S), seasonal forcing, and
/// gives -inf in their PGAS runs. If this test fails, we've
/// reproduced the bug.
#[test]
fn test_density_downstream_seir_spatial_5() {
    let path = "tests/fixtures/seir_spatial_5.ir.json";
    let model = match std::fs::read_to_string(path) {
        Ok(json) => match serde_json::from_str::<ir::Model>(&json) {
            Ok(m) => m,
            Err(e) => { eprintln!("  skip: cannot parse: {}", e); return; }
        },
        Err(_) => { eprintln!("  skip: {} not found", path); return; }
    };

    let mut model = model;
    for p in &mut model.parameters {
        if p.value.is_none() {
            p.value = Some(match p.name.as_str() {
                "R0" => 20.0, "sigma" => 0.125, "gamma" => 0.2,
                "amplitude" => 0.3, "s0" => 0.06, "kappa" => 0.05,
                "rho" => 0.4, "sigma_se" => 0.05, "k" => 10.0,
                "N0_p1" => 100000.0, "N0_p2" => 80000.0,
                "N0_p3" => 60000.0, "N0_p4" => 50000.0,
                "N0_p5" => 150000.0,
                _ => 1.0,
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

    eprintln!("  downstream SEIR spatial 5: {} transitions, {} source groups",
        compiled.model.transitions.len(), compiled.source_groups.len());
    for (i, (src, group)) in compiled.source_groups.iter().enumerate() {
        let names: Vec<&str> = group.iter()
            .map(|&j| compiled.model.transitions[j].name.as_str()).collect();
        if group.len() > 1 {
            eprintln!("    group {}: src={}, {} tr: {:?}", i, src, group.len(), names);
        }
    }

    let mut rng = StatefulRng::new(42);
    let dt = compiled.model.simulation.dt.unwrap_or(1.0);
    let t_end = compiled.model.simulation.t_end;
    let trajectory = simulate_reference(&compiled, &params, t_end, dt, &mut rng).unwrap();
    eprintln!("  {} substeps", trajectory.substeps.len());

    // Check EACH substep individually to find the first -inf
    let t_start = compiled.model.simulation.t_start;
    for s in 0..trajectory.substeps.len() {
        let t = t_start + s as f64 * dt;
        let counts_before = if s == 0 {
            &trajectory.initial_counts
        } else {
            &trajectory.substeps[s - 1].counts
        };
        let rec = &trajectory.substeps[s];

        let td = log_transition_density_substep(
            &compiled, counts_before, &rec.flows, &rec.gammas, &params, t, dt,
        ).unwrap();

        if !td.is_finite() {
            eprintln!("\n  FIRST -inf at substep {} (t={:.1}):", s, t);
            eprintln!("  counts_before ({} compartments): {:?}", counts_before.len(), counts_before);
            eprintln!("  flows ({} transitions): {:?}", rec.flows.len(), &rec.flows);
            eprintln!("  gammas: {:?}", &rec.gammas);

            // Evaluate propensities to find the mismatch
            let mut propensities = vec![0.0; compiled.model.transitions.len()];
            let int_s = sim::state::IntState { counts: counts_before.to_vec() };
            let real_s = sim::state::RealState::new(compiled.real_local_to_global.len());
            sim::propensity::eval_propensities(
                &compiled, &int_s, &real_s, &params, t, &mut propensities
            ).unwrap();

            for &(src_local, ref group) in &compiled.source_groups {
                for &tr_idx in group {
                    let rate = propensities[tr_idx];
                    let flow = rec.flows[tr_idx];
                    if (rate <= 0.0 && flow > 0) || (flow > 0) {
                        eprintln!("    {} (idx={}): rate={:.6e}, flow={}, src_count={}",
                            compiled.model.transitions[tr_idx].name, tr_idx,
                            rate, flow, counts_before[src_local]);
                    }
                }
            }

            panic!("density -inf at substep {} — see diagnostics above", s);
        }
    }

    let ll = complete_data_loglik(
        &compiled, &trajectory, &params, &[], dt, &[], &[],
    ).unwrap();
    eprintln!("  complete-data LL = {:.4}", ll);
    assert!(ll.is_finite(), "LL should be finite, got {}", ll);
}

/// Test: multi-seed round-trip on downstream SEIR spatial model.
/// Runs 100 different seeds to catch rare stochastic edge cases.
#[test]
fn test_density_downstream_multi_seed() {
    let path = "tests/fixtures/seir_spatial_5.ir.json";
    let model = match std::fs::read_to_string(path) {
        Ok(json) => match serde_json::from_str::<ir::Model>(&json) {
            Ok(m) => m,
            Err(e) => { eprintln!("  skip: {}", e); return; }
        },
        Err(_) => { eprintln!("  skip: not found"); return; }
    };
    let mut model = model;
    for p in &mut model.parameters {
        if p.value.is_none() {
            p.value = Some(match p.name.as_str() {
                "R0" => 20.0, "sigma" => 0.125, "gamma" => 0.2,
                "amplitude" => 0.3, "s0" => 0.06, "kappa" => 0.05,
                "rho" => 0.4, "sigma_se" => 0.05, "k" => 10.0,
                "N0_p1" => 100000.0, "N0_p2" => 80000.0,
                "N0_p3" => 60000.0, "N0_p4" => 50000.0,
                "N0_p5" => 150000.0,
                _ => 1.0,
            });
        }
    }
    let compiled = CompiledModel::new(model).unwrap();
    let mut params = vec![0.0; compiled.param_index.len()];
    for p in &compiled.model.parameters {
        if let Some(v) = p.value { params[compiled.param_index[p.name.as_str()]] = v; }
    }

    let dt = compiled.model.simulation.dt.unwrap_or(1.0);
    let t_end = compiled.model.simulation.t_end;
    let t_start = compiled.model.simulation.t_start;
    let mut n_inf = 0;

    for seed in 0..100 {
        let mut rng = StatefulRng::new(seed);
        let trajectory = simulate_reference(&compiled, &params, t_end, dt, &mut rng).unwrap();

        // Check per-substep
        let mut _this_inf = false;
        for s in 0..trajectory.substeps.len() {
            let t = t_start + s as f64 * dt;
            let counts_before = if s == 0 {
                &trajectory.initial_counts
            } else {
                &trajectory.substeps[s - 1].counts
            };
            let rec = &trajectory.substeps[s];
            let td = log_transition_density_substep(
                &compiled, counts_before, &rec.flows, &rec.gammas, &params, t, dt,
            ).unwrap();
            if !td.is_finite() {
                if n_inf == 0 {
                    // Print diagnostic for FIRST failure
                    eprintln!("\n  FIRST -inf at seed={}, substep {} (t={:.1}):", seed, s, t);

                    let mut propensities = vec![0.0; compiled.model.transitions.len()];
                    let int_s = sim::state::IntState { counts: counts_before.to_vec() };
                    let real_s = sim::state::RealState::new(compiled.real_local_to_global.len());
                    sim::propensity::eval_propensities(
                        &compiled, &int_s, &real_s, &params, t, &mut propensities
                    ).unwrap();

                    for &(src_local, ref group) in &compiled.source_groups {
                        for &tr_idx in group {
                            if rec.flows[tr_idx] > 0 || propensities[tr_idx] <= 0.0 {
                                eprintln!("    {} (idx={}): rate={:.6e}, flow={}, src_count={}",
                                    compiled.model.transitions[tr_idx].name, tr_idx,
                                    propensities[tr_idx], rec.flows[tr_idx],
                                    counts_before[src_local]);
                            }
                        }
                    }
                }
                n_inf += 1;
                _this_inf = true;
                break;
            }
        }
    }

    eprintln!("  multi-seed: {}/100 seeds produced -inf", n_inf);
    assert_eq!(n_inf, 0, "{}/100 seeds produced -inf density at own params", n_inf);
}
