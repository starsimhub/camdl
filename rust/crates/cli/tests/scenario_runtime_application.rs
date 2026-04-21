//! End-to-end runtime tests for scenario `set = {...}` and `scale = {...}`.
//!
//! Audit gap P1.1/P1.2 from `docs/dev/reviews/2026-04-21-spec-claims-vs-tests.md`:
//! the OCaml compiler tests verify the preset's `set`/`scale` fields are
//! stored in the IR correctly (`test_compiler.ml:761`), but nothing tested
//! that the Rust runtime actually applies them. `util.rs:753` does the
//! multiplication; if that line were removed, every scenario-scale
//! sensitivity analysis would silently run at baseline values — the same
//! silent-wrong-answer class as the 2026-04-21 table-unit incident.
//!
//! Strategy: pure-death model with `@ mu * S`. At a fixed seed, the
//! trajectory of S depends on mu. Baseline and a scenario that modifies
//! mu must produce visibly different trajectories in the expected
//! direction.

use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../target/release/camdl-sim")
}

fn skip_if_missing_binary() -> Option<PathBuf> {
    let bin = binary();
    if !bin.exists() {
        eprintln!("skipping: binary not built at {}", bin.display());
        return None;
    }
    Some(bin)
}

/// Write a pure-death SIR-ish camdl with named scenarios that set or
/// scale the death rate `mu`. S decays via `@ mu * S`.
fn write_pure_death_model(path: &Path) {
    let src = r#"
time_unit = 'days

compartments { S }

parameters {
  mu : rate in [0.001, 10.0]
}

init { S = 1000 }

transitions {
  death : S -->   @ mu * S
}

simulate { from = 0 'days  to = 20 'days }

scenarios {
  slow { set   = { mu = 0.01 } }
  fast { set   = { mu = 0.5  } }
  doubled_from_baseline { scale = { mu = 2.0 } }
}
"#;
    std::fs::write(path, src).unwrap();
}

/// Write a baseline params.toml with mu=0.1 (the fixed "starting value"
/// that `scale` multiplies against).
fn write_baseline_params(path: &Path) {
    std::fs::write(path, "mu = 0.1\n").unwrap();
}

/// Simulate `model` at `params` under scenario `scenario_name` with the
/// given seed, returning S at the final time (t=20). Uses the
/// deterministic ODE backend so there's no RNG variation masking the
/// scenario effect.
fn simulate_terminal_s(
    bin: &Path, model: &Path, params: &Path, scenario: Option<&str>, seed: u64,
) -> f64 {
    let tmp = tempfile::tempdir().unwrap();
    let out_path = tmp.path().join("traj.tsv");
    let mut cmd = Command::new(bin);
    cmd.args([
        "simulate", &model.to_string_lossy(),
        "--params", &params.to_string_lossy(),
        "--backend", "ode",   // deterministic
        "--seed", &seed.to_string(),
        "-o", &out_path.to_string_lossy(),
    ]);
    if let Some(s) = scenario {
        cmd.args(["--scenario", s]);
    }
    let out = cmd.output().expect("spawn");
    assert!(out.status.success(),
        "simulate failed; stderr: {}", String::from_utf8_lossy(&out.stderr));

    // traj.tsv: columns `t`, `S`. Final row = t=20. Return S.
    let content = std::fs::read_to_string(&out_path).unwrap();
    let last_line = content.lines().filter(|l| !l.trim().is_empty()).last().unwrap();
    let fields: Vec<&str> = last_line.split('\t').collect();
    fields[1].parse::<f64>().unwrap()
}

#[test]
fn scenario_set_replaces_mu_value() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("pd.camdl");
    let params = tmp.path().join("p.toml");
    write_pure_death_model(&model);
    write_baseline_params(&params);

    let s_baseline = simulate_terminal_s(&bin, &model, &params, None, 1);
    let s_slow     = simulate_terminal_s(&bin, &model, &params, Some("slow"), 1);
    let s_fast     = simulate_terminal_s(&bin, &model, &params, Some("fast"), 1);

    // Baseline: mu = 0.1, S(20) = 1000 * exp(-0.1 * 20) ≈ 135.3
    // Slow:     mu = 0.01, S(20) = 1000 * exp(-0.01 * 20) ≈ 818.7
    // Fast:     mu = 0.5, S(20) = 1000 * exp(-0.5 * 20) ≈ 0.045
    assert!(s_slow > s_baseline,
        "`set = {{ mu = 0.01 }}` (slower) must leave more S than baseline (mu=0.1). \
         Got: slow={}, baseline={}. If these are equal, the scenario's `set` \
         is not being applied at runtime.", s_slow, s_baseline);
    assert!(s_fast < s_baseline,
        "`set = {{ mu = 0.5 }}` (faster) must leave less S than baseline (mu=0.1). \
         Got: fast={}, baseline={}. If these are equal, the scenario's `set` \
         is not being applied at runtime.", s_fast, s_baseline);

    // Quantitative check: the 'slow' scenario should produce ≈ exp(-0.2) = 0.82
    // fraction, the baseline ≈ exp(-2) = 0.135 fraction. Allow generous
    // tolerance — only testing that the set value is plumbed end-to-end.
    let frac_slow = s_slow / 1000.0;
    assert!(frac_slow > 0.7 && frac_slow < 0.9,
        "slow-scenario terminal S should be ≈ 0.82 × 1000 = 818.7; got {} \
         (frac = {:.3})", s_slow, frac_slow);
}

#[test]
fn scenario_scale_multiplies_mu_value() {
    let Some(bin) = skip_if_missing_binary() else { return; };
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("pd.camdl");
    let params = tmp.path().join("p.toml");
    write_pure_death_model(&model);
    write_baseline_params(&params);

    // Baseline mu = 0.1 → S(20) ≈ 135.3
    // Scale ×2 → mu = 0.2 → S(20) ≈ 18.3
    let s_baseline = simulate_terminal_s(&bin, &model, &params, None, 1);
    let s_doubled  = simulate_terminal_s(&bin, &model, &params,
                                          Some("doubled_from_baseline"), 1);

    assert!(s_doubled < s_baseline,
        "`scale = {{ mu = 2.0 }}` must leave less S than baseline (faster decay). \
         Got: doubled={}, baseline={}. If these are equal, the scenario's `scale` \
         multiplier is not being applied at runtime.", s_doubled, s_baseline);

    // The doubled scenario should produce roughly exp(-4) = 0.0183 fraction.
    let frac = s_doubled / 1000.0;
    assert!(frac < 0.05,
        "scale=2.0 on mu=0.1 → expected terminal S ≈ 18.3 (exp(-4)×1000); got {} \
         (frac = {:.3}). This is the silent-wrong-answer class from the \
         2026-04-21 table-unit incident: if scale were a no-op, \
         we'd see baseline ≈ 135.", s_doubled, frac);
}
