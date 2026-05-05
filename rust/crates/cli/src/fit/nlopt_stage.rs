//! Per-stage runner for NLopt deterministic MLE — Phase 1 of the
//! ODE-inference proposal
//! (`docs/dev/proposals/2026-05-04-ode-inference-three-phase.md`).
//!
//! Mirrors the shape of `pgas::run_stage` / `pmmh::run_stage`: parses the
//! NLopt-shaped `Stage` variant, builds a `FitRunConfig`, draws LHS-spread
//! per-chain starts, runs each chain's NLopt optimization in parallel via
//! rayon, aggregates the winner, writes per-chain outputs +
//! `fit_state.toml`, and emits a two-leg convergence diagnostic
//! (chain-agreement + decibans-spread) as a stdout verdict line.
//!
//! The optimizer itself lives in `sim::inference::deterministic`; this
//! module is the orchestration layer wiring it to the fit framework.

use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;
use sim::inference::deterministic::{
    optimize_det, NloptAlgorithm, OptResult, OptStatus,
};

use crate::fit::config_v2::{FitConfigV2, GateConfig, NloptStageConfig, Stage};
use crate::fit::init::{build_chain_param_vecs, InitMethod};
use crate::fit::runner::{compute_ode_loglik, ode_step_dt, FitRunConfig};
use crate::fit::state::FitState;

