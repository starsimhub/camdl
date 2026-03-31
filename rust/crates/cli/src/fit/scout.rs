//! `camdl fit scout` — landscape discovery with random starts and MAD-based auto rw_sd.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::runner::{self, FitRunConfig};
use sim::inference::if2::IF2Param;
use std::collections::HashMap;

const DEFAULT_CHAINS: usize = 8;
const DEFAULT_PARTICLES: usize = 200;
const DEFAULT_ITERATIONS: usize = 20;
const DEFAULT_COOLING: f64 = 1.0; // no cooling — pure exploration

pub fn run_scout(fit: &FitToml, seed: u64, force: bool) -> Result<(), String> {
    let stage_dir = format!("{}/scout", fit.fit.output_dir);

    // TODO: cache check via input_hash once provenance is wired through

    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create {}: {}", stage_dir, e))?;

    // Build base config (no prior state, random starts)
    let config = FitRunConfig::build(
        fit, None,
        DEFAULT_CHAINS, DEFAULT_PARTICLES, DEFAULT_ITERATIONS,
        DEFAULT_COOLING, seed, false, // random_starts handled per-chain below
    )?;

    // Generate per-chain random starts
    let mut rng = sim::ekrng::StatefulRng::new(seed ^ 0xcafe_u64);
    let per_chain_params: Vec<Vec<IF2Param>> = (0..DEFAULT_CHAINS).map(|_| {
        config.if2_params.iter().map(|spec| {
            let initial = if spec.lower.is_finite() && spec.upper.is_finite() {
                let u = rng.uniform();
                spec.lower + u * (spec.upper - spec.lower)
            } else {
                // Unbounded: jitter ±50% from default
                let v = config.base_params[spec.index];
                v * (0.5 + rng.uniform())
            };
            IF2Param {
                initial,
                ..spec.clone()
            }
        }).collect()
    }).collect();

    eprintln!("scout: {} chains with random starts", DEFAULT_CHAINS);
    let t0 = std::time::Instant::now();
    let chain_results = runner::run_chains_with_per_chain_params(&config, Some(&per_chain_params));
    let elapsed = t0.elapsed();

    // MAD-based auto rw_sd
    let (auto_rw_sd, n_good) = runner::auto_rw_sd(&chain_results.results, &config.if2_params)?;

    eprintln!("\nauto rw_sd ({}/{} good chains):", n_good, DEFAULT_CHAINS);
    for spec in &config.if2_params {
        let rw = auto_rw_sd.get(&spec.name).unwrap();
        eprintln!("  {:12} rw_sd={:.4} (was {:.4})", spec.name, rw, spec.rw_sd);
    }

    // Collect best chain's MLE parameters as start values
    let best = &chain_results.results.iter()
        .find(|(id, _)| *id == chain_results.best_chain)
        .unwrap().1;
    // Store ALL param values (estimated from MLE + fixed from base) so
    // fit_state is self-contained and robust to model edits between stages.
    let start_values: HashMap<String, f64> = runner::collect_all_params(
        &best.mle, &config.if2_params, &config.model,
        &config.base_params, &config.compiled,
    );

    // Compute initial loglik (at starting params before fitting)
    // Approximate: use worst chain's first-iteration loglik
    let initial_loglik = chain_results.results.iter()
        .map(|(_, r)| r.iterations.first().map_or(f64::NEG_INFINITY, |it| it.log_likelihood))
        .fold(f64::INFINITY, f64::min);

    // Write fit_state.toml
    let state = FitState {
        stage: "scout".into(),
        seed,
        timestamp: now_iso8601(),
        best_loglik: chain_results.best_loglik,
        initial_loglik,
        best_chain: chain_results.best_chain,
        n_chains: DEFAULT_CHAINS,
        n_good_chains: Some(n_good),
        start_values,
        rw_sd: auto_rw_sd.clone(),
    };
    state.save(&stage_dir)?;

    // Write per-chain outputs
    let param_names: Vec<String> = config.model.parameters.iter().map(|p| p.name.clone()).collect();
    runner::write_chain_outputs(
        &stage_dir, &chain_results.results, &config.if2_params,
        &param_names, &config.base_params, &config.compiled,
    )?;
    runner::write_diagnostics(&stage_dir, &chain_results.results)?;

    // Write scout_summary.json
    write_summary(&stage_dir, &chain_results, &config, &auto_rw_sd, n_good, initial_loglik)?;

    let wall_secs = elapsed.as_secs_f64();
    let per_iter = wall_secs / (DEFAULT_CHAINS as f64 * DEFAULT_ITERATIONS as f64);
    eprintln!("\nscout complete in {:.1}s ({:.2}s/chain-iteration): {}/",
        wall_secs, per_iter, stage_dir);
    eprintln!("  best loglik: {:.1} (chain {})", chain_results.best_loglik, chain_results.best_chain + 1);
    eprintln!("  good chains: {}/{}", n_good, DEFAULT_CHAINS);
    eprintln!("\nnext: camdl fit refine fit.toml --starts-from {}/", stage_dir);

    Ok(())
}

fn write_summary(
    dir: &str,
    results: &runner::ChainResults,
    config: &FitRunConfig,
    auto_rw_sd: &HashMap<String, f64>,
    n_good: usize,
    initial_loglik: f64,
) -> Result<(), String> {
    let summary = serde_json::json!({
        "stage": "scout",
        "status": if n_good >= config.n_chains / 2 { "ok" } else { "warning" },
        "n_chains": config.n_chains,
        "n_good_chains": n_good,
        "best_loglik": results.best_loglik,
        "best_chain": results.best_chain + 1,
        "initial_loglik": initial_loglik,
        "parameters": config.if2_params.iter().map(|spec| {
            let rhat = results.rhat.get(&spec.name).copied().unwrap_or(f64::NAN);
            let rw = auto_rw_sd.get(&spec.name).copied().unwrap_or(0.0);
            serde_json::json!({
                "name": spec.name,
                "rhat": rhat,
                "recommended_rw_sd": rw,
                "original_rw_sd": spec.rw_sd,
            })
        }).collect::<Vec<_>>(),
        "next_step": if n_good >= config.n_chains / 2 { "refine" } else { "fix_model" },
    });

    let path = format!("{}/scout_summary.json", dir);
    let contents = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("json error: {}", e))?;
    std::fs::write(&path, contents)
        .map_err(|e| format!("cannot write {}: {}", path, e))
}

pub fn now_iso8601_pub() -> String { now_iso8601() }

fn now_iso8601() -> String {
    use std::time::SystemTime;
    let dur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let secs = dur.as_secs();
    // Simple ISO 8601 without a datetime library
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate date from days since epoch (good enough for timestamps)
    let (year, month, day) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, hours, minutes, seconds)
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's civil_from_days
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m, d)
}
