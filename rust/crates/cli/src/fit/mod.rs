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
pub mod synthetic;
pub mod summary;
pub mod gating;

use config::FitToml;

// Old per-stage CLI entry points. Superseded by cmd_fit_run_v2 but kept
// because the legacy bridge and internal runners still reference FitToml.
#[allow(dead_code)]
pub fn cmd_fit_scout(args: &[String]) {
    let (fit, fit_path, seed, force) = parse_fit_args(args, false);

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

    let fit_hash = fit.fit_content_hash(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });
    let (fit, _fit_root) = prepare_v1_cell(&fit, &fit_path, seed);
    let t0 = std::time::Instant::now();
    scout::run_scout(&fit, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let stage_dir = std::path::PathBuf::from(&fit.fit.output_dir).join("scout");
    let (best_ll, best_c) = read_v1_stage_best(&stage_dir);
    let n_chains = fit.scout.as_ref().and_then(|s| s.chains).unwrap_or(4);
    let algo = serde_json::json!({ "method": "if2-scout", "chains": n_chains });
    write_v1_stage_run(&stage_dir, &fit_hash, "scout", "if2", seed,
        n_chains, algo, best_ll, best_c, t0.elapsed().as_secs_f64(), None);
}

#[allow(dead_code)]
pub fn cmd_fit_refine(args: &[String]) {
    let (fit, fit_path, seed, force) = parse_fit_args(args, true);
    let starts_from = parse_starts_from(args);
    let allow_nonconverged_scout = args.iter().any(|a| a == "--allow-nonconverged-scout");

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    let fit_hash = fit.fit_content_hash(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });
    let (fit, _fit_root) = prepare_v1_cell(&fit, &fit_path, seed);
    let t0 = std::time::Instant::now();
    refine::run_refine(&fit, &starts_from, seed, force, allow_nonconverged_scout)
        .unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });
    let stage_dir = std::path::PathBuf::from(&fit.fit.output_dir).join("refine");
    let (best_ll, best_c) = read_v1_stage_best(&stage_dir);
    let n_chains = fit.refine.as_ref().and_then(|s| s.chains).unwrap_or(4);
    let algo = serde_json::json!({ "method": "if2-refine", "chains": n_chains });
    write_v1_stage_run(&stage_dir, &fit_hash, "refine", "if2", seed,
        n_chains, algo, best_ll, best_c, t0.elapsed().as_secs_f64(),
        Some(&starts_from));
}

#[allow(dead_code)]
pub fn cmd_fit_validate(args: &[String]) {
    let (fit, fit_path, seed, force) = parse_fit_args(args, true);
    let starts_from = parse_starts_from(args);

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    let fit_hash = fit.fit_content_hash(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });
    let (fit, _fit_root) = prepare_v1_cell(&fit, &fit_path, seed);
    let t0 = std::time::Instant::now();
    validate::run_validate(&fit, &starts_from, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let stage_dir = std::path::PathBuf::from(&fit.fit.output_dir).join("validate");
    let (best_ll, best_c) = read_v1_stage_best(&stage_dir);
    let n_chains = fit.validate.as_ref().and_then(|s| s.chains).unwrap_or(4);
    let algo = serde_json::json!({ "method": "if2-validate", "chains": n_chains });
    write_v1_stage_run(&stage_dir, &fit_hash, "validate", "if2", seed,
        n_chains, algo, best_ll, best_c, t0.elapsed().as_secs_f64(),
        Some(&starts_from));
}

#[allow(dead_code)]
pub fn cmd_fit_pmmh(args: &[String]) {
    let (fit, fit_path, seed, force) = parse_fit_args(args, false);
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

    let fit_hash = fit.fit_content_hash(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });
    let (fit, _fit_root) = prepare_v1_cell(&fit, &fit_path, seed);
    let t0 = std::time::Instant::now();
    pmmh::run_pmmh_cli(&fit, starts_from.as_deref(), seed, force, check_variance, resume).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let stage_dir = std::path::PathBuf::from(&fit.fit.output_dir).join("pmmh");
    let n_chains = fit.pmmh.as_ref().and_then(|s| s.chains).unwrap_or(4);
    let algo = serde_json::json!({ "method": "pmmh", "chains": n_chains });
    write_v1_stage_run(&stage_dir, &fit_hash, "pmmh", "pmmh", seed,
        n_chains, algo, None, None, t0.elapsed().as_secs_f64(),
        starts_from.as_deref());
}

