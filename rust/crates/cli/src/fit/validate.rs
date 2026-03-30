//! `camdl fit validate` — final IF2 + profiles + precise pfilter at MLE.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use crate::fit::runner::{self, FitRunConfig, ObsModelKind};
use crate::fit::provenance::{self, MleMetadata};
use crate::hashing;
use sha2::Digest;
use sim::inference::{
    bootstrap_filter,
    if2::{IF2Config, IF2Param, run_if2},
    obs_loglik::{negbin_logpmf, discretized_normal_logpmf_tol},
    ParticleState,
};
use sim::chain_binomial::step_one;
use sim::ekrng::StatefulRng;
use std::collections::HashMap;

const DEFAULT_CHAINS: usize = 4;
const DEFAULT_PARTICLES: usize = 5000;
const DEFAULT_ITERATIONS: usize = 100;
const DEFAULT_COOLING: f64 = 0.95;
const PFILTER_PARTICLES: usize = 10000;

pub fn run_validate(fit: &FitToml, starts_from: &str, seed: u64, force: bool) -> Result<(), String> {
    let stage_dir = format!("{}/validate", fit.fit.output_dir);
    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create {}: {}", stage_dir, e))?;

    let prior_state = FitState::load(starts_from)?;
    eprintln!("validate: starting from {} (loglik={:.1})", starts_from, prior_state.best_loglik);

    let config = FitRunConfig::build(
        fit, Some(&prior_state),
        DEFAULT_CHAINS, DEFAULT_PARTICLES, DEFAULT_ITERATIONS,
        DEFAULT_COOLING, seed, false,
    )?;

    // ── Phase 1: Run IF2 chains ──────────────────────────────────────────────
    let t0_if2 = std::time::Instant::now();
    let chain_results = runner::run_chains(&config);
    let if2_elapsed = t0_if2.elapsed();

    let all_converged = chain_results.rhat.values().all(|&r| r < 1.1);
    if !all_converged {
        eprintln!("\nwarning: not all parameters converged in validate stage");
    }

    let best = &chain_results.results.iter()
        .find(|(id, _)| *id == chain_results.best_chain)
        .unwrap().1;

    // ── Phase 2: Precise pfilter at MLE ──────────────────────────────────────
    eprintln!("\nrunning pfilter at MLE with {} particles...", PFILTER_PARTICLES);

    let mut mle_params = config.base_params.clone();
    for spec in &config.if2_params {
        mle_params[spec.index] = best.mle[spec.index];
    }

    let pf_result = run_pfilter_at_mle(&config, &mle_params, PFILTER_PARTICLES, seed)?;
    let loglik = pf_result.loglik;
    let loglik_sd = pf_result.loglik_sd;
    let ess_mean = pf_result.ess_mean;
    let ess_min = pf_result.ess_min;

    eprintln!("  loglik = {:.1} ± {:.1}", loglik, loglik_sd);
    eprintln!("  ESS at MLE: mean={:.0}, min={:.0}", ess_mean, ess_min);

    if ess_min < PFILTER_PARTICLES as f64 / 4.0 {
        eprintln!("\n\x1b[33mwarning: low ESS at MLE (min={:.0})\x1b[0m", ess_min);
        eprintln!("  Possible causes:");
        eprintln!("    - Observation model too tight (estimate psi, or increase it)");
        eprintln!("    - Process noise too low (estimate sigma_se, or increase it)");
        eprintln!("    - Model structure cannot reproduce observed dynamics");
    }

    // Write ESS trace
    write_ess_trace(&stage_dir, &pf_result)?;

    // ── Phase 3: Profile likelihoods ─────────────────────────────────────────
    eprintln!("\nprofiling all {} estimated parameters...", config.if2_params.len());
    let profile_dir = format!("{}/profiles", stage_dir);
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("cannot create {}: {}", profile_dir, e))?;

    let profiles = run_profiles(&config, &mle_params, &profile_dir, seed)?;

    // ── Write outputs ────────────────────────────────────────────────────────
    let start_values: HashMap<String, f64> = config.if2_params.iter()
        .map(|spec| (spec.name.clone(), best.mle[spec.index]))
        .collect();

    let rw_sd = match runner::auto_rw_sd(&chain_results.results, &config.if2_params) {
        Ok((rw, _)) => rw,
        Err(_) => config.if2_params.iter()
            .map(|s| (s.name.clone(), s.rw_sd * 0.5))
            .collect(),
    };

    let state = FitState {
        stage: "validate".into(),
        seed,
        timestamp: crate::fit::scout::now_iso8601_pub(),
        best_loglik: loglik,
        initial_loglik: prior_state.best_loglik,
        best_chain: chain_results.best_chain,
        n_chains: DEFAULT_CHAINS,
        n_good_chains: None,
        start_values,
        rw_sd,
    };
    state.save(&stage_dir)?;

    // Per-chain outputs
    let param_names: Vec<String> = config.model.parameters.iter().map(|p| p.name.clone()).collect();
    runner::write_chain_outputs(
        &stage_dir, &chain_results.results, &config.if2_params,
        &param_names, &config.base_params,
    )?;
    runner::write_diagnostics(&stage_dir, &chain_results.results)?;

    // mle_params.toml with provenance
    let all_params = runner::collect_all_params(
        &best.mle, &config.if2_params, &config.model,
        &config.base_params, &config.compiled,
    );
    let model_hash = hashing::model_hash(&config.model_ir_json);
    let input_hash = compute_input_hash(fit, &config, seed);

    let metadata = MleMetadata {
        input_hash: input_hash.clone(),
        model_path: fit.fit.model.clone(),
        model_hash: model_hash.clone(),
        data_hashes: fit.data.iter().map(|(name, path)| {
            let bytes = std::fs::read(path).unwrap_or_default();
            let hash = hex::encode(&sha2::Sha256::digest(&bytes)[..4]);
            (format!("{} ({})", name, path), hash)
        }).collect(),
        seed,
        stage: "validate".into(),
        best_chain: chain_results.best_chain,
        loglik,
        loglik_sd,
        n_particles: PFILTER_PARTICLES,
        ess_at_mle: Some((ess_mean, ess_min)),
        timestamp: state.timestamp.clone(),
    };
    provenance::write_mle_params(
        &format!("{}/mle_params.toml", stage_dir),
        &all_params,
        &metadata,
    )?;

    // fit_record.json
    write_fit_record(&stage_dir, fit, &config, &chain_results, &metadata, &profiles, &all_params)?;

    // fit_report.txt
    write_fit_report(&stage_dir, fit, &config, &chain_results, &metadata, &profiles)?;

    // pfilter_loglik.txt
    std::fs::write(
        format!("{}/pfilter_loglik.txt", stage_dir),
        format!("{:.4} ± {:.4} (N={})\n", loglik, loglik_sd, PFILTER_PARTICLES),
    ).ok();

    // Summary JSON
    write_summary(&stage_dir, &chain_results, &config, &metadata, &profiles)?;

    let total_elapsed = t0_if2.elapsed();
    eprintln!("\nvalidate complete in {:.1}s (IF2: {:.1}s): {}/",
        total_elapsed.as_secs_f64(), if2_elapsed.as_secs_f64(), stage_dir);
    eprintln!("  loglik: {:.1} ± {:.1} (N={})", loglik, loglik_sd, PFILTER_PARTICLES);
    eprintln!("  ESS: mean={:.0}, min={:.0}", ess_mean, ess_min);
    eprintln!("  converged: {}", if all_converged { "yes" } else { "NO" });
    eprintln!("  profiles: {}/profiles/", stage_dir);
    eprintln!("\nnext: camdl experiment run experiment.toml --params {}/mle_params.toml", stage_dir);

    Ok(())
}

