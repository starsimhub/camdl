mod util;
mod hashing;
mod sampling;
#[allow(dead_code)]
mod experiment; // used by --batch delegation
mod serve;
mod eval;
#[allow(dead_code)]
mod pfilter; // used internally by fit runner for data loading
mod data;
mod fit;
pub mod version;

// Modules kept for internal use but with no direct CLI entry points:
#[allow(dead_code)] mod analyze;
#[allow(dead_code)] mod summarize;
#[allow(dead_code)] mod voi;
#[allow(dead_code)] mod if2;
#[allow(dead_code)] mod profile;

use sim::{write_diagnostics_tsv, warn_zero_firings};
use std::collections::HashMap;

fn usage() -> ! {
    eprintln!("camdl simulate MODEL.ir.json [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --params   FILE.toml     load parameter values from TOML (repeatable, later overrides earlier)");
    eprintln!("  --backend  gillespie|tau_leap|chain_binomial  (default: gillespie)");
    eprintln!("  --dt       DT   step size for tau_leap / chain_binomial");
    eprintln!("  --seed     N    RNG seed (default: 1)");
    eprintln!("  --scenario NAME          select a named scenario from the model file");
    eprintln!("  --enable   NAME          enable a named intervention (ad-hoc; mutually exclusive with --scenario)");
    eprintln!("  --disable  NAME          disable a named intervention (ad-hoc; mutually exclusive with --scenario)");
    eprintln!("  --param    NAME=VALUE    override a parameter value");
    eprintln!("  --param-vec PREFIX=FILE  override indexed params from a keyed TSV (name<TAB>value)");
    eprintln!("  --table    NAME=FILE     supply a runtime external() table from CSV/TSV/JSON");
    eprintln!("  --obs      FILE          generate synthetic observations (wide-format TSV)");
    eprintln!("  --obs-dir  DIR           generate one TSV per observation stream in DIR");
    eprintln!("  --obs-only FILE|DIR      like --obs/--obs-dir but suppress trajectory output");
    eprintln!("  --replicates N           run N independent simulations (adds replicate column)");
    eprintln!("  --draws    FILE.tsv      simulate at each row of a draws file (posterior/prior predictive)");
    std::process::exit(1);
}

fn main() {
    let all_args: Vec<String> = std::env::args().skip(1).collect();
    if all_args.is_empty() { usage(); }

    // --version / -V anywhere in args
    if all_args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", version::VERSION);
        return;
    }

    // --verbosity LEVEL: set log level (default: warn)
    // Levels: error, warn, info, debug, trace
    let verbosity = all_args.iter()
        .position(|a| a == "--verbosity")
        .and_then(|i| all_args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("warn");
    let log_level = match verbosity {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "info" => log::LevelFilter::Info,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        other => {
            eprintln!("invalid verbosity level: '{}'. Use: error, warn, info, debug, trace", other);
            std::process::exit(1);
        }
    };
    env_logger::builder().filter_level(log_level).init();

    // Strip --verbosity LEVEL from args before dispatch
    let all_args: Vec<String> = {
        let mut filtered = Vec::new();
        let mut skip_next = false;
        for arg in &all_args {
            if skip_next { skip_next = false; continue; }
            if arg == "--verbosity" { skip_next = true; continue; }
            filtered.push(arg.clone());
        }
        filtered
    };

    // Dispatch on first argument
    match all_args[0].as_str() {
        // ── Compiler delegation (transparent camdlc invocation) ──
        "compile" => {
            let args: Vec<&str> = all_args[1..].iter().map(|s| s.as_str()).collect();
            util::delegate_to_camdlc(&args).unwrap_or_else(|e| {
                eprintln!("error: {}", e); std::process::exit(1);
            });
        }
        "check" => {
            let mut args = vec!["check"];
            args.extend(all_args[1..].iter().map(|s| s.as_str()));
            util::delegate_to_camdlc(&args).unwrap_or_else(|e| {
                eprintln!("error: {}", e); std::process::exit(1);
            });
        }
        "inspect" => {
            let mut args = vec!["inspect"];
            args.extend(all_args[1..].iter().map(|s| s.as_str()));
            util::delegate_to_camdlc(&args).unwrap_or_else(|e| {
                eprintln!("error: {}", e); std::process::exit(1);
            });
        }
        // ── Simulation ──
        "simulate" | "sim" => {
            let args = &all_args[1..];
            if args.is_empty() { usage(); }
            if args.iter().any(|a| a == "--batch") {
                let batch_args: Vec<String> = args.iter()
                    .filter(|a| *a != "--batch")
                    .cloned()
                    .collect();
                experiment::cmd_experiment_run(&batch_args);
            } else {
                run_simulate(args);
            }
        }
        // ── Inference ──
        "fit" => {
            match all_args.get(1).map(|s| s.as_str()) {
                Some("run")    => fit::cmd_fit_run_v2(&all_args[2..]),
                Some("status") => fit::cmd_fit_status(&all_args[2..]),
                Some("diff")   => fit::cmd_fit_diff(&all_args[2..]),
                Some("new")    => fit::cmd_fit_new(&all_args[2..]),
                _ => {
                    eprintln!("usage: camdl fit <run|status|diff|new> FIT.toml");
                    std::process::exit(1);
                }
            }
        }
        // ── Utilities ──
        "eval" => {
            eval::cmd_eval(&all_args[1..]);
        }
        "data" => {
            match all_args.get(1).map(|s| s.as_str()) {
                Some("split") => data::cmd_data_split(&all_args[2..]),
                _ => {
                    eprintln!("usage: camdl data split FILE --at-time T [--train OUT] [--holdout OUT]");
                    std::process::exit(1);
                }
            }
        }
        "serve" => {
            serve::cmd_serve(&all_args[1..]);
        }
        _ => {
            // Accept bare "camdl FILE ..." for simulation
            run_simulate(&all_args);
        }
    }
}

