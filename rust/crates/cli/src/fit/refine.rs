//! `camdl fit refine` — convergent IF2 from scout's best parameters.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::runner::{self, FitRunConfig};
use crate::fit::provenance::{self, MleMetadata};
use crate::hashing;
use sha2::Digest;
use std::collections::HashMap;

const DEFAULT_CHAINS: usize = 4;
const DEFAULT_PARTICLES: usize = 1000;
const DEFAULT_ITERATIONS: usize = 50;
const DEFAULT_COOLING: f64 = 0.95;

pub fn run_refine(fit: &FitToml, starts_from: &str, seed: u64, force: bool) -> Result<(), String> {
    let stage_dir = format!("{}/refine", fit.fit.output_dir);
    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create {}: {}", stage_dir, e))?;

    let prior_state = FitState::load(starts_from)?;
    eprintln!("refine: starting from {} (loglik={:.1}, {} good chains)",
        starts_from, prior_state.best_loglik,
        prior_state.n_good_chains.unwrap_or(prior_state.n_chains));

    let config = FitRunConfig::build(
        fit, Some(&prior_state),
        DEFAULT_CHAINS, DEFAULT_PARTICLES, DEFAULT_ITERATIONS,
        DEFAULT_COOLING, seed, false,
    )?;

    let t0 = std::time::Instant::now();
    let chain_results = runner::run_chains(&config);
    let elapsed = t0.elapsed();

    // Check convergence
    let all_converged = chain_results.rhat.values().all(|&r| r < 1.1);
    let loglik_spread = {
        let logliks: Vec<f64> = chain_results.results.iter().map(|(_, r)| r.final_loglik).collect();
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
    let start_values: HashMap<String, f64> = config.if2_params.iter()
        .map(|spec| (spec.name.clone(), best.mle[spec.index]))
        .collect();

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
        best_loglik: chain_results.best_loglik,
        initial_loglik: prior_state.best_loglik,
        best_chain: chain_results.best_chain,
        n_chains: DEFAULT_CHAINS,
        n_good_chains: None,
        start_values,
        rw_sd,
    };
    state.save(&stage_dir)?;

    // Write per-chain outputs
    let param_names: Vec<String> = config.model.parameters.iter().map(|p| p.name.clone()).collect();
    runner::write_chain_outputs(
        &stage_dir, &chain_results.results, &config.if2_params,
        &param_names, &config.base_params,
    )?;
    runner::write_diagnostics(&stage_dir, &chain_results.results)?;

    // Write mle_params.toml
    let all_params = runner::collect_all_params(
        &best.mle, &config.if2_params, &config.model,
        &config.base_params, &config.compiled,
    );
    let model_hash = hashing::model_hash(&config.model_ir_json);
    let input_hash = compute_input_hash(fit, &config, seed);

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
        n_particles: DEFAULT_PARTICLES,
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

fn compute_input_hash(fit: &FitToml, config: &FitRunConfig, seed: u64) -> String {
    let fit_toml_bytes = toml::to_string(fit).unwrap_or_default().into_bytes();
    let mut data_files: Vec<(String, Vec<u8>)> = fit.data.iter().map(|(name, path)| {
        (name.clone(), std::fs::read(path).unwrap_or_default())
    }).collect();
    provenance::compute_input_hash(
        config.model_ir_json.as_bytes(),
        &mut data_files,
        &fit_toml_bytes,
        seed,
    )
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