/// Run a single NLopt-flavoured `Stage` (`Stage::NlSbplx` or
/// `Stage::NlBobyqa`). Errors with a clear message if `stage` is anything
/// else — caller's job to dispatch correctly.
pub fn run_stage(
    fit: &FitConfigV2,
    stage_name: &str,
    stage: &Stage,
    stage_dir: &Path,
    seed: u64,
    starts_from: Option<&str>,
) -> Result<(), String> {
    let (algorithm, knobs) = extract_nlopt_config(stage)?;

    eprintln!(
        "\x1b[33mℹ {} ({}):\x1b[0m deterministic MLE on the ODE-skeleton \
         likelihood.",
        algorithm.as_str(),
        match algorithm {
            NloptAlgorithm::Sbplx => "Subspace simplex; robust to boundary non-smoothness",
            NloptAlgorithm::Bobyqa => "Quadratic trust region; smooth-objective only",
        }
    );
    eprintln!(
        "  camdl computes p(y|θ, ODE_skeleton) under {algorithm}, not the \
         stochastic-process p(y|θ) IF2/PGAS/PMMH compute. In low-noise \
         regimes the two converge empirically; verify rather than assume.",
        algorithm = algorithm.as_str()
    );

    let prior_state = starts_from.map(FitState::load).transpose()?;
    let n_chains = knobs.chains;
    if n_chains == 0 {
        return Err(format!(
            "stage '{stage_name}': chains must be ≥ 1; got 0"
        ));
    }

    // FitRunConfig is shaped around IF2 / PF (n_particles, n_iterations,
    // cooling). NLopt doesn't use any of those — pass placeholders that
    // produce a valid config without affecting the run. `dt` is read from
    // `model.simulation.dt` inside `compute_ode_loglik`; if2_config.dt is
    // a fallback only.
    let run_config = FitRunConfig::build(
        fit,
        prior_state.as_ref(),
        n_chains,
        /* n_particles */ 1,
        /* n_iterations */ 1,
        /* cooling */ 1.0,
        /* cooling_target_iters */ 1,
        seed,
        /* random_starts */ prior_state.is_none(),
    )?;

    let bounds: Vec<(f64, f64)> = run_config
        .estimated_params
        .iter()
        .map(|p| (p.lower, p.upper))
        .collect();
    let est_indices: Vec<usize> = run_config
        .estimated_params
        .iter()
        .map(|p| p.index)
        .collect();
    let est_names: Vec<String> = run_config
        .estimated_params
        .iter()
        .map(|p| p.name.clone())
        .collect();

    // Default to LHS-spread starts even when the user didn't set
    // init_method explicitly — deterministic optimizers from a single
    // starting point find one basin, and the chain-agreement gate has
    // nothing to disagree about.
    let init_method = if knobs.init_method == InitMethod::default() {
        InitMethod::Lhs
    } else {
        knobs.init_method
    };
    let chain_starts: Vec<Vec<f64>> = build_chain_param_vecs(
        init_method,
        &run_config.estimated_params,
        &run_config.base_params,
        n_chains,
        seed,
    )
    .unwrap_or_else(|| vec![run_config.base_params.clone(); n_chains]);

    std::fs::create_dir_all(stage_dir).map_err(|e| {
        format!("creating {}: {}", stage_dir.display(), e)
    })?;

    let arc_config = Arc::new(run_config);
    let bounds_ref = bounds.clone();
    let est_indices_ref = est_indices.clone();

    let t0 = std::time::Instant::now();
    let mut chain_outcomes: Vec<(usize, ChainOutcome)> = (0..n_chains)
        .into_par_iter()
        .map(|chain_idx| {
            let outcome = run_one_chain(
                algorithm,
                knobs,
                &arc_config,
                &bounds_ref,
                &est_indices_ref,
                &chain_starts[chain_idx],
            );
            (chain_idx, outcome)
        })
        .collect();
    chain_outcomes.sort_by_key(|(i, _)| *i);
    let elapsed = t0.elapsed();

    // Pick winner. NEG_INFINITY logliks (model blew up) sort below
    // anything finite; finite-loglik chains beat them automatically.
    let (winner_idx, winner) = chain_outcomes
        .iter()
        .max_by(|a, b| {
            a.1.loglik
                .partial_cmp(&b.1.loglik)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, c)| (*i, c.clone()))
        .ok_or_else(|| "no chains ran".to_string())?;

    // Per-chain final params dump for inspection.
    write_per_chain_files(stage_dir, &chain_outcomes, &est_names)?;

    // Emit convergence diagnostic before writing fit_state — the stdout
    // verdict tells the user whether to trust the winner.
    let chain_logliks: Vec<f64> =
        chain_outcomes.iter().map(|(_, c)| c.loglik).collect();
    let convergence = check_convergence(
        &chain_outcomes,
        &est_names,
        &bounds,
        &knobs.gate,
        knobs.tolerance,
    );
    print_verdict(
        &convergence,
        elapsed.as_secs_f64(),
        n_chains,
        knobs.gate.decibans_thresh,
    );

    // Persist the winner's full parameter vector via fit_state.toml so
    // downstream stages (`refine`, `pgas`, `posterior`) can resume from it.
    let mut start_values = std::collections::HashMap::new();
    let mut rw_sd = std::collections::HashMap::new();
    for (slot, name) in est_names.iter().enumerate() {
        start_values.insert(name.clone(), winner.params[slot]);
        // Keep the auto-derived rw_sd from the run config so a downstream
        // IF2/PMMH refine doesn't have to re-derive it from scratch.
        if let Some(p) = arc_config
            .estimated_params
            .iter()
            .find(|p| p.name == *name)
        {
            rw_sd.insert(name.clone(), p.rw_sd);
        }
    }
    let ivp_params: Vec<String> = arc_config
        .estimated_params
        .iter()
        .filter(|p| p.ivp)
        .map(|p| p.name.clone())
        .collect();

    let fit_state = FitState {
        stage: stage_name.to_string(),
        seed,
        timestamp: crate::cas::iso8601_utc(std::time::SystemTime::now()),
        input_hash: None,
        camdl_version: Some(crate::version::VERSION.to_string()),
        best_loglik: winner.loglik,
        initial_loglik: f64::NAN,
        best_chain: winner_idx,
        n_chains,
        n_good_chains: Some(
            chain_outcomes
                .iter()
                .filter(|(_, c)| matches!(c.status, OptStatus::Converged(_)))
                .count(),
        ),
        start_values,
        rw_sd,
        loglik_type: Some("ode_marginal".to_string()),
        acceptance_rate: None,
        tail_chain_agreement: convergence.chain_agreement.clone(),
        ivp_params,
        chain_logliks,
        chain_eval_logliks: Vec::new(),
        chain_eval_ses: Vec::new(),
        resolved_gate: Some(knobs.gate.clone()),
        resolved_loglik_eval: None,
    };
    fit_state
        .save(&stage_dir.to_string_lossy())
        .map_err(|e| format!("writing fit_state.toml: {e}"))?;

    Ok(())
}

fn extract_nlopt_config(
    stage: &Stage,
) -> Result<(NloptAlgorithm, &NloptStageConfig), String> {
    match stage {
        Stage::NlSbplx(c) => Ok((NloptAlgorithm::Sbplx, c)),
        Stage::NlBobyqa(c) => Ok((NloptAlgorithm::Bobyqa, c)),
        other => Err(format!(
            "nlopt_stage::run_stage: expected nl-sbplx or nl-bobyqa, got {}",
            other.method_name()
        )),
    }
}

#[derive(Clone)]
struct ChainOutcome {
    /// Optimized parameter vector restricted to estimated slots
    /// (in `est_names` order).
    params: Vec<f64>,
    loglik: f64,
    status: OptStatus,
    n_evals: usize,
}

