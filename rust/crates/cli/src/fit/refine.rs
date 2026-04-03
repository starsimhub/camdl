//! `camdl fit refine` — convergent IF2 from scout's best parameters.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::runner::{self, FitRunConfig};
use crate::fit::provenance::{self, MleMetadata};
use crate::hashing;
use sha2::Digest;
use std::collections::HashMap;

const REFINE_CHAINS: usize = 4;
const REFINE_PARTICLES: usize = 1000;
const REFINE_ITERATIONS: usize = 50;
const REFINE_COOLING: f64 = 0.05; // cf50: 5% at halfway, 0.25% at end — converge to MLE

pub fn run_refine(fit: &FitToml, starts_from: &str, seed: u64, force: bool) -> Result<(), String> {
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

    let mut config = FitRunConfig::build(
        fit, Some(&prior_state),
        n_chains, n_particles, n_iterations,
        cooling, seed, false,
    )?;

    if rw_sd_scale != 1.0 {
        for p in &mut config.if2_params { p.rw_sd *= rw_sd_scale; }
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

    let t0 = std::time::Instant::now();
    let chain_results = runner::run_chains(&config);
    let elapsed = t0.elapsed();

    // Check convergence
    let all_converged = chain_results.rhat.values().all(|&r| r < 1.1);
    let loglik_spread = {
        let logliks: Vec<f64> = chain_results.results.iter()
            .map(|(_, r)| r.final_loglik).collect();
        logliks.iter().cloned().fold(f64::NEG_INFINITY, f64::max) -
        logliks.iter().cloned().fold(f64::INFINITY, f64::min)
    };

    if !all_converged {
        eprintln!("\nwarning: not all parameters converged (Rhat > 1.1)");
        eprintln!("  consider running with more iterations or particles");
    }
    if loglik_spread > 10.0 {
        eprintln!("\nwarning: loglik spread across chains = {:.1} (> 10)", loglik_spread);
    }

    // Best chain's MLE
    let best = &chain_results.results.iter()
        .find(|(id, _)| *id == chain_results.best_chain)
        .unwrap().1;

    // Start values for next stage
    let start_values: HashMap<String, f64> = runner::collect_all_params(
        &best.mle, &config.if2_params, &config.model,
        &config.base_params, &config.compiled,
    );

    // Auto rw_sd from this stage's convergence
    let rw_sd = match runner::auto_rw_sd(&chain_results.results, &config.if2_params) {
        Ok((rw, _)) => rw,
        Err(_) => {
            // If auto fails, halve the incoming rw_sd
            config.if2_params.iter()
                .map(|s| (s.name.clone(), s.rw_sd * 0.5))
                .collect()
        }
    };

    let state = FitState {
        stage: "refine".into(),
        seed,
        timestamp: crate::fit::scout::now_iso8601_pub(),
        input_hash: Some(input_hash.clone()),
        camdl_version: Some(crate::version::VERSION_SHORT.into()),
        best_loglik: chain_results.best_loglik,
        initial_loglik: prior_state.best_loglik,
        best_chain: chain_results.best_chain,
        n_chains: n_chains,
        n_good_chains: None,
        start_values,
        rw_sd,
    };
    state.save(&stage_dir)?;

    // Write per-chain outputs
    let param_names: Vec<String> = config.model.parameters.iter().map(|p| p.name.clone()).collect();
    runner::write_chain_outputs(
        &stage_dir, &chain_results.results, &config.if2_params,
        &param_names, &config.base_params, &config.compiled,
    )?;
    runner::write_diagnostics(&stage_dir, &chain_results.results)?;

    // Write mle_params.toml
    let all_params = runner::collect_all_params(
        &best.mle, &config.if2_params, &config.model,
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
        loglik: chain_results.best_loglik,
        loglik_sd: 0.0, // Not computed in refine
        n_particles: n_particles,
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
        "parameters": config.if2_params.iter().map(|spec| {
            let rhat = results.rhat.get(&spec.name).copied().unwrap_or(f64::NAN);
            let best = &results.results.iter()
                .find(|(id, _)| *id == results.best_chain).unwrap().1;
            serde_json::json!({
                "name": spec.name,
                "estimate": best.mle[spec.index],
                "rhat": rhat,
            })
        }).collect::<Vec<_>>(),
    });

    let path = format!("{}/refine_summary.json", dir);
    std::fs::write(&path, serde_json::to_string_pretty(&summary).unwrap())
        .map_err(|e| format!("cannot write {}: {}", path, e))
}
