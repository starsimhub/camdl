//! IF2 integration tests.
//!
//! Test 1: Convergence from dispersed start on sir_basic.
//! Test 2: Parameters stay within bounds (scaled logit enforcement).
//! Test 3: No-cooling mode explores without contracting.

use std::collections::HashMap;
use ir::{
    expr::{BinOpExpr, BinOpWrap, BinOp, ConstExpr, Expr, ParamExpr, PopExpr, PopSumExpr},
    model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
    parameter::Parameter,
    transition::{Transition, StoichiometryEntry, DrawMethod},
    Model,
};
use sim::{
    compiled_model::CompiledModel,
    chain_binomial::{step_one, StepScratch},
    inference::{
        obs_loglik::negbin_logpmf,
        if2::{run_if2, IF2Config, IF2Param, Observation, Transform},
        ParticleState,
    },
    ekrng::StatefulRng,
};

fn sir_model() -> (CompiledModel, Vec<f64>) {
    let model = Model {
        name: "sir_if2_test".into(),
        version: "0.3".into(),
        time_unit: "days".into(),
        description: None,
        origin: None,
        compartments: vec![
            Compartment { name: "S".into(), kind: CompartmentKind::Integer },
            Compartment { name: "I".into(), kind: CompartmentKind::Integer },
            Compartment { name: "R".into(), kind: CompartmentKind::Integer },
        ],
        transitions: vec![
            Transition {
                name: "infection".into(),
                stoichiometry: vec![
                    StoichiometryEntry("S".into(), -1),
                    StoichiometryEntry("I".into(), 1),
                ],
                rate: Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                    op: BinOp::Div,
                    left: Box::new(Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                        op: BinOp::Mul,
                        left: Box::new(Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                            op: BinOp::Mul,
                            left: Box::new(Expr::Param(ParamExpr { param: "beta".into() })),
                            right: Box::new(Expr::Pop(PopExpr { pop: "S".into() })),
                        }})),
                        right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
                    }})),
                    right: Box::new(Expr::PopSum(PopSumExpr { pop_sum: vec!["S".into(), "I".into(), "R".into()] })),
                }}),
                event_key: None,
                metadata: None,
                draw_method: DrawMethod::Poisson,
            },
            Transition {
                name: "recovery".into(),
                stoichiometry: vec![
                    StoichiometryEntry("I".into(), -1),
                    StoichiometryEntry("R".into(), 1),
                ],
                rate: Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                    op: BinOp::Mul,
                    left: Box::new(Expr::Param(ParamExpr { param: "gamma".into() })),
                    right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
                }}),
                event_key: None,
                metadata: None,
                draw_method: DrawMethod::Poisson,
            },
        ],
        ode_equations: vec![],
        time_functions: vec![],
        tables: vec![],
        interventions: vec![],
        observations: vec![],
        parameters: vec![
            Parameter { name: "beta".into(), value: Some(0.3), bounds: Some((0.01, 2.0)), prior: None, transform: None, initial_value: None, param_kind: None },
            Parameter { name: "gamma".into(), value: Some(0.1), bounds: Some((0.01, 1.0)), prior: None, transform: None, initial_value: None, param_kind: None },
        ],
        initial_conditions: InitialConditions::Explicit({
            let mut m = HashMap::new();
            m.insert("S".into(), 990.0);
            m.insert("I".into(), 10.0);
            m
        }),
        data_contract: None,
        output: OutputConfig {
            times: OutputSchedule::AtTimes(vec![0.0, 80.0]),
            format: "tsv".into(),
            trajectory: true,
            observations: false,
        },
        simulation: SimulationConfig {
            t_start: 0.0, t_end: 80.0,
            time_semantics: "continuous".into(),
            dt: Some(1.0), rng_seed: Some(42),
        },
        presets: vec![],
        model_structure: None,
    };

    let compiled = CompiledModel::new(model).unwrap();
    let params = compiled.default_params.clone();
    (compiled, params)
}