#[allow(dead_code)]
pub fn cmd_fit_pgas(args: &[String]) {
    let (fit, fit_path, seed, force) = parse_fit_args(args, false);
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

    let fit_hash = fit.fit_content_hash(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });
    let (fit, _fit_root) = prepare_v1_cell(&fit, &fit_path, seed);
    let t0 = std::time::Instant::now();
    pgas::run_pgas_cli(&fit, starts_from.as_deref(), seed, force, !no_nuts, !diagonal_mass, resume).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let stage_dir = std::path::PathBuf::from(&fit.fit.output_dir).join("pgas");
    let n_chains = fit.pgas.as_ref().and_then(|s| s.chains).unwrap_or(4);
    let algo = serde_json::json!({ "method": "pgas", "chains": n_chains });
    write_v1_stage_run(&stage_dir, &fit_hash, "pgas", "pgas", seed,
        n_chains, algo, None, None, t0.elapsed().as_secs_f64(),
        starts_from.as_deref());
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
                    match config.fit_dir(path) {
                        Ok(fit_dir) if fit_dir.exists() => {
                            run_status_v2_dir(&fit_dir.to_string_lossy());
                        }
                        Ok(fit_dir) => {
                            eprintln!("no results found at {}", fit_dir.display());
                        }
                        Err(e) => {
                            eprintln!("error computing fit directory: {}", e);
                            std::process::exit(1);
                        }
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
    // Fall back to v1: treat the fit.toml as a v1 FitToml and walk the
    // computed cell dir (same shape as v2). Legacy v1 status expected
    // `fit.fit.output_dir` to be the stage-container dir directly; now
    // it's a root, so we route through `cell_dir` to land at
    // `<root>/fits/<stem>-<hash>/real/fit_<seed>/`.
    let (fit, fit_path, seed, _) = parse_fit_args(args, false);
    let cell = fit.cell_dir(&fit_path, seed).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });
    if !cell.exists() {
        eprintln!("no results at {}", cell.display());
        return;
    }
    run_status_v2_dir(&cell.to_string_lossy());
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
            // Check if this is a stage dir (has fit_state.toml or run.json)
            let has_fit_state = entry_path.join("fit_state.toml").exists();
            let has_run_json  = entry_path.join("run.json").exists();

            if has_fit_state || has_run_json {
                // Direct stage
                print_stage_status(&name, &entry_path.to_string_lossy());
                found_stages = true;
            } else {
                // Might be a sweep point dir — check children
                let mut child_entries: Vec<_> = std::fs::read_dir(&entry_path)
                    .into_iter().flatten().flatten().collect();
                child_entries.sort_by_key(|e| e.file_name());
                let has_child_stages = child_entries.iter().any(|c| {
                    c.path().join("fit_state.toml").exists() || c.path().join("run.json").exists()
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
    use crate::run_meta::{Run, RunKind};

    // A completed v2 stage always has a FitStage run.json. The
    // fit_state.toml path (checked by the caller's directory walk)
    // is written earlier in the stage, so a dir with fit_state.toml
    // but no run.json is an interrupted run.
    match Run::read(std::path::Path::new(stage_dir)) {
        Ok(run) => {
            if let RunKind::FitStage(m) = &run.kind {
                let ll    = m.best_loglik.map(|l| format!("{:.1}", l)).unwrap_or_else(|| "—".into());
                let chain = m.best_chain.map(|c| format!(" (chain {})", c + 1)).unwrap_or_default();
                println!("    {:12} \x1b[32m✓\x1b[0m {} — loglik={}{}, {:.0}s",
                    name, m.method, ll, chain, run.wall_time_seconds);
            }
        }
        Err(_) => {
            println!("    {:12} \x1b[33m⚠\x1b[0m incomplete (no run.json)", name);
        }
    }
}

// ─── New `camdl fit run` entry point (config_v2) ────────────────────────────

pub fn cmd_fit_run_v2(args: &[String]) {
    use config_v2::{FitConfigV2, Stage, StartsFrom};

    let mut fit_path: Option<String> = None;
    let mut seed = 1_u64;
    let mut force = false;
    let mut stage_filter: Option<String> = None;
    let mut starts_from_override: Option<String> = None;
    let mut has_seed_flag = false;
    let mut sweep_args: Vec<String> = Vec::new();
    let mut allow_nonconverged_scout = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => { i += 1; seed = args[i].parse().expect("--seed needs integer"); has_seed_flag = true; }
            "--force" => { force = true; }
            "--stage" => { i += 1; stage_filter = Some(args[i].clone()); }
            "--starts-from" => { i += 1; starts_from_override = Some(args[i].clone()); }
            "--sweep" => { i += 1; sweep_args.push(args[i].clone()); }
            "--allow-nonconverged-scout" => { allow_nonconverged_scout = true; }
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
    if let Some(msg) = config.dangling_priors_warning() {
        // Warning, not error: a staged Bayesian workflow (scout → pgas)
        // legitimately declares priors that the IF2 stage ignores — so
        // we can't refuse here. But silent would hide the copy-paste /
        // mental-model-mismatch class of bug that's the actual risk.
        eprintln!("\x1b[33mwarning:\x1b[0m {}", msg);
    }

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

    let fit_dir = config.fit_dir(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    // ── Build + write top-level run.json at the fit root ──────────────
    //
    // Per-stage run.json records live inside each stage dir; this one
    // describes the fit as a whole so `camdl list` / `camdl show` can
    // surface fits alongside simulate runs. `Run.hash` is the seed-
    // independent content hash (same suffix used in the directory name).
    //
    // We write once here (so the fit is listable even if interrupted)
    // and rewrite once at end-of-fit to capture `wall_time_seconds`.
    // The parent fit hash is also reused by every stage to populate
    // `FitStageMeta.fit_hash` — computing it once here avoids the O(stages
    // × full-I/O rehash) pattern.
    let fit_start = std::time::Instant::now();
    let mut run_fit = build_fit_run(&config, &fit_path);
    let parent_fit_hash = run_fit.hash.clone();
    if let Err(e) = run_fit.write(&fit_dir) {
        eprintln!("warning: cannot write {}/run.json: {}", fit_dir.display(), e);
    }

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

    // IC-free inference diagnostic: when ic_free = true, make it
    // visible on the startup block so the user can confirm the PF is
    // computing log L_c (conditional on y₁) rather than log L. Silent
    // when ic_free is false or absent. See
    // docs/dev/proposals/2026-04-18-ic-free-inference.md.
    if config.ic_free.unwrap_or(false) {
        let ivp_params: Vec<&str> = config.estimate.iter()
            .filter(|(_, spec)| spec.ivp)
            .map(|(n, _)| n.as_str())
            .collect();
        eprintln!("\n  \x1b[36mic-free inference:\x1b[0m conditioning on y₁");
        eprintln!("    - initial state spread from ivp params: [{}]", ivp_params.join(", "));
        eprintln!("    - log-likelihood accumulation from t = 2 (y₁ reweights and resamples only)");
    }

    // Seed: CLI --seed > default (1). Deterministic by default for reproducibility.
    let base_seed = if has_seed_flag { seed } else { 1 };

    // ── Build the replicate grid: (dataset_idx, fit_seed) cells ──────────
    //
    // Four canonical modes, all routed through the same grid:
    //   Mode                     synthetic?  fit_seeds     Cells
    //   Single fit               no          None/scalar   1       → real/fit_<base>/
    //   Start-sensitivity        no          list of M     M       → real/fit_<s_i>/
    //   SBC (classical)          yes         None/scalar   N       → synthetic/ds_NN/fit_<base>/
    //   SBC × start-sensitivity  yes         list of M     N × M   → synthetic/ds_NN/fit_<s_i>/
    //
    // For synthetic modes the datasets are generated once up front and the
    // per-cell DataSpec is materialised from their on-disk paths. See
    // docs/dev/proposals/2026-04-17-synthetic-fit-replicates.md.
    let fit_seeds: Vec<u64> = match &config.fit_seeds {
        Some(list) => list.clone(),
        None       => vec![base_seed],
    };

    let synthetic_datasets: Vec<synthetic::SyntheticDataset> = if let Some(spec) = &config.synthetic {
        let datasets = synthetic::generate_synthetic_datasets(
            spec,
            &config.model.camdl,
            &fit_dir,
            &config.config.backend,
            config.config.dt,
        ).unwrap_or_else(|e| {
            eprintln!("error: synthetic-data generation failed: {}", e);
            std::process::exit(1);
        });
        eprintln!("synthetic: generated {} dataset{} under {}/synthetic/data/",
            datasets.len(),
            if datasets.len() == 1 { "" } else { "s" },
            fit_dir.display());
        datasets
    } else {
        Vec::new()
    };

    // A cell is one (data_source, fit_seed) pair. Real-data cells carry
    // `dataset_idx = None` and leave the existing `config.data` in place;
    // synthetic cells carry `Some(idx)` and replace `config.data` with a
    // DataSpec pointing at the generated TSV.
    struct Cell {
        dataset_idx: Option<usize>,
        fit_seed: u64,
        // None → keep config.data; Some → overwrite with synthetic path.
        data_override: Option<config_v2::DataSpec>,
    }
    let cells: Vec<Cell> = if synthetic_datasets.is_empty() {
        fit_seeds.iter().map(|&s| Cell {
            dataset_idx: None,
            fit_seed: s,
            data_override: None,
        }).collect()
    } else {
        // Determine the observation stream name(s) for the generated TSVs
        // from the model itself — synthetic generation writes one column
        // per declared observation block, so the fit data map points each
        // stream name at the same ds_NN.tsv file (the data loader picks
        // its named column).
        let model_for_obs = {
            let (m, _) = crate::util::load_model(&config.model.camdl).unwrap_or_else(|e| {
                eprintln!("error loading model for obs stream names: {}", e);
                std::process::exit(1);
            });
            m
        };
        let obs_names: Vec<String> = model_for_obs.observations.iter()
            .map(|o| o.name.clone()).collect();
        let mut out = Vec::with_capacity(synthetic_datasets.len() * fit_seeds.len());
        for ds in &synthetic_datasets {
            let mut observations = indexmap::IndexMap::new();
            for n in &obs_names {
                observations.insert(n.clone(), ds.path.to_string_lossy().to_string());
            }
            let data_spec = config_v2::DataSpec {
                observations,
                holdout_after: None,
                holdout: None,
            };
            for &fs in &fit_seeds {
                out.push(Cell {
                    dataset_idx: Some(ds.idx),
                    fit_seed: fs,
                    data_override: Some(data_spec.clone()),
                });
            }
        }
        out
    };

    let total_cells = cells.len();
    if total_cells > 1 {
        eprintln!("grid: {} cell{}", total_cells,
            if total_cells == 1 { "" } else { "s" });
    }

    // ── Execute grid: cell × sweep_point × stage ──
    for (cell_i, cell) in cells.iter().enumerate() {
        let mut cell_config = config.clone();
        if let Some(spec) = &cell.data_override {
            // Materialise the synthetic cell's data path. Keep
            // `synthetic` set so `per_fit_prefix` picks the
            // `synthetic/ds_NN/fit_<seed>/` branch; `data_spec()`
            // returns `data` when both are present, which is the
            // per-cell behaviour we want.
            cell_config.data = Some(spec.clone());
        }
        let seed = cell.fit_seed;
        if total_cells > 1 {
            match cell.dataset_idx {
                Some(idx) => eprintln!("\n━━━ cell {}/{}: ds_{:02} × fit_seed={} ━━━",
                    cell_i + 1, total_cells, idx, seed),
                None      => eprintln!("\n━━━ cell {}/{}: fit_seed={} ━━━",
                    cell_i + 1, total_cells, seed),
            }
        }

    // Execute stages: sweep_point × stage
    for (pt_idx, sweep_point) in sweep_points.iter().enumerate() {
        // Build a config with swept values applied to [fixed]
        let mut sweep_config = cell_config.clone();
        for (name, val) in sweep_point {
            sweep_config.fixed.values.insert(name.clone(), *val);
        }

        // Recalculate the legacy bridge with swept values
        let sweep_legacy = sweep_config.to_legacy_toml().unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });

        // Per-cell output directory:
        //   real-data:  <fit_dir>/real/fit_<seed>/<stage>/
        //   synthetic:  <fit_dir>/synthetic/ds_NN/fit_<seed>/<stage>/
        // Sweep slug (when present) is nested under the per-fit prefix.
        let per_fit_prefix = sweep_config.per_fit_prefix(seed, cell.dataset_idx);
        let sweep_fit_dir = if has_sweep {
            let slug: String = sweep_point.iter()
                .map(|(k, v)| format!("{}_{:.3}", k, v))
                .collect::<Vec<_>>()
                .join("__");
            if pt_idx == 0 {
                eprintln!("");
            }
            eprintln!("═══ sweep point {}/{}: {} ═══", pt_idx + 1, sweep_points.len(), slug);
            fit_dir.join(&per_fit_prefix).join(slug)
        } else {
            fit_dir.join(&per_fit_prefix)
        };

    for (stage_name, stage) in &stages_to_run {
        let stage_dir = sweep_fit_dir.join(stage_name);
        eprintln!("\n── stage: {} (method={}) ──", stage_name, stage.method_name());

        // Config hash staleness check
        let fixed_resolved = sweep_config.fixed.resolve().unwrap_or_default();
        let data_spec = sweep_config.data_spec().unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });
        let config_hash = provenance::fit_stage_hash(
            &model_json, &data_spec.observations, &sweep_config.estimate,
            &fixed_resolved, stage_name, stage, seed,
        ).unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });
        if !force {
            match crate::run_meta::Run::check_cache(&stage_dir, &config_hash) {
                crate::run_meta::CacheStatus::Hit { .. } => {
                    eprintln!("  \x1b[33mskipped — results already exist for these inputs.\x1b[0m");
                    eprintln!("  config_hash: {}", &config_hash[..16]);
                    eprintln!("  Use --force to re-run.");
                    continue;
                }
                crate::run_meta::CacheStatus::Stale { stored, current } => {
                    eprintln!("  \x1b[33mstale results detected — config has changed. Re-running.\x1b[0m");
                    eprintln!("  stored:  {}", &stored[..16.min(stored.len())]);
                    eprintln!("  current: {}", &current[..16.min(current.len())]);
                }
                crate::run_meta::CacheStatus::Miss => {}
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

                // Gate 1 — pre-stage: if this stage consumes a prior
                // stage (starts_from), refuse to run when the prior
                // stage's tail-Rhat failed convergence on any
                // non-IVP param. Skipped when starts_from is absent
                // (this stage is itself the scout). Overridable via
                // --allow-nonconverged-scout. See proposal
                // docs/dev/proposals/2026-04-19-refine-gates-scout-convergence.md.
                let (scout_best_for_gate2, scout_chain_logliks_for_gate2):
                    (Option<f64>, Vec<f64>) = match prior_state.as_ref() {
                    Some(ps) => {
                        use gating::ScoutGateVerdict;
                        match gating::check_scout_convergence(ps) {
                            ScoutGateVerdict::Ok => {}
                            ScoutGateVerdict::SoftWarn { param_rhats } => {
                                eprintln!("\x1b[33m  warning:\x1b[0m prior stage tail-Rhat in \
                                           1.05–1.10 grey zone for: {}",
                                    param_rhats.iter()
                                        .map(|(n, r)| format!("{} (Rhat={:.2})", n, r))
                                        .collect::<Vec<_>>().join(", "));
                            }
                            ScoutGateVerdict::Hard { failing, all_structural, ivp, loglik_spread } => {
                                let msg = gating::format_hard_verdict(
                                    &failing, &all_structural, &ivp,
                                    loglik_spread, ps.best_loglik, None);
                                if allow_nonconverged_scout {
                                    eprintln!("\x1b[33m  warning:\x1b[0m {}", msg);
                                    eprintln!("\n  --allow-nonconverged-scout: proceeding anyway.");
                                } else {
                                    eprintln!("error: {}", msg);
                                    std::process::exit(1);
                                }
                            }
                        }
                        (Some(ps.best_loglik), ps.chain_logliks.clone())
                    }
                    None => (None, Vec::new()),
                };

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
                // When a stage has no `starts_from` predecessor and runs
                // > 1 chain, give each chain its own random starting
                // point from bounds. This matches v1 scout's default
                // and is what makes Rhat across chains meaningful —
                // chains starting from the same point only diverge via
                // per-chain RNG, so their between-chain variance
                // reflects sampling noise rather than
                // independence-of-starts. When `starts_from` resolves
                // to a prior stage, every chain correctly starts from
                // that stage's MLE (the intent of the handoff).
                let per_chain_params = if effective_starts.is_none() && *chains > 1 {
                    Some(runner::build_random_chain_starts(&run_config, seed, *chains))
                } else {
                    None
                };
                let chain_results = runner::run_chains_with_per_chain_params(
                    &run_config, per_chain_params.as_deref(), &collector);
                let elapsed = t0.elapsed();

                // Gate 2 — post-stage: refine must not regress below
                // scout's best. Not overridable — a regression is a
                // pipeline failure regardless of user preference.
                // Fires only when a prior stage was consumed (scout→
                // refine handoff). Fails before writing any
                // "stage completed" artefacts so the filesystem tells
                // the truth.
                if let Some(scout_best) = scout_best_for_gate2 {
                    if let Err(msg) = gating::check_loglik_regression(
                        scout_best, chain_results.best_loglik,
                        &scout_chain_logliks_for_gate2,
                    ) {
                        eprintln!("error: {}", msg);
                        std::process::exit(1);
                    }
                }

                // Write outputs
                let param_names: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
                runner::write_chain_outputs(
                    &stage_dir.to_string_lossy(), &chain_results.results,
                    &run_config.estimated_params, &param_names,
                    &run_config.base_params, &run_config.compiled,
                ).unwrap_or_else(|e| eprintln!("warning: {}", e));
                // Pre-filter starts — records whatever per-chain
                // initial points IF2 actually received. With the
                // per-chain random-start builder above, this file now
                // shows genuine independence across chains when
                // `starts_from` is None.
                runner::write_chain_starts(
                    &stage_dir.to_string_lossy(),
                    per_chain_params.as_deref(),
                    &run_config.estimated_params, *chains,
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
                    tail_rhat: chain_results.rhat.clone(),
                    ivp_params: run_config.estimated_params.iter()
                        .filter(|p| p.ivp).map(|p| p.name.clone()).collect(),
                    chain_logliks: chain_results.results.iter()
                        .map(|(_, r)| r.final_loglik).collect(),
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
                let data_hashes: Vec<(String, String)> = sweep_config.data_spec()
                    .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
                    .observations.iter()
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
                    seed, force, true, true, false,
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
                // Bubble loglik from fit_state.toml written by PGAS runner
                if let Ok(fs) = state::FitState::load(&stage_dir.to_string_lossy()) {
                    stage_best_loglik = Some(fs.best_loglik);
                    stage_best_chain = Some(fs.best_chain);
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
                    seed, force, false, false,
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
                if let Ok(fs) = state::FitState::load(&stage_dir.to_string_lossy()) {
                    stage_best_loglik = Some(fs.best_loglik);
                    stage_best_chain = Some(fs.best_chain);
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
                stage_best_loglik = Some(mean_ll);
            }
        }

        // ── Shared run.json write (all stage types) ─────────────────────────
        let stage_elapsed = stage_t0.elapsed();
        // Resolve the upstream stage's name + hash from its run.json
        // (if it exists). `effective_starts` is a directory path — for
        // in-fit references it points to a sibling stage that's already
        // written its run.json by the time we run; for external
        // `--starts-from` it points to an arbitrary directory whose
        // run.json may or may not exist.
        let starts_from_ref = effective_starts.as_ref().map(|dir_path| {
            let p = std::path::Path::new(dir_path);
            let stage_name = p.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let stage_hash = crate::run_meta::Run::read(p)
                .map(|r| r.hash)
                .unwrap_or_default();
            crate::run_meta::StartsFromRef { stage: stage_name, stage_hash }
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
        let n_chains = match stage {
            Stage::IF2  { chains, .. } => *chains,
            Stage::PGAS { chains, .. } => *chains,
            Stage::PMMH { chains, .. } => *chains,
            Stage::PFilter { .. }      => 1,
        };
        let stage_run = crate::run_meta::Run {
            hash: config_hash.clone(),
            version: crate::version::VERSION_SHORT.to_string(),
            created_at: scout::now_iso8601_pub(),
            argv: std::env::args().collect(),
            wall_time_seconds: stage_elapsed.as_secs_f64(),
            kind: crate::run_meta::RunKind::FitStage(crate::run_meta::FitStageMeta {
                fit_hash: parent_fit_hash.clone(),
                stage: stage_name.to_string(),
                method: stage.method_name().to_string(),
                seed,
                n_chains,
                algorithm: algo_json,
                best_loglik: stage_best_loglik,
                best_chain: stage_best_chain,
                starts_from: starts_from_ref,
                derived_from: sweep_config.provenance.as_ref()
                    .and_then(|p| p.derived_from.clone()),
            }),
        };
        if let Err(e) = stage_run.write(&stage_dir) {
            eprintln!("warning: could not write {}/run.json: {}", stage_dir.display(), e);
        }

    } // end stages
    } // end sweep_points
    } // end cells

    // ── Post-grid aggregation: summary.tsv (+ coverage.tsv for synthetic)
    //
    // Walk each cell's terminal-stage output, parse the `mle_params.toml`
    // back into a row, and write the tables. `summary.tsv` lives under
    // `real/` or `synthetic/` — the visual subdir that groups all of a
    // fit's cells.
    let terminal_stage = stages_to_run.last()
        .map(|(n, _)| n.to_string())
        .unwrap_or_else(|| "mle".to_string());

    let source = if config.synthetic.is_some() { "synthetic" } else { "real" };
    let mut rows: Vec<summary::SummaryRow> = Vec::new();
    for (cell_i, cell) in cells.iter().enumerate() {
        let (dataset, cell_dir) = match cell.dataset_idx {
            Some(idx) => {
                let ds = format!("ds_{:02}", idx);
                let dir = fit_dir.join("synthetic").join(&ds).join(format!("fit_{}", cell.fit_seed));
                (ds, dir)
            }
            None => {
                let dir = fit_dir.join("real").join(format!("fit_{}", cell.fit_seed));
                ("real".to_string(), dir)
            }
        };
        match summary::read_cell_row(&cell_dir, &terminal_stage, &dataset, cell.fit_seed) {
            Some(r) => rows.push(r),
            None    => eprintln!(
                "warning: cell {}/{} ({} × fit_seed={}) produced no mle_params.toml at {}",
                cell_i + 1, cells.len(), dataset, cell.fit_seed,
                cell_dir.join(&terminal_stage).display()),
        }
    }

    if !rows.is_empty() {
        match summary::write_summary(&fit_dir, source, &rows) {
            Ok(p)  => eprintln!("summary: {}", p.display()),
            Err(e) => eprintln!("warning: could not write summary.tsv: {}", e),
        }
        if config.synthetic.is_some() {
            match summary::load_truth(&fit_dir) {
                Ok(truth) => match summary::write_coverage(&fit_dir, &truth, &rows) {
                    Ok(p)  => eprintln!("coverage: {}", p.display()),
                    Err(e) => eprintln!("warning: could not write coverage.tsv: {}", e),
                },
                Err(e) => eprintln!("warning: no truth for coverage: {}", e),
            }
        }
    }

    // ── Final rewrite: top-level run.json with accumulated wall time ──
    //
    // The top-level Run::Fit was written at fit-start with wall_time=0
    // so the fit is listable even if interrupted. Now that every stage
    // has completed (or aggregate post-processing has finished), patch
    // the wall-clock so `camdl list` / `camdl show` report honest
    // totals.
    run_fit.wall_time_seconds = fit_start.elapsed().as_secs_f64();
    if let Err(e) = run_fit.write(&fit_dir) {
        eprintln!("warning: cannot rewrite {}/run.json: {}", fit_dir.display(), e);
    }
}

// ─── camdl fit diff ─────────────────────────────────────────────────────────

/// Reshape a v1 FitToml so its `output_dir` points at the unified-tree
/// cell directory (`<output_root>/fits/<stem>-<hash>/real/fit_<seed>/`).
/// v1 stage writers (scout/refine/validate/pmmh/pgas) all use
/// `fit.fit.output_dir` as "the directory under which my stage dir
/// lives" — by overwriting it here we land them in the same shape as
/// v2 without touching the stage-writer internals. Also writes (once)
/// the top-level Run::Fit at the fit root so `camdl list` can surface
/// the fit even before the first stage completes.
fn prepare_v1_cell(fit: &FitToml, fit_path: &str, seed: u64) -> (FitToml, std::path::PathBuf) {
    let cell = fit.cell_dir(fit_path, seed).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let fit_root = fit.fit_root(fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    if let Err(e) = std::fs::create_dir_all(&cell) {
        eprintln!("error: cannot create {}: {}", cell.display(), e);
        std::process::exit(1);
    }
    // Write the top-level Run::Fit if absent. Stage re-runs skip this.
    if !fit_root.join("run.json").exists() {
        let run_fit = build_v1_fit_run(fit, fit_path);
        if let Err(e) = run_fit.write(&fit_root) {
            eprintln!("warning: cannot write {}/run.json: {}", fit_root.display(), e);
        }
    }
    let mut reshaped = fit.clone();
    reshaped.fit.output_dir = cell.to_string_lossy().to_string();
    (reshaped, fit_root)
}

/// Build a Run::Fit record from a v1 FitToml. Analogous to
/// `build_fit_run` for v2; emits the same schema so `camdl list`
/// treats v1 fits identically.
fn build_v1_fit_run(fit: &FitToml, fit_path: &str) -> crate::run_meta::Run {
    let fit_hash = fit.fit_content_hash(fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let model_ir_json = std::fs::read_to_string(&fit.fit.model).unwrap_or_default();
    let model_hash = if model_ir_json.is_empty() {
        String::new()
    } else {
        crate::hashing::model_hash(&model_ir_json)
    };
    let fit_toml_bytes = std::fs::read(fit_path).unwrap_or_default();
    let fit_toml_hash = crate::hashing::sha256_hex(&fit_toml_bytes);
    let data_hashes: std::collections::HashMap<String, String> = fit.data.iter()
        .filter_map(|(name, path)|
            crate::hashing::file_hash(path).map(|h| (name.clone(), h)))
        .collect();
    let estimated: Vec<String> = fit.estimate.keys().cloned().collect();
    let fixed: std::collections::HashMap<String, f64> = fit.fixed.iter()
        .filter_map(|(k, v)| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
            .map(|f| (k.clone(), f)))
        .collect();
    // v1 declares stages implicitly via [scout]/[refine]/[validate]/
    // [pmmh]/[pgas] tables; the list reflects which are present in
    // declaration order (not execution order — v1 has no chained
    // stage graph).
    let mut stages_declared: Vec<String> = Vec::new();
    if fit.scout.is_some()    { stages_declared.push("scout".into()); }
    if fit.refine.is_some()   { stages_declared.push("refine".into()); }
    if fit.validate.is_some() { stages_declared.push("validate".into()); }
    if fit.pmmh.is_some()     { stages_declared.push("pmmh".into()); }
    if fit.pgas.is_some()     { stages_declared.push("pgas".into()); }
    crate::run_meta::Run {
        hash: fit_hash,
        version: crate::version::VERSION_SHORT.to_string(),
        created_at: crate::cas::iso8601_utc(std::time::SystemTime::now()),
        argv: std::env::args().collect(),
        wall_time_seconds: 0.0,
        kind: crate::run_meta::RunKind::Fit(crate::run_meta::FitMeta {
            model: fit.fit.model.clone(),
            model_hash,
            fit_toml_path: fit_path.to_string(),
            fit_toml_hash,
            data_hashes,
            estimated,
            fixed,
            stages_declared,
            ic_free: fit.fit.ic_free,
        }),
    }
}

/// After a v1 stage writer (scout / refine / …) has completed, write
/// the unified-schema Run::FitStage `run.json` at the stage dir.
/// Idempotent: re-running overwrites with fresh timing + hash.
fn write_v1_stage_run(
    stage_dir: &std::path::Path,
    fit_hash: &str,
    stage: &str,
    method: &str,
    seed: u64,
    n_chains: usize,
    algorithm: serde_json::Value,
    best_loglik: Option<f64>,
    best_chain: Option<usize>,
    wall_time_seconds: f64,
    starts_from: Option<&str>,
) {
    // stage_hash for v1 is a modest content hash over the stage's
    // filesystem output. v1 doesn't have v2's full stage-input hash
    // because its stage config (StageConfig etc.) isn't versioned
    // with the same discipline; hashing the actual output is the
    // honest alternative.
    let stage_hash = crate::hashing::file_hash(
        &stage_dir.join("fit_state.toml").to_string_lossy())
        .unwrap_or_default();
    let starts_from_ref = starts_from.map(|dir_path| {
        let p = std::path::Path::new(dir_path);
        let stage_name = p.file_name()
            .and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
        let stage_hash = crate::run_meta::Run::read(p)
            .map(|r| r.hash).unwrap_or_default();
        crate::run_meta::StartsFromRef { stage: stage_name, stage_hash }
    });
    // Hash scope: (stage name + seed + stage output hash). Enough
    // for cache-staleness detection; not pretending to be a full
    // input-closure hash like v2's fit_stage_hash.
    let run_hash = {
        use sha2::{Sha256, Digest};
        let mut h = Sha256::new();
        h.update(stage.as_bytes()); h.update(b"\x00");
        h.update(seed.to_le_bytes());
        h.update(stage_hash.as_bytes());
        hex::encode(h.finalize())
    };
    let run = crate::run_meta::Run {
        hash: run_hash,
        version: crate::version::VERSION_SHORT.to_string(),
        created_at: crate::cas::iso8601_utc(std::time::SystemTime::now()),
        argv: std::env::args().collect(),
        wall_time_seconds,
        kind: crate::run_meta::RunKind::FitStage(crate::run_meta::FitStageMeta {
            fit_hash: fit_hash.to_string(),
            stage: stage.to_string(),
            method: method.to_string(),
            seed,
            n_chains,
            algorithm,
            best_loglik,
            best_chain,
            starts_from: starts_from_ref,
            derived_from: None,
        }),
    };
    if let Err(e) = run.write(stage_dir) {
        eprintln!("warning: cannot write {}/run.json: {}", stage_dir.display(), e);
    }
}

/// Read `best_loglik` and `best_chain` from a v1 stage's
/// `fit_state.toml`. Returns `(None, None)` when unavailable (e.g.
/// the stage didn't produce fit_state).
fn read_v1_stage_best(stage_dir: &std::path::Path) -> (Option<f64>, Option<usize>) {
    use crate::fit::state::FitState;
    match FitState::load(&stage_dir.to_string_lossy()) {
        Ok(s) => (Some(s.best_loglik), Some(s.best_chain)),
        Err(_) => (None, None),
    }
}

/// Build the top-level `Run::Fit` record for a fit.toml. Fields that
/// require I/O (model IR, data files, fit.toml bytes) are read here
/// and hashed; `wall_time_seconds` is initialised to 0 and patched at
/// end-of-fit by the caller. Silent fallbacks (empty strings / empty
/// maps) cover the read-error case so a partially-written fit still
/// produces something `camdl list` can display.
fn build_fit_run(config: &config_v2::FitConfigV2, fit_path: &str) -> crate::run_meta::Run {
    let fit_hash = config.fit_content_hash(fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let model_ir_json = std::fs::read_to_string(&config.model.camdl).unwrap_or_default();
    let model_hash = if model_ir_json.is_empty() {
        String::new()
    } else {
        crate::hashing::model_hash(&model_ir_json)
    };
    let fit_toml_bytes = std::fs::read(fit_path).unwrap_or_default();
    let fit_toml_hash = crate::hashing::sha256_hex(&fit_toml_bytes);
    let data_hashes: std::collections::HashMap<String, String> = config
        .data.as_ref()
        .map(|d| d.observations.iter()
            .filter_map(|(name, path)| {
                crate::hashing::file_hash(path).map(|h| (name.clone(), h))
            })
            .collect())
        .unwrap_or_default();
    let estimated: Vec<String> = config.estimate.keys().cloned().collect();
    let fixed: std::collections::HashMap<String, f64> = config.fixed
        .resolve().unwrap_or_default().into_iter().collect();
    let stages_declared: Vec<String> = config.stages.keys().cloned().collect();
    crate::run_meta::Run {
        hash: fit_hash,
        version: crate::version::VERSION_SHORT.to_string(),
        created_at: crate::cas::iso8601_utc(std::time::SystemTime::now()),
        argv: std::env::args().collect(),
        wall_time_seconds: 0.0,
        kind: crate::run_meta::RunKind::Fit(crate::run_meta::FitMeta {
            model: config.model.camdl.clone(),
            model_hash,
            fit_toml_path: fit_path.to_string(),
            fit_toml_hash,
            data_hashes,
            estimated,
            fixed,
            stages_declared,
            ic_free: config.ic_free.unwrap_or(false),
        }),
    }
}

fn format_prior(p: &Option<config_v2::PriorSpec>) -> String {
    match p {
        None => "(none)".to_string(),
        Some(config_v2::PriorSpec::LogNormal { mu, sigma }) => format!("log_normal(mu={}, sigma={})", mu, sigma),
        Some(config_v2::PriorSpec::Normal { mu, sigma }) => format!("normal(mu={}, sigma={})", mu, sigma),
        Some(config_v2::PriorSpec::Beta { alpha, beta }) => format!("beta(alpha={}, beta={})", alpha, beta),
        Some(config_v2::PriorSpec::Uniform) => "uniform".to_string(),
        Some(config_v2::PriorSpec::HalfNormal { sigma }) => format!("half_normal(sigma={})", sigma),
    }
}

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
    // Prior changes
    for name in a_est.intersection(&b_est) {
        let ap = &a.estimate[*name].prior;
        let bp = &b.estimate[*name].prior;
        let ap_str = format_prior(ap);
        let bp_str = format_prior(bp);
        if ap_str != bp_str {
            println!("  {}: prior {} → {}", name, ap_str, bp_str);
            param_changes = true;
        }
    }
    if !param_changes {
        println!("  (no parameter changes)");
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
        let source_fit_dir = match cfg.fit_dir(&from) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("warning: could not compute source fit dir: {}", e);
                return;
            }
        };
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

fn parse_fit_args(args: &[String], _needs_starts_from: bool) -> (FitToml, String, u64, bool) {
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

    (fit, fit_path, seed, force)
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
