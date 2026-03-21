use std::collections::HashMap;
use ir::table::TableSource;
use sim::{
    CompiledModel, GillespieSim, TauLeapSim, ChainBinomialSim,
    config::{GillespieConfig, TauLeapConfig, ChainBinomialConfig, SimConfig},
    simulate::Simulate,
    Trajectory,
};

// ─── IR path resolver ────────────────────────────────────────────────────────

/// If path ends with `.camdl`, compile it via camdlc and write to a temp file.
/// Returns (resolved_path, Some(tmpfile)) or (path, None) for plain .ir.json.
pub fn resolve_ir_path(path: &str) -> Result<(String, Option<std::path::PathBuf>), String> {
    if !path.ends_with(".camdl") {
        return Ok((path.to_string(), None));
    }
    let tmp = std::env::temp_dir()
        .join(format!("camdl_{}.ir.json", std::process::id()));
    let camdlc = std::env::var("CAMDLC").unwrap_or_else(|_| "camdlc".to_string());
    let out = std::process::Command::new(&camdlc)
        .arg(path)
        .output()
        .map_err(|e| format!("could not run camdlc: {}", e))?;
    if !out.status.success() {
        return Err(format!("camdlc failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    std::fs::write(&tmp, &out.stdout)
        .map_err(|e| format!("error writing temp IR: {}", e))?;
    Ok((tmp.to_string_lossy().into_owned(), Some(tmp)))
}

// ─── Loader helpers ──────────────────────────────────────────────────────────

/// Load a flat Vec<Expr::Const> from a CSV, TSV, or JSON file.
pub fn load_table_file(path: &str) -> Result<Vec<ir::expr::Expr>, String> {
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

/// Load parameter overrides from a TOML file.
pub fn load_params_toml(path: &str) -> Result<HashMap<String, f64>, String> {
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

/// Load a keyed TSV file (two columns: name<TAB>value) for --param-vec.
pub fn load_keyed_tsv(path: &str) -> Result<Vec<(String, f64)>, String> {
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

// ─── SimRun / SimOutput ───────────────────────────────────────────────────────

/// All inputs needed to run one simulation.
pub struct SimRun {
    pub ir_path: String,
    pub params_files: Vec<String>,
    pub overrides: HashMap<String, f64>,
    pub set_vec_entries: Vec<(String, String)>,
    pub table_files: HashMap<String, String>,
    pub scenario_name: Option<String>,
    pub adhoc_enable: Vec<String>,
    pub adhoc_disable: Vec<String>,
    pub backend: String,
    pub dt: f64,
    pub seed: u64,
}

impl Default for SimRun {
    fn default() -> Self {
        SimRun {
            ir_path: String::new(),
            params_files: Vec::new(),
            overrides: HashMap::new(),
            set_vec_entries: Vec::new(),
            table_files: HashMap::new(),
            scenario_name: None,
            adhoc_enable: Vec::new(),
            adhoc_disable: Vec::new(),
            backend: "gillespie".to_string(),
            dt: 1.0,
            seed: 1,
        }
    }
}

/// Run a simulation and return the full trajectory.
pub fn run_simulation(run: &SimRun) -> Result<(Trajectory, ir::Model), String> {
    // Load IR source (handles .camdl compilation via camdlc)
    let (ir_path_resolved, _tmpfile) = resolve_ir_path(&run.ir_path)?;

    let src = std::fs::read_to_string(&ir_path_resolved)
        .map_err(|e| format!("cannot read {}: {}", ir_path_resolved, e))?;
    let mut model: ir::Model = serde_json::from_str(&src)
        .map_err(|e| format!("IR parse error: {}", e))?;

    // Apply scenario patch
    {
        let (active_enable, active_disable, scenario_params): (Vec<String>, Vec<String>, Vec<(String, f64)>) =
            if let Some(ref name) = run.scenario_name {
                let preset = model.presets.iter().find(|p| p.name == *name)
                    .ok_or_else(|| {
                        let available: Vec<&str> = model.presets.iter()
                            .map(|p| p.name.as_str()).collect();
                        format!("scenario '{}' not found in model. Available: {}",
                            name,
                            if available.is_empty() { "(none)".to_string() }
                            else { available.join(", ") })
                    })?;
                (preset.enable.clone(), preset.disable.clone(),
                 preset.params.iter().map(|(k, &v)| (k.clone(), v)).collect())
            } else {
                (run.adhoc_enable.clone(), run.adhoc_disable.clone(), vec![])
            };

        if !active_enable.is_empty() {
            model.interventions.retain(|iv| active_enable.contains(&iv.name));
        } else if !active_disable.is_empty() {
            model.interventions.retain(|iv| !active_disable.contains(&iv.name));
        } else {
            model.interventions.clear();
        }

        for (k, v) in scenario_params {
            for p in &mut model.parameters {
                if p.name == k { p.value = Some(v); }
            }
        }
    }

    // Apply --params TOML files
    for path in &run.params_files {
        let toml_overrides = load_params_toml(path)?;
        for p in &mut model.parameters {
            if let Some(&v) = toml_overrides.get(&p.name) {
                p.value = Some(v);
            }
        }
    }

    // Apply --param-vec entries
    if !run.set_vec_entries.is_empty() {
        let known_param_names: std::collections::HashSet<String> =
            model.parameters.iter().map(|p| p.name.clone()).collect();
        let mut resolved: Vec<(String, f64)> = Vec::new();
        for (prefix, file) in &run.set_vec_entries {
            let entries = load_keyed_tsv(file)?;
            for (key, val) in entries {
                let full_name = format!("{}_{}", prefix, key);
                if !known_param_names.contains(&full_name) {
                    return Err(format!("--param-vec {}: unknown parameter '{}'", prefix, full_name));
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

    // Apply scalar overrides (highest priority)
    for p in &mut model.parameters {
        if let Some(&v) = run.overrides.get(&p.name) { p.value = Some(v); }
    }

    // Fill external tables
    for table in &mut model.tables {
        if let TableSource::External { external: ref name } = table.source {
            let logical_name = name.clone();
            match run.table_files.get(&logical_name) {
                None => {
                    return Err(format!(
                        "table '{}' is declared as external() but --table {}=<file> was not provided",
                        logical_name, logical_name));
                }
                Some(path) => {
                    let values = load_table_file(path)?;
                    table.source = TableSource::Inline { values };
                }
            }
        }
    }

    let compiled = CompiledModel::new(model.clone())
        .map_err(|e| format!("model compile error: {:?}", e))?;
    let params  = compiled.default_params.clone();
    let t_start = model.simulation.t_start;
    let t_end   = model.simulation.t_end;

    let config = match run.backend.as_str() {
        "gillespie"      => SimConfig::Gillespie(GillespieConfig { t_start, t_end, output_dt: None }),
        "tau_leap"       => SimConfig::TauLeap(TauLeapConfig { t_start, t_end, dt: run.dt }),
        "chain_binomial" => SimConfig::ChainBinomial(ChainBinomialConfig { t_start, t_end, dt: run.dt }),
        "ode" => return Err("ODE backend not yet implemented".to_string()),
        s => return Err(format!("unknown backend: {}", s)),
    };

    let traj = match run.backend.as_str() {
        "gillespie"      => GillespieSim.run(&compiled, &params, run.seed, &config),
        "tau_leap"       => TauLeapSim.run(&compiled, &params, run.seed, &config),
        "chain_binomial" => ChainBinomialSim.run(&compiled, &params, run.seed, &config),
        _ => unreachable!(),
    }.map_err(|e| format!("simulation error: {:?}", e))?;

    Ok((traj, model))
}

/// Write a trajectory to a TSV file (same format as `camdl simulate` stdout).
pub fn write_traj_tsv(path: &str, model: &ir::Model, traj: &Trajectory, emit_flows: bool) -> Result<(), String> {
    use std::io::Write;
    use std::fs::File;

    let int_names: Vec<&str> = model.compartments.iter()
        .filter(|c| c.kind == ir::model::CompartmentKind::Integer)
        .map(|c| c.name.as_str()).collect();
    let real_names: Vec<&str> = model.compartments.iter()
        .filter(|c| c.kind == ir::model::CompartmentKind::Real)
        .map(|c| c.name.as_str()).collect();
    let tr_names: Vec<&str> = model.transitions.iter()
        .map(|t| t.name.as_str()).collect();

    let mut f = File::create(path)
        .map_err(|e| format!("cannot create {}: {}", path, e))?;

    // Header
    write!(f, "t").map_err(|e| e.to_string())?;
    for n in &int_names  { write!(f, "\t{}", n).map_err(|e| e.to_string())?; }
    for n in &real_names { write!(f, "\t{}", n).map_err(|e| e.to_string())?; }
    if emit_flows {
        for n in &tr_names { write!(f, "\tflow_{}", n).map_err(|e| e.to_string())?; }
    }
    writeln!(f).map_err(|e| e.to_string())?;

    // Rows
    for snap in &traj.snapshots {
        write!(f, "{}", snap.t).map_err(|e| e.to_string())?;
        for &c in &snap.int_state.counts  { write!(f, "\t{}", c).map_err(|e| e.to_string())?; }
        for &v in &snap.real_state.values { write!(f, "\t{:.4}", v).map_err(|e| e.to_string())?; }
        if emit_flows {
            for &fl in &snap.flows.counts { write!(f, "\t{}", fl).map_err(|e| e.to_string())?; }
        }
        writeln!(f).map_err(|e| e.to_string())?;
    }
    Ok(())
}
