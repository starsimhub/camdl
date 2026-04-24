//! Refine-stage convergence gates.
//!
//! Two gates protect against the "refine launders an unconverged
//! scout" failure mode documented in
//! `docs/dev/proposals/2026-04-19-refine-gates-scout-convergence.md`:
//!
//! - Gate 1 (pre-refine): scout's tail chain-agreement (Â) on every
//!   non-IVP estimated parameter must be ≤ `A_HARD`. If it isn't, refine refuses to
//!   start. Overridable via `--allow-nonconverged-scout`.
//!
//! - Gate 2 (post-refine): refine's best loglik must not regress
//!   below scout's by more than a tolerance ε. If it does, refine's
//!   output is rejected — this is a near-certain bug in the run
//!   itself, not a statistical choice, so there's no override.
//!
//! Both gates produce actionable error messages that name the failing
//! values AND suggest fixes.

use super::state::FitState;

/// Hard threshold: any non-IVP param with tail Â > this blocks
/// refine from running. Matches Brooks-Gelman-Rubin convention
/// (the underlying formula is G-R 1992; we relabel the output as
/// chain agreement because it is applied to IF2 optimizer chains,
/// not posterior samples).
pub const A_HARD: f64 = 1.10;

/// Soft threshold: params between these get a prominent warning but
/// refine still runs. Matches the existing scout diagnostic
/// colour-coding (red ≥ 1.10, yellow 1.05–1.10, green < 1.05).
pub const A_SOFT: f64 = 1.05;

/// Minimum ε for Gate 2. Scout's noise floor on a typical PF-based
/// loglik estimator at reasonable particle counts. `epsilon` takes the
/// max of this and `2 * σ_scout_chains` so multi-modal scout runs (high
/// between-chain σ) get a proportionally wider tolerance.
pub const LOGLIK_EPSILON_MIN: f64 = 3.0;

/// Verdict from the pre-refine convergence check. `SoftWarn` callers
/// should print the named parameters prominently. `Hard` callers
/// should error unless the user passed `--allow-nonconverged-scout`,
/// in which case downgrade to SoftWarn.
#[derive(Debug)]
pub enum ScoutGateVerdict {
    Ok,
    SoftWarn { param_agreement: Vec<(String, f64)> },
    Hard {
        /// All non-IVP params with Â > A_HARD. Named and sorted
        /// worst-first so the error message leads with the most
        /// obvious failure.
        failing: Vec<(String, f64)>,
        /// Every non-IVP Â, for the full diagnostic table.
        all_structural: Vec<(String, f64)>,
        /// IVP Â values (reported but not gated).
        ivp: Vec<(String, f64)>,
        /// Spread across scout's per-chain final logliks. A wide
        /// spread is the strongest signal of multi-modality.
        loglik_spread: f64,
    },
}

/// Check scout's fit_state for pre-refine convergence.
pub fn check_scout_convergence(scout: &FitState) -> ScoutGateVerdict {
    // Absent tail_chain_agreement means legacy fit_state — can't gate.
    // Caller handles the warn-and-proceed branch.
    if scout.tail_chain_agreement.is_empty() {
        return ScoutGateVerdict::Ok;
    }

    let ivp_set: std::collections::HashSet<&str> = scout.ivp_params.iter()
        .map(|s| s.as_str()).collect();
    let mut structural: Vec<(String, f64)> = Vec::new();
    let mut ivp: Vec<(String, f64)> = Vec::new();
    for (name, &agreement) in &scout.tail_chain_agreement {
        if ivp_set.contains(name.as_str()) {
            ivp.push((name.clone(), agreement));
        } else {
            structural.push((name.clone(), agreement));
        }
    }
    structural.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ivp.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let worst = structural.iter().map(|(_, r)| *r)
        .fold(0.0_f64, f64::max);

    if worst > A_HARD {
        let failing: Vec<(String, f64)> = structural.iter()
            .filter(|(_, r)| *r > A_HARD)
            .cloned().collect();
        let loglik_spread = if scout.chain_logliks.len() >= 2 {
            let hi = scout.chain_logliks.iter().cloned()
                .fold(f64::NEG_INFINITY, f64::max);
            let lo = scout.chain_logliks.iter().cloned()
                .fold(f64::INFINITY, f64::min);
            hi - lo
        } else { 0.0 };
        ScoutGateVerdict::Hard {
            failing,
            all_structural: structural,
            ivp,
            loglik_spread,
        }
    } else if worst > A_SOFT {
        let warnable: Vec<(String, f64)> = structural.into_iter()
            .filter(|(_, r)| *r > A_SOFT)
            .collect();
        ScoutGateVerdict::SoftWarn { param_agreement: warnable }
    } else {
        ScoutGateVerdict::Ok
    }
}

