/// `camdl experiment analyze` — Sobol sensitivity index computation.
///
/// Reads `parameter_points.tsv` + trajectory TSV files from a design run and
/// computes global sensitivity indices using the Saltelli (2010) estimators.
///
/// **Estimators** (Saltelli et al. 2010, equations (b) and (f)):**
///
///   V_total = Var(Y_A)
///   S1[i]   = (1/n) Σ_j Y_B[j] * (Y_ABi[j] - Y_A[j])  / V_total    [eq. (b)]
///   ST[i]   = (1/2n) Σ_j (Y_A[j] - Y_ABi[j])^2          / V_total    [eq. (f)]
///
/// Output files (per design, per output column):
///   analysis/sensitivity/{design}/sobol_indices.tsv
///   analysis/sensitivity/{design}/convergence.tsv
///   analysis/sensitivity/{design}/assumptions.txt
///
/// Optional: --json writes sobol_indices.json alongside the TSV.

use std::collections::HashMap;

// ─── CLI entry point ──────────────────────────────────────────────────────────

pub fn cmd_experiment_analyze(args: &[String]) {
    let mut toml_path: Option<String> = None;
    let mut design_filter: Option<String> = None;
    let mut output_dir_override: Option<String> = None;
    let mut output_cols: Option<Vec<String>> = None;
    let mut bootstrap_n: usize = 1000;
    let mut confidence: f64 = 0.95;
    let mut emit_json = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--design"     => { i += 1; design_filter = Some(args[i].clone()); }
            "--output"     => { i += 1; output_dir_override = Some(args[i].clone()); }
            "--outputs"    => {
                i += 1;
                output_cols = Some(args[i].split(',').map(|s| s.trim().to_string()).collect());
            }
            "--bootstrap"  => {
                i += 1;
                bootstrap_n = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("error: --bootstrap requires an integer"); std::process::exit(1);
                });
            }
            "--confidence" => {
                i += 1;
                confidence = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("error: --confidence requires a float"); std::process::exit(1);
                });
            }
            "--json" => { emit_json = true; }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                analyze_usage();
            }
            path => { toml_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let toml_path = toml_path.unwrap_or_else(|| {
        eprintln!("error: analyze requires an experiment TOML file path");
        analyze_usage();
    });

    let toml_src = std::fs::read_to_string(&toml_path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", toml_path, e);
        std::process::exit(1);
    });

    // Extract output_dir from TOML (minimal parse)
    let output_dir = output_dir_override.unwrap_or_else(|| {
        extract_output_dir(&toml_src).unwrap_or_else(|| "output".to_string())
    });

    // Extract [analyze] block for outputs list
    let toml_outputs = extract_analyze_outputs(&toml_src);
    let toml_confidence = extract_analyze_confidence(&toml_src).unwrap_or(0.95);
    let effective_confidence = if confidence != 0.95 { confidence } else { toml_confidence };

    // Find design directories
    let designs_root = format!("{}/designs", output_dir);
    let design_dirs: Vec<(String, String)> = if let Some(ref name) = design_filter {
        let d = format!("{}/{}", designs_root, name);
        if !std::path::Path::new(&d).exists() {
            eprintln!("error: design directory not found: {}", d);
            std::process::exit(1);
        }
        vec![(name.clone(), d)]
    } else {
        // Enumerate all subdirectories of designs/
        match std::fs::read_dir(&designs_root) {
            Err(_) => {
                eprintln!("error: no designs directory found at {}", designs_root);
                eprintln!("  Run 'camdl experiment run' first with [design.*] blocks.");
                std::process::exit(1);
            }
            Ok(entries) => {
                let mut dirs: Vec<(String, String)> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| (e.file_name().to_string_lossy().to_string(), e.path().to_string_lossy().to_string()))
                    .collect();
                dirs.sort_by_key(|(n, _)| n.clone());
                dirs
            }
        }
    };

    if design_dirs.is_empty() {
        eprintln!("error: no design directories found in {}", designs_root);
        std::process::exit(1);
    }

    eprintln!("Analyzing {} design(s)...", design_dirs.len());

    let mut all_results: Vec<SobolResult> = Vec::new();

    for (design_name, design_dir) in &design_dirs {
        eprintln!("  Design '{}'", design_name);

        // Load parameter_points.tsv
        let pts_path = format!("{}/parameter_points.tsv", design_dir);
        let (param_names, param_matrix) = match read_tsv_matrix(&pts_path, Some("point_id")) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  warning: cannot read {}: {}", pts_path, e);
                continue;
            }
        };
        let n_points = param_matrix.len();
        if n_points < 4 {
            eprintln!("  warning: too few parameter points ({}) for Sobol analysis", n_points);
            continue;
        }

        // Infer n (block size) from number of parameter points: n_total = n(2+k)
        let k = param_names.len();
        let n = n_points / (2 + k);
        if n * (2 + k) != n_points {
            eprintln!("  warning: {} parameter points is not divisible by (2+{})={}", n_points, k, 2+k);
            eprintln!("  This design was not generated with method=sobol. Skipping Sobol analysis.");
            continue;
        }

        // Determine which output columns to analyze
        let effective_cols = output_cols.as_ref().cloned()
            .or_else(|| toml_outputs.clone())
            .unwrap_or_default();   // empty = use all columns from outputs.tsv

        // Build outputs.tsv path (aggregated from all scenario runs — computed lazily)
        let outputs_path = format!("{}/outputs.tsv", design_dir);
        let output_matrix = match build_outputs_tsv(design_dir, &outputs_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  warning: cannot build outputs.tsv: {}", e);
                continue;
            }
        };

        let (output_names, output_rows) = output_matrix;
        let analyze_cols: Vec<String> = if effective_cols.is_empty() {
            output_names.clone()
        } else {
            effective_cols.iter()
                .filter(|c| output_names.contains(c))
                .cloned()
                .collect()
        };

        if analyze_cols.is_empty() {
            eprintln!("  warning: no matching output columns found");
            continue;
        }

        // Per-point output vector (averaged over seeds)
        let averaged = average_over_seeds(&output_names, &output_rows, n_points);

        // Compute Sobol indices for each output column
        let mut design_results: Vec<SobolResult> = Vec::new();
        for col_name in &analyze_cols {
            let col_idx = match output_names.iter().position(|n| n == col_name) {
                Some(i) => i,
                None => continue,
            };
            let y: Vec<f64> = averaged.iter().map(|row| row[col_idx]).collect();
            let results = compute_sobol_indices(
                &y, n, k, &param_names, col_name, design_name,
                bootstrap_n, effective_confidence,
            );
            design_results.extend(results);
        }

        // Write output files
        let sens_dir = format!("{}/analysis/sensitivity/{}", output_dir, design_name);
        std::fs::create_dir_all(&sens_dir).unwrap_or_else(|e| {
            eprintln!("warning: cannot create {}: {}", sens_dir, e);
        });

        write_sobol_indices_tsv(&format!("{}/sobol_indices.tsv", sens_dir), &design_results);
        write_convergence_tsv(&format!("{}/convergence.tsv", sens_dir),
                              &averaged, n, k, &param_names, &analyze_cols, design_name);
        write_assumptions_txt(&format!("{}/assumptions.txt", sens_dir),
                              &design_results, design_name, &param_names, n, k,
                              bootstrap_n, effective_confidence);
        if emit_json {
            write_sobol_indices_json(&format!("{}/sobol_indices.json", sens_dir), &design_results);
        }

        eprintln!("    Wrote {}/sobol_indices.tsv", sens_dir);
        all_results.extend(design_results);
    }

    eprintln!("Analysis complete. {} result rows.", all_results.len());
}

