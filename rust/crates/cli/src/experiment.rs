use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use rayon::prelude::*;

use crate::util::{run_simulation, write_traj_tsv, load_params_toml, resolve_ir_path, SimRun};
use crate::hashing::{model_hash, sim_hash, scen_hash, canonical_params, slug};
use crate::sampling::{generate_design, DesignParam, PriorSpec};

// ─── TOML schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ExperimentToml {
    config: ConfigSection,
    #[serde(default)]
    scenario: Vec<ScenarioEntry>,
    #[serde(default)]
    sweep: HashMap<String, SweepSpec>,
    #[serde(default)]
    design: HashMap<String, DesignBlock>,
}

// ─── Design specification ─────────────────────────────────────────────────────

/// A named experimental design block (`[design.NAME]`).
/// Represents a named belief state: parameter ranges + sampling method.
#[derive(Debug, Deserialize)]
struct DesignBlock {
    method: String,   // "sobol" | "lhs" | "random"
    n: usize,
    #[serde(default)]
    parameters: HashMap<String, DesignParamToml>,
}

/// Per-parameter specification within a design block.
#[derive(Debug, Deserialize)]
struct DesignParamToml {
    range: RangeMinMax,
    #[serde(default)]
    transform: Option<String>,   // "log" | "logit" | None (linear)
    #[serde(default)]
    prior: Option<PriorSpec>,    // prior distribution for VOI importance weighting
}

#[derive(Debug, Deserialize)]
struct RangeMinMax {
    min: f64,
    max: f64,
}

// ─── Sweep specification ─────────────────────────────────────────────────────

/// One swept parameter's value specification.
/// TOML forms:
///   vacc_eff = [0.1, 0.3, 0.5]
///   vacc_eff = { linspace = { min = 0.1, max = 0.9, n = 9 } }
///   kappa    = { logspace = { min = 0.001, max = 0.1, n = 5 } }
///   R0       = { range = { min = 1.0, max = 5.0, step = 0.5 } }
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SweepSpec {
    List(Vec<f64>),
    Linspace { linspace: LinspaceSpec },
    Logspace  { logspace: LinspaceSpec },
    Range     { range: RangeSpec },
}

#[derive(Debug, Deserialize)]
struct LinspaceSpec {
    min: f64,
    max: f64,
    n: usize,
}

#[derive(Debug, Deserialize)]
struct RangeSpec {
    min: f64,
    max: f64,
    #[serde(default = "default_step")]
    step: f64,
}

fn default_step() -> f64 { 1.0 }

impl SweepSpec {
    /// Expand to a concrete vector of values.
    fn expand(&self) -> Vec<f64> {
        match self {
            SweepSpec::List(v) => v.clone(),
            SweepSpec::Linspace { linspace: s } => {
                if s.n == 1 {
                    return vec![s.min];
                }
                (0..s.n).map(|i| s.min + (s.max - s.min) * i as f64 / (s.n - 1) as f64).collect()
            }
            SweepSpec::Logspace { logspace: s } => {
                if s.n == 1 {
                    return vec![s.min];
                }
                let log_min = s.min.ln();
                let log_max = s.max.ln();
                (0..s.n).map(|i| {
                    let t = i as f64 / (s.n - 1) as f64;
                    (log_min + (log_max - log_min) * t).exp()
                }).collect()
            }
            SweepSpec::Range { range: s } => {
                let mut v = Vec::new();
                let mut x = s.min;
                while x <= s.max + 1e-12 * s.step.abs() {
                    v.push(x);
                    x += s.step;
                }
                v
            }
        }
    }
}

/// Expand the full `[sweep]` section into a list of parameter override maps.
/// If no sweep keys are defined, returns a single empty map (one "null" point).
fn expand_sweep(sweep: &HashMap<String, SweepSpec>) -> Vec<HashMap<String, f64>> {
    if sweep.is_empty() {
        return vec![HashMap::new()];
    }
    // Sort keys for deterministic ordering
    let mut keys: Vec<&String> = sweep.keys().collect();
    keys.sort();
    let values: Vec<Vec<f64>> = keys.iter().map(|k| sweep[*k].expand()).collect();

    // Cartesian product via fold
    let mut points: Vec<HashMap<String, f64>> = vec![HashMap::new()];
    for (key, vals) in keys.iter().zip(values.iter()) {
        let mut next = Vec::with_capacity(points.len() * vals.len());
        for point in &points {
            for &v in vals {
                let mut p = point.clone();
                p.insert(key.to_string(), v);
                next.push(p);
            }
        }
        points = next;
    }
    points
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
    from: Option<u64>,
    to:   Option<u64>,
    list: Option<Vec<u64>>,
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
        Ok(vec![1])
    }
}

