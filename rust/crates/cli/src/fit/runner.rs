//! Shared chain-running logic for all fit stages.
//!
//! Handles: model loading, IF2Param construction from fit.toml,
//! dmeasure construction from IR observation model, chain execution,
//! Rhat computation, and MAD-based auto rw_sd calibration.

use crate::fit::config::FitToml;
use crate::fit::state::FitState;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sim::{
    compiled_model::CompiledModel,
    chain_binomial::step_one,
    inference::{
        if2::{run_if2_with_progress, IF2Config, IF2Param, IF2Result, Observation, Transform},
        ParticleState,
    },
    ekrng::StatefulRng,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Everything needed to run IF2 chains, built from fit.toml + optional prior state.
pub struct FitRunConfig {
    pub compiled: Arc<CompiledModel>,
    pub model: ir::Model,
    pub model_ir_json: String,
    pub base_params: Vec<f64>,
    pub if2_params: Vec<IF2Param>,
    pub observations: Vec<Observation>,
    pub flow_indices: Vec<usize>,
    pub if2_config: IF2Config,
    pub n_chains: usize,
    pub seed: u64,
    /// The observation model from the IR (used to compile dmeasure).
    pub obs_model_ir: ir::observation::ObservationModel,
}

/// Result of running multiple IF2 chains.
pub struct ChainResults {
    pub results: Vec<(usize, IF2Result)>,
    pub best_chain: usize,
    pub best_loglik: f64,
    pub rhat: HashMap<String, f64>,
}

impl FitRunConfig {
    /// Build from fit.toml, optionally overriding from a prior fit_state.
    pub fn build(
        fit: &FitToml,
        prior_state: Option<&FitState>,
        n_chains: usize,
        n_particles: usize,
        n_iterations: usize,
        cooling: f64,
        seed: u64,
        random_starts: bool,
    ) -> Result<Self, String> {
        // Load model
        let model_path = &fit.fit.model;
        let (mut model, model_ir_json) = load_model(model_path)?;

        // Apply parameter values from fit.toml BEFORE compiling, so that
        // parameters without model defaults get values.
        // Priority: fit_state start_values > estimate start > fixed value > model default
        for (name, spec) in &fit.estimate {
            if let Some(start) = spec.start {
                if let Some(p) = model.parameters.iter_mut().find(|p| p.name == *name) {
                    if p.value.is_none() { p.value = Some(start); }
                }
            }
        }
        for (name, val) in &fit.fixed {
            if let Some(v) = val.as_float().or_else(|| val.as_integer().map(|i| i as f64)) {
                if let Some(p) = model.parameters.iter_mut().find(|p| p.name == *name) {
                    if p.value.is_none() { p.value = Some(v); }
                }
            }
        }

        let compiled = CompiledModel::new(model.clone())
            .map_err(|e| format!("compile error: {:?}", e))?;
        let mut base_params = compiled.default_params.clone();

        // Apply start overrides from fit_state if provided (overrides model defaults)
        if let Some(state) = prior_state {
            for (name, &value) in &state.start_values {
                if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                    base_params[idx] = value;
                }
            }
        }
        // Apply estimate start values to base_params (may override model defaults)
        for (name, spec) in &fit.estimate {
            if let Some(start) = spec.start {
                if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                    base_params[idx] = start;
                }
            }
        }
        // Apply fixed numeric values to base_params
        for (name, val) in &fit.fixed {
            if let Some(v) = val.as_float().or_else(|| val.as_integer().map(|i| i as f64)) {
                if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                    base_params[idx] = v;
                }
            }
        }

        // Build IF2Param specs
        let if2_params = build_if2_params(
            fit, prior_state, &model, &compiled, &base_params, random_starts, seed,
        )?;

        // Load data (currently single-stream only)
        if fit.data.len() != 1 {
            return Err(format!(
                "fit currently supports exactly 1 data stream, got {}. Multi-stream support coming soon.",
                fit.data.len()
            ));
        }
        let (stream_name, data_path) = fit.data.iter().next().unwrap();

        // Validate observation time alignment
        let dt = fit.config.dt;
        let observations = load_observations(data_path, dt)?;

        // Resolve flow indices from the model's observation blocks
        let flow_indices = resolve_flow_indices(&model, stream_name)?;

        // Get observation model from IR
        let obs_model_ir = model.observations.iter()
            .find(|o| o.name == *stream_name)
            .cloned()
            .ok_or_else(|| format!(
                "no observation block named '{}'. Available: {}",
                stream_name,
                model.observations.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ")
            ))?;

        let config = IF2Config {
            n_particles,
            n_iterations,
            cooling_fraction: cooling,
            cooling_target_iters: 50,
            dt,
        };

        Ok(FitRunConfig {
            compiled: Arc::new(compiled),
            model,
            model_ir_json,
            base_params,
            if2_params,
            observations,
            flow_indices,
            if2_config: config,
            n_chains,
            seed,
            obs_model_ir,
        })
    }
}