fn run_one_chain(
    algorithm: NloptAlgorithm,
    knobs: &NloptStageConfig,
    config: &Arc<FitRunConfig>,
    bounds: &[(f64, f64)],
    est_indices: &[usize],
    full_start: &[f64],
) -> ChainOutcome {
    // Extract only the estimated-param slots from the chain's full
    // starting vector — that's what NLopt sees.
    let initial_est: Vec<f64> = est_indices.iter().map(|&i| full_start[i]).collect();

    // Build the obs model + obs-time vector ONCE per chain. The NLopt
    // closure runs hundreds of times; rebuilding inside the closure
    // would re-resolve all stream likelihoods on every call. The shared
    // FitRunConfig already paid for those resolutions.
    let obs_model = config.build_obs_model();
    let obs_times: Vec<f64> = config.observations.iter().map(|o| o.time).collect();
    let dt = ode_step_dt(config);

    // Closure must outlive every NLopt callback. Owns: full param vector
    // (mutated in-place per call), the index list mapping optimizer
    // slots to model indices, and the per-chain obs-eval handles.
    let mut full_params = full_start.to_vec();
    let est_indices_local = est_indices.to_vec();
    let config_local = Arc::clone(config);
    let objective = move |est: &[f64]| -> f64 {
        for (slot, &model_idx) in est_indices_local.iter().enumerate() {
            full_params[model_idx] = est[slot];
        }
        compute_ode_loglik(
            &config_local.compiled,
            &obs_model,
            &obs_times,
            dt,
            &full_params,
        )
        .unwrap_or(f64::NEG_INFINITY)
    };

    let result: Result<OptResult, String> = optimize_det(
        algorithm,
        &initial_est,
        bounds,
        knobs.tolerance,
        knobs.max_evals,
        objective,
    );

    match result {
        Ok(r) => ChainOutcome {
            params: r.params,
            loglik: r.loglik,
            status: r.status,
            n_evals: r.n_evals,
        },
        Err(e) => {
            eprintln!("nlopt chain failed config: {e}");
            ChainOutcome {
                params: initial_est,
                loglik: f64::NEG_INFINITY,
                status: OptStatus::Failed,
                n_evals: 0,
            }
        }
    }
}

fn write_per_chain_files(
    stage_dir: &Path,
    chain_outcomes: &[(usize, ChainOutcome)],
    est_names: &[String],
) -> Result<(), String> {
    use std::io::Write;
    let path = stage_dir.join("chain_results.tsv");
    let mut f = std::fs::File::create(&path)
        .map_err(|e| format!("creating {}: {}", path.display(), e))?;
    write!(f, "chain\tloglik\tstatus\tn_evals").map_err(io_err)?;
    for name in est_names {
        write!(f, "\t{name}").map_err(io_err)?;
    }
    writeln!(f).map_err(io_err)?;
    for (chain_idx, c) in chain_outcomes {
        write!(
            f,
            "{}\t{:.6}\t{}\t{}",
            chain_idx + 1,
            c.loglik,
            c.status.as_str(),
            c.n_evals,
        )
        .map_err(io_err)?;
        for v in &c.params {
            write!(f, "\t{v:.10}").map_err(io_err)?;
        }
        writeln!(f).map_err(io_err)?;
    }
    Ok(())
}

fn io_err(e: std::io::Error) -> String {
    format!("io error: {e}")
}

/// Two-leg convergence diagnostic for NLopt stages — generalises IF2's
/// compound gate (chain-agreement + decibans-spread) to deterministic
/// optimizers. See proposal §"Convergence diagnostics for NLopt chains".
struct ConvergenceVerdict {
    /// Per-parameter relative range across converged chains.
    /// `(name, rel_range, abs_range, bound_width)`. Used by the
    /// chain-agreement leg of the gate.
    chain_agreement: std::collections::HashMap<String, f64>,
    /// `max(rel_range) / bound_width` over params — single scalar
    /// summary for the verdict line.
    max_rel_range: f64,
    /// Maximum absolute range over params, in natural units. Used to
    /// distinguish "tight cluster, large bound" from "tight bound, big
    /// optimizer noise".
    max_abs_range: f64,
    /// `max(loglik) - min(loglik)` across converged chains, in nats.
    /// Decibans = nats × NATS_TO_DB; the threshold compare uses
    /// `delta_nats * NATS_TO_DB` against `gate.decibans_thresh`.
    delta_loglik: f64,
    /// `true` iff the configured thresholds were both exceeded.
    chain_agreement_failed: bool,
    decibans_failed: bool,
    /// Number of converged (Success / X/F-tol) chains.
    n_converged: usize,
    /// Number of soft-failed (MaxEvalReached) chains.
    n_maxeval: usize,
    /// Number of hard-failed (Failed) chains.
    n_failed: usize,
}

