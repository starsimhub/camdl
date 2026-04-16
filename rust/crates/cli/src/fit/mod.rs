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
#[allow(dead_code)]
pub mod scout;
#[allow(dead_code)]
pub mod refine;
#[allow(dead_code)]
pub mod validate;
pub mod status;
pub mod pmmh;
pub mod pgas;
pub mod trace_writer;

use config::FitToml;

// Old per-stage CLI entry points. Superseded by cmd_fit_run_v2 but kept
// because the legacy bridge and internal runners still reference FitToml.
#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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
    if let Some(path) = args.first() {
        if !path.starts_with("--") {
            let p = std::path::Path::new(path);
            // Directory → walk it directly
            if p.is_dir() {
                run_status_v2_dir(path);
                return;
            }
            // Try v2 config format
            match config_v2::FitConfigV2::load(path) {
                Ok(config) => {
                    let fit_dir = config.fit_dir(path);
                    if fit_dir.exists() {
                        run_status_v2_dir(&fit_dir.to_string_lossy());
                    } else {
                        eprintln!("no results found at {}", fit_dir.display());
                    }
                    return;
                }
                Err(e) => {
                    // Check if it has [stages] (v2 marker) — if so, the error is real
                    if let Ok(contents) = std::fs::read_to_string(path) {
                        if contents.contains("[stages.") || contents.contains("[stages]") {
                            eprintln!("error parsing v2 fit.toml: {}", e);
                            std::process::exit(1);
                        }
                    }
                    // Otherwise fall through to v1
                }
            }
        }
    }
    // Fall back to v1
    let (fit, _, _) = parse_fit_args(args, false);
    status::run_status(&fit).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

/// Walk a results directory and report status of all stages found.
fn run_status_v2_dir(dir: &str) {
    let path = std::path::Path::new(dir);
    if !path.exists() {
        eprintln!("no results at {}", dir);
        return;
    }

    println!("{}/", dir);

    // Check for sweep subdirectories (contain stage dirs)
    // or direct stage dirs
    let mut found_stages = false;
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .unwrap_or_else(|e| { eprintln!("cannot read {}: {}", dir, e); std::process::exit(1); })
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = entry.path();

        if entry_path.is_dir() {
            // Check if this is a stage dir (has fit_state.toml or provenance.json)
            let has_fit_state = entry_path.join("fit_state.toml").exists();
            let has_provenance = entry_path.join("provenance.json").exists();

            if has_fit_state || has_provenance {
                // Direct stage
                print_stage_status(&name, &entry_path.to_string_lossy());
                found_stages = true;
            } else {
                // Might be a sweep point dir — check children
                let mut child_entries: Vec<_> = std::fs::read_dir(&entry_path)
                    .into_iter().flatten().flatten().collect();
                child_entries.sort_by_key(|e| e.file_name());
                let has_child_stages = child_entries.iter().any(|c| {
                    c.path().join("fit_state.toml").exists() || c.path().join("provenance.json").exists()
                });
                if has_child_stages {
                    println!("\n  \x1b[1m{}/\x1b[0m", name);
                    for child in &child_entries {
                        let child_name = child.file_name().to_string_lossy().to_string();
                        if child.path().is_dir() {
                            let child_has = child.path().join("fit_state.toml").exists()
                                || child.path().join("provenance.json").exists();
                            if child_has {
                                print_stage_status(&child_name, &child.path().to_string_lossy());
                            }
                        }
                    }
                    found_stages = true;
                }
            }
        }
    }

    if !found_stages {
        println!("  (no completed stages found)");
    }
}