/// Load a .camdl or .ir.json model, returning the parsed model and the raw IR JSON.
fn load_model(path: &str) -> Result<(ir::Model, String), String> {
    if path.ends_with(".camdl") {
        let camdlc = std::env::var("CAMDLC").unwrap_or_else(|_| "camdlc".into());
        let output = std::process::Command::new(&camdlc).arg(path).output()
            .map_err(|e| format!("cannot run camdlc: {}", e))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        let json = String::from_utf8(output.stdout)
            .map_err(|e| format!("camdlc output not UTF-8: {}", e))?;
        let model: ir::Model = serde_json::from_str(&json)
            .map_err(|e| format!("parse error: {}", e))?;
        Ok((model, json))
    } else {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path, e))?;
        let model: ir::Model = serde_json::from_str(&json)
            .map_err(|e| format!("parse error: {}", e))?;
        Ok((model, json))
    }
}

/// Build IF2Param specs from fit.toml [estimate] + optional prior state overrides.
fn build_if2_params(
    fit: &FitToml,
    prior_state: Option<&FitState>,
    model: &ir::Model,
    compiled: &CompiledModel,
    base_params: &[f64],
    random_starts: bool,
    seed: u64,
) -> Result<Vec<IF2Param>, String> {
    let mut rng = StatefulRng::new(seed ^ 0xdeadbeef_u64);
    let mut params = Vec::new();

    for (name, spec) in &fit.estimate {
        let idx = compiled.param_index.get(name.as_str()).copied()
            .ok_or_else(|| format!("estimated parameter '{}' not in model", name))?;
        let ir_param = model.parameters.iter().find(|p| p.name == *name).unwrap();

        // Determine bounds: fit bounds override model bounds
        let (lower, upper) = spec.bounds
            .or(ir_param.bounds)
            .unwrap_or((0.0, f64::INFINITY));

        // Determine transform: fit.toml override > param_kind > bounds fallback
        let transform = if let Some(ref t) = spec.transform {
            // Explicit override in fit.toml
            match t.as_str() {
                "log" => Transform::Log,
                "logit" => Transform::Logit,
                "identity" | "none" => Transform::None,
                other => return Err(format!("unknown transform '{}' for '{}'", other, name)),
            }
        } else if let Some(ref kind) = ir_param.param_kind {
            // Derive from DSL parameter type
            match kind.as_str() {
                "probability" => Transform::Logit,
                "rate" | "positive" | "count" => Transform::Log,
                _ => Transform::None,
            }
        } else {
            // Fallback for IR files without param_kind (pre-schema-change)
            if lower >= 0.0 { Transform::Log } else { Transform::None }
        };

        // Determine rw_sd: prior state > explicit fit.toml > auto from bounds
        let rw_sd = prior_state
            .and_then(|s| s.rw_sd.get(name))
            .copied()
            .or(spec.rw_sd)
            .unwrap_or_else(|| auto_rw_sd_from_value(base_params[idx], lower, upper, &transform));

        // Determine initial value
        let initial = if random_starts {
            // Uniform random within bounds
            if lower.is_finite() && upper.is_finite() {
                let u = rng.uniform();
                lower + u * (upper - lower)
            } else {
                // Can't do uniform on unbounded; use current value with jitter
                let v = base_params[idx];
                v * (1.0 + 0.2 * (rng.uniform() - 0.5))
            }
        } else if let Some(ref state) = prior_state {
            state.start_values.get(name).copied().unwrap_or(base_params[idx])
        } else {
            spec.start.unwrap_or(base_params[idx])
        };

        params.push(IF2Param {
            name: name.clone(),
            index: idx,
            initial,
            rw_sd,
            transform,
            lower,
            upper,
            ivp: spec.ivp,
        });
    }

    Ok(params)
}

