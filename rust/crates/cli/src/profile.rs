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
    inference::{
        if2::{run_if2, IF2Config, Observation},
        ParticleState,
        ChainBinomialProcess, MultiStreamObsModel,
        multi_stream_obs::StreamSpec,
        traits::ObservationModel,
    },
};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;


pub fn cmd_profile(args: &[String]) {
    let mut ir_path: Option<String> = None;
    let mut params_files: Vec<String> = Vec::new();
    let mut data_path: Option<String> = None;
    let mut focal_str: Option<String> = None;
    let mut grid_str: Option<String> = None;
    let mut named_grids: HashMap<String, String> = HashMap::new();
    let mut n_particles = 1000_usize;
    let mut n_iterations = 50_usize;
    let mut n_starts = 3_usize;
    let mut cooling = 0.95_f64;
    let mut dt = 1.0_f64;
    let mut seed = 1_u64;
    let mut parallel = 0_usize; // 0 = rayon default (num_cpus)
    let mut overrides: HashMap<String, f64> = HashMap::new();
    let mut scenario_name: Option<String> = None;
    let mut _obs_model = "negbin".to_string();
    let mut _tol = 0.0_f64;
    let mut flow_name: Option<String> = None;
    let mut rw_sd_str: Option<String> = None;
    let mut fixed_str: Option<String> = None;
    let mut output_path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--params"     => { i += 1; params_files.push(args[i].clone()); }
            "--data"       => { i += 1; data_path = Some(args[i].clone()); }
            "--focal"      => { i += 1; focal_str = Some(args[i].clone()); }
            "--grid"       => { i += 1; grid_str = Some(args[i].clone()); }
            s if s.starts_with("--grid-") => {
                let name = s.strip_prefix("--grid-").unwrap().to_string();
                i += 1;
                named_grids.insert(name, args[i].clone());
            }
            "--particles"  => { i += 1; n_particles = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs integer"); std::process::exit(1); }); }
            "--iterations" => { i += 1; n_iterations = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs integer"); std::process::exit(1); }); }
            "--starts"     => { i += 1; n_starts = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs integer"); std::process::exit(1); }); }
            "--cooling"    => { i += 1; cooling = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs number"); std::process::exit(1); }); }
            "--dt"         => { i += 1; dt = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs number"); std::process::exit(1); }); }
            "--seed"       => { i += 1; seed = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs integer"); std::process::exit(1); }); }
            "--parallel"   => { i += 1; parallel = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs integer"); std::process::exit(1); }); }
            "--scenario"   => { i += 1; scenario_name = Some(args[i].clone()); }
            "--obs-model"  => { i += 1; _obs_model = args[i].clone(); }
            "--tol"        => { i += 1; _tol = args[i].parse().unwrap_or_else(|_| { eprintln!("error: needs number"); std::process::exit(1); }); }
            "--flow"       => { i += 1; flow_name = Some(args[i].clone()); }
            "--rw-sd"      => { i += 1; rw_sd_str = Some(args[i].clone()); }
            "--fixed"      => { i += 1; fixed_str = Some(args[i].clone()); }
            "--output" | "-o" => { i += 1; output_path = Some(args[i].clone()); }
            "--param"      => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap().to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| { eprintln!("error: --param needs NAME=VALUE"); std::process::exit(1); });
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
        eprintln!("  2D:  --focal alpha,gamma --grid-alpha \"0.9,0.95,1.0\" --grid-gamma \"0.06,0.08,0.10\"");
        std::process::exit(1);
    });
    let data_path = data_path.unwrap_or_else(|| { eprintln!("--data required"); std::process::exit(1); });
    let focal_str = focal_str.unwrap_or_else(|| { eprintln!("--focal required"); std::process::exit(1); });
    let rw_sd_str = rw_sd_str.unwrap_or_else(|| { eprintln!("--rw-sd required"); std::process::exit(1); });

    // Parse focal parameter(s) and their grids
    let focal_names: Vec<String> = focal_str.split(',').map(|s| s.trim().to_string()).collect();

    // Build per-focal grids. For 1D: --grid "values". For 2D+: --grid-NAME "values".
    struct FocalGrid { name: String, values: Vec<f64>, param_idx: usize }
    let mut focal_grids: Vec<FocalGrid> = Vec::new();

    // Parse rw_sd — supports "auto" and "name=value,name=auto" forms
    let rw_sd_auto = rw_sd_str.trim() == "auto";
    let rw_sd_map_raw: HashMap<String, Option<f64>> = if rw_sd_auto {
        HashMap::new()
    } else {
        rw_sd_str.split(',')
            .map(|kv| {
                let mut parts = kv.trim().splitn(2, '=');
                let k = parts.next().unwrap().to_string();
                let v_str = parts.next().unwrap_or("auto");
                let v: Option<f64> = if v_str == "auto" { None } else {
                    Some(v_str.parse().unwrap_or_else(|_| {
                        eprintln!("bad --rw-sd entry: {}", kv); std::process::exit(1);
                    }))
                };
                (k, v)
            })
            .collect()
    };

    // Load model
    let (mut model, _model_json) = crate::util::load_model(&ir_path)
        .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });

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
    let flow_indices = crate::util::resolve_flow_indices(&model, flow_name.as_deref())
        .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });
    let flow_indices = Arc::new(flow_indices);

    // Build focal grids with param indices
    for name in &focal_names {
        let idx = compiled.param_index.get(name.as_str()).copied()
            .unwrap_or_else(|| { eprintln!("focal parameter '{}' not found", name); std::process::exit(1); });
        let grid_values = if focal_names.len() == 1 {
            // 1D: use --grid
            let gs = grid_str.as_ref().unwrap_or_else(|| { eprintln!("--grid required for 1D profile"); std::process::exit(1); });
            gs.split(',').map(|s| s.trim().parse().unwrap_or_else(|_| { eprintln!("error: grid values must be numbers"); std::process::exit(1); })).collect()
        } else {
            // 2D+: use --grid-NAME
            let gs = named_grids.get(name).unwrap_or_else(|| {
                eprintln!("--grid-{} required for multi-focal profile", name); std::process::exit(1);
            });
            gs.split(',').map(|s| s.trim().parse().unwrap_or_else(|_| { eprintln!("error: grid values must be numbers"); std::process::exit(1); })).collect()
        };
        focal_grids.push(FocalGrid { name: name.clone(), values: grid_values, param_idx: idx });
    }

    // Parse --fixed "N0,mu,rho,..."
    let fixed_names: std::collections::HashSet<String> = fixed_str
        .map(|s| s.split(',').map(|n| n.trim().to_string()).collect())
        .unwrap_or_default();

    // Build IF2 param specs (excluding focal + fixed params)
    // Focal params are fixed at grid values by the profile loop.
    // Fixed params are held constant at their --params values.
    let exclude: std::collections::HashSet<String> = focal_names.iter()
        .chain(fixed_names.iter())
        .cloned()
        .collect();

    let param_names_to_estimate: Vec<String> = if rw_sd_auto {
        model.parameters.iter()
            .filter(|p| !exclude.contains(&p.name))
            .filter(|p| compiled.param_index.contains_key(p.name.as_str()))
            .map(|p| p.name.clone())
            .collect()
    } else {
        rw_sd_map_raw.keys()
            .filter(|name| !exclude.contains(*name))
            .cloned()
            .collect()
    };

    let specs: Vec<crate::fit::runner::ParamSpec> = param_names_to_estimate.iter().map(|name| {
        crate::fit::runner::ParamSpec {
            name: name.clone(),
            rw_sd: rw_sd_map_raw.get(name).and_then(|v| *v),
            transform: None,
            ivp: false,
            start: None,
        }
    }).collect();

    let if2_params = crate::fit::runner::build_if2_params_from_specs(
        &model, &compiled, &base_params, &specs,
    ).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });
    let if2_params = Arc::new(if2_params);

    // Build process + observation model via traits
    let process = Arc::new(ChainBinomialProcess::new(compiled.clone()));
    let obs_model_obj: Arc<dyn ObservationModel<ParticleState> + Send + Sync> = {
        let obs_block = model.observations.first();
        if let Some(obs) = obs_block {
            eprintln!("profile: using observation model '{}' from IR", obs.name);
            Arc::new(MultiStreamObsModel::new(
                vec![StreamSpec {
                    flow_indices: flow_indices.to_vec(),
                    ir_model: obs.clone(),
                    observations: observations.iter().map(|o| o.value).collect(),
                    obs_times: observations.iter().map(|o| o.time).collect(),
                }],
                compiled.clone(),
            ))
        } else {
            eprintln!("error: model has no observations block");
            std::process::exit(1);
        }
    };

    // Build Cartesian product of all focal grids.
    // Each job is a Vec<(param_idx, value)> for the focal params at that grid point.
    let mut grid_points: Vec<Vec<(usize, f64)>> = vec![vec![]];
    for fg in &focal_grids {
        let mut expanded = Vec::new();
        for existing in &grid_points {
            for &val in &fg.values {
                let mut point = existing.clone();
                point.push((fg.param_idx, val));
                expanded.push(point);
            }
        }
        grid_points = expanded;
    }

    let total_jobs = grid_points.len() * n_starts;
    let dim_str = focal_grids.iter().map(|fg| format!("{}={}", fg.name, fg.values.len())).collect::<Vec<_>>().join(" × ");
    eprintln!("profile: {} grid ({}) × {} starts = {} IF2 runs ({} particles × {} iter each)",
        grid_points.len(), dim_str, n_starts, total_jobs, n_particles, n_iterations);

    // ── Progress bar ─────────────────────────────────────────────────────
    let mp = MultiProgress::new();
    let overall_style = ProgressStyle::with_template(
        "  {prefix:>12} {bar:40.cyan/dim} {pos:>3}/{len:3} {msg}"
    ).unwrap().progress_chars("━╸─");
    let overall_pb = mp.add(ProgressBar::new(total_jobs as u64));
    overall_pb.set_style(overall_style);
    overall_pb.set_prefix("profile");

    // Initialize rayon global pool (controls all parallelism: grid jobs + particles).
    if parallel > 0 {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(parallel)
            .build_global();
    }

    // Job list: (grid_point_idx, start_idx)
    let jobs: Vec<(usize, usize)> = (0..grid_points.len())
        .flat_map(|gi| (0..n_starts).map(move |si| (gi, si)))
        .collect();

    let results: Vec<(usize, Vec<f64>, f64, Vec<f64>)> = {
        jobs.par_iter().map(|&(grid_idx, start_idx)| {
            let process = Arc::clone(&process);
            let obs_model_obj = Arc::clone(&obs_model_obj);
            let if2_params = Arc::clone(&if2_params);
            let focal_values: Vec<f64> = grid_points[grid_idx].iter().map(|&(_, v)| v).collect();

            // Set focal parameters
            let mut params = base_params.clone();
            for &(idx, val) in &grid_points[grid_idx] {
                params[idx] = val;
            }

            let config = IF2Config {
                n_particles, n_iterations,
                cooling_fraction: cooling, cooling_target_iters: n_iterations, dt,
                t_start: process.compiled.model.simulation.t_start,
                simplex_groups: vec![],
            };
            let job_seed = seed ^ (grid_idx as u64 * 1000 + start_idx as u64);

            let result = run_if2(
                &*process, &*obs_model_obj, &params, &if2_params, &config, job_seed,
            );

            overall_pb.inc(1);

            match result {
                Ok(r) => (grid_idx, focal_values, r.final_loglik, r.mle),
                Err(_) => (grid_idx, focal_values, f64::NEG_INFINITY, params),
            }
        }).collect()
    };

    overall_pb.finish_with_message("done");

    // ── Aggregate: best loglik per grid point across starts ──────────────
    let mut best: HashMap<usize, (Vec<f64>, f64, Vec<f64>)> = HashMap::new();
    for (grid_idx, focal_vals, loglik, mle_params) in results {
        let entry = best.entry(grid_idx).or_insert((focal_vals.clone(), f64::NEG_INFINITY, vec![]));
        if loglik > entry.1 {
            *entry = (focal_vals, loglik, mle_params);
        }
    }

    // ── Output TSV ───────────────────────────────────────────────────────
    let mut out: Box<dyn std::io::Write> = match &output_path {
        Some(path) => {
            let f = std::fs::File::create(path)
                .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", path, e); std::process::exit(1); });
            Box::new(std::io::BufWriter::new(f))
        }
        None => Box::new(std::io::stdout().lock()),
    };

    writeln!(out, "# {}", crate::version::VERSION).unwrap();
    for fg in &focal_grids { write!(out, "{}\t", fg.name).unwrap(); }
    write!(out, "max_loglik").unwrap();
    for spec in if2_params.iter() { write!(out, "\t{}", spec.name).unwrap(); }
    writeln!(out).unwrap();

    let mut sorted: Vec<_> = best.into_iter().collect();
    sorted.sort_by_key(|&(idx, _)| idx);

    for (_, (focal_vals, loglik, mle_params)) in sorted {
        for v in &focal_vals { write!(out, "{:.4}\t", v).unwrap(); }
        write!(out, "{:.2}", loglik).unwrap();
        for spec in if2_params.iter() { write!(out, "\t{:.6}", mle_params[spec.index]).unwrap(); }
        writeln!(out).unwrap();
    }

    if let Some(ref path) = output_path {
        eprintln!("profile written to {}", path);
    }
}
