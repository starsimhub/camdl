use std::collections::HashMap;
use ir::table::TableSource;
use ir::intervention::Intervention;
use serde::Deserialize;
use sim::{
    CompiledModel, GillespieSim, TauLeapSim, ChainBinomialSim, OdeSim,
    config::{GillespieConfig, TauLeapConfig, ChainBinomialConfig, OdeConfig, SimConfig},
    simulate::Simulate,
    Trajectory,
};

// ─── Experiment TOML parsing ─────────────────────────────────────────────────

/// Fields extracted from an experiment.toml needed by summarize, analyze, voi.
pub struct ExperimentInfo {
    pub output_dir:         String,
    pub design_names:       Vec<String>,
    pub analyze_outputs:    Option<Vec<String>>,
    pub analyze_confidence: Option<f64>,
}

/// Parse an experiment.toml source string using proper TOML deserialization.
/// Returns an error string on parse failure.
pub fn parse_experiment_toml(src: &str) -> Result<ExperimentInfo, String> {
    #[derive(Deserialize, Default)]
    struct ConfigSection {
        output_dir: Option<String>,
    }
    #[derive(Deserialize, Default)]
    struct AnalyzeSection {
        outputs:    Option<Vec<String>>,
        confidence: Option<f64>,
    }
    #[derive(Deserialize)]
    struct ExperimentDoc {
        #[serde(default)]
        config:  ConfigSection,
        #[serde(default)]
        design:  HashMap<String, toml::Value>,
        #[serde(default)]
        analyze: AnalyzeSection,
    }

    let doc: ExperimentDoc = toml::from_str(src)
        .map_err(|e| format!("experiment TOML parse error: {}", e))?;

    let mut design_names: Vec<String> = doc.design.into_keys().collect();
    design_names.sort();

    Ok(ExperimentInfo {
        output_dir:         doc.config.output_dir.unwrap_or_else(|| "output".to_string()),
        design_names,
        analyze_outputs:    doc.analyze.outputs,
        analyze_confidence: doc.analyze.confidence,
    })
}

// ─── Compiler discovery ─────────────────────────────────────────────────────

fn camdlc_name() -> &'static str {
    if cfg!(windows) { "camdlc.exe" } else { "camdlc" }
}

/// Find the camdlc compiler binary via a priority chain:
/// 1. Same directory as the running binary (release zip layout)
/// 2. CAMDLC_PATH or CAMDLC environment variable
/// 3. On system PATH
fn find_camdlc() -> Result<std::path::PathBuf, String> {
    use std::path::PathBuf;

    // 1. Same directory as the running binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(camdlc_name());
            if candidate.exists() { return Ok(candidate); }
        }
    }

    // 2. Environment variable override
    for var in &["CAMDLC_PATH", "CAMDLC"] {
        if let Ok(path) = std::env::var(var) {
            let p = PathBuf::from(&path);
            if p.exists() { return Ok(p); }
        }
    }

    // 3. System PATH
    // Try running it to see if it exists
    if std::process::Command::new(camdlc_name())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
    {
        return Ok(PathBuf::from(camdlc_name()));
    }

    Err(format!(
        "camdlc not found.\n\
         Place it next to camdl{} or add it to PATH.\n\
         Set CAMDLC_PATH to override.",
        if cfg!(windows) { ".exe" } else { "" }
    ))
}

