use sim::{
    CompiledModel, GillespieSim, TauLeapSim, ChainBinomialSim,
    config::{GillespieConfig, TauLeapConfig, ChainBinomialConfig, SimConfig},
    simulate::Simulate,
    write_diagnostics_tsv, warn_zero_firings,
};
use ir::table::TableSource;
use std::collections::HashMap;
use toml;

/// Load a flat Vec<Expr::Const> from a CSV, TSV, or JSON file.
/// CSV/TSV: rows of numbers, row-major. JSON: `[n, ...]` or `[[n,...],...]`.
fn load_table_file(path: &str) -> Result<Vec<ir::expr::Expr>, String> {
    use ir::expr::Expr;
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {}", path, e))?;
    let ext = std::path::Path::new(path)
        .extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    if ext == "json" {
        let v: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("JSON parse error in {}: {}", path, e))?;
        let mut out = Vec::new();
        match &v {
            serde_json::Value::Array(rows) => {
                for row in rows {
                    match row {
                        serde_json::Value::Array(cols) => {
                            for cell in cols {
                                let f = cell.as_f64().ok_or_else(||
                                    format!("expected number in {}", path))?;
                                out.push(Expr::const_(f));
                            }
                        }
                        _ => {
                            let f = row.as_f64().ok_or_else(||
                                format!("expected number in {}", path))?;
                            out.push(Expr::const_(f));
                        }
                    }
                }
            }
            _ => return Err(format!("expected JSON array in {}", path)),
        }
        Ok(out)
    } else {
        // CSV or TSV
        let sep = if ext == "tsv" { '\t' } else { ',' };
        let mut out = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            for cell in line.split(sep) {
                let cell = cell.trim();
                let f: f64 = cell.parse()
                    .map_err(|_| format!("expected number, got '{}' in {}", cell, path))?;
                out.push(Expr::const_(f));
            }
        }
        Ok(out)
    }
}

fn usage() -> ! {
    eprintln!("camdl simulate MODEL.ir.json [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --params   FILE.toml     load parameter values from TOML (repeatable, later overrides earlier)");
    eprintln!("  --backend  gillespie|tau_leap|chain_binomial  (default: gillespie)");
    eprintln!("  --dt       DT   step size for tau_leap / chain_binomial");
    eprintln!("  --seed     N    RNG seed (default: 1)");
    eprintln!("  --set      NAME=VALUE    override a parameter value");
    eprintln!("  --set-vec  PREFIX=FILE   override indexed params from a keyed TSV (name<TAB>value)");
    eprintln!("  --table    NAME=FILE     supply a runtime external() table from CSV/TSV/JSON");
    std::process::exit(1);
}

/// Load parameter overrides from a TOML file.
///
/// Supports two forms:
///   - Top-level scalar:  `gamma = 0.1`  → parameter "gamma"
///   - Indexed section:   `[R0]`         → prefix "R0"
///                        `urban = 2.5`  → parameter "R0_urban"
fn load_params_toml(path: &str) -> Result<HashMap<String, f64>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {}", path, e))?;
    let table: toml::Table = content.parse()
        .map_err(|e| format!("TOML parse error in {}: {}", path, e))?;
    let mut out = HashMap::new();
    for (key, val) in &table {
        match val {
            toml::Value::Float(f)   => { out.insert(key.clone(), *f); }
            toml::Value::Integer(i) => { out.insert(key.clone(), *i as f64); }
            toml::Value::Table(section) => {
                // Indexed section: [PREFIX] with scalar entries → PREFIX_key
                for (subkey, subval) in section {
                    let full = format!("{}_{}", key, subkey);
                    match subval {
                        toml::Value::Float(f)   => { out.insert(full, *f); }
                        toml::Value::Integer(i) => { out.insert(full, *i as f64); }
                        _ => return Err(format!(
                            "{}:[{}].{}: expected a number, got {:?}", path, key, subkey, subval
                        )),
                    }
                }
            }
            _ => return Err(format!(
                "{}:{}: expected a number or table section, got {:?}", path, key, val
            )),
        }
    }
    Ok(out)
}