fn run_simulate(args: &[String]) {
    let mut ir_path:     Option<String> = None;
    let mut backend    = "gillespie".to_string();
    let mut dt         = 1.0_f64;
    let mut seed       = 1_u64;
    let mut overrides: HashMap<String, f64> = HashMap::new();
    let mut table_files: HashMap<String, String> = HashMap::new();
    let mut params_files: Vec<String> = Vec::new();
    let mut scenario_names: Vec<String> = Vec::new();
    let mut adhoc_enable: Vec<String> = Vec::new();
    let mut adhoc_disable: Vec<String> = Vec::new();
    let mut seeds_spec: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut obs_path: Option<String> = None;
    let mut obs_dir: Option<String> = None;
    let mut obs_only: Option<String> = None;
    let mut replicates: usize = 1;
    let mut draws_path: Option<String> = None;
    let mut n_draws_arg: Option<usize> = None;

    // Collect --param-vec PREFIX=FILE entries for deferred validation after model load
    let mut set_vec_entries: Vec<(String, String)> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backend"  => { i += 1; backend = args[i].clone(); }
            "--dt"       => { i += 1; dt      = args[i].parse().unwrap_or_else(|_| { eprintln!("error: --dt needs a number"); std::process::exit(1); }); }
            "--seed"     => { i += 1; seed    = args[i].parse().unwrap_or_else(|_| { eprintln!("error: --seed needs an integer"); std::process::exit(1); }); }
            "--params"   => { i += 1; params_files.push(args[i].clone()); }
            "--scenario" => { i += 1; scenario_names.extend(args[i].split(',').map(|s| s.trim().to_string())); }
            "--seeds"   => { i += 1; seeds_spec = Some(args[i].clone()); }
            "--enable"   => { i += 1; adhoc_enable.push(args[i].clone()); }
            "--disable"  => { i += 1; adhoc_disable.push(args[i].clone()); }
            "--param"     => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap_or_else(|| { eprintln!("error: --param needs NAME=VALUE"); std::process::exit(1); }).to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| { eprintln!("error: --param value must be a number"); std::process::exit(1); });
                overrides.insert(k, v);
            }
            "--param-vec" => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let prefix = parts.next().unwrap_or_else(|| { eprintln!("error: --param-vec needs PREFIX=FILE"); std::process::exit(1); }).to_string();
                let file   = parts.next().unwrap_or_else(|| { eprintln!("error: --param-vec needs PREFIX=FILE"); std::process::exit(1); }).to_string();
                set_vec_entries.push((prefix, file));
            }
            "--table"   => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap_or_else(|| { eprintln!("error: --table needs NAME=FILE"); std::process::exit(1); }).to_string();
                let v = parts.next().unwrap_or_else(|| { eprintln!("error: --table needs NAME=FILE"); std::process::exit(1); }).to_string();
                table_files.insert(k, v);
            }
            "--output" | "-o" => { i += 1; output_path = Some(args[i].clone()); }
            "--obs"      => { i += 1; obs_path = Some(args[i].clone()); }
            "--obs-dir"  => { i += 1; obs_dir = Some(args[i].clone()); }
            "--obs-only" => { i += 1; obs_only = Some(args[i].clone()); }
            "--replicates" => { i += 1; replicates = args[i].parse().unwrap_or_else(|_| { eprintln!("error: --replicates needs a positive integer"); std::process::exit(1); }); }
            "--draws" => { i += 1; draws_path = Some(args[i].clone()); }
            "-n" | "--n-draws" => { i += 1; n_draws_arg = Some(args[i].parse().unwrap_or_else(|_| { eprintln!("error: -n needs a positive integer"); std::process::exit(1); })); }
            s if s.starts_with("--") => { eprintln!("unknown flag: {}", s); usage(); }
            path => { ir_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let ir_path = ir_path.unwrap_or_else(|| { eprintln!("missing IR file argument"); usage(); });

    // --obs-only implies --obs or --obs-dir (infer from path: trailing / or existing dir → obs-dir)
    if let Some(ref path) = obs_only {
        if obs_path.is_some() || obs_dir.is_some() {
            eprintln!("error: --obs-only cannot be combined with --obs or --obs-dir");
            std::process::exit(1);
        }
        if path.ends_with('/') || std::path::Path::new(path).is_dir() {
            obs_dir = Some(path.clone());
        } else {
            obs_path = Some(path.clone());
        }
    }
    let suppress_trajectory = obs_only.is_some();

    if replicates < 1 {
        eprintln!("error: --replicates must be >= 1");
        std::process::exit(1);
    }

    let want_obs = obs_path.is_some() || obs_dir.is_some();

    // Parse --seeds spec
    let seeds: Vec<u64> = if let Some(ref spec) = seeds_spec {
        parse_seeds_spec(spec).unwrap_or_else(|e| {
            eprintln!("error: --seeds: {}", e);
            std::process::exit(1);
        })
    } else {
        vec![seed]
    };
    if seeds_spec.is_some() && replicates > 1 {
        eprintln!("error: --seeds and --replicates are mutually exclusive.\n  \
                   --seeds provides explicit seed values.\n  \
                   --replicates generates N deterministic seeds from --seed.");
        std::process::exit(1);
    }
    // If using --seeds, replicates tracks seed count
    let replicates = if seeds_spec.is_some() { seeds.len() } else { replicates };

    // Validate mutually exclusive σ flags
    if !scenario_names.is_empty() && (!adhoc_enable.is_empty() || !adhoc_disable.is_empty()) {
        eprintln!("error: --scenario and --enable/--disable are mutually exclusive.");
        eprintln!("  --scenario selects a named scenario from the model file.");
        eprintln!("  --enable/--disable compose an ad-hoc scenario.");
        eprintln!("  To combine both, define a composed scenario in the model file.");
        std::process::exit(1);
    }

    // If no scenarios specified, use a single None (baseline)
    let scenario_list: Vec<Option<String>> = if scenario_names.is_empty() {
        vec![None]
    } else {
        scenario_names.iter().map(|s| Some(s.clone())).collect()
    };

    let base_sim_run = util::SimRun {
        ir_path: ir_path.clone(),
        params_files,
        overrides,
        set_vec_entries,
        table_files,
        scenario_name: None, // set per-scenario in the loop
        adhoc_enable,
        adhoc_disable,
        backend,
        dt,
        seed, // overridden per-replicate below
    };

    // ── Pre-flight: validate obs model availability ─────────────────────────
    // We need the model to check observation blocks, but we don't want to
    // run simulation twice. Do a dry load to validate, then run in the loop.
    if want_obs {
        let (model_check, _) = util::load_model(&ir_path).unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });
        if model_check.observations.is_empty() {
            eprintln!("error: --obs/--obs-dir requested but model has no observations blocks");
            std::process::exit(1);
        }
        // Validate schedule compatibility for --obs (single file)
        if obs_path.is_some() && model_check.observations.len() > 1 {
            let schedules: Vec<_> = model_check.observations.iter()
                .map(|o| obs_schedule_times(&o.schedule, model_check.simulation.t_start, model_check.simulation.t_end))
                .collect();
            let all_same = schedules.windows(2).all(|w| w[0] == w[1]);
            if !all_same {
                let descs: Vec<String> = model_check.observations.iter()
                    .map(|o| format!("{}: {:?}", o.name, o.schedule))
                    .collect();
                eprintln!("error: observation streams have different schedules ({}).\n\
                           Use --obs-dir to produce one file per stream.",
                    descs.join(", "));
                std::process::exit(1);
            }
        }
    }

    // ── Prepare obs-dir output directory ────────────────────────────────────
    if let Some(ref dir) = obs_dir {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| {
            eprintln!("error: cannot create obs directory '{}': {}", dir, e);
            std::process::exit(1);
        });
    }

    use std::io::Write;

    // ── Trajectory output setup ─────────────────────────────────────────────
    let mut traj_out: Option<Box<dyn Write>> = if !suppress_trajectory {
        Some(match &output_path {
            Some(path) => {
                let f = std::fs::File::create(path)
                    .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", path, e); std::process::exit(1); });
                Box::new(std::io::BufWriter::new(f))
            }
            None => Box::new(std::io::BufWriter::new(std::io::stdout().lock())),
        })
    } else {
        None
    };
    let mut traj_header_written = false;

    // ── Load draws if --draws is specified ─────────────────────────────────
    let draws: Vec<HashMap<String, f64>> = if let Some(ref source) = draws_path {
        if source == "uniform" {
            let n = n_draws_arg.unwrap_or_else(|| {
                eprintln!("error: --draws uniform requires -n N");
                std::process::exit(1);
            });
            generate_uniform_draws(&ir_path, n, seed).unwrap_or_else(|e| {
                eprintln!("error: {}", e);
                std::process::exit(1);
            })
        } else {
            // File path
            load_draws_tsv(source).unwrap_or_else(|e| {
                eprintln!("error loading draws: {}", e);
                std::process::exit(1);
            })
        }
    } else {
        // No draws — single point (parameters come from --params / --param)
        vec![HashMap::new()]
    };
    let n_draws = draws.len();
    let n_scenarios = scenario_list.len();
    let total_runs = n_draws * replicates * n_scenarios;
    if total_runs > 1 {
        let parts: Vec<String> = [
            if n_draws > 1 { Some(format!("{} draws", n_draws)) } else { None },
            if n_scenarios > 1 { Some(format!("{} scenarios", n_scenarios)) } else { None },
            if replicates > 1 { Some(format!("{} replicates", replicates)) } else { None },
        ].iter().flatten().cloned().collect();
        eprintln!("{} = {} runs", parts.join(" × "), total_runs);
    }

    // ── Observation accumulators ────────────────────────────────────────────
    struct ObsRow { time: f64, replicate: usize, value: f64 }
    let mut obs_data: Vec<Vec<ObsRow>> = Vec::new(); // per-stream
    let mut obs_stream_names: Vec<String> = Vec::new();
    let mut obs_times_cache: Vec<Vec<f64>> = Vec::new();

    // ── Main loop: scenarios × draws × replicates ─────────────────────────
    let mut run_idx = 0usize;
    for scenario in &scenario_list {
    for (draw_idx, draw_overrides) in draws.iter().enumerate() {
        for rep in 0..replicates {
            let process_seed = if seeds_spec.is_some() {
                seeds[rep] // explicit seeds
            } else if total_runs == 1 {
                seed
            } else {
                seed ^ ((draw_idx as u64).wrapping_mul(0x9e3779b97f4a7c15))
                     ^ ((rep as u64).wrapping_mul(0x517cc1b727220a95))
            };
            let obs_seed = process_seed ^ 0xa5a5a5a5a5a5;

            // Merge draw overrides with CLI --param overrides
            let mut combined_overrides = base_sim_run.overrides.clone();
            combined_overrides.extend(draw_overrides.iter().map(|(k, v)| (k.clone(), *v)));

            let mut sim_run = util::SimRun { seed: process_seed, ..Default::default() };
            sim_run.ir_path = base_sim_run.ir_path.clone();
            sim_run.params_files = base_sim_run.params_files.clone();
            sim_run.overrides = combined_overrides;
            sim_run.set_vec_entries = base_sim_run.set_vec_entries.clone();
            sim_run.table_files = base_sim_run.table_files.clone();
            sim_run.scenario_name = scenario.clone();
            sim_run.adhoc_enable = base_sim_run.adhoc_enable.clone();
            sim_run.adhoc_disable = base_sim_run.adhoc_disable.clone();
            sim_run.backend = base_sim_run.backend.clone();
            sim_run.dt = base_sim_run.dt;

        let (traj, model) = util::run_simulation(&sim_run).unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });

        // Write diagnostics (first run only)
        if run_idx == 0 && !traj.transition_diagnostics.is_empty() {
            match write_diagnostics_tsv("diagnostics.tsv", &traj.transition_diagnostics) {
                Ok(zero_count) => {
                    if zero_count > 0 { warn_zero_firings(&traj.transition_diagnostics); }
                }
                Err(e) => eprintln!("warning: could not write diagnostics.tsv: {}", e),
            }
        }

        // ── Trajectory output ───────────────────────────────────────────────
        if let Some(ref mut out) = traj_out {
            let int_names: Vec<&str> = model.compartments.iter()
                .filter(|c| c.kind == ir::model::CompartmentKind::Integer)
                .map(|c| c.name.as_str()).collect();
            let real_names: Vec<&str> = model.compartments.iter()
                .filter(|c| c.kind == ir::model::CompartmentKind::Real)
                .map(|c| c.name.as_str()).collect();
            let tr_names: Vec<&str> = model.transitions.iter().map(|t| t.name.as_str()).collect();

            if !traj_header_written {
                writeln!(out, "# {}", version::VERSION).unwrap();
                if total_runs > 1 { write!(out, "replicate\t").unwrap(); }
                write!(out, "t").unwrap();
                for n in &int_names  { write!(out, "\t{}", n).unwrap(); }
                for n in &real_names { write!(out, "\t{}", n).unwrap(); }
                for n in &tr_names   { write!(out, "\tflow_{}", n).unwrap(); }
                writeln!(out).unwrap();
                traj_header_written = true;
            }

            for snap in &traj.snapshots {
                if total_runs > 1 { write!(out, "{}\t", run_idx + 1).unwrap(); }
                write!(out, "{}", snap.t).unwrap();
                for &c in &snap.int_state.counts  { write!(out, "\t{}", c).unwrap(); }
                for &v in &snap.real_state.values { write!(out, "\t{:.4}", v).unwrap(); }
                for &f in &snap.flows.counts      { write!(out, "\t{}", f).unwrap(); }
                writeln!(out).unwrap();
            }
        }

        // ── Observation sampling ───────────��────────────────────────────────
        if want_obs {
            let compiled = std::sync::Arc::new(
                sim::CompiledModel::new(model.clone()).unwrap_or_else(|e| {
                    eprintln!("error compiling model for obs: {:?}", e);
                    std::process::exit(1);
                })
            );
            let params = compiled.default_params.clone();
            let mut obs_rng = sim::rng::StatefulRng::new(obs_seed);

            // Initialize stream names and obs data on first run
            if run_idx == 0 {
                for obs_model in &model.observations {
                    obs_stream_names.push(obs_model.name.clone());
                    obs_data.push(Vec::new());
                    let times = obs_schedule_times(
                        &obs_model.schedule,
                        model.simulation.t_start,
                        model.simulation.t_end,
                    );
                    obs_times_cache.push(times);
                }
            }

            for (si, obs_ir) in model.observations.iter().enumerate() {
                let sampler = sim::inference::obs_model::compile_obs_sample_pf(
                    obs_ir, compiled.clone(), &params,
                );
                let obs_times = &obs_times_cache[si];
                let projected_values = project_all_obs_times(
                    &traj, obs_ir, &model, obs_times,
                );

                for (ti, &obs_t) in obs_times.iter().enumerate() {
                    let draw = sampler(projected_values[ti], &mut obs_rng);
                    obs_data[si].push(ObsRow {
                        time: obs_t,
                        replicate: run_idx + 1,
                        value: draw,
                    });
                }
            }
        }

            run_idx += 1;
        } // end replicates
    } // end draws
    } // end scenarios

    // Flush trajectory output
    drop(traj_out);
    if let Some(ref path) = output_path {
        eprintln!("trajectory written to {}", path);
    }

    // ── Write observation output ────────────���───────────────────────────────
    if want_obs && !obs_data.is_empty() {
        let multi_rep = total_runs > 1;

        // --obs: single wide-format file
        if let Some(ref path) = obs_path {
            let f = std::fs::File::create(path)
                .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", path, e); std::process::exit(1); });
            let mut out = std::io::BufWriter::new(f);

            // Header
            if multi_rep { write!(out, "replicate\t").unwrap(); }
            write!(out, "time").unwrap();
            for name in &obs_stream_names { write!(out, "\t{}", name).unwrap(); }
            writeln!(out).unwrap();

            // All streams share the same schedule (validated above).
            // Rows: iterate over (replicate, time), collect values across streams.
            let n_times = obs_times_cache[0].len();
            for run in 0..total_runs {
                for ti in 0..n_times {
                    let row_idx = run * n_times + ti;
                    if multi_rep { write!(out, "{}\t", run + 1).unwrap(); }
                    write!(out, "{}", obs_data[0][row_idx].time).unwrap();
                    for si in 0..obs_stream_names.len() {
                        let val = obs_data[si][row_idx].value;
                        if val == val.round() && val.abs() < 1e15 {
                            write!(out, "\t{}", val as i64).unwrap();
                        } else {
                            write!(out, "\t{:.6}", val).unwrap();
                        }
                    }
                    writeln!(out).unwrap();
                }
            }
            drop(out);
            eprintln!("observations written to {}", path);
        }

        // --obs-dir: one file per stream
        if let Some(ref dir) = obs_dir {
            for (si, name) in obs_stream_names.iter().enumerate() {
                let path = format!("{}/{}.tsv", dir, name);
                let f = std::fs::File::create(&path)
                    .unwrap_or_else(|e| { eprintln!("cannot create {}: {}", path, e); std::process::exit(1); });
                let mut out = std::io::BufWriter::new(f);

                if multi_rep { write!(out, "replicate\t").unwrap(); }
                writeln!(out, "time\t{}", name).unwrap();

                for row in &obs_data[si] {
                    if multi_rep { write!(out, "{}\t", row.replicate).unwrap(); }
                    let val = row.value;
                    if val == val.round() && val.abs() < 1e15 {
                        writeln!(out, "{}\t{}", row.time, val as i64).unwrap();
                    } else {
                        writeln!(out, "{}\t{:.6}", row.time, val).unwrap();
                    }
                }
                drop(out);
                eprintln!("observations written to {}", path);
            }
        }
    }
}