/// Run camdlc on a .camdl file and return the IR JSON as a string.
fn run_camdlc(camdl_path: &str) -> Result<String, String> {
    let camdlc = find_camdlc()?;
    let output = std::process::Command::new(&camdlc)
        .arg(camdl_path)
        .output()
        .map_err(|e| format!("cannot run {}: {}", camdlc.display(), e))?;
    if !output.status.success() {
        // camdlc prints errors to stderr — pass them through
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    String::from_utf8(output.stdout)
        .map_err(|e| format!("camdlc output not UTF-8: {}", e))
}

// ─── IR path resolver ────────────────────────────────────────────────────────

/// If path ends with `.camdl`, compile it via camdlc and write to a temp file.
/// Returns (resolved_path, Some(tmpfile)) or (path, None) for plain .ir.json.
pub fn resolve_ir_path(path: &str) -> Result<(String, Option<std::path::PathBuf>), String> {
    if !path.ends_with(".camdl") {
        return Ok((path.to_string(), None));
    }
    let json = run_camdlc(path)?;
    let tmp = std::env::temp_dir()
        .join(format!("camdl_{}.ir.json", std::process::id()));
    std::fs::write(&tmp, &json)
        .map_err(|e| format!("error writing temp IR: {}", e))?;
    Ok((tmp.to_string_lossy().into_owned(), Some(tmp)))
}

/// Load a .camdl or .ir.json model, returning the parsed model and raw IR JSON.
/// The JSON is needed for provenance hashing. Compiles via camdlc if needed.
pub fn load_model(path: &str) -> Result<(ir::Model, String), String> {
    let json = if path.ends_with(".camdl") {
        run_camdlc(path)?
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path, e))?
    };
    let model: ir::Model = serde_json::from_str(&json)
        .map_err(|e| format!("parse error: {}", e))?;
    Ok((model, json))
}

/// Delegate a subcommand directly to camdlc, passing through all args.
/// Used for compile, check, inspect which are purely compiler operations.
pub fn delegate_to_camdlc(args: &[&str]) -> Result<(), String> {
    let camdlc = find_camdlc()?;
    let status = std::process::Command::new(&camdlc)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("cannot run camdlc: {}", e))?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Resolve flow indices for a named transition (or all transmission transitions).
