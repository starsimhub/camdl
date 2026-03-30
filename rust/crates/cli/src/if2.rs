//! `camdl if2` — iterated filtering for maximum likelihood estimation.
//!
//! Usage:
//!   camdl if2 MODEL --params P.toml --data cases.tsv \
//!       --particles 2000 --iterations 100 --cooling 0.95 \
//!       --rw-sd "R0=5,sigma=0.01,gamma=0.01" \
//!       --fixed "N0,mu" --dt 1.0 --seed 1
//!
//! Output: parameter_traces.tsv to stdout, log-likelihood trace to stderr.

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

pub fn cmd_if2(args: &[String]) {
    let mut ir_path: Option<String> = None;
    let mut params_files: Vec<String> = Vec::new();
    let mut data_path: Option<String> = None;
    let mut n_particles = 2000_usize;
    let mut n_iterations = 100_usize;
    let mut cooling = 0.95_f64;
    let mut dt = 1.0_f64;
    let mut seed = 1_u64;
    let mut overrides: HashMap<String, f64> = HashMap::new();
    let mut scenario_name: Option<String> = None;
    let mut adhoc_enable: Vec<String> = Vec::new();
    let mut obs_model = "negbin".to_string();
    let mut tol = DEFAULT_TOL;
    let mut flow_name: Option<String> = None;
    let mut rw_sd_str: Option<String> = None;
    let mut fixed_str: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--params"     => { i += 1; params_files.push(args[i].clone()); }
            "--data"       => { i += 1; data_path = Some(args[i].clone()); }
            "--particles"  => { i += 1; n_particles = args[i].parse().expect("--particles needs integer"); }
            "--iterations" => { i += 1; n_iterations = args[i].parse().expect("--iterations needs integer"); }
            "--cooling"    => { i += 1; cooling = args[i].parse().expect("--cooling needs number"); }
            "--dt"         => { i += 1; dt = args[i].parse().expect("--dt needs number"); }
            "--seed"       => { i += 1; seed = args[i].parse().expect("--seed needs integer"); }
            "--scenario"   => { i += 1; scenario_name = Some(args[i].clone()); }
            "--enable"     => { i += 1; adhoc_enable.push(args[i].clone()); }
            "--obs-model"  => { i += 1; obs_model = args[i].clone(); }
            "--tol"        => { i += 1; tol = args[i].parse().expect("--tol needs number"); }
            "--flow"       => { i += 1; flow_name = Some(args[i].clone()); }
            "--rw-sd"      => { i += 1; rw_sd_str = Some(args[i].clone()); }
            "--fixed"      => { i += 1; fixed_str = Some(args[i].clone()); }
            "--param"      => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap().to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok()).expect("--param needs NAME=VALUE");
                overrides.insert(k, v);
            }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                std::process::exit(1);
            }
            path => { ir_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let ir_path = ir_path.unwrap_or_else(|| {
        eprintln!("usage: camdl if2 MODEL --params P.toml --data cases.tsv --rw-sd \"R0=5,gamma=0.01\" --particles 2000 --iterations 100");
        std::process::exit(1);
    });
    let data_path = data_path.unwrap_or_else(|| {
        eprintln!("error: --data required"); std::process::exit(1);
    });
    let rw_sd_str = rw_sd_str.unwrap_or_else(|| {
        eprintln!("error: --rw-sd required (e.g., --rw-sd \"R0=5,sigma=0.01\")"); std::process::exit(1);
    });

    // Parse --rw-sd "R0=5,sigma=0.01,gamma=0.01"
    let rw_sd_map: HashMap<String, f64> = rw_sd_str.split(',')
        .map(|kv| {
            let mut parts = kv.trim().splitn(2, '=');
            let k = parts.next().unwrap().to_string();
            let v: f64 = parts.next().and_then(|s| s.parse().ok())
                .unwrap_or_else(|| { eprintln!("bad --rw-sd entry: {}", kv); std::process::exit(1); });
            (k, v)
        })
        .collect();

    // Parse --fixed "N0,mu,k"
    let fixed_set: std::collections::HashSet<String> = fixed_str
        .map(|s| s.split(',').map(|n| n.trim().to_string()).collect())
        .unwrap_or_default();

    // Load model
    let mut model: ir::Model = if ir_path.ends_with(".camdl") {
        let camdlc = std::env::var("CAMDLC").unwrap_or_else(|_| "camdlc".into());
        let output = std::process::Command::new(&camdlc).arg(&ir_path).output()
            .unwrap_or_else(|e| { eprintln!("cannot run camdlc: {}", e); std::process::exit(1); });
        if !output.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&output.stderr)); std::process::exit(1);
        }
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| { eprintln!("parse error: {}", e); std::process::exit(1); })
    } else {
        let contents = std::fs::read_to_string(&ir_path)
            .unwrap_or_else(|e| { eprintln!("cannot read {}: {}", ir_path, e); std::process::exit(1); });
        serde_json::from_str(&contents)
            .unwrap_or_else(|e| { eprintln!("parse error: {}", e); std::process::exit(1); })
    };

    // Apply params files
    for pf in &params_files {
        crate::util::apply_params_file(&mut model, pf)
            .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });
    }

    // Apply scenario
    if let Some(ref name) = scenario_name {
        if let Some(preset) = model.presets.iter().find(|p| p.name == *name) {
            for p in &mut model.parameters {
                if let Some(&v) = preset.params.get(&p.name) { p.value = Some(v); }
            }
        }
    }

    // Apply overrides
    for p in &mut model.parameters {
        if let Some(&v) = overrides.get(&p.name) { p.value = Some(v); }
    }

    let compiled = CompiledModel::new(model.clone())
        .unwrap_or_else(|e| { eprintln!("compile error: {:?}", e); std::process::exit(1); });
    let params = compiled.default_params.clone();

    // Load data
    let observations: Vec<Observation> = crate::pfilter::load_data_tsv_pub(&data_path)
        .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
        .into_iter()
        .map(|o| Observation { time: o.time, value: o.value })
        .collect();

    // Flow indices for projection
    let flow_indices: Vec<usize> = if let Some(ref name) = flow_name {
        model.transitions.iter().enumerate()
            .filter(|(_, tr)| tr.name == *name || tr.name.starts_with(&format!("{}_", name)))
            .map(|(i, _)| i).collect()
    } else {
        model.transitions.iter().enumerate()
            .filter(|(_, tr)| tr.metadata.as_ref()
                .and_then(|m| m.origin_kind.as_deref())
                .map_or(false, |k| k == "transmission"))
            .map(|(i, _)| i).collect()
    };
    if flow_indices.is_empty() {
        eprintln!("error: no projection flows found; use --flow NAME"); std::process::exit(1);
    }

    // Build IF2Param specs from --rw-sd
    let mut if2_params: Vec<IF2Param> = Vec::new();
    for (name, &rw_sd) in &rw_sd_map {
        let idx = compiled.param_index.get(name.as_str()).copied()
            .unwrap_or_else(|| { eprintln!("error: --rw-sd parameter '{}' not in model", name); std::process::exit(1); });

        // Derive transform from parameter type
        let ir_param = model.parameters.iter().find(|p| p.name == *name).unwrap();
        let (transform, lower, upper) = match ir_param.transform {
            Some(ir::parameter::Transform::Log) => (Transform::Log, 0.0, f64::INFINITY),
            Some(ir::parameter::Transform::Logit) => {
                let (lo, hi) = ir_param.bounds.unwrap_or((0.0, 1.0));
                (Transform::Logit, lo, hi)
            }
            _ => {
                // Infer from bounds
                if let Some((lo, hi)) = ir_param.bounds {
                    if lo == 0.0 && hi == 1.0 {
                        (Transform::Logit, 0.0, 1.0)
                    } else if lo >= 0.0 {
                        (Transform::Log, lo, hi)
                    } else {
                        (Transform::None, lo, hi)
                    }
                } else {
                    (Transform::Log, 0.0, f64::INFINITY)
                }
            }
        };

        if2_params.push(IF2Param {
            name: name.clone(),
            index: idx,
            initial: params[idx],
            rw_sd,
            transform,
            lower,
            upper,
        });
    }

    // Observation model parameters
    let rho_idx = compiled.param_index.get("rho").copied();
    let k_idx = compiled.param_index.get("k").copied();
    let psi_idx = compiled.param_index.get("psi").copied();

    eprintln!("if2: {} observations, {} particles, {} iterations, cooling={}, dt={}, seed={}",
        observations.len(), n_particles, n_iterations, cooling, dt, seed);
    eprintln!("if2: estimating {} parameters, {} fixed",
        if2_params.len(), fixed_set.len());

    // Parameter scale diagnostics: warn if rw_sd / value ratios are extreme
    for spec in &if2_params {
        let value = params[spec.index];
        if value.abs() < 1e-10 { continue; } // skip zero-valued params
        let ratio = spec.rw_sd / value.abs();
        if ratio > 0.5 {
            eprintln!("warning: rw_sd for '{}' is {:.0}% of its value ({:.4}) — \
                perturbations may be too large. Consider reducing --rw-sd {}={:.4}",
                spec.name, ratio * 100.0, value, spec.name, value.abs() * 0.1);
        }
        if ratio < 0.001 {
            eprintln!("warning: rw_sd for '{}' is {:.2}% of its value ({:.4}) — \
                perturbations may be too small to explore. Consider increasing --rw-sd {}={:.4}",
                spec.name, ratio * 100.0, value, spec.name, value.abs() * 0.05);
        }
    }

    let config = IF2Config {
        n_particles,
        n_iterations,
        cooling_fraction: cooling,
        cooling_target_iters: 50, // matches pomp's cooling.fraction.50
        dt,
    };

    // Step function: takes per-particle params
    let step_fn = |state: &mut ParticleState, p: &[f64], t: f64, step_dt: f64, rng: &mut StatefulRng| {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, p, t, step_dt, rng)
    };

    let project_fn = |state: &ParticleState| -> f64 {
        flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
    };

    // dmeasure takes (projected, observed, params) — params needed for rho/k/psi
    let dmeasure_fn: Box<dyn Fn(f64, f64, &[f64]) -> f64> = match obs_model.as_str() {
        "negbin" => Box::new(move |projected: f64, observed: f64, p: &[f64]| {
            let rho = rho_idx.map_or(1.0, |i| p[i]);
            let k = k_idx.map_or(10.0, |i| p[i]);
            negbin_logpmf(observed, rho * projected, k)
        }),
        "discretized_normal" => Box::new(move |projected: f64, observed: f64, p: &[f64]| {
            let rho = rho_idx.map_or(1.0, |i| p[i]);
            let psi = psi_idx.map_or(0.116, |i| p[i]);
            let mu = rho * projected;
            let variance = mu * (1.0 - rho + psi * psi * mu);
            discretized_normal_logpmf_tol(observed, mu, variance, tol)
        }),
        other => { eprintln!("unknown --obs-model '{}'", other); std::process::exit(1); }
    };

    let result = run_if2(
        &compiled, &params, &if2_params, &observations, &config,
        &step_fn, &project_fn, &*dmeasure_fn, seed,
    ).unwrap_or_else(|e| {
        eprintln!("if2 error: {:?}", e); std::process::exit(1);
    });

    // Output: parameter traces TSV to stdout
    // Header
    print!("iteration\tloglik");
    for spec in &if2_params {
        print!("\t{}", spec.name);
    }
    println!();

    for iter_result in &result.iterations {
        print!("{}\t{:.2}", iter_result.iteration, iter_result.log_likelihood);
        for spec in &if2_params {
            print!("\t{:.6}", iter_result.param_means[spec.index]);
        }
        println!();
    }

    // Final MLE to stderr
    eprintln!("\nIF2 converged. Final estimates:");
    for spec in &if2_params {
        eprintln!("  {} = {:.6}", spec.name, result.mle[spec.index]);
    }
    eprintln!("  loglik = {:.2}", result.final_loglik);
}
