//! `camdl profile` — profile likelihood via parallel IF2 runs.
//!
//! For a focal parameter, fix it at a grid of values and run IF2 to
//! maximize over the remaining parameters at each grid point. The
//! profile likelihood shows how the MLE changes as you move the focal
//! parameter — revealing identifiability, confidence intervals, and
//! parameter interactions.
//!
//! Usage:
//!   camdl profile MODEL --params P.toml --data cases.tsv \
//!       --focal R0 --grid "10,20,30,40,50,60,70,80" \
//!       --rw-sd "sigma=0.01,gamma=0.01,rho=0.02" \
//!       --particles 1000 --iterations 50 --starts 3 \
//!       --parallel 4 --dt 1.0 --seed 1
//!
//! Output: profile_{focal}.tsv with columns:
//!   focal_value  max_loglik  [all estimated param means]

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sim::{
    compiled_model::CompiledModel,
    chain_binomial::step_one,
    inference::{
        obs_loglik::{negbin_logpmf, discretized_normal_logpmf_tol, DEFAULT_TOL},
        if2::{run_if2, IF2Config, IF2Param, Observation, Transform},
        ParticleState,
    },
    ekrng::StatefulRng,
};
use std::collections::HashMap;
use std::sync::Arc;

/// One profile point: a focal value + IF2 result.
struct ProfilePoint {
    focal_value: f64,
    best_loglik: f64,
    best_params: Vec<f64>,
}