// ─── Saltelli 2010 estimators ─────────────────────────────────────────────────

/// One row of sensitivity output.
#[derive(Debug, Clone)]
struct SobolResult {
    design: String,
    output: String,
    parameter: String,
    s1: f64,
    s1_ci_low: f64,
    s1_ci_high: f64,
    st: f64,
    st_ci_low: f64,
    st_ci_high: f64,
}

/// Compute first-order (S1) and total-order (ST) Sobol indices.
///
/// Saltelli et al. (2010) estimators, equations (b) and (f):
///   S1[i] = (1/n) Σ Y_B[j] * (Y_ABi[j] - Y_A[j])  / Var(Y_A)    [eq. (b)]
///   ST[i] = (1/2n) Σ (Y_A[j] - Y_ABi[j])^2          / Var(Y_A)   [eq. (f)]
///
/// `y` must have exactly n*(2+k) rows in the Saltelli block layout:
///   rows 0..n         → Y_A
///   rows n..2n        → Y_B
///   rows (2+i)n..(3+i)n → Y_ABi (A with column i replaced by B[:,i])
fn compute_sobol_indices(
    y: &[f64],
    n: usize,
    k: usize,
    param_names: &[String],
    output_name: &str,
    design_name: &str,
    bootstrap_n: usize,
    confidence: f64,
) -> Vec<SobolResult> {
    assert_eq!(y.len(), n * (2 + k), "y length must be n*(2+k)");

    let y_a  = &y[0..n];
    let y_b  = &y[n..2*n];

    let var_ya = variance(y_a);
    if var_ya < 1e-30 {
        // Zero variance — indices are undefined
        return param_names.iter().map(|p| SobolResult {
            design: design_name.to_string(), output: output_name.to_string(),
            parameter: p.clone(),
            s1: 0.0, s1_ci_low: 0.0, s1_ci_high: 0.0,
            st: 0.0, st_ci_low: 0.0, st_ci_high: 0.0,
        }).collect();
    }

    let mut results = Vec::new();
    for (param_idx, param_name) in param_names.iter().enumerate() {
        let y_abi = &y[(2 + param_idx) * n..(3 + param_idx) * n];

        let (s1, st) = saltelli_estimators(y_a, y_b, y_abi, var_ya);

        // Bootstrap CIs: resample row indices jointly
        let alpha = 1.0 - confidence;
        let (s1_lo, s1_hi, st_lo, st_hi) = bootstrap_ci(
            y_a, y_b, y_abi, var_ya, bootstrap_n, alpha, param_idx as u64,
        );

        results.push(SobolResult {
            design: design_name.to_string(),
            output: output_name.to_string(),
            parameter: param_name.clone(),
            s1, s1_ci_low: s1_lo, s1_ci_high: s1_hi,
            st, st_ci_low: st_lo, st_ci_high: st_hi,
        });
    }
    results
}

