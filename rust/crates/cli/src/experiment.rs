use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use rayon::prelude::*;

use crate::util::{run_simulation, write_traj_tsv, load_params_toml, resolve_ir_path, SimRun};
use crate::hashing::{model_hash, config_hash, input_hash, canonical_params};

// ─── TOML schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ExperimentToml {
    config: ConfigSection,
    #[serde(default)]
    scenario: Vec<ScenarioEntry>,
}

#[derive(Debug, Deserialize)]
struct ConfigSection {
    model: String,
    #[serde(default)]
    params: Option<String>,
    #[serde(default)]
    geo: Option<String>,
    #[serde(default = "default_backend")]
    backend: String,
    #[serde(default = "default_dt")]
    dt: f64,
    #[serde(default = "default_output_dir")]
    output_dir: String,
    #[serde(default = "default_parallel")]
    parallel: usize,
    #[serde(default)]
    seeds: SeedsSection,
}

fn default_backend() -> String { "gillespie".to_string() }
fn default_dt() -> f64 { 1.0 }
fn default_output_dir() -> String { "output".to_string() }
fn default_parallel() -> usize { 1 }

#[derive(Debug, Deserialize, Default)]
struct SeedsSection {
    /// seeds from..=to
    from: Option<u64>,
    to:   Option<u64>,
    /// explicit list
    list: Option<Vec<u64>>,
    /// n seeds starting at `start` (default 1)
    n:    Option<u64>,
    start: Option<u64>,
}

impl SeedsSection {
    fn resolve(&self) -> Result<Vec<u64>, String> {
        if let Some(ref list) = self.list {
            return Ok(list.clone());
        }
        if let Some(n) = self.n {
            let start = self.start.unwrap_or(1);
            return Ok((start..start + n).collect());
        }
        if let (Some(from), Some(to)) = (self.from, self.to) {
            return Ok((from..=to).collect());
        }
        // default: single seed 1
        Ok(vec![1])
    }
}

#[derive(Debug, Deserialize, Clone)]
struct ScenarioEntry {
    name: String,
    #[serde(default)]
    params: HashMap<String, f64>,
    #[serde(default)]
    enable: Vec<String>,
    #[serde(default)]
    disable: Vec<String>,
}

// ─── Manifest / run metadata ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct RunMeta {
    scenario: String,
    seed: u64,
    input_hash: String,
    config_hash: String,
    model_hash: String,
}

/// Minimal descriptor for one completed run, included in manifest.json.
/// The web app uses input_hash to construct the URL: /runs/{input_hash}/traj.tsv
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunEntry {
    scenario: String,
    seed: u64,
    input_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    model: String,
    scenarios: Vec<String>,
    seeds: Vec<u64>,
    total_runs: usize,
    completed: usize,
    output_dir: String,
    /// Relative URL to the GeoJSON boundary file, if provided via config.geo.
    #[serde(skip_serializing_if = "Option::is_none")]
    geo: Option<String>,
    /// All completed runs, in scenario×seed order. Used by web app to load trajectories.
    runs: Vec<RunEntry>,
}

// ─── cmd_experiment_run ──────────────────────────────────────────────────────

