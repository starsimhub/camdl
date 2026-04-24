//! `camdl fit scout` — landscape discovery with random starts and MAD-based auto rw_sd.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::provenance;
use crate::fit::runner::{self, FitRunConfig};
use sim::inference::if2::EstimatedParam;
use sim::inference::diagnostic::{DiagnosticCollector, DiagnosticKind};
use std::collections::HashMap;

const SCOUT_CHAINS: usize = 8;
const SCOUT_PARTICLES: usize = 500;
const SCOUT_ITERATIONS: usize = 30;
const SCOUT_COOLING: f64 = 0.70; // cf50: 70% at halfway, 49% at end — find basins
const SCOUT_RW_SD_SCALE: f64 = 1.0; // /20 log default is already calibrated for scout

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
        for p in &mut config.estimated_params {
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

    // Determine which chains are seeded near start values vs fully random.
    // A chain is "seeded" if at least one parameter has a start value in [estimate].
    let has_any_starts = fit.estimate.values().any(|est| est.start.is_some());
    let start_chains = if has_any_starts {
        sc.and_then(|s| s.start_chains).unwrap_or(1)
    } else {
        0
    };
    let n_random = n_chains - start_chains.min(n_chains);

    // Generate per-chain starts
    let mut rng = sim::rng::StatefulRng::new(seed ^ 0xcafe_u64);
    let per_chain_params: Vec<Vec<EstimatedParam>> = (0..n_chains).map(|chain_id| {
        config.estimated_params.iter().map(|spec| {
            let initial = if chain_id < start_chains {
                // Seeded chain: use start value with jitter, or random if no start
                if let Some(start) = fit.estimate.get(&spec.name).and_then(|e| e.start) {
                    let jitter = rng.normal() * spec.rw_sd;
                    (start + jitter).clamp(spec.lower, spec.upper)
                } else {
                    random_from_bounds(spec, &mut rng)
                }
            } else {
                // Fully random chain
                random_from_bounds(spec, &mut rng)
            };
            EstimatedParam {
                initial,
                ..spec.clone()
            }
        }).collect()
    }).collect();

    let collector = DiagnosticCollector::new("scout");

    eprintln!("scout: {} chains ({} seeded, {} random) × {} particles × {} iterations, cooling={}, rw_sd×{:.1}",
        n_chains, start_chains.min(n_chains), n_random, n_particles, n_iterations, cooling, rw_sd_scale);
    let t0 = std::time::Instant::now();
    let chain_results = runner::run_chains_with_per_chain_params(&config, Some(&per_chain_params), &collector);
    let elapsed = t0.elapsed();

    // Record each chain's pre-filter start — diagnostics use this to
    // check "did chains span the bounds?" and "did all chains collapse
    // to the same basin in one filter pass?"
    std::fs::create_dir_all(&stage_dir).ok();
    runner::write_chain_starts(
        &stage_dir, Some(&per_chain_params),
        &config.estimated_params, n_chains,
    ).unwrap_or_else(|e| eprintln!("warning: {}", e));

    // Check for degenerate filter: if best chain's loglik at early iterations is -inf,
    // the particle count is too low for this model's dimensionality.
    let early_check_iter = 5.min(n_iterations.saturating_sub(1));
    let best_early_ll = chain_results.results.iter()
        .filter_map(|(_, r)| r.iterations.get(early_check_iter).map(|it| it.if2_perturbed_loglik))
        .fold(f64::NEG_INFINITY, f64::max);
    if !best_early_ll.is_finite() {
        collector.push(DiagnosticKind::InitialLoglikInfinite);
        eprintln!("\n\x1b[31mscout: filter degenerate — all chains have -inf loglik at iteration {}.\x1b[0m", early_check_iter);
        eprintln!("  The particle count ({}) is likely too low for {} estimated parameters.", n_particles, config.estimated_params.len());
        eprintln!("  Add to fit.toml:");
        eprintln!("    [scout]");
        eprintln!("    particles = {}", n_particles * 4);
        eprintln!();
    }

    // Collect best chain's MLE parameters as start values
    let best = &chain_results.results.iter()
        .find(|(id, _)| *id == chain_results.best_chain)
        .unwrap().1;
    // Store ALL param values (estimated from MLE + fixed from base) so
    // fit_state is self-contained and robust to model edits between stages.
    let start_values: HashMap<String, f64> = runner::collect_all_params(
        &best.mle, &config.estimated_params, &config.model,
        &config.base_params, &config.compiled,
    );

    // Compute initial loglik: quick pfilter at starting params
    eprintln!("\npfilter at starting params ({} particles)...", n_particles);
    let initial_loglik = runner::run_quick_pfilter(&config, &config.base_params, n_particles, seed);
    eprintln!("  initial loglik: {:.1}", initial_loglik);

    // Per-chain final loglik + IVP param names, for Gate 2 and the
    // IVP exemption in Gate 1 of refine's convergence check. See
    // docs/dev/proposals/2026-04-19-refine-gates-scout-convergence.md.
    let chain_logliks: Vec<f64> = chain_results.results.iter()
        .map(|(_, r)| r.final_loglik).collect();
    let ivp_params: Vec<String> = config.estimated_params.iter()
        .filter(|p| p.ivp).map(|p| p.name.clone()).collect();

    // Write fit_state.toml
    let state = FitState {
        stage: "scout".into(),
        seed,
        timestamp: now_iso8601(),
        input_hash: Some(input_hash.clone()),
        camdl_version: Some(crate::version::VERSION_SHORT.into()),
        best_loglik: chain_results.best_loglik,
        initial_loglik,
        best_chain: chain_results.best_chain,
        n_chains,
        n_good_chains: None,
        start_values,
        rw_sd: HashMap::new(),
        loglik_type: Some("if2".into()),
        acceptance_rate: None,
        tail_chain_agreement: chain_results.chain_agreement.clone(),
        ivp_params,
        chain_logliks,
    };
    state.save(&stage_dir)?;

    // Write per-chain outputs
    let param_names: Vec<String> = config.model.parameters.iter().map(|p| p.name.clone()).collect();
    runner::write_chain_outputs(
        &stage_dir, &chain_results.results, &config.estimated_params,
        &param_names, &config.base_params, &config.compiled,
    )?;
    runner::write_diagnostics(&stage_dir, &chain_results.results)?;

    // Write scout_best_params.toml — best chain's params for downstream use
    // Named "scout_best" (not "mle") to signal these are scout-level estimates.
    let all_params = runner::collect_all_params(
        &best.mle, &config.estimated_params, &config.model,
        &config.base_params, &config.compiled,
    );
    let best_params_path = format!("{}/scout_best_params.toml", stage_dir);
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&best_params_path)
            .map_err(|e| format!("cannot write {}: {}", best_params_path, e))?;
        writeln!(f, "# Scout best-chain parameters (chain {}, loglik = {:.1})", chain_results.best_chain + 1, chain_results.best_loglik).unwrap();
        writeln!(f, "# These are exploration-level estimates. Use camdl fit refine for convergence.").unwrap();
        writeln!(f).unwrap();
        let mut pairs: Vec<(&String, &f64)> = all_params.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        for (name, value) in pairs {
            writeln!(f, "{} = {}", name, runner::format_param_value(*value)).unwrap();
        }
    }

    // Write scout_summary.json
    write_summary(&stage_dir, &chain_results, &config, initial_loglik, &input_hash)?;

    // Render and persist diagnostics
    collector.render_to_stderr();
    let _ = collector.write_json(&format!("{}/diagnostics.json", stage_dir));

    let wall_secs = elapsed.as_secs_f64();
    let per_iter = wall_secs / (n_chains as f64 * n_iterations as f64);
    eprintln!("\nscout complete in {:.1}s ({:.2}s/chain-iteration): {}/",
        wall_secs, per_iter, stage_dir);
    eprintln!("  best loglik: {:.1} (chain {})", chain_results.best_loglik, chain_results.best_chain + 1);
    eprintln!("\nnext: camdl fit refine fit.toml --starts-from {}/", stage_dir);

    Ok(())
}

