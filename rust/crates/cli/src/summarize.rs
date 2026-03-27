//! `camdl experiment summarize` — aggregate trajectory TSVs into outputs.tsv.
//!
//! Two modes:
//!
//! **Design mode** (experiment.toml argument):
//!   Walks `designs/{design}/runs/` for each design block, reads `run.json` to
//!   recover (point_id, scenario, seed), computes summary statistics from
//!   `traj.tsv`, and writes `designs/{design}/outputs.tsv` with schema:
//!
//!     point_id  scenario  seed  peak_X  tpeak_X  final_X  cum_X  ...
//!
//!   This is the canonical intermediate file consumed by `camdl experiment analyze`
//!   and `camdl voi run`.
//!
//! **Manifest mode** (output-dir argument, legacy):
//!   Reads `manifest.json` and writes per-scenario summary tables to
//!   `analysis/summaries/{scenario}.tsv` — one row per seed.

use std::collections::HashMap;
use serde::Deserialize;

// ─── Manifest-mode types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Manifest {
    scenarios: Vec<String>,
    #[allow(dead_code)]
    output_dir: String,
    runs: Vec<ManifestRun>,
}

#[derive(Deserialize)]
struct ManifestRun {
    scenario: String,
    seed: u64,
    run_path: String,
}

// ─── TSV helpers ──────────────────────────────────────────────────────────────

/// Parse a trajectory TSV. Returns (headers, rows) where each row is Vec<f64>.
fn parse_traj_tsv(path: &str) -> Result<(Vec<String>, Vec<Vec<f64>>), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path, e))?;
    let mut lines = content.lines();
    let header_line = lines.next().ok_or_else(|| format!("empty file: {}", path))?;
    let headers: Vec<String> = header_line.split('\t').map(|s| s.to_string()).collect();
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for line in lines {
        if line.trim().is_empty() { continue; }
        let vals: Vec<f64> = line.split('\t')
            .map(|s| s.trim().parse::<f64>().unwrap_or(f64::NAN))
            .collect();
        rows.push(vals);
    }
    Ok((headers, rows))
}

/// Compute summary scalars for a single trajectory.
/// Returns a sorted Vec of (column_name, value) pairs.
///
/// For each non-time column produces: peak_X, tpeak_X, final_X, cum_X.
pub fn summarize_trajectory(headers: &[String], rows: &[Vec<f64>]) -> Vec<(String, f64)> {
    if rows.is_empty() || headers.is_empty() { return vec![]; }
    let n_cols = headers.len();
    let mut peak: Vec<f64>   = vec![f64::NEG_INFINITY; n_cols];
    let mut tpeak: Vec<f64>  = vec![0.0; n_cols];
    let mut cumsum: Vec<f64> = vec![0.0; n_cols];

    for row in rows {
        let t = row.first().copied().unwrap_or(0.0);
        for (j, &v) in row.iter().enumerate().take(n_cols) {
            if v > peak[j] { peak[j] = v; tpeak[j] = t; }
            cumsum[j] += v;
        }
    }
    let last = rows.last().unwrap();

    let mut out: Vec<(String, f64)> = Vec::new();
    for (j, hdr) in headers.iter().enumerate() {
        if hdr == "t" { continue; }
        let pv = if peak[j].is_infinite() { 0.0 } else { peak[j] };
        out.push((format!("peak_{}", hdr),  pv));
        out.push((format!("tpeak_{}", hdr), tpeak[j]));
        out.push((format!("final_{}", hdr), last.get(j).copied().unwrap_or(f64::NAN)));
        out.push((format!("cum_{}", hdr),   cumsum[j]));
    }
    out.sort_by(|(a, _), (b, _)| a.cmp(b));
    out
}

// ─── Design-mode: run.json reading ────────────────────────────────────────────

/// Read (design_point_index, scenario, seed) from run.json.
///
/// Falls back to path parsing for legacy run.json that only contains
/// `design_point_index` (no `scenario`/`seed` fields). Path structure:
/// `runs/{sim_hash}/{slug}-{scen_hash}/seed_{N}/`
fn read_run_json(run_dir: &str) -> Option<(usize, String, u64)> {
    let src = std::fs::read_to_string(format!("{}/run.json", run_dir)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&src).ok()?;
    let point_id = v["design_point_index"].as_u64()? as usize;

    // Prefer fields written by the new experiment runner
    let scenario = if let Some(s) = v["scenario"].as_str() {
        s.to_string()
    } else {
        // Legacy fallback: parse from path `…/{slug}-{8hex}/seed_N/`
        parse_scenario_from_path(run_dir)?
    };
    let seed = if let Some(s) = v["seed"].as_u64() {
        s
    } else {
        parse_seed_from_path(run_dir)?
    };

    Some((point_id, scenario, seed))
}