pub fn cmd_experiment_run(args: &[String]) {
    // Parse flags
    let mut toml_path: Option<String> = None;
    let mut output_dir_override: Option<String> = None;
    let mut parallel_override: Option<usize> = None;
    let mut force = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--output-dir" => {
                i += 1;
                output_dir_override = Some(args[i].clone());
            }
            "--parallel" => {
                i += 1;
                parallel_override = Some(args[i].parse().unwrap_or_else(|_| {
                    eprintln!("error: --parallel requires an integer");
                    std::process::exit(1);
                }));
            }
            "--force" => { force = true; }
            "--resume" => { /* default, no-op */ }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                experiment_usage();
            }
            path => { toml_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let toml_path = toml_path.unwrap_or_else(|| {
        eprintln!("error: experiment run requires a TOML file path");
        experiment_usage();
    });

    // Load and parse TOML
    let toml_src = std::fs::read_to_string(&toml_path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", toml_path, e);
        std::process::exit(1);
    });
    let exp: ExperimentToml = toml::from_str(&toml_src).unwrap_or_else(|e| {
        eprintln!("error: TOML parse error in {}: {}", toml_path, e);
        std::process::exit(1);
    });

    let output_dir = output_dir_override.unwrap_or(exp.config.output_dir.clone());
    let parallel = parallel_override.unwrap_or(exp.config.parallel);
    let backend = exp.config.backend.clone();
    let dt = exp.config.dt;
    let model_path = exp.config.model.clone();

    // Read IR JSON for hashing
    let (ir_path_resolved, _tmpfile) = resolve_ir_path(&model_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    let ir_json = std::fs::read_to_string(&ir_path_resolved).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", ir_path_resolved, e);
        std::process::exit(1);
    });
    let mhash = model_hash(&ir_json);

    // Build params map for hashing (only from params file, not per-scenario overrides)
    let base_params: HashMap<String, f64> = if let Some(ref pf) = exp.config.params {
        load_params_toml(pf).unwrap_or_else(|e| {
            eprintln!("error: cannot load params {}: {}", pf, e);
            std::process::exit(1);
        })
    } else {
        HashMap::new()
    };
    let params_str = canonical_params(&base_params);
    let cfg_hash = config_hash(&mhash, &params_str, &backend);

    // Resolve seeds and scenarios
    let seeds = exp.config.seeds.resolve().unwrap_or_else(|e| {
        eprintln!("error resolving seeds: {}", e);
        std::process::exit(1);
    });

    // If no [[scenario]] entries, run a single implicit "baseline"
    let scenarios: Vec<ScenarioEntry> = if exp.scenario.is_empty() {
        vec![ScenarioEntry { name: "baseline".to_string(), params: HashMap::new(), enable: Vec::new(), disable: Vec::new() }]
    } else {
        exp.scenario
    };

    // Build the full list of (scenario, seed) pairs
    let mut work_items: Vec<(ScenarioEntry, u64)> = Vec::new();
    for sc in &scenarios {
        for &seed in &seeds {
            work_items.push((sc.clone(), seed));
        }
    }
    let total = work_items.len();

    // Ensure output dir exists
    let runs_dir = format!("{}/runs", output_dir);
    std::fs::create_dir_all(&runs_dir).unwrap_or_else(|e| {
        eprintln!("error: cannot create output dir {}: {}", runs_dir, e);
        std::process::exit(1);
    });

    // Copy model IR to output dir so the web app can load it via camdl serve.
    let model_ir_dest = format!("{}/model.ir.json", output_dir);
    std::fs::write(&model_ir_dest, &ir_json).unwrap_or_else(|e| {
        eprintln!("warning: could not write model.ir.json: {}", e);
    });

    // Copy GeoJSON boundary file if specified (→ <output-dir>/geo/boundaries.geojson).
    let geo_url: Option<String> = if let Some(ref geo_src) = exp.config.geo {
        let geo_dir = format!("{}/geo", output_dir);
        let geo_dest = format!("{}/boundaries.geojson", geo_dir);
        match std::fs::create_dir_all(&geo_dir).and_then(|_| std::fs::copy(geo_src, &geo_dest)) {
            Ok(_) => Some("geo/boundaries.geojson".to_string()),
            Err(e) => {
                eprintln!("warning: could not copy geo file '{}': {}", geo_src, e);
                None
            }
        }
    } else {
        None
    };

    let counter = Arc::new(AtomicUsize::new(0));
    let scenario_names: Vec<String> = scenarios.iter().map(|s| s.name.clone()).collect();

    // Determine parallelism
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(if parallel > 0 { parallel } else { 1 })
        .build()
        .unwrap_or_else(|e| {
            eprintln!("error building thread pool: {}", e);
            std::process::exit(1);
        });

    let params_file_opt = exp.config.params.clone();
    // Each work item produces Ok(RunEntry) on success or Err(message) on failure.
    let results: Vec<Result<RunEntry, String>> = pool.install(|| {
        work_items.par_iter().map(|(sc, seed)| {
            let ihash = input_hash(&cfg_hash, &sc.name, *seed);
            let run_dir = format!("{}/{}", runs_dir, ihash);
            let traj_path = format!("{}/traj.tsv", run_dir);

            // Skip if output exists and not --force
            if !force && std::path::Path::new(&traj_path).exists() {
                let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!("[{}/{}] scenario={} seed={} (skipped — already exists)", n, total, sc.name, seed);
                return Ok(RunEntry { scenario: sc.name.clone(), seed: *seed, input_hash: ihash });
            }

            // Build SimRun
            let mut overrides_map: HashMap<String, f64> = HashMap::new();
            for (k, v) in &sc.params {
                overrides_map.insert(k.clone(), *v);
            }

            let sim_run = SimRun {
                ir_path: ir_path_resolved.clone(),
                params_files: params_file_opt.as_ref().map(|p| vec![p.clone()]).unwrap_or_default(),
                overrides: overrides_map,
                scenario_name: None, // We handle scenario param overrides above
                adhoc_enable: sc.enable.clone(),
                adhoc_disable: sc.disable.clone(),
                backend: backend.clone(),
                dt,
                seed: *seed,
                ..Default::default()
            };

            // Run simulation
            match run_simulation(&sim_run) {
                Err(e) => {
                    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!("[{}/{}] scenario={} seed={} ERROR: {}", n, total, sc.name, seed, e);
                    Err(format!("scenario={} seed={}: {}", sc.name, seed, e))
                }
                Ok((traj, model)) => {
                    // Create run dir
                    if let Err(e) = std::fs::create_dir_all(&run_dir) {
                        return Err(format!("cannot create {}: {}", run_dir, e));
                    }

                    // Write traj.tsv
                    if let Err(e) = write_traj_tsv(&traj_path, &model, &traj, false) {
                        return Err(format!("cannot write {}: {}", traj_path, e));
                    }

                    // Write run.json metadata
                    let meta = RunMeta {
                        scenario: sc.name.clone(),
                        seed: *seed,
                        input_hash: ihash.clone(),
                        config_hash: cfg_hash.clone(),
                        model_hash: mhash.clone(),
                    };
                    let meta_json = serde_json::to_string_pretty(&meta)
                        .unwrap_or_else(|_| "{}".to_string());
                    let meta_path = format!("{}/run.json", run_dir);
                    if let Err(e) = std::fs::write(&meta_path, meta_json) {
                        return Err(format!("cannot write {}: {}", meta_path, e));
                    }

                    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!("[{}/{}] scenario={} seed={}", n, total, sc.name, seed);
                    Ok(RunEntry { scenario: sc.name.clone(), seed: *seed, input_hash: ihash })
                }
            }
        }).collect()
    });

    let mut errors: Vec<String> = Vec::new();
    let mut completed_runs: Vec<RunEntry> = Vec::new();
    for result in results {
        match result {
            Ok(entry) => completed_runs.push(entry),
            Err(e)    => errors.push(e),
        }
    }

    if !errors.is_empty() {
        eprintln!("Errors encountered:");
        for e in &errors {
            eprintln!("  {}", e);
        }
    }

    let completed = completed_runs.len();

    // Write manifest.json — includes completed run entries so the web app can load trajectories.
    let manifest = Manifest {
        model: model_path.clone(),
        scenarios: scenario_names,
        seeds: seeds.clone(),
        total_runs: total,
        completed,
        output_dir: output_dir.clone(),
        geo: geo_url,
        runs: completed_runs,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .unwrap_or_else(|_| "{}".to_string());
    let manifest_path = format!("{}/manifest.json", output_dir);
    std::fs::write(&manifest_path, &manifest_json).unwrap_or_else(|e| {
        eprintln!("warning: could not write manifest.json: {}", e);
    });

    eprintln!("Done: {}/{} runs completed. Manifest: {}", completed, total, manifest_path);

    if !errors.is_empty() {
        std::process::exit(1);
    }
}

