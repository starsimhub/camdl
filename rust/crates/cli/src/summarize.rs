//! `camdl experiment summarize` — aggregate trajectory TSVs into per-scenario summary tables.
//!
//! For each scenario in the experiment manifest, reads all completed run trajectories
//! and computes per-seed summary scalars:
//!   - peak_{col}         : max value of column across the trajectory
//!   - final_{col}        : value at last time point
//!   - cum_{flow_col}     : cumulative sum of flow columns (totals across the run)
//!   - tpeak_{col}        : time at which the column reaches its peak
//!
//! Output: `{output_dir}/analysis/summaries/{scenario}.tsv`
//! One row per seed; columns derived from trajectory headers.

use std::collections::HashMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Manifest {
    scenarios: Vec<String>,
    output_dir: String,
    runs: Vec<RunEntry>,
}

#[derive(Debug, Deserialize)]
struct RunEntry {
    scenario: String,
    seed: u64,
    input_hash: String,
}

fn usage() -> ! {
    eprintln!("usage: camdl experiment summarize <output-dir>");
    eprintln!();
    eprintln!("  Reads manifest.json from <output-dir>, loads all completed trajectory");
    eprintln!("  TSVs, and writes per-scenario summary tables to:");
    eprintln!("    <output-dir>/analysis/summaries/<scenario>.tsv");
    eprintln!();
    eprintln!("  Summary columns per trajectory:");
    eprintln!("    peak_<col>    — maximum value across all time points");
    eprintln!("    tpeak_<col>   — time at which the column reaches its maximum");
    eprintln!("    final_<col>   — value at the last recorded time point");
    eprintln!("    cum_<col>     — cumulative sum (for flow_ columns: total events fired)");
    std::process::exit(1);
}

/// Parse a TSV trajectory file. Returns (headers, rows) where each row is a Vec<f64>.
fn parse_tsv(path: &str) -> Result<(Vec<String>, Vec<Vec<f64>>), String> {
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
/// Returns a HashMap: summary_column_name → value.
fn summarize_trajectory(headers: &[String], rows: &[Vec<f64>]) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    if rows.is_empty() || headers.is_empty() { return out; }

    let n_cols = headers.len();

    // Initialize accumulators
    let mut peak: Vec<f64>    = vec![f64::NEG_INFINITY; n_cols];
    let mut tpeak: Vec<f64>   = vec![0.0; n_cols];
    let mut cumsum: Vec<f64>  = vec![0.0; n_cols];

    for row in rows {
        let t = row.first().copied().unwrap_or(0.0);
        for (j, &v) in row.iter().enumerate() {
            if j >= n_cols { break; }
            if v > peak[j] {
                peak[j] = v;
                tpeak[j] = t;
            }
            cumsum[j] += v;
        }
    }

    let last = rows.last().unwrap();

    for (j, hdr) in headers.iter().enumerate() {
        if hdr == "t" { continue; }  // skip time column

        let pv = if peak[j] == f64::NEG_INFINITY { 0.0 } else { peak[j] };
        out.insert(format!("peak_{}", hdr), pv);
        out.insert(format!("tpeak_{}", hdr), tpeak[j]);
        out.insert(format!("final_{}", hdr), last.get(j).copied().unwrap_or(f64::NAN));

        // Cumulative sum is most meaningful for flow columns, but include all.
        out.insert(format!("cum_{}", hdr), cumsum[j]);
    }

    out
}

pub fn cmd_experiment_summarize(args: &[String]) {
    let mut output_dir: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => usage(),
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                usage();
            }
            path => {
                if output_dir.is_some() {
                    eprintln!("unexpected argument: {}", path);
                    usage();
                }
                output_dir = Some(path.to_string());
            }
        }
        i += 1;
    }

    let output_dir = output_dir.unwrap_or_else(|| usage());

    // Load manifest
    let manifest_path = format!("{}/manifest.json", output_dir);
    let manifest_src = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", manifest_path, e);
        std::process::exit(1);
    });
    let manifest: Manifest = serde_json::from_str(&manifest_src).unwrap_or_else(|e| {
        eprintln!("error: manifest parse error: {}", e);
        std::process::exit(1);
    });

    // Create analysis/summaries output dir
    let summary_dir = format!("{}/analysis/summaries", output_dir);
    std::fs::create_dir_all(&summary_dir).unwrap_or_else(|e| {
        eprintln!("error: cannot create {}: {}", summary_dir, e);
        std::process::exit(1);
    });

    let mut total_written = 0usize;
    let mut total_errors  = 0usize;

    for scenario in &manifest.scenarios {
        let runs: Vec<&RunEntry> = manifest.runs.iter()
            .filter(|r| &r.scenario == scenario)
            .collect();

        if runs.is_empty() {
            eprintln!("warning: no completed runs for scenario '{}'", scenario);
            continue;
        }

        // Determine summary columns from the first trajectory
        let first_run = runs[0];
        let first_traj_path = format!("{}/runs/{}/traj.tsv", output_dir, first_run.input_hash);
        let (headers, first_rows) = match parse_tsv(&first_traj_path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {}", e);
                total_errors += 1;
                continue;
            }
        };

        // Build ordered list of summary column names from the first trajectory
        let proto = summarize_trajectory(&headers, &first_rows);
        // Order: peak_*, tpeak_*, final_*, cum_* — deterministic via sorted keys
        let mut summary_cols: Vec<String> = proto.keys().cloned().collect();
        summary_cols.sort();

        // Collect rows: one per seed
        let mut seed_rows: Vec<(u64, HashMap<String, f64>)> = Vec::new();

        for run in &runs {
            let traj_path = format!("{}/runs/{}/traj.tsv", output_dir, run.input_hash);
            match parse_tsv(&traj_path) {
                Err(e) => {
                    eprintln!("warning: {}", e);
                    total_errors += 1;
                }
                Ok((hdrs, rows)) => {
                    let sums = summarize_trajectory(&hdrs, &rows);
                    seed_rows.push((run.seed, sums));
                }
            }
        }

        // Sort by seed for reproducible output
        seed_rows.sort_by_key(|(seed, _)| *seed);

        // Write summary TSV
        let out_path = format!("{}/{}.tsv", summary_dir, scenario);
        let mut out = String::new();

        // Header line
        out.push_str("seed");
        for col in &summary_cols {
            out.push('\t');
            out.push_str(col);
        }
        out.push('\n');

        // Data rows
        for (seed, sums) in &seed_rows {
            out.push_str(&seed.to_string());
            for col in &summary_cols {
                out.push('\t');
                let v = sums.get(col).copied().unwrap_or(f64::NAN);
                // Format: integer-valued floats without decimals for cleaner output
                if v.is_nan() {
                    out.push_str("NaN");
                } else if v.fract() == 0.0 && v.abs() < 1e15 {
                    out.push_str(&(v as i64).to_string());
                } else {
                    out.push_str(&format!("{:.4}", v));
                }
            }
            out.push('\n');
        }

        std::fs::write(&out_path, &out).unwrap_or_else(|e| {
            eprintln!("error: cannot write {}: {}", out_path, e);
            std::process::exit(1);
        });

        eprintln!("  {} → {} rows, {} summary columns", out_path, seed_rows.len(), summary_cols.len());
        total_written += 1;
    }

    eprintln!(
        "Done: {} scenario summaries written to {}/analysis/summaries/  ({} errors)",
        total_written, output_dir, total_errors
    );

    if total_errors > 0 {
        std::process::exit(1);
    }
}