struct PfilterResult {
    loglik: f64,
    loglik_sd: f64,
    ess_mean: f64,
    ess_min: f64,
    ess_trace: Vec<f64>,
    obs_times: Vec<f64>,
}

fn run_pfilter_at_mle(
    config: &FitRunConfig,
    params: &[f64],
    n_particles: usize,
    seed: u64,
) -> Result<PfilterResult, String> {
    let compiled = &*config.compiled;
    let observations: Vec<sim::inference::particle_filter::Observation> = config.observations.iter()
        .map(|o| sim::inference::particle_filter::Observation { time: o.time, value: o.value })
        .collect();

    let step_fn = |state: &mut ParticleState, t: f64, step_dt: f64, rng: &mut StatefulRng| {
        step_one(compiled, &mut state.counts, &mut state.flow_accumulators, params, t, step_dt, rng)
    };
    let flow_indices = &config.flow_indices;
    let project_fn = |state: &ParticleState| -> f64 {
        flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
    };
    let dmeasure_fn: Box<dyn Fn(f64, f64) -> f64> = match config.obs_model {
        ObsModelKind::NegBin => {
            let rho = config.rho_idx.map_or(1.0, |i| params[i]);
            let k = config.k_idx.map_or(10.0, |i| params[i]);
            Box::new(move |proj: f64, obs: f64| negbin_logpmf(obs, rho * proj, k))
        }
        ObsModelKind::DiscretizedNormal => {
            let rho = config.rho_idx.map_or(1.0, |i| params[i]);
            let psi = config.psi_idx.map_or(0.116, |i| params[i]);
            let tol = config.tol;
            Box::new(move |proj: f64, obs: f64| {
                let mu = rho * proj;
                discretized_normal_logpmf_tol(obs, mu, mu * (1.0 - rho + psi * psi * mu), tol)
            })
        }
    };

    let result = bootstrap_filter(
        compiled, params, &observations, n_particles, config.if2_config.dt,
        &step_fn, &project_fn, &*dmeasure_fn, seed,
    ).map_err(|e| format!("pfilter error: {:?}", e))?;

    let ess_mean = result.ess_trace.iter().sum::<f64>() / result.ess_trace.len() as f64;
    let ess_min = result.ess_trace.iter().cloned().fold(f64::INFINITY, f64::min);

    // Estimate loglik SD via batching (split observations into 10 blocks)
    let loglik_sd = estimate_loglik_sd(&result.ll_increments);

    Ok(PfilterResult {
        loglik: result.log_likelihood,
        loglik_sd,
        ess_mean,
        ess_min,
        ess_trace: result.ess_trace,
        obs_times: observations.iter().map(|o| o.time).collect(),
    })
}