fn write_summary(
    dir: &str,
    results: &runner::ChainResults,
    config: &FitRunConfig,
    initial_loglik: f64,
    input_hash: &str,
) -> Result<(), String> {
    let summary = serde_json::json!({
        "stage": "scout",
        "n_chains": config.n_chains,
        "best_loglik": results.best_loglik,
        "best_chain": results.best_chain + 1,
        "initial_loglik": initial_loglik,
        "parameters": config.estimated_params.iter().map(|spec| {
            let agreement = results.chain_agreement.get(&spec.name).copied().unwrap_or(f64::NAN);
            serde_json::json!({
                "name": spec.name,
                "chain_agreement": agreement,
                "rw_sd": spec.rw_sd,
            })
        }).collect::<Vec<_>>(),
        "input_hash": input_hash,
    });

    let path = format!("{}/scout_summary.json", dir);
    let contents = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("json error: {}", e))?;
    std::fs::write(&path, contents)
        .map_err(|e| format!("cannot write {}: {}", path, e))
}

fn random_from_bounds(spec: &EstimatedParam, rng: &mut sim::rng::StatefulRng) -> f64 {
    if spec.lower.is_finite() && spec.upper.is_finite() {
        spec.lower + rng.uniform() * (spec.upper - spec.lower)
    } else {
        // Unbounded: jitter ±50% from default
        spec.initial * (0.5 + rng.uniform())
    }
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
