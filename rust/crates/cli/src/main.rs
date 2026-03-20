mod util;
mod hashing;
mod experiment;
mod serve;
mod summarize;

use sim::{
    CompiledModel, GillespieSim, TauLeapSim, ChainBinomialSim,
    config::{GillespieConfig, TauLeapConfig, ChainBinomialConfig, SimConfig},
    simulate::Simulate,
    write_diagnostics_tsv, warn_zero_firings,
};
use ir::table::TableSource;
use std::collections::HashMap;

fn usage() -> ! {
    eprintln!("camdl simulate MODEL.ir.json [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --params   FILE.toml     load parameter values from TOML (repeatable, later overrides earlier)");
    eprintln!("  --backend  gillespie|tau_leap|chain_binomial  (default: gillespie)");
    eprintln!("  --dt       DT   step size for tau_leap / chain_binomial");
    eprintln!("  --seed     N    RNG seed (default: 1)");
    eprintln!("  --scenario NAME          select a named scenario from the model file");
    eprintln!("  --enable   NAME          enable a named intervention (ad-hoc; mutually exclusive with --scenario)");
    eprintln!("  --disable  NAME          disable a named intervention (ad-hoc; mutually exclusive with --scenario)");
    eprintln!("  --param    NAME=VALUE    override a parameter value");
    eprintln!("  --param-vec PREFIX=FILE  override indexed params from a keyed TSV (name<TAB>value)");
    eprintln!("  --table    NAME=FILE     supply a runtime external() table from CSV/TSV/JSON");
    std::process::exit(1);
}

fn main() {
    let all_args: Vec<String> = std::env::args().skip(1).collect();
    if all_args.is_empty() { usage(); }

    // Dispatch on first argument
    match all_args[0].as_str() {
        "experiment" => {
            match all_args.get(1).map(|s| s.as_str()) {
                Some("run")       => experiment::cmd_experiment_run(&all_args[2..]),
                Some("status")    => experiment::cmd_experiment_status(&all_args[2..]),
                Some("summarize") => summarize::cmd_experiment_summarize(&all_args[2..]),
                _ => {
                    eprintln!("usage: camdl experiment <run|status|summarize> ...");
                    std::process::exit(1);
                }
            }
        }
        "serve" => {
            serve::cmd_serve(&all_args[1..]);
        }
        _ => {
            // Accept "camdl simulate FILE ..." or bare "camdl FILE ..."
            let args: &[String] = if all_args[0] == "simulate" || all_args[0] == "sim" {
                &all_args[1..]
            } else {
                &all_args
            };
            if args.is_empty() { usage(); }
            run_simulate(args);
        }
    }
}