/// Run a quick pfilter at given params and return the loglik.
/// Used by scout for initial_loglik baseline.
pub fn run_quick_pfilter(config: &FitRunConfig, params: &[f64], n_particles: usize, seed: u64) -> f64 {
    use sim::inference::particle_filter::{self, Observation as PfObs};

    let compiled = &*config.compiled;
    let observations: Vec<PfObs> = config.observations.iter()
        .map(|o| PfObs { time: o.time, value: o.value })
        .collect();

    let step_fn = |state: &mut ParticleState, t: f64, step_dt: f64, rng: &mut StatefulRng, scratch: &mut sim::chain_binomial::StepScratch| {
        step_one(compiled, &mut state.counts, &mut state.flow_accumulators, params, t, step_dt, rng, scratch)
    };
    let flow_indices = &config.flow_indices;
    let project_fn = |state: &ParticleState| -> f64 {
        flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
    };
    let dmeasure_fn = sim::inference::dmeasure::compile_dmeasure_pf(
        &config.obs_model_ir, config.compiled.clone(), params,
    );

    match particle_filter::bootstrap_filter(
        compiled, params, &observations, n_particles, config.if2_config.dt,
        &step_fn, &project_fn, &*dmeasure_fn, None, None, seed,
    ) {
        Ok(result) => result.log_likelihood,
        Err(_) => f64::NEG_INFINITY,
    }
}

/// Public wrapper for use by `camdl if2 --rw-sd auto`.
pub fn auto_rw_sd_from_value_pub(current_value: f64, lower: f64, upper: f64, transform: &Transform) -> f64 {
    auto_rw_sd_from_value(current_value, lower, upper, transform)
}

/// Auto-compute rw_sd from current value and bounds using a fixed perturbation
/// on the transformed scale, converted to natural scale via the delta method.
///
/// Uses 0.1 on the transformed scale → ~10% perturbation for log-transformed
/// parameters, proportional to the local logit gradient for logit-transformed.
/// This matches how IF2Param::transformed_sd converts rw_sd internally.
///
/// Previous implementation used `(hi - lo) / 6` which produced absurd values
/// for wide-range parameters (e.g., S0 in [1000, 10M] → rw_sd = 19M).
fn auto_rw_sd_from_value(current_value: f64, lower: f64, upper: f64, transform: &Transform) -> f64 {
    const TRANSFORMED_SD: f64 = 0.1;

    match transform {
        Transform::Log => {
            // d/dz[e^z] = e^z = current_value
            // 0.1 on log scale → ~10% perturbation
            current_value.abs().max(1e-10) * TRANSFORMED_SD
        }
        Transform::Logit => {
            // Scaled logit on [lower, upper]:
            // d/dz = (x - lo)(hi - x) / (hi - lo)
            let range = upper - lower;
            if range < 1e-10 { return current_value.abs().max(1e-10) * TRANSFORMED_SD; }
            let x = current_value.clamp(lower + 1e-10 * range, upper - 1e-10 * range);
            let deriv = (x - lower) * (upper - x) / range;
            deriv * TRANSFORMED_SD
        }
        Transform::None => {
            let v = current_value.abs();
            if v > 1e-10 {
                v * TRANSFORMED_SD
            } else {
                let lo = if lower.is_finite() { lower } else { -1e6 };
                let hi = if upper.is_finite() { upper } else { 1e6 };
                (hi - lo) / 200.0
            }
        }
    }
}

/// Load observations from TSV, validating time alignment with dt.
fn load_observations(path: &str, dt: f64) -> Result<Vec<Observation>, String> {
    let observations = crate::pfilter::load_data_tsv_pub(path)?;
    // Validate time alignment
    for obs in &observations {
        let remainder = obs.time % dt;
        let aligned = remainder.abs() < 1e-9 || (dt - remainder.abs()).abs() < 1e-9;
        if !aligned {
            return Err(format!(
                "observation at t={} is not a multiple of dt={}.\n\
                 The chain-binomial state only exists at step boundaries.\n\
                 Adjust observation times or dt to align.",
                obs.time, dt
            ));
        }
    }
    Ok(observations.into_iter().map(|o| Observation { time: o.time, value: o.value }).collect())
}