/// Extract scenario slug from `…/{slug}-{8hex}/seed_N/` path.
fn parse_scenario_from_path(run_dir: &str) -> Option<String> {
    // run_dir ends with seed_N; parent dir is {slug}-{8hex}
    let path = std::path::Path::new(run_dir);
    let scen_dir = path.parent()?.file_name()?.to_str()?;
    // Strip trailing `-{8 hex chars}`
    if scen_dir.len() > 9 {
        let (slug_part, hash_part) = scen_dir.split_at(scen_dir.len() - 9);
        if hash_part.starts_with('-') && hash_part[1..].chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(slug_part.to_string());
        }
    }
    // No recognizable hash suffix — use whole dir name
    Some(scen_dir.to_string())
}

/// Extract seed from `…/seed_{N}/` path.
fn parse_seed_from_path(run_dir: &str) -> Option<u64> {
    let dir_name = std::path::Path::new(run_dir).file_name()?.to_str()?;
    dir_name.strip_prefix("seed_")?.parse().ok()
}

fn walk_dirs(path: &str) -> Vec<String> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path().to_string_lossy().to_string())
        .collect()
}

// ─── Design-mode entry point ──────────────────────────────────────────────────

fn summarize_design(toml_path: &str) {
    let toml_src = std::fs::read_to_string(toml_path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", toml_path, e);
        std::process::exit(1);
    });

    let output_dir = extract_output_dir(&toml_src).unwrap_or_else(|| "output".to_string());
    let design_names = extract_design_names(&toml_src);

    if design_names.is_empty() {
        eprintln!("error: no [design.*] blocks found in {}", toml_path);
        std::process::exit(1);
    }

    eprintln!("Summarizing {} design(s) in {}...", design_names.len(), output_dir);

    for design_name in &design_names {
        let design_dir = format!("{}/designs/{}", output_dir, design_name);
        let runs_dir   = format!("{}/runs", design_dir);

        if !std::path::Path::new(&runs_dir).exists() {
            eprintln!("  warning: no runs/ directory for design '{}' — skipping", design_name);
            continue;
        }

        // Collect one row per completed run
        struct RunRow {
            point_id: usize,
            scenario: String,
            seed:     u64,
            summary:  Vec<(String, f64)>,
        }
        let mut run_rows: Vec<RunRow> = Vec::new();
        let mut col_order: Option<Vec<String>> = None;

        for sim_dir in walk_dirs(&runs_dir) {
            for scen_dir in walk_dirs(&sim_dir) {
                for seed_dir in walk_dirs(&scen_dir) {
                    let traj_path = format!("{}/traj.tsv", seed_dir);
                    if !std::path::Path::new(&traj_path).exists() { continue; }

                    let meta = match read_run_json(&seed_dir) {
                        Some(m) => m,
                        None => {
                            eprintln!("    warning: missing/incomplete run.json in {} — skipping",
                                seed_dir);
                            continue;
                        }
                    };
                    let (point_id, scenario, seed) = meta;

                    match parse_traj_tsv(&traj_path) {
                        Err(e) => eprintln!("    warning: {}", e),
                        Ok((headers, rows)) => {
                            let summary = summarize_trajectory(&headers, &rows);
                            if col_order.is_none() {
                                col_order = Some(summary.iter().map(|(k, _)| k.clone()).collect());
                            }
                            run_rows.push(RunRow { point_id, scenario, seed, summary });
                        }
                    }
                }
            }
        }

        if run_rows.is_empty() {
            eprintln!("  warning: no completed runs found for design '{}'", design_name);
            continue;
        }

        let col_order = col_order.unwrap();
        // Sort for deterministic output: point_id, then scenario, then seed
        run_rows.sort_by_key(|r| (r.point_id, r.scenario.clone(), r.seed));

        // Build outputs.tsv
        let mut tsv = String::from("point_id\tscenario\tseed");
        for col in &col_order {
            tsv.push('\t');
            tsv.push_str(col);
        }
        tsv.push('\n');

        for row in &run_rows {
            let summary_map: HashMap<&str, f64> =
                row.summary.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            tsv.push_str(&row.point_id.to_string());
            tsv.push('\t');
            tsv.push_str(&row.scenario);
            tsv.push('\t');
            tsv.push_str(&row.seed.to_string());
            for col in &col_order {
                tsv.push('\t');
                let v = summary_map.get(col.as_str()).copied().unwrap_or(f64::NAN);
                if v.is_nan() {
                    tsv.push_str("NaN");
                } else if v.fract() == 0.0 && v.abs() < 1e15 {
                    tsv.push_str(&(v as i64).to_string());
                } else {
                    tsv.push_str(&format!("{:.6}", v));
                }
            }
            tsv.push('\n');
        }

        let out_path = format!("{}/outputs.tsv", design_dir);
        std::fs::write(&out_path, &tsv).unwrap_or_else(|e| {
            eprintln!("error: cannot write {}: {}", out_path, e);
            std::process::exit(1);
        });
        eprintln!("  {} → {} runs, {} summary columns",
            out_path, run_rows.len(), col_order.len());
    }
}

// ─── Manifest-mode entry point ────────────────────────────────────────────────

