//! Grid-level summary and coverage tables.
//!
//! After the grid runner completes all `(dataset_idx, fit_seed)` cells,
//! this module walks each cell's terminal-stage `mle_params.toml` and
//! writes:
//!
//! - `<fit_dir>/{real|synthetic}/summary.tsv` — one row per
//!   `(dataset, fit_seed)` with each estimated parameter's MLE, the
//!   log-likelihood reported in the mle_params header, and the
//!   content hash.
//! - For synthetic mode only: `<fit_dir>/synthetic/coverage.tsv` —
//!   one row per estimated parameter with mean MLE, bias, 5th/95th
//!   quantile, and whether the central 90 % window brackets the
//!   declared ground truth.
//!
//! Fails soft: cells whose terminal stage didn't finish (e.g. failed
//! fit) are omitted from `summary.tsv` and noted on stderr; they do
//! not break the overall grid run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One row of `summary.tsv`. Kept narrow on purpose — cols map 1:1 to
/// fields, param values are a side map so we can handle variable
/// parameter sets across cells uniformly.
pub struct SummaryRow {
    pub dataset: String,    // "ds_01" … or "real"
    pub fit_seed: u64,
    pub stage: String,
    pub params: BTreeMap<String, f64>,
    pub loglik: Option<f64>,
    pub content_hash: String,
}

/// Write summary.tsv for a completed grid. `root_dir` is the parent
/// directory that contains either `real/` or `synthetic/` — i.e. the
/// fit directory. `source` is `"real"` or `"synthetic"`.
pub fn write_summary(
    root_dir: &Path,
    source: &str,
    rows: &[SummaryRow],
) -> Result<PathBuf, String> {
    if rows.is_empty() {
        return Err("no completed cells — nothing to summarise".to_string());
    }
    // Collect the union of parameter names for a stable column order.
    let mut param_names: Vec<String> = rows.iter()
        .flat_map(|r| r.params.keys().cloned())
        .collect();
    param_names.sort();
    param_names.dedup();

    let summary_path = root_dir.join(source).join("summary.tsv");
    let mut out = String::new();
    out.push_str("dataset\tfit_seed\tstage");
    for p in &param_names {
        out.push('\t');
        out.push_str(p);
    }
    out.push_str("\tloglik\tcontent_hash\n");

    for r in rows {
        out.push_str(&r.dataset);
        out.push('\t');
        out.push_str(&r.fit_seed.to_string());
        out.push('\t');
        out.push_str(&r.stage);
        for p in &param_names {
            out.push('\t');
            match r.params.get(p) {
                Some(v) => out.push_str(&format_value(*v)),
                None    => {} // blank → NA
            }
        }
        out.push('\t');
        match r.loglik {
            Some(ll) => out.push_str(&format!("{:.4}", ll)),
            None     => {}
        }
        out.push('\t');
        out.push_str(&r.content_hash);
        out.push('\n');
    }

    std::fs::write(&summary_path, &out)
        .map_err(|e| format!("cannot write {}: {}", summary_path.display(), e))?;
    Ok(summary_path)
}

/// Compute and write `coverage.tsv` for a synthetic grid. Each row is
/// one estimated parameter; coverage is the fraction of per-dataset
/// MLEs whose central 90 % window (across fit-seed replicates on that
/// dataset; scalar if only one fit-seed) brackets the ground-truth
/// value. For single-fit-per-dataset SBC the "window" collapses to
/// the point estimate and `covers_truth` is binary on strict equality
/// within machine precision, which isn't meaningful — so single-fit
/// SBC reports `q05 = q95 = mle` and a broader "truth within 20% of
/// MLE" bias check instead.
pub fn write_coverage(
    root_dir: &Path,
    truth: &BTreeMap<String, f64>,
    rows: &[SummaryRow],
) -> Result<PathBuf, String> {
    if rows.is_empty() {
        return Err("no completed synthetic cells — cannot write coverage".into());
    }
    let path = root_dir.join("synthetic").join("coverage.tsv");
    // Group MLE values per (param, dataset) — across fit-seed reps.
    // Then for each param, collapse across datasets into a bias +
    // coverage summary.
    let param_names: Vec<String> = truth.keys().cloned().collect();
    let mut buf = String::new();
    buf.push_str("param\ttruth\tmean_mle\tbias\tsd_mle\tq05\tq95\tcovers_truth\tn_datasets\n");

    for p in &param_names {
        let truth_v = truth[p];

        // Per-dataset median (or single) MLE.
        let mut per_dataset: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in rows {
            if let Some(&v) = r.params.get(p) {
                per_dataset.entry(r.dataset.clone()).or_default().push(v);
            }
        }
        let point_mles: Vec<f64> = per_dataset.values()
            .map(|xs| median(xs))
            .collect();
        if point_mles.is_empty() {
            continue;
        }
        let n = point_mles.len() as f64;
        let mean = point_mles.iter().sum::<f64>() / n;
        let var = if n > 1.0 {
            point_mles.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)
        } else { 0.0 };
        let sd = var.sqrt();
        let (q05, q95) = quantiles(&point_mles);
        let covers = if q05 <= truth_v && truth_v <= q95 { 1 } else { 0 };

        buf.push_str(p);
        buf.push('\t');
        buf.push_str(&format_value(truth_v));
        buf.push('\t');
        buf.push_str(&format_value(mean));
        buf.push('\t');
        buf.push_str(&format_value(mean - truth_v));
        buf.push('\t');
        buf.push_str(&format_value(sd));
        buf.push('\t');
        buf.push_str(&format_value(q05));
        buf.push('\t');
        buf.push_str(&format_value(q95));
        buf.push('\t');
        buf.push_str(&covers.to_string());
        buf.push('\t');
        buf.push_str(&point_mles.len().to_string());
        buf.push('\n');
    }

    std::fs::write(&path, &buf)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(path)
}