/// Resolve flow indices from the model's observation blocks.
fn resolve_flow_indices(model: &ir::Model, stream_name: &str) -> Result<Vec<usize>, String> {
    // Find observation block matching the data stream name
    if let Some(obs_model) = model.observations.iter().find(|o| o.name == *stream_name) {
        match &obs_model.projection {
            ir::observation::Projection::CumulativeFlow(flow_name) => {
                let indices: Vec<usize> = model.transitions.iter().enumerate()
                    .filter(|(_, tr)| tr.name == *flow_name || tr.name.starts_with(&format!("{}_", flow_name)))
                    .map(|(i, _)| i)
                    .collect();
                if indices.is_empty() {
                    return Err(format!("observation '{}' projects flow '{}', but no matching transition found", stream_name, flow_name));
                }
                Ok(indices)
            }
            _ => Err(format!(
                "observation '{}' uses unsupported projection type. Only CumulativeFlow is supported for fitting.",
                stream_name
            )),
        }
    } else {
        Err(format!(
            "no observation block named '{}' in model.\n\
             Available observations: {}\n\
             The [data] key in fit.toml must match an observation block name in the model.",
            stream_name,
            model.observations.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ")
        ))
    }
}

/// Run one IF2 chain (called from thread::scope).
fn run_one_chain(
    chain_id: usize,
    config: &FitRunConfig,
    per_chain_params: Option<&[IF2Param]>,
    pb: Option<&ProgressBar>,
) -> IF2Result {
    let chain_seed = config.seed ^ (chain_id as u64).wrapping_mul(0x9e3779b97f4a7c15);
    let if2_params = per_chain_params.unwrap_or(&config.if2_params);

    let step_fn = |state: &mut ParticleState, p: &[f64], t: f64, step_dt: f64, rng: &mut StatefulRng, scratch: &mut sim::chain_binomial::StepScratch| {
        step_one(&config.compiled, &mut state.counts, &mut state.flow_accumulators, p, t, step_dt, rng, scratch)
    };
    let project_fn = |state: &ParticleState| -> f64 {
        config.flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
    };
    // Compile dmeasure from the IR observation model
    let dmeasure_fn = sim::inference::dmeasure::compile_dmeasure_if2(
        &config.obs_model_ir, config.compiled.clone(),
    );

    let progress_cb = |iter: usize, loglik: f64| {
        if let Some(bar) = pb {
            bar.set_position((iter + 1) as u64);
            if loglik.is_finite() {
                bar.set_message(format!("ll={:.1}", loglik));
            } else {
                bar.set_message("ll=-inf".to_string());
            }
        }
    };

    let result = run_if2_with_progress(
        &config.compiled, &config.base_params, if2_params, &config.observations,
        &config.if2_config, &step_fn, &project_fn, &*dmeasure_fn, chain_seed,
        Some(&progress_cb),
    ).unwrap_or_else(|e| {
        eprintln!("chain {} error: {:?}", chain_id + 1, e);
        std::process::exit(1);
    });

    if let Some(bar) = pb {
        bar.finish_with_message(format!("ll={:.1}", result.final_loglik));
    }

    result
}

/// Run N chains in parallel, compute Rhat, find best.
pub fn run_chains(config: &FitRunConfig) -> ChainResults {
    run_chains_with_per_chain_params(config, None)
}