fn print_stage_status(name: &str, stage_dir: &str) {
    use crate::fit::state::FitState;
    use crate::fit::provenance;

    // Try provenance.json first (v2), then fit_state.toml (v1)
    if let Ok(prov) = provenance::read_provenance_json(stage_dir) {
        let ll = prov.best_loglik.map(|l| format!("{:.1}", l)).unwrap_or_else(|| "—".into());
        let chain = prov.best_chain.map(|c| format!(" (chain {})", c + 1)).unwrap_or_default();
        let method = &prov.algorithm.get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        println!("    {:12} \x1b[32m✓\x1b[0m {} — loglik={}{}, {:.0}s",
            name, method, ll, chain, prov.wall_time_seconds);
    } else if let Ok(state) = FitState::load(stage_dir) {
        let n_good = state.n_good_chains.unwrap_or(state.n_chains);
        println!("    {:12} \x1b[32m✓\x1b[0m {} chains, loglik={:.1}, {}/{} good",
            name, state.n_chains, state.best_loglik, n_good, state.n_chains);
    }
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
    let mut sweep_args: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => { i += 1; seed = args[i].parse().expect("--seed needs integer"); has_seed_flag = true; }
            "--force" => { _force = true; }
            "--stage" => { i += 1; stage_filter = Some(args[i].clone()); }
            "--starts-from" => { i += 1; starts_from_override = Some(args[i].clone()); }
            "--sweep" => { i += 1; sweep_args.push(args[i].clone()); }
            "--resume" => {
                eprintln!("error: --resume is not yet implemented for `camdl fit run`.");
                eprintln!("  Use the legacy `camdl fit pgas` or `camdl fit pmmh` with --resume.");
                std::process::exit(1);
            }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                eprintln!("usage: camdl fit run FIT.toml [--stage NAME] [--seed N] [--force] [--sweep \"NAME=V1,V2,...\"]");
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

    // ── Parse and validate sweeps ─────────────────────────────────────────
    let sweep_specs: Vec<(String, Vec<f64>)> = sweep_args.iter().map(|arg| {
        let mut parts = arg.splitn(2, '=');
        let name = parts.next().unwrap().trim().to_string();
        let values_str = parts.next().unwrap_or_else(|| {
            eprintln!("error: --sweep requires NAME=V1,V2,...");
            std::process::exit(1);
        });
        let values: Vec<f64> = values_str.split(',')
            .map(|s| s.trim().parse().unwrap_or_else(|_| {
                eprintln!("error: cannot parse sweep value '{}' for '{}'", s.trim(), name);
                std::process::exit(1);
            }))
            .collect();
        (name, values)
    }).collect();

    // Validate: swept params must be in [fixed], not [estimate]
    let fixed_resolved = config.fixed.resolve().unwrap_or_default();
    for (name, _) in &sweep_specs {
        if config.estimate.contains_key(name) {
            eprintln!("error: cannot sweep '{}' — it is in [estimate].\n  \
                       Sweeps override [fixed] parameters. Move '{}' to [fixed] first.",
                name, name);
            std::process::exit(1);
        }
        if !fixed_resolved.contains_key(name) {
            eprintln!("error: sweep parameter '{}' not found in [fixed].\n  \
                       Available fixed params: {}",
                name, fixed_resolved.keys().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
            std::process::exit(1);
        }
    }

    // Expand Cartesian product of sweep points
    let sweep_points: Vec<Vec<(String, f64)>> = if sweep_specs.is_empty() {
        vec![vec![]]
    } else {
        let mut points: Vec<Vec<(String, f64)>> = vec![vec![]];
        for (name, values) in &sweep_specs {
            let mut next = Vec::new();
            for pt in &points {
                for &v in values {
                    let mut new_pt = pt.clone();
                    new_pt.push((name.clone(), v));
                    next.push(new_pt);
                }
            }
            points = next;
        }
        points
    };
    let has_sweep = sweep_points.len() > 1;
    if has_sweep {
        eprintln!("sweep: {} points", sweep_points.len());
    }

    // Validate --starts-from requires --stage
    if starts_from_override.is_some() && stage_filter.is_none() {
        eprintln!("error: --starts-from requires --stage to disambiguate which stage it applies to.");
        std::process::exit(1);
    }

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

    // Seed: CLI --seed > default (1). Deterministic by default for reproducibility.
    let seed = if has_seed_flag { seed } else { 1 };

    // Execute stages: sweep_point × stage
    for (pt_idx, sweep_point) in sweep_points.iter().enumerate() {
        // Build a config with swept values applied to [fixed]
        let mut sweep_config = config.clone();
        for (name, val) in sweep_point {
            sweep_config.fixed.values.insert(name.clone(), *val);
        }

        // Recalculate the legacy bridge with swept values
        let sweep_legacy = sweep_config.to_legacy_toml().unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });

        // Base directory for this sweep point
        let sweep_fit_dir = if has_sweep {
            let slug: String = sweep_point.iter()
                .map(|(k, v)| format!("{}_{:.3}", k, v))
                .collect::<Vec<_>>()
                .join("__");
            if pt_idx == 0 {
                eprintln!("");
            }
            eprintln!("═══ sweep point {}/{}: {} ═══", pt_idx + 1, sweep_points.len(), slug);
            fit_dir.join(slug)
        } else {
            fit_dir.clone()
        };

    for (stage_name, stage) in &stages_to_run {
        let stage_dir = sweep_fit_dir.join(stage_name);
        eprintln!("\n── stage: {} (method={}) ──", stage_name, stage.method_name());

        // Config hash staleness check
        let fixed_resolved = sweep_config.fixed.resolve().unwrap_or_default();
        let config_hash = provenance::compute_config_hash_v2(
            &model_json, &sweep_config.data.observations, &sweep_config.estimate,
            &fixed_resolved, stage_name, stage, seed,
        ).unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });
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
                    Some(sweep_fit_dir.join(dep_name).to_string_lossy().to_string())
                }
                StartsFrom::Directory(ref path) => {
                    Some(path.to_string_lossy().to_string())
                }
            }
        };

        let stage_t0 = std::time::Instant::now();
        let mut stage_best_loglik: Option<f64> = None;
        let mut stage_best_chain: Option<usize> = None;

        match stage {
            Stage::IF2 { chains, particles, iterations, cooling, .. } => {
                let prior_state = effective_starts.as_ref().and_then(|dir| {
                    state::FitState::load(dir).ok()
                });

                let run_config = runner::FitRunConfig::build(
                    &sweep_legacy,
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
                let data_hashes: Vec<(String, String)> = sweep_config.data.observations.iter()
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
                    model_path: sweep_config.model.camdl.clone(),
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

                collector.render_to_stderr();

                stage_best_loglik = Some(chain_results.best_loglik);
                stage_best_chain = Some(chain_results.best_chain);

                eprintln!("\n{} complete in {:.1}s: {}/", stage_name, elapsed.as_secs_f64(), stage_dir.display());
                eprintln!("  best loglik: {:.1} (chain {})", chain_results.best_loglik, chain_results.best_chain + 1);
            }
            Stage::PGAS { chains, particles, sweeps, burn_in, thin, .. } => {
                // Override legacy PGAS config with v2 stage values
                let mut legacy_pgas = sweep_legacy.pgas.clone().unwrap_or_default();
                legacy_pgas.chains = Some(*chains);
                legacy_pgas.particles = Some(*particles);
                legacy_pgas.sweeps = Some(*sweeps);
                if let Some(b) = burn_in { legacy_pgas.burn_in = Some(*b); }
                if let Some(t) = thin { legacy_pgas.thin = Some(*t); }
                legacy_pgas.starts_from = effective_starts.clone();
                let mut legacy_with_pgas = sweep_legacy.clone();
                legacy_with_pgas.pgas = Some(legacy_pgas);
                legacy_with_pgas.fit.output_dir = sweep_fit_dir.to_string_lossy().to_string();

                pgas::run_pgas_cli(
                    &legacy_with_pgas,
                    effective_starts.as_deref(),
                    seed, _force, true, true, false,
                ).unwrap_or_else(|e| {
                    eprintln!("error running pgas stage '{}': {}", stage_name, e);
                    std::process::exit(1);
                });

                // Rename pgas/ → {stage_name}/ if they differ
                // PGAS runner writes to {output_dir}/pgas/. If our stage has a
                // different name, move the output to the correct stage directory.
                let pgas_dir = sweep_fit_dir.join("pgas");
                if stage_name != &"pgas" && pgas_dir.exists() {
                    // Remove target if it exists (stale results from a previous run)
                    if stage_dir.exists() {
                        let _ = std::fs::remove_dir_all(&stage_dir);
                    }
                    std::fs::rename(&pgas_dir, &stage_dir).unwrap_or_else(|e| {
                        eprintln!("error: could not rename pgas/ to {}: {}", stage_name, e);
                        std::process::exit(1);
                    });
                }
            }
            Stage::PMMH { chains, particles, iterations, burn_in, thin, .. } => {
                let mut legacy_pmmh = sweep_legacy.pmmh.clone().unwrap_or_default();
                legacy_pmmh.chains = Some(*chains);
                legacy_pmmh.particles = Some(*particles);
                legacy_pmmh.steps = Some(*iterations);
                if let Some(b) = burn_in { legacy_pmmh.burn_in = Some(*b); }
                if let Some(t) = thin { legacy_pmmh.thin = Some(*t); }
                let mut legacy_with_pmmh = sweep_legacy.clone();
                legacy_with_pmmh.pmmh = Some(legacy_pmmh);
                legacy_with_pmmh.fit.output_dir = sweep_fit_dir.to_string_lossy().to_string();

                pmmh::run_pmmh_cli(
                    &legacy_with_pmmh,
                    effective_starts.as_deref(),
                    seed, _force, false, false,
                ).unwrap_or_else(|e| {
                    eprintln!("error running pmmh stage '{}': {}", stage_name, e);
                    std::process::exit(1);
                });

                let pmmh_dir = sweep_fit_dir.join("pmmh");
                if stage_name != &"pmmh" && pmmh_dir.exists() {
                    if stage_dir.exists() {
                        let _ = std::fs::remove_dir_all(&stage_dir);
                    }
                    std::fs::rename(&pmmh_dir, &stage_dir).unwrap_or_else(|e| {
                        eprintln!("error: could not rename pmmh/ to {}: {}", stage_name, e);
                        std::process::exit(1);
                    });
                }
            }
            Stage::PFilter { particles, replicates, .. } => {
                let n_reps = replicates.unwrap_or(1);
                let prior_state = effective_starts.as_ref().and_then(|dir| {
                    state::FitState::load(dir).ok()
                });
                if prior_state.is_none() && !effective_starts.as_ref().map_or(true, |s| s.is_empty()) {
                    eprintln!("warning: could not load fit_state from starts_from");
                }

                // Build run config (reuse IF2 builder with 1 chain, N particles)
                let run_config = runner::FitRunConfig::build(
                    &sweep_legacy,
                    prior_state.as_ref(),
                    1, *particles, 1, 1.0, seed, false,
                ).unwrap_or_else(|e| {
                    eprintln!("error building pfilter config: {}", e);
                    std::process::exit(1);
                });

                std::fs::create_dir_all(&stage_dir).unwrap_or_else(|e| {
                    eprintln!("error creating {}: {}", stage_dir.display(), e);
                    std::process::exit(1);
                });

                // Run PF at MLE params
                let mle_params = run_config.base_params.clone();
                let t0 = std::time::Instant::now();

                let mut logliks = Vec::new();
                for r in 0..n_reps {
                    let pf_seed = seed ^ ((r as u64).wrapping_mul(0x7f4a7c15_u64));
                    let process = run_config.build_process();
                    let obs_model = run_config.build_obs_model();
                    let smc_config = run_config.smc_config();
                    let result = sim::inference::bootstrap_filter(
                        &process, &obs_model, &mle_params, &smc_config, pf_seed,
                    ).unwrap_or_else(|e| {
                        eprintln!("pfilter error: {:?}", e);
                        std::process::exit(1);
                    });
                    logliks.push(result.log_likelihood);
                    if n_reps <= 10 || r % (n_reps / 10) == 0 {
                        eprintln!("  pfilter rep {}/{}: loglik={:.1}", r + 1, n_reps, result.log_likelihood);
                    }
                }
                let elapsed = t0.elapsed();

                let mean_ll = logliks.iter().sum::<f64>() / logliks.len() as f64;
                let sd_ll = if logliks.len() > 1 {
                    let var = logliks.iter().map(|l| (l - mean_ll).powi(2)).sum::<f64>() / (logliks.len() - 1) as f64;
                    var.sqrt()
                } else { 0.0 };

                eprintln!("\n  loglik = {:.1} ± {:.1} ({} reps, {} particles, {:.1}s)",
                    mean_ll, sd_ll, n_reps, particles, elapsed.as_secs_f64());

                // Write logliks.tsv
                {
                    use std::io::Write;
                    let path = format!("{}/logliks.tsv", stage_dir.display());
                    let mut f = std::fs::File::create(&path).unwrap();
                    writeln!(f, "replicate\tloglik").unwrap();
                    for (i, ll) in logliks.iter().enumerate() {
                        writeln!(f, "{}\t{:.4}", i + 1, ll).unwrap();
                    }
                }
            }
        }

        // ── Shared provenance write (all stage types) ───────────────────────
        let stage_elapsed = stage_t0.elapsed();
        let starts_from_prov = effective_starts.as_ref().map(|s| {
            provenance::StartsFromProv {
                source: s.clone(),
                source_hash: None,
            }
        });
        let algo_json = match stage {
            Stage::IF2 { chains, particles, iterations, cooling, .. } =>
                serde_json::json!({ "method": "if2", "chains": chains, "particles": particles, "iterations": iterations, "cooling": cooling }),
            Stage::PGAS { chains, particles, sweeps, .. } =>
                serde_json::json!({ "method": "pgas", "chains": chains, "particles": particles, "sweeps": sweeps }),
            Stage::PMMH { chains, particles, iterations, .. } =>
                serde_json::json!({ "method": "pmmh", "chains": chains, "particles": particles, "iterations": iterations }),
            Stage::PFilter { particles, replicates, .. } =>
                serde_json::json!({ "method": "pfilter", "particles": particles, "replicates": replicates }),
        };
        let stage_prov = provenance::StageProvenance {
            camdl_version: crate::version::VERSION_SHORT.to_string(),
            timestamp: scout::now_iso8601_pub(),
            config_hash: config_hash.clone(),
            fit_config: fit_path.clone(),
            stage: stage_name.to_string(),
            model: sweep_config.model.camdl.clone(),
            model_hash: crate::hashing::model_hash(&model_json),
            data_hashes: sweep_config.data.observations.iter()
                .map(|(name, path)| {
                    let hash = provenance::file_content_hash(path).unwrap_or_default();
                    (name.clone(), hash)
                }).collect(),
            estimated: sweep_config.estimate.keys().cloned().collect(),
            fixed: fixed_resolved.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            algorithm: algo_json,
            starts_from: starts_from_prov,
            derived_from: sweep_config.provenance.as_ref()
                .and_then(|p| p.derived_from.clone()),
            seed,
            wall_time_seconds: stage_elapsed.as_secs_f64(),
            best_loglik: stage_best_loglik,
            best_chain: stage_best_chain,
        };
        provenance::write_provenance_json(
            &stage_dir.to_string_lossy(), &stage_prov,
        ).unwrap_or_else(|e| eprintln!("warning: could not write provenance.json: {}", e));

    } // end stages
    } // end sweep_points
}