/// Saltelli 2010 estimators, equations (b) and (f).
fn saltelli_estimators(y_a: &[f64], y_b: &[f64], y_abi: &[f64], var_ya: f64) -> (f64, f64) {
    let n = y_a.len() as f64;

    // S1: eq. (b): (1/n) Σ Y_B * (Y_ABi - Y_A) / Var(Y_A)
    let s1_num: f64 = y_b.iter().zip(y_abi.iter()).zip(y_a.iter())
        .map(|((yb, yabi), ya)| yb * (yabi - ya))
        .sum::<f64>() / n;
    let s1 = s1_num / var_ya;

    // ST: eq. (f): (1/(2n)) Σ (Y_A - Y_ABi)^2 / Var(Y_A)
    let st_num: f64 = y_a.iter().zip(y_abi.iter())
        .map(|(ya, yabi)| (ya - yabi).powi(2))
        .sum::<f64>() / (2.0 * n);
    let st = st_num / var_ya;

    (s1, st)
}

fn variance(y: &[f64]) -> f64 {
    let n = y.len() as f64;
    let mean = y.iter().sum::<f64>() / n;
    y.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n
}

/// Bootstrap confidence intervals for S1 and ST.
///
/// Resamples row indices (with replacement) jointly across Y_A, Y_B, Y_ABi.
/// This preserves the row correspondence between blocks.
fn bootstrap_ci(
    y_a: &[f64], y_b: &[f64], y_abi: &[f64],
    var_ya: f64,
    n_resamples: usize,
    alpha: f64,
    seed_offset: u64,
) -> (f64, f64, f64, f64) {
    let n = y_a.len();
    let mut rng: u64 = 0xdeadbeef12345678u64.wrapping_add(seed_offset);
    let next_idx = |rng: &mut u64| -> usize {
        *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*rng >> 33) as usize) % n
    };

    let mut s1_samples = Vec::with_capacity(n_resamples);
    let mut st_samples = Vec::with_capacity(n_resamples);

    for _ in 0..n_resamples {
        // Resample n row indices with replacement
        let indices: Vec<usize> = (0..n).map(|_| next_idx(&mut rng)).collect();
        let ya_r:  Vec<f64> = indices.iter().map(|&i| y_a[i]).collect();
        let yb_r:  Vec<f64> = indices.iter().map(|&i| y_b[i]).collect();
        let yabi_r: Vec<f64> = indices.iter().map(|&i| y_abi[i]).collect();
        let var_r = variance(&ya_r);
        if var_r < 1e-30 { continue; }
        let (s1_r, st_r) = saltelli_estimators(&ya_r, &yb_r, &yabi_r, var_r);
        s1_samples.push(s1_r);
        st_samples.push(st_r);
    }

    let quantile = |mut v: Vec<f64>, q: f64| -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((v.len() as f64 - 1.0) * q) as usize;
        v.get(idx).copied().unwrap_or(0.0)
    };

    let lo = alpha / 2.0;
    let hi = 1.0 - alpha / 2.0;

    // Use the full var_ya (not resampled) for the point estimate comparison
    let _ = var_ya;

    (
        quantile(s1_samples.clone(), lo),
        quantile(s1_samples, hi),
        quantile(st_samples.clone(), lo),
        quantile(st_samples, hi),
    )
}

