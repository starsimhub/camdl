//! `camdl data split` — split a TSV at a time threshold.

use std::io::Write;

pub fn cmd_data_split(args: &[String]) {
    let mut input_path: Option<String> = None;
    let mut at_time: Option<f64> = None;
    let mut train_path: Option<String> = None;
    let mut holdout_path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--at-time" => {
                i += 1;
                at_time = Some(args[i].parse().unwrap_or_else(|_| {
                    eprintln!("error: --at-time needs a number"); std::process::exit(1);
                }));
            }
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
        eprintln!("usage: camdl data split FILE --at-time T [--train OUT] [--holdout OUT]");
        std::process::exit(1);
    });
    let threshold = at_time.unwrap_or_else(|| {
        eprintln!("error: --at-time required"); std::process::exit(1);
    });

    // Auto-name outputs if not specified
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

    let mut train_rows = Vec::new();
    let mut holdout_rows = Vec::new();

    for line in lines {
        if line.trim().is_empty() { continue; }
        let time: f64 = line.split('\t').next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or_else(|| {
                eprintln!("error: cannot parse time from line: {}", line);
                std::process::exit(1);
            });
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

    let train_t_range = train_rows.first().and_then(|r| r.split('\t').next()?.parse::<f64>().ok())
        .map(|lo| {
            let hi = train_rows.last().and_then(|r| r.split('\t').next()?.parse::<f64>().ok()).unwrap_or(lo);
            format!("[{}, {}]", lo, hi)
        }).unwrap_or_else(|| "—".into());
    let holdout_t_range = holdout_rows.first().and_then(|r| r.split('\t').next()?.parse::<f64>().ok())
        .map(|lo| {
            let hi = holdout_rows.last().and_then(|r| r.split('\t').next()?.parse::<f64>().ok()).unwrap_or(lo);
            format!("[{}, {}]", lo, hi)
        }).unwrap_or_else(|| "—".into());

    eprintln!("Split at t = {}", threshold);
    eprintln!("  Train:   {} observations, t ∈ {}", train_rows.len(), train_t_range);
    eprintln!("  Holdout: {} observations, t ∈ {}", holdout_rows.len(), holdout_t_range);
    eprintln!("  Written: {}, {}", train_path, holdout_path);
}