fn estimate_loglik_sd(ll_increments: &[f64]) -> f64 {
    // Bootstrap SE estimate: SD of 50 replicate loglik sums using resampled increments
    // Simpler approach: use batch means with ~10 batches
    let n = ll_increments.len();
    if n < 10 { return 0.0; }
    let batch_size = n / 10;
    let batch_sums: Vec<f64> = (0..10).map(|b| {
        let start = b * batch_size;
        let end = if b == 9 { n } else { start + batch_size };
        ll_increments[start..end].iter().sum::<f64>()
    }).collect();
    let mean = batch_sums.iter().sum::<f64>() / 10.0;
    let var = batch_sums.iter().map(|&s| (s - mean).powi(2)).sum::<f64>() / 9.0;
    (var * 10.0).sqrt() // SE of the total sum
}

fn write_ess_trace(dir: &str, pf: &PfilterResult) -> Result<(), String> {
    use std::io::Write;
    let path = format!("{}/ess_at_mle.tsv", dir);
    let mut f = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {}", path, e))?;
    writeln!(f, "time\tESS").unwrap();
    for (t, ess) in pf.obs_times.iter().zip(&pf.ess_trace) {
        writeln!(f, "{}\t{:.1}", t, ess).unwrap();
    }
    Ok(())
}