// ─── Output aggregation ───────────────────────────────────────────────────────

/// Load or build `outputs.tsv`: one row per (point_id, scenario, seed) with
/// summary statistics from `traj.tsv`.
///
/// For now, computes peak value for each compartment column found in traj.tsv.
/// Returns (column_names, rows) where each row is a Vec<f64> of column values.
fn build_outputs_tsv(
    design_dir: &str,
    outputs_path: &str,
) -> Result<(Vec<String>, Vec<(usize, Vec<f64>)>), String> {
    // If outputs.tsv already exists, read it
    if std::path::Path::new(outputs_path).exists() {
        let (col_names, rows) = read_tsv_with_point_id(outputs_path)?;
        return Ok((col_names, rows));
    }

    // Build from traj.tsv files in runs/ subdirectory
    let runs_dir = format!("{}/runs", design_dir);
    if !std::path::Path::new(&runs_dir).exists() {
        return Err(format!("no runs directory found at {} and no outputs.tsv", runs_dir));
    }

    // Walk the runs tree: runs/{sim_hash_8}/{scen_slug}-{scen_hash_8}/seed_{N}/traj.tsv
    let mut output_col_names: Option<Vec<String>> = None;
    let mut rows: Vec<(usize, Vec<f64>)> = Vec::new();

    for sim_dir in walk_dirs(&runs_dir)? {
        for scen_dir in walk_dirs(&sim_dir)? {
            for seed_dir in walk_dirs(&scen_dir)? {
                let traj_path = format!("{}/traj.tsv", seed_dir);
                if !std::path::Path::new(&traj_path).exists() { continue; }

                // Extract point_id from run.json if available, else skip
                let run_json_path = format!("{}/run.json", seed_dir);
                let point_id = extract_point_id_from_run_json(&run_json_path);

                match compute_peak_outputs(&traj_path) {
                    Err(e) => eprintln!("    warning: {}: {}", traj_path, e),
                    Ok((col_names, peak_values)) => {
                        if output_col_names.is_none() {
                            output_col_names = Some(col_names);
                        }
                        rows.push((point_id.unwrap_or(rows.len()), peak_values));
                    }
                }
            }
        }
    }

    let col_names = output_col_names.ok_or_else(|| "no traj.tsv files found".to_string())?;

    // Write outputs.tsv for future use
    let mut tsv = String::from("point_id");
    for name in &col_names {
        tsv.push('\t');
        tsv.push_str(name);
    }
    tsv.push('\n');
    for (pid, vals) in &rows {
        tsv.push_str(&pid.to_string());
        for v in vals {
            tsv.push('\t');
            tsv.push_str(&format!("{:.6}", v));
        }
        tsv.push('\n');
    }
    let _ = std::fs::write(outputs_path, &tsv);

    Ok((col_names, rows))
}

fn walk_dirs(path: &str) -> Result<Vec<String>, String> {
    std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| Ok(e.path().to_string_lossy().to_string()))
        .collect()
}

fn extract_point_id_from_run_json(path: &str) -> Option<usize> {
    let src = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&src).ok()?;
    v.get("design_point_index")?.as_u64().map(|n| n as usize)
}

/// Compute peak value for each (non-t) column in a traj.tsv.
fn compute_peak_outputs(traj_path: &str) -> Result<(Vec<String>, Vec<f64>), String> {
    let src = std::fs::read_to_string(traj_path).map_err(|e| e.to_string())?;
    let mut lines = src.lines();
    let header = lines.next().ok_or("empty traj.tsv")?;
    let col_names: Vec<String> = header.split('\t')
        .skip(1)   // skip 't' column
        .map(|s| format!("peak_{}", s))
        .collect();
    let k = col_names.len();
    let mut peaks = vec![f64::NEG_INFINITY; k];

    for line in lines {
        let parts: Vec<&str> = line.split('\t').collect();
        for (i, p) in parts.iter().skip(1).enumerate().take(k) {
            if let Ok(v) = p.parse::<f64>() {
                if v > peaks[i] { peaks[i] = v; }
            }
        }
    }
    // Replace -inf with 0 for completely empty columns
    for v in &mut peaks { if v.is_infinite() { *v = 0.0; } }

    Ok((col_names, peaks))
}