const NATS_TO_DB: f64 = 4.342944819032518;
/// Per the proposal, threshold the chain-agreement leg fires only when
/// BOTH relative range > 5% bound AND absolute range > 2 × `xtol_rel`-
/// implied numerical floor are violated. The 0.05 placeholder is
/// calibrated against the typhoid diagnostic experiment downstream.
const DET_REL_RANGE_THRESH: f64 = 0.05;
const DET_ABS_RANGE_FACTOR: f64 = 2.0;

fn check_convergence(
    chain_outcomes: &[(usize, ChainOutcome)],
    est_names: &[String],
    bounds: &[(f64, f64)],
    gate: &GateConfig,
    tolerance: f64,
) -> ConvergenceVerdict {
    use std::collections::HashMap;

    let n_converged = chain_outcomes
        .iter()
        .filter(|(_, c)| matches!(c.status, OptStatus::Converged(_)))
        .count();
    let n_maxeval = chain_outcomes
        .iter()
        .filter(|(_, c)| matches!(c.status, OptStatus::MaxEvalReached))
        .count();
    let n_failed = chain_outcomes
        .iter()
        .filter(|(_, c)| matches!(c.status, OptStatus::Failed | OptStatus::MaxTimeReached))
        .count();

    let mut chain_agreement = HashMap::new();
    let mut max_rel = 0.0f64;
    let mut max_abs = 0.0f64;
    let mut chain_agreement_failed = false;
    if chain_outcomes.len() >= 2 {
        for (slot, name) in est_names.iter().enumerate() {
            let vals: Vec<f64> =
                chain_outcomes.iter().map(|(_, c)| c.params[slot]).collect();
            let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let abs_range = max - min;
            let bound_width = (bounds[slot].1 - bounds[slot].0).abs().max(1e-300);
            let rel_range = abs_range / bound_width;
            chain_agreement.insert(name.clone(), rel_range);
            if rel_range > max_rel {
                max_rel = rel_range;
            }
            if abs_range > max_abs {
                max_abs = abs_range;
            }
            // Per proposal: refuse only if BOTH legs exceed thresholds —
            // a tight cluster on a wide bound is fine.
            let abs_floor = DET_ABS_RANGE_FACTOR
                * tolerance
                * (vals[0].abs().max(1.0));
            if rel_range > DET_REL_RANGE_THRESH && abs_range > abs_floor {
                chain_agreement_failed = true;
            }
        }
    }

    let logliks: Vec<f64> = chain_outcomes
        .iter()
        .filter(|(_, c)| matches!(c.status, OptStatus::Converged(_)))
        .map(|(_, c)| c.loglik)
        .collect();
    let delta_loglik = if logliks.len() >= 2 {
        let lmax = logliks.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let lmin = logliks.iter().cloned().fold(f64::INFINITY, f64::min);
        lmax - lmin
    } else {
        0.0
    };
    let decibans_failed = delta_loglik * NATS_TO_DB > gate.decibans_thresh;

    ConvergenceVerdict {
        chain_agreement,
        max_rel_range: max_rel,
        max_abs_range: max_abs,
        delta_loglik,
        chain_agreement_failed,
        decibans_failed,
        n_converged,
        n_maxeval,
        n_failed,
    }
}

fn print_verdict(
    v: &ConvergenceVerdict,
    wall_secs: f64,
    n_chains: usize,
    decibans_thresh: f64,
) {
    let ok = "\x1b[32m✓\x1b[0m";
    let bad = "\x1b[31m✗\x1b[0m";
    eprintln!();
    eprintln!(
        "  status: {} converged, {} max-eval, {} failed (of {})",
        v.n_converged, v.n_maxeval, v.n_failed, n_chains
    );
    eprintln!(
        "  chain-agreement: rel range = {:.2}% bound | abs range = {:.3e}   {}",
        v.max_rel_range * 100.0,
        v.max_abs_range,
        if v.chain_agreement_failed { bad } else { ok }
    );
    eprintln!(
        "  loglik-eval:     Δ = {:.1} dB / threshold {:.0} dB                {}",
        v.delta_loglik * NATS_TO_DB,
        decibans_thresh,
        if v.decibans_failed { bad } else { ok }
    );
    eprintln!("  wall: {:.2}s ({} chains)", wall_secs, n_chains);
}