/// Load a keyed TSV file (two columns: name<TAB>value) for --set-vec.
/// Returns Vec<(key, value)>. Skips blank lines and # comments.
fn load_keyed_tsv(path: &str) -> Result<Vec<(String, f64)>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {}", path, e))?;
    let mut out = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let mut parts = line.splitn(2, '\t');
        let key = parts.next()
            .ok_or_else(|| format!("{}:{}: expected key<TAB>value", path, lineno + 1))?
            .trim().to_string();
        let val_str = parts.next()
            .ok_or_else(|| format!("{}:{}: missing value column", path, lineno + 1))?
            .trim();
        let val: f64 = val_str.parse()
            .map_err(|_| format!("{}:{}: expected number, got '{}'", path, lineno + 1, val_str))?;
        out.push((key, val));
    }
    Ok(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() { usage(); }

    // Accept "camdl simulate FILE ..." or bare "camdl FILE ..."
    let args: &[String] = if args[0] == "simulate" { &args[1..] } else { &args };
    if args.is_empty() { usage(); }

    let mut ir_path:     Option<String> = None;
    let mut backend    = "gillespie".to_string();
    let mut dt         = 1.0_f64;
    let mut seed       = 1_u64;
    let mut overrides: HashMap<String, f64> = HashMap::new();
    let mut table_files: HashMap<String, String> = HashMap::new();
    let mut params_files: Vec<String> = Vec::new();

    // Collect --set-vec PREFIX=FILE entries for deferred validation after model load
    let mut set_vec_entries: Vec<(String, String)> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backend" => { i += 1; backend = args[i].clone(); }
            "--dt"      => { i += 1; dt      = args[i].parse().expect("--dt needs a number"); }
            "--seed"    => { i += 1; seed    = args[i].parse().expect("--seed needs an integer"); }
            "--params"  => { i += 1; params_files.push(args[i].clone()); }
            "--set"     => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().expect("--set needs NAME=VALUE").to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok())
                    .expect("--set value must be a number");
                overrides.insert(k, v);
            }
            "--set-vec" => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let prefix = parts.next().expect("--set-vec needs PREFIX=FILE").to_string();
                let file   = parts.next().expect("--set-vec needs PREFIX=FILE").to_string();
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

    // Apply parameters: resolution order (highest priority last, so later writes win):
    //   1. IR value (already in model, may be None)
    //   2. --params FILE.toml (in order given, later files override earlier)
    //   3. --set-vec PREFIX=FILE (keyed TSV)
    //   4. --set NAME=VALUE (highest priority)

    // Step 2: apply --params TOML files in order
    for path in &params_files {
        let toml_overrides = load_params_toml(path).unwrap_or_else(|e| {
            eprintln!("error: --params {}: {}", path, e);
            std::process::exit(1);
        });
        for p in &mut model.parameters {
            if let Some(&v) = toml_overrides.get(&p.name) {
                p.value = Some(v);
            }
        }
    }

    // Step 3: apply --set-vec PREFIX=FILE overrides (keyed TSV: name<TAB>value)
    if !set_vec_entries.is_empty() {
        let known_param_names: std::collections::HashSet<String> =
            model.parameters.iter().map(|p| p.name.clone()).collect();
        let mut resolved: Vec<(String, f64)> = Vec::new();
        for (prefix, file) in &set_vec_entries {
            let entries = load_keyed_tsv(file).unwrap_or_else(|e| {
                eprintln!("error: --set-vec {}: {}", prefix, e);
                std::process::exit(1);
            });
            for (key, val) in entries {
                let full_name = format!("{}_{}", prefix, key);
                if !known_param_names.contains(&full_name) {
                    eprintln!("error: --set-vec {}: unknown parameter '{}'", prefix, full_name);
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

    // Step 4: apply --set scalar overrides (highest priority)
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
                    let values = load_table_file(path).unwrap_or_else(|e| {
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