/// Average output values over seeds for each point_id.
/// Returns one row per point_id (sorted ascending), with averaged values.
fn average_over_seeds(
    col_names: &[String],
    rows: &[(usize, Vec<f64>)],
    n_points: usize,
) -> Vec<Vec<f64>> {
    let k = col_names.len();
    let mut sums: HashMap<usize, (Vec<f64>, usize)> = HashMap::new();
    for (pid, vals) in rows {
        let entry = sums.entry(*pid).or_insert_with(|| (vec![0.0; k], 0));
        for (i, v) in vals.iter().enumerate().take(k) {
            entry.0[i] += v;
        }
        entry.1 += 1;
    }
    (0..n_points).map(|pid| {
        match sums.get(&pid) {
            None => vec![0.0; k],
            Some((sum, count)) => sum.iter().map(|s| s / *count as f64).collect(),
        }
    }).collect()
}

// ─── TSV helpers ──────────────────────────────────────────────────────────────

/// Read a TSV file, optionally skipping an `id_col` column.
/// Returns (column_names, rows_as_float_vecs).
fn read_tsv_matrix(path: &str, skip_col: Option<&str>) -> Result<(Vec<String>, Vec<Vec<f64>>), String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut lines = src.lines();
    let header_line = lines.next().ok_or("empty file")?;
    let header: Vec<&str> = header_line.split('\t').collect();

    // Find which column to skip
    let skip_idx: Option<usize> = skip_col.and_then(|c| header.iter().position(|h| *h == c));
    let col_names: Vec<String> = header.iter().enumerate()
        .filter(|(i, _)| Some(*i) != skip_idx)
        .map(|(_, h)| h.to_string())
        .collect();

    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() { continue; }
        let parts: Vec<&str> = line.split('\t').collect();
        let row: Vec<f64> = parts.iter().enumerate()
            .filter(|(i, _)| Some(*i) != skip_idx)
            .map(|(_, v)| v.parse::<f64>().unwrap_or(0.0))
            .collect();
        if row.len() == col_names.len() {
            rows.push(row);
        }
    }
    Ok((col_names, rows))
}

/// Read a TSV file with a `point_id` column, returning (non-id columns, rows with point_id).
fn read_tsv_with_point_id(path: &str) -> Result<(Vec<String>, Vec<(usize, Vec<f64>)>), String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut lines = src.lines();
    let header_line = lines.next().ok_or("empty file")?;
    let header: Vec<&str> = header_line.split('\t').collect();
    let id_idx = header.iter().position(|h| *h == "point_id");

    let col_names: Vec<String> = header.iter().enumerate()
        .filter(|(i, _)| Some(*i) != id_idx)
        .map(|(_, h)| h.to_string())
        .collect();

    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() { continue; }
        let parts: Vec<&str> = line.split('\t').collect();
        let point_id = id_idx
            .and_then(|i| parts.get(i))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let vals: Vec<f64> = parts.iter().enumerate()
            .filter(|(i, _)| Some(*i) != id_idx)
            .map(|(_, v)| v.parse::<f64>().unwrap_or(0.0))
            .collect();
        rows.push((point_id, vals));
    }
    Ok((col_names, rows))
}

// ─── TOML extraction (minimal, no full parse needed) ─────────────────────────

fn extract_output_dir(toml_src: &str) -> Option<String> {
    for line in toml_src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("output_dir") {
            if let Some(eq) = trimmed.find('=') {
                let val = trimmed[eq+1..].trim().trim_matches('"').trim_matches('\'');
                return Some(val.to_string());
            }
        }
    }
    None
}

