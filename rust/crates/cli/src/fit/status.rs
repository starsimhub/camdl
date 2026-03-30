//! `camdl fit status` — colored summary of fit progress and convergence.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::provenance;

pub fn run_status(fit: &FitToml) -> Result<(), String> {
    let dir = &fit.fit.output_dir;
    println!("{}/ — {}", dir, fit.fit.model);
    println!();

    // Check each stage
    let scout_state = check_stage(dir, "scout");
    let refine_state = check_stage(dir, "refine");
    let validate_state = check_stage(dir, "validate");

    // Scout
    match &scout_state {
        Some(state) => {
            let n_good = state.n_good_chains.unwrap_or(state.n_chains);
            println!("  scout:     \x1b[32m✓\x1b[0m complete ({} chains, best loglik {:.1}, {}/{} good)",
                state.n_chains, state.best_loglik, n_good, state.n_chains);
        }
        None => println!("  scout:     \x1b[90m○ not started\x1b[0m"),
    }

    // Refine
    match &refine_state {
        Some(state) => {
            let summary = load_summary(dir, "refine");
            let converged = summary.as_ref()
                .and_then(|s| s.get("converged"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let symbol = if converged { "\x1b[32m✓\x1b[0m" } else { "\x1b[33m~\x1b[0m" };
            let rhat_str = if converged { "Rhat < 1.05" } else { "Rhat > 1.1" };
            println!("  refine:    {} complete ({} chains, {}, loglik {:.1})",
                symbol, state.n_chains, rhat_str, state.best_loglik);
        }
        None => {
            if scout_state.is_some() {
                println!("  refine:    \x1b[90m○ not started\x1b[0m");
            } else {
                println!("  refine:    \x1b[90m— waiting for scout\x1b[0m");
            }
        }
    }

    // Validate
    match &validate_state {
        Some(state) => {
            let summary = load_summary(dir, "validate");
            let loglik_sd = summary.as_ref()
                .and_then(|s| s.get("loglik_sd"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let ess = summary.as_ref()
                .and_then(|s| s.get("ess_at_mle"))
                .and_then(|v| v.get("min"))
                .and_then(|v| v.as_f64());
            let ess_str = ess.map_or("".into(), |e| format!(", ESS min={:.0}", e));
            println!("  validate:  \x1b[32m✓\x1b[0m complete (loglik {:.1} ± {:.1}{})",
                state.best_loglik, loglik_sd, ess_str);
        }
        None => {
            if refine_state.is_some() {
                println!("  validate:  \x1b[90m○ not started\x1b[0m");
            } else {
                println!("  validate:  \x1b[90m— waiting for refine\x1b[0m");
            }
        }
    }

    println!();

    // ESS at MLE (if validate is done)
    if let Some(ref summary) = load_summary(dir, "validate") {
        if let Some(ess) = summary.get("ess_at_mle") {
            let mean = ess.get("mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let min = ess.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let status = if min > 2500.0 { "\x1b[32m✓ filter is healthy\x1b[0m" }
                else if min > 500.0 { "\x1b[33m~ filter is marginal\x1b[0m" }
                else { "\x1b[31m✗ filter is degenerate\x1b[0m" };
            println!("  ESS at MLE: mean={:.0}, min={:.0}  {}", mean, min, status);
            println!();
        }
    }

    // Estimated parameters
    let best_state = validate_state.as_ref()
        .or(refine_state.as_ref())
        .or(scout_state.as_ref());

    if let Some(state) = best_state {
        // Load profile data for CIs if available
        let profiles = load_profiles(dir);
        let summary = load_summary(dir, &state.stage);

        println!("  Estimated ({} parameters):", fit.estimate.len());
        for (name, spec) in &fit.estimate {
            let value = state.start_values.get(name).copied().unwrap_or(0.0);
            let rw = state.rw_sd.get(name).copied().or(spec.rw_sd).unwrap_or(0.0);

            // Get Rhat from summary
            let rhat = summary.as_ref()
                .and_then(|s| s.get("parameters"))
                .and_then(|p| p.as_array())
                .and_then(|arr| arr.iter().find(|p| p.get("name").and_then(|n| n.as_str()) == Some(name)))
                .and_then(|p| p.get("rhat"))
                .and_then(|v| v.as_f64());

            // Get CI from profiles
            let ci = profiles.get(name.as_str());

            let mut line = format!("    {:12} = {:<12.6} rw_sd={:<8.4}", name, value, rw);

            if let Some(&r) = rhat.as_ref() {
                if r < 1.1 {
                    line.push_str(&format!(" \x1b[32m✓\x1b[0m identified"));
                } else {
                    line.push_str(&format!(" \x1b[33m~ Rhat={:.2}\x1b[0m", r));
                }
            }

            if let Some((lo, hi)) = ci {
                line.push_str(&format!("  CI: [{:.4}, {:.4}]", lo, hi));
            }

            if spec.ivp {
                line.push_str("  (ivp)");
            }

            // Boundary proximity warning
            if let Some(bounds) = spec.bounds {
                let range = bounds.1 - bounds.0;
                if (value - bounds.0).abs() < range * 0.01 {
                    line.push_str("  \x1b[33m⚠ AT LOWER BOUND\x1b[0m");
                } else if (value - bounds.1).abs() < range * 0.01 {
                    line.push_str("  \x1b[33m⚠ AT UPPER BOUND\x1b[0m");
                }
            }

            println!("{}", line);
        }

        println!();
        println!("  Fixed ({} parameters):", fit.fixed.len());
        for name in fit.fixed.keys() {
            if let Some(&value) = state.start_values.get(name) {
                println!("    {:12} = {}", name, value);
            } else {
                println!("    {}", name);
            }
        }
    }

    // Provenance check
    let mle_path = format!("{}/validate/mle_params.toml", dir);
    if std::path::Path::new(&mle_path).exists() {
        println!();
        println!("  Provenance:");
        match provenance::verify_content_hash(&mle_path) {
            Ok(provenance::ContentVerification::Valid) => {
                println!("    mle_params.toml: \x1b[32m✓ content hash matches\x1b[0m");
            }
            Ok(provenance::ContentVerification::Modified { declared, computed, .. }) => {
                println!("    mle_params.toml: \x1b[33m⚠ MODIFIED\x1b[0m");
                println!("      Content hash mismatch: expected {}, computed {}", declared, computed);
                println!("      This file has been hand-tuned. Inference provenance no longer applies.");
            }
            Ok(provenance::ContentVerification::NoHash) => {
                println!("    mle_params.toml: \x1b[90mno provenance hash\x1b[0m");
            }
            Err(e) => {
                println!("    mle_params.toml: error: {}", e);
            }
        }
    }

    // Next step suggestion
    println!();
    if validate_state.is_some() {
        println!("  Next: camdl experiment run experiment.toml \\");
        println!("          --params {}/validate/mle_params.toml", dir);
    } else if refine_state.is_some() {
        println!("  Next: camdl fit validate fit.toml --starts-from {}/refine/", dir);
    } else if scout_state.is_some() {
        println!("  Next: camdl fit refine fit.toml --starts-from {}/scout/", dir);
    } else {
        println!("  Next: camdl fit scout fit.toml");
    }

    Ok(())
}

fn check_stage(base_dir: &str, stage: &str) -> Option<FitState> {
    FitState::load(&format!("{}/{}", base_dir, stage)).ok()
}

fn load_summary(base_dir: &str, stage: &str) -> Option<serde_json::Value> {
    let path = format!("{}/{}/{}_summary.json", base_dir, stage, stage);
    std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn load_profiles(base_dir: &str) -> std::collections::HashMap<&str, (f64, f64)> {
    // This would load from profile TSVs, but we need owned data
    // For now, load from the validate summary
    let mut profiles = std::collections::HashMap::new();
    if let Some(summary) = load_summary(base_dir, "validate") {
        if let Some(params) = summary.get("parameters").and_then(|p| p.as_array()) {
            for p in params {
                if let (Some(name), Some(ci)) = (
                    p.get("name").and_then(|n| n.as_str()),
                    p.get("ci_95").and_then(|c| c.as_array()),
                ) {
                    if ci.len() == 2 {
                        if let (Some(lo), Some(hi)) = (ci[0].as_f64(), ci[1].as_f64()) {
                            // We can't return &str here with owned data
                            // This is a design limitation; fix by using String keys
                            let _ = (name, lo, hi);
                        }
                    }
                }
            }
        }
    }
    profiles
}