// ── Observation helpers ─────────────────────────────────────��───────────────

/// Generate observation times from an IR schedule.
fn obs_schedule_times(
    schedule: &ir::observation::ObservationSchedule,
    t_start: f64,
    t_end: f64,
) -> Vec<f64> {
    match schedule {
        ir::observation::ObservationSchedule::Regular(reg) => {
            let mut times = Vec::new();
            let mut t = reg.start;
            while t <= reg.end + 1e-9 {
                times.push(t);
                t += reg.step;
            }
            times
        }
        ir::observation::ObservationSchedule::AtTimes(times) => times.clone(),
        ir::observation::ObservationSchedule::FromData => {
            // In simulate mode there's no data — generate a reasonable grid
            // using the simulation output times (every dt from t_start to t_end).
            eprintln!("warning: observation schedule is 'from_data' but no data provided; \
                       using simulation output grid (every 1 unit from {} to {})", t_start, t_end);
            let mut times = Vec::new();
            let mut t = t_start + 1.0;
            while t <= t_end + 1e-9 {
                times.push(t);
                t += 1.0;
            }
            times
        }
    }
}

/// Project observable quantities from a trajectory at all observation times.
///
/// For CumulativeFlow: accumulate per-snapshot flows, difference between
/// consecutive observation times to get per-interval flow counts.
/// For CurrentPop/CurrentPopSum: read state at snapshot closest to each obs time.
fn project_all_obs_times(
    traj: &sim::Trajectory,
    obs_ir: &ir::observation::ObservationModel,
    model: &ir::Model,
    obs_times: &[f64],
) -> Vec<f64> {
    match &obs_ir.projection {
        ir::observation::Projection::CumulativeFlow(flow_name) => {
            let flow_indices: Vec<usize> = model.transitions.iter().enumerate()
                .filter(|(_, tr)| tr.name == *flow_name || tr.name.starts_with(&format!("{}_", flow_name)))
                .map(|(i, _)| i)
                .collect();

            // Build running cumulative flow at each snapshot time
            let mut cum_at_snap: Vec<(f64, u64)> = Vec::with_capacity(traj.snapshots.len());
            let mut running = 0u64;
            for snap in &traj.snapshots {
                for &fi in &flow_indices {
                    running += snap.flows.counts[fi];
                }
                cum_at_snap.push((snap.t, running));
            }

            // For each obs time, find cumulative flow up to that time.
            // Then difference consecutive obs times.
            let mut cum_at_obs = Vec::with_capacity(obs_times.len());
            let mut snap_idx = 0;
            for &obs_t in obs_times {
                // Advance to last snapshot at or before obs_t
                while snap_idx + 1 < cum_at_snap.len()
                    && cum_at_snap[snap_idx + 1].0 <= obs_t + 1e-9
                {
                    snap_idx += 1;
                }
                cum_at_obs.push(if snap_idx < cum_at_snap.len() && cum_at_snap[snap_idx].0 <= obs_t + 1e-9 {
                    cum_at_snap[snap_idx].1
                } else {
                    0
                });
            }

            // Difference: flow in interval (prev_obs_t, obs_t]
            let mut result = Vec::with_capacity(obs_times.len());
            let mut prev_cum = 0u64;
            for &cum in &cum_at_obs {
                result.push((cum - prev_cum) as f64);
                prev_cum = cum;
            }
            result
        }
        ir::observation::Projection::CurrentPop(comp_name) => {
            let loc = resolve_comp_local(model, &obs_ir.name, comp_name);
            obs_times.iter().map(|&obs_t| {
                let snap = snap_at(traj, obs_t);
                read_comp(snap, &loc)
            }).collect()
        }
        ir::observation::Projection::CurrentPopSum(names) => {
            let locs: Vec<_> = names.iter()
                .map(|name| resolve_comp_local(model, &obs_ir.name, name))
                .collect();
            obs_times.iter().map(|&obs_t| {
                let snap = snap_at(traj, obs_t);
                locs.iter().map(|loc| read_comp(snap, loc)).sum()
            }).collect()
        }
        ir::observation::Projection::DerivedExpr(_) => {
            eprintln!("error: DerivedExpr projection not yet supported for synthetic observations");
            std::process::exit(1);
        }
    }
}

