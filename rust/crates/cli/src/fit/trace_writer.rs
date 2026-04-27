//! Shared streaming TSV trace writer for MCMC methods (PGAS, PMMH).
//!
//! Handles header construction, append mode for `--resume`, periodic
//! flushing, and thread-safe writing via Mutex.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::Mutex;

/// Streaming TSV trace writer for MCMC traces.
///
/// Shared columns: `{index_col}`, `log_likelihood`, `log_posterior`.
/// Method-specific columns (e.g., `trajectory_renewal`, `accepted`)
/// are passed as `extra_columns` at construction and `extra_values`
/// at each write.
pub struct TraceWriter {
    file: Mutex<BufWriter<File>>,
    flush_interval: usize,
    row_count: std::sync::atomic::AtomicUsize,
}

impl TraceWriter {
    /// Create a new trace writer.
    ///
    /// - `append = false`: creates file and writes header.
    /// - `append = true`: opens in append mode (header already exists).
    pub fn new(
        path: &str,
        index_col: &str,
        extra_columns: &[&str],
        param_names: &[String],
        append: bool,
    ) -> Self {
        let file = if append && std::path::Path::new(path).exists() {
            BufWriter::new(
                OpenOptions::new().append(true).open(path)
                    .unwrap_or_else(|e| panic!("cannot open {} for append: {}", path, e))
            )
        } else {
            let mut f = BufWriter::new(
                File::create(path)
                    .unwrap_or_else(|e| panic!("cannot create {}: {}", path, e))
            );
            // Write header
            write!(f, "{}\tlog_likelihood\tlog_posterior", index_col).unwrap();
            for col in extra_columns {
                write!(f, "\t{}", col).unwrap();
            }
            for name in param_names {
                write!(f, "\t{}", name).unwrap();
            }
            writeln!(f).unwrap();
            f
        };

        TraceWriter {
            file: Mutex::new(file),
            flush_interval: 50,
            row_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Write one trace row.
    ///
    /// `extra_values` must match the `extra_columns` passed to `new()`.
    /// Values are pre-formatted by the caller (e.g., `"0.9571"` or `"1"`).
    pub fn write_row(
        &self,
        index: usize,
        log_likelihood: f64,
        log_posterior: f64,
        extra_values: &[&str],
        param_values: &[f64],
    ) {
        if let Ok(mut f) = self.file.lock() {
            write!(f, "{}\t{:.4}\t{:.4}", index, log_likelihood, log_posterior).unwrap();
            for val in extra_values {
                write!(f, "\t{}", val).unwrap();
            }
            for &v in param_values {
                write!(f, "\t{:.6}", v).unwrap();
            }
            writeln!(f).unwrap();

            let n = self.row_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if (n + 1).is_multiple_of(self.flush_interval) {
                f.flush().ok();
            }
        }
    }

}
