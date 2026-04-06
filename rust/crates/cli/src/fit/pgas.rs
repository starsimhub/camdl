//! `camdl fit pgas` — PGAS posterior sampling.
//!
//! Runs PGAS Gibbs sampler chains, each alternating exact parameter
//! updates (θ | X) with conditional SMC trajectory updates (X | θ, y).
//! Outputs per-chain trace files, convergence diagnostics, and summary.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::runner::FitRunConfig;
use crate::fit::scout::now_iso8601_pub;
use sim::inference::{
    if2::IF2Param,
    pmmh::{Prior, mcmc_ess},
    pgas::{PGASConfig, run_pgas, PGASSweep, PGASTrajectory},
};
use std::collections::HashMap;

const DEFAULT_CHAINS: usize = 4;
const DEFAULT_SWEEPS: usize = 10_000;
const DEFAULT_PARTICLES: usize = 100;
const DEFAULT_BURN_IN: usize = 2000;
const DEFAULT_THIN: usize = 5;

pub fn run_pgas_cli(
    fit: &FitToml,
    starts_from: Option<&str>,
    seed: u64,
    force: bool,
) -> Result<(), String> {
    let stage_dir = format!("{}/pgas", fit.fit.output_dir);
    let sc = fit.pgas.as_ref();

    let n_chains = sc.and_then(|s| s.chains).unwrap_or(DEFAULT_CHAINS);
    let n_sweeps = sc.and_then(|s| s.sweeps).unwrap_or(DEFAULT_SWEEPS);
    let n_particles = sc.and_then(|s| s.particles).unwrap_or(DEFAULT_PARTICLES);
    let burn_in = sc.and_then(|s| s.burn_in).unwrap_or(DEFAULT_BURN_IN);
    let thin = sc.and_then(|s| s.thin).unwrap_or(DEFAULT_THIN);

    if !force {
        let state_path = format!("{}/fit_state.toml", stage_dir);
        if std::path::Path::new(&state_path).exists() {
            eprintln!("\x1b[33mpgas results already exist in {}. Use --force to re-run.\x1b[0m", stage_dir);
            return Ok(());
        }
    }

    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create {}: {}", stage_dir, e))?;

    // Load prior state if --starts-from provided
    let starts_from = starts_from
        .map(String::from)
        .or_else(|| sc.and_then(|s| s.starts_from.clone()));
    let prior_state = starts_from.as_deref().map(FitState::load).transpose()?;

    // Build FitRunConfig (reuse existing builder)
    let config = FitRunConfig::build(
        fit, prior_state.as_ref(),
        n_chains, n_particles, 1,
        1.0, seed, false,
    )?;

    let dt = config.if2_config.dt;

    // Build priors (flat for now)
    let priors: Vec<Prior> = config.if2_params.iter()
        .map(|_| Prior::Flat)
        .collect();

    eprintln!("\npgas: {} chains × {} sweeps × {} particles, burn_in={}, thin={}",
        n_chains, n_sweeps, n_particles, burn_in, thin);
    eprintln!("  estimated output: {} posterior samples per chain",
        (n_sweeps.saturating_sub(burn_in)) / thin);

    // Pre-create chain directories (must happen before parallel spawn)
    for chain_id in 0..n_chains {
        let chain_dir = format!("{}/chain_{}", stage_dir, chain_id + 1);
        std::fs::create_dir_all(&chain_dir)
            .map_err(|e| format!("cannot create {}: {}", chain_dir, e))?;
    }

    let t0 = std::time::Instant::now();
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());

    // Run chains in parallel (each chain is independent: own seed, own
    // trajectory, own RNG). Same pattern as PMMH.
    use rayon::prelude::*;
    let all_results: Vec<Result<(usize, Vec<PGASSweep>, Vec<f64>), String>> = (0..n_chains)
        .into_par_iter()
        .map(|chain_id| {
            let chain_seed = seed ^ (chain_id as u64).wrapping_mul(0x9e3779b97f4a7c15);
            let chain_dir = format!("{}/chain_{}", stage_dir, chain_id + 1);

            let pgas_config = PGASConfig {
                n_particles,
                n_sweeps,
                burn_in,
                thin,
                dt,
            };

            // Each chain gets its own dmeasure closure
            let compiled = &*config.compiled;
            let dmeasure_fn = sim::inference::dmeasure::compile_dmeasure_pf(
                &config.obs_model_ir, config.compiled.clone(), &config.base_params,
            );

            let observations: Vec<sim::inference::particle_filter::Observation> =
                config.observations.iter()
                    .map(|o| sim::inference::particle_filter::Observation {
                        time: o.time, value: o.value,
                    })
                    .collect();

            // Streaming trace file
            let trace_path = format!("{}/trace.tsv", chain_dir);
            let trace_file = std::sync::Mutex::new({
                use std::io::Write;
                let mut f = std::io::BufWriter::new(
                    std::fs::File::create(&trace_path).unwrap()
                );
                write!(f, "sweep\tlog_complete_data_ll").unwrap();
                for spec in &config.if2_params { write!(f, "\t{}", spec.name).unwrap(); }
                writeln!(f).unwrap();
                f
            });

            let chain_start = std::time::Instant::now();

            let progress_cb = |sweep: usize, result: &PGASSweep, _traj: &PGASTrajectory| {
                // Stream trace row
                {
                    use std::io::Write;
                    if let Ok(mut f) = trace_file.lock() {
                        write!(f, "{}\t{:.4}", sweep, result.log_complete_data_ll).unwrap();
                        for spec in &config.if2_params {
                            write!(f, "\t{:.6}", result.params[spec.index]).unwrap();
                        }
                        writeln!(f).unwrap();
                        if sweep % 50 == 0 { f.flush().ok(); }
                    }
                }

                // Progress (non-TTY only for parallel — TTY would interleave)
                if sweep % 500 == 0 || sweep == n_sweeps - 1 {
                    let elapsed = chain_start.elapsed().as_secs();
                    let n_acc: usize = result.accepted.iter().filter(|&&a| a).count();
                    eprintln!("[pgas] chain {}: {}/{} ({:.0}%) ll={:.1} acc={}/{} elapsed={}s",
                        chain_id + 1, sweep + 1, n_sweeps,
                        (sweep + 1) as f64 / n_sweeps as f64 * 100.0,
                        result.log_complete_data_ll,
                        n_acc, result.accepted.len(), elapsed);
                }
            };

            let result = run_pgas(
                compiled,
                &config.if2_params,
                &priors,
                &config.base_params,
                &pgas_config,
                &observations,
                &*dmeasure_fn,
                &config.flow_indices,
                chain_seed,
                Some(&progress_cb),
            ).map_err(|e| format!("pgas chain {} error: {}", chain_id + 1, e))?;

            let chain_elapsed = chain_start.elapsed();
            eprintln!("  chain {} done: {:.1}s, acceptance: [{}]",
                chain_id + 1,
                chain_elapsed.as_secs_f64(),
                config.if2_params.iter().zip(&result.acceptance_rates)
                    .map(|(p, &r)| format!("{}={:.0}%", p.name, r * 100.0))
                    .collect::<Vec<_>>().join(", "));

            Ok((chain_id, result.sweeps, result.acceptance_rates))
        })
        .collect();

    // Unwrap results (propagate first error)
    let all_results: Vec<(usize, Vec<PGASSweep>, Vec<f64>)> = all_results
        .into_iter()
        .collect::<Result<Vec<_>, String>>()?;

    let elapsed = t0.elapsed();

    // Compute diagnostics
    let diagnostics = compute_diagnostics(&all_results, &config.if2_params);

    // Report
    eprintln!("\nacceptance rates:");
    for &(chain_id, _, ref rates) in &all_results {
        let summary: Vec<String> = config.if2_params.iter().zip(rates)
            .map(|(p, &r)| {
                let status = if r < 0.10 { "\x1b[31m" }
                    else if r > 0.50 { "\x1b[33m" }
                    else { "\x1b[32m" };
                format!("  {}={}{:.0}%\x1b[0m", p.name, status, r * 100.0)
            })
            .collect();
        eprintln!("  chain {}: {}", chain_id + 1, summary.join(" "));
    }

    if n_chains > 1 {
        eprintln!("\nRhat / ESS:");
        for spec in &config.if2_params {
            if let Some(&rhat) = diagnostics.rhat.get(&spec.name) {
                let status = if rhat < 1.1 { "\x1b[32m✓\x1b[0m" }
                    else if rhat < 1.5 { "\x1b[33m~\x1b[0m" }
                    else { "\x1b[31m✗\x1b[0m" };
                let ess = diagnostics.ess.get(&spec.name).copied().unwrap_or(0.0);
                eprintln!("  {:12} Rhat={:.3} {} ESS={:.0}", spec.name, rhat, status, ess);
            }
        }
    }

    // Write summary JSON
    write_summary(&stage_dir, &all_results, &config, &diagnostics)?;

    // Write fit_state.toml with best params
    let best_chain = all_results.iter()
        .max_by(|a, b| {
            let best_ll_a = a.1.iter().map(|s| s.log_complete_data_ll)
                .fold(f64::NEG_INFINITY, f64::max);
            let best_ll_b = b.1.iter().map(|s| s.log_complete_data_ll)
                .fold(f64::NEG_INFINITY, f64::max);
            best_ll_a.total_cmp(&best_ll_b)
        })
        .unwrap();

    let best_sweep = best_chain.1.iter()
        .max_by(|a, b| a.log_complete_data_ll.total_cmp(&b.log_complete_data_ll))
        .unwrap();

    let mut start_values = HashMap::new();
    for spec in &config.if2_params {
        start_values.insert(spec.name.clone(), best_sweep.params[spec.index]);
    }
    for p in &config.model.parameters {
        if !start_values.contains_key(&p.name) {
            if let Some(&idx) = config.compiled.param_index.get(p.name.as_str()) {
                start_values.insert(p.name.clone(), config.base_params[idx]);
            }
        }
    }

    let state = FitState {
        stage: "pgas".into(),
        seed,
        timestamp: now_iso8601_pub(),
        input_hash: None,
        camdl_version: Some(crate::version::VERSION_SHORT.into()),
        best_loglik: best_sweep.log_complete_data_ll,
        initial_loglik: f64::NEG_INFINITY,
        best_chain: best_chain.0,
        n_chains,
        n_good_chains: None,
        start_values,
        rw_sd: HashMap::new(),
    };
    state.save(&stage_dir)?;

    let wall_secs = elapsed.as_secs_f64();
    eprintln!("\npgas complete in {:.1}s: {}/", wall_secs, stage_dir);
    eprintln!("  best complete-data ll: {:.1} (chain {})",
        best_sweep.log_complete_data_ll, best_chain.0 + 1);

    Ok(())
}

