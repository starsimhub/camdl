//! `camdl fit` — structured inference workflow.
//!
//! Usage:
//!   camdl fit scout    fit.toml [--seed N] [--force]
//!   camdl fit refine   fit.toml --starts-from scout/ [--seed N] [--force]
//!   camdl fit validate fit.toml --starts-from refine/ [--seed N] [--force]
//!   camdl fit pmmh     fit.toml [--starts-from validate/] [--seed N] [--force] [--resume] [--check-variance]
//!   camdl fit pgas     fit.toml [--starts-from validate/] [--seed N] [--force]
//!   camdl fit status   fit.toml

pub mod config;
#[allow(dead_code)]
pub mod config_v2;
pub mod state;
pub mod provenance;
pub mod runner;
pub mod scout;
pub mod refine;
pub mod validate;
pub mod status;
pub mod pmmh;
pub mod pgas;
pub mod trace_writer;

use config::FitToml;

pub fn cmd_fit_scout(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, false);

    // Validate partition
    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    fit.validate_bounds(&model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    scout::run_scout(&fit, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_refine(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, true);
    let starts_from = parse_starts_from(args);

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    refine::run_refine(&fit, &starts_from, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_validate(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, true);
    let starts_from = parse_starts_from(args);

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    validate::run_validate(&fit, &starts_from, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_pmmh(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, false);
    let starts_from = parse_optional_starts_from(args);
    let check_variance = args.iter().any(|a| a == "--check-variance");
    let resume = args.iter().any(|a| a == "--resume");

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    fit.validate_bounds(&model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    pmmh::run_pmmh_cli(&fit, starts_from.as_deref(), seed, force, check_variance, resume).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_pgas(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, false);
    let starts_from = parse_optional_starts_from(args);
    let no_nuts = args.iter().any(|a| a == "--no-nuts");
    let diagonal_mass = args.iter().any(|a| a == "--diagonal-mass");
    let resume = args.iter().any(|a| a == "--resume");

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    fit.validate_bounds(&model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    pgas::run_pgas_cli(&fit, starts_from.as_deref(), seed, force, !no_nuts, !diagonal_mass, resume).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_status(args: &[String]) {
    let (fit, _, _) = parse_fit_args(args, false);
    status::run_status(&fit).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

// ─── New `camdl fit run` entry point (config_v2) ────────────────────────────

pub fn cmd_fit_run_v2(args: &[String]) {
    use config_v2::{FitConfigV2, Stage, StartsFrom};

    let mut fit_path: Option<String> = None;
    let mut seed = 1_u64;
    let mut _force = false;
    let mut stage_filter: Option<String> = None;
    let mut starts_from_override: Option<String> = None;
    let mut has_seed_flag = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => { i += 1; seed = args[i].parse().expect("--seed needs integer"); has_seed_flag = true; }
            "--force" => { _force = true; }
            "--stage" => { i += 1; stage_filter = Some(args[i].clone()); }
            "--starts-from" => { i += 1; starts_from_override = Some(args[i].clone()); }
            "--resume" => { /* TODO: wire up */ }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                eprintln!("usage: camdl fit run FIT.toml [--stage NAME] [--seed N] [--force]");
                std::process::exit(1);
            }
            path => { fit_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let fit_path = fit_path.unwrap_or_else(|| {
        eprintln!("usage: camdl fit run FIT.toml [--stage NAME] [--seed N] [--force]");
        std::process::exit(1);
    });

    // Load v2 config
    let config = FitConfigV2::load(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    // Load model and validate completeness
    let (model, model_json) = crate::util::load_model(&config.model.camdl).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    config.validate(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    // Determine which stages to run
    let stages_to_run: Vec<(&str, &Stage)> = if let Some(ref name) = stage_filter {
        match config.stages.get(name.as_str()) {
            Some(stage) => vec![(name.as_str(), stage)],
            None => {
                let available: Vec<&str> = config.stages.keys().map(|s| s.as_str()).collect();
                eprintln!("error: stage '{}' not found. Available: {}", name, available.join(", "));
                std::process::exit(1);
            }
        }
    } else {
        config.stages.iter().map(|(k, v)| (k.as_str(), v)).collect()
    };

    let fit_dir = config.fit_dir(&fit_path);

    eprintln!("fit: {} ({} stage{})",
        fit_path,
        stages_to_run.len(),
        if stages_to_run.len() == 1 { "" } else { "s" },
    );
    eprintln!("  model:    {}", config.model.camdl);
    eprintln!("  estimate: {}", config.estimate.keys()
        .map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
    eprintln!("  fixed:    {}", {
        let resolved = config.fixed.resolve().unwrap_or_default();
        resolved.keys().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    });
    eprintln!("  output:   {}", fit_dir.display());

    // Convert to legacy FitToml for runner compatibility
    let legacy = config.to_legacy_toml().unwrap_or_else(|e| {
        eprintln!("error converting to legacy format: {}", e);
        std::process::exit(1);
    });

    // Seed: CLI > random
    let seed = if has_seed_flag {
        seed
    } else {
        use std::time::SystemTime;
        let dur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
        dur.as_nanos() as u64 % 1_000_000
    };

    // Execute stages in order
    for (stage_name, stage) in &stages_to_run {
        let stage_dir = fit_dir.join(stage_name);
        eprintln!("\n── stage: {} (method={}) ──", stage_name, stage.method_name());

        // Config hash staleness check
        let fixed_resolved = config.fixed.resolve().unwrap_or_default();
        let config_hash = provenance::compute_config_hash_v2(
            &model_json, &config.data.observations, &config.estimate,
            &fixed_resolved, stage_name, stage, seed,
        );
        if !_force {
            match provenance::check_config_hash(&stage_dir.to_string_lossy(), &config_hash) {
                provenance::ConfigCacheStatus::Match => {
                    eprintln!("  \x1b[33mskipped — results already exist for these inputs.\x1b[0m");
                    eprintln!("  config_hash: {}", &config_hash[..16]);
                    eprintln!("  Use --force to re-run.");
                    continue;
                }
                provenance::ConfigCacheStatus::Stale { stored, current } => {
                    eprintln!("  \x1b[33mstale results detected — config has changed. Re-running.\x1b[0m");
                    eprintln!("  stored:  {}", &stored[..16.min(stored.len())]);
                    eprintln!("  current: {}", &current[..16.min(current.len())]);
                }
                provenance::ConfigCacheStatus::NotFound => {}
            }
        }

        // Resolve starts_from: CLI override > stage config
        let effective_starts = if let Some(ref cli_sf) = starts_from_override {
            // CLI --starts-from applies to the target stage only
            if stages_to_run.len() == 1 {
                Some(cli_sf.clone())
            } else {
                None // only applies when running a single stage
            }
        } else {
            match stage.starts_from() {
                StartsFrom::Random => None,
                StartsFrom::Stage(ref dep_name) => {
                    // Resolve to the directory of a prior stage in this fit
                    Some(fit_dir.join(dep_name).to_string_lossy().to_string())
                }
                StartsFrom::Directory(ref path) => {
                    Some(path.to_string_lossy().to_string())
                }
            }
        };

        match stage {
            Stage::IF2 { chains, particles, iterations, cooling, .. } => {
                let prior_state = effective_starts.as_ref().and_then(|dir| {
                    state::FitState::load(dir).ok()
                });

                let run_config = runner::FitRunConfig::build(
                    &legacy,
                    prior_state.as_ref(),
                    *chains, *particles, *iterations,
                    *cooling, seed, effective_starts.is_none(),
                ).unwrap_or_else(|e| {
                    eprintln!("error building run config: {}", e);
                    std::process::exit(1);
                });

                std::fs::create_dir_all(&stage_dir).unwrap_or_else(|e| {
                    eprintln!("error creating {}: {}", stage_dir.display(), e);
                    std::process::exit(1);
                });

                let collector = sim::inference::diagnostic::DiagnosticCollector::new(stage_name);
                let t0 = std::time::Instant::now();
                let chain_results = runner::run_chains_with_diagnostics(&run_config, &collector);
                let elapsed = t0.elapsed();

                // Write outputs
                let param_names: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
                runner::write_chain_outputs(
                    &stage_dir.to_string_lossy(), &chain_results.results,
                    &run_config.estimated_params, &param_names,
                    &run_config.base_params, &run_config.compiled,
                ).unwrap_or_else(|e| eprintln!("warning: {}", e));
                runner::write_diagnostics(&stage_dir.to_string_lossy(), &chain_results.results)
                    .unwrap_or_else(|e| eprintln!("warning: {}", e));

                // Write fit_state.toml for downstream stages
                let best = &chain_results.results.iter()
                    .find(|(id, _)| *id == chain_results.best_chain)
                    .unwrap().1;
                let start_values = runner::collect_all_params(
                    &best.mle, &run_config.estimated_params, &run_config.model,
                    &run_config.base_params, &run_config.compiled,
                );
                let rw_sd = match runner::auto_rw_sd(&chain_results.results, &run_config.estimated_params) {
                    Ok((rw, _)) => rw,
                    Err(_) => run_config.estimated_params.iter()
                        .map(|s| (s.name.clone(), s.rw_sd * 0.5))
                        .collect(),
                };
                let fit_state = state::FitState {
                    stage: stage_name.to_string(),
                    seed,
                    timestamp: scout::now_iso8601_pub(),
                    input_hash: None,
                    camdl_version: Some(crate::version::VERSION_SHORT.into()),
                    best_loglik: chain_results.best_loglik,
                    initial_loglik: f64::NEG_INFINITY,
                    best_chain: chain_results.best_chain,
                    n_chains: *chains,
                    n_good_chains: None,
                    start_values,
                    rw_sd,
                    loglik_type: Some("if2".into()),
                    acceptance_rate: None,
                };
                fit_state.save(&stage_dir.to_string_lossy()).unwrap_or_else(|e| {
                    eprintln!("warning: could not save fit_state: {}", e);
                });

                // Write mle_params.toml
                let all_params = runner::collect_all_params(
                    &best.mle, &run_config.estimated_params, &run_config.model,
                    &run_config.base_params, &run_config.compiled,
                );
                let mle_path = format!("{}/mle_params.toml", stage_dir.display());
                let model_hash = crate::hashing::model_hash(&run_config.model_ir_json);
                let data_hashes: Vec<(String, String)> = config.data.observations.iter()
                    .map(|(name, path)| {
                        let bytes = std::fs::read(path).unwrap_or_default();
                        let hash = {
                            use sha2::{Sha256, Digest};
                            let result = Sha256::digest(&bytes);
                            hex::encode(&result[..4])
                        };
                        (format!("{} ({})", name, path), hash)
                    })
                    .collect();
                let metadata = provenance::MleMetadata {
                    input_hash: model_hash[..8].to_string(),
                    model_path: config.model.camdl.clone(),
                    model_hash: model_hash.clone(),
                    data_hashes: data_hashes.clone(),
                    seed,
                    stage: stage_name.to_string(),
                    best_chain: chain_results.best_chain,
                    loglik: chain_results.best_loglik,
                    loglik_sd: 0.0,
                    n_particles: *particles,
                    ess_at_mle: None,
                    timestamp: fit_state.timestamp.clone(),
                };
                provenance::write_mle_params(&mle_path, &all_params, &metadata)
                    .unwrap_or_else(|e| eprintln!("warning: {}", e));

                // Write provenance.json
                let starts_from_prov = effective_starts.as_ref().map(|s| {
                    provenance::StartsFromProv {
                        source: s.clone(),
                        source_hash: None,
                    }
                });
                let algo_json = serde_json::json!({
                    "method": "if2",
                    "chains": chains,
                    "particles": particles,
                    "iterations": iterations,
                    "cooling": cooling,
                });
                let stage_prov = provenance::StageProvenance {
                    camdl_version: crate::version::VERSION_SHORT.to_string(),
                    timestamp: fit_state.timestamp.clone(),
                    config_hash: config_hash.clone(),
                    fit_config: fit_path.clone(),
                    stage: stage_name.to_string(),
                    model: config.model.camdl.clone(),
                    model_hash: model_hash.clone(),
                    data_hashes: data_hashes.iter()
                        .map(|(n, h)| (n.clone(), h.clone())).collect(),
                    estimated: config.estimate.keys().cloned().collect(),
                    fixed: fixed_resolved.iter()
                        .map(|(k, v)| (k.clone(), *v)).collect(),
                    algorithm: algo_json,
                    starts_from: starts_from_prov,
                    derived_from: config.provenance.as_ref()
                        .and_then(|p| p.derived_from.clone()),
                    seed,
                    wall_time_seconds: elapsed.as_secs_f64(),
                    best_loglik: Some(chain_results.best_loglik),
                    best_chain: Some(chain_results.best_chain),
                };
                provenance::write_provenance_json(
                    &stage_dir.to_string_lossy(), &stage_prov,
                ).unwrap_or_else(|e| eprintln!("warning: {}", e));

                collector.render_to_stderr();

                eprintln!("\n{} complete in {:.1}s: {}/", stage_name, elapsed.as_secs_f64(), stage_dir.display());
                eprintln!("  best loglik: {:.1} (chain {})", chain_results.best_loglik, chain_results.best_chain + 1);
            }
            Stage::PGAS { chains, particles, sweeps, burn_in, thin, .. } => {
                // Override legacy PGAS config with v2 stage values
                let mut legacy_pgas = legacy.pgas.clone().unwrap_or_default();
                legacy_pgas.chains = Some(*chains);
                legacy_pgas.particles = Some(*particles);
                legacy_pgas.sweeps = Some(*sweeps);
                if let Some(b) = burn_in { legacy_pgas.burn_in = Some(*b); }
                if let Some(t) = thin { legacy_pgas.thin = Some(*t); }
                legacy_pgas.starts_from = effective_starts.clone();
                let mut legacy_with_pgas = legacy.clone();
                legacy_with_pgas.pgas = Some(legacy_pgas);
                // Override output_dir to place output in the stage subdir
                legacy_with_pgas.fit.output_dir = fit_dir.to_string_lossy().to_string();

                pgas::run_pgas_cli(
                    &legacy_with_pgas,
                    effective_starts.as_deref(),
                    seed, _force, true, true, false,
                ).unwrap_or_else(|e| {
                    eprintln!("error running pgas stage '{}': {}", stage_name, e);
                    std::process::exit(1);
                });

                // Rename pgas/ → {stage_name}/ if they differ
                let pgas_dir = fit_dir.join("pgas");
                if stage_name != &"pgas" && pgas_dir.exists() {
                    std::fs::rename(&pgas_dir, &stage_dir).unwrap_or_else(|e| {
                        eprintln!("warning: could not rename pgas/ to {}: {}", stage_name, e);
                    });
                }
            }
            Stage::PMMH { chains, particles, iterations, burn_in, thin, .. } => {
                let mut legacy_pmmh = legacy.pmmh.clone().unwrap_or_default();
                legacy_pmmh.chains = Some(*chains);
                legacy_pmmh.particles = Some(*particles);
                legacy_pmmh.steps = Some(*iterations);
                if let Some(b) = burn_in { legacy_pmmh.burn_in = Some(*b); }
                if let Some(t) = thin { legacy_pmmh.thin = Some(*t); }
                let mut legacy_with_pmmh = legacy.clone();
                legacy_with_pmmh.pmmh = Some(legacy_pmmh);
                legacy_with_pmmh.fit.output_dir = fit_dir.to_string_lossy().to_string();

                pmmh::run_pmmh_cli(
                    &legacy_with_pmmh,
                    effective_starts.as_deref(),
                    seed, _force, false, false,
                ).unwrap_or_else(|e| {
                    eprintln!("error running pmmh stage '{}': {}", stage_name, e);
                    std::process::exit(1);
                });

                let pmmh_dir = fit_dir.join("pmmh");
                if stage_name != &"pmmh" && pmmh_dir.exists() {
                    std::fs::rename(&pmmh_dir, &stage_dir).unwrap_or_else(|e| {
                        eprintln!("warning: could not rename pmmh/ to {}: {}", stage_name, e);
                    });
                }
            }
            Stage::PFilter { particles, replicates, .. } => {
                // PFilter at fixed params — run a bootstrap particle filter
                // This is what validate's "pfilter at MLE" does
                eprintln!("  PFilter evaluation: {} particles, {} replicates",
                    particles, replicates.unwrap_or(1));
                eprintln!("  (pfilter dispatch not yet implemented — use camdl pfilter directly)");
            }
        }
    }
}

fn parse_fit_args(args: &[String], _needs_starts_from: bool) -> (FitToml, u64, bool) {
    let mut fit_path: Option<String> = None;
    let mut seed = 1_u64;
    let mut force = false;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--seed" => { i += 1; seed = args[i].parse().expect("--seed needs integer"); }
            "--force" => { force = true; }
            "--starts-from" => { i += 1; } // consumed by parse_starts_from / parse_optional_starts_from
            "--check-variance" => {} // consumed by cmd_fit_pmmh
            "--no-nuts" => {} // consumed by cmd_fit_pgas
            "--diagonal-mass" => {} // consumed by cmd_fit_pgas
            "--resume" => {} // consumed by cmd_fit_pgas / cmd_fit_pmmh
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                std::process::exit(1);
            }
            path => { fit_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let fit_path = fit_path.unwrap_or_else(|| {
        eprintln!("usage: camdl fit <scout|refine|validate|pmmh|pgas|status> FIT.toml");
        std::process::exit(1);
    });

    let fit = FitToml::load(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    // Seed priority: CLI --seed > fit.toml seed > random from entropy
    let seed = if args.iter().any(|a| a == "--seed") {
        seed
    } else if let Some(s) = fit.fit.seed {
        s
    } else {
        use std::time::SystemTime;
        let dur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
        dur.as_nanos() as u64 % 1_000_000
    };

    (fit, seed, force)
}

fn parse_optional_starts_from(args: &[String]) -> Option<String> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--starts-from" {
            return Some(args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("--starts-from requires a directory path");
                std::process::exit(1);
            }));
        }
    }
    None
}

fn parse_starts_from(args: &[String]) -> String {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--starts-from" {
            return args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("--starts-from requires a directory path");
                std::process::exit(1);
            });
        }
    }
    eprintln!("error: --starts-from required for refine/validate");
    eprintln!("  usage: camdl fit refine fit.toml --starts-from scout/");
    std::process::exit(1);
}

fn load_model_for_validation(fit: &FitToml) -> (ir::Model, String) {
    crate::util::load_model(&fit.fit.model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    })
}
