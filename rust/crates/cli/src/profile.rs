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


pub fn cmd_profile(a: &crate::args::ProfileArgs) {
    let ir_path = a.model.to_string_lossy().into_owned();
    let data_path = a.data.to_string_lossy().into_owned();
    let n_particles = a.inference.particles;
    let n_iterations = a.iterations;
    let n_starts = a.starts;
    let cooling = a.cooling;
    let dt = a.inference.dt;
    let seed = a.inference.seed;
    let parallel = a.inference.parallel;
    let output_path: Option<String> = a.output.as_ref().map(|p| p.to_string_lossy().into_owned());
    let scenario_name = a.scenario.scenario.clone();
    let flow_name = a.flow.flow.clone();
    let overrides: HashMap<String, f64> = a.model_overrides.param.iter()
        .map(|p| (p.name.clone(), p.value))
        .collect();

    // focal names come from the sweep specs; grid values inline
    let focal_names: Vec<String> = a.sweep.iter().map(|s| s.name.clone()).collect();

    struct FocalGrid { name: String, values: Vec<f64>, param_idx: usize }
    let mut focal_grids: Vec<FocalGrid> = Vec::new();

    let rw_sd = a.rw_sd.as_ref().unwrap_or_else(|| {
        eprintln!("error: --rw-sd required (e.g., --rw-sd \"sigma=0.01\" or --rw-sd auto)");
        std::process::exit(1);
    });
    let rw_sd_auto = matches!(rw_sd, crate::args::types::RwSd::Auto);
    let rw_sd_map_raw: HashMap<String, Option<f64>> = match rw_sd {
        crate::args::types::RwSd::Auto => HashMap::new(),
        crate::args::types::RwSd::Map(m) => m.clone(),
    };

    // Load model
    let (mut model, _model_json) = crate::util::load_model(&ir_path)
        .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });

    for pf in &a.model_overrides.params {
        crate::util::apply_params_file(&mut model, &pf.to_string_lossy()).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); });
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

    for sw in &a.sweep {
        let idx = compiled.param_index.get(sw.name.as_str()).copied()
            .unwrap_or_else(|| { eprintln!("focal parameter '{}' not found", sw.name); std::process::exit(1); });
        focal_grids.push(FocalGrid { name: sw.name.clone(), values: sw.values.clone(), param_idx: idx });
    }

    let fixed_names: std::collections::HashSet<String> = a.fixed.iter().cloned().collect();

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
            let projection = if flow_name.is_some() {
                sim::inference::multi_stream_obs::StreamProjection::FlowSum(flow_indices.to_vec())
            } else {
                sim::inference::multi_stream_obs::StreamProjection::from_ir(
                    &obs.projection, &compiled, &obs.name,
                ).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
            };
            Arc::new(MultiStreamObsModel::new(
                vec![StreamSpec {
                    projection,
                    ir_model: obs.clone(),
                    observations: observations.iter().map(|o| o.value).collect(),
                    obs_times: observations.iter().map(|o| o.time).collect(),
                }],
                compiled.clone(),
            ).unwrap_or_else(|e| {
                eprintln!("error: observation model construction failed: {:?}", e);
                std::process::exit(1);
            }))
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
    let mp = MultiProgress::with_draw_target(crate::progress::draw_target());
    let overall_style = ProgressStyle::with_template(
        "  {prefix:>12} {bar:40.cyan/dim} {pos:>3}/{len:3} {msg}"
    ).unwrap().progress_chars("━╸─");
    let overall_pb = mp.add(ProgressBar::new(total_jobs as u64));
    overall_pb.set_style(overall_style);
    overall_pb.set_prefix("profile");

    // Plain-mode fallback (GH #14): throttled log lines since bars are
    // hidden under --progress plain. Shared across rayon workers via
    // Mutex — contention is negligible at the per-job cadence. Cadence
    // from progress::DEFAULT_THROTTLE.
    let plain = crate::progress::is_plain();
    let throttle = std::sync::Mutex::new(crate::progress::Throttle::default());
    if plain {
        log::info!("profile: {} grid points × {} starts = {} jobs",
            grid_points.len(), n_starts, total_jobs);
    }

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
                // Profile doesn't surface ic_free; it's a 2D β-γ scan
                // assuming a committed initial state.
                skip_first_obs_from_loglik: false,
            };
            let job_seed = seed ^ (grid_idx as u64 * 1000 + start_idx as u64);

            let result = run_if2(
                &*process, &*obs_model_obj, &params, &if2_params, &config, job_seed,
            );

            overall_pb.inc(1);
            if plain {
                let done = overall_pb.position();
                if throttle.lock().map(|mut t| t.ready()).unwrap_or(true) || done == total_jobs as u64 {
                    log::info!("profile: {}/{} jobs complete", done, total_jobs);
                }
            }

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
