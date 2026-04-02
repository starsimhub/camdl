//! `camdl fit scout` — landscape discovery with random starts and MAD-based auto rw_sd.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::provenance;
use crate::fit::runner::{self, FitRunConfig};
use sim::inference::if2::IF2Param;
use std::collections::HashMap;

const SCOUT_CHAINS: usize = 8;
const SCOUT_PARTICLES: usize = 500;
const SCOUT_ITERATIONS: usize = 30;
const SCOUT_COOLING: f64 = 0.5;  // mild contraction — find basins, don't converge
const SCOUT_RW_SD_SCALE: f64 = 1.5; // slightly aggressive exploration

pub fn run_scout(fit: &FitToml, seed: u64, force: bool) -> Result<(), String> {
    let stage_dir = format!("{}/scout", fit.fit.output_dir);
    let sc = fit.scout.as_ref();

    let n_chains = sc.and_then(|s| s.chains).unwrap_or(SCOUT_CHAINS);
    let n_particles = sc.and_then(|s| s.particles).unwrap_or(SCOUT_PARTICLES);
    let n_iterations = sc.and_then(|s| s.iterations).unwrap_or(SCOUT_ITERATIONS);
    let cooling = sc.and_then(|s| s.cooling).unwrap_or(SCOUT_COOLING);
    let rw_sd_scale = sc.and_then(|s| s.rw_sd_scale).unwrap_or(SCOUT_RW_SD_SCALE);

    let mut config = FitRunConfig::build(
        fit, None,
        n_chains, n_particles, n_iterations,
        cooling, seed, false,
    )?;

    // Apply rw_sd_scale from [scout] config
    if rw_sd_scale != 1.0 {
        for p in &mut config.if2_params {
            p.rw_sd *= rw_sd_scale;
        }
        eprintln!("scout: rw_sd scaled by {:.1}×", rw_sd_scale);
    }

    // Cache check
    let input_hash = runner::compute_fit_input_hash(fit, &config, seed);
    if !force {
        match provenance::check_cache(&stage_dir, &input_hash) {
            provenance::CacheStatus::Match => {
                eprintln!("\x1b[33mscout skipped — results already exist for these inputs.\x1b[0m");
                eprintln!("  output:     {}/", stage_dir);
                eprintln!("  input hash: {}", input_hash);
                eprintln!("  Use --force to re-run.");
                return Ok(());
            }
            provenance::CacheStatus::Mismatch => {
                eprintln!("\x1b[33mscout — prior results exist but inputs have changed. Re-running.\x1b[0m");
            }
            provenance::CacheStatus::NotFound => {}
        }
    }

    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create {}: {}", stage_dir, e))?;

    // Generate per-chain random starts
    let mut rng = sim::ekrng::StatefulRng::new(seed ^ 0xcafe_u64);
    let per_chain_params: Vec<Vec<IF2Param>> = (0..n_chains).map(|_| {
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

    eprintln!("scout: {} chains × {} particles × {} iterations, cooling={}, rw_sd×{:.1}",
        n_chains, n_particles, n_iterations, cooling, rw_sd_scale);
    let t0 = std::time::Instant::now();
    let chain_results = runner::run_chains_with_per_chain_params(&config, Some(&per_chain_params));
    let elapsed = t0.elapsed();

    // Check for degenerate filter: if best chain's loglik at early iterations is -inf,
    // the particle count is too low for this model's dimensionality.
    let early_check_iter = 5.min(n_iterations.saturating_sub(1));
    let best_early_ll = chain_results.results.iter()
        .filter_map(|(_, r)| r.iterations.get(early_check_iter).map(|it| it.log_likelihood))
        .fold(f64::NEG_INFINITY, f64::max);
    if !best_early_ll.is_finite() {
        eprintln!("\n\x1b[31mscout: filter degenerate — all chains have -inf loglik at iteration {}.\x1b[0m", early_check_iter);
        eprintln!("  The particle count ({}) is likely too low for {} estimated parameters.", n_particles, config.if2_params.len());
        eprintln!("  Add to fit.toml:");
        eprintln!("    [scout]");
        eprintln!("    particles = {}", n_particles * 4);
        eprintln!();
    }

    // MAD-based auto rw_sd — never fail, always write output
    let (auto_rw_sd, n_good) = match runner::auto_rw_sd(&chain_results.results, &config.if2_params) {
        Ok(result) => result,
        Err(msg) => {
            eprintln!("\n\x1b[33mwarning: auto rw_sd failed: {}\x1b[0m", msg);
            eprintln!("  Falling back to bounds-based rw_sd. Scout output will still be written.");
            eprintln!("  Inspect chain traces to diagnose convergence issues.\n");
            // Fall back to bounds-based rw_sd from the current values
            let fallback: HashMap<String, f64> = config.if2_params.iter()
                .map(|spec| (spec.name.clone(), spec.rw_sd))
                .collect();
            (fallback, 0)
        }
    };

    eprintln!("\nauto rw_sd ({}/{} good chains):", n_good, n_chains);
    for spec in &config.if2_params {
        let rw = auto_rw_sd.get(&spec.name).unwrap_or(&spec.rw_sd);
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

    // Compute initial loglik: quick pfilter at starting params
    eprintln!("\npfilter at starting params ({} particles)...", n_particles);
    let initial_loglik = runner::run_quick_pfilter(&config, &config.base_params, n_particles, seed);
    eprintln!("  initial loglik: {:.1}", initial_loglik);

    // Write fit_state.toml
    let state = FitState {
        stage: "scout".into(),
        seed,
        timestamp: now_iso8601(),
        input_hash: Some(input_hash.clone()),
        best_loglik: chain_results.best_loglik,
        initial_loglik,
        best_chain: chain_results.best_chain,
        n_chains: n_chains,
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
    write_summary(&stage_dir, &chain_results, &config, &auto_rw_sd, n_good, initial_loglik, &input_hash)?;

    let wall_secs = elapsed.as_secs_f64();
    let per_iter = wall_secs / (n_chains as f64 * n_iterations as f64);
    eprintln!("\nscout complete in {:.1}s ({:.2}s/chain-iteration): {}/",
        wall_secs, per_iter, stage_dir);
    eprintln!("  best loglik: {:.1} (chain {})", chain_results.best_loglik, chain_results.best_chain + 1);
    eprintln!("  good chains: {}/{}", n_good, n_chains);
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
    input_hash: &str,
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
        "input_hash": input_hash,
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