/// Resolved compartment location: integer (local index) or real (local index).
enum CompLoc { Int(usize), Real(usize) }

fn resolve_comp_local(model: &ir::Model, obs_name: &str, comp_name: &str) -> CompLoc {
    let mut int_idx = 0usize;
    let mut real_idx = 0usize;
    for c in &model.compartments {
        if c.name == comp_name {
            return match c.kind {
                ir::model::CompartmentKind::Integer => CompLoc::Int(int_idx),
                ir::model::CompartmentKind::Real => CompLoc::Real(real_idx),
            };
        }
        match c.kind {
            ir::model::CompartmentKind::Integer => int_idx += 1,
            ir::model::CompartmentKind::Real => real_idx += 1,
        }
    }
    eprintln!("error: observation '{}' projects compartment '{}' which doesn't exist",
        obs_name, comp_name);
    std::process::exit(1);
}

fn snap_at(traj: &sim::Trajectory, obs_t: f64) -> &sim::Snapshot {
    traj.snapshots.iter().rev()
        .find(|s| s.t <= obs_t + 1e-9)
        .unwrap_or_else(|| {
            eprintln!("error: no snapshot at or before t={}", obs_t);
            std::process::exit(1);
        })
}

fn read_comp(snap: &sim::Snapshot, loc: &CompLoc) -> f64 {
    match loc {
        CompLoc::Int(i) => snap.int_state.counts[*i] as f64,
        CompLoc::Real(i) => snap.real_state.values[*i],
    }
}