/// Per-scenario specification as parsed from the experiment TOML.
#[derive(Debug, Deserialize, Clone)]
pub struct ScenarioEntry {
    pub name: String,
    #[serde(default)]
    pub params: HashMap<String, f64>,
    #[serde(default)]
    pub enable: Vec<String>,
    #[serde(default)]
    pub disable: Vec<String>,
}

// ─── Run planning ─────────────────────────────────────────────────────────────

/// Whether a planned run should be skipped (cache hit) or executed (cache miss).
#[derive(Debug, PartialEq)]
pub enum RunDecision {
    /// traj.tsv already exists and --force was not set; cached result will be reused.
    CacheHit,
    /// traj.tsv is absent or --force was set; this run must be executed.
    CacheMiss,
}

/// A fully-resolved description of one (sweep_point, scenario, seed) run,
/// including its cache decision. Produced by `plan_runs` before any simulation
/// is started.
#[derive(Debug)]
pub struct RunPlan {
    pub scenario: String,
    pub seed: u64,
    /// Sweep parameter overrides for this run (empty if no sweep).
    pub sweep_overrides: HashMap<String, f64>,
    /// Index of the design/sweep point (0-based). Used by design experiments
    /// to write run.json so analyze can recover point_id without hashing.
    pub point_idx: usize,
    /// Path relative to runs/: {sim_hash_8}/{scenario_slug}-{scen_hash_8}/seed_{seed}
    pub run_path: String,
    /// Absolute path to the run directory.
    pub run_dir: String,
    pub decision: RunDecision,
}

/// Classify every (sweep_point, scenario, seed) triple as CacheHit or CacheMiss
/// by inspecting the filesystem. Does not simulate or write anything.
///
/// `sweep_points` is a list of parameter override maps from `[sweep]`. Pass
/// `&[HashMap::new()]` (one empty map) when there is no sweep.
///
/// `shash` must be the full 64-char hex sim_hash; only the first 8 chars are
/// used in paths. `runs_dir` is the absolute path to the runs/ subdirectory.
pub fn plan_runs(
    scenarios: &[ScenarioEntry],
    sweep_points: &[HashMap<String, f64>],
    seeds: &[u64],
    shash: &str,
    runs_dir: &str,
    force: bool,
) -> Vec<RunPlan> {
    let sim_hash_8 = &shash[..8];
    let effective_points: &[HashMap<String, f64>] = if sweep_points.is_empty() {
        &[HashMap::new()]
    } else {
        sweep_points
    };

    let mut plans = Vec::with_capacity(effective_points.len() * scenarios.len() * seeds.len());
    for (pt_idx, sweep) in effective_points.iter().enumerate() {
        for sc in scenarios {
            // Merge sweep overrides into scenario params for hashing
            let mut merged_params = sc.params.clone();
            merged_params.extend(sweep.iter().map(|(k, v)| (k.clone(), *v)));

            let sc_hash = scen_hash(&sc.enable, &sc.disable, &merged_params);
            let sc_slug = slug(&sc.name);
            for &seed in seeds {
                let run_path = format!("{}/{}-{}/seed_{}", sim_hash_8, sc_slug, &sc_hash[..8], seed);
                let run_dir  = format!("{}/{}", runs_dir, run_path);
                let traj_exists = std::path::Path::new(&format!("{}/traj.tsv", run_dir)).exists();
                let decision = if !force && traj_exists {
                    RunDecision::CacheHit
                } else {
                    RunDecision::CacheMiss
                };
                plans.push(RunPlan {
                    scenario: sc.name.clone(),
                    seed,
                    sweep_overrides: sweep.clone(),
                    point_idx: pt_idx,
                    run_path,
                    run_dir,
                    decision,
                });
            }
        }
    }
    plans
}

// ─── Manifest / run metadata ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct RunMeta {
    scenario: String,
    seed: u64,
    sim_hash: String,
    scen_hash: String,
    model_hash: String,
}

/// Minimal descriptor for one completed run, included in manifest.json.
/// The web app uses run_path to construct the URL: /runs/{run_path}/traj.tsv
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunEntry {
    scenario: String,
    seed: u64,
    run_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    model: String,
    scenarios: Vec<String>,
    seeds: Vec<u64>,
    total_runs: usize,
    completed: usize,
    output_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo: Option<String>,
    /// Completed runs. run_path is relative to runs/ and used by the web app
    /// to fetch trajectories: GET /runs/{run_path}/traj.tsv
    runs: Vec<RunEntry>,
}

// ─── cmd_experiment_run ──────────────────────────────────────────────────────

