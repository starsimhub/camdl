//! Gradient validation: compare analytical gradients (from compiler-emitted
//! derivative expressions) against finite-difference approximations.

use sim::compiled_model::CompiledModel;
use sim::inference::pgas::{IVPMapping, simulate_reference, complete_data_loglik};
use sim::inference::pgas_grad::complete_data_loglik_grad;
use sim::inference::particle_filter::Observation;
use sim::rng::StatefulRng;

fn load_model(path: &str) -> ir::Model {
    let json = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", path, e));
    serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("cannot parse {}: {}", path, e))
}

#[test]
fn test_gradient_vs_finite_differences_sir() {
    // Load a golden SIR model (compiled with autodiff → has rate_grad)
    let model = load_model("../../../ocaml/golden/sir_basic.ir.json");

    // Verify rate_grad is populated
    let has_grads = model.transitions.iter().any(|t| !t.rate_grad.is_empty());
    if !has_grads {
        eprintln!("  skipping: no rate_grad in golden file (run make update-golden)");
        return;
    }

    // Set parameter values (the golden file may not have defaults)
    let mut model = model;
    for p in &mut model.parameters {
        if p.value.is_none() {
            p.value = Some(match p.name.as_str() {
                "beta" => 0.4,
                "gamma" => 0.1,
                "mu" => 0.01,
                _ => 0.5,
            });
        }
    }
    let compiled = CompiledModel::new(model).unwrap();

    let param_names: Vec<String> = compiled.model.parameters.iter()
        .map(|p| p.name.clone()).collect();
    let param_indices: Vec<usize> = param_names.iter()
        .map(|n| *compiled.param_index.get(n.as_str()).unwrap())
        .collect();

    let mut params = vec![0.0; compiled.param_index.len()];
    for p in &compiled.model.parameters {
        if let Some(v) = p.value {
            params[compiled.param_index[p.name.as_str()]] = v;
        }
    }

    // Simulate a trajectory
    let mut rng = StatefulRng::new(42);
    let t_end = compiled.model.simulation.t_end;
    let dt = compiled.model.simulation.dt.unwrap_or(1.0);
    let trajectory = simulate_reference(&compiled, &params, t_end, dt, &mut rng).unwrap();

    let observations: Vec<Observation> = vec![];
    let flow_indices: Vec<usize> = vec![];
    let ivp_mappings: Vec<IVPMapping> = vec![];

    let dmeasure_fn = |_: f64, _: f64| -> f64 { 0.0 };

    // Analytical gradient
    let (ll, grad) = complete_data_loglik_grad(
        &compiled, &trajectory, &params, &observations, dt,
        &dmeasure_fn, &flow_indices, &ivp_mappings,
        &param_names, &param_indices,
    ).unwrap();

    eprintln!("  log-likelihood: {:.4}", ll);
    assert!(ll.is_finite(), "LL must be finite");

    // Finite-difference gradient for each parameter
    let eps = 1e-5;
    let mut max_rel_err = 0.0_f64;
    for i in 0..param_names.len() {
        let mut p_plus = params.clone();
        let mut p_minus = params.clone();
        p_plus[param_indices[i]] += eps;
        p_minus[param_indices[i]] -= eps;

        let ll_plus = complete_data_loglik(
            &compiled, &trajectory, &p_plus, &observations, dt,
            &dmeasure_fn, &flow_indices, &ivp_mappings,
        ).unwrap();
        let ll_minus = complete_data_loglik(
            &compiled, &trajectory, &p_minus, &observations, dt,
            &dmeasure_fn, &flow_indices, &ivp_mappings,
        ).unwrap();

        let fd = (ll_plus - ll_minus) / (2.0 * eps);

        let rel_err = if fd.abs() > 1e-8 {
            (grad[i] - fd).abs() / fd.abs()
        } else {
            (grad[i] - fd).abs()
        };
        max_rel_err = max_rel_err.max(rel_err);

        eprintln!("  d(ll)/d({:12}) = {:12.4} (analytical) vs {:12.4} (fd), rel_err = {:.2e}",
            param_names[i], grad[i], fd, rel_err);

        assert!(rel_err < 0.01,
            "gradient mismatch for {}: analytical={:.6}, fd={:.6}, rel_err={:.2e}",
            param_names[i], grad[i], fd, rel_err);
    }

    eprintln!("  max relative error: {:.2e}", max_rel_err);
}