/// Run N chains with optional per-chain IF2Param overrides (for scout random starts).
pub fn run_chains_with_per_chain_params(
    config: &FitRunConfig,
    per_chain_params: Option<&[Vec<IF2Param>]>,
) -> ChainResults {
    eprintln!("running {} chains × {} particles × {} iterations, cooling={}, dt={}",
        config.n_chains, config.if2_config.n_particles, config.if2_config.n_iterations,
        config.if2_config.cooling_fraction, config.if2_config.dt);

    let mp = MultiProgress::new();
    let bar_style = ProgressStyle::default_bar()
        .template("  chain {prefix} [{bar:25.cyan/dim}] {pos}/{len} {msg}")
        .unwrap()
        .progress_chars("━╸─");

    let bars: Vec<ProgressBar> = (0..config.n_chains).map(|chain_id| {
        let pb = mp.add(ProgressBar::new(config.if2_config.n_iterations as u64));
        pb.set_style(bar_style.clone());
        pb.set_prefix(format!("{}", chain_id + 1));
        pb
    }).collect();

    let results: Vec<(usize, IF2Result)> = (0..config.n_chains)
        .into_par_iter()
        .map(|chain_id| {
            let per_chain = per_chain_params.map(|pcp| &pcp[chain_id][..]);
            let result = run_one_chain(chain_id, config, per_chain, Some(&bars[chain_id]));
            (chain_id, result)
        })
        .collect();

    // Find best chain
    let (best_chain, best_loglik) = results.iter()
        .max_by(|a, b| a.1.final_loglik.partial_cmp(&b.1.final_loglik).unwrap())
        .map(|(id, r)| (*id, r.final_loglik))
        .unwrap();

    // Compute Rhat
    let rhat = compute_rhat(&results, &config.if2_params, config.if2_config.n_iterations);

    // Report
    eprintln!("\nbest chain: {} (loglik={:.2})", best_chain + 1, best_loglik);
    if config.n_chains > 1 {
        let logliks: Vec<f64> = results.iter().map(|(_, r)| r.final_loglik).collect();
        eprintln!("chain logliks: [{}]",
            logliks.iter().map(|l| format!("{:.1}", l)).collect::<Vec<_>>().join(", "));
    }

    // Report Rhat
    if config.n_chains > 1 {
        eprintln!("\nRhat:");
        for spec in &config.if2_params {
            if let Some(&r) = rhat.get(&spec.name) {
                let status = if r < 1.1 { "\x1b[32m✓\x1b[0m" } else if r < 1.5 { "\x1b[33m~\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
                eprintln!("  {:12} Rhat={:.3} {}", spec.name, r, status);
            }
        }
    }

    ChainResults { results, best_chain, best_loglik, rhat }
}

/// Compute Rhat across chains (last half of iterations).
pub fn compute_rhat(
    results: &[(usize, IF2Result)],
    if2_params: &[IF2Param],
    n_iterations: usize,
) -> HashMap<String, f64> {
    let n_chains = results.len();
    if n_chains < 2 { return HashMap::new(); }

    let n_tail = (n_iterations / 2).max(1);
    let mut rhat_map = HashMap::new();

    for spec in if2_params {
        let chain_means: Vec<f64> = results.iter().map(|(_, r)| {
            let tail: Vec<f64> = r.iterations.iter()
                .skip(n_iterations.saturating_sub(n_tail))
                .map(|it| it.param_means[spec.index])
                .collect();
            tail.iter().sum::<f64>() / tail.len() as f64
        }).collect();

        let chain_vars: Vec<f64> = results.iter().map(|(_, r)| {
            let tail: Vec<f64> = r.iterations.iter()
                .skip(n_iterations.saturating_sub(n_tail))
                .map(|it| it.param_means[spec.index])
                .collect();
            let m = tail.iter().sum::<f64>() / tail.len() as f64;
            tail.iter().map(|&x| (x - m).powi(2)).sum::<f64>() / (tail.len() - 1).max(1) as f64
        }).collect();

        let grand_mean = chain_means.iter().sum::<f64>() / n_chains as f64;
        let between = chain_means.iter().map(|&m| (m - grand_mean).powi(2)).sum::<f64>()
            * n_tail as f64 / (n_chains - 1).max(1) as f64;
        let within = chain_vars.iter().sum::<f64>() / n_chains as f64;
        let rhat = if within > 0.0 {
            (((n_tail as f64 - 1.0) / n_tail as f64 * within + between / n_tail as f64) / within).sqrt()
        } else { f64::NAN };

        rhat_map.insert(spec.name.clone(), rhat);
    }

    rhat_map
}

/// MAD-based auto rw_sd calibration from chain best-loglik parameters.
///
/// Returns (rw_sd map, n_good_chains) or error if no consensus.
pub fn auto_rw_sd(
    results: &[(usize, IF2Result)],
    if2_params: &[IF2Param],
) -> Result<(HashMap<String, f64>, usize), String> {
    let n_chains = results.len();
    if n_chains < 3 {
        return Err("auto rw_sd requires at least 3 chains".into());
    }

    // Collect each chain's best-loglik parameter set
    let chain_params: Vec<Vec<f64>> = results.iter().map(|(_, r)| {
        r.mle.clone()
    }).collect();

    // Per-parameter: compute median and MAD
    let mut medians: Vec<f64> = Vec::new();
    let mut mads: Vec<f64> = Vec::new();

    for spec in if2_params {
        let mut values: Vec<f64> = chain_params.iter()
            .map(|p| p[spec.index])
            .collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let median = if values.len() % 2 == 0 {
            (values[values.len() / 2 - 1] + values[values.len() / 2]) / 2.0
        } else {
            values[values.len() / 2]
        };

        let mut abs_devs: Vec<f64> = values.iter().map(|&v| (v - median).abs()).collect();
        abs_devs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mad = if abs_devs.len() % 2 == 0 {
            (abs_devs[abs_devs.len() / 2 - 1] + abs_devs[abs_devs.len() / 2]) / 2.0
        } else {
            abs_devs[abs_devs.len() / 2]
        };

        medians.push(median);
        mads.push(mad);
    }

    // Classify chains as "good" (all params within 3×MAD of median)
    let good_chains: Vec<bool> = (0..n_chains).map(|c| {
        if2_params.iter().enumerate().all(|(pi, spec)| {
            let v = chain_params[c][spec.index];
            let mad = mads[pi];
            if mad < 1e-15 {
                // All chains agree perfectly on this parameter
                true
            } else {
                (v - medians[pi]).abs() <= 3.0 * mad
            }
        })
    }).collect();

    let n_good = good_chains.iter().filter(|&&g| g).count();

    if n_good < n_chains / 2 {
        // Report which chains diverged and their parameters
        let diverged: Vec<usize> = good_chains.iter().enumerate()
            .filter(|(_, &g)| !g).map(|(i, _)| i + 1).collect();
        return Err(format!(
            "No consensus across chains ({}/{} good). Divergent chains: {:?}\n\
             The likelihood surface may be multimodal or scout iterations are too few.\n\
             Re-run with more iterations or check model specification.",
            n_good, n_chains, diverged
        ));
    }

    if n_good < n_chains {
        let diverged: Vec<usize> = good_chains.iter().enumerate()
            .filter(|(_, &g)| !g).map(|(i, _)| i + 1).collect();
        eprintln!("warning: {}/{} chains diverged ({:?}), excluded from rw_sd calibration",
            n_chains - n_good, n_chains, diverged);
    }

    // rw_sd = 0.5 × MAD of good chains
    let mut rw_sd_map = HashMap::new();
    for (pi, spec) in if2_params.iter().enumerate() {
        let good_values: Vec<f64> = (0..n_chains)
            .filter(|&c| good_chains[c])
            .map(|c| chain_params[c][spec.index])
            .collect();

        let mut abs_devs: Vec<f64> = good_values.iter()
            .map(|&v| (v - medians[pi]).abs())
            .collect();
        abs_devs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let good_mad = if abs_devs.len() % 2 == 0 {
            (abs_devs[abs_devs.len() / 2 - 1] + abs_devs[abs_devs.len() / 2]) / 2.0
        } else {
            abs_devs[abs_devs.len() / 2]
        };

        let rw = 0.5 * good_mad;
        // Floor: don't let rw_sd go below 1% of the median (prevents convergence stall)
        let floor = medians[pi].abs() * 0.01;
        rw_sd_map.insert(spec.name.clone(), rw.max(floor));
    }

    Ok((rw_sd_map, n_good))
}

/// Write per-chain output files: parameter_traces.tsv and final_params.toml.
pub fn write_chain_outputs(
    dir: &str,
    results: &[(usize, IF2Result)],
    if2_params: &[IF2Param],
    all_param_names: &[String],
    base_params: &[f64],
    compiled: &CompiledModel,
) -> Result<(), String> {
    use std::io::Write;

    for (chain_id, result) in results {
        let chain_dir = format!("{}/chain_{}", dir, chain_id + 1);
        std::fs::create_dir_all(&chain_dir)
            .map_err(|e| format!("cannot create {}: {}", chain_dir, e))?;

        // Parameter traces
        let trace_path = format!("{}/parameter_traces.tsv", chain_dir);
        let mut f = std::fs::File::create(&trace_path)
            .map_err(|e| format!("cannot write {}: {}", trace_path, e))?;
        write!(f, "iteration\tloglik").unwrap();
        for spec in if2_params { write!(f, "\t{}", spec.name).unwrap(); }
        writeln!(f).unwrap();
        for it in &result.iterations {
            write!(f, "{}\t{:.2}", it.iteration, it.log_likelihood).unwrap();
            for spec in if2_params { write!(f, "\t{:.6}", it.param_means[spec.index]).unwrap(); }
            writeln!(f).unwrap();
        }

        // Final params TOML (all params, not just estimated)
        let toml_path = format!("{}/final_params.toml", chain_dir);
        let mut f = std::fs::File::create(&toml_path)
            .map_err(|e| format!("cannot write {}: {}", toml_path, e))?;
        writeln!(f, "# Chain {} final parameters", chain_id + 1).unwrap();
        writeln!(f, "# loglik = {:.2}", result.final_loglik).unwrap();
        writeln!(f).unwrap();
        for name in all_param_names {
            let value = if let Some(spec) = if2_params.iter().find(|p| p.name == *name) {
                result.mle[spec.index]
            } else if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                base_params[idx]
            } else {
                0.0
            };
            writeln!(f, "{} = {}", name, format_param_value(value)).unwrap();
        }
    }
    Ok(())
}

