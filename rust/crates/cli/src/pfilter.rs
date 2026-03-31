//! `camdl pfilter` — bootstrap particle filter for log-likelihood estimation.
//!
//! Usage:
//!   camdl pfilter MODEL --params P.toml --data cases.tsv \
//!       --particles 5000 --dt 1.0 --seed 1
//!
//! Output: log-likelihood estimate to stdout.
//! With --trace: per-observation TSV (time, ll_increment, ESS).

use sim::{
    compiled_model::CompiledModel,
    chain_binomial::step_one,
    inference::{
        bootstrap_filter,
        obs_loglik::negbin_logpmf,
        particle_filter::Observation,
    },
};
use std::collections::HashMap;

pub fn cmd_pfilter(args: &[String]) {
    let mut ir_path: Option<String> = None;
    let mut params_files: Vec<String> = Vec::new();
    let mut data_path: Option<String> = None;
    let mut n_particles = 1000_usize;
    let mut dt = 1.0_f64;
    let mut seed = 1_u64;
    let mut trace_path: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut overrides: HashMap<String, f64> = HashMap::new();
    let mut scenario_name: Option<String> = None;
    let mut adhoc_enable: Vec<String> = Vec::new();
    let mut obs_model = "negbin".to_string(); // "negbin" or "discretized_normal"
    let mut tol = sim::inference::obs_loglik::DEFAULT_TOL;
    let mut flow_name: Option<String> = None; // --flow recovery → project that transition
    let mut save_final_state: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--params"    => { i += 1; params_files.push(args[i].clone()); }
            "--data"      => { i += 1; data_path = Some(args[i].clone()); }
            "--particles" => { i += 1; n_particles = args[i].parse().expect("--particles needs an integer"); }
            "--dt"        => { i += 1; dt = args[i].parse().expect("--dt needs a number"); }
            "--seed"      => { i += 1; seed = args[i].parse().expect("--seed needs an integer"); }
            "--trace"     => { i += 1; trace_path = Some(args[i].clone()); }
            "--output" | "-o" => { i += 1; output_path = Some(args[i].clone()); }
            "--scenario"  => { i += 1; scenario_name = Some(args[i].clone()); }
            "--enable"    => { i += 1; adhoc_enable.push(args[i].clone()); }
            "--obs-model" => { i += 1; obs_model = args[i].clone(); }
            "--tol"       => { i += 1; tol = args[i].parse().expect("--tol needs a number"); }
            "--flow"      => { i += 1; flow_name = Some(args[i].clone()); }
            "--save-final-state" => { i += 1; save_final_state = Some(args[i].clone()); }
            "--param"     => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap().to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok()).expect("--param needs NAME=VALUE");
                overrides.insert(k, v);
            }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                eprintln!("usage: camdl pfilter MODEL --params P.toml --data cases.tsv --particles 5000 --dt 1.0 --seed 1");
                std::process::exit(1);
            }
            path => { ir_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let ir_path = ir_path.unwrap_or_else(|| {
        eprintln!("usage: camdl pfilter MODEL --params P.toml --data cases.tsv --particles 5000");
        std::process::exit(1);
    });
    let data_path = data_path.unwrap_or_else(|| {
        eprintln!("error: --data required");
        std::process::exit(1);
    });

    // Load model (supports .camdl via camdlc)
    let mut model: ir::Model = if ir_path.ends_with(".camdl") {
        let camdlc = std::env::var("CAMDLC").unwrap_or_else(|_| "camdlc".into());
        let output = std::process::Command::new(&camdlc).arg(&ir_path).output()
            .unwrap_or_else(|e| { eprintln!("cannot run camdlc: {}", e); std::process::exit(1); });
        if !output.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            std::process::exit(1);
        }
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| { eprintln!("cannot parse camdlc output: {}", e); std::process::exit(1); })
    } else {
        let contents = std::fs::read_to_string(&ir_path)
            .unwrap_or_else(|e| { eprintln!("cannot read {}: {}", ir_path, e); std::process::exit(1); });
        serde_json::from_str(&contents)
            .unwrap_or_else(|e| { eprintln!("cannot parse {}: {}", ir_path, e); std::process::exit(1); })
    };

    // Apply params
    for pf in &params_files {
        crate::util::apply_params_file(&mut model, pf)
            .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });
    }

    // Apply scenario preset params
    if let Some(ref name) = scenario_name {
        if let Some(preset) = model.presets.iter().find(|p| p.name == *name) {
            for p in &mut model.parameters {
                if let Some(&v) = preset.params.get(&p.name) { p.value = Some(v); }
            }
            // Enable interventions from scenario
            let enable_names: Vec<String> = preset.enable.clone();
            model.interventions.retain(|iv| {
                enable_names.iter().any(|en| {
                    iv.name == *en || iv.base_name.as_deref() == Some(en.as_str())
                })
            });
        } else {
            eprintln!("error: scenario '{}' not found", name);
            std::process::exit(1);
        }
    }
    // Ad-hoc enable
    if !adhoc_enable.is_empty() {
        model.interventions.retain(|iv| {
            adhoc_enable.iter().any(|en| {
                iv.name == *en || iv.base_name.as_deref() == Some(en.as_str())
            })
        });
    }

    // Apply overrides
    for p in &mut model.parameters {
        if let Some(&v) = overrides.get(&p.name) { p.value = Some(v); }
    }

    let compiled = CompiledModel::new(model.clone())
        .unwrap_or_else(|e| { eprintln!("compile error: {:?}", e); std::process::exit(1); });
    let params = compiled.default_params.clone();

    // Load data
    let observations = load_data_tsv(&data_path)
        .unwrap_or_else(|e| { eprintln!("error loading data: {}", e); std::process::exit(1); });

    eprintln!("pfilter: {} observations, {} particles, dt={}, seed={}",
        observations.len(), n_particles, dt, seed);

    // Find the observation projection: sum of flows for the specified transition(s).
    // --flow NAME: project incidence of that transition (e.g., --flow recovery).
    // Default: project all transitions with origin_kind = "transmission".
    let flow_indices: Vec<usize> = if let Some(ref name) = flow_name {
        let indices: Vec<usize> = model.transitions.iter().enumerate()
            .filter(|(_, tr)| tr.name == *name || tr.name.starts_with(&format!("{}_", name)))
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            eprintln!("error: no transition named '{}' found. Available: {}",
                name, model.transitions.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "));
            std::process::exit(1);
        }
        eprintln!("pfilter: projecting incidence({}) → {} flow(s)", name, indices.len());
        indices
    } else {
        let indices: Vec<usize> = model.transitions.iter().enumerate()
            .filter(|(_, tr)| {
                tr.metadata.as_ref()
                    .and_then(|m| m.origin_kind.as_deref())
                    .map_or(false, |k| k == "transmission")
            })
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            eprintln!("warning: no transmission transitions found; use --flow NAME to specify projection");
            vec![0]
        } else {
            indices
        }
    };

    // Get observation model parameters
    let rho_idx = compiled.param_index.get("rho").copied();
    let k_idx = compiled.param_index.get("k").copied();
    let psi_idx = compiled.param_index.get("psi").copied();

    eprintln!("pfilter: obs_model={}", obs_model);

    // Run particle filter
    let step_fn = |state: &mut sim::inference::ParticleState, t: f64, step_dt: f64, rng: &mut sim::ekrng::StatefulRng, scratch: &mut sim::chain_binomial::StepScratch| -> Result<(), sim::error::SimError> {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, &params, t, step_dt, rng, scratch)
    };

    let project_fn = |state: &sim::inference::ParticleState| -> f64 {
        flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
    };

    let dmeasure_fn: Box<dyn Fn(f64, f64) -> f64> = match obs_model.as_str() {
        "negbin" => {
            let rho = rho_idx.map_or(1.0, |i| params[i]);
            let k = k_idx.map_or(10.0, |i| params[i]);
            Box::new(move |projected: f64, observed: f64| -> f64 {
                let mu = rho * projected;
                negbin_logpmf(observed, mu, k)
            })
        }
        "discretized_normal" => {
            use sim::inference::obs_loglik::discretized_normal_logpmf_tol;
            let rho = rho_idx.map_or(1.0, |i| params[i]);
            let psi = psi_idx.map_or(0.116, |i| params[i]);
            Box::new(move |projected: f64, observed: f64| -> f64 {
                let mu = rho * projected;
                let variance = mu * (1.0 - rho + psi * psi * mu);
                discretized_normal_logpmf_tol(observed, mu, variance, tol)
            })
        }
        other => {
            eprintln!("error: unknown --obs-model '{}'. Use 'negbin' or 'discretized_normal'", other);
            std::process::exit(1);
        }
    };

    let result = bootstrap_filter(
        &compiled, &params, &observations, n_particles, dt,
        &step_fn, &project_fn, &*dmeasure_fn, seed,
    ).unwrap_or_else(|e| {
        eprintln!("pfilter error: {:?}", e);
        std::process::exit(1);
    });

    // Write trace diagnostics
    if let Some(ref path) = trace_path {
        let mut f = std::fs::File::create(path)
            .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", path, e); std::process::exit(1); });
        writeln!(f, "time\tll_increment\tESS\tpred_mean\tpred_q05\tpred_q50\tpred_q95\tobserved").unwrap();
        for (i, obs) in observations.iter().enumerate() {
            let p = &result.predictions[i];
            writeln!(f, "{}\t{:.4}\t{:.1}\t{:.1}\t{:.0}\t{:.0}\t{:.0}\t{:.0}",
                obs.time, result.ll_increments[i], result.ess_trace[i],
                p.mean, p.q05, p.q50, p.q95, obs.value).unwrap();
        }
        eprintln!("trace written to {}", path);
    }

    // Save final particle states
    if let Some(ref path) = save_final_state {
        if let Some(ref states) = result.final_states {
            write_final_states(path, states, &model).unwrap_or_else(|e| {
                eprintln!("error writing final states: {}", e);
                std::process::exit(1);
            });
            eprintln!("final particle states ({} particles) written to {}", states.len(), path);
        }
    }

    // Write loglik
    match &output_path {
        Some(path) => {
            std::fs::write(path, format!("{:.4}\n", result.log_likelihood))
                .unwrap_or_else(|e| { eprintln!("cannot write {}: {}", path, e); std::process::exit(1); });
            eprintln!("loglik written to {}", path);
        }
        None => {
            println!("{:.4}", result.log_likelihood);
        }
    }
}

