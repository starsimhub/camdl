/// Deserialise all golden IR files and assert no errors.
///
/// Run with:  cd rust && cargo test --test golden_deser

use std::fs;
use std::path::Path;

fn golden_dir() -> std::path::PathBuf {
    // Works whether run from `rust/` or from the repo root.
    let candidates = [
        Path::new("../ir/golden"),
        Path::new("ir/golden"),
    ];
    for c in &candidates {
        if c.is_dir() {
            return c.to_path_buf();
        }
    }
    panic!("cannot locate ir/golden directory (tried ../ir/golden and ir/golden)");
}

fn deser_golden(name: &str) {
    let dir = golden_dir();
    let path = dir.join(format!("{}.ir.json", name));
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let model = ir::from_str(&contents)
        .unwrap_or_else(|e| panic!("failed to deserialise {}: {}", name, e));

    // Round-trip: serialise and deserialise again; structural equality must hold.
    let json2 = ir::to_string_pretty(&model)
        .unwrap_or_else(|e| panic!("failed to serialise {}: {}", name, e));
    let model2 = ir::from_str(&json2)
        .unwrap_or_else(|e| panic!("round-trip deserialise failed for {}: {}", name, e));

    assert_eq!(model, model2, "round-trip equality failed for {}", name);

    // Basic sanity: version field
    assert_eq!(model.version, "0.3", "unexpected version in {}", name);

    // Run validation
    ir::validate::validate(&model)
        .unwrap_or_else(|errs| {
            let msgs: Vec<_> = errs.iter().map(|e| e.to_string()).collect();
            panic!("validation errors in {}:\n  {}", name, msgs.join("\n  "));
        });
}

#[test] fn golden_sir_basic()         { deser_golden("sir_basic"); }
#[test] fn golden_sir_demography()    { deser_golden("sir_demography"); }
#[test] fn golden_sir_vaccination()   { deser_golden("sir_vaccination"); }
#[test] fn golden_pure_death()        { deser_golden("pure_death"); }
#[test] fn golden_birth_death()       { deser_golden("birth_death"); }
#[test] fn golden_two_state()         { deser_golden("two_state"); }
#[test] fn golden_cholera_siwr()      { deser_golden("cholera_siwr"); }
#[test] fn golden_seir_age()          { deser_golden("seir_age"); }
#[test] fn golden_sir_placebo_ekrng() { deser_golden("sir_placebo_ekrng"); }
