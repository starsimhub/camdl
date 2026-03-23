//! Tests for intervention application, including BUG-5: FractionTransfer uses floor not round.

use std::collections::HashMap;
use ir::{
    expr::{ConstExpr, Expr},
    intervention::{Action, FractionTransfer, Intervention, InterventionSchedule},
    model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
    Model,
    parameter::Parameter,
};
use sim::{
    compiled_model::CompiledModel,
    intervention::apply_interventions_at,
    state::{IntState, RealState},
};

fn int_comp(name: &str) -> Compartment {
    Compartment { name: name.into(), kind: CompartmentKind::Integer }
}

fn minimal_model_with_interventions(
    compartments: Vec<Compartment>,
    params: Vec<Parameter>,
    interventions: Vec<Intervention>,
) -> Model {
    Model {
        name: "test".into(),
        version: "0.3".into(),
        description: None,
        compartments,
        transitions: vec![],
        ode_equations: vec![],
        time_functions: vec![],
        tables: vec![],
        interventions,
        observations: vec![],
        parameters: params,
        initial_conditions: InitialConditions::Parameterized(HashMap::new()),
        data_contract: None,
        output: OutputConfig {
            times: OutputSchedule::AtTimes(vec![0.0]),
            format: "tsv".into(),
            trajectory: true,
            observations: false,
        },
        simulation: SimulationConfig {
            t_start: 0.0,
            t_end: 100.0,
            time_semantics: "continuous".into(),
            dt: None,
            rng_seed: Some(42),
        },
        presets: vec![],
        model_structure: None,
    }
}

/// BUG-5: FractionTransfer should use floor, not round.
/// source=1, fraction=0.6: floor(0.6) = 0, round(0.6) = 1.
/// This test ensures we transfer 0 (floor), not 1 (round).
#[test]
fn test_fraction_transfer_uses_floor_not_round() {
    let intervention = Intervention {
        name: "test_iv".into(),
        base_name: None,
        schedule: InterventionSchedule::AtTimes(vec![30.0]),
        actions: vec![
            Action::FractionTransfer(FractionTransfer {
                src: "S".into(),
                dst: "V".into(),
                fraction: Expr::Const(ConstExpr { value: 0.6 }),
            }),
        ],
    };

    let model = CompiledModel::new(minimal_model_with_interventions(
        vec![int_comp("S"), int_comp("V")],
        vec![],
        vec![intervention],
    )).unwrap();

    let mut int_s = IntState::from_vec(vec![1, 0]); // S=1, V=0
    let mut real_s = RealState::new(0);

    apply_interventions_at(30.0, &model, &mut int_s, &mut real_s, &[], 1e-10).unwrap();

    // floor(1 * 0.6) = floor(0.6) = 0: no transfer should happen
    assert_eq!(int_s.counts[0], 1, "S should remain 1 (no transfer)");
    assert_eq!(int_s.counts[1], 0, "V should remain 0 (no transfer)");
}

/// Sanity check: fraction=0.8, source=5 → floor(5*0.8) = floor(4.0) = 4 transferred.
#[test]
fn test_fraction_transfer_floor_larger() {
    let intervention = Intervention {
        name: "test_iv".into(),
        base_name: None,
        schedule: InterventionSchedule::AtTimes(vec![30.0]),
        actions: vec![
            Action::FractionTransfer(FractionTransfer {
                src: "S".into(),
                dst: "V".into(),
                fraction: Expr::Const(ConstExpr { value: 0.8 }),
            }),
        ],
    };

    let model = CompiledModel::new(minimal_model_with_interventions(
        vec![int_comp("S"), int_comp("V")],
        vec![],
        vec![intervention],
    )).unwrap();

    let mut int_s = IntState::from_vec(vec![5, 0]); // S=5, V=0
    let mut real_s = RealState::new(0);

    apply_interventions_at(30.0, &model, &mut int_s, &mut real_s, &[], 1e-10).unwrap();

    // floor(5 * 0.8) = floor(4.0) = 4
    assert_eq!(int_s.counts[0], 1, "S should be 1 after 4 transferred");
    assert_eq!(int_s.counts[1], 4, "V should be 4 after transfer");
}
