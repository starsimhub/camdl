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
    let mut flow_name: Option<String> = None; // --flow recovery → project that transition
    let mut obs_name: Option<String> = None; // --obs NAME → select observation block
    let mut save_final_state: Option<String> = None;
    let mut n_replicates = 1_usize;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--params"    => { i += 1; params_files.push(args[i].clone()); }
            "--data"      => { i += 1; data_path = Some(args[i].clone()); }
            "--replicates" => { i += 1; n_replicates = args[i].parse().unwrap_or_else(|_| { eprintln!("error: --replicates needs integer"); std::process::exit(1); }); }
            "--particles" => { i += 1; n_particles = args[i].parse().unwrap_or_else(|_| { eprintln!("error: --particles needs an integer"); std::process::exit(1); }); }
            "--dt"        => { i += 1; dt = args[i].parse().unwrap_or_else(|_| { eprintln!("error: --dt needs a number"); std::process::exit(1); }); }
            "--seed"      => { i += 1; seed = args[i].parse().unwrap_or_else(|_| { eprintln!("error: --seed needs an integer"); std::process::exit(1); }); }
            "--trace"     => {
                // Accept both: --trace FILE and bare --trace (stdout)
                if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    i += 1;
                    trace_path = Some(args[i].clone());
                } else {
                    trace_path = Some("-".to_string()); // sentinel for stdout
                }
            }
            "--output" | "-o" => { i += 1; output_path = Some(args[i].clone()); }
            "--scenario"  => { i += 1; scenario_name = Some(args[i].clone()); }
            "--enable"    => { i += 1; adhoc_enable.push(args[i].clone()); }
            "--obs"       => { i += 1; obs_name = Some(args[i].clone()); }
            "--flow"      => { i += 1; flow_name = Some(args[i].clone()); }
            "--save-final-state" => { i += 1; save_final_state = Some(args[i].clone()); }
            "--param"     => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap().to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| { eprintln!("error: --param needs NAME=VALUE"); std::process::exit(1); });
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
    let (mut model, _model_json) = crate::util::load_model(&ir_path)
        .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });

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
    let mut flow_indices: Vec<usize> = if let Some(ref name) = flow_name {
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

    // Find observation model from the IR
    let obs_model_ir = if let Some(ref name) = obs_name {
        model.observations.iter().find(|o| o.name == *name)
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("error: no observation block '{}'. Available: {}",
                    name, model.observations.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", "));
                std::process::exit(1);
            })
    } else if model.observations.len() == 1 {
        model.observations[0].clone()
    } else if !model.observations.is_empty() {
        eprintln!("error: model has {} observation blocks. Use --obs NAME to select one:", model.observations.len());
        for o in &model.observations { eprintln!("  {}", o.name); }
        std::process::exit(1);
    } else {
        eprintln!("error: model has no observations block. Cannot run pfilter without an observation model.");
        std::process::exit(1);
    };

    // Override flow indices from obs model projection if --flow not specified
    if flow_name.is_none() {
        if let ir::observation::Projection::CumulativeFlow(ref name) = obs_model_ir.projection {
            let obs_flow_indices: Vec<usize> = model.transitions.iter().enumerate()
                .filter(|(_, tr)| tr.name == *name || tr.name.starts_with(&format!("{}_", name)))
                .map(|(i, _)| i)
                .collect();
            if !obs_flow_indices.is_empty() {
                flow_indices = obs_flow_indices;
                eprintln!("pfilter: projecting incidence({}) from observation model", name);
            }
        }
    }

    eprintln!("pfilter: obs_model={}, likelihood={}", obs_model_ir.name,
        match &obs_model_ir.likelihood {
            ir::observation::Likelihood::NegBinomial(_) => "neg_binomial",
            ir::observation::Likelihood::Normal(_) => "normal",
            ir::observation::Likelihood::Poisson(_) => "poisson",
            ir::observation::Likelihood::Binomial(_) => "binomial",
            ir::observation::Likelihood::BetaBinomial(_) => "beta_binomial",
            ir::observation::Likelihood::Bernoulli(_) => "bernoulli",
        });

    // Compile dmeasure from IR observation model
    let compiled = std::sync::Arc::new(compiled);

    // Run particle filter
    let step_fn = |state: &mut sim::inference::ParticleState, t: f64, step_dt: f64, rng: &mut sim::rng::StatefulRng, scratch: &mut sim::chain_binomial::StepScratch| -> Result<(), sim::error::SimError> {
        step_one(&compiled, &mut state.counts, &mut state.flow_accumulators, &params, t, step_dt, rng, scratch)
    };

    let project_fn = |state: &sim::inference::ParticleState| -> f64 {
        flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
    };

    let dmeasure_fn = sim::inference::dmeasure::compile_dmeasure_pf(
        &obs_model_ir, compiled.clone(), &params,
    );
    let rmeasure_fn = sim::inference::dmeasure::compile_rmeasure_pf(
        &obs_model_ir, compiled.clone(), &params,
    );
    let obs_mean_fn = sim::inference::dmeasure::compile_obs_mean_pf(
        &obs_model_ir, compiled.clone(), &params,
    );

    // ── Replicates mode: run N independent pfilters, output loglik summary ──
    if n_replicates > 1 {
        eprintln!("pfilter: {} replicates × {} particles", n_replicates, n_particles);
        let mut logliks = Vec::with_capacity(n_replicates);
        for rep in 0..n_replicates {
            let rep_seed = seed + rep as u64;
            let result = bootstrap_filter(
                &compiled, &params, &observations, n_particles, dt,
                &step_fn, &project_fn, &*dmeasure_fn, None, None, rep_seed,
            ).unwrap_or_else(|e| {
                eprintln!("pfilter replicate {} error: {:?}", rep + 1, e);
                std::process::exit(1);
            });
            logliks.push(result.log_likelihood);
            if (rep + 1) % 10 == 0 || rep + 1 == n_replicates {
                eprint!("\r  {}/{} replicates", rep + 1, n_replicates);
            }
        }
        eprintln!();

        let mean_ll = logliks.iter().sum::<f64>() / n_replicates as f64;
        let var_ll = logliks.iter().map(|&l| (l - mean_ll).powi(2)).sum::<f64>() / (n_replicates - 1) as f64;
        let sd_ll = var_ll.sqrt();

        eprintln!("loglik = {:.1} ± {:.1} ({} replicates, N={})", mean_ll, sd_ll, n_replicates, n_particles);

        // Output: TSV of seed + loglik, or summary to --output
        match &output_path {
            Some(path) => {
                let mut f = std::fs::File::create(path)
                    .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", path, e); std::process::exit(1); });
                writeln!(f, "seed\tloglik").unwrap();
                for (rep, ll) in logliks.iter().enumerate() {
                    writeln!(f, "{}\t{:.4}", seed + rep as u64, ll).unwrap();
                }
                eprintln!("replicate logliks written to {}", path);
            }
            None => {
                println!("seed\tloglik");
                for (rep, ll) in logliks.iter().enumerate() {
                    println!("{}\t{:.4}", seed + rep as u64, ll);
                }
            }
        }
        return;
    }

    // ── Single pfilter run ─────────────────────────────────────────────────
    let result = bootstrap_filter(
        &compiled, &params, &observations, n_particles, dt,
        &step_fn, &project_fn, &*dmeasure_fn,
        Some(&*rmeasure_fn), Some(&*obs_mean_fn),
        seed,
    ).unwrap_or_else(|e| {
        eprintln!("pfilter error: {:?}", e);
        std::process::exit(1);
    });

    // Write trace diagnostics
    let trace_to_stdout = trace_path.as_deref() == Some("-");
    if let Some(ref path) = trace_path {
        let mut out: Box<dyn Write> = if path == "-" {
            Box::new(std::io::BufWriter::new(std::io::stdout().lock()))
        } else {
            let f = std::fs::File::create(path)
                .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", path, e); std::process::exit(1); });
            Box::new(std::io::BufWriter::new(f))
        };
        if let Some(ref preds) = result.predictions {
            writeln!(out, "time\tll_increment\tESS\tobs_mean\tobs_q05\tobs_q50\tobs_q95\tstate_mean\tstate_q05\tstate_q50\tstate_q95\tobserved").unwrap();
            for (i, obs) in observations.iter().enumerate() {
                let p = &preds[i];
                writeln!(out, "{}\t{:.4}\t{:.1}\t{:.1}\t{:.0}\t{:.0}\t{:.0}\t{:.1}\t{:.0}\t{:.0}\t{:.0}\t{:.0}",
                    obs.time, result.ll_increments[i], result.ess_trace[i],
                    p.obs_mean, p.obs_q05, p.obs_q50, p.obs_q95,
                    p.state_mean, p.state_q05, p.state_q50, p.state_q95,
                    obs.value).unwrap();
            }
        } else {
            writeln!(out, "time\tll_increment\tESS\tobserved").unwrap();
            for (i, obs) in observations.iter().enumerate() {
                writeln!(out, "{}\t{:.4}\t{:.1}\t{:.0}",
                    obs.time, result.ll_increments[i], result.ess_trace[i],
                    obs.value).unwrap();
            }
        }
        drop(out);
        if path != "-" {
            eprintln!("trace written to {}", path);
        }
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
            if trace_to_stdout {
                eprintln!("{:.4}", result.log_likelihood);
            } else {
                println!("{:.4}", result.log_likelihood);
            }
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
