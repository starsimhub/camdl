//! `camdl data split` — split a TSV by time threshold or fraction.

use std::io::Write;

pub fn cmd_data_split(a: &crate::args::DataSplitArgs) {
    if a.at_time.is_none() && a.fraction.is_none() {
        eprintln!("error: specify --at-time T or --fraction F (e.g., --fraction 0.7)");
        std::process::exit(1);
    }

    let stem = a.file.to_str().unwrap_or_default().trim_end_matches(".tsv");
    let train_path = a.train.clone()
        .unwrap_or_else(|| std::path::PathBuf::from(format!("{}_train.tsv", stem)));
    let holdout_path = a.holdout.clone()
        .unwrap_or_else(|| std::path::PathBuf::from(format!("{}_holdout.tsv", stem)));

    let content = std::fs::read_to_string(&a.file)
        .unwrap_or_else(|e| { eprintln!("cannot read {}: {}", a.file.display(), e); std::process::exit(1); });
    let mut lines = content.lines();
    let header = lines.next().unwrap_or_else(|| {
        eprintln!("error: empty file"); std::process::exit(1);
    });

    // Find time column index
    let cols: Vec<&str> = header.split('\t').collect();
    let time_idx = if let Some(ref name) = a.time_col {
        cols.iter().position(|&c| c == name).unwrap_or_else(|| {
            eprintln!("error: column '{}' not found. Available: {}", name, cols.join(", "));
            std::process::exit(1);
        })
    } else {
        // Auto-detect: look for "time" or "t"
        cols.iter().position(|&c| c == "time" || c == "t")
            .unwrap_or_else(|| {
                eprintln!("error: no 'time' or 't' column found. Use --time-col to specify.");
                eprintln!("  Available columns: {}", cols.join(", "));
                std::process::exit(1);
            })
    };

    // Collect all rows with their time values
    let rows: Vec<(&str, f64)> = lines
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let time: f64 = line.split('\t').nth(time_idx)
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or_else(|| {
                    eprintln!("error: cannot parse time from column {} of line: {}", time_idx, line);
                    std::process::exit(1);
                });
            (line, time)
        })
        .collect();

    // Determine the split threshold
    let threshold = if let Some(t) = a.at_time {
        t
    } else {
        let f = a.fraction.unwrap();
        if f <= 0.0 || f >= 1.0 {
            eprintln!("error: --fraction must be between 0 and 1 (e.g., 0.7)");
            std::process::exit(1);
        }
        let n_train = (rows.len() as f64 * f).round() as usize;
        if n_train == 0 || n_train >= rows.len() {
            eprintln!("error: --fraction {} gives {} train rows out of {} total",
                f, n_train, rows.len());
            std::process::exit(1);
        }
        // Threshold = time of the last training row
        rows[n_train - 1].1
    };

    let mut train_rows = Vec::new();
    let mut holdout_rows = Vec::new();
    for &(line, time) in &rows {
        if time <= threshold {
            train_rows.push(line);
        } else {
            holdout_rows.push(line);
        }
    }

    {
        let mut f = std::fs::File::create(&train_path)
            .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", train_path.display(), e); std::process::exit(1); });
        writeln!(f, "{}", header).unwrap();
        for row in &train_rows { writeln!(f, "{}", row).unwrap(); }
    }

    {
        let mut f = std::fs::File::create(&holdout_path)
            .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", holdout_path.display(), e); std::process::exit(1); });
        writeln!(f, "{}", header).unwrap();
        for row in &holdout_rows { writeln!(f, "{}", row).unwrap(); }
    }

    let fmt_range = |rows: &[&str]| -> String {
        let first = rows.first().and_then(|r| r.split('\t').nth(time_idx)?.parse::<f64>().ok());
        let last = rows.last().and_then(|r| r.split('\t').nth(time_idx)?.parse::<f64>().ok());
        match (first, last) {
            (Some(lo), Some(hi)) => format!("[{}, {}]", lo, hi),
            _ => "—".into(),
        }
    };

    eprintln!("Split at t = {} (column '{}')", threshold, cols[time_idx]);
    eprintln!("  Train:   {} observations, t ∈ {}", train_rows.len(), fmt_range(&train_rows));
    eprintln!("  Holdout: {} observations, t ∈ {}", holdout_rows.len(), fmt_range(&holdout_rows));
    eprintln!("  Written: {}, {}", train_path.display(), holdout_path.display());
}
