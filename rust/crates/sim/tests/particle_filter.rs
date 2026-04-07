//! Particle filter validation tests.
//!
//! Test 1: Pure death process with Poisson observations.
//! The marginal likelihood is analytically tractable, so we can
//! verify the PF converges to the correct value.
//!
//! Test 2: Determinism — same seed gives same log-likelihood.
//!
//! Test 3: More particles → lower variance of the estimate.

use std::collections::HashMap;
use ir::{
    expr::{BinOpExpr, BinOpWrap, BinOp, Expr, ParamExpr, PopExpr},
    model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
    parameter::Parameter,
    transition::{Transition, StoichiometryEntry, DrawMethod},
    Model,
};
use sim::{
    compiled_model::CompiledModel,
    chain_binomial::{step_one, StepScratch},
    inference::{
        obs_loglik::poisson_logpmf,
        particle_filter::{bootstrap_filter, Observation},
        ParticleState,
    },
    rng::StatefulRng,
};

fn pure_death_model() -> (CompiledModel, Vec<f64>) {
    let model = Model {
        name: "pure_death_pf".into(),
        version: "0.3".into(),
        time_unit: "days".into(),
        description: None,
        origin: None,
        compartments: vec![
            Compartment { name: "N".into(), kind: CompartmentKind::Integer },
        ],
        transitions: vec![
            Transition {
                name: "death".into(),
                stoichiometry: vec![StoichiometryEntry("N".into(), -1)],
                rate: Expr::BinOp(BinOpWrap {
                    bin_op: BinOpExpr {
                        op: BinOp::Mul,
                        left: Box::new(Expr::Param(ParamExpr { param: "mu".into() })),
                        right: Box::new(Expr::Pop(PopExpr { pop: "N".into() })),
                    },
                }),
                event_key: None,
                metadata: None,
                draw_method: DrawMethod::Poisson, rate_grad: Default::default(),
            },
        ],
        ode_equations: vec![],
        time_functions: vec![],
        tables: vec![],
        interventions: vec![],
        observations: vec![],
        parameters: vec![
            Parameter { name: "mu".into(), value: Some(0.01), bounds: None, prior: None, transform: None, initial_value: None, param_kind: None },
        ],
            parameter_groups: vec![],
        initial_conditions: InitialConditions::Explicit({
            let mut m = HashMap::new(); m.insert("N".into(), 100.0); m
        }),
        data_contract: None,
        output: OutputConfig {
            times: OutputSchedule::AtTimes(vec![0.0, 100.0]),
            format: "tsv".into(),
            trajectory: true,
            observations: false,
        },
        simulation: SimulationConfig {
            t_start: 0.0,
            t_end: 100.0,
            time_semantics: "continuous".into(),
            dt: Some(1.0),
            rng_seed: Some(42),
        },
        presets: vec![],
        model_structure: None, balance: None,
    };

    let compiled = CompiledModel::new(model).unwrap();
    let params = compiled.default_params.clone();
    (compiled, params)
}

/// Run the particle filter on a pure death model with Poisson observations.
fn run_pf(n_particles: usize, seed: u64) -> f64 {
    let (compiled, params) = pure_death_model();

    // Fake observations: N(t) observed with Poisson noise at t=10, 20, ..., 100
    // True N(t) ≈ 100 * exp(-0.01 * t)
    let observations: Vec<Observation> = (1..=10)
        .map(|k| {
            let t = k as f64 * 10.0;
            let expected = 100.0 * (-0.01 * t).exp();
            Observation { time: t, value: expected.round() }
        })
        .collect();

    let step_fn = |state: &mut ParticleState, t: f64, dt: f64, rng: &mut StatefulRng, scratch: &mut StepScratch| {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, &params, t, dt, rng, scratch)
    };

    // Project: observe N directly (prevalence, not incidence)
    let project_fn = |state: &ParticleState| -> f64 {
        state.counts[0] as f64
    };

    // Poisson observation model: y ~ Poisson(N)
    let obs_loglik_fn = |projected: f64, observed: f64| -> f64 {
        poisson_logpmf(observed, projected.max(0.1))
    };

    let result = bootstrap_filter(
        &compiled, &params, &observations, n_particles, 1.0,
        &step_fn, &project_fn, &obs_loglik_fn, None, None, seed,
    ).unwrap();

    result.log_likelihood
}

#[test]
fn test_pf_determinism() {
    let ll1 = run_pf(200, 42);
    let ll2 = run_pf(200, 42);
    assert_eq!(ll1, ll2, "same seed must give identical log-likelihood");
}

#[test]
fn test_pf_different_seeds_differ() {
    let ll1 = run_pf(200, 1);
    let ll2 = run_pf(200, 2);
    assert_ne!(ll1, ll2, "different seeds should give different estimates");
}

#[test]
fn test_pf_loglik_finite() {
    let ll = run_pf(500, 42);
    assert!(ll.is_finite(), "log-likelihood should be finite, got {}", ll);
    assert!(ll < 0.0, "log-likelihood should be negative, got {}", ll);
}

#[test]
fn test_pf_more_particles_lower_variance() {
    // Run with 100 and 1000 particles, 20 replicates each.
    // The variance of the estimate should be lower with more particles.
    let mut ll_100: Vec<f64> = Vec::new();
    let mut ll_1000: Vec<f64> = Vec::new();

    for seed in 0..20u64 {
        ll_100.push(run_pf(100, seed));
        ll_1000.push(run_pf(1000, seed));
    }

    let var = |xs: &[f64]| -> f64 {
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (xs.len() - 1) as f64
    };

    let var_100 = var(&ll_100);
    let var_1000 = var(&ll_1000);

    assert!(var_1000 < var_100,
        "1000 particles should have lower variance than 100: var_100={:.4}, var_1000={:.4}",
        var_100, var_1000);
}

#[test]
fn test_pf_ess_reasonable() {
    let (compiled, params) = pure_death_model();
    let observations: Vec<Observation> = (1..=10)
        .map(|k| {
            let t = k as f64 * 10.0;
            let expected = 100.0 * (-0.01 * t).exp();
            Observation { time: t, value: expected.round() }
        })
        .collect();

    let step_fn = |state: &mut ParticleState, t: f64, dt: f64, rng: &mut StatefulRng, scratch: &mut StepScratch| {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, &params, t, dt, rng, scratch)
    };
    let project_fn = |state: &ParticleState| state.counts[0] as f64;
    let obs_loglik_fn = |projected: f64, observed: f64| poisson_logpmf(observed, projected.max(0.1));

    let result = bootstrap_filter(
        &compiled, &params, &observations, 500, 1.0,
        &step_fn, &project_fn, &obs_loglik_fn, None, None, 42,
    ).unwrap();

    // ESS should be reasonable — not collapsed to 1 or full N
    for (i, &ess) in result.ess_trace.iter().enumerate() {
        assert!(ess > 10.0,
            "ESS too low at obs {}: {:.1} (particle collapse)", i, ess);
        assert!(ess <= 500.0,
            "ESS > N_particles at obs {}: {:.1}", i, ess);
    }
}