fn extract_analyze_outputs(toml_src: &str) -> Option<Vec<String>> {
    // Look for outputs = ["X", "Y", "Z"] in [analyze] section
    let mut in_analyze = false;
    for line in toml_src.lines() {
        let trimmed = line.trim();
        if trimmed == "[analyze]" { in_analyze = true; continue; }
        if trimmed.starts_with('[') { in_analyze = false; continue; }
        if in_analyze && trimmed.starts_with("outputs") {
            if let Some(bracket) = trimmed.find('[') {
                if let Some(end) = trimmed.rfind(']') {
                    let inner = &trimmed[bracket+1..end];
                    let names: Vec<String> = inner.split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !names.is_empty() { return Some(names); }
                }
            }
        }
    }
    None
}

fn extract_analyze_confidence(toml_src: &str) -> Option<f64> {
    let mut in_analyze = false;
    for line in toml_src.lines() {
        let trimmed = line.trim();
        if trimmed == "[analyze]" { in_analyze = true; continue; }
        if trimmed.starts_with('[') { in_analyze = false; continue; }
        if in_analyze && trimmed.starts_with("confidence") {
            if let Some(eq) = trimmed.find('=') {
                return trimmed[eq+1..].trim().parse::<f64>().ok();
            }
        }
    }
    None
}

// ─── File writers ─────────────────────────────────────────────────────────────

fn write_sobol_indices_tsv(path: &str, results: &[SobolResult]) {
    let mut tsv = String::from("design\toutput\tparameter\tS1\tS1_ci_low\tS1_ci_high\tST\tST_ci_low\tST_ci_high\n");
    for r in results {
        tsv.push_str(&format!("{}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\n",
            r.design, r.output, r.parameter,
            r.s1, r.s1_ci_low, r.s1_ci_high,
            r.st, r.st_ci_low, r.st_ci_high));
    }
    let _ = std::fs::write(path, &tsv);
}

fn write_sobol_indices_json(path: &str, results: &[SobolResult]) {
    let entries: Vec<serde_json::Value> = results.iter().map(|r| {
        serde_json::json!({
            "design": r.design,
            "output": r.output,
            "parameter": r.parameter,
            "S1": r.s1, "S1_ci_low": r.s1_ci_low, "S1_ci_high": r.s1_ci_high,
            "ST": r.st, "ST_ci_low": r.st_ci_low, "ST_ci_high": r.st_ci_high,
        })
    }).collect();
    let json = serde_json::to_string_pretty(&serde_json::json!({"sobol_indices": entries}))
        .unwrap_or_default();
    let _ = std::fs::write(path, json);
}

fn write_convergence_tsv(
    path: &str,
    averaged: &[Vec<f64>],
    n: usize,
    k: usize,
    param_names: &[String],
    output_names: &[String],
    design_name: &str,
) {
    let mut tsv = String::from("design\toutput\tparameter\tn_samples\tS1\tST\n");

    // Compute indices at n/8, n/4, n/2, n
    let checkpoints = [n / 8, n / 4, n / 2, n];
    for &cp in &checkpoints {
        if cp == 0 { continue; }
        for (col_idx, col_name) in output_names.iter().enumerate() {
            let y_sub: Vec<f64> = averaged.iter().map(|r| r[col_idx]).collect();
            let total = cp * (2 + k);
            if total > y_sub.len() { continue; }
            let y_slice = &y_sub[..total];
            let var_ya = variance(&y_slice[0..cp]);
            if var_ya < 1e-30 { continue; }
            for (pi, pname) in param_names.iter().enumerate() {
                let y_a   = &y_slice[0..cp];
                let y_b   = &y_slice[cp..2*cp];
                let y_abi = &y_slice[(2+pi)*cp..(3+pi)*cp];
                let (s1, st) = saltelli_estimators(y_a, y_b, y_abi, var_ya);
                tsv.push_str(&format!("{}\t{}\t{}\t{}\t{:.4}\t{:.4}\n",
                    design_name, col_name, pname, cp, s1, st));
            }
        }
    }
    let _ = std::fs::write(path, &tsv);
}