/// Generate N uniform random draws from model parameter bounds.
fn generate_uniform_draws(
    ir_path: &str,
    n: usize,
    seed: u64,
) -> Result<Vec<HashMap<String, f64>>, String> {
    let (model, _) = util::load_model(ir_path)?;
    let mut rng = sim::rng::StatefulRng::new(seed ^ 0xd4a5_b1ce_u64);

    let mut draws = Vec::with_capacity(n);
    for _ in 0..n {
        let mut row = HashMap::new();
        for p in &model.parameters {
            let val = if let Some((lo, hi)) = p.bounds {
                lo + (hi - lo) * rng.uniform()
            } else if let Some(v) = p.value {
                // No bounds — use the default value (constant)
                v
            } else {
                return Err(format!(
                    "parameter '{}' has no bounds and no default value.\n  \
                     --draws uniform requires bounds on all parameters.",
                    p.name
                ));
            };
            row.insert(p.name.clone(), val);
        }
        draws.push(row);
    }
    eprintln!("generated {} uniform draws from parameter bounds ({} params)",
        n, model.parameters.len());
    Ok(draws)
}

/// Parse a seeds spec: "1:100" (range), "42" (single), "1,2,3,42" (list).
fn parse_seeds_spec(spec: &str) -> Result<Vec<u64>, String> {
    // Range: "1:100"
    if spec.contains(':') {
        let parts: Vec<&str> = spec.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("invalid range '{}', expected FROM:TO", spec));
        }
        let from: u64 = parts[0].trim().parse()
            .map_err(|_| format!("cannot parse '{}' as integer", parts[0]))?;
        let to: u64 = parts[1].trim().parse()
            .map_err(|_| format!("cannot parse '{}' as integer", parts[1]))?;
        if from > to {
            return Err(format!("empty range {}:{}", from, to));
        }
        Ok((from..=to).collect())
    }
    // Comma-separated list: "1,2,3,42"
    else if spec.contains(',') {
        spec.split(',')
            .map(|s| s.trim().parse::<u64>()
                .map_err(|_| format!("cannot parse '{}' as integer", s.trim())))
            .collect()
    }
    // Single: "42"
    else {
        let n: u64 = spec.trim().parse()
            .map_err(|_| format!("cannot parse '{}' as integer", spec))?;
        Ok(vec![n])
    }
}

