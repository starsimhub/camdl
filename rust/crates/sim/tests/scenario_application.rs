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
    if !enable.is_empty() || !disable.is_empty() {
        model.interventions.retain(|iv| {
            let kept_by_enable  = enable.is_empty() || enable.contains(&iv.name);
            let kept_by_disable = !disable.contains(&iv.name);
            kept_by_enable && kept_by_disable
        });
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

#[test]
fn test_enable_and_disable_compose() {
    // polio_spatial_5 has: sia_north, sia_south, sia_east, sia_west, sia_center
    let mut model = load_model("polio_spatial_5.ir.json");
    assert_eq!(model.interventions.len(), 5);

    // enable=[north, south, east], disable=[east] → should keep north and south only
    let enable  = vec!["sia_north".to_string(), "sia_south".to_string(), "sia_east".to_string()];
    let disable = vec!["sia_east".to_string()];
    apply_scenario_patch(&mut model, &enable, &disable);

    let names: Vec<&str> = model.interventions.iter().map(|iv| iv.name.as_str()).collect();
    assert!(names.contains(&"sia_north"),  "sia_north should be retained");
    assert!(names.contains(&"sia_south"),  "sia_south should be retained");
    assert!(!names.contains(&"sia_east"),  "sia_east should be excluded by disable");
    assert!(!names.contains(&"sia_west"),  "sia_west should be excluded (not enabled)");
    assert!(!names.contains(&"sia_center"),"sia_center should be excluded (not enabled)");
    assert_eq!(names.len(), 2);
}

#[test]
fn test_disable_only_keeps_all_except_disabled() {
    // enable=[], disable=[sia_north] → keep all 4 others
    let mut model = load_model("polio_spatial_5.ir.json");
    let disable = vec!["sia_north".to_string()];
    apply_scenario_patch(&mut model, &[], &disable);
    assert_eq!(model.interventions.len(), 4);
    assert!(model.interventions.iter().all(|iv| iv.name != "sia_north"));
}