/// Used by pfilter, if2, profile for --flow NAME.
pub fn resolve_flow_indices(model: &ir::Model, flow_name: Option<&str>) -> Result<Vec<usize>, String> {
    if let Some(name) = flow_name {
        let indices: Vec<usize> = model.transitions.iter().enumerate()
            .filter(|(_, tr)| tr.name == name || tr.name.starts_with(&format!("{}_", name)))
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            return Err(format!("no transition named '{}'. Available: {}",
                name, model.transitions.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
        Ok(indices)
    } else {
        let indices: Vec<usize> = model.transitions.iter().enumerate()
            .filter(|(_, tr)| tr.metadata.as_ref()
                .and_then(|m| m.origin_kind.as_deref())
                .map_or(false, |k| k == "transmission"))
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            return Err("no transmission transitions found. Use --flow NAME to specify.".into());
        }
        Ok(indices)
    }
}

// ─── Loader helpers ──────────────────────────────────────────────────────────

/// Load a flat Vec<Expr::Const> from a CSV, TSV, or JSON file.
// --table loads flat row-major float arrays (not long format).
// Long format (read_long) is resolved at compile time by the OCaml frontend.
// External tables supplied at runtime must match the flat order used at compile time.
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

/// Load a TOML params file and apply values to the model's parameters.
pub fn apply_params_file(model: &mut ir::Model, path: &str) -> Result<(), String> {
    let vals = load_params_toml(path)?;
    for p in &mut model.parameters {
        if let Some(&v) = vals.get(&p.name) {
            p.value = Some(v);
        }
    }
    Ok(())
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

// ─── Enable/disable resolution ───────────────────────────────────────────────

/// Resolve a list of enable/disable names, expanding family names via base_name.
///
/// Resolution rule:
/// 1. Exact match: name == iv.name → enable that one
/// 2. Family match: name == iv.base_name → enable all members of that family
/// 3. No match: error with available names and families
pub fn resolve_enable_list(
    names: &[String],
    interventions: &[Intervention],
) -> Result<Vec<String>, String> {
    let mut resolved: Vec<String> = Vec::new();
    for name in names {
        // 1. Exact match
        if interventions.iter().any(|iv| iv.name == *name) {
            resolved.push(name.clone());
            continue;
        }
        // 2. Family match
        let family: Vec<String> = interventions.iter()
            .filter(|iv| iv.base_name.as_deref() == Some(name.as_str()))
            .map(|iv| iv.name.clone())
            .collect();
        if !family.is_empty() {
            resolved.extend(family);
            continue;
        }
        // 3. No match
        let mut families: Vec<&str> = interventions.iter()
            .filter_map(|iv| iv.base_name.as_deref())
            .collect::<std::collections::HashSet<_>>()
            .into_iter().collect();
        families.sort();
        return Err(format!(
            "'{}' does not match any intervention or family.\n  \
             Families: {}\n  Names (first 10): {}",
            name,
            if families.is_empty() { "(none)".to_string() }
            else { families.join(", ") },
            interventions.iter().take(10)
                .map(|iv| iv.name.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(resolved)
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
        let (raw_enable, raw_disable, scenario_params, scenario_scale, scenario_compose):
            (Vec<String>, Vec<String>, Vec<(String, f64)>, Vec<(String, f64)>, Vec<String>) =
            if let Some(ref name) = run.scenario_name {
                let preset = model.presets.iter().find(|p| p.name == *name)
                    .ok_or_else(|| {
                        let available: Vec<&str> = model.presets.iter()
                            .map(|p| p.name.as_str()).collect();
                        format!("scenario '{}' not found in model. Available: {}",
                            name,
                            if available.is_empty() { "(none)".to_string() }
                            else { available.join(", ") })
                    })?.clone();
                // Compose: apply sub-scenarios left-to-right (flat only — no nested compose)
                let mut composed_enable: Vec<String> = Vec::new();
                let mut composed_disable: Vec<String> = Vec::new();
                let mut composed_params: Vec<(String, f64)> = Vec::new();
                let mut composed_scale: Vec<(String, f64)> = Vec::new();
                if !preset.compose.is_empty() {
                    for sc_name in &preset.compose {
                        let sub = model.presets.iter().find(|p| p.name == *sc_name)
                            .ok_or_else(|| format!(
                                "compose: scenario '{}' not found in model", sc_name))?;
                        if !sub.compose.is_empty() {
                            return Err(format!(
                                "nested compose is not supported. Scenario '{}' referenced \
                                 in compose = [...] itself uses compose.",
                                sc_name));
                        }
                        composed_enable.extend(sub.enable.clone());
                        composed_disable.extend(sub.disable.clone());
                        composed_params.extend(sub.params.iter().map(|(k, &v)| (k.clone(), v)));
                        composed_scale.extend(sub.scale.iter().map(|(k, &v)| (k.clone(), v)));
                    }
                }
                // Own enable/disable/params override composed ones
                composed_enable.extend(preset.enable.clone());
                composed_disable.extend(preset.disable.clone());
                composed_params.extend(preset.params.iter().map(|(k, &v)| (k.clone(), v)));
                composed_scale.extend(preset.scale.iter().map(|(k, &v)| (k.clone(), v)));
                (composed_enable, composed_disable, composed_params, composed_scale, preset.compose.clone())
            } else {
                (run.adhoc_enable.clone(), run.adhoc_disable.clone(), vec![], vec![], vec![])
            };

        // Resolve family names → concrete intervention names
        let active_enable  = resolve_enable_list(&raw_enable,  &model.interventions)?;
        let active_disable = resolve_enable_list(&raw_disable, &model.interventions)?;

        if !active_enable.is_empty() || !active_disable.is_empty() {
            model.interventions.retain(|iv| {
                let kept_by_enable  = active_enable.is_empty() || active_enable.contains(&iv.name);
                let kept_by_disable = !active_disable.contains(&iv.name);
                kept_by_enable && kept_by_disable
            });
        } else {
            // Baseline identity patch: no interventions fire
            model.interventions.clear();
        }

        for (k, v) in scenario_params {
            for p in &mut model.parameters {
                if p.name == k { p.value = Some(v); }
            }
        }

        // Apply scale: multiply existing param values
        for (k, factor) in &scenario_scale {
            for p in &mut model.parameters {
                if p.name == *k {
                    if let Some(v) = p.value {
                        p.value = Some(v * factor);
                    }
                }
            }
        }

        let _ = scenario_compose; // compose is consumed above; suppress unused warning
    }

    // Apply --params TOML files (layered, later overrides earlier)
    let model_param_set: std::collections::HashSet<String> = model.parameters.iter()
        .map(|p| p.name.clone()).collect();
    for path in &run.params_files {
        let toml_overrides = load_params_toml(path)?;
        // Check for unknown params in the file
        for name in toml_overrides.keys() {
            if !model_param_set.contains(name) {
                return Err(format!(
                    "unknown parameter '{}' in params file '{}'.\n  \
                     Available parameters: {}",
                    name, path,
                    model.parameters.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for p in &mut model.parameters {
            if let Some(&v) = toml_overrides.get(&p.name) {
                if let Some(old) = p.value {
                    if (old - v).abs() > 1e-15 {
                        log::info!("--params {}: {}={} overrides previous value {}", path, p.name, v, old);
                    }
                }
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
    // Check for unknown params first
    let model_param_names: std::collections::HashSet<&str> = model.parameters.iter()
        .map(|p| p.name.as_str()).collect();
    for name in run.overrides.keys() {
        if !model_param_names.contains(name.as_str()) {
            return Err(format!(
                "unknown parameter '{}' in --param override.\n  \
                 Available parameters: {}",
                name,
                model.parameters.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    for p in &mut model.parameters {
        if let Some(&v) = run.overrides.get(&p.name) {
            if let Some(old) = p.value {
                if (old - v).abs() > 1e-15 {
                    log::info!("--param {}={} overrides previous value {}", p.name, v, old);
                }
            }
            p.value = Some(v);
        }
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
        "ode"            => SimConfig::Ode(OdeConfig { t_start, t_end, dt: run.dt }),
        s => return Err(format!("unknown backend: {}", s)),
    };

    // Check backend compatibility before running
    let backend: &dyn Simulate = match run.backend.as_str() {
        "gillespie"      => &GillespieSim,
        "tau_leap"       => &TauLeapSim,
        "chain_binomial" => &ChainBinomialSim,
        "ode"            => &OdeSim,
        _ => unreachable!(),
    };
    let unsupported = compiled.required_capabilities() - backend.capabilities();
    if !unsupported.is_empty() {
        let mut features = Vec::new();
        if unsupported.contains(sim::Capabilities::OVERDISPERSION) {
            features.push("OVERDISPERSION: transitions with overdispersion require --backend tau_leap or chain_binomial");
        }
        if unsupported.contains(sim::Capabilities::REAL_COMPARTMENTS) {
            features.push("REAL_COMPARTMENTS: real-valued compartments with ODE equations");
        }
        return Err(format!(
            "model requires capabilities not supported by backend '{}':\n  - {}",
            backend.name(), features.join("\n  - ")
        ));
    }

    let traj = backend.run(&compiled, &params, run.seed, &config)
        .map_err(|e| format!("simulation error: {:?}", e))?;

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

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: path to the sir_vaccination golden IR (has 4 params:
    /// beta=0.3, gamma=0.1, vaccine_coverage=0.5, rho=10.0).
    /// Resolves relative to the repo root (tests run from rust/).
    fn sir_model() -> String {
        // Resolve relative to the crate manifest dir (rust/crates/cli/)
        let manifest = env!("CARGO_MANIFEST_DIR");
        let path = std::path::PathBuf::from(manifest)
            .join("../../../ir/golden/sir_vaccination.ir.json");
        let path = path.canonicalize()
            .unwrap_or_else(|_| panic!(
                "cannot find sir_vaccination.ir.json (tried {})", path.display()));
        path.to_str().unwrap().to_string()
    }

    fn write_toml(dir: &std::path::Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn base_sim_run(ir_path: &str) -> SimRun {
        SimRun {
            ir_path: ir_path.to_string(),
            backend: "chain_binomial".to_string(),
            dt: 1.0,
            seed: 1,
            ..Default::default()
        }
    }

    /// Extract final param values from a successful simulation.
    fn resolved_params(run: &SimRun) -> Result<HashMap<String, f64>, String> {
        let (_, model) = run_simulation(run)?;
        let compiled = sim::CompiledModel::new(model.clone())
            .map_err(|e| format!("{:?}", e))?;
        Ok(model.parameters.iter().map(|p| {
            let idx = compiled.param_index[p.name.as_str()];
            (p.name.clone(), compiled.default_params[idx])
        }).collect())
    }

    // ── Params file loading ─────────────────────────────────────────────────

    #[test]
    fn single_params_file_sets_values() {
        let dir = tempfile::tempdir().unwrap();
        let pf = write_toml(dir.path(), "params.toml",
            "beta = 0.5\ngamma = 0.2\nvaccine_coverage = 0.8\nrho = 5.0\n");
        let run = SimRun { params_files: vec![pf], ..base_sim_run(&sir_model()) };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 0.5).abs() < 1e-10);
        assert!((params["gamma"] - 0.2).abs() < 1e-10);
        assert!((params["rho"] - 5.0).abs() < 1e-10);
    }

    #[test]
    fn unknown_param_in_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let pf = write_toml(dir.path(), "params.toml", "betta = 0.5\n");
        let run = SimRun { params_files: vec![pf], ..base_sim_run(&sir_model()) };
        let err = run_simulation(&run).unwrap_err();
        assert!(err.contains("unknown parameter 'betta'"));
        assert!(err.contains("Available parameters"));
    }

    // ── Stacked params files (later overrides earlier) ──────────────────────

    #[test]
    fn stacked_params_later_overrides_earlier() {
        let dir = tempfile::tempdir().unwrap();
        let pf1 = write_toml(dir.path(), "base.toml",
            "beta = 0.3\ngamma = 0.1\nvaccine_coverage = 0.5\nrho = 10.0\n");
        let pf2 = write_toml(dir.path(), "override.toml",
            "beta = 0.7\ngamma = 0.2\n");
        let run = SimRun {
            params_files: vec![pf1, pf2],
            ..base_sim_run(&sir_model())
        };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 0.7).abs() < 1e-10, "beta should be 0.7, got {}", params["beta"]);
        assert!((params["gamma"] - 0.2).abs() < 1e-10, "gamma should be 0.2");
        assert!((params["rho"] - 10.0).abs() < 1e-10);
        assert!((params["vaccine_coverage"] - 0.5).abs() < 1e-10);
    }

    #[test]
    fn three_stacked_params_last_wins() {
        let dir = tempfile::tempdir().unwrap();
        let pf1 = write_toml(dir.path(), "a.toml",
            "beta = 0.1\ngamma = 0.1\nvaccine_coverage = 0.5\nrho = 10.0\n");
        let pf2 = write_toml(dir.path(), "b.toml", "beta = 0.2\n");
        let pf3 = write_toml(dir.path(), "c.toml", "beta = 0.9\n");
        let run = SimRun {
            params_files: vec![pf1, pf2, pf3],
            ..base_sim_run(&sir_model())
        };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 0.9).abs() < 1e-10, "third file should win");
    }

    #[test]
    fn unknown_param_in_second_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let pf1 = write_toml(dir.path(), "base.toml",
            "beta = 0.3\ngamma = 0.1\nvaccine_coverage = 0.5\nrho = 10.0\n");
        let pf2 = write_toml(dir.path(), "bad.toml", "typo_param = 0.5\n");
        let run = SimRun {
            params_files: vec![pf1, pf2],
            ..base_sim_run(&sir_model())
        };
        let err = run_simulation(&run).unwrap_err();
        assert!(err.contains("unknown parameter 'typo_param'"));
    }

    // ── --param CLI overrides ───────────────────────────────────────────────

    #[test]
    fn cli_param_overrides_file() {
        let dir = tempfile::tempdir().unwrap();
        let pf = write_toml(dir.path(), "params.toml",
            "beta = 0.3\ngamma = 0.1\nvaccine_coverage = 0.5\nrho = 10.0\n");
        let run = SimRun {
            params_files: vec![pf],
            overrides: [("beta".to_string(), 0.99)].into(),
            ..base_sim_run(&sir_model())
        };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 0.99).abs() < 1e-10, "CLI --param should override file");
        assert!((params["gamma"] - 0.1).abs() < 1e-10, "gamma unchanged");
    }

    #[test]
    fn cli_param_overrides_stacked_files() {
        let dir = tempfile::tempdir().unwrap();
        let pf1 = write_toml(dir.path(), "base.toml",
            "beta = 0.3\ngamma = 0.1\nvaccine_coverage = 0.5\nrho = 10.0\n");
        let pf2 = write_toml(dir.path(), "override.toml", "beta = 0.7\n");
        let run = SimRun {
            params_files: vec![pf1, pf2],
            overrides: [("beta".to_string(), 1.5)].into(),
            ..base_sim_run(&sir_model())
        };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 1.5).abs() < 1e-10, "CLI --param beats stacked files");
    }

    #[test]
    fn unknown_cli_param_errors() {
        let run = SimRun {
            overrides: [("nonexistent".to_string(), 0.5)].into(),
            ..base_sim_run(&sir_model())
        };
        let err = run_simulation(&run).unwrap_err();
        assert!(err.contains("unknown parameter 'nonexistent'"));
    }

    // ── Model defaults (no params file, no overrides) ───────────────────────

    #[test]
    fn model_defaults_used_when_no_params() {
        let run = base_sim_run(&sir_model());
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 0.3).abs() < 1e-10);
        assert!((params["gamma"] - 0.1).abs() < 1e-10);
        assert!((params["vaccine_coverage"] - 0.5).abs() < 1e-10);
        assert!((params["rho"] - 10.0).abs() < 1e-10);
    }

    #[test]
    fn cli_param_without_file_overrides_model_default() {
        let run = SimRun {
            overrides: [("beta".to_string(), 2.0)].into(),
            ..base_sim_run(&sir_model())
        };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 2.0).abs() < 1e-10);
        assert!((params["gamma"] - 0.1).abs() < 1e-10);
    }

    // ── Partial params files ────────────────────────────────────────────────

    #[test]
    fn partial_params_file_leaves_others_at_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let pf = write_toml(dir.path(), "partial.toml", "beta = 0.99\n");
        let run = SimRun {
            params_files: vec![pf],
            ..base_sim_run(&sir_model())
        };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 0.99).abs() < 1e-10);
        assert!((params["gamma"] - 0.1).abs() < 1e-10);
        assert!((params["rho"] - 10.0).abs() < 1e-10);
    }

    // ── Edge cases ──────────────────────────────────────────────────────────

    #[test]
    fn same_value_override_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let pf = write_toml(dir.path(), "params.toml",
            "beta = 0.3\ngamma = 0.1\nvaccine_coverage = 0.5\nrho = 10.0\n");
        let run = SimRun {
            params_files: vec![pf],
            overrides: [("beta".to_string(), 0.3)].into(),
            ..base_sim_run(&sir_model())
        };
        let params = resolved_params(&run).unwrap();
        assert!((params["beta"] - 0.3).abs() < 1e-10);
    }

    #[test]
    fn load_params_toml_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(dir.path(), "test.toml", "x = 1.5\ny = 2\n");
        let vals = load_params_toml(&path).unwrap();
        assert!((vals["x"] - 1.5).abs() < 1e-10);
        assert!((vals["y"] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn load_params_toml_handles_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(dir.path(), "test.toml",
            "# This is a comment\nx = 1.5\n# Another comment\ny = 2.0\n");
        let vals = load_params_toml(&path).unwrap();
        assert_eq!(vals.len(), 2);
        assert!((vals["x"] - 1.5).abs() < 1e-10);
    }
}