pub fn cmd_experiment_run(args: &[String]) {
    let mut toml_path: Option<String> = None;
    let mut output_dir_override: Option<String> = None;
    let mut parallel_override: Option<usize> = None;
    let mut force = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--output-dir" => { i += 1; output_dir_override = Some(args[i].clone()); }
            "--parallel"   => {
                i += 1;
                parallel_override = Some(args[i].parse().unwrap_or_else(|_| {
                    eprintln!("error: --parallel requires an integer");
                    std::process::exit(1);
                }));
            }
            "--force"  => { force = true; }
            "--resume" => { /* default, no-op */ }
            s if s.starts_with("--") => { eprintln!("unknown flag: {}", s); experiment_usage(); }
            path => { toml_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let toml_path = toml_path.unwrap_or_else(|| {
        eprintln!("error: experiment run requires a TOML file path");
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

    let output_dir = output_dir_override.unwrap_or(exp.config.output_dir.clone());
    let parallel   = parallel_override.unwrap_or(exp.config.parallel);
    let backend    = exp.config.backend.clone();
    let dt         = exp.config.dt;
    let model_path = exp.config.model.clone();

    let (ir_path_resolved, _tmpfile) = resolve_ir_path(&model_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    let ir_json = std::fs::read_to_string(&ir_path_resolved).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", ir_path_resolved, e);
        std::process::exit(1);
    });
    let mhash = model_hash(&ir_json);

    let base_params: HashMap<String, f64> = if let Some(ref pf) = exp.config.params {
        load_params_toml(pf).unwrap_or_else(|e| {
            eprintln!("error: cannot load params {}: {}", pf, e);
            std::process::exit(1);
        })
    } else {
        HashMap::new()
    };
    let shash = sim_hash(&mhash, &canonical_params(&base_params), &backend, dt);

    let seeds = exp.config.seeds.resolve().unwrap_or_else(|e| {
        eprintln!("error resolving seeds: {}", e);
        std::process::exit(1);
    });

    // Validate [sweep] and [design.*] are mutually exclusive.
    if !exp.sweep.is_empty() && !exp.design.is_empty() {
        eprintln!("error: [sweep] and [design.*] are mutually exclusive.");
        eprintln!("  [sweep] — deterministic grid for specific parameter values");
        eprintln!("  [design.*] — space-filling for sensitivity/VOI analysis");
        eprintln!("  Use one or the other in a single experiment file.");
        std::process::exit(1);
    }

    let params_file_opt = exp.config.params.clone();

    // Expand [design.*] blocks into parameter points (writes parameter_points.tsv per design).
    if !exp.design.is_empty() {
        // Resolve scenarios before consuming exp
        let scenarios: Vec<ScenarioEntry> = if exp.scenario.is_empty() {
            vec![ScenarioEntry { name: "baseline".to_string(), params: HashMap::new(), enable: vec![], disable: vec![] }]
        } else {
            exp.scenario.clone()
        };
        run_design_experiment(scenarios, exp.design, &ir_path_resolved, &output_dir, &shash,
                              &backend, dt, force, parallel, &params_file_opt, &seeds);
        return;
    }

    // Expand [sweep] into parameter points (empty sweep → one null point).
    let sweep_points = expand_sweep(&exp.sweep);
    let has_sweep = !exp.sweep.is_empty();

    let scenarios: Vec<ScenarioEntry> = if exp.scenario.is_empty() {
        vec![ScenarioEntry { name: "baseline".to_string(), params: HashMap::new(), enable: vec![], disable: vec![] }]
    } else {
        exp.scenario
    };

    let runs_dir = format!("{}/runs", output_dir);
    std::fs::create_dir_all(&runs_dir).unwrap_or_else(|e| {
        eprintln!("error: cannot create output dir {}: {}", runs_dir, e);
        std::process::exit(1);
    });

    std::fs::write(format!("{}/model.ir.json", output_dir), &ir_json).unwrap_or_else(|e| {
        eprintln!("warning: could not write model.ir.json: {}", e);
    });

    let geo_url: Option<String> = if let Some(ref geo_src) = exp.config.geo {
        let geo_dest = format!("{}/geo/boundaries.geojson", output_dir);
        match std::fs::create_dir_all(format!("{}/geo", output_dir))
            .and_then(|_| std::fs::copy(geo_src, &geo_dest))
        {
            Ok(_) => Some("geo/boundaries.geojson".to_string()),
            Err(e) => { eprintln!("warning: could not copy geo file '{}': {}", geo_src, e); None }
        }
    } else {
        None
    };

    if has_sweep {
        eprintln!("Sweep: {} parameter points", sweep_points.len());
        for (i, pt) in sweep_points.iter().enumerate().take(3) {
            let desc: Vec<String> = pt.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            eprintln!("  point {}: {}", i, desc.join(", "));
        }
        if sweep_points.len() > 3 {
            eprintln!("  ... ({} more)", sweep_points.len() - 3);
        }
    }

    // Classify every (sweep_point, scenario, seed) triple before touching the thread pool.
    let plans = plan_runs(&scenarios, &sweep_points, &seeds, &shash, &runs_dir, force);
    let total = plans.len();
    let scenario_names: Vec<String> = scenarios.iter().map(|s| s.name.clone()).collect();

    let counter = Arc::new(AtomicUsize::new(0));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(if parallel > 0 { parallel } else { 1 })
        .build()
        .unwrap_or_else(|e| { eprintln!("error building thread pool: {}", e); std::process::exit(1); });

    let results: Vec<Result<RunEntry, String>> = pool.install(|| {
        plans.par_iter().map(|plan| {
            if plan.decision == RunDecision::CacheHit {
                let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!("[{}/{}] scenario={} seed={} (skipped — already exists)", n, total, plan.scenario, plan.seed);
                return Ok(RunEntry { scenario: plan.scenario.clone(), seed: plan.seed, run_path: plan.run_path.clone() });
            }

            // Build per-run overrides: sweep point (M layer) + scenario params (σ layer)
            let sc = scenarios.iter().find(|s| s.name == plan.scenario).unwrap();
            let mut overrides_map: HashMap<String, f64> = plan.sweep_overrides.clone();
            // Scenario params overlay sweep params (scenario σ layer is after M layer)
            overrides_map.extend(sc.params.iter().map(|(k, v)| (k.clone(), *v)));

            let sim_run = SimRun {
                ir_path: ir_path_resolved.clone(),
                params_files: params_file_opt.as_ref().map(|p| vec![p.clone()]).unwrap_or_default(),
                overrides: overrides_map,
                scenario_name: None,
                adhoc_enable: sc.enable.clone(),
                adhoc_disable: sc.disable.clone(),
                backend: backend.clone(),
                dt,
                seed: plan.seed,
                ..Default::default()
            };

            match run_simulation(&sim_run) {
                Err(e) => {
                    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!("[{}/{}] scenario={} seed={} ERROR: {}", n, total, plan.scenario, plan.seed, e);
                    Err(format!("scenario={} seed={}: {}", plan.scenario, plan.seed, e))
                }
                Ok((traj, model)) => {
                    if let Err(e) = std::fs::create_dir_all(&plan.run_dir) {
                        return Err(format!("cannot create {}: {}", plan.run_dir, e));
                    }
                    if let Err(e) = write_traj_tsv(&format!("{}/traj.tsv", plan.run_dir), &model, &traj, false) {
                        return Err(format!("cannot write traj.tsv in {}: {}", plan.run_dir, e));
                    }
                    let mut merged_params = plan.sweep_overrides.clone();
                    merged_params.extend(sc.params.iter().map(|(k, v)| (k.clone(), *v)));
                    let meta = RunMeta {
                        scenario: plan.scenario.clone(),
                        seed: plan.seed,
                        sim_hash: shash.clone(),
                        scen_hash: scen_hash(&sc.enable, &sc.disable, &merged_params),
                        model_hash: mhash.clone(),
                    };
                    let meta_path = format!("{}/run.json", plan.run_dir);
                    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap_or_default())
                        .map_err(|e| format!("cannot write {}: {}", meta_path, e))?;

                    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!("[{}/{}] scenario={} seed={}", n, total, plan.scenario, plan.seed);
                    Ok(RunEntry { scenario: plan.scenario.clone(), seed: plan.seed, run_path: plan.run_path.clone() })
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
        for e in &errors { eprintln!("  {}", e); }
    }

    let completed = completed_runs.len();
    let manifest = Manifest {
        model: model_path,
        scenarios: scenario_names,
        seeds,
        total_runs: total,
        completed,
        output_dir: output_dir.clone(),
        geo: geo_url,
        runs: completed_runs,
    };
    let manifest_path = format!("{}/manifest.json", output_dir);
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap_or_default())
        .unwrap_or_else(|e| eprintln!("warning: could not write manifest.json: {}", e));

    eprintln!("Done: {}/{} runs completed. Manifest: {}", completed, total, manifest_path);
    if !errors.is_empty() { std::process::exit(1); }
}

// ─── Design experiment execution ─────────────────────────────────────────────

/// Run a design-based experiment (VOI/sensitivity analysis).
///
/// For each named design:
///   1. Generate parameter points via the specified method (sobol/lhs/random)
///   2. Write `{output_dir}/designs/{design}/parameter_points.tsv`
///   3. Run all (point, scenario, seed) combinations
///   4. Collect summary outputs → `outputs.tsv` (prepared for `camdl experiment analyze`)
#[allow(clippy::too_many_arguments)]
fn run_design_experiment(
    scenarios: Vec<ScenarioEntry>,
    designs: HashMap<String, DesignBlock>,
    ir_path: &str,
    output_dir: &str,
    shash: &str,
    backend: &str,
    dt: f64,
    force: bool,
    parallel: usize,
    params_file_opt: &Option<String>,
    seeds: &[u64],
) {
    // Sort design names for deterministic output
    let mut design_names: Vec<&String> = designs.keys().collect();
    design_names.sort();

    for design_name in &design_names {
        let block = &designs[*design_name];
        eprintln!("Design '{}': method={} n={} parameters={}",
            design_name, block.method, block.n, block.parameters.len());

        // Build sorted parameter list
        let mut param_names: Vec<&String> = block.parameters.keys().collect();
        param_names.sort();
        let params: Vec<(String, DesignParam)> = param_names.iter().map(|name| {
            let p = &block.parameters[*name];
            ((*name).clone(), DesignParam {
                min: p.range.min,
                max: p.range.max,
                transform: p.transform.clone(),
                prior: p.prior.clone(),
            })
        }).collect();

        // Generate design points
        let design_result = generate_design(&params, block.n, &block.method);
        let n_points = design_result.points.len();
        eprintln!("  Generated {} parameter points", n_points);

        // Write parameter_points.tsv
        let design_dir = format!("{}/designs/{}", output_dir, design_name);
        std::fs::create_dir_all(&design_dir).unwrap_or_else(|e| {
            eprintln!("error: cannot create design dir {}: {}", design_dir, e);
            std::process::exit(1);
        });

        let pts_path = format!("{}/parameter_points.tsv", design_dir);
        let mut pts_tsv = String::new();
        // Header: point_id + sorted param names
        pts_tsv.push_str("point_id");
        for name in &design_result.param_names {
            pts_tsv.push('\t');
            pts_tsv.push_str(name);
        }
        pts_tsv.push('\n');
        for (i, pt) in design_result.points.iter().enumerate() {
            pts_tsv.push_str(&i.to_string());
            for name in &design_result.param_names {
                pts_tsv.push('\t');
                pts_tsv.push_str(&format!("{:.8}", pt[name]));
            }
            pts_tsv.push('\n');
        }
        std::fs::write(&pts_path, &pts_tsv).unwrap_or_else(|e| {
            eprintln!("warning: could not write {}: {}", pts_path, e);
        });
        eprintln!("  Wrote {}", pts_path);

        // Write priors.txt if any parameter has a prior specification
        let priors_txt = build_priors_txt(&params);
        if let Some(txt) = priors_txt {
            let priors_path = format!("{}/priors.txt", design_dir);
            let _ = std::fs::write(&priors_path, txt);
        }

        // Run all (point, scenario, seed) combinations
        let runs_dir = format!("{}/designs/{}/runs", output_dir, design_name);
        std::fs::create_dir_all(&runs_dir).unwrap_or_else(|e| {
            eprintln!("error: cannot create runs dir {}: {}", runs_dir, e);
            std::process::exit(1);
        });

        // Annotate each point with its index for run.json
        let sweep_points = &design_result.points;
        let plans = plan_runs(&scenarios, sweep_points, &seeds, shash, &runs_dir, force);
        let total = plans.len();
        let counter = Arc::new(AtomicUsize::new(0));

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(if parallel > 0 { parallel } else { 1 })
            .build()
            .unwrap_or_else(|e| { eprintln!("error building thread pool: {}", e); std::process::exit(1); });

        pool.install(|| {
            plans.par_iter().for_each(|plan| {
                if plan.decision == RunDecision::CacheHit {
                    counter.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let sc = scenarios.iter().find(|s| s.name == plan.scenario).unwrap();
                let mut overrides_map: HashMap<String, f64> = plan.sweep_overrides.clone();
                overrides_map.extend(sc.params.iter().map(|(k, v)| (k.clone(), *v)));

                let sim_run = SimRun {
                    ir_path: ir_path.to_string(),
                    params_files: params_file_opt.as_ref().map(|p| vec![p.clone()]).unwrap_or_default(),
                    overrides: overrides_map,
                    scenario_name: None,
                    adhoc_enable: sc.enable.clone(),
                    adhoc_disable: sc.disable.clone(),
                    backend: backend.to_string(),
                    dt,
                    seed: plan.seed,
                    ..Default::default()
                };

                match run_simulation(&sim_run) {
                    Err(e) => {
                        let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                        eprintln!("[{}/{}] design={} scenario={} seed={} ERROR: {}",
                            n, total, design_name, plan.scenario, plan.seed, e);
                    }
                    Ok((traj, model)) => {
                        if let Err(e) = std::fs::create_dir_all(&plan.run_dir) {
                            eprintln!("error: cannot create {}: {}", plan.run_dir, e);
                            return;
                        }
                        if let Err(e) = write_traj_tsv(&format!("{}/traj.tsv", plan.run_dir), &model, &traj, false) {
                            eprintln!("error: cannot write traj.tsv in {}: {}", plan.run_dir, e);
                            return;
                        }
                        // Write run.json so analyze can recover design_point_index
                        // without re-hashing parameter values.
                        let run_json = format!("{{\"design_point_index\":{}}}\n", plan.point_idx);
                        let _ = std::fs::write(&format!("{}/run.json", plan.run_dir), run_json);
                        let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                        eprintln!("[{}/{}] design={} scenario={} seed={}", n, total, design_name, plan.scenario, plan.seed);
                    }
                }
            });
        });
        eprintln!("Design '{}' complete.", design_name);
    }
}

// ─── Prior spec helpers ───────────────────────────────────────────────────────

/// Build human-readable priors.txt content for a design's parameter list.
/// Returns None if no parameters have prior specifications.
fn build_priors_txt(params: &[(String, DesignParam)]) -> Option<String> {
    let with_priors: Vec<&(String, DesignParam)> = params.iter()
        .filter(|(_, p)| p.prior.is_some())
        .collect();
    if with_priors.is_empty() {
        return None;
    }
    let mut txt = String::from("Parameter priors:\n\n");
    for (name, param) in params {
        let prior_desc = match &param.prior {
            Some(p) => p.describe(),
            None => "Uniform (no prior specified)".to_string(),
        };
        let transform_desc = match param.transform.as_deref() {
            Some("log") => " [log-uniform sampling]",
            Some("logit") => " [logit-uniform sampling]",
            _ => "",
        };
        txt.push_str(&format!("  {}: {} over [{}, {}]{}\n",
            name, prior_desc, param.min, param.max, transform_desc));
    }
    txt.push('\n');
    txt.push_str("These priors are used by the VOI tool (camdl voi run) for importance\n");
    txt.push_str("weighting. If no prior is specified for a parameter, uniform is assumed.\n");
    Some(txt)
}

// ─── cmd_experiment_status ───────────────────────────────────────────────────

pub fn cmd_experiment_status(args: &[String]) {
    let mut toml_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            s if s.starts_with("--") => { eprintln!("unknown flag: {}", s); experiment_usage(); }
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

    let output_dir   = exp.config.output_dir.clone();
    let manifest_path = format!("{}/manifest.json", output_dir);

    if let Ok(src) = std::fs::read_to_string(&manifest_path) {
        if let Ok(manifest) = serde_json::from_str::<Manifest>(&src) {
            println!("Experiment status for: {}", toml_path);
            println!("  Model:      {}", manifest.model);
            println!("  Output dir: {}", manifest.output_dir);
            println!("  Scenarios:  {}", manifest.scenarios.join(", "));
            println!("  Seeds:      {} total ({:?}..={:?})",
                manifest.seeds.len(),
                manifest.seeds.first().unwrap_or(&0),
                manifest.seeds.last().unwrap_or(&0));
            println!("  Completed:  {}/{}", manifest.completed, manifest.total_runs);

            if let Ok(ir_json) = std::fs::read_to_string(&exp.config.model) {
                let mhash   = model_hash(&ir_json);
                let base_params: HashMap<String, f64> = exp.config.params.as_ref()
                    .and_then(|p| load_params_toml(p).ok())
                    .unwrap_or_default();
                let shash   = sim_hash(&mhash, &canonical_params(&base_params), &exp.config.backend, exp.config.dt);
                let scenarios: Vec<ScenarioEntry> = if exp.scenario.is_empty() {
                    vec![ScenarioEntry { name: "baseline".to_string(), params: HashMap::new(), enable: vec![], disable: vec![] }]
                } else {
                    exp.scenario
                };
                let seeds   = exp.config.seeds.resolve().unwrap_or_default();
                let sweep_points = expand_sweep(&exp.sweep);
                let runs_dir = format!("{}/runs", output_dir);
                let plans   = plan_runs(&scenarios, &sweep_points, &seeds, &shash, &runs_dir, false);
                let live_hits = plans.iter().filter(|p| p.decision == RunDecision::CacheHit).count();
                println!("  Live count: {}/{} traj.tsv files present", live_hits, plans.len());
            }
            return;
        }
    }

    println!("Experiment status for: {}", toml_path);
    println!("  No manifest.json found at {}", manifest_path);
    println!("  Run 'camdl experiment run {}' to start.", toml_path);
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(name: &str) -> ScenarioEntry {
        ScenarioEntry { name: name.to_string(), params: HashMap::new(), enable: vec![], disable: vec![] }
    }

    fn sc_enable(name: &str, enables: &[&str]) -> ScenarioEntry {
        ScenarioEntry {
            name: name.to_string(),
            params: HashMap::new(),
            enable: enables.iter().map(|s| s.to_string()).collect(),
            disable: vec![],
        }
    }

    fn sc_params(name: &str, kv: &[(&str, f64)]) -> ScenarioEntry {
        ScenarioEntry {
            name: name.to_string(),
            params: kv.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            enable: vec![],
            disable: vec![],
        }
    }

    fn seed_traj(run_dir: &str) {
        std::fs::create_dir_all(run_dir).unwrap();
        std::fs::write(format!("{}/traj.tsv", run_dir), "t\n").unwrap();
    }

    fn no_sweep() -> Vec<HashMap<String, f64>> { vec![HashMap::new()] }

    fn sweep1(kv: &[(&str, f64)]) -> Vec<HashMap<String, f64>> {
        vec![kv.iter().map(|(k, v)| (k.to_string(), *v)).collect()]
    }

    // ── basic classification ─────────────────────────────────────────────────

    #[test]
    fn all_miss_on_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let plans = plan_runs(&[sc("baseline"), sc("with_sia")], &no_sweep(), &[1, 2, 3],
            "aaaa1111bbbb2222", dir.path().to_str().unwrap(), false);
        assert_eq!(plans.len(), 6);
        assert!(plans.iter().all(|p| p.decision == RunDecision::CacheMiss));
    }

    #[test]
    fn hit_when_traj_exists() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        // First pass to learn the path
        let plans = plan_runs(&[sc("baseline")], &no_sweep(), &[1, 2], "aaaa1111bbbb2222", runs_dir, false);
        seed_traj(&plans[0].run_dir); // seed 1 only
        // Re-classify
        let plans = plan_runs(&[sc("baseline")], &no_sweep(), &[1, 2], "aaaa1111bbbb2222", runs_dir, false);
        assert_eq!(plans[0].decision, RunDecision::CacheHit,  "seed 1 should be a hit");
        assert_eq!(plans[1].decision, RunDecision::CacheMiss, "seed 2 should be a miss");
    }

    #[test]
    fn force_ignores_existing_traj() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        let plans = plan_runs(&[sc("baseline")], &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, false);
        seed_traj(&plans[0].run_dir);
        let plans = plan_runs(&[sc("baseline")], &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, true);
        assert_eq!(plans[0].decision, RunDecision::CacheMiss);
    }

    // ── sim_hash invalidation ────────────────────────────────────────────────

    #[test]
    fn sim_hash_change_invalidates_all() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        // Populate under old sim_hash
        let old = plan_runs(&[sc("baseline")], &no_sweep(), &[1, 2], "aaaa1111bbbb2222", runs_dir, false);
        for p in &old { seed_traj(&p.run_dir); }
        // New sim_hash → different tier, all miss
        let new = plan_runs(&[sc("baseline")], &no_sweep(), &[1, 2], "cccc3333dddd4444", runs_dir, false);
        assert!(new.iter().all(|p| p.decision == RunDecision::CacheMiss));
        // Old paths unchanged
        for p in &old {
            assert!(std::path::Path::new(&format!("{}/traj.tsv", p.run_dir)).exists());
        }
    }

    // ── scen_hash invalidation ───────────────────────────────────────────────

    #[test]
    fn scen_change_invalidates_only_that_scenario() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        let scenarios = vec![sc("baseline"), sc_enable("with_sia", &["sia_r1"])];
        // Populate all runs
        let plans = plan_runs(&scenarios, &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, false);
        for p in &plans { seed_traj(&p.run_dir); }
        // Change only with_sia's enable list
        let new_scenarios = vec![sc("baseline"), sc_enable("with_sia", &["sia_r1", "sia_r2"])];
        let new = plan_runs(&new_scenarios, &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, false);
        let baseline = new.iter().find(|p| p.scenario == "baseline").unwrap();
        let with_sia = new.iter().find(|p| p.scenario == "with_sia").unwrap();
        assert_eq!(baseline.decision, RunDecision::CacheHit,  "baseline must be reused");
        assert_eq!(with_sia.decision, RunDecision::CacheMiss, "with_sia must be invalidated");
    }

    #[test]
    fn scen_param_change_invalidates_only_that_scenario() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        let scenarios = vec![sc("baseline"), sc_params("variant", &[("vacc_frac", 0.7)])];
        let plans = plan_runs(&scenarios, &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, false);
        for p in &plans { seed_traj(&p.run_dir); }
        let new_scenarios = vec![sc("baseline"), sc_params("variant", &[("vacc_frac", 0.9)])];
        let new = plan_runs(&new_scenarios, &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, false);
        assert_eq!(new.iter().find(|p| p.scenario == "baseline").unwrap().decision, RunDecision::CacheHit);
        assert_eq!(new.iter().find(|p| p.scenario == "variant").unwrap().decision, RunDecision::CacheMiss);
    }

    // ── seed extension ───────────────────────────────────────────────────────

    #[test]
    fn adding_seeds_reuses_existing() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        // Populate seeds 1-3
        let plans = plan_runs(&[sc("baseline")], &no_sweep(), &[1, 2, 3], "aaaa1111bbbb2222", runs_dir, false);
        for p in &plans { seed_traj(&p.run_dir); }
        // Extend to seeds 1-5
        let plans = plan_runs(&[sc("baseline")], &no_sweep(), &[1, 2, 3, 4, 5], "aaaa1111bbbb2222", runs_dir, false);
        let (hits, misses): (Vec<_>, Vec<_>) = plans.iter()
            .partition(|p| p.decision == RunDecision::CacheHit);
        assert_eq!(hits.len(), 3,   "seeds 1-3 must be reused");
        assert_eq!(misses.len(), 2, "seeds 4-5 must be new");
        let miss_seeds: Vec<u64> = misses.iter().map(|p| p.seed).collect();
        assert!(miss_seeds.contains(&4) && miss_seeds.contains(&5));
    }

    // ── run_path structure ───────────────────────────────────────────────────

    #[test]
    fn run_path_format() {
        let dir = tempfile::tempdir().unwrap();
        let plans = plan_runs(&[sc("with sia!")], &no_sweep(), &[42], "aaaa1111bbbb2222", dir.path().to_str().unwrap(), false);
        // sim_hash_8 / slug-scen_hash_8 / seed_N
        let parts: Vec<&str> = plans[0].run_path.splitn(3, '/').collect();
        assert_eq!(parts[0], "aaaa1111",            "sim_hash_8");
        assert!(parts[1].starts_with("with_sia_"),  "slug must sanitize spaces and '!'");
        assert_eq!(parts[2], "seed_42",             "seed component");
    }

    #[test]
    fn rename_scenario_same_semantics_same_scen_hash() {
        // Two scenarios with identical overrides but different names share the same
        // scen_hash suffix — demonstrating that renaming doesn't create a new cache entry
        // for semantically identical runs.
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        let p1 = plan_runs(&[sc_enable("old_name", &["sia"])], &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, false);
        let p2 = plan_runs(&[sc_enable("new_name", &["sia"])], &no_sweep(), &[1], "aaaa1111bbbb2222", runs_dir, false);
        // Slugs differ but scen_hash_8 (embedded in dir name) is identical
        let hash1: &str = p1[0].run_path.splitn(3, '/').nth(1).unwrap().splitn(2, '-').nth(1).unwrap();
        let hash2: &str = p2[0].run_path.splitn(3, '/').nth(1).unwrap().splitn(2, '-').nth(1).unwrap();
        assert_eq!(hash1, hash2, "same enables/params → same scen_hash_8");
    }

    // ── sweep expansion ──────────────────────────────────────────────────────

    #[test]
    fn sweep_linspace_expansion() {
        let mut sweep = HashMap::new();
        sweep.insert("x".to_string(), SweepSpec::Linspace {
            linspace: LinspaceSpec { min: 0.0, max: 1.0, n: 5 }
        });
        let points = expand_sweep(&sweep);
        assert_eq!(points.len(), 5);
        let vals: Vec<f64> = points.iter().map(|p| p["x"]).collect();
        assert!((vals[0] - 0.0).abs() < 1e-10);
        assert!((vals[2] - 0.5).abs() < 1e-10);
        assert!((vals[4] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn sweep_list_expansion() {
        let mut sweep = HashMap::new();
        sweep.insert("y".to_string(), SweepSpec::List(vec![1.0, 2.0, 4.0]));
        let points = expand_sweep(&sweep);
        assert_eq!(points.len(), 3);
        let vals: Vec<f64> = points.iter().map(|p| p["y"]).collect();
        assert_eq!(vals, vec![1.0, 2.0, 4.0]);
    }

    #[test]
    fn sweep_cartesian_product() {
        let mut sweep = HashMap::new();
        sweep.insert("a".to_string(), SweepSpec::List(vec![1.0, 2.0]));
        sweep.insert("b".to_string(), SweepSpec::List(vec![10.0, 20.0]));
        let points = expand_sweep(&sweep);
        assert_eq!(points.len(), 4, "2 × 2 = 4");
    }

    #[test]
    fn sweep_empty_returns_one_null_point() {
        let sweep = HashMap::new();
        let points = expand_sweep(&sweep);
        assert_eq!(points.len(), 1);
        assert!(points[0].is_empty());
    }

    #[test]
    fn sweep_changes_scen_hash() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().to_str().unwrap();
        let pt1 = sweep1(&[("vacc_eff", 0.3)]);
        let pt2 = sweep1(&[("vacc_eff", 0.7)]);
        let p1 = plan_runs(&[sc("baseline")], &pt1, &[1], "aaaa1111bbbb2222", runs_dir, false);
        let p2 = plan_runs(&[sc("baseline")], &pt2, &[1], "aaaa1111bbbb2222", runs_dir, false);
        // Different sweep values → different scen_hash → different directories
        assert_ne!(p1[0].run_path, p2[0].run_path, "distinct sweep points must produce distinct paths");
    }

    #[test]
    fn sweep_count_correct() {
        let dir = tempfile::tempdir().unwrap();
        let mut sweep = HashMap::new();
        sweep.insert("x".to_string(), SweepSpec::Linspace {
            linspace: LinspaceSpec { min: 0.0, max: 1.0, n: 5 }
        });
        let points = expand_sweep(&sweep);
        // 5 sweep × 2 scenarios × 3 seeds = 30
        let plans = plan_runs(&[sc("baseline"), sc("with_sia")], &points, &[1, 2, 3],
            "aaaa1111bbbb2222", dir.path().to_str().unwrap(), false);
        assert_eq!(plans.len(), 30);
    }
}