fn run_simulate(args: &[String]) {
    let mut ir_path:     Option<String> = None;
    let mut backend    = "gillespie".to_string();
    let mut dt         = 1.0_f64;
    let mut seed       = 1_u64;
    let mut overrides: HashMap<String, f64> = HashMap::new();
    let mut table_files: HashMap<String, String> = HashMap::new();
    let mut params_files: Vec<String> = Vec::new();
    let mut scenario_name: Option<String> = None;
    let mut adhoc_enable: Vec<String> = Vec::new();
    let mut adhoc_disable: Vec<String> = Vec::new();

    // Collect --param-vec PREFIX=FILE entries for deferred validation after model load
    let mut set_vec_entries: Vec<(String, String)> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backend"  => { i += 1; backend = args[i].clone(); }
            "--dt"       => { i += 1; dt      = args[i].parse().expect("--dt needs a number"); }
            "--seed"     => { i += 1; seed    = args[i].parse().expect("--seed needs an integer"); }
            "--params"   => { i += 1; params_files.push(args[i].clone()); }
            "--scenario" => { i += 1; scenario_name = Some(args[i].clone()); }
            "--enable"   => { i += 1; adhoc_enable.push(args[i].clone()); }
            "--disable"  => { i += 1; adhoc_disable.push(args[i].clone()); }
            "--param"     => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().expect("--param needs NAME=VALUE").to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok())
                    .expect("--param value must be a number");
                overrides.insert(k, v);
            }
            "--param-vec" => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let prefix = parts.next().expect("--param-vec needs PREFIX=FILE").to_string();
                let file   = parts.next().expect("--param-vec needs PREFIX=FILE").to_string();
                set_vec_entries.push((prefix, file));
            }
            "--table"   => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().expect("--table needs NAME=FILE").to_string();
                let v = parts.next().expect("--table needs NAME=FILE").to_string();
                table_files.insert(k, v);
            }
            s if s.starts_with("--") => { eprintln!("unknown flag: {}", s); usage(); }
            path => { ir_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let ir_path = ir_path.unwrap_or_else(|| { eprintln!("missing IR file argument"); usage(); });

    // Validate mutually exclusive σ flags
    if scenario_name.is_some() && (!adhoc_enable.is_empty() || !adhoc_disable.is_empty()) {
        eprintln!("error: --scenario and --enable/--disable are mutually exclusive.");
        eprintln!("  --scenario selects a named scenario from the model file.");
        eprintln!("  --enable/--disable compose an ad-hoc scenario.");
        eprintln!("  To combine both, define a composed scenario in the model file.");
        std::process::exit(1);
    }

    // If given a .camdl source file, compile it via camdlc (outputs JSON to stdout)
    let (ir_path, _tmpfile) = if ir_path.ends_with(".camdl") {
        let tmp = std::env::temp_dir().join(format!("camdl_{}.ir.json", std::process::id()));
        let camdlc = std::env::var("CAMDLC").unwrap_or_else(|_| "camdlc".to_string());
        let out = std::process::Command::new(&camdlc)
            .arg(&ir_path)
            .output()
            .unwrap_or_else(|e| {
                eprintln!("error: could not run camdlc: {}", e);
                eprintln!("Make sure camdlc is on your PATH (run 'dune build' in the ocaml/ directory).");
                std::process::exit(1);
            });
        if !out.status.success() {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), &out.stderr);
            std::process::exit(out.status.code().unwrap_or(1));
        }
        std::fs::write(&tmp, &out.stdout)
            .unwrap_or_else(|e| { eprintln!("error writing temp IR: {}", e); std::process::exit(1); });
        (tmp.to_string_lossy().into_owned(), Some(tmp))
    } else {
        (ir_path, None)
    };

    // Load and parse IR
    let src = std::fs::read_to_string(&ir_path)
        .unwrap_or_else(|e| { eprintln!("cannot read {}: {}", ir_path, e); std::process::exit(1); });
    let mut model: ir::Model = serde_json::from_str(&src)
        .unwrap_or_else(|e| { eprintln!("IR parse error: {}", e); std::process::exit(1); });

    // Apply scenario patch (σ layer): resolve which interventions are active and
    // apply set-style param overrides. Interventions are DISABLED by default
    // (baseline identity patch = no interventions). Scenarios explicitly enable them.
    {
        let (active_enable, active_disable, scenario_params): (Vec<String>, Vec<String>, Vec<(String, f64)>) =
            if let Some(ref name) = scenario_name {
                // Named scenario: look up in presets manifest
                let preset = model.presets.iter().find(|p| p.name == *name)
                    .unwrap_or_else(|| {
                        eprintln!("error: scenario '{}' not found in model. Available: {}",
                            name,
                            model.presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
                        std::process::exit(1);
                    });
                (preset.enable.clone(), preset.disable.clone(),
                 preset.params.iter().map(|(k, &v)| (k.clone(), v)).collect())
            } else {
                // Ad-hoc flags
                (adhoc_enable.clone(), adhoc_disable.clone(), vec![])
            };

        // Filter interventions: baseline = none; enable list = keep only those;
        // disable list = keep all except those.
        if !active_enable.is_empty() {
            model.interventions.retain(|iv| active_enable.contains(&iv.name));
        } else if !active_disable.is_empty() {
            model.interventions.retain(|iv| !active_disable.contains(&iv.name));
        } else {
            // Baseline identity patch: no interventions fire
            model.interventions.clear();
        }

        // Apply scenario set-style param overrides (lower priority than --param)
        for (k, v) in scenario_params {
            for p in &mut model.parameters {
                if p.name == k { p.value = Some(v); }
            }
        }
    }

    // Apply parameters: resolution order (highest priority last, so later writes win):
    //   1. IR value (already in model, may be None)
    //   2. --params FILE.toml (in order given, later files override earlier)
    //   3. --param-vec PREFIX=FILE (keyed TSV)
    //   4. --param NAME=VALUE (highest priority)

    // Step 2: apply --params TOML files in order
    for path in &params_files {
        let toml_overrides = util::load_params_toml(path).unwrap_or_else(|e| {
            eprintln!("error: --params {}: {}", path, e);
            std::process::exit(1);
        });
        for p in &mut model.parameters {
            if let Some(&v) = toml_overrides.get(&p.name) {
                p.value = Some(v);
            }
        }
    }

    // Step 3: apply --param-vec PREFIX=FILE overrides (keyed TSV: name<TAB>value)
    if !set_vec_entries.is_empty() {
        let known_param_names: std::collections::HashSet<String> =
            model.parameters.iter().map(|p| p.name.clone()).collect();
        let mut resolved: Vec<(String, f64)> = Vec::new();
        for (prefix, file) in &set_vec_entries {
            let entries = util::load_keyed_tsv(file).unwrap_or_else(|e| {
                eprintln!("error: --param-vec {}: {}", prefix, e);
                std::process::exit(1);
            });
            for (key, val) in entries {
                let full_name = format!("{}_{}", prefix, key);
                if !known_param_names.contains(&full_name) {
                    eprintln!("error: --param-vec {}: unknown parameter '{}'", prefix, full_name);
                    std::process::exit(1);
                }
                resolved.push((full_name, val));
            }
        }
        for (full_name, val) in resolved {
            for p in &mut model.parameters {
                if p.name == full_name { p.value = Some(val); }
            }
        }
    }

    // Step 4: apply --param scalar overrides (highest priority)
    for p in &mut model.parameters {
        if let Some(&v) = overrides.get(&p.name) { p.value = Some(v); }
    }

    // Fill external() tables from --table NAME=FILE
    for table in &mut model.tables {
        if let TableSource::External { external: ref name } = table.source {
            let logical_name = name.clone();
            match table_files.get(&logical_name) {
                None => {
                    eprintln!("error: table '{}' is declared as external() but --table {}=<file> was not provided",
                        logical_name, logical_name);
                    std::process::exit(1);
                }
                Some(path) => {
                    let values = util::load_table_file(path).unwrap_or_else(|e| {
                        eprintln!("error loading table '{}' from {}: {}", logical_name, path, e);
                        std::process::exit(1);
                    });
                    table.source = TableSource::Inline { values };
                }
            }
        }
    }

    let compiled = CompiledModel::new(model.clone())
        .unwrap_or_else(|e| { eprintln!("model compile error: {:?}", e); std::process::exit(1); });
    let params  = compiled.default_params.clone();
    let t_start = model.simulation.t_start;
    let t_end   = model.simulation.t_end;

    let config = match backend.as_str() {
        "gillespie"      => SimConfig::Gillespie(GillespieConfig { t_start, t_end, output_dt: None }),
        "tau_leap"       => SimConfig::TauLeap(TauLeapConfig { t_start, t_end, dt }),
        "chain_binomial" => SimConfig::ChainBinomial(ChainBinomialConfig { t_start, t_end, dt }),
        "ode" => {
            eprintln!("error: ODE backend not yet implemented. Use --backend gillespie (default), tau_leap, or chain_binomial.");
            std::process::exit(1);
        }
        s => { eprintln!("unknown backend: {}", s); usage(); }
    };

    let traj = match backend.as_str() {
        "gillespie"      => GillespieSim.run(&compiled, &params, seed, &config),
        "tau_leap"       => TauLeapSim.run(&compiled, &params, seed, &config),
        "chain_binomial" => ChainBinomialSim.run(&compiled, &params, seed, &config),
        _ => unreachable!(),
    }.unwrap_or_else(|e| { eprintln!("simulation error: {:?}", e); std::process::exit(1); });

    // Write diagnostics.tsv unconditionally
    if !traj.transition_diagnostics.is_empty() {
        match write_diagnostics_tsv("diagnostics.tsv", &traj.transition_diagnostics) {
            Ok(zero_count) => {
                if zero_count > 0 {
                    warn_zero_firings(&traj.transition_diagnostics);
                }
            }
            Err(e) => eprintln!("warning: could not write diagnostics.tsv: {}", e),
        }
    }

    // TSV header
    let int_names: Vec<&str> = model.compartments.iter()
        .filter(|c| c.kind == ir::model::CompartmentKind::Integer)
        .map(|c| c.name.as_str()).collect();
    let real_names: Vec<&str> = model.compartments.iter()
        .filter(|c| c.kind == ir::model::CompartmentKind::Real)
        .map(|c| c.name.as_str()).collect();
    let tr_names: Vec<&str> = model.transitions.iter().map(|t| t.name.as_str()).collect();

    print!("t");
    for n in &int_names  { print!("\t{}", n); }
    for n in &real_names { print!("\t{}", n); }
    for n in &tr_names   { print!("\tflow_{}", n); }
    println!();

    // TSV rows
    for snap in &traj.snapshots {
        print!("{}", snap.t);
        for &c in &snap.int_state.counts  { print!("\t{}", c); }
        for &v in &snap.real_state.values { print!("\t{:.4}", v); }
        for &f in &snap.flows.counts      { print!("\t{}", f); }
        println!();
    }
}