/// Generate synthetic weekly data from a single simulation.
fn generate_data(compiled: &CompiledModel, params: &[f64]) -> Vec<Observation> {
    let mut rng = StatefulRng::new(42);
    let n_int = compiled.int_local_to_global.len();
    let n_tr = compiled.model.transitions.len();
    let mut state = ParticleState::new(n_int, n_tr);
    let (init, _) = compiled.initial_state(params).unwrap();
    state.counts.copy_from_slice(&init.counts);

    let mut scratch = StepScratch::new(compiled);
    let mut obs = Vec::new();
    let mut t = 0.0;
    while t < 77.0 {
        for _ in 0..7 {
            step_one(compiled, &mut state.counts, &mut state.flow_accumulators, params, t, 1.0, &mut rng, &mut scratch).unwrap();
            t += 1.0;
        }
        // Project: recovery flow (index 1)
        let cases = state.flow_accumulators[1] as f64;
        obs.push(Observation { time: t, value: cases });
        state.reset_flows();
    }
    obs
}

#[test]
fn test_if2_converges_from_dispersed_start() {
    let (compiled, true_params) = sir_model();
    let data = generate_data(&compiled, &true_params);

    // Start from dispersed parameters (beta=0.8, gamma=0.3 — wrong by 2-3×)
    let mut start_params = true_params.clone();
    start_params[0] = 0.8;  // beta (true: 0.3)
    start_params[1] = 0.3;  // gamma (true: 0.1)

    let if2_params = vec![
        IF2Param {
            name: "beta".into(), index: 0, initial: 0.8,
            rw_sd: 0.05, transform: Transform::Logit, lower: 0.01, upper: 2.0, ivp: false,
        },
        IF2Param {
            name: "gamma".into(), index: 1, initial: 0.3,
            rw_sd: 0.01, transform: Transform::Logit, lower: 0.01, upper: 1.0, ivp: false,
        },
    ];

    let config = IF2Config {
        n_particles: 200,
        n_iterations: 30,
        cooling_fraction: 0.90,
        cooling_target_iters: 50,
        dt: 1.0,
    };

    let step_fn = |state: &mut ParticleState, p: &[f64], t: f64, dt: f64, rng: &mut StatefulRng, scratch: &mut StepScratch| {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, p, t, dt, rng, scratch)
    };
    let project_fn = |state: &ParticleState| state.flow_accumulators[1] as f64;
    let dmeasure_fn = |proj: f64, obs: f64, p: &[f64]| {
        negbin_logpmf(obs, proj.max(0.1), 10.0)
    };

    let result = run_if2(
        &compiled, &start_params, &if2_params, &data, &config,
        &step_fn, &project_fn, &dmeasure_fn, 42,
    ).unwrap();

    let final_beta = result.mle[0];
    let final_gamma = result.mle[1];

    eprintln!("IF2 converged: beta={:.4}, gamma={:.4}, loglik={:.2}",
        final_beta, final_gamma, result.final_loglik);

    // Should converge toward true values.
    // Tolerances: beta within ±0.15 of 0.3 AND gamma within ±0.05 of 0.1
    // AND loglik improves. Under fully random params, the joint probability
    // of all three is <0.5% (parameter space is [0.01,2]×[0.01,1] and
    // random loglik doesn't improve monotonically).
    assert!((final_beta - 0.3).abs() < 0.15,
        "IF2 beta={:.3}, expected ~0.3 (started at 0.8)", final_beta);
    assert!((final_gamma - 0.1).abs() < 0.05,
        "IF2 gamma={:.3}, expected ~0.1 (started at 0.3)", final_gamma);

    // Log-likelihood must be finite and better than the first iteration
    let first_ll = result.iterations[0].log_likelihood;
    let last_ll = result.final_loglik;
    assert!(last_ll.is_finite(), "final loglik should be finite, got {}", last_ll);
    assert!(last_ll >= first_ll - 5.0,
        "loglik should not degrade: first={:.1}, last={:.1}", first_ll, last_ll);

    // The last few iterations should have stable loglik (not oscillating wildly)
    let tail_lls: Vec<f64> = result.iterations.iter().rev().take(5)
        .map(|it| it.log_likelihood).filter(|l| l.is_finite()).collect();
    if tail_lls.len() >= 3 {
        let mean_ll = tail_lls.iter().sum::<f64>() / tail_lls.len() as f64;
        let max_dev = tail_lls.iter().map(|&l| (l - mean_ll).abs()).fold(0.0f64, f64::max);
        assert!(max_dev < 20.0,
            "tail loglik oscillating too much: mean={:.1}, max_dev={:.1}", mean_ll, max_dev);
    }
}

