//! Summary TSV read/write.
//!
//! The standard summary format is a wide table with one row per named
//! statistic:
//!
//! ```text
//! stat_name         mean        sd          q025        q500        q975        n
//! total_cases       538418.2    11273.4     517903      538105      560772      200
//! ```
//!
//! Shared between reference-side (after transformation of whatever the
//! external tool produces) and camdl-side.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct SummaryRow {
    pub mean: f64,
    pub sd: f64,
    pub q025: f64,
    pub q500: f64,
    pub q975: f64,
    pub n: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub rows: BTreeMap<String, SummaryRow>,
}

impl Summary {
    pub fn from_samples(name: &str, samples: &[f64]) -> (String, SummaryRow) {
        let n = samples.len();
        assert!(n > 0, "Summary::from_samples: empty samples for '{}'", name);
        let mut sorted: Vec<f64> = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mean = samples.iter().sum::<f64>() / n as f64;
        let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        let sd = var.sqrt();
        let row = SummaryRow {
            mean, sd, n,
            q025: quantile_sorted(&sorted, 0.025),
            q500: quantile_sorted(&sorted, 0.500),
            q975: quantile_sorted(&sorted, 0.975),
        };
        (name.to_string(), row)
    }

    pub fn read_tsv(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read {}: {}", path.display(), e))?;
        let mut lines = content.lines();
        let header = lines.next().ok_or_else(|| anyhow::anyhow!("empty summary TSV: {}", path.display()))?;
        // Require exact header to catch summariser-format drift.
        let expected = "stat_name\tmean\tsd\tq025\tq500\tq975\tn";
        if header.trim() != expected {
            return Err(anyhow::anyhow!(
                "summary TSV {} has unexpected header:\n  expected: {}\n  got:      {}",
                path.display(), expected, header.trim()
            ));
        }
        let mut rows = BTreeMap::new();
        for (i, line) in lines.enumerate() {
            if line.trim().is_empty() { continue; }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != 7 {
                return Err(anyhow::anyhow!(
                    "summary TSV {} line {}: expected 7 fields, got {}",
                    path.display(), i + 2, fields.len()
                ));
            }
            let parse = |s: &str, col: &str| -> anyhow::Result<f64> {
                s.parse::<f64>().map_err(|_| anyhow::anyhow!(
                    "summary TSV {} line {}: cannot parse {} = {:?}",
                    path.display(), i + 2, col, s
                ))
            };
            let row = SummaryRow {
                mean: parse(fields[1], "mean")?,
                sd:   parse(fields[2], "sd")?,
                q025: parse(fields[3], "q025")?,
                q500: parse(fields[4], "q500")?,
                q975: parse(fields[5], "q975")?,
                n:    fields[6].parse::<usize>().map_err(|_| anyhow::anyhow!(
                    "summary TSV {} line {}: cannot parse n = {:?}",
                    path.display(), i + 2, fields[6]
                ))?,
            };
            rows.insert(fields[0].to_string(), row);
        }
        Ok(Summary { rows })
    }

    pub fn write_tsv(&self, path: &Path) -> anyhow::Result<()> {
        let mut s = String::new();
        s.push_str("stat_name\tmean\tsd\tq025\tq500\tq975\tn\n");
        for (name, r) in &self.rows {
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                name, r.mean, r.sd, r.q025, r.q500, r.q975, r.n,
            ));
        }
        std::fs::write(path, s)
            .map_err(|e| anyhow::anyhow!("write {}: {}", path.display(), e))
    }
}

/// Linear-interpolation quantile on a pre-sorted slice. Matches
/// numpy.quantile(interpolation="linear") / R's type=7.
fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    assert!((0.0..=1.0).contains(&q));
    let n = sorted.len();
    if n == 0 { return f64::NAN; }
    if n == 1 { return sorted[0]; }
    let h = q * (n - 1) as f64;
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = h - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantile_matches_numpy() {
        let s = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((quantile_sorted(&s, 0.0) - 1.0).abs() < 1e-12);
        assert!((quantile_sorted(&s, 0.5) - 3.0).abs() < 1e-12);
        assert!((quantile_sorted(&s, 1.0) - 5.0).abs() < 1e-12);
        // numpy.quantile([1,2,3,4,5], 0.25) == 2.0
        assert!((quantile_sorted(&s, 0.25) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn summary_from_samples() {
        let samples: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        let (name, row) = Summary::from_samples("x", &samples);
        assert_eq!(name, "x");
        assert_eq!(row.n, 100);
        assert!((row.mean - 50.5).abs() < 1e-10);
        // q500 ≈ 50.5 by linear interpolation
        assert!((row.q500 - 50.5).abs() < 1.0);
    }

    #[test]
    fn roundtrip_tsv() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut s = Summary::default();
        s.rows.insert("a".to_string(), SummaryRow {
            mean: 1.5, sd: 0.25, q025: 1.0, q500: 1.5, q975: 2.0, n: 100,
        });
        s.write_tsv(tmp.path()).unwrap();
        let read = Summary::read_tsv(tmp.path()).unwrap();
        assert_eq!(read.rows["a"], s.rows["a"]);
    }
}