/// Format a parameter value with appropriate precision.
/// Shared by chain output and provenance output.
pub fn format_param_value(v: f64) -> String {
    if v.abs() < 1e-6 && v != 0.0 {
        format!("{:.8e}", v)
    } else if v == v.floor() && v.abs() < 1e15 {
        format!("{:.1}", v)
    } else {
        let s = format!("{:.10}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Write diagnostics.tsv: per-iteration loglik for all chains.
pub fn write_diagnostics(dir: &str, results: &[(usize, IF2Result)]) -> Result<(), String> {
    use std::io::Write;
    let path = format!("{}/diagnostics.tsv", dir);
    let mut f = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {}", path, e))?;
    writeln!(f, "chain\titeration\tloglik").unwrap();
    for (chain_id, result) in results {
        for it in &result.iterations {
            writeln!(f, "{}\t{}\t{:.2}", chain_id + 1, it.iteration, it.log_likelihood).unwrap();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that write_chain_outputs writes correct values for BOTH
    /// estimated and fixed parameters. Regression test for bug where
    /// fixed params all got base_params[0] instead of their actual value.
    #[test]
    fn chain_output_fixed_params_correct() {
        use std::collections::HashMap;
        use ir::{
            expr::{BinOpExpr, BinOpWrap, BinOp, Expr, ParamExpr, PopExpr, PopSumExpr},
            model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
            parameter::Parameter,
            transition::{Transition, StoichiometryEntry, DrawMethod},
            Model,
        };

        // SIR model: beta (estimated), gamma (fixed), N0 (fixed)
        let model = Model {
            name: "test".into(),
            version: "0.3".into(),
            time_unit: "days".into(),
            description: None, origin: None,
            compartments: vec![
                Compartment { name: "S".into(), kind: CompartmentKind::Integer },
                Compartment { name: "I".into(), kind: CompartmentKind::Integer },
                Compartment { name: "R".into(), kind: CompartmentKind::Integer },
            ],
            transitions: vec![
                Transition {
                    name: "infection".into(),
                    stoichiometry: vec![StoichiometryEntry("S".into(), -1), StoichiometryEntry("I".into(), 1)],
                    rate: Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                        op: BinOp::Mul,
                        left: Box::new(Expr::Param(ParamExpr { param: "beta".into() })),
                        right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
                    }}),
                    event_key: None, metadata: None, draw_method: DrawMethod::Poisson,
                },
                Transition {
                    name: "recovery".into(),
                    stoichiometry: vec![StoichiometryEntry("I".into(), -1), StoichiometryEntry("R".into(), 1)],
                    rate: Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                        op: BinOp::Mul,
                        left: Box::new(Expr::Param(ParamExpr { param: "gamma".into() })),
                        right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
                    }}),
                    event_key: None, metadata: None, draw_method: DrawMethod::Poisson,
                },
            ],
            ode_equations: vec![], time_functions: vec![], tables: vec![],
            interventions: vec![], observations: vec![],
            parameters: vec![
                Parameter { name: "beta".into(), value: Some(0.3), bounds: Some((0.01, 2.0)), prior: None, transform: None, initial_value: None, param_kind: None },
                Parameter { name: "gamma".into(), value: Some(0.1), bounds: Some((0.01, 1.0)), prior: None, transform: None, initial_value: None, param_kind: None },
                Parameter { name: "N0".into(), value: Some(1000.0), bounds: Some((100.0, 100000.0)), prior: None, transform: None, initial_value: None, param_kind: None },
            ],
            initial_conditions: InitialConditions::Explicit({
                let mut m = HashMap::new();
                m.insert("S".into(), 990.0);
                m.insert("I".into(), 10.0);
                m
            }),
            data_contract: None,
            output: OutputConfig { times: OutputSchedule::AtTimes(vec![0.0, 80.0]), format: "tsv".into(), trajectory: true, observations: false },
            simulation: SimulationConfig { t_start: 0.0, t_end: 80.0, time_semantics: "continuous".into(), dt: Some(1.0), rng_seed: Some(42) },
            presets: vec![], model_structure: None,
        };

        let compiled = CompiledModel::new(model).unwrap();
        let base_params = compiled.default_params.clone();

        // beta is estimated, gamma and N0 are fixed
        let if2_params = vec![IF2Param {
            name: "beta".into(),
            index: compiled.param_index["beta"],
            initial: 0.3,
            rw_sd: 0.05,
            transform: Transform::Log,
            lower: 0.01,
            upper: 2.0,
            ivp: false,
        }];

        // Fake chain result: MLE has beta=0.5
        let mut mle = base_params.clone();
        mle[compiled.param_index["beta"]] = 0.5;

        let results = vec![(0_usize, IF2Result {
            iterations: vec![],
            mle,
            final_loglik: -100.0,
            last_loglik: -100.0,
        })];

        let dir = std::env::temp_dir().join("camdl_test_chain_output");
        let _ = std::fs::remove_dir_all(&dir);

        let param_names: Vec<String> = vec!["beta".into(), "gamma".into(), "N0".into()];
        write_chain_outputs(
            dir.to_str().unwrap(), &results, &if2_params,
            &param_names, &base_params, &compiled,
        ).unwrap();

        // Read back and verify
        let content = std::fs::read_to_string(dir.join("chain_1/final_params.toml")).unwrap();
        let parsed: HashMap<String, f64> = content.lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(|l| {
                let mut parts = l.splitn(2, '=');
                let k = parts.next()?.trim().to_string();
                let v: f64 = parts.next()?.trim().parse().ok()?;
                Some((k, v))
            })
            .collect();

        assert_eq!(parsed["beta"], 0.5, "estimated param should be MLE value");
        assert_eq!(parsed["gamma"], 0.1, "fixed param gamma should be 0.1, not base_params[0]");
        assert_eq!(parsed["N0"], 1000.0, "fixed param N0 should be 1000.0, not base_params[0]");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Compute input hash for provenance (shared by refine and validate).
pub fn compute_fit_input_hash(fit: &FitToml, config: &FitRunConfig, seed: u64) -> String {
    let fit_toml_bytes = toml::to_string(fit).unwrap_or_default().into_bytes();
    let mut data_files: Vec<(String, Vec<u8>)> = fit.data.iter().map(|(name, path)| {
        (name.clone(), std::fs::read(path).unwrap_or_default())
    }).collect();
    crate::fit::provenance::compute_input_hash(
        config.model_ir_json.as_bytes(),
        &mut data_files,
        &fit_toml_bytes,
        seed,
    )
}

/// Collect ALL parameter values (estimated + fixed) for MLE output.
pub fn collect_all_params(
    mle: &[f64],
    if2_params: &[IF2Param],
    model: &ir::Model,
    base_params: &[f64],
    compiled: &CompiledModel,
) -> HashMap<String, f64> {
    let mut params = HashMap::new();
    for p in &model.parameters {
        let idx = compiled.param_index.get(p.name.as_str()).copied().unwrap();
        let value = if let Some(spec) = if2_params.iter().find(|s| s.name == p.name) {
            mle[spec.index]
        } else {
            base_params[idx]
        };
        params.insert(p.name.clone(), value);
    }
    params
}
