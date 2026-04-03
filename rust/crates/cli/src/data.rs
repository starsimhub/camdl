//! `camdl data split` — split a TSV by time threshold or fraction.

use std::io::Write;

pub fn cmd_data_split(args: &[String]) {
    let mut input_path: Option<String> = None;
    let mut at_time: Option<f64> = None;
    let mut fraction: Option<f64> = None;
    let mut train_path: Option<String> = None;
    let mut holdout_path: Option<String> = None;
    let mut time_col: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--at-time" => {
                i += 1;
                at_time = Some(args[i].parse().unwrap_or_else(|_| {
                    eprintln!("error: --at-time needs a number"); std::process::exit(1);
                }));
            }
            "--fraction" => {
                i += 1;
                fraction = Some(args[i].parse().unwrap_or_else(|_| {
                    eprintln!("error: --fraction needs a number between 0 and 1"); std::process::exit(1);
                }));
            }
            "--time-col" => { i += 1; time_col = Some(args[i].clone()); }
            "--train" => { i += 1; train_path = Some(args[i].clone()); }
            "--holdout" => { i += 1; holdout_path = Some(args[i].clone()); }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s); std::process::exit(1);
            }
            path => { input_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let input_path = input_path.unwrap_or_else(|| {
        eprintln!("usage: camdl data split FILE <--at-time T | --fraction F> [--time-col COL] [--train OUT] [--holdout OUT]");
        std::process::exit(1);
    });
    if at_time.is_none() && fraction.is_none() {
        eprintln!("error: specify --at-time T or --fraction F (e.g., --fraction 0.7)");
        std::process::exit(1);
    }

    // Auto-name outputs
    let stem = input_path.trim_end_matches(".tsv");
    let train_path = train_path.unwrap_or_else(|| format!("{}_train.tsv", stem));
    let holdout_path = holdout_path.unwrap_or_else(|| format!("{}_holdout.tsv", stem));

    // Read input
    let content = std::fs::read_to_string(&input_path)
        .unwrap_or_else(|e| { eprintln!("cannot read {}: {}", input_path, e); std::process::exit(1); });
    let mut lines = content.lines();
    let header = lines.next().unwrap_or_else(|| {
        eprintln!("error: empty file"); std::process::exit(1);
    });

    // Find time column index
    let cols: Vec<&str> = header.split('\t').collect();
    let time_idx = if let Some(ref name) = time_col {
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
    let threshold = if let Some(t) = at_time {
        t
    } else {
        let f = fraction.unwrap();
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

    // Write train
    {
        let mut f = std::fs::File::create(&train_path)
            .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", train_path, e); std::process::exit(1); });
        writeln!(f, "{}", header).unwrap();
        for row in &train_rows { writeln!(f, "{}", row).unwrap(); }
    }

    // Write holdout
    {
        let mut f = std::fs::File::create(&holdout_path)
            .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", holdout_path, e); std::process::exit(1); });
        writeln!(f, "{}", header).unwrap();
        for row in &holdout_rows { writeln!(f, "{}", row).unwrap(); }
    }

    // Report
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
    eprintln!("  Written: {}, {}", train_path, holdout_path);
}