fn summarize_manifest(output_dir: &str) {
    let manifest_path = format!("{}/manifest.json", output_dir);
    let manifest_src = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", manifest_path, e);
        std::process::exit(1);
    });
    let manifest: Manifest = serde_json::from_str(&manifest_src).unwrap_or_else(|e| {
        eprintln!("error: manifest parse error: {}", e);
        std::process::exit(1);
    });

    let summary_dir = format!("{}/analysis/summaries", output_dir);
    std::fs::create_dir_all(&summary_dir).unwrap_or_else(|e| {
        eprintln!("error: cannot create {}: {}", summary_dir, e);
        std::process::exit(1);
    });

    let mut total_written = 0usize;
    let mut total_errors  = 0usize;

    for scenario in &manifest.scenarios {
        let runs: Vec<&ManifestRun> = manifest.runs.iter()
            .filter(|r| &r.scenario == scenario)
            .collect();
        if runs.is_empty() {
            eprintln!("warning: no completed runs for scenario '{}'", scenario);
            continue;
        }

        let first_traj = format!("{}/runs/{}/traj.tsv", output_dir, runs[0].run_path);
        let (headers, first_rows) = match parse_traj_tsv(&first_traj) {
            Ok(r) => r,
            Err(e) => { eprintln!("error: {}", e); total_errors += 1; continue; }
        };
        let proto = summarize_trajectory(&headers, &first_rows);
        let summary_cols: Vec<String> = proto.iter().map(|(k, _)| k.clone()).collect();

        let mut seed_rows: Vec<(u64, Vec<(String, f64)>)> = Vec::new();
        for run in &runs {
            let traj_path = format!("{}/runs/{}/traj.tsv", output_dir, run.run_path);
            match parse_traj_tsv(&traj_path) {
                Err(e) => { eprintln!("warning: {}", e); total_errors += 1; }
                Ok((hdrs, rows)) => { seed_rows.push((run.seed, summarize_trajectory(&hdrs, &rows))); }
            }
        }
        seed_rows.sort_by_key(|(seed, _)| *seed);

        let out_path = format!("{}/{}.tsv", summary_dir, scenario);
        let mut out = String::from("seed");
        for col in &summary_cols { out.push('\t'); out.push_str(col); }
        out.push('\n');
        for (seed, sums) in &seed_rows {
            let map: HashMap<&str, f64> = sums.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            out.push_str(&seed.to_string());
            for col in &summary_cols {
                out.push('\t');
                let v = map.get(col.as_str()).copied().unwrap_or(f64::NAN);
                if v.is_nan() { out.push_str("NaN"); }
                else if v.fract() == 0.0 && v.abs() < 1e15 { out.push_str(&(v as i64).to_string()); }
                else { out.push_str(&format!("{:.4}", v)); }
            }
            out.push('\n');
        }
        std::fs::write(&out_path, &out).unwrap_or_else(|e| {
            eprintln!("error: cannot write {}: {}", out_path, e);
            std::process::exit(1);
        });
        eprintln!("  {} → {} rows, {} columns", out_path, seed_rows.len(), summary_cols.len());
        total_written += 1;
    }

    eprintln!("Done: {} summaries written ({} errors)", total_written, total_errors);
    if total_errors > 0 { std::process::exit(1); }
}

// ─── CLI entry point ──────────────────────────────────────────────────────────

pub fn cmd_experiment_summarize(args: &[String]) {
    let mut path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => usage(),
            s if s.starts_with("--") => { eprintln!("unknown flag: {}", s); usage(); }
            p => {
                if path.is_some() { eprintln!("unexpected argument: {}", p); usage(); }
                path = Some(p.to_string());
            }
        }
        i += 1;
    }
    let path = path.unwrap_or_else(|| usage());

    if path.ends_with(".toml") {
        summarize_design(&path);
    } else {
        summarize_manifest(&path);
    }
}

fn usage() -> ! {
    eprintln!("usage: camdl experiment summarize EXPERIMENT.toml");
    eprintln!("       camdl experiment summarize OUTPUT-DIR");
    eprintln!();
    eprintln!("  EXPERIMENT.toml — design experiment: writes designs/*/outputs.tsv");
    eprintln!("  OUTPUT-DIR      — manifest experiment: writes analysis/summaries/*.tsv");
    std::process::exit(1);
}

// ─── TOML extraction helpers ──────────────────────────────────────────────────

fn extract_output_dir(toml_src: &str) -> Option<String> {
    for line in toml_src.lines() {
        let t = line.trim();
        if t.starts_with("output_dir") {
            if let Some(eq) = t.find('=') {
                return Some(t[eq+1..].trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    None
}

fn extract_design_names(toml_src: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in toml_src.lines() {
        let t = line.trim();
        if t.starts_with("[design.") && !t.contains("parameters") {
            let inner = t.trim_start_matches('[').trim_end_matches(']');
            // "[design.NAME]" → "NAME"
            if let Some(name) = inner.strip_prefix("design.") {
                let name = name.trim().to_string();
                if !name.is_empty() && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names
}
