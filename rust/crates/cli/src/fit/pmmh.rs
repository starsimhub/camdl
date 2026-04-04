//! `camdl fit pmmh` — PMMH posterior sampling.
//!
//! Runs multiple MCMC chains in parallel, each using the bootstrap particle
//! filter as an unbiased likelihood estimator. Outputs per-chain trace files,
//! convergence diagnostics (R̂, ESS), and a summary JSON.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::runner::{self, FitRunConfig};
use crate::fit::scout::now_iso8601_pub;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sim::inference::{
    if2::IF2Param,
    pmmh::{run_pmmh, Prior, PMMHConfig, PMMHResult, mcmc_ess},
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

const DEFAULT_CHAINS: usize = 4;
const DEFAULT_STEPS: usize = 50_000;
const DEFAULT_PARTICLES: usize = 2000;
const DEFAULT_BURN_IN: usize = 5000;
const DEFAULT_THIN: usize = 10;
const DEFAULT_ADAPT_START: usize = 500;

pub fn run_pmmh_cli(
    fit: &FitToml,
    starts_from: Option<&str>,
    seed: u64,
    force: bool,
    check_variance: bool,
) -> Result<(), String> {
    let stage_dir = format!("{}/pmmh", fit.fit.output_dir);
    let sc = fit.pmmh.as_ref();

    let n_chains = sc.and_then(|s| s.chains).unwrap_or(DEFAULT_CHAINS);
    let n_steps = sc.and_then(|s| s.steps).unwrap_or(DEFAULT_STEPS);
    let n_particles = sc.and_then(|s| s.particles).unwrap_or(DEFAULT_PARTICLES);
    let burn_in = sc.and_then(|s| s.burn_in).unwrap_or(DEFAULT_BURN_IN);
    let thin = sc.and_then(|s| s.thin).unwrap_or(DEFAULT_THIN);
    let adapt = sc.and_then(|s| s.adapt).unwrap_or(true);
    let adapt_start = sc.and_then(|s| s.adapt_start).unwrap_or(DEFAULT_ADAPT_START);

    // Load prior state if --starts-from provided
    let prior_state = starts_from.map(FitState::load).transpose()?;

    // Build FitRunConfig (reuse existing builder)
    let config = FitRunConfig::build(
        fit, prior_state.as_ref(),
        n_chains, n_particles, 1, // iterations=1 unused for PMMH
        1.0, // cooling unused
        seed, false,
    )?;

    // Build proposal SDs
    let proposal_sd = build_proposal_sd(&config, sc, starts_from)?;

    // Preflight: PF variance check
    eprintln!("\npfilter variance check ({} particles, 20 replicates)...", n_particles);
    let base = prior_state.as_ref().map(|s| {
        let mut p = config.base_params.clone();
        for spec in &config.if2_params {
            if let Some(&v) = s.start_values.get(&spec.name) {
                p[spec.index] = v;
            }
        }
        p
    }).unwrap_or_else(|| config.base_params.clone());

    let logliks: Vec<f64> = (0..20)
        .map(|i| runner::run_quick_pfilter(&config, &base, n_particles, seed + i))
        .collect();
    let ll_mean = logliks.iter().sum::<f64>() / logliks.len() as f64;
    let ll_var = logliks.iter().map(|&l| (l - ll_mean).powi(2)).sum::<f64>() / (logliks.len() - 1) as f64;
    let ll_sd = ll_var.sqrt();

    eprintln!("  log L̂ mean = {:.1}, sd = {:.2}", ll_mean, ll_sd);
    if ll_sd > 5.0 {
        eprintln!("  \x1b[33m⚠ PF variance high (sd={:.1} > 5). Consider doubling particles to {}.\x1b[0m",
            ll_sd, n_particles * 2);
    } else if ll_sd < 0.5 && n_particles > 200 {
        eprintln!("  \x1b[32m✓ PF variance low (sd={:.2}). Could halve particles to {} for 2× speed.\x1b[0m",
            ll_sd, n_particles / 2);
    } else {
        eprintln!("  \x1b[32m✓ PF variance OK (target: 1-3)\x1b[0m");
    }

    if check_variance {
        eprintln!("\n--check-variance: stopping here (no MCMC run).");
        return Ok(());
    }

    if !force {
        let state_path = format!("{}/fit_state.toml", stage_dir);
        if std::path::Path::new(&state_path).exists() {
            eprintln!("\x1b[33mpmmh results already exist in {}. Use --force to re-run.\x1b[0m", stage_dir);
            return Ok(());
        }
    }

    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create {}: {}", stage_dir, e))?;

    // Build priors (flat for now — future: [prior] section in fit.toml)
    let priors: Vec<Prior> = config.if2_params.iter()
        .map(|_| Prior::Flat)
        .collect();

    let dt = config.if2_config.dt;

    eprintln!("\npmmh: {} chains × {} steps × {} particles, burn_in={}, thin={}, adapt={}",
        n_chains, n_steps, n_particles, burn_in, thin, adapt);
    eprintln!("  proposal_sd (transformed): [{}]",
        config.if2_params.iter().zip(&proposal_sd)
            .map(|(p, &sd)| format!("{}={:.4}", p.name, sd))
            .collect::<Vec<_>>().join(", "));

    let mp = MultiProgress::new();
    let bar_style = ProgressStyle::default_bar()
        .template("  chain {prefix} [{bar:25.cyan/dim}] {pos}/{len} {msg}")
        .unwrap()
        .progress_chars("━╸─");

    let bars: Vec<ProgressBar> = (0..n_chains).map(|chain_id| {
        let pb = mp.add(ProgressBar::new(n_steps as u64));
        pb.set_style(bar_style.clone());
        pb.set_prefix(format!("{}", chain_id + 1));
        pb
    }).collect();

    let t0 = std::time::Instant::now();

    // Run chains in parallel
    let results: Vec<(usize, PMMHResult)> = (0..n_chains)
        .into_par_iter()
        .map(|chain_id| {
            let chain_seed = seed ^ (chain_id as u64).wrapping_mul(0x9e3779b97f4a7c15);

            let pmmh_config = PMMHConfig {
                n_steps,
                n_particles,
                dt,
                proposal_sd: proposal_sd.clone(),
                adapt,
                adapt_start,
                thin,
                burn_in,
            };

            // Build the loglik evaluator closure for this chain
            let eval_loglik = |params: &[f64], pf_seed: u64| -> f64 {
                runner::run_quick_pfilter(&config, params, n_particles, pf_seed)
            };

            let bar = &bars[chain_id];
            let accepted_count = AtomicUsize::new(0);
            let progress_cb = |step: usize, loglik: f64, accepted: bool| {
                if accepted { accepted_count.fetch_add(1, Ordering::Relaxed); }
                if step % 100 == 0 || step == n_steps - 1 {
                    bar.set_position(step as u64 + 1);
                    let acc = accepted_count.load(Ordering::Relaxed) as f64 / (step + 1) as f64;
                    if loglik.is_finite() {
                        bar.set_message(format!("ll={:.1} acc={:.0}%", loglik, acc * 100.0));
                    } else {
                        bar.set_message(format!("ll=-inf acc={:.0}%", acc * 100.0));
                    }
                }
            };

            let result = run_pmmh(
                &config.if2_params, &priors, &config.base_params,
                &pmmh_config, &eval_loglik, chain_seed, Some(&progress_cb),
            );

            bar.finish_with_message(format!(
                "ll={:.1} acc={:.0}%", result.map_loglik, result.acceptance_rate * 100.0
            ));

            (chain_id, result)
        })
        .collect();

    let elapsed = t0.elapsed();

    // Write per-chain traces
    write_chain_traces(&stage_dir, &results, &config.if2_params)?;

    // Compute diagnostics
    let diagnostics = compute_diagnostics(&results, &config.if2_params);

    // Report
    eprintln!("\nacceptance rates:");
    for (chain_id, result) in &results {
        let status = if result.acceptance_rate < 0.10 {
            "\x1b[31m✗ too low\x1b[0m"
        } else if result.acceptance_rate > 0.50 {
            "\x1b[33m~ high\x1b[0m"
        } else {
            "\x1b[32m✓\x1b[0m"
        };
        eprintln!("  chain {}: {:.1}% {}", chain_id + 1, result.acceptance_rate * 100.0, status);
    }

    if n_chains > 1 {
        eprintln!("\nRhat:");
        for spec in &config.if2_params {
            if let Some(&rhat) = diagnostics.rhat.get(&spec.name) {
                let status = if rhat < 1.1 { "\x1b[32m✓\x1b[0m" } else if rhat < 1.5 { "\x1b[33m~\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
                let ess = diagnostics.ess.get(&spec.name).copied().unwrap_or(0.0);
                eprintln!("  {:12} Rhat={:.3} {} ESS={:.0}", spec.name, rhat, status, ess);
            }
        }
    }

    // Find MAP across all chains
    let (map_chain, map_result) = results.iter()
        .max_by(|a, b| a.1.map_log_posterior.total_cmp(&b.1.map_log_posterior))
        .unwrap();

    // Write summary JSON
    write_summary(&stage_dir, &results, &config, &diagnostics)?;

    // Write fit_state.toml
    let mut start_values = HashMap::new();
    for spec in config.if2_params.iter() {
        start_values.insert(spec.name.clone(), map_result.map_params[spec.index]);
    }
    // Include fixed params too
    for p in &config.model.parameters {
        if !start_values.contains_key(&p.name) {
            if let Some(&idx) = config.compiled.param_index.get(p.name.as_str()) {
                start_values.insert(p.name.clone(), config.base_params[idx]);
            }
        }
    }

    let state = FitState {
        stage: "pmmh".into(),
        seed,
        timestamp: now_iso8601_pub(),
        input_hash: None,
        camdl_version: Some(crate::version::VERSION_SHORT.into()),
        best_loglik: map_result.map_loglik,
        initial_loglik: ll_mean,
        best_chain: *map_chain,
        n_chains,
        n_good_chains: None,
        start_values,
        rw_sd: HashMap::new(),
    };
    state.save(&stage_dir)?;

    let wall_secs = elapsed.as_secs_f64();
    let total_pf_calls = n_chains * n_steps;
    eprintln!("\npmmh complete in {:.1}s ({} PF evaluations, {:.1}ms/eval): {}/",
        wall_secs, total_pf_calls,
        wall_secs * 1000.0 / total_pf_calls as f64 * n_chains as f64,
        stage_dir);
    eprintln!("  MAP loglik: {:.1} (chain {})", map_result.map_loglik, map_chain + 1);

    Ok(())
}

/// Build proposal SDs on the transformed scale.
fn build_proposal_sd(
    config: &FitRunConfig,
    sc: Option<&crate::fit::config::PMMHSampleConfig>,
    starts_from: Option<&str>,
) -> Result<Vec<f64>, String> {
    // Try to load scout chain endpoints for empirical covariance
    let proposal_dir = sc.and_then(|s| s.proposal_from.as_deref()).or(starts_from);
    if let Some(dir) = proposal_dir {
        if let Ok(sds) = load_scout_proposal_sd(dir, &config.if2_params) {
            eprintln!("  proposal_sd seeded from chain spread in {}/", dir);
            return Ok(sds);
        }
    }

    // Fallback: use rw_sd from [estimate], scaled up for MH jumps
    // IF2 rw_sd is per-perturbation-step; PMMH needs per-proposal (larger)
    Ok(config.if2_params.iter().map(|p| {
        p.transformed_sd(p.rw_sd, p.initial) * 5.0
    }).collect())
}

/// Load chain endpoint parameters from a prior stage and compute
/// empirical SD on the transformed scale. Scale by 2.38/√d (optimal RWM).
fn load_scout_proposal_sd(dir: &str, if2_params: &[IF2Param]) -> Result<Vec<f64>, String> {
    // Find chain directories
    let mut chain_dirs: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {}", dir, e))? {
        let entry = entry.map_err(|e| format!("{}", e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("chain_") && entry.path().is_dir() {
            chain_dirs.push(entry.path().to_string_lossy().to_string());
        }
    }
    if chain_dirs.len() < 2 {
        return Err("need at least 2 chains for empirical covariance".into());
    }

    // Read final params from each chain
    let d = if2_params.len();
    let mut transformed_endpoints: Vec<Vec<f64>> = Vec::new();

    for chain_dir in &chain_dirs {
        let toml_path = format!("{}/final_params.toml", chain_dir);
        let contents = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("{}: {}", toml_path, e))?;
        let parsed: HashMap<String, toml::Value> = toml::from_str(&contents)
            .map_err(|e| format!("{}: {}", toml_path, e))?;

        let mut z = Vec::with_capacity(d);
        for spec in if2_params {
            let v = parsed.get(&spec.name)
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .ok_or_else(|| format!("missing {} in {}", spec.name, toml_path))?;
            z.push(spec.to_transformed(v));
        }
        transformed_endpoints.push(z);
    }

    // Compute per-parameter SD on transformed scale
    let n = transformed_endpoints.len() as f64;
    let scale = 2.38 / (d as f64).sqrt();

    let sds: Vec<f64> = (0..d).map(|i| {
        let mean = transformed_endpoints.iter().map(|z| z[i]).sum::<f64>() / n;
        let var = transformed_endpoints.iter().map(|z| (z[i] - mean).powi(2)).sum::<f64>() / (n - 1.0);
        (var.sqrt() * scale).max(0.01) // floor to prevent zero proposal
    }).collect();

    Ok(sds)
}

struct Diagnostics {
    rhat: HashMap<String, f64>,
    ess: HashMap<String, f64>,
}

fn compute_diagnostics(
    results: &[(usize, PMMHResult)],
    if2_params: &[IF2Param],
) -> Diagnostics {
    let n_chains = results.len();
    let mut rhat_map = HashMap::new();
    let mut ess_map = HashMap::new();

    for spec in if2_params {
        // Collect per-chain samples for this parameter
        let chains: Vec<Vec<f64>> = results.iter()
            .map(|(_, r)| r.steps.iter().map(|s| s.params[spec.index]).collect())
            .collect();

        // ESS: sum across chains
        let total_ess: f64 = chains.iter().map(|c| mcmc_ess(c)).sum();
        ess_map.insert(spec.name.clone(), total_ess);

        // Rhat across chains
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

fn write_chain_traces(
    dir: &str,
    results: &[(usize, PMMHResult)],
    if2_params: &[IF2Param],
) -> Result<(), String> {
    use std::io::Write;

    for (chain_id, result) in results {
        let chain_dir = format!("{}/chain_{}", dir, chain_id + 1);
        std::fs::create_dir_all(&chain_dir)
            .map_err(|e| format!("cannot create {}: {}", chain_dir, e))?;

        let trace_path = format!("{}/trace.tsv", chain_dir);
        let mut f = std::fs::File::create(&trace_path)
            .map_err(|e| format!("cannot write {}: {}", trace_path, e))?;
        writeln!(f, "# {}", crate::version::VERSION).unwrap();
        write!(f, "step\tlog_likelihood\tlog_posterior\taccepted").unwrap();
        for spec in if2_params { write!(f, "\t{}", spec.name).unwrap(); }
        writeln!(f).unwrap();

        for step in &result.steps {
            let log_posterior = step.log_likelihood + step.log_prior;
            write!(f, "{}\t{:.4}\t{:.4}\t{}",
                step.step, step.log_likelihood, log_posterior,
                if step.accepted { 1 } else { 0 }
            ).unwrap();
            for spec in if2_params {
                write!(f, "\t{:.6}", step.params[spec.index]).unwrap();
            }
            writeln!(f).unwrap();
        }
    }
    Ok(())
}

fn write_summary(
    dir: &str,
    results: &[(usize, PMMHResult)],
    config: &FitRunConfig,
    diagnostics: &Diagnostics,
) -> Result<(), String> {
    let acceptance_rates: Vec<f64> = results.iter().map(|(_, r)| r.acceptance_rate).collect();

    let (map_chain, map_result) = results.iter()
        .max_by(|a, b| a.1.map_log_posterior.total_cmp(&b.1.map_log_posterior))
        .unwrap();

    let map_params: HashMap<String, f64> = config.if2_params.iter()
        .map(|spec| (spec.name.clone(), map_result.map_params[spec.index]))
        .collect();

    let summary = serde_json::json!({
        "stage": "pmmh",
        "n_chains": results.len(),
        "steps_per_chain": results.first().map(|(_, r)| r.n_steps).unwrap_or(0),
        "acceptance_rate": acceptance_rates,
        "rhat": diagnostics.rhat,
        "ess": diagnostics.ess,
        "map_loglik": map_result.map_loglik,
        "map_chain": map_chain + 1,
        "map_params": map_params,
    });

    let path = format!("{}/pmmh_summary.json", dir);
    let contents = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("json error: {}", e))?;
    std::fs::write(&path, contents)
        .map_err(|e| format!("cannot write {}: {}", path, e))
}