/// Load observation data from a TSV file.
/// Expected columns: time, then one or more value columns.
/// Uses the first value column.
pub fn load_data_tsv_pub(path: &str) -> Result<Vec<Observation>, String> {
    load_data_tsv(path)
}

fn load_data_tsv(path: &str) -> Result<Vec<Observation>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {}", path, e))?;
    let mut lines = content.lines();
    let header = lines.next().ok_or("empty data file")?;
    let cols: Vec<&str> = header.split('\t').collect();
    if cols.len() < 2 {
        return Err(format!("data file needs at least 2 columns (time, value), got {}", cols.len()));
    }

    let mut observations = Vec::new();
    for (line_num, line) in lines.enumerate() {
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            return Err(format!("line {}: expected 2+ columns, got {}", line_num + 2, fields.len()));
        }
        let time: f64 = fields[0].trim().parse()
            .map_err(|_| format!("line {}: cannot parse time '{}'", line_num + 2, fields[0]))?;
        let value: f64 = fields[1].trim().parse()
            .map_err(|_| format!("line {}: cannot parse value '{}'", line_num + 2, fields[1]))?;
        observations.push(Observation { time, value });
    }

    Ok(observations)
}

use std::io::Write;

/// Write final particle states to a TSV file.
/// Columns: particle_id, then one column per compartment, then flow_<transition>.
fn write_final_states(
    path: &str,
    states: &[sim::inference::ParticleState],
    model: &ir::Model,
) -> Result<(), String> {
    let mut f = std::fs::File::create(path)
        .map_err(|e| format!("cannot create {}: {}", path, e))?;

    // Header
    write!(f, "particle").unwrap();
    for c in &model.compartments {
        if c.kind == ir::model::CompartmentKind::Integer {
            write!(f, "\t{}", c.name).unwrap();
        }
    }
    for tr in &model.transitions {
        write!(f, "\tflow_{}", tr.name).unwrap();
    }
    writeln!(f).unwrap();

    // Rows
    for (i, state) in states.iter().enumerate() {
        write!(f, "{}", i).unwrap();
        for &c in &state.counts {
            write!(f, "\t{}", c).unwrap();
        }
        for &fl in &state.flow_accumulators {
            write!(f, "\t{}", fl).unwrap();
        }
        writeln!(f).unwrap();
    }

    Ok(())
}
