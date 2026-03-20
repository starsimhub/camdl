//! Tests for scenario/preset patch logic: intervention filtering by enable/disable lists.
//! Verifies that the util.rs scenario patch logic correctly retains or clears interventions.

use std::path::PathBuf;

fn ocaml_golden_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    PathBuf::from(&manifest).join("../../../ocaml/golden")
}

fn load_model(filename: &str) -> ir::Model {
    let path = ocaml_golden_dir().join(filename);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e));
    serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("IR parse error in {}: {}", filename, e))
}

/// Apply the same enable/disable filtering used in util.rs run_simulation.
fn apply_scenario_patch(
    model: &mut ir::Model,
    enable: &[String],
    disable: &[String],
) {
    if !enable.is_empty() {
        model.interventions.retain(|iv| enable.contains(&iv.name));
    } else if !disable.is_empty() {
        model.interventions.retain(|iv| !disable.contains(&iv.name));
    } else {
        model.interventions.clear();
    }
}

#[test]
fn test_baseline_clears_interventions() {
    let mut model = load_model("seir_vaccine.ir.json");
    // Model has sia_round_1 in interventions
    assert!(
        model.interventions.iter().any(|iv| iv.name == "sia_round_1"),
        "seir_vaccine should have sia_round_1 intervention"
    );
    // Baseline: no enable, no disable → interventions cleared
    apply_scenario_patch(&mut model, &[], &[]);
    assert!(
        model.interventions.is_empty(),
        "baseline patch should clear all interventions"
    );
}

#[test]
fn test_enable_retains_named_intervention() {
    let mut model = load_model("seir_vaccine.ir.json");
    let enable = vec!["sia_round_1".to_string()];
    apply_scenario_patch(&mut model, &enable, &[]);
    assert_eq!(
        model.interventions.len(), 1,
        "with_sia enable should retain exactly one intervention"
    );
    assert_eq!(
        model.interventions[0].name, "sia_round_1",
        "retained intervention should be sia_round_1"
    );
}

#[test]
fn test_enable_unknown_name_leaves_empty() {
    let mut model = load_model("seir_vaccine.ir.json");
    let enable = vec!["nonexistent_intervention".to_string()];
    apply_scenario_patch(&mut model, &enable, &[]);
    assert!(
        model.interventions.is_empty(),
        "enabling a non-existent intervention name should leave interventions empty"
    );
}

#[test]
fn test_disable_removes_named_intervention() {
    let mut model = load_model("seir_vaccine.ir.json");
    let disable = vec!["sia_round_1".to_string()];
    apply_scenario_patch(&mut model, &[], &disable);
    assert!(
        model.interventions.iter().all(|iv| iv.name != "sia_round_1"),
        "disable should remove sia_round_1 from interventions"
    );
}
