//! `camdl fit refine` — convergent IF2 from scout's best parameters.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::runner::{self, FitRunConfig};
use crate::fit::provenance::{self, MleMetadata};
use crate::hashing;
use sha2::Digest;
use sim::inference::diagnostic::{DiagnosticCollector, DiagnosticKind};
use std::collections::HashMap;

const REFINE_CHAINS: usize = 4;
const REFINE_PARTICLES: usize = 1000;
const REFINE_ITERATIONS: usize = 50;
const REFINE_COOLING: f64 = 0.05; // cf50: 5% at halfway, 0.25% at end — converge to MLE

pub fn run_refine(
    fit: &FitToml, starts_from: &str, seed: u64, force: bool,
    allow_nonconverged_scout: bool,
) -> Result<(), String> {
    let stage_dir = format!("{}/refine", fit.fit.output_dir);
    let rc = fit.refine.as_ref();

    let n_chains = rc.and_then(|s| s.chains).unwrap_or(REFINE_CHAINS);
    let n_particles = rc.and_then(|s| s.particles).unwrap_or(REFINE_PARTICLES);
    let n_iterations = rc.and_then(|s| s.iterations).unwrap_or(REFINE_ITERATIONS);
    let cooling = rc.and_then(|s| s.cooling).unwrap_or(REFINE_COOLING);
    let rw_sd_scale = rc.and_then(|s| s.rw_sd_scale).unwrap_or(1.0);

    let prior_state = FitState::load(starts_from)?;
    if !prior_state.best_loglik.is_finite() {
        return Err(format!(
            "prior stage produced -inf loglik — cannot use as starting point.\n\
             Re-run the prior stage with more particles or check model specification.\n\
             Source: {}/fit_state.toml", starts_from
        ));
    }
    eprintln!("refine: starting from {} (loglik={:.1}, {} good chains)",
        starts_from, prior_state.best_loglik,
        prior_state.n_good_chains.unwrap_or(prior_state.n_chains));

    // Gate 1: compound scout-convergence check (Â + decibans-spread).
    // See docs/dev/proposals/2026-04-19-refine-gates-scout-convergence.md
    // and the §Proposal 3 extension in the 2026-04-24 if2-scout
    // remediation proposal. The legacy `camdl fit refine` subcommand
    // doesn't carry a per-stage GateConfig, so the proposal's defaults
    // (a_thresh=1.01, decibans_thresh=30.0) apply.
    let gate = crate::fit::config_v2::GateConfig::default();
    match crate::fit::gating::check_scout_convergence(&prior_state, &gate) {
        crate::fit::gating::ScoutGateVerdict::Ok => {}
        crate::fit::gating::ScoutGateVerdict::SoftWarn { param_agreement } => {
            eprintln!("\x1b[33m  warning:\x1b[0m scout tail Â in the SoftWarn \
                       band ([{:.2}, {:.2})) for: {}",
                crate::fit::gating::A_SOFT, gate.a_thresh,
                param_agreement.iter()
                    .map(|(n, r)| format!("{} (Â={:.2})", n, r))
                    .collect::<Vec<_>>().join(", "));
            eprintln!("  proceeding with refine, but scout's convergence is \
                       weaker than ideal — inspect results carefully.");
        }
        crate::fit::gating::ScoutGateVerdict::Hard { failing, all_structural, ivp, loglik_spread } => {
            let msg = crate::fit::gating::format_hard_verdict(
                &failing, &all_structural, &ivp,
                loglik_spread, prior_state.best_loglik,
                None,
            );
            if allow_nonconverged_scout {
                eprintln!("\x1b[33m  warning:\x1b[0m {}", msg);
                eprintln!("\n  --allow-nonconverged-scout: proceeding anyway.");
            } else {
                return Err(msg);
            }
        }
        crate::fit::gating::ScoutGateVerdict::DecibansSpread {
            delta_db, threshold_db, sigma_max, chain_logliks,
        } => {
            let msg = crate::fit::gating::format_decibans_spread_verdict(
                delta_db, threshold_db, sigma_max, &chain_logliks);
            if allow_nonconverged_scout {
                eprintln!("\x1b[33m  warning:\x1b[0m {}", msg);
                eprintln!("\n  --allow-nonconverged-scout: proceeding anyway.");
            } else {
                return Err(msg);
            }
        }
    }
    // Snapshot scout's chain logliks + best for the post-refine Gate 2
    // check. Even if Gate 1 was overridden, Gate 2 still fires.
    let scout_best_loglik = prior_state.best_loglik;
    let scout_chain_logliks = prior_state.chain_logliks.clone();

    let mut config = FitRunConfig::build(
        fit, Some(&prior_state),
        n_chains, n_particles, n_iterations,
        cooling, seed, false,
    )?;

    if rw_sd_scale != 1.0 {
        for p in &mut config.estimated_params { p.rw_sd *= rw_sd_scale; }
        eprintln!("refine: rw_sd scaled by {:.1}×", rw_sd_scale);
    }

    // Cache check
    let input_hash = runner::compute_fit_input_hash(fit, &config, seed);
    if !force {
        match provenance::check_cache(&stage_dir, &input_hash) {
            provenance::CacheStatus::Match => {
                eprintln!("\x1b[33mrefine skipped — results already exist for these inputs.\x1b[0m");
                eprintln!("  output:     {}/", stage_dir);
                eprintln!("  input hash: {}", input_hash);
                eprintln!("  Use --force to re-run.");
                return Ok(());
            }
            provenance::CacheStatus::Mismatch => {
                eprintln!("\x1b[33mrefine — prior results exist but inputs have changed. Re-running.\x1b[0m");
            }
            provenance::CacheStatus::NotFound => {}
        }
    }

    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create {}: {}", stage_dir, e))?;

    let collector = DiagnosticCollector::new("refine");

    let t0 = std::time::Instant::now();
    let chain_results = runner::run_chains_with_diagnostics(&config, &collector);
    let elapsed = t0.elapsed();

    // Gate 2: refine must not regress below scout. Not overridable —
    // if this fires, refine landed in a worse basin than scout had
    // found, which is a run-time pipeline failure rather than a
    // user-facing choice. Fail before writing any "refine completed"
    // artefact so the filesystem matches the actual truth of the run.
    crate::fit::gating::check_loglik_regression(
        scout_best_loglik,
        chain_results.best_loglik,
        &scout_chain_logliks,
    )?;

    // Check convergence
    let all_converged = chain_results.chain_agreement.values().all(|&r| r < 1.1);
    let loglik_spread = {
        let logliks: Vec<f64> = chain_results.results.iter()
            .map(|(_, r)| r.final_loglik).collect();
        logliks.iter().cloned().fold(f64::NEG_INFINITY, f64::max) -
        logliks.iter().cloned().fold(f64::INFINITY, f64::min)
    };

    if !all_converged {
        let max_chain_agreement = chain_results.chain_agreement.values().cloned().fold(0.0_f64, f64::max);
        let n_unconverged = chain_results.chain_agreement.values().filter(|&&r| r > 1.1).count();
        let n_total = chain_results.chain_agreement.len();
        collector.push(DiagnosticKind::ConvergenceIncomplete { max_chain_agreement, n_unconverged, n_total });
    }
    if loglik_spread > 10.0 {
        let max_chain_agreement = chain_results.chain_agreement.values().cloned().fold(0.0_f64, f64::max);
        collector.push(DiagnosticKind::MultimodalLikelihood { ll_spread: loglik_spread, max_chain_agreement });
    }

    // Best chain's MLE
    let best = &chain_results.results.iter()
        .find(|(id, _)| *id == chain_results.best_chain)
        .unwrap().1;

    // Start values for next stage
    let start_values: HashMap<String, f64> = runner::collect_all_params(
        &best.mle, &config.estimated_params, &config.model,
        &config.base_params, &config.compiled,
    );

    // Auto rw_sd from this stage's convergence
    let rw_sd = match runner::auto_rw_sd(&chain_results.results, &config.estimated_params) {
        Ok((rw, _)) => rw,
        Err(_) => {
            // If auto fails, halve the incoming rw_sd
            config.estimated_params.iter()
                .map(|s| (s.name.clone(), s.rw_sd * 0.5))
                .collect()
        }
    };

    let refine_chain_logliks: Vec<f64> = chain_results.results.iter()
        .map(|(_, r)| r.final_loglik).collect();
    let refine_ivp_params: Vec<String> = config.estimated_params.iter()
        .filter(|p| p.ivp).map(|p| p.name.clone()).collect();

    let state = FitState {
        stage: "refine".into(),
        seed,
        timestamp: crate::fit::scout::now_iso8601_pub(),
        input_hash: Some(input_hash.clone()),
        camdl_version: Some(crate::version::VERSION_SHORT.into()),
        best_loglik: chain_results.best_loglik,
        initial_loglik: prior_state.best_loglik,
        best_chain: chain_results.best_chain,
        n_chains,
        n_good_chains: None,
        start_values,
        rw_sd,
        loglik_type: Some("if2".into()),
        acceptance_rate: None,
        tail_chain_agreement: chain_results.chain_agreement.clone(),
        ivp_params: refine_ivp_params,
        chain_logliks: refine_chain_logliks,
        chain_clean_logliks: chain_results.chain_clean_logliks(),
        chain_clean_ses: chain_results.chain_clean_ses(),
    };
    state.save(&stage_dir)?;

    // Write per-chain outputs
    let param_names: Vec<String> = config.model.parameters.iter().map(|p| p.name.clone()).collect();
    runner::write_chain_outputs(
        &stage_dir, &chain_results.results, &config.estimated_params,
        &param_names, &config.base_params, &config.compiled,
        Some(&chain_results.clean_eval),
    )?;
    runner::write_clean_eval_tsv(
        &stage_dir, &chain_results.clean_eval, &config.estimated_params,
    )?;
    runner::write_run_root_final_params(
        &stage_dir, &chain_results.clean_eval, &config.estimated_params,
        &param_names, &config.base_params, &config.compiled,
    )?;
    runner::write_chain_starts(
        &stage_dir, None, &config.estimated_params, n_chains,
    )?;
    runner::write_diagnostics(&stage_dir, &chain_results.results)?;

    // Write mle_params.toml
    let all_params = runner::collect_all_params(
        &best.mle, &config.estimated_params, &config.model,
        &config.base_params, &config.compiled,
    );
    let model_hash = hashing::model_hash(&config.model_ir_json);
    let input_hash = runner::compute_fit_input_hash(fit, &config, seed);

    let metadata = MleMetadata {
        input_hash: input_hash.clone(),
        model_path: fit.fit.model.clone(),
        model_hash,
        data_hashes: fit.data.iter().map(|(name, path)| {
            let bytes = std::fs::read(path).unwrap_or_default();
            let hash = hex::encode(&sha2::Sha256::digest(&bytes)[..4]);
            (format!("{} ({})", name, path), hash)
        }).collect(),
        seed,
        stage: "refine".into(),
        best_chain: chain_results.best_chain,
        backend: fit.config.backend.clone(),
        dt: fit.config.dt,
        loglik: chain_results.best_loglik,
        loglik_sd: 0.0, // Not computed in refine
        n_particles,
        ess_at_mle: None,
        timestamp: state.timestamp.clone(),
    };
    provenance::write_mle_params(
        &format!("{}/mle_params.toml", stage_dir),
        &all_params,
        &metadata,
    )?;

    // Write summary
    write_summary(&stage_dir, &chain_results, &config, all_converged, loglik_spread)?;

    // Render and persist diagnostics
    collector.render_to_stderr();
    let _ = collector.write_json(&format!("{}/diagnostics.json", stage_dir));

    let wall_secs = elapsed.as_secs_f64();
    eprintln!("\nrefine complete in {:.1}s: {}/", wall_secs, stage_dir);
    eprintln!("  best loglik: {:.1} (chain {})", chain_results.best_loglik, chain_results.best_chain + 1);
    eprintln!("  converged: {}", if all_converged { "yes" } else { "NO" });
    eprintln!("\nnext: camdl fit validate fit.toml --starts-from {}/", stage_dir);

    Ok(())
}


fn write_summary(
    dir: &str,
    results: &runner::ChainResults,
    config: &FitRunConfig,
    converged: bool,
    loglik_spread: f64,
) -> Result<(), String> {
    let summary = serde_json::json!({
        "stage": "refine",
        "n_chains": config.n_chains,
        "best_loglik": results.best_loglik,
        "best_chain": results.best_chain + 1,
        "converged": converged,
        "loglik_spread": loglik_spread,
        "parameters": config.estimated_params.iter().map(|spec| {
            let agreement = results.chain_agreement.get(&spec.name).copied().unwrap_or(f64::NAN);
            let best = &results.results.iter()
                .find(|(id, _)| *id == results.best_chain).unwrap().1;
            serde_json::json!({
                "name": spec.name,
                "estimate": best.mle[spec.index],
                "chain_agreement": agreement,
            })
        }).collect::<Vec<_>>(),
    });

    let path = format!("{}/refine_summary.json", dir);
    std::fs::write(&path, serde_json::to_string_pretty(&summary).unwrap())
        .map_err(|e| format!("cannot write {}: {}", path, e))
}