/// Compute the ε tolerance for Gate 2: `max(LOGLIK_EPSILON_MIN,
/// 2 · σ(scout.chain_logliks))`. A wider scout spread (more evidence
/// of multi-modality) gives refine proportionally more room.
pub fn loglik_regression_epsilon(scout_chain_logliks: &[f64]) -> f64 {
    if scout_chain_logliks.len() < 2 {
        return LOGLIK_EPSILON_MIN;
    }
    let n = scout_chain_logliks.len() as f64;
    let mean = scout_chain_logliks.iter().sum::<f64>() / n;
    let var = scout_chain_logliks.iter()
        .map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let two_sigma = 2.0 * var.sqrt();
    LOGLIK_EPSILON_MIN.max(two_sigma)
}

/// Check Gate 2: refine's best loglik must not be worse than scout's
/// by more than ε. Returns `Ok(())` on pass, `Err(msg)` with a
/// human-readable diagnosis naming both logliks, the delta, and ε.
pub fn check_loglik_regression(
    scout_best: f64,
    refine_best: f64,
    scout_chain_logliks: &[f64],
) -> Result<(), String> {
    let epsilon = loglik_regression_epsilon(scout_chain_logliks);
    let delta = refine_best - scout_best;
    if delta >= -epsilon {
        Ok(())
    } else {
        Err(format!(
            "refine regressed below scout.\n\n  \
             scout  best_loglik = {:.1}\n  \
             refine best_loglik = {:.1}   delta = {:+.1}, threshold ε = {:.1}\n\n  \
             Refine landed in a worse basin than scout found. This is a\n  \
             pipeline failure, not a user-facing knob — refine is supposed\n  \
             to polish scout's best, not regress from it. Possible causes:\n\n    \
             - scout was multi-modal and refine's starts_from filter picked\n      \
             top-K chains from the wrong basin (re-run with tighter bounds\n      \
             around scout's best chain)\n    \
             - refine cooling too aggressive given rw_sd; collapsed on the\n      \
             first accessible local maximum\n    \
             - the model or data changed between stages (hash mismatch —\n      \
             check run.json)\n\n  \
             scout/fit_state.toml is authoritative for \"what scout's best\n  \
             looked like.\" Investigate before re-running.",
            scout_best, refine_best, delta, epsilon))
    }
}

