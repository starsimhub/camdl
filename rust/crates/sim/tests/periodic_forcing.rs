//! Validate periodic forcing bin lookup against hand-computed values.
//! An off-by-one in the bin index would shift the school calendar by one bin width.

use std::collections::HashMap;
use ir::{
    expr::{ConstExpr, Expr},
    model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
    time_func::{Periodic, TimeFuncKind, TimeFunction},
    Model,
};
use sim::{
    compiled_model::CompiledModel,
    propensity::eval_time_func,
};

fn model_with_periodic(period: f64, values: Vec<f64>) -> CompiledModel {
    let tf = TimeFunction {
        name: "school".into(),
        kind: TimeFuncKind::Periodic(Periodic {
            period: Expr::Const(ConstExpr { value: period }),
            values: values.iter().map(|&v| Expr::Const(ConstExpr { value: v })).collect(),
        }),
    };
    let model = Model {
        name: "test".into(),
        version: "0.3".into(),
        time_unit: "days".into(),
        description: None,
        origin: None,
        compartments: vec![Compartment { name: "S".into(), kind: CompartmentKind::Integer }],
        transitions: vec![],
        ode_equations: vec![],
        time_functions: vec![tf],
        tables: vec![],
        interventions: vec![],
        observations: vec![],
        parameters: vec![],
        initial_conditions: InitialConditions::Parameterized(HashMap::new()),
        data_contract: None,
        output: OutputConfig {
            times: OutputSchedule::AtTimes(vec![0.0]),
            format: "tsv".into(),
            trajectory: true,
            observations: false,
        },
        simulation: SimulationConfig {
            t_start: 0.0, t_end: 1.0,
            time_semantics: "continuous".into(),
            dt: None, rng_seed: Some(42),
        },
        presets: vec![],
        model_structure: None,
    };
    CompiledModel::new(model).unwrap()
}

#[test]
fn test_periodic_4_bins() {
    // Period=100, 4 bins of width 25: values [1, 2, 3, 4]
    // Bin 0: t ∈ [0, 25)  → 1
    // Bin 1: t ∈ [25, 50) → 2
    // Bin 2: t ∈ [50, 75) → 3
    // Bin 3: t ∈ [75, 100) → 4
    let cm = model_with_periodic(100.0, vec![1.0, 2.0, 3.0, 4.0]);
    let tf = &cm.time_func_cache[0].kind;

    let cases: Vec<(f64, f64)> = vec![
        (0.0, 1.0), (12.5, 1.0), (24.9, 1.0),
        (25.0, 2.0), (37.5, 2.0), (49.9, 2.0),
        (50.0, 3.0), (62.5, 3.0), (74.9, 3.0),
        (75.0, 4.0), (87.5, 4.0), (99.9, 4.0),
        // Wrapping: t=100 → phase=0 → bin 0
        (100.0, 1.0), (125.0, 2.0),
        // Negative time: t=-10 wraps to phase=90 → bin 3
        (-10.0, 4.0),
    ];

    for (t, expected) in &cases {
        let actual = eval_time_func(tf, *t);
        assert_eq!(
            actual, *expected,
            "periodic(t={}) = {}, expected {}", t, actual, expected
        );
    }
}

#[test]
fn test_periodic_school_calendar_52_weeks() {
    // He et al. school calendar: 52 bins over 365.25 days (7.024 days/bin)
    // Known values: bin 0 (day 0) = holiday, bin 2 (day ~14) = term, bin 15 (day ~105) = holiday
    let mut values = vec![0.0; 52];
    // Term weeks (1-indexed in He et al.): 2-14, 17-28, 37-43, 45-51
    // 0-indexed: 1-13, 16-27, 36-42, 44-50
    for i in 1..=13 { values[i] = 1.0; }
    for i in 16..=27 { values[i] = 1.0; }
    for i in 36..=42 { values[i] = 1.0; }
    for i in 44..=50 { values[i] = 1.0; }

    let cm = model_with_periodic(365.25, values.clone());
    let tf = &cm.time_func_cache[0].kind;
    let bin_width = 365.25 / 52.0;

    // Check each bin's midpoint
    for (i, &expected) in values.iter().enumerate() {
        let t = (i as f64 + 0.5) * bin_width; // midpoint of bin i
        let actual = eval_time_func(tf, t);
        assert_eq!(
            actual, expected,
            "school(t={:.1}, bin={}) = {}, expected {}", t, i, actual, expected
        );
    }

    // Check that day 0 is holiday (bin 0)
    assert_eq!(eval_time_func(tf, 0.0), 0.0, "day 0 should be holiday");

    // Check wrapping: year 2, same pattern
    let t_year2 = 365.25 + 2.0 * bin_width; // bin 2 of year 2
    assert_eq!(eval_time_func(tf, t_year2), 1.0, "year 2 bin 2 should be term");
}

#[test]
fn test_periodic_boundary_no_out_of_bounds() {
    // Edge case: t exactly at period boundary
    let cm = model_with_periodic(10.0, vec![1.0, 2.0]);
    let tf = &cm.time_func_cache[0].kind;

    // t=10.0 → phase=0.0 → bin 0
    assert_eq!(eval_time_func(tf, 10.0), 1.0);
    // t=5.0 → phase=5.0 → bin 1
    assert_eq!(eval_time_func(tf, 5.0), 2.0);
    // t=4.99999 → bin 0
    assert_eq!(eval_time_func(tf, 4.999), 1.0);
}