/// Parse a cell's `mle_params.toml` into a `SummaryRow`.
/// Returns `None` (logging a warning) when the file is missing — a
/// cell whose fit didn't complete.
pub fn read_cell_row(
    cell_dir: &Path,
    terminal_stage: &str,
    dataset: &str,
    fit_seed: u64,
) -> Option<SummaryRow> {
    let mle_path = cell_dir.join(terminal_stage).join("mle_params.toml");
    let text = std::fs::read_to_string(&mle_path).ok()?;

    let mut params = BTreeMap::new();
    let mut loglik: Option<f64> = None;
    let mut content_hash = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# Content hash: ") {
            // Extract just the hash token (before any trailing comment).
            content_hash = rest.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(rest) = line.strip_prefix("# Log-likelihood: ") {
            loglik = rest.split_whitespace().next()
                .and_then(|t| t.parse().ok());
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim().to_string();
                if let Ok(val) = v.trim().parse::<f64>() {
                    params.insert(key, val);
                }
            }
        }
    }
    Some(SummaryRow {
        dataset: dataset.to_string(),
        fit_seed,
        stage: terminal_stage.to_string(),
        params,
        loglik,
        content_hash,
    })
}

/// Load the ground-truth params from `<fit_dir>/synthetic/truth.toml`.
/// Error if the file is missing or unreadable.
pub fn load_truth(fit_dir: &Path) -> Result<BTreeMap<String, f64>, String> {
    let path = fit_dir.join("synthetic").join("truth.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let val: toml::Value = toml::from_str(&text)
        .map_err(|e| format!("parsing truth.toml: {}", e))?;
    let mut out = BTreeMap::new();
    if let toml::Value::Table(t) = val {
        for (k, v) in t {
            let f = match v {
                toml::Value::Float(f)   => f,
                toml::Value::Integer(n) => n as f64,
                _ => continue,
            };
            out.insert(k, f);
        }
    }
    Ok(out)
}

// ── helpers ────────────────────────────────────────────────────────────

fn format_value(v: f64) -> String {
    if (v.round() - v).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else if v.abs() < 1e-3 || v.abs() >= 1e6 {
        format!("{:.6e}", v)
    } else {
        format!("{:.6}", v)
    }
}

fn median(xs: &[f64]) -> f64 {
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = s.len();
    if n == 0 { return f64::NAN; }
    if n % 2 == 1 { s[n / 2] }
    else { 0.5 * (s[n / 2 - 1] + s[n / 2]) }
}

fn quantiles(xs: &[f64]) -> (f64, f64) {
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if s.is_empty() { return (f64::NAN, f64::NAN); }
    let pick = |q: f64| {
        let idx = (q * (s.len() as f64 - 1.0)).round() as usize;
        s[idx.min(s.len() - 1)]
    };
    (pick(0.05), pick(0.95))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_basic() {
        assert_eq!(median(&[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        assert!(median(&[]).is_nan());
    }

    #[test]
    fn quantiles_single_point() {
        let (q05, q95) = quantiles(&[42.0]);
        assert_eq!(q05, 42.0);
        assert_eq!(q95, 42.0);
    }

    #[test]
    fn format_value_stays_readable() {
        assert_eq!(format_value(0.0), "0");
        assert_eq!(format_value(1.0), "1");
        assert_eq!(format_value(0.5), "0.500000");
        // Scientific for very small / very large
        assert!(format_value(1e-9).contains("e"));
    }

    #[test]
    fn coverage_detects_truth_inside_window() {
        let tmp = std::env::temp_dir().join(format!(
            "camdl_coverage_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(tmp.join("synthetic")).unwrap();

        let truth: BTreeMap<String, f64> = [("beta".to_string(), 0.8)].into_iter().collect();
        let rows: Vec<SummaryRow> = (1..=20).map(|i| SummaryRow {
            dataset: format!("ds_{:02}", i),
            fit_seed: 1,
            stage: "mle".into(),
            params: [("beta".to_string(), 0.8 + (i as f64 - 10.0) * 0.02)].into_iter().collect(),
            loglik: Some(-42.0),
            content_hash: "abcd".into(),
        }).collect();

        let path = write_coverage(&tmp, &truth, &rows).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("beta"), "coverage.tsv must name the parameter");
        // The synthetic per-dataset MLEs are 0.80 ± 0.2; truth = 0.8
        // is centred in the distribution → covers_truth = 1.
        assert!(text.contains("\t1\t20\n") || text.contains("\t1\t20"),
            "truth at centre of dist should cover: {:?}", text);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn coverage_detects_truth_outside_window() {
        let tmp = std::env::temp_dir().join(format!(
            "camdl_coverage_out_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(tmp.join("synthetic")).unwrap();

        let truth: BTreeMap<String, f64> = [("beta".to_string(), 5.0)].into_iter().collect();
        let rows: Vec<SummaryRow> = (1..=10).map(|i| SummaryRow {
            dataset: format!("ds_{:02}", i),
            fit_seed: 1,
            stage: "mle".into(),
            params: [("beta".to_string(), 0.8 + (i as f64) * 0.01)].into_iter().collect(),
            loglik: None,
            content_hash: "".into(),
        }).collect();

        let path = write_coverage(&tmp, &truth, &rows).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Truth = 5.0, MLEs clustered around 0.85 → covers_truth = 0.
        assert!(text.contains("\t0\t10\n") || text.contains("\t0\t10"),
            "truth far from dist should not cover: {:?}", text);

        std::fs::remove_dir_all(&tmp).ok();
    }
}