/// Load a draws TSV file. Each row is a complete parameter vector.
/// Column names must match model parameter names.
/// Returns Vec<HashMap<param_name, value>>.
fn load_draws_tsv(path: &str) -> Result<Vec<HashMap<String, f64>>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path, e))?;
    let mut lines = content.lines();
    let header = lines.next()
        .ok_or_else(|| format!("empty draws file: {}", path))?;
    let col_names: Vec<&str> = header.split('\t').collect();
    if col_names.len() < 2 {
        return Err(format!("draws file needs at least 2 columns, got {}", col_names.len()));
    }

    let mut draws = Vec::new();
    for (line_num, line) in lines.enumerate() {
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != col_names.len() {
            return Err(format!(
                "draws file line {}: expected {} columns, got {}",
                line_num + 2, col_names.len(), fields.len()
            ));
        }
        let mut row = HashMap::new();
        for (col, field) in col_names.iter().zip(fields.iter()) {
            let val: f64 = field.trim().parse()
                .map_err(|_| format!(
                    "draws file line {}, column '{}': cannot parse '{}' as number",
                    line_num + 2, col, field
                ))?;
            row.insert(col.to_string(), val);
        }
        draws.push(row);
    }

    if draws.is_empty() {
        return Err(format!("draws file has header but no data rows: {}", path));
    }
    Ok(draws)
}