#[derive(Default)]
struct ProfileResult {
    name: String,
    ci_lower: f64,
    ci_upper: f64,
    curvature: f64,
}

fn run_profiles(
    config: &FitRunConfig,
    mle_params: &[f64],
    profile_dir: &str,
    seed: u64,
) -> Result<Vec<ProfileResult>, String> {
    use rayon::prelude::*;
    use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

    let n_grid = 21; // points per profile
    let mp = MultiProgress::new();
    let style = ProgressStyle::default_bar()
        .template("{prefix:>12} [{bar:20}] {pos}/{len}")
        .unwrap();

    let results: Vec<ProfileResult> = config.if2_params.par_iter().map(|focal| {
        let pb = mp.add(ProgressBar::new(n_grid as u64));
        pb.set_style(style.clone());
        pb.set_prefix(focal.name.clone());

        let mle_value = mle_params[focal.index];
        let (lo, hi) = if focal.lower.is_finite() && focal.upper.is_finite() {
            (focal.lower, focal.upper)
        } else {
            (mle_value * 0.5, mle_value * 1.5)
        };

        let grid: Vec<f64> = (0..n_grid).map(|i| {
            lo + (hi - lo) * i as f64 / (n_grid - 1) as f64
        }).collect();

        let mut profile_points: Vec<(f64, f64)> = Vec::new();

        for &focal_value in &grid {
            // Run short IF2 with focal param fixed at grid value
            let mut fixed_params: Vec<IF2Param> = config.if2_params.iter()
                .filter(|p| p.name != focal.name)
                .cloned()
                .collect();
            // Set initial values to MLE for non-focal params
            for p in &mut fixed_params {
                p.initial = mle_params[p.index];
                p.rw_sd *= 0.5; // tighter perturbation for profile
            }

            let mut base = mle_params.to_vec();
            base[focal.index] = focal_value;

            let profile_config = IF2Config {
                n_particles: 1000,
                n_iterations: 30,
                cooling_fraction: 0.95,
                cooling_target_iters: 30,
                dt: config.if2_config.dt,
            };

            let step_fn = |state: &mut ParticleState, p: &[f64], t: f64, step_dt: f64, rng: &mut StatefulRng| {
                step_one(&config.compiled, &mut state.counts, &mut state.flow_accumulators, p, t, step_dt, rng)
            };
            let flow_indices = &config.flow_indices;
            let project_fn = |state: &ParticleState| -> f64 {
                flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
            };
            let dmeasure_fn: Box<dyn Fn(f64, f64, &[f64]) -> f64> = match config.obs_model {
                ObsModelKind::NegBin => Box::new(move |proj: f64, obs: f64, p: &[f64]| {
                    let rho = config.rho_idx.map_or(1.0, |i| p[i]);
                    let k = config.k_idx.map_or(10.0, |i| p[i]);
                    negbin_logpmf(obs, rho * proj, k)
                }),
                ObsModelKind::DiscretizedNormal => {
                    let tol = config.tol;
                    Box::new(move |proj: f64, obs: f64, p: &[f64]| {
                        let rho = config.rho_idx.map_or(1.0, |i| p[i]);
                        let psi = config.psi_idx.map_or(0.116, |i| p[i]);
                        let mu = rho * proj;
                        discretized_normal_logpmf_tol(obs, mu, mu * (1.0 - rho + psi * psi * mu), tol)
                    })
                }
            };

            let chain_seed = seed ^ focal.index as u64 ^ (focal_value.to_bits());
            let result = run_if2(
                &config.compiled, &base, &fixed_params, &config.observations,
                &profile_config, &step_fn, &project_fn, &*dmeasure_fn, chain_seed,
            );

            match result {
                Ok(r) => profile_points.push((focal_value, r.final_loglik)),
                Err(e) => {
                    eprintln!("warning: profile point {}={:.4} failed: {:?}", focal.name, focal_value, e);
                    profile_points.push((focal_value, f64::NEG_INFINITY));
                }
            }
            pb.inc(1);
        }
        pb.finish();

        // Write profile TSV
        {
            use std::io::Write;
            let path = format!("{}/{}_profile.tsv", profile_dir, focal.name);
            if let Ok(mut f) = std::fs::File::create(&path) {
                writeln!(f, "{}\tloglik", focal.name).ok();
                for (v, ll) in &profile_points {
                    writeln!(f, "{:.6}\t{:.2}", v, ll).ok();
                }
            }
        }

        // Compute 95% CI from profile (chi-squared cutoff = 1.92)
        let max_ll = profile_points.iter().map(|(_, ll)| *ll).fold(f64::NEG_INFINITY, f64::max);
        let threshold = max_ll - 1.92;
        let above: Vec<f64> = profile_points.iter()
            .filter(|(_, ll)| *ll >= threshold)
            .map(|(v, _)| *v)
            .collect();
        let ci_lower = above.iter().cloned().fold(f64::INFINITY, f64::min);
        let ci_upper = above.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        // Rough curvature at MLE
        let curvature = if profile_points.len() >= 3 {
            let step = (hi - lo) / (n_grid - 1) as f64;
            let mid_idx = n_grid / 2;
            if mid_idx > 0 && mid_idx < profile_points.len() - 1 {
                let ll_minus = profile_points[mid_idx - 1].1;
                let ll_mid = profile_points[mid_idx].1;
                let ll_plus = profile_points[mid_idx + 1].1;
                -(ll_plus - 2.0 * ll_mid + ll_minus) / (step * step)
            } else { 0.0 }
        } else { 0.0 };

        ProfileResult {
            name: focal.name.clone(),
            ci_lower,
            ci_upper,
            curvature,
        }
    }).collect();

    Ok(results)
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

fn write_fit_record(
    dir: &str,
    fit: &FitToml,
    config: &FitRunConfig,
    results: &runner::ChainResults,
    metadata: &MleMetadata,
    profiles: &[ProfileResult],
    all_params: &HashMap<String, f64>,
) -> Result<(), String> {
    let record = serde_json::json!({
        "model": {
            "path": fit.fit.model,
            "hash": &metadata.model_hash[..8.min(metadata.model_hash.len())],
        },
        "data": fit.data.iter().map(|(name, path)| {
            (name.clone(), serde_json::json!({ "path": path }))
        }).collect::<serde_json::Map<String, serde_json::Value>>(),
        "fit_config": {
            "estimated": config.if2_params.iter().map(|p| &p.name).collect::<Vec<_>>(),
            "fixed": fit.fixed.keys().collect::<Vec<_>>(),
        },
        "method": {
            "algorithm": "IF2",
            "backend": fit.config.backend,
            "dt": fit.config.dt,
            "seed": metadata.seed,
        },
        "results": {
            "mle": all_params,
            "loglik": metadata.loglik,
            "loglik_sd": metadata.loglik_sd,
            "ess_at_mle": metadata.ess_at_mle.map(|(m, n)| serde_json::json!({"mean": m, "min": n})),
            "convergence": {
                "rhat_max": results.rhat.values().cloned().fold(0.0_f64, f64::max),
                "all_converged": results.rhat.values().all(|&r| r < 1.1),
            },
            "identifiability": profiles.iter().map(|p| {
                (p.name.clone(), serde_json::json!({
                    "ci_95": [p.ci_lower, p.ci_upper],
                    "curvature": p.curvature,
                }))
            }).collect::<serde_json::Map<String, serde_json::Value>>(),
        },
        "provenance": {
            "input_hash": metadata.input_hash,
            "content_hash": provenance::compute_content_hash(all_params),
            "timestamp": metadata.timestamp,
            "camdl_version": env!("CARGO_PKG_VERSION"),
        },
    });

    let path = format!("{}/fit_record.json", dir);
    std::fs::write(&path, serde_json::to_string_pretty(&record).unwrap())
        .map_err(|e| format!("cannot write {}: {}", path, e))
}

fn write_fit_report(
    dir: &str,
    fit: &FitToml,
    config: &FitRunConfig,
    results: &runner::ChainResults,
    metadata: &MleMetadata,
    profiles: &[ProfileResult],
) -> Result<(), String> {
    use std::io::Write;
    let path = format!("{}/fit_report.txt", dir);
    let mut f = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {}", path, e))?;

    writeln!(f, "camdl fit report").unwrap();
    writeln!(f, "================").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "Model:     {}", fit.fit.model).unwrap();
    writeln!(f, "Backend:   {}", fit.config.backend).unwrap();
    writeln!(f, "Timestamp: {}", metadata.timestamp).unwrap();
    writeln!(f).unwrap();
    writeln!(f, "Log-likelihood: {:.1} ± {:.1} (N={})", metadata.loglik, metadata.loglik_sd, metadata.n_particles).unwrap();
    if let Some((ess_mean, ess_min)) = metadata.ess_at_mle {
        writeln!(f, "ESS at MLE:     mean={:.0}, min={:.0}", ess_mean, ess_min).unwrap();
    }
    writeln!(f).unwrap();
    writeln!(f, "Estimated parameters:").unwrap();
    for spec in &config.if2_params {
        let best = &results.results.iter()
            .find(|(id, _)| *id == results.best_chain).unwrap().1;
        let rhat = results.rhat.get(&spec.name).copied().unwrap_or(f64::NAN);
        let profile = profiles.iter().find(|p| p.name == spec.name);
        let ci = profile.map(|p| format!("[{:.4}, {:.4}]", p.ci_lower, p.ci_upper))
            .unwrap_or_else(|| "—".into());
        writeln!(f, "  {:12} = {:<12.6} Rhat={:.3}  CI_95={}", spec.name, best.mle[spec.index], rhat, ci).unwrap();
    }
    writeln!(f).unwrap();
    writeln!(f, "Fixed parameters:").unwrap();
    for name in fit.fixed.keys() {
        writeln!(f, "  {}", name).unwrap();
    }
    writeln!(f).unwrap();
    writeln!(f, "Provenance: input_hash={}, content_hash={}",
        metadata.input_hash, provenance::compute_content_hash(&HashMap::new())).unwrap();

    Ok(())
}

