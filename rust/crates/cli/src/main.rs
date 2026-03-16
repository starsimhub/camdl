use sim::{
    CompiledModel, GillespieSim, TauLeapSim, ChainBinomialSim,
    config::{GillespieConfig, TauLeapConfig, ChainBinomialConfig, SimConfig},
    simulate::Simulate,
    write_diagnostics_tsv, warn_zero_firings,
};
use std::collections::HashMap;

fn usage() -> ! {
    eprintln!("camdl simulate MODEL.ir.json [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --backend  gillespie|tau_leap|chain_binomial  (default: gillespie)");
    eprintln!("  --dt       DT   step size for tau_leap / chain_binomial");
    eprintln!("  --seed     N    RNG seed (default: 42)");
    eprintln!("  --set      NAME=VALUE  override a parameter value");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() { usage(); }

    // Accept "camdl simulate FILE ..." or bare "camdl FILE ..."
    let args: &[String] = if args[0] == "simulate" { &args[1..] } else { &args };
    if args.is_empty() { usage(); }

    let mut ir_path:   Option<String> = None;
    let mut backend  = "gillespie".to_string();
    let mut dt       = 1.0_f64;
    let mut seed     = 42_u64;
    let mut overrides: HashMap<String, f64> = HashMap::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backend" => { i += 1; backend = args[i].clone(); }
            "--dt"      => { i += 1; dt      = args[i].parse().expect("--dt needs a number"); }
            "--seed"    => { i += 1; seed    = args[i].parse().expect("--seed needs an integer"); }
            "--set"     => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().expect("--set needs NAME=VALUE").to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok())
                    .expect("--set value must be a number");
                overrides.insert(k, v);
            }
            s if s.starts_with("--") => { eprintln!("unknown flag: {}", s); usage(); }
            path => { ir_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let ir_path = ir_path.unwrap_or_else(|| { eprintln!("missing IR file argument"); usage(); });

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

    // Apply --set overrides
    for p in &mut model.parameters {
        if let Some(&v) = overrides.get(&p.name) { p.value = v; }
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