// ─── camdl fit diff ─────────────────────────────────────────────────────────

pub fn cmd_fit_diff(args: &[String]) {
    use config_v2::FitConfigV2;

    if args.len() < 2 {
        eprintln!("usage: camdl fit diff A.toml B.toml");
        std::process::exit(1);
    }
    let a = FitConfigV2::load(&args[0]).unwrap_or_else(|e| {
        eprintln!("error loading {}: {}", args[0], e);
        std::process::exit(1);
    });
    let b = FitConfigV2::load(&args[1]).unwrap_or_else(|e| {
        eprintln!("error loading {}: {}", args[1], e);
        std::process::exit(1);
    });

    println!("diff: {} → {}", args[0], args[1]);
    println!();

    // Parameter changes
    let a_est: std::collections::BTreeSet<&str> = a.estimate.keys().map(|s| s.as_str()).collect();
    let b_est: std::collections::BTreeSet<&str> = b.estimate.keys().map(|s| s.as_str()).collect();
    let a_fixed = a.fixed.resolve().unwrap_or_default();
    let b_fixed = b.fixed.resolve().unwrap_or_default();
    let a_fix_keys: std::collections::BTreeSet<&str> = a_fixed.keys().map(|s| s.as_str()).collect();
    let b_fix_keys: std::collections::BTreeSet<&str> = b_fixed.keys().map(|s| s.as_str()).collect();

    let mut param_changes = false;
    // Moved from estimate → fixed
    for name in a_est.difference(&b_est) {
        if b_fix_keys.contains(name) {
            println!("  {}: [estimate] → [fixed] = {}", name, b_fixed.get(*name).unwrap());
            param_changes = true;
        }
    }
    // Moved from fixed → estimate
    for name in b_est.difference(&a_est) {
        if a_fix_keys.contains(name) {
            println!("  {}: [fixed] = {} → [estimate]", name, a_fixed.get(*name).unwrap());
            param_changes = true;
        }
    }
    // Fixed value changed
    for name in a_fix_keys.intersection(&b_fix_keys) {
        let va = a_fixed.get(*name).unwrap();
        let vb = b_fixed.get(*name).unwrap();
        if (va - vb).abs() > 1e-15 {
            println!("  {}: [fixed] {} → {}", name, va, vb);
            param_changes = true;
        }
    }
    // Bounds changed
    for name in a_est.intersection(&b_est) {
        let ab = a.estimate[*name].bounds;
        let bb = b.estimate[*name].bounds;
        if (ab.0 - bb.0).abs() > 1e-15 || (ab.1 - bb.1).abs() > 1e-15 {
            println!("  {}: bounds [{}, {}] → [{}, {}]", name, ab.0, ab.1, bb.0, bb.1);
            param_changes = true;
        }
    }
    if !param_changes {
        println!("  (no parameter changes)");
    }

    // Prior changes
    let mut _prior_changes = false;
    for name in a_est.intersection(&b_est) {
        let ap = &a.estimate[*name].prior;
        let bp = &b.estimate[*name].prior;
        let ap_str = format!("{:?}", ap);
        let bp_str = format!("{:?}", bp);
        if ap_str != bp_str {
            println!("  {}: prior {:?} → {:?}", name, ap, bp);
            _prior_changes = true;
        }
    }

    // Stage changes
    println!();
    println!("Stages:");
    let a_stages: std::collections::BTreeSet<&str> = a.stages.keys().map(|s| s.as_str()).collect();
    let b_stages: std::collections::BTreeSet<&str> = b.stages.keys().map(|s| s.as_str()).collect();
    let mut stage_changes = false;
    for name in b_stages.difference(&a_stages) {
        let s = &b.stages[*name];
        println!("  stage '{}': (new) {}", name, s.method_name());
        stage_changes = true;
    }
    for name in a_stages.difference(&b_stages) {
        println!("  stage '{}': (removed)", name);
        stage_changes = true;
    }
    for name in a_stages.intersection(&b_stages) {
        let sa = &a.stages[*name];
        let sb = &b.stages[*name];
        let sa_json = serde_json::to_string(sa).unwrap_or_default();
        let sb_json = serde_json::to_string(sb).unwrap_or_default();
        if sa_json != sb_json {
            // Show detailed changes
            let mut details = Vec::new();
            if sa.method_name() != sb.method_name() {
                details.push(format!("method {}→{}", sa.method_name(), sb.method_name()));
            }
            if sa.chains() != sb.chains() {
                details.push(format!("chains {}→{}", sa.chains(), sb.chains()));
            }
            // Compare serialized for catch-all
            if details.is_empty() {
                details.push("settings changed".to_string());
            }
            println!("  stage '{}': {}", name, details.join(", "));
            stage_changes = true;
        }
    }
    if !stage_changes {
        println!("  (no stage changes)");
    }
}