fn write_summary(
    dir: &str,
    results: &runner::ChainResults,
    config: &FitRunConfig,
    metadata: &MleMetadata,
    profiles: &[ProfileResult],
) -> Result<(), String> {
    let summary = serde_json::json!({
        "stage": "validate",
        "n_chains": config.n_chains,
        "best_loglik": metadata.loglik,
        "loglik_sd": metadata.loglik_sd,
        "ess_at_mle": metadata.ess_at_mle.map(|(m, n)| serde_json::json!({"mean": m, "min": n})),
        "input_hash": metadata.input_hash,
        "converged": results.rhat.values().all(|&r| r < 1.1),
        "parameters": config.if2_params.iter().map(|spec| {
            let rhat = results.rhat.get(&spec.name).copied().unwrap_or(f64::NAN);
            let best = &results.results.iter()
                .find(|(id, _)| *id == results.best_chain).unwrap().1;
            let profile = profiles.iter().find(|p| p.name == spec.name);
            serde_json::json!({
                "name": spec.name,
                "estimate": best.mle[spec.index],
                "rhat": rhat,
                "ci_95": profile.map(|p| vec![p.ci_lower, p.ci_upper]),
                "curvature": profile.map(|p| p.curvature),
            })
        }).collect::<Vec<_>>(),
    });

    let path = format!("{}/validate_summary.json", dir);
    std::fs::write(&path, serde_json::to_string_pretty(&summary).unwrap())
        .map_err(|e| format!("cannot write {}: {}", path, e))
}