fn write_assumptions_txt(
    path: &str,
    results: &[SobolResult],
    design_name: &str,
    param_names: &[String],
    n: usize,
    k: usize,
    bootstrap_n: usize,
    confidence: f64,
) {
    let _ = param_names;
    let mut txt = String::from("Sensitivity analysis assumptions:\n\n");
    txt.push_str("Methodology:\n");
    txt.push_str("  Sobol first-order and total-order indices (Saltelli et al. 2010)\n");
    txt.push_str(&format!("  Bootstrap confidence intervals ({} resamples, {:.0}% level)\n\n",
        bootstrap_n, confidence * 100.0));
    txt.push_str("Sampling:\n");
    txt.push_str("  Quasi-random (Halton sequence) over specified parameter ranges\n");
    txt.push_str("  Output variance decomposition is CONDITIONAL on these bounds\n");
    txt.push_str("  Different bounds (designs) produce different indices\n\n");
    txt.push_str(&format!("Design \"{}\":\n", design_name));
    txt.push_str(&format!("  Base sample size: n = {}\n", n));
    txt.push_str(&format!("  Parameters: k = {}\n", k));
    txt.push_str(&format!("  Total points: n(2k+2) = {}\n\n", n * (2 + k)));
    txt.push_str("Results summary:\n");
    for r in results {
        txt.push_str(&format!("  {} | {} | {}: S1={:.3} ST={:.3}\n",
            r.design, r.output, r.parameter, r.s1, r.st));
    }
    txt.push('\n');
    txt.push_str("Cross-design VOI interpretation:\n");
    txt.push_str("  Comparing sensitivity indices across designs estimates the VALUE of\n");
    txt.push_str("  narrowing parameter uncertainty. This is a sensitivity landscape\n");
    txt.push_str("  comparison, NOT a Bayesian posterior. The magnitude depends on the\n");
    txt.push_str("  assumed ranges. For prior-weighted analysis, use parameter_points.tsv\n");
    txt.push_str("  and outputs.tsv with importance weights.\n");
    let _ = std::fs::write(path, &txt);
}

// ─── Usage ────────────────────────────────────────────────────────────────────

