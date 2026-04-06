//! `camdl fit` — structured inference workflow.
//!
//! Usage:
//!   camdl fit scout    fit.toml [--seed N] [--force]
//!   camdl fit refine   fit.toml --starts-from scout/ [--seed N] [--force]
//!   camdl fit validate fit.toml --starts-from refine/ [--seed N] [--force]
//!   camdl fit pmmh     fit.toml [--starts-from validate/] [--seed N] [--force] [--check-variance]
//!   camdl fit pgas     fit.toml [--starts-from validate/] [--seed N] [--force]
//!   camdl fit status   fit.toml

pub mod config;
pub mod state;
pub mod provenance;
pub mod runner;
pub mod scout;
pub mod refine;
pub mod validate;
pub mod status;
pub mod pmmh;
pub mod pgas;

use config::FitToml;

pub fn cmd_fit_scout(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, false);

    // Validate partition
    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    fit.validate_bounds(&model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    scout::run_scout(&fit, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_refine(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, true);
    let starts_from = parse_starts_from(args);

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    refine::run_refine(&fit, &starts_from, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_validate(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, true);
    let starts_from = parse_starts_from(args);

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    validate::run_validate(&fit, &starts_from, seed, force).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_pmmh(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, false);
    let starts_from = parse_optional_starts_from(args);
    let check_variance = args.iter().any(|a| a == "--check-variance");

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    fit.validate_bounds(&model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    pmmh::run_pmmh_cli(&fit, starts_from.as_deref(), seed, force, check_variance).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_pgas(args: &[String]) {
    let (fit, seed, force) = parse_fit_args(args, false);
    let starts_from = parse_optional_starts_from(args);
    let no_nuts = args.iter().any(|a| a == "--no-nuts");

    let (model, _) = load_model_for_validation(&fit);
    let model_params: Vec<String> = model.parameters.iter().map(|p| p.name.clone()).collect();
    fit.validate_partition(&model_params).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
    fit.validate_bounds(&model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    pgas::run_pgas_cli(&fit, starts_from.as_deref(), seed, force, !no_nuts).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

pub fn cmd_fit_status(args: &[String]) {
    let (fit, _, _) = parse_fit_args(args, false);
    status::run_status(&fit).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });
}

fn parse_fit_args(args: &[String], _needs_starts_from: bool) -> (FitToml, u64, bool) {
    let mut fit_path: Option<String> = None;
    let mut seed = 1_u64;
    let mut force = false;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--seed" => { i += 1; seed = args[i].parse().expect("--seed needs integer"); }
            "--force" => { force = true; }
            "--starts-from" => { i += 1; } // consumed by parse_starts_from / parse_optional_starts_from
            "--check-variance" => {} // consumed by cmd_fit_pmmh
            "--no-nuts" => {} // consumed by cmd_fit_pgas
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                std::process::exit(1);
            }
            path => { fit_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let fit_path = fit_path.unwrap_or_else(|| {
        eprintln!("usage: camdl fit <scout|refine|validate|pmmh|pgas|status> FIT.toml");
        std::process::exit(1);
    });

    let fit = FitToml::load(&fit_path).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    });

    // Seed priority: CLI --seed > fit.toml seed > random from entropy
    let seed = if args.iter().any(|a| a == "--seed") {
        seed
    } else if let Some(s) = fit.fit.seed {
        s
    } else {
        use std::time::SystemTime;
        let dur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
        dur.as_nanos() as u64 % 1_000_000
    };

    (fit, seed, force)
}

fn parse_optional_starts_from(args: &[String]) -> Option<String> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--starts-from" {
            return Some(args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("--starts-from requires a directory path");
                std::process::exit(1);
            }));
        }
    }
    None
}

fn parse_starts_from(args: &[String]) -> String {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--starts-from" {
            return args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("--starts-from requires a directory path");
                std::process::exit(1);
            });
        }
    }
    eprintln!("error: --starts-from required for refine/validate");
    eprintln!("  usage: camdl fit refine fit.toml --starts-from scout/");
    std::process::exit(1);
}

fn load_model_for_validation(fit: &FitToml) -> (ir::Model, String) {
    crate::util::load_model(&fit.fit.model).unwrap_or_else(|e| {
        eprintln!("error: {}", e);
        std::process::exit(1);
    })
}