/// Render the Gate 1 Hard verdict as a human error message.
pub fn format_hard_verdict(
    failing: &[(String, f64)],
    all_structural: &[(String, f64)],
    ivp: &[(String, f64)],
    loglik_spread: f64,
    scout_best_loglik: f64,
    scout_best_chain_values: Option<&[(String, f64)]>,
) -> String {
    let mut msg = String::from(
        "refine stage requires scout convergence.\n\n  \
         Scout tail Â (last half of iterations):\n");
    for (name, agreement) in all_structural {
        let marker = if *agreement > A_HARD { "✗" }
                     else if *agreement > A_SOFT { "~" }
                     else { " " };
        msg.push_str(&format!("    {} {:<10} Â = {:>6.3}{}\n",
            marker, name, agreement,
            if *agreement > A_HARD { "   (> 1.10)" } else { "" }));
    }
    for (name, agreement) in ivp {
        msg.push_str(&format!("      {:<10} Â = {:>6.3}   (ivp — not gated)\n",
            name, agreement));
    }
    if loglik_spread > 0.0 {
        msg.push_str(&format!("\n  Scout loglik spread: {:.1} (best chain loglik {:.1})\n",
            loglik_spread, scout_best_loglik));
    }
    if loglik_spread > LOGLIK_EPSILON_MIN * 3.0 {
        msg.push_str("  -> likelihood surface is almost certainly multi-modal.\n");
    }
    msg.push_str(&format!("\n  Failing: {}\n",
        failing.iter().map(|(n, r)| format!("{} (Â={:.2})", n, r))
            .collect::<Vec<_>>().join(", ")));
    msg.push_str("\n  Pick one:\n    \
                  - re-run scout with more chains or iterations\n    \
                  - narrow bounds to the basin scout's best chain found");
    if let Some(vals) = scout_best_chain_values {
        msg.push_str(":\n");
        for (name, value) in vals {
            msg.push_str(&format!("        {} ≈ {:.4}\n", name, value));
        }
        msg.push_str("      copy into [estimate.*] bounds / start values\n    ");
    } else {
        msg.push_str("\n    ");
    }
    msg.push_str("- mark weakly-identified params as `ivp = true`\n      \
                  (reported but not gated)\n\n  \
                  To run refine anyway (results may launder multi-modality):\n    \
                  camdl fit run fit.toml --allow-nonconverged-scout");
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_state(
        tail_chain_agreement: &[(&str, f64)],
        ivp_params: &[&str],
        chain_logliks: &[f64],
        best_loglik: f64,
    ) -> FitState {
        FitState {
            stage: "scout".into(),
            seed: 1,
            timestamp: "2026-04-19T00:00:00Z".into(),
            input_hash: None, camdl_version: None,
            best_loglik,
            initial_loglik: f64::NEG_INFINITY,
            best_chain: 0,
            n_chains: chain_logliks.len().max(1),
            n_good_chains: None,
            start_values: HashMap::new(),
            rw_sd: HashMap::new(),
            loglik_type: Some("if2".into()),
            acceptance_rate: None,
            tail_chain_agreement: tail_chain_agreement.iter()
                .map(|(k, v)| (k.to_string(), *v)).collect(),
            ivp_params: ivp_params.iter().map(|s| s.to_string()).collect(),
            chain_logliks: chain_logliks.to_vec(),
        }
    }

    #[test]
    fn hard_gate_fires_when_structural_agreement_exceeds_threshold() {
        let s = make_state(
            &[("beta", 3.5), ("gamma", 1.2), ("I0", 16.5)],
            &["I0"],
            &[-60.2, -62.5, -63.3, -64.5, -66.2, -68.7, -854.6],
            -60.2,
        );
        match check_scout_convergence(&s) {
            ScoutGateVerdict::Hard { failing, loglik_spread, .. } => {
                let names: Vec<&str> = failing.iter()
                    .map(|(n, _)| n.as_str()).collect();
                assert!(names.contains(&"beta"),
                    "beta (Â=3.5) must fail the gate: {:?}", names);
                assert!(names.contains(&"gamma"),
                    "gamma (Â=1.2) must fail: {:?}", names);
                // IVP param I0 must NOT appear in failing.
                assert!(!names.contains(&"I0"),
                    "I0 is ivp — must not be in failing: {:?}", names);
                // Loglik spread computed: 854.6 − 60.2 = 794.4
                assert!((loglik_spread - 794.4).abs() < 0.1,
                    "loglik spread {:.1}, expected 794.4", loglik_spread);
            }
            other => panic!("expected Hard, got {:?}", other),
        }
    }

    #[test]
    fn ivp_agreement_not_gated_even_when_extreme() {
        // All structural params are green; only IVP has extreme Â.
        // Gate should pass — IVP is expected to be hard to identify.
        let s = make_state(
            &[("beta", 1.02), ("gamma", 1.01), ("I0", 16.5), ("R_init", 5.5)],
            &["I0", "R_init"],
            &[-60.2, -60.5],
            -60.2,
        );
        match check_scout_convergence(&s) {
            ScoutGateVerdict::Ok => (),
            other => panic!("expected Ok (IVP exempt), got {:?}", other),
        }
    }

    #[test]
    fn soft_warn_between_thresholds() {
        let s = make_state(
            &[("beta", 1.07), ("gamma", 1.02)],
            &[],
            &[-60.2, -60.5],
            -60.2,
        );
        match check_scout_convergence(&s) {
            ScoutGateVerdict::SoftWarn { param_agreement } => {
                let names: Vec<&str> = param_agreement.iter()
                    .map(|(n, _)| n.as_str()).collect();
                assert!(names.contains(&"beta"));
                assert!(!names.contains(&"gamma"),
                    "gamma (1.02) is below soft threshold, shouldn't be warned");
            }
            other => panic!("expected SoftWarn, got {:?}", other),
        }
    }

    #[test]
    fn legacy_state_with_no_agreement_returns_ok() {
        // Absent tail_chain_agreement (legacy fit_state from pre-2026-04-19):
        // caller treats this as "unknown, warn and proceed" via the
        // Ok verdict.
        let s = make_state(&[], &[], &[-60.0], -60.0);
        match check_scout_convergence(&s) {
            ScoutGateVerdict::Ok => (),
            other => panic!("legacy state → Ok, got {:?}", other),
        }
    }

    #[test]
    fn loglik_regression_fires_when_refine_below_scout() {
        // Scout best = -60.2; refine best = -76.3. Regression of 16.1.
        // Scout chain spread is wide (-60.2 to -68.7, σ ≈ 3), so
        // ε = max(3, 2·3) ≈ 6. Delta of -16.1 >> ε → error.
        let scout_lls = vec![-60.2, -62.5, -63.3, -64.5, -66.2, -68.7];
        let err = check_loglik_regression(-60.2, -76.3, &scout_lls)
            .expect_err("refine regressed far below scout");
        assert!(err.contains("-60.2") && err.contains("-76.3"),
            "error must name both logliks: {}", err);
        assert!(err.contains("regressed"),
            "error must use the word 'regressed': {}", err);
    }

    #[test]
    fn loglik_regression_tolerates_small_delta() {
        // Scout best = -60.2; refine best = -62.0. Delta 1.8 < ε (3).
        // Should pass — within the noise floor of the PF loglik.
        let scout_lls = vec![-60.2, -60.3, -60.1, -60.4];  // tight
        check_loglik_regression(-60.2, -62.0, &scout_lls)
            .expect("small regression within ε should pass");
    }

    #[test]
    fn loglik_regression_passes_when_refine_better() {
        // Refine improved on scout's best — always passes.
        let scout_lls = vec![-60.2, -62.5, -63.3];
        check_loglik_regression(-60.2, -58.0, &scout_lls)
            .expect("refine improvement must pass");
    }

    #[test]
    fn epsilon_widens_with_scout_loglik_spread() {
        let tight = vec![-60.0, -60.1, -60.0, -59.9];
        let wide  = vec![-60.0, -70.0, -80.0, -55.0];
        let eps_tight = loglik_regression_epsilon(&tight);
        let eps_wide  = loglik_regression_epsilon(&wide);
        assert!(eps_wide > eps_tight * 2.0,
            "wider scout spread should give proportionally larger ε: \
             tight={:.2}, wide={:.2}", eps_tight, eps_wide);
        assert!(eps_tight >= LOGLIK_EPSILON_MIN,
            "ε must never drop below the floor: {}", eps_tight);
    }
}
