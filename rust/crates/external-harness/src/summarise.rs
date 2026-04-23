//! Aggregate per-seed camdl output TSVs into a standard `Summary`.
//!
//! Reads the `[summary]` block of a case manifest and the per-seed output
//! files produced by `subprocess::run_camdl_seed`, computes the declared
//! statistics, and emits a `Summary` with one row per stat.

use crate::manifest::{AggregateOp, StatSpec, SummarySpec};
use crate::subprocess::CamdlRun;
use crate::summary::Summary;
use std::collections::HashMap;
use std::path::Path;

/// Summarise a long-format ensemble TSV: one row per (seed × time) with a
/// named `seed_col` identifying seeds and `stats.over` naming the value
/// column. This is the natural output shape for R/pomp (`simulate(..., nsim=N)`
/// in long format) and most Python/NumPyro references.
///
/// Groups by seed, then applies each `StatSpec` the same way
/// `summarise_runs` does for the per-seed-directory layout used by camdl.
pub fn summarise_long_tsv(
    spec: &SummarySpec,
    tsv_path: &Path,
    seed_col: &str,
) -> anyhow::Result<Summary> {
    let stats = match spec {
        SummarySpec::EnsembleStats { stats } => stats,
        SummarySpec::Prebaked => {
            return Err(anyhow::anyhow!(
                "summarise_long_tsv called with Prebaked spec"));
        }
    };
    let needed_cols: std::collections::HashSet<&str> = std::iter::once(seed_col)
        .chain(stats.iter().map(|s| s.over.as_str()))
        .collect();
    let cols = read_tsv_columns(tsv_path, &needed_cols)?;

    // Partition rows by seed.
    let seed_values = cols.get(seed_col).ok_or_else(|| anyhow::anyhow!(
        "{}: seed column '{}' missing", tsv_path.display(), seed_col))?;
    let n_rows = seed_values.len();
    let mut per_seed: HashMap<u64, HashMap<String, Vec<f64>>> = HashMap::new();
    for i in 0..n_rows {
        let seed = seed_values[i] as u64;
        let entry = per_seed.entry(seed).or_default();
        for (name, vs) in &cols {
            if name == seed_col { continue; }
            entry.entry(name.clone()).or_default().push(vs[i]);
        }
    }

    let mut summary = Summary::default();
    for stat in stats {
        let samples = aggregate_across_seeds(stat, &per_seed)?;
        let (name, row) = Summary::from_samples(&stat.name, &samples);
        summary.rows.insert(name, row);
    }
    Ok(summary)
}

pub fn summarise_runs(
    spec: &SummarySpec,
    runs: &[CamdlRun],
) -> anyhow::Result<Summary> {
    let stats = match spec {
        SummarySpec::EnsembleStats { stats } => stats,
        SummarySpec::Prebaked => {
            return Err(anyhow::anyhow!(
                "summarise_runs called with Prebaked spec; prebaked cases \
                 should short-circuit before here"));
        }
    };

    // Per-seed column values. key: (seed, column) → Vec<f64>
    let mut per_seed_columns: HashMap<u64, HashMap<String, Vec<f64>>> = HashMap::new();
    let needed_cols: std::collections::HashSet<&str> =
        stats.iter().map(|s| s.over.as_str()).collect();

    for run in runs {
        if !run.succeeded() {
            return Err(anyhow::anyhow!(
                "summarise_runs: seed {} did not exit cleanly (exit={:?}); \
                 check {}", run.seed, run.exit_code, run.stderr_path.display()));
        }
        let obs_path = run.seed_dir.join("obs.tsv");
        let cols = read_tsv_columns(&obs_path, &needed_cols)?;
        per_seed_columns.insert(run.seed, cols);
    }

    let mut summary = Summary::default();
    for stat in stats {
        let samples = aggregate_across_seeds(stat, &per_seed_columns)?;
        let (name, row) = Summary::from_samples(&stat.name, &samples);
        summary.rows.insert(name, row);
    }
    Ok(summary)
}

/// Read only the requested columns from a TSV, keeping parse errors
/// specific. Tab-separated, first row is header.
fn read_tsv_columns(
    path: &Path,
    cols: &std::collections::HashSet<&str>,
) -> anyhow::Result<HashMap<String, Vec<f64>>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {}", path.display(), e))?;
    let mut lines = content.lines();
    let header = lines.next().ok_or_else(|| anyhow::anyhow!(
        "{}: empty TSV", path.display()))?;
    let header_fields: Vec<&str> = header.split('\t').collect();
    let mut col_idx: HashMap<&str, usize> = HashMap::new();
    for (i, name) in header_fields.iter().enumerate() {
        if cols.contains(*name) { col_idx.insert(*name, i); }
    }
    for c in cols {
        if !col_idx.contains_key(c) {
            return Err(anyhow::anyhow!(
                "{}: column {:?} not found in header {:?}",
                path.display(), c, header_fields));
        }
    }
    let mut out: HashMap<String, Vec<f64>> = col_idx.keys().map(|k| (k.to_string(), Vec::new())).collect();
    for (line_no, line) in lines.enumerate() {
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split('\t').collect();
        for (col, &i) in &col_idx {
            let raw = fields.get(i).ok_or_else(|| anyhow::anyhow!(
                "{}:{}: row shorter than header ({} fields, need col {})",
                path.display(), line_no + 2, fields.len(), i))?;
            let v: f64 = raw.parse().map_err(|_| anyhow::anyhow!(
                "{}:{}: cannot parse {} = {:?}",
                path.display(), line_no + 2, col, raw))?;
            out.get_mut(*col).unwrap().push(v);
        }
    }
    Ok(out)
}

fn aggregate_across_seeds(
    stat: &StatSpec,
    per_seed: &HashMap<u64, HashMap<String, Vec<f64>>>,
) -> anyhow::Result<Vec<f64>> {
    let mut samples: Vec<f64> = Vec::with_capacity(per_seed.len());
    for (_seed, cols) in per_seed {
        let col = cols.get(&stat.over).ok_or_else(|| anyhow::anyhow!(
            "stat '{}': column '{}' missing", stat.name, stat.over))?;
        let scoped: &[f64] = match stat.scope.as_deref() {
            Some("per-seed") | None => col,
            Some("last-year-per-seed") => {
                // Assume weekly observations: last 52 rows.
                let n = col.len();
                if n > 52 { &col[n - 52..] } else { col }
            }
            Some(other) => return Err(anyhow::anyhow!(
                "stat '{}': unknown scope '{}'", stat.name, other)),
        };
        let per_seed_value = match stat.aggregate {
            AggregateOp::Sum  => scoped.iter().sum::<f64>(),
            AggregateOp::Max  => scoped.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            AggregateOp::Mean => {
                if scoped.is_empty() { 0.0 } else { scoped.iter().sum::<f64>() / scoped.len() as f64 }
            }
            AggregateOp::Frac => {
                // Per-seed reduction for `frac` is the *total* across scope
                // compared to `threshold`. The across-seeds aggregation then
                // becomes a mean over the 0/1 indicator, producing a rate.
                let total: f64 = scoped.iter().sum();
                let thr = stat.threshold.ok_or_else(|| anyhow::anyhow!(
                    "stat '{}': aggregate 'frac' requires a threshold", stat.name))?;
                if total >= thr { 1.0 } else { 0.0 }
            }
        };
        samples.push(per_seed_value);
    }
    Ok(samples)
}