// ─── cmd_experiment_status ───────────────────────────────────────────────────

pub fn cmd_experiment_status(args: &[String]) {
    let mut toml_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                experiment_usage();
            }
            path => { toml_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let toml_path = toml_path.unwrap_or_else(|| {
        eprintln!("error: experiment status requires a TOML file path");
        experiment_usage();
    });

    let toml_src = std::fs::read_to_string(&toml_path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", toml_path, e);
        std::process::exit(1);
    });
    let exp: ExperimentToml = toml::from_str(&toml_src).unwrap_or_else(|e| {
        eprintln!("error: TOML parse error in {}: {}", toml_path, e);
        std::process::exit(1);
    });

    let output_dir = exp.config.output_dir.clone();
    let manifest_path = format!("{}/manifest.json", output_dir);

    // Try to read manifest first
    if let Ok(manifest_src) = std::fs::read_to_string(&manifest_path) {
        if let Ok(manifest) = serde_json::from_str::<Manifest>(&manifest_src) {
            println!("Experiment status for: {}", toml_path);
            println!("  Model:      {}", manifest.model);
            println!("  Output dir: {}", manifest.output_dir);
            println!("  Scenarios:  {}", manifest.scenarios.join(", "));
            println!("  Seeds:      {} total ({:?}..={:?})",
                manifest.seeds.len(),
                manifest.seeds.first().unwrap_or(&0),
                manifest.seeds.last().unwrap_or(&0));
            println!("  Total runs: {}", manifest.total_runs);
            println!("  Completed:  {}/{}", manifest.completed, manifest.total_runs);

            // Recount from filesystem to show live status
            let backend = exp.config.backend.clone();
            let model_path = exp.config.model.clone();
            let base_params: HashMap<String, f64> = if let Some(ref pf) = exp.config.params {
                load_params_toml(pf).unwrap_or_default()
            } else {
                HashMap::new()
            };

            // Try to compute config hash for live count
            if let Ok(ir_json) = std::fs::read_to_string(&model_path) {
                let mhash = model_hash(&ir_json);
                let params_str = canonical_params(&base_params);
                let cfg_hash = config_hash(&mhash, &params_str, &backend);
                let scenarios: Vec<ScenarioEntry> = if exp.scenario.is_empty() {
                    vec![ScenarioEntry { name: "baseline".to_string(), params: HashMap::new(), enable: Vec::new(), disable: Vec::new() }]
                } else {
                    exp.scenario
                };
                let scenario_names: Vec<String> = scenarios.iter().map(|s| s.name.clone()).collect();
                let seeds = exp.config.seeds.resolve().unwrap_or_default();
                let runs_dir = format!("{}/runs", output_dir);
                let live_completed = count_completed_runs(&runs_dir, &cfg_hash, &scenario_names, &seeds);
                let total = scenario_names.len() * seeds.len();
                println!("  Live count: {}/{} traj.tsv files present", live_completed, total);
            }
            return;
        }
    }

    // No manifest — scan filesystem
    println!("Experiment status for: {}", toml_path);
    println!("  No manifest.json found at {}", manifest_path);
    println!("  Run 'camdl experiment run {}' to start.", toml_path);
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn count_completed_runs(
    runs_dir: &str,
    cfg_hash: &str,
    scenario_names: &[String],
    seeds: &[u64],
) -> usize {
    let mut completed = 0;
    for sc_name in scenario_names {
        for &seed in seeds {
            let ihash = input_hash(cfg_hash, sc_name, seed);
            let traj_path = format!("{}/{}/traj.tsv", runs_dir, ihash);
            if std::path::Path::new(&traj_path).exists() {
                completed += 1;
            }
        }
    }
    completed
}

fn experiment_usage() -> ! {
    eprintln!("usage: camdl experiment <run|status> EXPERIMENT.toml [OPTIONS]");
    eprintln!();
    eprintln!("  run OPTIONS:");
    eprintln!("    --output-dir DIR   override output_dir from TOML");
    eprintln!("    --parallel N       override parallel from TOML");
    eprintln!("    --resume           skip runs where output already exists (default)");
    eprintln!("    --force            re-run even if output exists");
    std::process::exit(1);
}