// ── Diagnostics ──────────────────────────────────────────────────

struct Diagnostics {
    rhat: HashMap<String, f64>,
    ess: HashMap<String, f64>,
}

fn compute_diagnostics(
    results: &[(usize, Vec<PGASSweep>, Vec<f64>)],
    if2_params: &[IF2Param],
) -> Diagnostics {
    let n_chains = results.len();
    let mut rhat_map = HashMap::new();
    let mut ess_map = HashMap::new();

    for spec in if2_params {
        let chains: Vec<Vec<f64>> = results.iter()
            .map(|(_, sweeps, _)| sweeps.iter().map(|s| s.params[spec.index]).collect())
            .collect();

        let total_ess: f64 = chains.iter().map(|c| mcmc_ess(c)).sum();
        ess_map.insert(spec.name.clone(), total_ess);

        if n_chains >= 2 && chains.iter().all(|c| c.len() >= 4) {
            let chain_means: Vec<f64> = chains.iter().map(|c| {
                c.iter().sum::<f64>() / c.len() as f64
            }).collect();
            let chain_vars: Vec<f64> = chains.iter().map(|c| {
                let m = c.iter().sum::<f64>() / c.len() as f64;
                c.iter().map(|&x| (x - m).powi(2)).sum::<f64>() / (c.len() - 1).max(1) as f64
            }).collect();

            let n_samples = chains[0].len() as f64;
            let grand_mean = chain_means.iter().sum::<f64>() / n_chains as f64;
            let between = chain_means.iter().map(|&m| (m - grand_mean).powi(2)).sum::<f64>()
                * n_samples / (n_chains - 1).max(1) as f64;
            let within = chain_vars.iter().sum::<f64>() / n_chains as f64;
            let rhat = if within > 0.0 {
                (((n_samples - 1.0) / n_samples * within + between / n_samples) / within).sqrt()
            } else { f64::NAN };

            rhat_map.insert(spec.name.clone(), rhat);
        }
    }

    Diagnostics { rhat: rhat_map, ess: ess_map }
}

fn write_summary(
    dir: &str,
    results: &[(usize, Vec<PGASSweep>, Vec<f64>)],
    _config: &FitRunConfig,
    diagnostics: &Diagnostics,
) -> Result<(), String> {
    let acceptance_rates: Vec<Vec<f64>> = results.iter()
        .map(|(_, _, rates)| rates.clone())
        .collect();

    let summary = serde_json::json!({
        "stage": "pgas",
        "n_chains": results.len(),
        "acceptance_rates": acceptance_rates,
        "rhat": diagnostics.rhat,
        "ess": diagnostics.ess,
    });

    let path = format!("{}/pgas_summary.json", dir);
    let contents = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("json error: {}", e))?;
    std::fs::write(&path, contents)
        .map_err(|e| format!("cannot write {}: {}", path, e))
}