fn analyze_usage() -> ! {
    eprintln!("usage: camdl experiment analyze EXPERIMENT.toml [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --design NAME       analyze specific design (default: all)");
    eprintln!("  --output DIR        output root override");
    eprintln!("  --outputs X,Y,Z     output columns to analyze (default: all)");
    eprintln!("  --bootstrap N       CI resamples (default: 1000)");
    eprintln!("  --confidence F      CI level (default: 0.95)");
    eprintln!("  --json              also write sobol_indices.json");
    std::process::exit(1);
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn linspace(min: f64, max: f64, n: usize) -> Vec<f64> {
        (0..n).map(|i| min + (max - min) * i as f64 / (n - 1) as f64).collect()
    }

    /// Build a synthetic Saltelli output for a linear model: y = a*x1 + b*x2
    /// where x1, x2 are sampled from their respective matrices.
    fn linear_model_y(
        param_matrix: &[Vec<f64>],
        a: f64, b: f64,
    ) -> Vec<f64> {
        param_matrix.iter().map(|row| a * row[0] + b * row[1]).collect()
    }

    #[test]
    fn variance_correct() {
        let v = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let var = variance(&v);
        // Population variance: mean=3, var = (4+1+0+1+4)/5 = 2.0
        assert!((var - 2.0).abs() < 1e-10, "variance = {}", var);
    }

    #[test]
    fn saltelli_estimator_linear_model() {
        // For a linear model y = a*x1 + b*x2 with x1, x2 ~ Uniform[0,1]:
        // Var(y) = a^2 * Var(x1) + b^2 * Var(x2) = (a^2 + b^2) / 12
        // S1[x1] = a^2 / (a^2 + b^2)
        // ST[x1] = a^2 / (a^2 + b^2)  (no interactions in linear model)
        use crate::sampling::saltelli_matrices;

        let n = 256;
        let k = 2;
        let a = 3.0_f64;
        let b = 1.0_f64;

        let mat = saltelli_matrices(n, k);
        let y = linear_model_y(&mat, a, b);

        let y_a  = &y[0..n];
        let y_b  = &y[n..2*n];
        let y_ab0 = &y[2*n..3*n];  // column 0 replaced by B
        let y_ab1 = &y[3*n..4*n];  // column 1 replaced by B

        let var_ya = variance(y_a);
        let (s1_x1, st_x1) = saltelli_estimators(y_a, y_b, y_ab0, var_ya);
        let (s1_x2, st_x2) = saltelli_estimators(y_a, y_b, y_ab1, var_ya);

        let expected_s1 = a.powi(2) / (a.powi(2) + b.powi(2));
        let expected_s2 = b.powi(2) / (a.powi(2) + b.powi(2));

        // Allow 5% tolerance — Halton sequences have finite-sample bias
        assert!((s1_x1 - expected_s1).abs() < 0.05,
            "S1[x1]={:.3} expected≈{:.3}", s1_x1, expected_s1);
        assert!((st_x1 - expected_s1).abs() < 0.05,
            "ST[x1]={:.3} expected≈{:.3}", st_x1, expected_s1);
        assert!((s1_x2 - expected_s2).abs() < 0.05,
            "S1[x2]={:.3} expected≈{:.3}", s1_x2, expected_s2);
        assert!((st_x2 - expected_s2).abs() < 0.05,
            "ST[x2]={:.3} expected≈{:.3}", st_x2, expected_s2);

        // S1 + S2 ≈ 1 for additive model
        assert!((s1_x1 + s1_x2 - 1.0).abs() < 0.05,
            "S1 should sum to 1 for additive model: {:.3}+{:.3}", s1_x1, s1_x2);
    }

    #[test]
    fn bootstrap_ci_covers_point_estimate() {
        use crate::sampling::saltelli_matrices;

        let n = 128;
        let k = 2;
        let mat = saltelli_matrices(n, k);
        let y = linear_model_y(&mat, 2.0, 1.0);

        let y_a   = &y[0..n];
        let y_b   = &y[n..2*n];
        let y_ab0 = &y[2*n..3*n];
        let var_ya = variance(y_a);
        let (s1, _) = saltelli_estimators(y_a, y_b, y_ab0, var_ya);
        let (lo, hi, _, _) = bootstrap_ci(y_a, y_b, y_ab0, var_ya, 500, 0.05, 0);

        assert!(lo <= s1 && s1 <= hi,
            "point estimate {:.3} not in CI [{:.3}, {:.3}]", s1, lo, hi);
        assert!(lo < hi, "CI should have positive width");
    }

    #[test]
    fn compute_sobol_results_structure() {
        use crate::sampling::saltelli_matrices;

        let n = 64;
        let k = 3;
        let mat = saltelli_matrices(n, k);
        let y = mat.iter().map(|r| r[0] * 2.0 + r[1] * 0.5 + r[2] * 0.1).collect::<Vec<_>>();
        let param_names: Vec<String> = vec!["a".into(), "b".into(), "c".into()];

        let results = compute_sobol_indices(&y, n, k, &param_names, "test_out", "design1", 200, 0.95);
        assert_eq!(results.len(), 3);
        for r in &results {
            assert!(r.s1_ci_low <= r.s1, "CI low {} > point estimate {}", r.s1_ci_low, r.s1);
            assert!(r.st_ci_low <= r.st, "CI low {} > point estimate {}", r.st_ci_low, r.st);
        }

        // The first parameter (a, coefficient 2.0) should have the highest S1
        let s1_a = results.iter().find(|r| r.parameter == "a").unwrap().s1;
        let s1_b = results.iter().find(|r| r.parameter == "b").unwrap().s1;
        let s1_c = results.iter().find(|r| r.parameter == "c").unwrap().s1;
        assert!(s1_a > s1_b, "a (coeff 2) should dominate b (coeff 0.5): {:.3} vs {:.3}", s1_a, s1_b);
        assert!(s1_b > s1_c, "b (coeff 0.5) should dominate c (coeff 0.1): {:.3} vs {:.3}", s1_b, s1_c);
    }

    #[test]
    fn tsv_matrix_parsing() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "point_id\tx\ty\n0\t1.0\t2.0\n1\t3.0\t4.0\n").unwrap();
        let (cols, rows) = read_tsv_matrix(tmp.path().to_str().unwrap(), Some("point_id")).unwrap();
        assert_eq!(cols, vec!["x", "y"]);
        assert_eq!(rows.len(), 2);
        assert!((rows[0][0] - 1.0).abs() < 1e-10);
        assert!((rows[1][1] - 4.0).abs() < 1e-10);
    }

    #[test]
    fn average_over_seeds_groups_by_point_id() {
        let col_names = vec!["out".to_string()];
        let rows = vec![
            (0, vec![10.0]),
            (0, vec![20.0]),   // same point_id, different seed
            (1, vec![30.0]),
        ];
        let averaged = average_over_seeds(&col_names, &rows, 2);
        assert!((averaged[0][0] - 15.0).abs() < 1e-10, "point 0 avg should be 15");
        assert!((averaged[1][0] - 30.0).abs() < 1e-10, "point 1 avg should be 30");
    }

    fn _suppress(_x: Vec<f64>) {} // suppress unused warning for linspace helper
}
