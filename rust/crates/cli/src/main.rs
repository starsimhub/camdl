mod util;
mod hashing;
mod sampling;
mod experiment;
mod analyze;
mod serve;
mod summarize;
mod voi;
mod eval;
mod pfilter;
mod if2;
mod profile;
mod fit;

use sim::{write_diagnostics_tsv, warn_zero_firings};
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
                Some("analyze")   => analyze::cmd_experiment_analyze(&all_args[2..]),
                _ => {
                    eprintln!("usage: camdl experiment <run|status|summarize|analyze> ...");
                    std::process::exit(1);
                }
            }
        }
        "voi" => {
            match all_args.get(1).map(|s| s.as_str()) {
                Some("run") => voi::cmd_voi_run(&all_args[2..]),
                _ => {
                    eprintln!("usage: camdl voi run VOI.toml");
                    std::process::exit(1);
                }
            }
        }
        "eval" => {
            eval::cmd_eval(&all_args[1..]);
        }
        "pfilter" => {
            pfilter::cmd_pfilter(&all_args[1..]);
        }
        "if2" | "mif2" => {
            if2::cmd_if2(&all_args[1..]);
        }
        "profile" => {
            profile::cmd_profile(&all_args[1..]);
        }
        "fit" => {
            match all_args.get(1).map(|s| s.as_str()) {
                Some("scout")    => fit::cmd_fit_scout(&all_args[2..]),
                Some("refine")   => fit::cmd_fit_refine(&all_args[2..]),
                Some("validate") => fit::cmd_fit_validate(&all_args[2..]),
                Some("status")   => fit::cmd_fit_status(&all_args[2..]),
                _ => {
                    eprintln!("usage: camdl fit <scout|refine|validate|status> FIT.toml");
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

    let sim_run = util::SimRun {
        ir_path: ir_path.clone(),
        params_files,
        overrides,
        set_vec_entries,
        table_files,
        scenario_name,
        adhoc_enable,
        adhoc_disable,
        backend,
        dt,
        seed,
    };

    let (traj, model) = util::run_simulation(&sim_run).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

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