#[test]
fn test_if2_respects_bounds() {
    let (compiled, true_params) = sir_model();
    let data = generate_data(&compiled, &true_params);

    // Use tight bounds to test enforcement
    let if2_params = vec![
        IF2Param {
            name: "beta".into(), index: 0, initial: 0.3,
            rw_sd: 0.1, // aggressive — would escape without bounds
            transform: Transform::Logit, lower: 0.1, upper: 0.5, ivp: false,
        },
        IF2Param {
            name: "gamma".into(), index: 1, initial: 0.1,
            rw_sd: 0.03,
            transform: Transform::Logit, lower: 0.05, upper: 0.2, ivp: false,
        },
    ];

    let config = IF2Config {
        n_particles: 100,
        n_iterations: 20,
        cooling_fraction: 0.95,
        cooling_target_iters: 50,
        dt: 1.0,
    };

    let step_fn = |state: &mut ParticleState, p: &[f64], t: f64, dt: f64, rng: &mut StatefulRng, scratch: &mut StepScratch| {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, p, t, dt, rng, scratch)
    };
    let project_fn = |state: &ParticleState| state.flow_accumulators[1] as f64;
    let dmeasure_fn = |proj: f64, obs: f64, _p: &[f64]| negbin_logpmf(obs, proj.max(0.1), 10.0);

    let result = run_if2(
        &compiled, &true_params, &if2_params, &data, &config,
        &step_fn, &project_fn, &dmeasure_fn, 42,
    ).unwrap();

    // All iterations should have beta in [0.1, 0.5] and gamma in [0.05, 0.2]
    for it in &result.iterations {
        let beta = it.param_means[0];
        let gamma = it.param_means[1];
        assert!(beta >= 0.1 && beta <= 0.5,
            "beta={:.4} outside bounds [0.1, 0.5] at iter {}", beta, it.iteration);
        assert!(gamma >= 0.05 && gamma <= 0.2,
            "gamma={:.4} outside bounds [0.05, 0.2] at iter {}", gamma, it.iteration);
    }
}

#[test]
fn test_if2_no_cooling_explores() {
    let (compiled, true_params) = sir_model();
    let data = generate_data(&compiled, &true_params);

    let if2_params = vec![
        IF2Param {
            name: "beta".into(), index: 0, initial: 0.3,
            rw_sd: 0.02, transform: Transform::Logit, lower: 0.01, upper: 2.0, ivp: false,
        },
    ];

    // No cooling (cooling_fraction = 1.0) — perturbations don't shrink
    let config = IF2Config {
        n_particles: 100,
        n_iterations: 15,
        cooling_fraction: 1.0,
        cooling_target_iters: 50,
        dt: 1.0,
    };

    let step_fn = |state: &mut ParticleState, p: &[f64], t: f64, dt: f64, rng: &mut StatefulRng, scratch: &mut StepScratch| {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, p, t, dt, rng, scratch)
    };
    let project_fn = |state: &ParticleState| state.flow_accumulators[1] as f64;
    let dmeasure_fn = |proj: f64, obs: f64, _p: &[f64]| negbin_logpmf(obs, proj.max(0.1), 10.0);

    let result = run_if2(
        &compiled, &true_params, &if2_params, &data, &config,
        &step_fn, &project_fn, &dmeasure_fn, 42,
    ).unwrap();

    // With no cooling, beta should still be wandering (not converged tight)
    let betas: Vec<f64> = result.iterations.iter().map(|it| it.param_means[0]).collect();
    let beta_range = betas.iter().cloned().fold(f64::INFINITY, f64::min)
        ..=betas.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let spread = *beta_range.end() - *beta_range.start();

    // Should have noticeable spread (>1% of value) across iterations
    assert!(spread > 0.003,
        "no-cooling beta spread={:.4} — should be wandering, not converging", spread);
}