pub fn cmd_profile(args: &[String]) {
    let mut ir_path: Option<String> = None;
    let mut params_files: Vec<String> = Vec::new();
    let mut data_path: Option<String> = None;
    let mut focal_name: Option<String> = None;
    let mut grid_str: Option<String> = None;
    let mut n_particles = 1000_usize;
    let mut n_iterations = 50_usize;
    let mut n_starts = 3_usize;
    let mut cooling = 0.95_f64;
    let mut dt = 1.0_f64;
    let mut seed = 1_u64;
    let mut parallel = 4_usize;
    let mut overrides: HashMap<String, f64> = HashMap::new();
    let mut scenario_name: Option<String> = None;
    let mut obs_model = "negbin".to_string();
    let mut tol = DEFAULT_TOL;
    let mut flow_name: Option<String> = None;
    let mut rw_sd_str: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--params"     => { i += 1; params_files.push(args[i].clone()); }
            "--data"       => { i += 1; data_path = Some(args[i].clone()); }
            "--focal"      => { i += 1; focal_name = Some(args[i].clone()); }
            "--grid"       => { i += 1; grid_str = Some(args[i].clone()); }
            "--particles"  => { i += 1; n_particles = args[i].parse().expect("needs integer"); }
            "--iterations" => { i += 1; n_iterations = args[i].parse().expect("needs integer"); }
            "--starts"     => { i += 1; n_starts = args[i].parse().expect("needs integer"); }
            "--cooling"    => { i += 1; cooling = args[i].parse().expect("needs number"); }
            "--dt"         => { i += 1; dt = args[i].parse().expect("needs number"); }
            "--seed"       => { i += 1; seed = args[i].parse().expect("needs integer"); }
            "--parallel"   => { i += 1; parallel = args[i].parse().expect("needs integer"); }
            "--scenario"   => { i += 1; scenario_name = Some(args[i].clone()); }
            "--obs-model"  => { i += 1; obs_model = args[i].clone(); }
            "--tol"        => { i += 1; tol = args[i].parse().expect("needs number"); }
            "--flow"       => { i += 1; flow_name = Some(args[i].clone()); }
            "--rw-sd"      => { i += 1; rw_sd_str = Some(args[i].clone()); }
            "--param"      => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap().to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok()).expect("--param needs NAME=VALUE");
                overrides.insert(k, v);
            }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s); std::process::exit(1);
            }
            path => { ir_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let ir_path = ir_path.unwrap_or_else(|| {
        eprintln!("usage: camdl profile MODEL --focal R0 --grid \"10,20,30\" --rw-sd \"sigma=0.01\" ...");
        std::process::exit(1);
    });
    let data_path = data_path.unwrap_or_else(|| { eprintln!("--data required"); std::process::exit(1); });
    let focal_name = focal_name.unwrap_or_else(|| { eprintln!("--focal required"); std::process::exit(1); });
    let grid_str = grid_str.unwrap_or_else(|| { eprintln!("--grid required"); std::process::exit(1); });
    let rw_sd_str = rw_sd_str.unwrap_or_else(|| { eprintln!("--rw-sd required"); std::process::exit(1); });

    // Parse grid
    let grid: Vec<f64> = grid_str.split(',')
        .map(|s| s.trim().parse().expect("grid values must be numbers"))
        .collect();

    // Parse rw_sd
    let rw_sd_map: HashMap<String, f64> = rw_sd_str.split(',')
        .map(|kv| {
            let mut parts = kv.trim().splitn(2, '=');
            let k = parts.next().unwrap().to_string();
            let v: f64 = parts.next().and_then(|s| s.parse().ok()).expect("bad rw-sd");
            (k, v)
        })
        .collect();

    // Load model
    let mut model: ir::Model = if ir_path.ends_with(".camdl") {
        let camdlc = std::env::var("CAMDLC").unwrap_or_else(|_| "camdlc".into());
        let output = std::process::Command::new(&camdlc).arg(&ir_path).output()
            .unwrap_or_else(|e| { eprintln!("cannot run camdlc: {}", e); std::process::exit(1); });
        if !output.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&output.stderr)); std::process::exit(1);
        }
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); })
    } else {
        let contents = std::fs::read_to_string(&ir_path).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); });
        serde_json::from_str(&contents).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); })
    };

    for pf in &params_files {
        crate::util::apply_params_file(&mut model, pf).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); });
    }
    if let Some(ref name) = scenario_name {
        if let Some(preset) = model.presets.iter().find(|p| p.name == *name) {
            for p in &mut model.parameters { if let Some(&v) = preset.params.get(&p.name) { p.value = Some(v); } }
        }
    }
    for p in &mut model.parameters { if let Some(&v) = overrides.get(&p.name) { p.value = Some(v); } }

    let compiled = Arc::new(CompiledModel::new(model.clone()).unwrap_or_else(|e| { eprintln!("{:?}", e); std::process::exit(1); }));
    let base_params = compiled.default_params.clone();

    let observations: Vec<Observation> = crate::pfilter::load_data_tsv_pub(&data_path)
        .unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); })
        .into_iter().map(|o| Observation { time: o.time, value: o.value }).collect();
    let observations = Arc::new(observations);

    // Flow indices
    let flow_indices: Vec<usize> = if let Some(ref name) = flow_name {
        model.transitions.iter().enumerate()
            .filter(|(_, tr)| tr.name == *name || tr.name.starts_with(&format!("{}_", name)))
            .map(|(i, _)| i).collect()
    } else {
        model.transitions.iter().enumerate()
            .filter(|(_, tr)| tr.metadata.as_ref().and_then(|m| m.origin_kind.as_deref()).map_or(false, |k| k == "transmission"))
            .map(|(i, _)| i).collect()
    };
    let flow_indices = Arc::new(flow_indices);

    // Focal parameter index
    let focal_idx = compiled.param_index.get(focal_name.as_str()).copied()
        .unwrap_or_else(|| { eprintln!("focal parameter '{}' not found", focal_name); std::process::exit(1); });

    // Build IF2 param specs (excluding focal)
    let if2_params: Vec<IF2Param> = rw_sd_map.iter()
        .filter(|(name, _)| name.as_str() != focal_name.as_str())
        .map(|(name, &rw_sd)| {
            let idx = compiled.param_index.get(name.as_str()).copied()
                .unwrap_or_else(|| { eprintln!("rw-sd param '{}' not found", name); std::process::exit(1); });
            let ir_param = model.parameters.iter().find(|p| p.name == *name).unwrap();
            let (transform, lower, upper) = match ir_param.transform {
                Some(ir::parameter::Transform::Log) => (Transform::Log, 0.0, f64::INFINITY),
                Some(ir::parameter::Transform::Logit) => { let (lo, hi) = ir_param.bounds.unwrap_or((0.0, 1.0)); (Transform::Logit, lo, hi) }
                _ => if let Some((lo, hi)) = ir_param.bounds {
                    if lo == 0.0 && hi == 1.0 { (Transform::Logit, 0.0, 1.0) } else { (Transform::Log, lo, hi) }
                } else { (Transform::Log, 0.0, f64::INFINITY) },
            };
            IF2Param { name: name.clone(), index: idx, initial: base_params[idx], rw_sd, transform, lower, upper }
        })
        .collect();
    let if2_params = Arc::new(if2_params);

    // Obs model params
    let rho_idx = compiled.param_index.get("rho").copied();
    let k_idx = compiled.param_index.get("k").copied();
    let psi_idx = compiled.param_index.get("psi").copied();

    let total_jobs = grid.len() * n_starts;
    eprintln!("profile: {} grid points × {} starts = {} IF2 runs ({} particles × {} iterations each)",
        grid.len(), n_starts, total_jobs, n_particles, n_iterations);

    // ── Progress bars ────────────────────────────────────────────────────
    let mp = MultiProgress::new();
    let overall_style = ProgressStyle::with_template(
        "  {prefix:>12} {bar:40.cyan/dim} {pos:>3}/{len:3} {msg}"
    ).unwrap().progress_chars("━╸─");

    let overall_pb = mp.add(ProgressBar::new(total_jobs as u64));
    overall_pb.set_style(overall_style);
    overall_pb.set_prefix("profile");

    // ── Parallel IF2 runs ────────────────────────────────────────────────
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(parallel)
        .build().unwrap();

    // Build job list: (grid_idx, start_idx, focal_value)
    let jobs: Vec<(usize, usize, f64)> = grid.iter().enumerate()
        .flat_map(|(gi, &fv)| (0..n_starts).map(move |si| (gi, si, fv)))
        .collect();

    let results: Vec<(usize, f64, f64, Vec<f64>)> = pool.install(|| {
        jobs.par_iter().map(|&(grid_idx, start_idx, focal_value)| {
            let compiled = Arc::clone(&compiled);
            let observations = Arc::clone(&observations);
            let flow_indices = Arc::clone(&flow_indices);
            let if2_params = Arc::clone(&if2_params);

            // Set focal parameter
            let mut params = base_params.clone();
            params[focal_idx] = focal_value;

            let config = IF2Config {
                n_particles,
                n_iterations,
                cooling_fraction: cooling,
                cooling_target_iters: 50,
                dt,
            };

            let job_seed = seed ^ (grid_idx as u64 * 1000 + start_idx as u64);

            let step_fn = |state: &mut ParticleState, p: &[f64], t: f64, step_dt: f64, rng: &mut StatefulRng| {
                step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, p, t, step_dt, rng)
            };
            let project_fn = |state: &ParticleState| -> f64 {
                flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
            };
            let dmeasure_fn: Box<dyn Fn(f64, f64, &[f64]) -> f64 + Send> = match obs_model.as_str() {
                "negbin" => Box::new(move |proj: f64, obs: f64, p: &[f64]| {
                    let rho = rho_idx.map_or(1.0, |i| p[i]);
                    let k = k_idx.map_or(10.0, |i| p[i]);
                    negbin_logpmf(obs, rho * proj, k)
                }),
                "discretized_normal" => Box::new(move |proj: f64, obs: f64, p: &[f64]| {
                    let rho = rho_idx.map_or(1.0, |i| p[i]);
                    let psi = psi_idx.map_or(0.116, |i| p[i]);
                    let mu = rho * proj;
                    discretized_normal_logpmf_tol(obs, mu, mu * (1.0 - rho + psi * psi * mu), tol)
                }),
                _ => unreachable!(),
            };

            let result = run_if2(
                &compiled, &params, &if2_params, &observations, &config,
                &step_fn, &project_fn, &*dmeasure_fn, job_seed,
            );

            overall_pb.inc(1);
            let msg = format!("{}={:.1}", focal_name, focal_value);
            overall_pb.set_message(msg);

            match result {
                Ok(r) => (grid_idx, focal_value, r.final_loglik, r.mle),
                Err(_) => (grid_idx, focal_value, f64::NEG_INFINITY, params),
            }
        }).collect()
    });

    overall_pb.finish_with_message("done");

    // ── Aggregate: best loglik per grid point across starts ──────────────
    let mut best_per_grid: HashMap<usize, ProfilePoint> = HashMap::new();
    for (grid_idx, focal_value, loglik, params) in results {
        let entry = best_per_grid.entry(grid_idx).or_insert(ProfilePoint {
            focal_value,
            best_loglik: f64::NEG_INFINITY,
            best_params: vec![],
        });
        if loglik > entry.best_loglik {
            entry.best_loglik = loglik;
            entry.best_params = params;
        }
    }

    // ── Output TSV ───────────────────────────────────────────────────────
    print!("{}\tmax_loglik", focal_name);
    for spec in if2_params.iter() {
        print!("\t{}", spec.name);
    }
    println!();

    let mut sorted: Vec<_> = best_per_grid.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    for (_, point) in sorted {
        print!("{:.4}\t{:.2}", point.focal_value, point.best_loglik);
        for spec in if2_params.iter() {
            print!("\t{:.6}", point.best_params[spec.index]);
        }
        println!();
    }
}