// ─── camdl fit new ──────────────────────────────────────────────────────────

pub fn cmd_fit_new(args: &[String]) {
    let mut from_path: Option<String> = None;
    let mut to_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" => { i += 1; from_path = Some(args[i].clone()); }
            s if !s.starts_with("--") => {
                if from_path.is_none() {
                    from_path = Some(s.to_string());
                } else {
                    to_path = Some(s.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    let from = from_path.unwrap_or_else(|| {
        eprintln!("usage: camdl fit new --from SOURCE.toml DEST.toml");
        std::process::exit(1);
    });
    let to = to_path.unwrap_or_else(|| {
        eprintln!("usage: camdl fit new --from SOURCE.toml DEST.toml");
        std::process::exit(1);
    });

    if std::path::Path::new(&to).exists() {
        eprintln!("error: {} already exists. Choose a different name.", to);
        std::process::exit(1);
    }

    // Read source, inject provenance
    let mut content = std::fs::read_to_string(&from).unwrap_or_else(|e| {
        eprintln!("error reading {}: {}", from, e);
        std::process::exit(1);
    });

    // Check if [provenance] already exists
    if !content.contains("[provenance]") {
        // Add provenance block at the top, after the first blank line or at start
        let prov_block = format!(
            "[provenance]\nderived_from = \"{}\"\nreason = \"\"\n\n",
            from
        );
        // Insert after any leading comments
        if let Some(pos) = content.find("\n[") {
            content.insert_str(pos + 1, &prov_block);
        } else {
            content = format!("{}{}", prov_block, content);
        }
    } else {
        // Update existing provenance
        // Simple approach: just warn
        eprintln!("note: {} already has [provenance]. Update derived_from manually.", to);
    }

    // Find the first stage and update starts_from to point to source's results
    let source_config = config_v2::FitConfigV2::load(&from).ok();
    if let Some(ref cfg) = source_config {
        let source_fit_dir = cfg.fit_dir(&from);
        if let Some(last_stage) = cfg.stages.keys().last() {
            let starts_path = source_fit_dir.join(last_stage);
            if starts_path.exists() {
                eprintln!("  [provenance] derived_from = \"{}\"", from);
                eprintln!("  hint: set starts_from = \"{}\" on your first stage",
                    starts_path.display());
            }
        }
    }

    std::fs::write(&to, &content).unwrap_or_else(|e| {
        eprintln!("error writing {}: {}", to, e);
        std::process::exit(1);
    });

    eprintln!("created {}", to);
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn load_model_for_validation(fit: &FitToml) -> (ir::Model, String) {
    crate::util::load_model(&fit.fit.model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    })
}
