//! Shared chain-running logic for all fit stages.
//!
//! Handles: model loading, EstimatedParam construction from fit.toml,
//! obs_loglik construction from IR observation model, chain execution,
//! chain-agreement (Â) computation, and MAD-based auto rw_sd calibration.

use crate::fit::loglik_eval;
use crate::fit::state::FitState;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sim::{
    compiled_model::CompiledModel,
    inference::{
        if2::{run_if2_with_progress, IF2Config, EstimatedParam, IF2Result, Observation, Transform},
        pmmh::Prior,
        diagnostic::{DiagnosticCollector, DiagnosticKind},
    },
    rng::StatefulRng,
};
use ir::parameter::HierarchicalKind;
use std::collections::HashMap;
use std::sync::Arc;

/// Everything needed to run IF2 chains, built from fit.toml + optional prior state.
/// One observation data stream with its projection and likelihood.
pub struct ObsStream {
    pub name: String,
    /// Resolved projection (incidence / prevalence / snapshot expression)
    /// built from the IR observation block.
    pub projection: sim::inference::multi_stream_obs::StreamProjection,
    pub obs_model_ir: ir::observation::ObservationModel,
    pub data: Vec<Observation>,
}

pub struct FitRunConfig {
    pub compiled: Arc<CompiledModel>,
    pub model: ir::Model,
    /// Pre-filter snapshot — every intervention and event declared in the
    /// model file, whether or not the active scenario enabled it. Used by
    /// `print_scheduled_actions_summary` to show a "N active of M declared"
    /// block on startup.
    pub model_declared: ir::Model,
    pub model_ir_json: String,
    pub base_params: Vec<f64>,
    /// Names of all parameters, parallel to `base_params`. Built from
    /// `model.parameters` at setup time. Used by the PMMH / PGAS
    /// hierarchical-prior env to resolve `Expr::Param(name)` references
    /// against current values. Wave 2 / #3 Gate 3a.
    pub param_names: Vec<String>,
    pub estimated_params: Vec<EstimatedParam>,
    /// Canonical observation times (shared across all streams).
    pub observations: Vec<Observation>,
    /// Per-stream data. For single-stream models, len() == 1.
    pub streams: Vec<ObsStream>,
    pub if2_config: IF2Config,
    pub n_chains: usize,
    pub seed: u64,
    /// IC-free inference flag. When true, the PF/IF2/PGAS log-likelihood
    /// accumulation skips the first observation (y₁ is still used to
    /// weight and resample — that's how the initial state gets pinned).
    /// Mirrors `FitConfigV2::ic_free`.
    /// Flows into `SMCConfig.skip_first_obs_from_loglik`. See
    /// docs/dev/proposals/2026-04-18-ic-free-inference.md.
    pub ic_free: bool,
    /// Clean-evaluation re-scoring config (Step 4 plumbing for §Proposal 1).
    /// Set per stage at the `camdl fit run` dispatch site (CLI overrides
    /// over stage TOML); legacy `camdl fit scout`/`fit refine` use the
    /// `Default` (4000 × 8, logmeanexp). Consumed by Step 5.
    pub loglik_eval: super::config_v2::LoglikEvalConfig,
    /// Compound scout-convergence gate config (Step 4 plumbing for
    /// §Proposal 3). Same per-stage override semantics as `loglik_eval`.
    /// Consumed by Step 8.
    pub gate: super::config_v2::GateConfig,
}

/// Result of running multiple IF2 chains.
///
/// `best_chain` / `best_loglik` are the clean-eval winner — each
/// chain's IF2 final-iteration mean re-scored with M high-particle
/// PF replicates and combined via logmeanexp on the likelihood scale
/// (matches pomp's `coef(mif2_out)` + `pfilter` workflow; Ionides et
/// al. 2015 PNAS). They no longer reflect in-run noisy
/// `IF2Result::final_loglik` argmax — that selection was upward-biased
/// from argmaxing over noisy in-run PF estimates. The full per-chain
/// table lives in `loglik_eval.per_chain`; consumers needing the
/// winner's θ̂ / SE read from
/// `loglik_eval.per_chain[overall_winner_idx]`.
pub struct ChainResults {
    pub results: Vec<(usize, IF2Result)>,
    pub best_chain: usize,
    pub best_loglik: f64,
    pub chain_agreement: HashMap<String, f64>,
    pub loglik_eval: super::loglik_eval::LoglikEvalOutcome,
}

impl FitRunConfig {
    /// Build from a v2 fit.toml, optionally overriding from a prior fit_state.
    ///
    /// `cooling_target_iters` is IF2-specific — for non-IF2 stages
    /// (PGAS / PMMH / PFilter), passing `n_iterations` matches the
    /// pre-2026-04-30 behavior. The IF2 dispatch site reads it from
    /// `Stage::IF2.cooling_target_iters` (default 50).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        fit: &super::config_v2::FitConfigV2,
        prior_state: Option<&FitState>,
        n_chains: usize,
        n_particles: usize,
        n_iterations: usize,
        cooling: f64,
        cooling_target_iters: usize,
        seed: u64,
        random_starts: bool,
    ) -> Result<Self, String> {
        // Load model
        let model_path = &fit.model.camdl;
        let (mut model, model_ir_json) = crate::util::load_model(model_path)?;
        // Keep a copy of the unfiltered model so the startup diagnostic
        // can show what was declared vs what's active. Cheap clone — the
        // intervention list is small.
        let model_declared = model.clone();

        // Apply scenario / enable / disable filter BEFORE compile.
        // Per spec §14.4, toggleable interventions default OFF; events
        // (always_active) stay on unless explicitly disabled. If neither
        // scenario nor enable/disable are set in fit.toml, interventions
        // are cleared (spec default). Shared helper with simulate/pfilter
        // so the three entry points cannot drift.
        if fit.scenario.is_some()
            && (!fit.enable.is_empty() || !fit.disable.is_empty())
        {
            return Err("fit.toml: `scenario` is mutually exclusive \
                with `enable`/`disable`. Use one approach.".into());
        }
        let (enable_list, disable_list) = if let Some(ref name) = fit.scenario {
            let preset = model.presets.iter().find(|p| p.name == *name).cloned()
                .ok_or_else(|| {
                    let avail: Vec<&str> = model.presets.iter()
                        .map(|p| p.name.as_str()).collect();
                    format!("scenario '{}' not found in model. Available: {}",
                        name,
                        if avail.is_empty() { "(none)".into() } else { avail.join(", ") })
                })?;
            // Apply scenario's param overrides so the fit sees the
            // scenario's parameter defaults (matches simulate semantics).
            for p in &mut model.parameters {
                if let Some(&v) = preset.params.get(&p.name) { p.value = Some(v); }
            }
            (preset.enable, preset.disable)
        } else {
            (fit.enable.clone(), fit.disable.clone())
        };
        crate::util::apply_scenario_filter(&mut model, &enable_list, &disable_list)?;

        // Resolve fixed up-front (file load + inline overlay). v2's
        // FixedParams.resolve() can fail (file-not-found / parse), so
        // propagate the Result rather than swallowing.
        let fixed_resolved = fit.fixed.resolve()?;

        // Apply parameter values from fit.toml BEFORE compiling, so that
        // parameters without model defaults get values.
        // Priority: fit_state start_values > estimate start > fixed value > model default
        //
        // gh#34: when [estimate] entry has no explicit `start =` AND
        // the model param has no value yet (no scenario default, no
        // model-declared `value`), fall back to the bounds midpoint.
        // Geometric mean for log-friendly bounds (both > 0); arithmetic
        // otherwise. Picks a position that won't immediately bias IF2
        // toward either bound — better than refusing to fit, much better
        // than forcing every estimate entry to carry a redundant start.
        for (name, spec) in &fit.estimate {
            if let Some(p) = model.parameters.iter_mut().find(|p| p.name == *name) {
                if p.value.is_none() {
                    let value = spec.start.unwrap_or_else(|| {
                        let (lo, hi) = spec.bounds;
                        if lo > 0.0 && hi > 0.0 {
                            (lo * hi).sqrt()         // geometric mean
                        } else {
                            0.5 * (lo + hi)          // arithmetic mean
                        }
                    });
                    p.value = Some(value);
                }
            }
        }
        for (name, &v) in &fixed_resolved {
            if let Some(p) = model.parameters.iter_mut().find(|p| p.name == *name) {
                if p.value.is_none() { p.value = Some(v); }
            }
        }

        // Bounds + finite-value check after all override paths resolved
        // (estimate.start, fixed, scenario params). Validates the `value`
        // field on `model.parameters`; the post-compile `base_params`
        // writes from `prior_state` are inference-engine state and out
        // of scope here. See gh#31.
        crate::util::validate_parameter_values(&model)?;

        let compiled = CompiledModel::new(model.clone())
            .map_err(|e| format!("compile error: {:?}", e))?;
        let mut base_params = compiled.default_params.clone();

        // Priority: prior_state > estimate.start > fixed > model default.
        // `base_params` is the single source of truth for IF2's starting
        // point: run_if2_with_progress initialises its particle cloud
        // from `base_params`, not from `EstimatedParam::initial`. If
        // prior_state is applied before est.start (as was the case
        // before 2026-04-18), the est.start write silently overwrites
        // the scout-best values, and `starts_from = "scout"` becomes a
        // no-op for refine's iter-0 parameters. See
        // docs/dev/incidents/2026-04-18-starts-from-scout-ignored.md.

        // 1. Apply estimate start values to base_params (override model defaults).
        for (name, spec) in &fit.estimate {
            if let Some(start) = spec.start {
                if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                    base_params[idx] = start;
                }
            }
        }
        // 2. Apply fixed numeric values (override model defaults).
        for (name, &v) in &fixed_resolved {
            if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                base_params[idx] = v;
            }
        }
        // 3. Apply prior_state last so it wins over config start/fixed.
        //    This is what makes `starts_from = "scout"` actually seed
        //    the IF2 search from scout's best MLE.
        if let Some(state) = prior_state {
            for (name, &value) in &state.start_values {
                if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                    base_params[idx] = value;
                }
            }
        }

        // Build EstimatedParam specs
        let if2_params = build_if2_params(
            &fit.estimate, prior_state, &model, &compiled, &base_params, random_starts, seed,
        )?;

        // Load data — one or more observation streams (real-data only;
        // synthetic-data fits route through a generator before this path).
        // Resolve any single-file shorthand (`[data] file = "..."`) into
        // the canonical per-stream map by mapping every model-declared
        // stream name to that file. From here on the loop is the same.
        let dt = fit.config.dt;
        let data_spec = fit.data_spec()?;
        let model_obs_names: Vec<String> = model.observations.iter()
            .map(|o| o.name.clone()).collect();
        let effective = data_spec.effective_observations(&model_obs_names)?;
        if effective.is_empty() {
            return Err(
                "fit.toml [data] resolves to zero observation streams. Either \
                 set `[data] file = \"<path>\"` (one wide TSV) or fill \
                 [data.observations] (per-stream paths).".into());
        }

        let mut streams = Vec::new();
        let mut canonical_times: Option<Vec<f64>> = None;

        // Sort by name for deterministic ordering. (IndexMap preserves
        // insertion order — we pin a sort here so two fits with the
        // same observations but different toml ordering still hash
        // identically downstream.)
        let mut data_entries: Vec<_> = effective.iter().collect();
        data_entries.sort_by_key(|(k, _)| k.as_str());

        for (stream_name, data_path) in &data_entries {
            let obs = load_observations(data_path, stream_name, dt)?;
            let obs_model = model.observations.iter()
                .find(|o| o.name == **stream_name)
                .cloned()
                .ok_or_else(|| format!(
                    "no observation block named '{}'. Available: {}",
                    stream_name,
                    model.observations.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ")
                ))?;
            let projection = sim::inference::multi_stream_obs::StreamProjection::from_ir(
                &obs_model.projection, &compiled, stream_name,
            )?;

            // Validate all streams share the same observation times
            let times: Vec<f64> = obs.iter().map(|o| o.time).collect();
            match &canonical_times {
                None => canonical_times = Some(times),
                Some(ct) => {
                    if ct.len() != times.len() || ct.iter().zip(&times).any(|(a, b)| (a - b).abs() > 1e-9) {
                        return Err(format!(
                            "observation times for stream '{}' differ from first stream. \
                             All streams must have identical observation times.",
                            stream_name
                        ));
                    }
                }
            }

            streams.push(ObsStream {
                name: stream_name.to_string(),
                projection,
                obs_model_ir: obs_model,
                data: obs,
            });
        }

        // Canonical observations (from first stream)
        let observations = streams[0].data.clone();

        if streams.len() > 1 {
            eprintln!("  {} observation streams: {}",
                streams.len(),
                streams.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "));
        }

        let ic_free = fit.ic_free.unwrap_or(false);

        // Resolve top-level fit.simplex_groups into sim::SimplexGroup
        // (param names → indices, rw_sds from EstimateSpecV2.rw_sd).
        // Only IF2 currently honours these; PGAS/PMMH/PFilter consume
        // the same FitRunConfig but ignore simplex_groups (validation
        // emits a warning when a non-IF2 stage runs against a fit
        // with simplex groups declared).
        let resolved_simplex_groups = resolve_simplex_groups(
            &fit.simplex_groups, &fit.estimate, &compiled.param_index, &if2_params)?;

        let config = IF2Config {
            n_particles,
            n_iterations,
            cooling_fraction: cooling,
            cooling_target_iters,
            simplex_groups: resolved_simplex_groups,
            dt,
            t_start: compiled.model.simulation.t_start,
            skip_first_obs_from_loglik: ic_free,
        };
        // IC-free precondition: at least one estimated param must be
        // marked ivp. Without per-particle spread at t=0, the first
        // reweight can't discriminate and ic-free degenerates to
        // silently dropping y₁. Error at config build so the mistake
        // surfaces before any PF time is spent.
        if ic_free && !if2_params.iter().any(|p| p.ivp) {
            return Err(
                "ic_free = true requires at least one [estimate.*] entry with \
                 ivp = true. Without per-particle variation at t=0, the first \
                 observation cannot discriminate between particles and ic_free \
                 degenerates to dropping the first data point.\n\n\
                 Example: mark your initial-state parameter as ivp:\n\n    \
                 [estimate]\n    I0 = { bounds = [1, 500], ivp = true }".into());
        }

        let param_names: Vec<String> =
            model.parameters.iter().map(|p| p.name.clone()).collect();
        Ok(FitRunConfig {
            compiled: Arc::new(compiled),
            model,
            model_declared,
            model_ir_json,
            base_params,
            param_names,
            estimated_params: if2_params,
            observations,
            streams,
            if2_config: config,
            n_chains,
            seed,
            ic_free,
            loglik_eval: super::config_v2::LoglikEvalConfig::default(),
            gate: super::config_v2::GateConfig::default(),
        })
    }

    pub fn build_process(&self) -> sim::inference::ChainBinomialProcess {
        sim::inference::ChainBinomialProcess::new(self.compiled.clone())
    }
    pub fn build_obs_model(&self) -> sim::inference::MultiStreamObsModel {
        sim::inference::MultiStreamObsModel::new(
            self.streams.iter().map(|s| sim::inference::multi_stream_obs::StreamSpec {
                projection: s.projection.clone(),
                ir_model: s.obs_model_ir.clone(),
                observations: s.data.iter().map(|o| o.value).collect(),
                obs_times: self.observations.iter().map(|o| o.time).collect(),
            }).collect(),
            self.compiled.clone(),
        ).unwrap_or_else(|e| {
            eprintln!("error: observation model construction failed: {:?}", e);
            std::process::exit(1);
        })
    }
    pub fn smc_config(&self) -> sim::inference::traits::SMCConfig {
        sim::inference::traits::SMCConfig {
            n_particles: self.if2_config.n_particles,
            dt: self.if2_config.dt,
            t_start: self.compiled.model.simulation.t_start,
            skip_first_obs_from_loglik: self.ic_free,
            record_ancestry: false,
            record_prequential: false,
        }
    }
}

// load_model is now in util.rs

/// Resolve top-level fit.toml `[[simplex_groups]]` entries (param
/// names) into runtime `sim::inference::if2::SimplexGroup` (indices
/// into the model param vector + rw_sds on the log-ratio scale).
///
/// Validation enforced here:
/// - Each member must appear in `[estimate]` (validated upstream by
///   `FitConfigV2::validate`, but defended again here).
/// - Each member must resolve to a model param index.
/// - rw_sd is read from the corresponding `EstimatedParam.rw_sd` (which
///   already encodes auto-derivation when `EstimateSpecV2.rw_sd` is None).
///
/// rw_sd semantics: the IF2 simplex transform perturbs members on the
/// log-ratio scale. The user's `EstimateSpecV2.rw_sd` for a simplex
/// member is taken as-is on that scale (matches pomp's
/// `parameter_trans(barycentric = ...)` + `rw.sd` semantics).
fn resolve_simplex_groups(
    cli_groups: &[super::config_v2::SimplexGroup],
    estimate: &indexmap::IndexMap<String, super::config_v2::EstimateSpecV2>,
    param_index: &HashMap<String, usize>,
    if2_params: &[EstimatedParam],
) -> Result<Vec<sim::inference::if2::SimplexGroup>, String> {
    let mut out = Vec::with_capacity(cli_groups.len());
    for (group_idx, group) in cli_groups.iter().enumerate() {
        let mut indices = Vec::with_capacity(group.params.len());
        let mut rw_sds = Vec::with_capacity(group.params.len());
        for name in &group.params {
            if !estimate.contains_key(name) {
                return Err(format!(
                    "simplex_groups[{}]: member '{}' not in [estimate]. \
                     Members must be free parameters.",
                    group_idx, name));
            }
            let &model_idx = param_index.get(name).ok_or_else(|| format!(
                "simplex_groups[{}]: member '{}' has no model param index \
                 (model load + estimate parity drift?)",
                group_idx, name))?;
            let if2_rw_sd = if2_params.iter()
                .find(|p| p.name == *name)
                .ok_or_else(|| format!(
                    "simplex_groups[{}]: member '{}' missing from \
                     resolved EstimatedParam list (build_if2_params drift?)",
                    group_idx, name))?
                .rw_sd;
            indices.push(model_idx);
            rw_sds.push(if2_rw_sd);
        }
        out.push(sim::inference::if2::SimplexGroup { indices, rw_sds });
    }
    Ok(out)
}

/// Build EstimatedParam specs from v2 [estimate] + optional prior state overrides.
/// Uses the shared build_if2_params_from_specs for core logic, then applies
/// fit-specific overrides (prior state rw_sd, start values, random starts).
fn build_if2_params(
    estimate: &indexmap::IndexMap<String, super::config_v2::EstimateSpecV2>,
    prior_state: Option<&FitState>,
    model: &ir::Model,
    compiled: &CompiledModel,
    base_params: &[f64],
    random_starts: bool,
    seed: u64,
) -> Result<Vec<EstimatedParam>, String> {
    // Build ParamSpecs from v2 [estimate]
    let specs: Vec<ParamSpec> = estimate.iter().map(|(name, est)| {
        // rw_sd priority: prior state > fit.toml explicit > None (auto)
        let rw_sd = prior_state
            .and_then(|s| s.rw_sd.get(name))
            .copied()
            .or(est.rw_sd);
        ParamSpec {
            name: name.clone(),
            rw_sd,
            transform: est.transform.as_ref().map(|t| t.as_str().to_string()),
            ivp: est.ivp,
        }
    }).collect();

    let mut params = build_if2_params_from_specs(model, compiled, base_params, &specs)?;

    // Sort by name for deterministic ordering. IndexMap preserves
    // insertion order, but to keep param order stable across configs
    // that list the same params in different orders (and so resume's
    // z-value mapping survives) we sort by name.
    params.sort_by(|a, b| a.name.cmp(&b.name));

    // Fit-specific: apply start values and random starts
    let mut rng = StatefulRng::new(seed ^ 0xdeadbeef_u64);
    for p in &mut params {
        if random_starts {
            if p.lower.is_finite() && p.upper.is_finite() {
                p.initial = p.lower + rng.uniform() * (p.upper - p.lower);
            } else {
                p.initial *= 1.0 + 0.2 * (rng.uniform() - 0.5);
            }
        } else if let Some(state) = prior_state {
            if let Some(&v) = state.start_values.get(&p.name) {
                p.initial = v;
            }
        } else if let Some(est) = estimate.get(&p.name) {
            if let Some(start) = est.start {
                p.initial = start;
            }
        }
    }

    Ok(params)
}

/// Run a quick pfilter at given params and return the loglik.
/// Used by scout for initial_loglik baseline.
pub fn run_quick_pfilter(config: &FitRunConfig, params: &[f64], n_particles: usize, seed: u64) -> f64 {
    run_quick_pfilter_full(config, params, n_particles, seed).0
}

/// Variant of `run_quick_pfilter` that also returns filter-health
/// statistics (mean / min ESS, the observation step where ESS is
/// worst, and a count of −∞ log-likelihood increments). Cheap to
/// compute since these are already in `PFilterResult.{ess_trace,
/// ll_increments}` — phase 2 of the fit-summary proposal just plumbs
/// them out instead of throwing them away.
///
/// On filter error returns `(NEG_INFINITY, FilterStats::failed())`.
pub fn run_quick_pfilter_full(
    config: &FitRunConfig,
    params: &[f64],
    n_particles: usize,
    seed: u64,
) -> (f64, super::loglik_eval::FilterStats) {
    let process = config.build_process();
    let obs_model = config.build_obs_model();
    let smc_config = sim::inference::traits::SMCConfig {
        n_particles,
        ..config.smc_config()
    };

    match sim::inference::bootstrap_filter(&process, &obs_model, params, &smc_config, seed) {
        Ok(result) => {
            let stats = super::loglik_eval::FilterStats::from_pfilter_result(
                &result.ess_trace, &result.ll_increments);
            (result.log_likelihood, stats)
        }
        Err(_) => (f64::NEG_INFINITY, super::loglik_eval::FilterStats::failed()),
    }
}

/// Print preflight transform report to stderr, pushing diagnostics to collector.
pub fn print_preflight(config: &FitRunConfig, collector: &DiagnosticCollector) {
    let n_auto = config.estimated_params.iter()
        .filter(|s| s.rw_sd_auto)
        .count();

    eprintln!("\ntransforms:");
    for spec in &config.estimated_params {
        let (tname, pos) = match &spec.transform {
            Transform::Log { lo, hi } => {
                let z = spec.initial.max(1e-300).ln();
                (format!("log     [{}, {}]", lo, hi), format!("log({:.4}) = {:.2}", spec.initial, z))
            }
            Transform::Logit { lo, hi } => {
                let p = ((spec.initial - lo) / (hi - lo)).clamp(1e-10, 1.0 - 1e-10);
                let z = (p / (1.0 - p)).ln();
                let compressed = z.abs() > 2.0;
                if compressed {
                    collector.push(DiagnosticKind::CompressedLogitPosition {
                        param: spec.name.clone(), z,
                    });
                }
                let mark = if compressed { " \x1b[33m⚠ compressed\x1b[0m" } else { "" };
                (format!("logit   [{}, {}]", lo, hi), format!("logit = {:.2}{}", z, mark))
            }
            Transform::None => {
                ("none".into(), format!("{:.4}", spec.initial))
            }
        };
        let source = if spec.rw_sd_auto { "\x1b[33mauto\x1b[0m" } else { "explicit" };
        let transformed_sd = spec.transformed_sd(spec.rw_sd, spec.initial);
        eprintln!("  {:12} {}  {}  rw_sd={:.4} ({:.3}/step, {})",
            spec.name, tname, pos, spec.rw_sd, transformed_sd, source);

        // Push auto rw_sd info diagnostic
        if spec.rw_sd_auto {
            collector.push(DiagnosticKind::AutoRwSd {
                param: spec.name.clone(), rw_sd: spec.rw_sd,
            });
        }
    }

    if n_auto > 0 {
        eprintln!("\n  \x1b[33m⚠ {}/{} parameters using auto rw_sd. Check traces and set explicit values.\x1b[0m",
            n_auto, config.estimated_params.len());
    }

    // Cooling schedule preview
    let frac = config.if2_config.cooling_fraction;
    let iters = config.if2_config.n_iterations;
    let target_iters = config.if2_config.cooling_target_iters;
    let n_obs = config.observations.len();
    let mid = iters / 2;

    let total_target_steps = target_iters as f64 * n_obs as f64;
    let per_step = frac.powf(2.0 / total_target_steps);
    let steps_per_iter = (1 + n_obs) as f64;

    let rw_at = |iter: usize| per_step.powf(iter as f64 * steps_per_iter);

    eprintln!("\ncooling: cf50={:.2} over {} iterations × {} observations", frac, iters, n_obs);
    eprintln!("  iter {:3}: rw_sd at {:.1}%", 1, rw_at(1) * 100.0);
    eprintln!("  iter {:3}: rw_sd at {:.1}% (halfway)", mid, rw_at(mid) * 100.0);
    eprintln!("  iter {:3}: rw_sd at {:.1}%", iters, rw_at(iters) * 100.0);

    // Warn if cooling exhausts well before the run ends
    let two_thirds = (iters * 2 / 3).max(1);
    let rw_at_two_thirds = rw_at(two_thirds);
    if rw_at_two_thirds < 0.01 {
        collector.push(DiagnosticKind::CoolingExhausted {
            exhausted_at_iter: two_thirds,
            total_iters: iters,
            rw_fraction_at_exhaustion: rw_at_two_thirds,
        });
    }
    eprintln!();
}

/// Derive the transform for a parameter from its IR metadata.
///
/// Priority: explicit override > param_kind > bounds fallback.
///
/// The param_kind field (populated by the OCaml compiler from the DSL type)
/// is the primary signal: probability → Logit, rate/positive/count → Log.
/// The bounds fallback (lo >= 0 → Log) exists for IR files predating
/// the param_kind field. The hi <= 1.0 probability-detector heuristic
/// was deliberately removed — it caused R0 on [1, 100] to get logit
/// instead of log, which is wrong.
pub fn derive_transform(
    ir_param: &ir::parameter::Parameter,
    transform_override: Option<&str>,
) -> Transform {
    let (lower, upper) = ir_param.bounds.unwrap_or((0.0, f64::INFINITY));
    if let Some(t) = transform_override {
        return match t {
            "log" => Transform::Log { lo: lower, hi: upper },
            "logit" => Transform::Logit { lo: lower, hi: upper },
            _ => Transform::None,
        };
    }
    if let Some(ref kind) = ir_param.param_kind {
        match kind.as_str() {
            "probability" => Transform::Logit { lo: lower, hi: upper },
            "rate" | "positive" | "count" => Transform::Log { lo: lower, hi: upper },
            _ => Transform::None,
        }
    } else {
        if lower >= 0.0 { Transform::Log { lo: lower, hi: upper } } else { Transform::None }
    }
}

// ── Shared IF2 parameter construction ────────────────────────────────────────

/// What the caller wants to estimate for one parameter.
///
/// Each CLI (if2, profile, fit) builds a Vec<ParamSpec> from its own
/// flags or config. The shared `build_if2_params_from_specs` turns
/// these into Vec<EstimatedParam> — the format the IF2 engine consumes.
///
/// Design: the caller decides WHAT to estimate (the partition).
/// The shared function decides HOW (transform, rw_sd, bounds).
/// This separation eliminates the DRY violations that caused
/// three bugs in one session (profile --rw-sd auto, profile missing
/// --fixed, transform derivation divergence).
pub struct ParamSpec {
    pub name: String,
    /// None = auto from bounds. Some(v) = explicit natural-scale rw_sd.
    pub rw_sd: Option<f64>,
    /// None = auto from param_kind. Some("log") = override.
    pub transform: Option<String>,
    pub ivp: bool,
}

/// Build EstimatedParam specs from caller-provided ParamSpecs.
/// Pure mechanical work: look up indices, derive transforms, compute auto rw_sd.
pub fn build_if2_params_from_specs(
    model: &ir::Model,
    compiled: &CompiledModel,
    base_params: &[f64],
    specs: &[ParamSpec],
) -> Result<Vec<EstimatedParam>, String> {
    let mut params = Vec::with_capacity(specs.len());

    for spec in specs {
        let ir_param = model.parameters.iter()
            .find(|p| p.name == spec.name)
            .ok_or_else(|| format!("parameter '{}' not in model", spec.name))?;
        let idx = *compiled.param_index.get(spec.name.as_str())
            .ok_or_else(|| format!("parameter '{}' not in compiled model", spec.name))?;

        let (lo, hi) = ir_param.bounds.unwrap_or((0.0, f64::INFINITY));

        // Transform: spec override > param_kind > fallback
        let transform = derive_transform(ir_param, spec.transform.as_deref());

        // rw_sd: spec explicit > auto from bounds
        let rw_sd = spec.rw_sd
            .unwrap_or_else(|| auto_rw_sd_from_value(base_params[idx], lo, hi, &transform));

        params.push(EstimatedParam {
            name: spec.name.clone(),
            index: idx,
            initial: base_params[idx],
            rw_sd,
            transform,
            lower: lo,
            upper: hi,
            ivp: spec.ivp,
            rw_sd_auto: spec.rw_sd.is_none(),
        });
    }

    Ok(params)
}

/// Auto-compute rw_sd from bounds on the transformed scale.
///
/// Returns a natural-scale rw_sd value. At each IF2 perturbation step,
/// `EstimatedParam::transformed_sd(natural_sd, current_value)` re-converts
/// this to the transformed scale using the delta method at the CURRENT
/// parameter value. So the midpoint used here is just a reference point
/// for expressing the natural-scale number — the actual perturbation
/// adapts to the current position through transformed_sd. Any reference
/// point (midpoint, lower bound, current value) would produce the same
/// perturbation on the transformed scale.
///
/// Log: log_range / 20 on transformed scale, converted to natural at geometric midpoint.
///   For sigma_se in [0.001, 5.0]: log_range = 8.5, log_sd = 0.43, meaning ~±50% per step.
/// Logit: range / 6 on natural scale. Logit range is ~12 (-6 to 6), /6 gives ~2.0 on logit.
/// Identity: (hi - lo) / 6.
///
/// The /20 vs /6 asymmetry: log is unbounded (perturbations accumulate) while logit
/// saturates at bounds. Log needs more conservative defaults.
///
/// This is a starting heuristic, not a solution. Scout's MAD-based
/// calibration replaces it for refine. The modeler can override with
/// explicit rw_sd in fit.toml or --rw-sd on the CLI.
pub fn auto_rw_sd_from_value(_current_value: f64, lower: f64, upper: f64, transform: &Transform) -> f64 {
    match transform {
        Transform::Log { lo, hi } => {
            let lo = lo.max(1e-300);
            let hi_val = if hi.is_finite() { *hi } else { lo * 1000.0 };
            let log_range = (hi_val / lo).ln();
            let log_sd = log_range / 20.0;
            // Convert to natural scale at geometric midpoint
            let midpoint = (lo * hi_val).sqrt();
            midpoint * log_sd
        }
        Transform::Logit { lo, hi } => {
            (hi - lo) / 6.0
        }
        Transform::None => {
            let lo = if lower.is_finite() { lower } else { -1e6 };
            let hi = if upper.is_finite() { upper } else { 1e6 };
            (hi - lo) / 6.0
        }
    }
}

/// Load observations from TSV, validating time alignment with dt.
fn load_observations(path: &str, column: &str, dt: f64) -> Result<Vec<Observation>, String> {
    let observations = crate::pfilter::load_data_tsv_column(path, column)?;
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


/// Run one IF2 chain (called from thread::scope).
fn run_one_chain(
    chain_id: usize,
    config: &FitRunConfig,
    per_chain_params: Option<&[EstimatedParam]>,
    pb: Option<&ProgressBar>,
) -> IF2Result {
    let chain_seed = crate::util::derive_chain_seed(config.seed, chain_id);
    let if2_params = per_chain_params.unwrap_or(&config.estimated_params);

    let process = config.build_process();
    let obs_model = config.build_obs_model();

    let n_iter = config.if2_config.n_iterations;
    let plain = crate::progress::is_plain();
    // RefCell so the progress closure can be `Fn` (run_if2_with_progress
    // takes `&dyn Fn`). The callback is single-threaded per chain.
    // Cadence from progress::DEFAULT_THROTTLE.
    let throttle = std::cell::RefCell::new(crate::progress::Throttle::default());

    let progress_cb = |iter: usize, loglik: f64| {
        if let Some(bar) = pb {
            bar.set_position((iter + 1) as u64);
            if loglik.is_finite() {
                bar.set_message(format!("ll={:.1}", loglik));
            } else {
                bar.set_message("ll=-inf".to_string());
            }
        }
        if plain && throttle.borrow_mut().ready() {
            if loglik.is_finite() {
                log::info!("fit chain {} iter {}/{} ll={:.1}",
                    chain_id + 1, iter + 1, n_iter, loglik);
            } else {
                log::info!("fit chain {} iter {}/{} ll=-inf",
                    chain_id + 1, iter + 1, n_iter);
            }
        }
    };

    let result = run_if2_with_progress(
        &process, &obs_model, &config.base_params, if2_params,
        &config.if2_config, chain_seed,
        Some(&progress_cb),
    ).unwrap_or_else(|e| {
        eprintln!("chain {} error: {:?}", chain_id + 1, e);
        std::process::exit(1);
    });

    if let Some(bar) = pb {
        bar.finish_with_message(format!("ll={:.1}", result.final_loglik));
    }
    if plain {
        log::info!("fit chain {} done iter {}/{} final_ll={:.1}",
            chain_id + 1, n_iter, n_iter, result.final_loglik);
    }

    result
}

/// Run N chains with optional per-chain EstimatedParam overrides (for scout random starts).
pub fn run_chains_with_per_chain_params(
    config: &FitRunConfig,
    per_chain_params: Option<&[Vec<EstimatedParam>]>,
    collector: &DiagnosticCollector,
) -> ChainResults {
    eprintln!("running {} chains × {} particles × {} iterations, cooling={}, dt={}",
        config.n_chains, config.if2_config.n_particles, config.if2_config.n_iterations,
        config.if2_config.cooling_fraction, config.if2_config.dt);

    // GH #14: draw target reflects --progress mode. In plain/none modes the
    // bars are hidden; per-chain log::info! lines from the progress callback
    // carry the state for tee/ssh/CI consumers.
    let mp = MultiProgress::with_draw_target(crate::progress::draw_target());
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

    // Preflight transform report
    print_preflight(config, collector);

    let results: Vec<(usize, IF2Result)> = (0..config.n_chains)
        .into_par_iter()
        .map(|chain_id| {
            let per_chain = per_chain_params.map(|pcp| &pcp[chain_id][..]);
            let result = run_one_chain(chain_id, config, per_chain, Some(&bars[chain_id]));
            (chain_id, result)
        })
        .collect();

    // Evaluate true (unperturbed) loglik at selected iterations for ALL chains.
    // Every 10 iterations, run a clean PF at the filter mean params.
    let eval_interval = 10;
    let mut results = results;
    {
        let n_eval_particles = config.if2_config.n_particles.min(500); // cap at 500 for speed
        eprintln!("\nevaluating loglik (every {} iterations, all {} chains)...",
            eval_interval, results.len());

        for (chain_id, result) in results.iter_mut() {
            for it in &mut result.iterations {
                if it.iteration % eval_interval == 0 || it.iteration == config.if2_config.n_iterations - 1 {
                    it.loglik = run_quick_pfilter(
                        config, &it.param_means,
                        n_eval_particles,
                        config.seed + *chain_id as u64 * 1000 + it.iteration as u64,
                    );
                }
            }
            // Overwrite final_loglik with the true loglik
            let true_ll = result.iterations.last()
                .map(|it| it.loglik).unwrap_or(f64::NEG_INFINITY);
            result.final_loglik = true_ll;
            eprint!("\r  chain {}: ll={:.1}    ", *chain_id + 1, true_ll);
        }
        eprintln!();
    }

    // Compute chain agreement (Â) on the per-iteration param-mean
    // trajectories — independent of clean-eval scoring.
    let chain_agreement = compute_chain_agreement(&results, &config.estimated_params, config.if2_config.n_iterations);

    // Step 6 (proposal §Proposal 1): clean-eval re-scoring at high
    // particle count and M replicates. The winner is the argmax over
    // logmeanexp-combined logliks across chains' IF2 final-iteration
    // means, matching pomp's coef(mif2_out) + pfilter convention.
    // Replaces the prior `argmax over result.final_loglik`, which was
    // driven by 500-particle in-run PF noise and exhibited a ~40-nat
    // extraction bias on production runs. The in-run trace above is
    // preserved for diagnostics (Unit B territory).
    eprintln!("\nloglik-eval: re-scoring final-iter θ̂ ({} chains × {} replicates @ {} particles)...",
        results.len(), config.loglik_eval.n_replicates, config.loglik_eval.n_particles);
    let loglik_eval_outcome = loglik_eval::run_loglik_eval(
        config, &results, &config.loglik_eval, config.seed,
    ).unwrap_or_else(|e| {
        eprintln!("error: loglik-eval failed: {}", e);
        std::process::exit(1);
    });

    let (best_chain, best_loglik, best_se) =
        select_winner_summary(&loglik_eval_outcome);

    // Report. `best_se` is derived locally; we log it here but don't
    // store it on `ChainResults` — readers that need it go to
    // `loglik_eval.per_chain[overall_winner_idx]`.
    eprintln!("\nbest chain: {} (loglik={:.2} ± {:.2})",
        best_chain + 1, best_loglik, best_se);
    if config.n_chains > 1 {
        let logliks: Vec<f64> = loglik_eval_outcome.per_chain.iter()
            .map(|s| s.loglik).collect();
        eprintln!("chain clean logliks: [{}]",
            logliks.iter().map(|l| format!("{:.1}", l)).collect::<Vec<_>>().join(", "));
    }

    // Report Â (chain agreement) with diagnostic warnings
    if config.n_chains > 1 {
        let max_chain_agreement = chain_agreement.values().cloned().fold(0.0_f64, f64::max);
        let logliks: Vec<f64> = results.iter().map(|(_, r)| r.final_loglik).collect();
        let ll_spread = logliks.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - logliks.iter().cloned().fold(f64::INFINITY, f64::min);

        // Â = chain agreement (renamed from Rhat: this is not a posterior
        // mixing statistic; it measures IF2 optimizer chain agreement).
        eprintln!("\nÂ:");
        for spec in &config.estimated_params {
            if let Some(&r) = chain_agreement.get(&spec.name) {
                let status = if r < 1.1 { "\x1b[32m✓\x1b[0m" } else if r < 1.5 { "\x1b[33m~\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
                eprintln!("  {:12} Â={:.3} {}", spec.name, r, status);
            }
        }

        // Diagnostic: high Â + large loglik spread → chains in different basins
        if max_chain_agreement > 1.5 && ll_spread > 50.0 {
            collector.push(DiagnosticKind::MultimodalLikelihood { ll_spread, max_chain_agreement });
        } else if max_chain_agreement > 1.1 {
            let n_unconverged = chain_agreement.values().filter(|&&r| r > 1.1).count();
            let n_total = chain_agreement.len();
            collector.push(DiagnosticKind::ConvergenceIncomplete { max_chain_agreement, n_unconverged, n_total });
        }
    }

    ChainResults {
        results,
        best_chain,
        best_loglik,
        chain_agreement,
        loglik_eval: loglik_eval_outcome,
    }
}

impl ChainResults {
    /// Per-chain clean-eval log-likelihoods in chain-id order. Used by
    /// scout/refine/validate to populate `FitState.chain_eval_logliks`
    /// for the compound scout-convergence gate.
    pub fn chain_eval_logliks(&self) -> Vec<f64> {
        let mut v: Vec<(usize, f64)> = self.loglik_eval.per_chain.iter()
            .map(|s| (s.chain_id, s.loglik)).collect();
        v.sort_by_key(|(id, _)| *id);
        v.into_iter().map(|(_, ll)| ll).collect()
    }

    /// Per-chain clean-eval standard errors in chain-id order, parallel
    /// to `chain_eval_logliks`.
    pub fn chain_eval_ses(&self) -> Vec<f64> {
        let mut v: Vec<(usize, f64)> = self.loglik_eval.per_chain.iter()
            .map(|s| (s.chain_id, s.se)).collect();
        v.sort_by_key(|(id, _)| *id);
        v.into_iter().map(|(_, se)| se).collect()
    }

    /// Estimated-param θ̂ of the overall clean-eval winner. Indexed by
    /// `EstimatedParam::index`, parallel to `IF2Result.mle`.
    ///
    /// **Use this — not `IF2Result.mle` of the winning chain — anywhere
    /// the user-facing "MLE" parameters are needed** (e.g. building
    /// `start_values` for a downstream stage, writing
    /// `mle_params.toml`, status / summary tables). `IF2Result.mle` is
    /// the IF2 chain's argmax over its own noisy `if2_perturbed_loglik`
    /// — a separate, biased estimator. The clean-eval θ̂ is the
    /// chain's IF2 final-iteration mean (Ionides et al. 2015's
    /// theoretical estimator), unchanged by clean re-evaluation; what
    /// the clean re-eval changes is the *loglik* attached to that θ̂,
    /// and the cross-chain selection of which chain's θ̂ to report.
    pub fn winner_theta(&self) -> &[f64] {
        &self.loglik_eval.per_chain[self.loglik_eval.overall_winner_idx].theta
    }

}

/// Pure helper: extract the (chain_id, ll, se) summary from a
/// `LoglikEvalOutcome`. Factored out so the wiring change in
/// `run_chains_with_per_chain_params` is unit-testable without paying
/// for a real IF2 + PF run. Tested in `tests::winner_summary_*`.
fn select_winner_summary(
    outcome: &loglik_eval::LoglikEvalOutcome,
) -> (usize, f64, f64) {
    let s = &outcome.per_chain[outcome.overall_winner_idx];
    (s.chain_id, s.loglik, s.se)
}

/// Compute chain agreement (Â) across IF2 chains (last half of
/// iterations). The underlying formula is Gelman-Rubin 1992 R̂; the
/// renamed output label reflects that this is applied to IF2
/// optimizer chains, not posterior samples. See
/// docs/dev/proposals/2026-04-24-if2-scout-findings-remediation.md.
///
/// See [`gelman_rubin_1992`] for the split-chain / rank-norm
/// caveat.
pub fn compute_chain_agreement(
    results: &[(usize, IF2Result)],
    if2_params: &[EstimatedParam],
    n_iterations: usize,
) -> HashMap<String, f64> {
    let n_chains = results.len();
    if n_chains < 2 { return HashMap::new(); }

    // Im25 in 2026-04-19 inference review batch 3: use each chain's
    // own last-half rather than `n_iterations` uniformly. Resumed
    // chains have `iterations.len() > n_iterations`; the old formula
    // was `skip(n_iterations − n_tail)` for all chains — so a
    // resumed chain's "last half" started at an absolute iteration
    // index that didn't correspond to the physical last half of its
    // trace. Now each chain defines its own last-half window.
    let mut agreement_map = HashMap::new();

    let chain_tail = |r: &IF2Result, spec: &EstimatedParam| -> Vec<f64> {
        let len = r.iterations.len().max(n_iterations);
        let n_tail = (len / 2).max(1);
        r.iterations.iter()
            .skip(r.iterations.len().saturating_sub(n_tail))
            .map(|it| it.param_means[spec.index])
            .collect()
    };

    for spec in if2_params {
        let chain_means: Vec<f64> = results.iter().map(|(_, r)| {
            let tail = chain_tail(r, spec);
            tail.iter().sum::<f64>() / tail.len() as f64
        }).collect();

        let chain_vars: Vec<f64> = results.iter().map(|(_, r)| {
            let tail = chain_tail(r, spec);
            let m = tail.iter().sum::<f64>() / tail.len() as f64;
            tail.iter().map(|&x| (x - m).powi(2)).sum::<f64>() / (tail.len() - 1).max(1) as f64
        }).collect();

        // For the G-R between/within formula, use the tail length of
        // the shortest chain — the formula uses a single N per chain
        // and conservatism argues for the min when lengths differ.
        let min_tail = results.iter()
            .map(|(_, r)| chain_tail(r, spec).len())
            .min().unwrap_or(0).max(1) as f64;
        let grand_mean = chain_means.iter().sum::<f64>() / n_chains as f64;
        let between = chain_means.iter().map(|&m| (m - grand_mean).powi(2)).sum::<f64>()
            * min_tail / (n_chains - 1).max(1) as f64;
        let within = chain_vars.iter().sum::<f64>() / n_chains as f64;
        let agreement = if within > 0.0 {
            (((min_tail - 1.0) / min_tail * within + between / min_tail) / within).sqrt()
        } else { f64::NAN };

        agreement_map.insert(spec.name.clone(), agreement);
    }

    agreement_map
}

/// Compute R-hat and ESS from per-chain parameter traces.
/// `chains[chain_id]` is a Vec of param values (one per sample).
/// Returns `(rhat, ess)`. R-hat requires >= 2 chains with >= 4
/// samples each, all chains equal length (Im24 in the 2026-04-19
/// inference review); returns `(NaN, NaN)` when not met.
///
/// IM12 in the same review: total ESS is only meaningful when R-hat
/// indicates convergence. If R-hat > 1.1, chains are effectively
/// sampling different distributions and summing their per-chain
/// ESS estimates is not interpretable — return `NaN` for ESS so
/// the caller doesn't display a misleading "total ESS" number that
/// makes a non-converged run look adequately sampled.
pub fn compute_rhat_ess(chains: &[Vec<f64>]) -> (f64, f64) {
    use sim::inference::pmmh::mcmc_ess;

    let n_chains = chains.len();
    // Im24: enforce equal chain lengths (the between-chain variance
    // formula below uses `chains[0].len()` as the sample count;
    // with unequal lengths it becomes biased).
    let equal_lengths = chains.first()
        .map(|first| chains.iter().all(|c| c.len() == first.len()))
        .unwrap_or(false);

    if n_chains < 2 || !equal_lengths || !chains.iter().all(|c| c.len() >= 4) {
        return (f64::NAN, f64::NAN);
    }

    let chain_means: Vec<f64> = chains.iter().map(|c| {
        c.iter().sum::<f64>() / c.len() as f64
    }).collect();
    let chain_vars: Vec<f64> = chains.iter().map(|c| {
        let m = c.iter().sum::<f64>() / c.len() as f64;
        c.iter().map(|&x| (x - m).powi(2)).sum::<f64>() / (c.len() - 1).max(1) as f64
    }).collect();

    let n_samples = chains[0].len() as f64;
    let grand_mean = chain_means.iter().sum::<f64>() / n_chains as f64;
    let between = chain_means.iter().map(|&m| (m - grand_mean).powi(2)).sum::<f64>()
        * n_samples / (n_chains - 1).max(1) as f64;
    let within = chain_vars.iter().sum::<f64>() / n_chains as f64;
    let rhat = if within > 0.0 {
        (((n_samples - 1.0) / n_samples * within + between / n_samples) / within).sqrt()
    } else { f64::NAN };

    // IM12: gate ESS on R-hat. 1.1 is the standard threshold (BDA3).
    const RHAT_THRESHOLD: f64 = 1.1;
    let total_ess = if rhat.is_finite() && rhat <= RHAT_THRESHOLD {
        chains.iter().map(|c| mcmc_ess(c)).sum()
    } else {
        f64::NAN
    };

    (rhat, total_ess)
}

/// MAD-based auto rw_sd calibration from chain best-loglik parameters.
///
/// Returns (rw_sd map, n_good_chains) or error if no consensus.
pub fn auto_rw_sd(
    results: &[(usize, IF2Result)],
    if2_params: &[EstimatedParam],
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
        // Filter non-finite values: chains with extreme parameter perturbations
        // can produce NaN (from -inf loglik propagation) or inf. These are dead
        // chains — they contributed nothing to inference. Including them in the
        // MAD would either panic (NaN in sort) or corrupt the scale estimate
        // (inf inflating the deviation).
        let mut values: Vec<f64> = chain_params.iter()
            .map(|p| p[spec.index])
            .filter(|v| v.is_finite())
            .collect();
        if values.len() < 2 {
            medians.push(0.0);
            mads.push(0.0);
            continue;
        }

        let med = median(&mut values);
        let m = mad(&values, med);

        medians.push(med);
        mads.push(m);
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
        let _diverged: Vec<usize> = good_chains.iter().enumerate()
            .filter(|(_, &g)| !g).map(|(i, _)| i + 1).collect();
        eprintln!("warning: {}/{} chains diverged ({:?}), excluded from rw_sd calibration",
            n_chains - n_good, n_chains, _diverged);
    }

    // rw_sd = 0.5 × MAD of good chains
    let mut rw_sd_map = HashMap::new();
    for (pi, spec) in if2_params.iter().enumerate() {
        let good_values: Vec<f64> = (0..n_chains)
            .filter(|&c| good_chains[c])
            .map(|c| chain_params[c][spec.index])
            .collect();

        let good_mad = mad(&good_values, medians[pi]);

        let rw = 0.5 * good_mad;
        // Floor: don't let rw_sd go below 1% of the median (prevents convergence stall)
        let floor = medians[pi].abs() * 0.01;
        rw_sd_map.insert(spec.name.clone(), rw.max(floor));
    }

    Ok((rw_sd_map, n_good))
}

/// Median of a mutable slice (sorts in place).
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 { return 0.0; }
    if n.is_multiple_of(2) {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    } else {
        v[n / 2]
    }
}

/// Median absolute deviation from a given center.
fn mad(v: &[f64], center: f64) -> f64 {
    let mut abs_devs: Vec<f64> = v.iter().map(|&x| (x - center).abs()).collect();
    median(&mut abs_devs)
}

/// Write per-chain output files: `parameter_traces.tsv` and
/// `final_params.toml` under `<dir>/chain_<N>/`.
///
/// When `loglik_eval` is `Some`, each chain's `final_params.toml` also
/// records the clean-eval winning candidate label and SE for that chain
/// (Step 7, proposal §Proposal 1). PMMH and other consumers that don't
/// run clean-eval pass `None`. The winning θ̂ written into the TOML is
/// also taken from the clean-eval per-chain winner when present (it can
/// be the tail mean or best-in-run iter, not just `result.mle`); this
/// is what makes scout→refine handoff consume the de-biased estimate.
pub fn write_chain_outputs(
    dir: &str,
    results: &[(usize, IF2Result)],
    if2_params: &[EstimatedParam],
    all_param_names: &[String],
    base_params: &[f64],
    compiled: &CompiledModel,
    loglik_eval: Option<&loglik_eval::LoglikEvalOutcome>,
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
        writeln!(f, "# {}", crate::version::VERSION).unwrap();
        write!(f, "iteration\tloglik\tif2_perturbed_loglik").unwrap();
        for spec in if2_params { write!(f, "\t{}", spec.name).unwrap(); }
        writeln!(f).unwrap();
        for it in &result.iterations {
            let loglik_str = if it.loglik.is_finite() { format!("{:.2}", it.loglik) } else { "NA".into() };
            write!(f, "{}\t{}\t{:.2}", it.iteration, loglik_str, it.if2_perturbed_loglik).unwrap();
            for spec in if2_params { write!(f, "\t{:.6}", it.param_means[spec.index]).unwrap(); }
            writeln!(f).unwrap();
        }

        // Resolve this chain's clean-eval score (if any). Falls back to
        // `result.mle` when no loglik_eval was run (PMMH path). Note: the
        // chain's θ̂ is the IF2 final-iteration mean either way; what
        // clean-eval changes is the *loglik* attached to that θ̂.
        let chain_score = loglik_eval.and_then(|ce|
            ce.per_chain.iter().find(|s| s.chain_id == *chain_id));

        // Final params TOML (all params, not just estimated).
        let toml_path = format!("{}/final_params.toml", chain_dir);
        let mut f = std::fs::File::create(&toml_path)
            .map_err(|e| format!("cannot write {}: {}", toml_path, e))?;
        writeln!(f, "# {}", crate::version::VERSION).unwrap();
        writeln!(f, "# Chain {} final parameters", chain_id + 1).unwrap();
        let header_ll = chain_score.map(|s| s.loglik).unwrap_or(result.final_loglik);
        if let Some(s) = chain_score {
            writeln!(f, "# loglik = {:.2} ± {:.2} (clean-eval re-score of IF2 final-iter mean)",
                header_ll, s.se).unwrap();
        } else {
            writeln!(f, "# loglik = {:.2}", header_ll).unwrap();
        }
        writeln!(f).unwrap();
        // Param key/value pairs at the top level so the file is loadable
        // via the standard params loader (`camdl pfilter --params …`,
        // `simulate --params`). Clean-eval metadata lives in a
        // `[provenance]` table at the bottom — keeping it within the
        // file but out of the flat-key namespace (the params loader
        // rejects non-numeric top-level keys; see GH #17).
        for name in all_param_names {
            let value = if let Some(spec) = if2_params.iter().find(|p| p.name == *name) {
                // Prefer clean-eval score's θ for estimated params.
                // (Equal to result.mle's per-spec entry under FinalIter
                // semantics, but kept routed through clean_eval for
                // consistency with the run-root final_params.toml.)
                chain_score
                    .map(|s| s.theta[spec.index])
                    .unwrap_or_else(|| result.mle[spec.index])
            } else if let Some(&idx) = compiled.param_index.get(name.as_str()) {
                base_params[idx]
            } else {
                0.0
            };
            writeln!(f, "{} = {}", name, format_param_value(value)).unwrap();
        }
        if let Some(s) = chain_score {
            writeln!(f).unwrap();
            writeln!(f, "[provenance]").unwrap();
            writeln!(f, "loglik = {:.6}", s.loglik).unwrap();
            writeln!(f, "se = {:.6}", s.se).unwrap();
            writeln!(f, "chain = {}", chain_id + 1).unwrap();
        }
    }
    Ok(())
}

/// Write `<dir>/chain_evaluations.tsv` — the per-chain clean-eval
/// score table. Schema:
/// `chain\tloglik\tse\tess_mean\tess_min\tess_min_step\tn_neg_inf_incr\t<param_1>\t<param_2>\t…`
/// with one header line + N data rows (one per chain) in chain-id
/// order. Each row reports the chain's IF2 final-iteration θ̂
/// re-scored with M high-particle PF replicates and combined via the
/// configured `combine` mode (logmeanexp by default).
///
/// `ess_min_step` is `-1` when no observations were scored (filter
/// failed); `n_neg_inf_incr` counts steps where the filter completely
/// lost the data.
///
/// Used downstream by `camdl fit summary`, the gate's per-chain SE
/// consumption, and book vignettes that report ESS-at-θ̂ diagnostics.
pub fn write_clean_eval_tsv(
    dir: &str,
    outcome: &loglik_eval::LoglikEvalOutcome,
    if2_params: &[EstimatedParam],
) -> Result<(), String> {
    use std::io::Write;
    let path = format!("{}/chain_evaluations.tsv", dir);
    let mut f = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {}", path, e))?;
    writeln!(f, "# {}", crate::version::VERSION).unwrap();
    write!(f, "chain\tloglik\tse\tess_mean\tess_min\tess_min_step\tn_neg_inf_incr").unwrap();
    for spec in if2_params { write!(f, "\t{}", spec.name).unwrap(); }
    writeln!(f).unwrap();
    for s in &outcome.per_chain {
        let ess_min_step_str = s.filter_stats.ess_min_step
            .map(|i| i.to_string()).unwrap_or_else(|| "-1".into());
        write!(f, "{}\t{:.6}\t{:.6}\t{:.2}\t{:.2}\t{}\t{}",
            s.chain_id + 1, s.loglik, s.se,
            s.filter_stats.ess_mean, s.filter_stats.ess_min,
            ess_min_step_str, s.filter_stats.n_neg_inf_increments).unwrap();
        for spec in if2_params {
            write!(f, "\t{:.6}", s.theta[spec.index]).unwrap();
        }
        writeln!(f).unwrap();
    }
    Ok(())
}

/// Write `<dir>/final_params.toml` at the stage root, capturing the
/// overall clean-eval winner across all chains. Mirrors the per-chain
/// TOML schema but identifies which chain produced it.
pub fn write_run_root_final_params(
    dir: &str,
    outcome: &loglik_eval::LoglikEvalOutcome,
    if2_params: &[EstimatedParam],
    all_param_names: &[String],
    base_params: &[f64],
    compiled: &CompiledModel,
) -> Result<(), String> {
    use std::io::Write;
    let s = &outcome.per_chain[outcome.overall_winner_idx];
    let path = format!("{}/final_params.toml", dir);
    let mut f = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {}", path, e))?;
    writeln!(f, "# {}", crate::version::VERSION).unwrap();
    writeln!(f, "# winner: chain={}", s.chain_id + 1).unwrap();
    writeln!(f, "# loglik = {:.2} ± {:.2} (clean-eval re-score of IF2 final-iter mean)",
        s.loglik, s.se).unwrap();
    writeln!(f).unwrap();
    // Top-level keys are parameters only — keeps the file loadable via
    // the standard params loader. Clean-eval metadata lives in the
    // `[provenance]` table at the bottom. See GH #17.
    for name in all_param_names {
        let value = if let Some(spec) = if2_params.iter().find(|p| p.name == *name) {
            s.theta[spec.index]
        } else if let Some(&idx) = compiled.param_index.get(name.as_str()) {
            base_params[idx]
        } else {
            0.0
        };
        writeln!(f, "{} = {}", name, format_param_value(value)).unwrap();
    }
    writeln!(f).unwrap();
    writeln!(f, "[provenance]").unwrap();
    writeln!(f, "loglik = {:.6}", s.loglik).unwrap();
    writeln!(f, "se = {:.6}", s.se).unwrap();
    writeln!(f, "chain = {}", s.chain_id + 1).unwrap();
    Ok(())
}

/// Build per-chain random starts for an IF2 dispatch. Each chain gets
/// its own draw over the declared bounds so Â across chains reflects
/// genuine independence-of-starts, not just per-chain RNG noise on a
/// shared initial point.
///
/// Mirrors the v1 scout policy: chain 0 keeps the seeded start (from
/// `config.estimated_params[i].initial` — i.e. `[estimate].*.start` or
/// `base_params[idx]`) so the first chain is reproducible given the
/// fit config; chains 1..N draw uniformly from bounds (or jitter ±50 %
/// of the seeded start when unbounded).
///
/// `seed` is the fit's top-level seed. Per-chain RNGs mix in the chain
/// id so adding chains doesn't disturb existing chains' starts.
pub fn build_random_chain_starts(
    config: &FitRunConfig,
    seed: u64,
    n_chains: usize,
) -> Vec<Vec<EstimatedParam>> {
    (0..n_chains).map(|chain_id| {
        let mut rng = StatefulRng::new(
            crate::util::derive_chain_seed(seed, chain_id));
        config.estimated_params.iter().map(|spec| {
            let initial = if chain_id == 0 {
                // Chain 0 keeps the seeded start for reproducibility.
                spec.initial
            } else if spec.lower.is_finite() && spec.upper.is_finite() {
                spec.lower + rng.uniform() * (spec.upper - spec.lower)
            } else {
                // Unbounded: jitter ±50 % from the seeded start.
                spec.initial * (0.5 + rng.uniform())
            };
            EstimatedParam { initial, ..spec.clone() }
        }).collect()
    }).collect()
}

/// Write `chain_starts.tsv` at the stage root — one row per chain
/// with the pre-filter starting values of every estimated parameter.
///
/// Diagnostic use: "did the random starts span the declared bounds?"
/// and "did all chains collapse to the same basin in one filter
/// pass?" — both questions that `parameter_traces.tsv` can't answer
/// because iteration 0 there is post-first-filter (already perturbed).
/// See the header in `chain_{N}/parameter_traces.tsv`.
///
/// `per_chain_params` is the same slice that `run_one_chain` receives:
/// `Some(&[Vec<EstimatedParam>])` when scout supplies per-chain random
/// starts, `None` when every chain starts from `config.estimated_params`.
pub fn write_chain_starts(
    dir: &str,
    per_chain_params: Option<&[Vec<EstimatedParam>]>,
    fallback: &[EstimatedParam],
    n_chains: usize,
) -> Result<(), String> {
    use std::io::Write;
    let path = format!("{}/chain_starts.tsv", dir);
    let mut f = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {}", path, e))?;
    writeln!(f, "# {}", crate::version::VERSION).unwrap();
    writeln!(f, "# pre-filter starting values per chain (before any IF2 perturbation).").unwrap();
    writeln!(f, "# pairs row-by-row with chain_{{chain}}/parameter_traces.tsv iter-0 rows").unwrap();
    writeln!(f, "# to visualise how far chains moved on the first filter pass.").unwrap();
    write!(f, "chain").unwrap();
    for spec in fallback { write!(f, "\t{}", spec.name).unwrap(); }
    writeln!(f).unwrap();

    for chain_id in 0..n_chains {
        let specs: &[EstimatedParam] = match per_chain_params {
            Some(pcp) => &pcp[chain_id],
            None      => fallback,
        };
        write!(f, "{}", chain_id + 1).unwrap();
        for spec in specs {
            write!(f, "\t{}", format_param_value(spec.initial)).unwrap();
        }
        writeln!(f).unwrap();
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
    writeln!(f, "# {}", crate::version::VERSION).unwrap();
    writeln!(f, "chain\titeration\tloglik\tif2_perturbed_loglik").unwrap();
    for (chain_id, result) in results {
        for it in &result.iterations {
            let loglik_str = if it.loglik.is_finite() { format!("{:.2}", it.loglik) } else { "NA".into() };
            writeln!(f, "{}\t{}\t{}\t{:.2}", chain_id + 1, it.iteration, loglik_str, it.if2_perturbed_loglik).unwrap();
        }
    }
    Ok(())
}

/// Collect ALL parameter values (estimated + fixed) for MLE output.
pub fn collect_all_params(
    mle: &[f64],
    if2_params: &[EstimatedParam],
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

/// Resolve the prior for a parameter using the precedence chain:
///
///   1. fit.toml `[estimate.<name>.prior]` (typed `ir::PriorDist`;
///      override for sensitivity analysis)
///   2. model IR parameter.prior (from `~` syntax in .camdl)
///   3. Prior::Flat (improper uniform, default for inference)
///
/// Returns the prior and a string describing the source (for logging).
pub fn resolve_prior(
    name: &str,
    estimate: &indexmap::IndexMap<String, super::config_v2::EstimateSpecV2>,
    model: &ir::Model,
) -> (Prior, &'static str) {
    // 1. fit.toml override.
    if let Some(est) = estimate.get(name) {
        if let Some(ref pd) = est.prior {
            return (Prior::from_ir(pd), "fit.toml");
        }
    }
    // 2. model IR
    if let Some(ir_param) = model.parameters.iter().find(|p| p.name == name) {
        if let Some(ref pd) = ir_param.prior {
            return (Prior::from_ir(pd), "model");
        }
        // Hierarchical priors carry expression-valued args; wrap them
        // verbatim — evaluation at each MCMC step resolves references
        // against current hyperparameter values. Wave 2 / #3 Gate 3a.
        if let Some(ref hp) = ir_param.hierarchical {
            return (Prior::Hierarchical(hp.clone()), "model (hierarchical)");
        }
    }
    // 3. fallback
    (Prior::Flat, "flat (default)")
}

/// IC4 in the 2026-04-19 inference review batch 3: validate that
/// each estimated parameter's resolved prior is compatible with its
/// transform. Wrong combinations silently produce a different prior
/// than the user wrote (log_normal on Transform::None collapses to
/// Normal-on-natural; log_normal on Transform::Logit becomes
/// logit-normal; etc.).
///
/// Compatibility matrix:
///   Prior::TransformedNormal (log_normal) — Transform::Log
///   Prior::Beta                           — Transform::Logit
///   Prior::HalfNormal, Gamma, Exponential — Transform::Log
///   Prior::Uniform, Normal, Flat          — any transform
///
/// Call from every fit-stage entry point *before* building IF2
/// params so the user sees a clean error, not a miscalibrated
/// posterior.
pub fn validate_prior_transform_compat(
    estimate: &indexmap::IndexMap<String, super::config_v2::EstimateSpecV2>,
    model: &ir::Model,
) -> Result<(), String> {
    for name in estimate.keys() {
        // Build the same Transform the engine will use.
        let ir_param = match model.parameters.iter().find(|p| p.name == *name) {
            Some(p) => p,
            None => continue, // validate_partition catches unknown params.
        };
        let transform_override = estimate.get(name)
            .and_then(|e| e.transform.as_ref())
            .map(|t| t.as_str());
        let transform = derive_transform(ir_param, transform_override);
        let (prior, source) = resolve_prior(name, estimate, model);

        let is_log   = matches!(transform, Transform::Log { .. });
        let is_logit = matches!(transform, Transform::Logit { .. });

        let prior_name = match &prior {
            Prior::TransformedNormal { .. } => "log_normal",
            Prior::Beta { .. }              => "beta",
            Prior::HalfNormal { .. }        => "half_normal",
            Prior::Gamma { .. }             => "gamma",
            Prior::Exponential { .. }       => "exponential",
            Prior::Normal { .. }            => "normal",
            Prior::Uniform { .. }           => "uniform",
            Prior::Flat                     => "flat",
            Prior::Hierarchical(h)          => h.kind.as_str(),
        };
        let transform_name = match &transform {
            Transform::Log { .. }   => "Log",
            Transform::Logit { .. } => "Logit",
            Transform::None         => "None",
        };
        let support_desc = match &prior {
            Prior::TransformedNormal { .. } => "log_normal",
            Prior::Beta { .. }              => "beta",
            _                               => "positive-support",
        };
        let err = |needs: &str| Err(format!(
            "parameter '{}': prior {} is incompatible with transform {}; \
             {} priors require a {} transform. Either fix the param_kind \
             in the model (or the `transform` override in fit.toml), or \
             pick a different prior.\n  (prior source: {})",
            name, prior_name, transform_name, support_desc, needs, source,
        ));

        match prior {
            Prior::TransformedNormal { .. }
            | Prior::HalfNormal { .. }
            | Prior::Gamma { .. }
            | Prior::Exponential { .. } => {
                if !is_log { return err("Log"); }
            }
            Prior::Beta { .. } => {
                if !is_logit { return err("Logit"); }
                // Beta is on [0, 1]; require logit bounds span that.
                if let Transform::Logit { lo, hi } = transform {
                    if lo != 0.0 || hi != 1.0 {
                        return Err(format!(
                            "parameter '{}': beta prior requires bounds [0, 1], \
                             got [{}, {}].", name, lo, hi));
                    }
                }
            }
            Prior::Uniform { .. } | Prior::Normal { .. } | Prior::Flat => {
                // Compatible with any transform.
            }
            // Hierarchical priors carry the same kind as their plain
            // counterpart. Reuse the same transform compatibility rules.
            // Wave 2 / #3 Gate 3a.
            Prior::Hierarchical(ref h) => match h.kind {
                HierarchicalKind::LogNormal
                | HierarchicalKind::HalfNormal
                | HierarchicalKind::Gamma
                | HierarchicalKind::Exponential => {
                    if !is_log { return err("Log"); }
                }
                HierarchicalKind::Beta => { if !is_logit { return err("Logit"); } }
                HierarchicalKind::Uniform | HierarchicalKind::Normal => {} // any transform ok
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 6 wiring regression: when in-run `IF2Result::final_loglik`
    /// disagrees with the clean-eval winner, `select_winner_summary`
    /// must follow clean-eval. The handoff calls this out as the
    /// canonical Step 6 test ("synthetic 2-chain run picks the
    /// higher-clean-ll chain even when the other has higher in-run
    /// final_loglik"). Since `run_chains_with_per_chain_params`
    /// requires a real PF, we test the post-IF2 selection helper
    /// (`select_winner_summary`) on a `LoglikEvalOutcome` constructed
    /// via `run_loglik_eval_with_scorer`. The synthetic IF2Results
    /// carry deliberately misleading `final_loglik` values; the
    /// helper must ignore them.
    #[test]
    fn winner_summary_follows_clean_eval_not_in_run_loglik() {
        use crate::fit::loglik_eval::run_loglik_eval_with_scorer;
        use crate::fit::config_v2::{LoglikEvalConfig, CombineMode};
        use sim::inference::if2::{IF2IterResult, IF2Result};

        // Two chains. Chain 0 has *higher* in-run final_loglik (the
        // misleading number); chain 1 has thetas the deterministic
        // scorer prefers. Clean-eval should pick chain 1.
        let mk_chain = |theta: f64, in_run_ll: f64| IF2Result {
            iterations: vec![IF2IterResult {
                iteration: 0,
                loglik: in_run_ll,
                if2_perturbed_loglik: in_run_ll,
                param_means: vec![theta],
                param_diag: vec![],
            }],
            mle: vec![theta],
            final_loglik: in_run_ll,
            last_loglik: in_run_ll,
        };
        let results = vec![
            (0usize, mk_chain(0.5,  -10.0)), // misleading: best in-run
            (1usize, mk_chain(50.0, -200.0)),
        ];

        let scorer = |theta: &[f64], _: usize, _: u64| {
            // Clean PF prefers theta around 50.
            let ll = if theta[0] < 10.0 { -100.0 } else { -50.0 };
            (ll, crate::fit::loglik_eval::FilterStats::failed())
        };
        let cfg = LoglikEvalConfig {
            n_particles: 1, n_replicates: 4, combine: CombineMode::LogMeanExp,
        };
        let outcome = run_loglik_eval_with_scorer(&results, &cfg, 0, scorer).unwrap();

        let (best_chain, best_ll, best_se) = select_winner_summary(&outcome);
        assert_eq!(best_chain, 1,
            "clean-eval must pick chain 1 despite chain 0's higher in-run loglik");
        assert!((best_ll - (-50.0)).abs() < 1e-12);
        assert!(best_se.abs() < 1e-12, "deterministic scorer → SE = 0");
    }

    /// Verify that write_chain_outputs writes correct values for BOTH
    /// estimated and fixed parameters. Regression test for bug where
    /// fixed params all got base_params[0] instead of their actual value.
    #[test]
    fn chain_output_fixed_params_correct() {
        use std::collections::HashMap;
        use ir::{
            expr::{BinOpExpr, BinOpWrap, BinOp, Expr, ParamExpr, PopExpr},
            model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
            parameter::Parameter,
            transition::{Transition, StoichiometryEntry},
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
                    metadata: None, draw_method: ir::transition::DrawMethod::Poisson,
                    rate_grad: Default::default(),
                },
                Transition {
                    name: "recovery".into(),
                    stoichiometry: vec![StoichiometryEntry("I".into(), -1), StoichiometryEntry("R".into(), 1)],
                    rate: Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                        op: BinOp::Mul,
                        left: Box::new(Expr::Param(ParamExpr { param: "gamma".into() })),
                        right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
                    }}),
                    metadata: None, draw_method: ir::transition::DrawMethod::Poisson,
                    rate_grad: Default::default(),
                },
            ],
            ode_equations: vec![], time_functions: vec![], tables: vec![],
            interventions: vec![], observations: vec![],
            parameters: vec![
                Parameter { name: "beta".into(), value: Some(0.3), bounds: Some((0.01, 2.0)), prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None },
                Parameter { name: "gamma".into(), value: Some(0.1), bounds: Some((0.01, 1.0)), prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None },
                Parameter { name: "N0".into(), value: Some(1000.0), bounds: Some((100.0, 100000.0)), prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None },
            ],
            initial_conditions: InitialConditions::Explicit({
                let mut m = HashMap::new();
                m.insert("S".into(), 990.0);
                m.insert("I".into(), 10.0);
                m
            }),
            output: OutputConfig { times: OutputSchedule::AtTimes(vec![0.0, 80.0]), format: "tsv".into(), trajectory: true, observations: false },
            simulation: SimulationConfig { t_start: 0.0, t_end: 80.0, time_semantics: "continuous".into(), dt: Some(1.0), rng_seed: Some(42) },
            presets: vec![], model_structure: None, balance: None,
        };

        let compiled = CompiledModel::new(model).unwrap();
        let base_params = compiled.default_params.clone();

        // beta is estimated, gamma and N0 are fixed
        let if2_params = vec![EstimatedParam {
            name: "beta".into(),
            index: compiled.param_index["beta"],
            initial: 0.3,
            rw_sd: 0.05,
            transform: Transform::Log { lo: 0.01, hi: 2.0 },
            lower: 0.01,
            upper: 2.0,
            ivp: false, rw_sd_auto: false,
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
            &param_names, &base_params, &compiled, None,
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

    /// Clean-eval TSV emission. Schema is
    /// `chain\tloglik\tse\tess_mean\tess_min\tess_min_step\tn_neg_inf_incr\t<param...>`
    /// with one header line + N data rows (one per chain), in chain-id
    /// order. Verified for N=2. ESS columns reflect synthetic
    /// `FilterStats::failed()` (NaN ess_mean/min, -1 step).
    #[test]
    fn clean_eval_tsv_schema_and_rows() {
        use crate::fit::loglik_eval::{ChainScore, LoglikEvalOutcome, FilterStats};

        let outcome = LoglikEvalOutcome {
            per_chain: vec![
                ChainScore {
                    chain_id: 0,
                    theta: vec![0.10, 0.20],
                    loglik: -100.0, se: 0.5,
                    filter_stats: FilterStats::failed(),
                },
                ChainScore {
                    chain_id: 1,
                    theta: vec![0.30, 0.40],
                    loglik: -50.0, se: 0.4,
                    filter_stats: FilterStats::failed(),
                },
            ],
            overall_winner_idx: 1,
        };

        let mk_param = |name: &str, idx: usize| EstimatedParam {
            name: name.into(), index: idx, initial: 0.0,
            lower: 0.0, upper: 10.0, rw_sd: 0.1, rw_sd_auto: false,
            transform: Transform::None,
            ivp: false,
        };
        let if2_params = vec![mk_param("beta", 0), mk_param("gamma", 1)];

        let dir = std::env::temp_dir().join("camdl_test_clean_eval_tsv");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        write_clean_eval_tsv(dir.to_str().unwrap(), &outcome, &if2_params).unwrap();

        let content = std::fs::read_to_string(dir.join("chain_evaluations.tsv")).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(lines.len(), 1 + 2, "1 header + 2 chain rows");
        assert_eq!(lines[0],
            "chain\tloglik\tse\tess_mean\tess_min\tess_min_step\tn_neg_inf_incr\tbeta\tgamma");
        assert!(lines[1].starts_with("1\t-100.000000\t0.500000"),
            "chain 1 prefix: {}", lines[1]);
        assert!(lines[1].ends_with("\t0.100000\t0.200000"),
            "chain 1 param suffix: {}", lines[1]);
        assert!(lines[2].starts_with("2\t-50.000000\t0.400000"),
            "chain 2 prefix: {}", lines[2]);
        assert!(lines[2].ends_with("\t0.300000\t0.400000"),
            "chain 2 param suffix: {}", lines[2]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Step 7: run-root `final_params.toml` carries the overall winner
    /// chain + candidate label and writes the winner's θ̂ for estimated
    /// params (here: chain 1's TailMean theta, NOT chain 0's MLE).
    #[test]
    fn run_root_final_params_uses_overall_winner() {
        use crate::fit::loglik_eval::{ChainScore, LoglikEvalOutcome, FilterStats};

        let outcome = LoglikEvalOutcome {
            per_chain: vec![
                ChainScore { chain_id: 0, theta: vec![0.10], loglik: -100.0, se: 0.3,
                    filter_stats: FilterStats::failed() },
                ChainScore { chain_id: 1, theta: vec![0.42], loglik: -50.0, se: 0.2,
                    filter_stats: FilterStats::failed() },
            ],
            overall_winner_idx: 1,
        };

        use ir::{
            model::{Compartment, CompartmentKind, InitialConditions, OutputConfig,
                    OutputSchedule, SimulationConfig},
            parameter::Parameter,
        };

        let if2_params = vec![EstimatedParam {
            name: "beta".into(), index: 0, initial: 0.0,
            lower: 0.0, upper: 10.0, rw_sd: 0.1, rw_sd_auto: false,
            transform: Transform::None, ivp: false,
        }];

        // Minimal compiled stand-in. The writer only reads
        // `compiled.param_index` for *fixed* params; here every name in
        // `param_names` is in `if2_params`, so the lookup never fires.
        // Compartments are required because CompiledModel::new validates
        // them, but the simulation isn't run.
        let model = ir::Model {
            name: "t".into(), version: "0.3".into(), time_unit: "days".into(),
            description: None, origin: None,
            compartments: vec![
                Compartment { name: "S".into(), kind: CompartmentKind::Integer },
            ],
            transitions: vec![], ode_equations: vec![],
            time_functions: vec![], tables: vec![], interventions: vec![],
            observations: vec![],
            parameters: vec![Parameter {
                name: "beta".into(), value: Some(0.0), bounds: Some((0.0, 10.0)),
                prior: None, transform: None, initial_value: None,
                param_kind: None, param_dim: None, hierarchical: None,
            }],
            initial_conditions: InitialConditions::Explicit({
                let mut m = HashMap::new(); m.insert("S".into(), 100.0); m
            }),
            output: OutputConfig {
                times: OutputSchedule::AtTimes(vec![0.0, 1.0]),
                format: "tsv".into(), trajectory: true, observations: false,
            },
            simulation: SimulationConfig {
                t_start: 0.0, t_end: 1.0, time_semantics: "continuous".into(),
                dt: Some(1.0), rng_seed: Some(42),
            },
            presets: vec![], model_structure: None, balance: None,
        };
        let compiled = CompiledModel::new(model).unwrap();

        let dir = std::env::temp_dir().join("camdl_test_run_root_final");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let param_names = vec!["beta".to_string()];
        write_run_root_final_params(
            dir.to_str().unwrap(), &outcome, &if2_params,
            &param_names, &[0.0], &compiled,
        ).unwrap();

        let content = std::fs::read_to_string(dir.join("final_params.toml")).unwrap();
        // Header records overall winner chain.
        assert!(content.contains("# winner: chain=2"),
            "header missing or wrong: {}", content);
        // Provenance moved under [provenance] table — top-level keys
        // are parameters only so the file is loadable via the standard
        // params loader (GH #17). The metadata is still present, just
        // under the right scope.
        assert!(content.contains("[provenance]"),
            "expected [provenance] table; got: {}", content);
        assert!(content.contains("chain = 2"));
        // The estimated-param value is the overall winner's θ (0.42),
        // NOT chain 0's 0.10.
        assert!(content.contains("beta = 0.42"),
            "expected beta = 0.42 (winner's θ); got: {}", content);

        // Schema invariant: top-level keys are parameters (numeric)
        // only — provenance metadata lives under [provenance] so the
        // standard params loader doesn't reject the file (GH #17).
        let parsed: toml::Value = toml::from_str(&content)
            .expect("final_params.toml must parse as TOML");
        let top = parsed.as_table().unwrap();
        for (k, v) in top {
            if k == "provenance" { continue; }
            assert!(v.as_float().is_some() || v.as_integer().is_some(),
                "top-level key `{}` is `{:?}`, must be numeric (param) — \
                 metadata belongs under [provenance]", k, v);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for GH #17: `final_params.toml` (run-root) must be
    /// loadable by the standard params loader. The bug pre-fix emitted
    /// a string-typed metadata key at the top level, which
    /// `load_params_toml` rejected with `expected a number or table
    /// section, got String("…")`. The post-fix writer keeps all
    /// metadata under a `[provenance]` table, which the loader skips.
    /// (The original key, `winning_candidate_label`, was itself
    /// dropped in commit `20d48fe`'s clean-eval strip — the
    /// loadability invariant is what this test now guards.) This
    /// asserts loadability + correct parameter values, both of which
    /// are required for "rerun pfilter at the reported MLE"
    /// workflows to function.
    #[test]
    fn final_params_toml_is_loadable_by_params_loader() {
        use crate::fit::loglik_eval::{ChainScore, LoglikEvalOutcome, FilterStats};
        use ir::{
            model::{Compartment, CompartmentKind, InitialConditions, OutputConfig,
                    OutputSchedule, SimulationConfig},
            parameter::Parameter,
        };

        let outcome = LoglikEvalOutcome {
            per_chain: vec![
                ChainScore { chain_id: 5, theta: vec![87.668938],
                    loglik: -6235.11, se: 2.19,
                    filter_stats: FilterStats::failed() },
            ],
            overall_winner_idx: 0,
        };
        let if2_params = vec![EstimatedParam {
            name: "R0".into(), index: 0, initial: 0.0,
            lower: 1.0, upper: 200.0, rw_sd: 1.0, rw_sd_auto: false,
            transform: Transform::None, ivp: false,
        }];
        let model = ir::Model {
            name: "t".into(), version: "0.3".into(), time_unit: "days".into(),
            description: None, origin: None,
            compartments: vec![
                Compartment { name: "S".into(), kind: CompartmentKind::Integer },
            ],
            transitions: vec![], ode_equations: vec![],
            time_functions: vec![], tables: vec![], interventions: vec![],
            observations: vec![],
            parameters: vec![Parameter {
                name: "R0".into(), value: Some(0.0), bounds: Some((1.0, 200.0)),
                prior: None, transform: None, initial_value: None,
                param_kind: None, param_dim: None, hierarchical: None,
            }],
            initial_conditions: InitialConditions::Explicit({
                let mut m = HashMap::new(); m.insert("S".into(), 100.0); m
            }),
            output: OutputConfig {
                times: OutputSchedule::AtTimes(vec![0.0, 1.0]),
                format: "tsv".into(), trajectory: true, observations: false,
            },
            simulation: SimulationConfig {
                t_start: 0.0, t_end: 1.0, time_semantics: "continuous".into(),
                dt: Some(1.0), rng_seed: Some(42),
            },
            presets: vec![], model_structure: None, balance: None,
        };
        let compiled = CompiledModel::new(model).unwrap();

        let dir = std::env::temp_dir().join("camdl_test_final_params_loadable");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("final_params.toml");

        write_run_root_final_params(
            dir.to_str().unwrap(), &outcome, &if2_params,
            &["R0".to_string()], &[0.0], &compiled,
        ).unwrap();

        // The actual contract: load_params_toml must return Ok and
        // surface R0 as the clean-eval winner's value.
        let loaded = crate::util::load_params_toml(path.to_str().unwrap())
            .expect("final_params.toml must be loadable via load_params_toml \
                     (GH #17). If this errored with `expected a number or \
                     table section, got String(...)`, a top-level string \
                     metadata key has leaked back into the writer.");
        let r0 = loaded.get("R0").copied()
            .expect("R0 must be present after load");
        assert!((r0 - 87.668938).abs() < 1e-6,
            "loaded R0 must equal clean-eval winner θ̂; got {}", r0);

        // Provenance keys are intentionally NOT in the parameter map
        // (the loader skips the [provenance] section).
        assert!(!loaded.contains_key("winning_candidate_label"));
        assert!(!loaded.contains_key("loglik"));
        assert!(!loaded.contains_key("se"));
        assert!(!loaded.contains_key("chain"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression for GH #16 (silent-wrong-answer): `winner_theta`
    /// returns the clean-eval θ̂ (= IF2 final-iter param_means under
    /// FinalIter-only semantics), NOT `IF2Result.mle` (= argmax over
    /// the IF2 chain's noisy `if2_perturbed_loglik`). These are
    /// distinct selection mechanisms even under FinalIter-only clean-
    /// eval: `IF2Result.mle` picks the iteration whose perturbed
    /// loglik happened to be highest, while clean-eval reports
    /// `iterations.last().param_means`. They generally agree when
    /// IF2 has converged but can diverge mid-cooling, and historically
    /// produced silent disagreement between `mle_params.toml` and
    /// `final_params.toml` (GH #16).
    #[test]
    fn winner_theta_picks_clean_eval_winner_not_if2_argmax() {
        use crate::fit::loglik_eval::{ChainScore, LoglikEvalOutcome, FilterStats};

        // IF2 results: .mle represents what pre-fix code would have
        // selected (the chain's IF2 argmax over perturbed loglik). The
        // clean-eval θ̂ for each chain is the chain's final-iter mean,
        // distinct from .mle when IF2's perturbed-loglik argmax landed
        // on a different iteration.
        let if2_chain0 = sim::inference::if2::IF2Result {
            mle: vec![0.10, 0.20],   // chain 0's IF2 argmax
            final_loglik: -100.0,
            last_loglik: -100.0,
            iterations: vec![],
        };
        let if2_chain1 = sim::inference::if2::IF2Result {
            mle: vec![0.30, 0.40],   // chain 1's IF2 argmax (pre-fix bug returned this)
            final_loglik: -50.0,
            last_loglik: -50.0,
            iterations: vec![],
        };

        // Clean-eval reports each chain's final-iter mean as θ̂. Chain
        // 1's clean-eval θ̂ ([0.31, 0.41]) differs from its IF2 .mle
        // ([0.30, 0.40]) — that divergence is what the test discriminates.
        let loglik_eval = LoglikEvalOutcome {
            per_chain: vec![
                ChainScore { chain_id: 0, theta: vec![0.10, 0.20],
                    loglik: -110.0, se: 0.5, filter_stats: FilterStats::failed() },
                ChainScore { chain_id: 1, theta: vec![0.31, 0.41],
                    loglik: -49.0,  se: 0.4, filter_stats: FilterStats::failed() },
            ],
            overall_winner_idx: 1,
        };

        let mut chain_agreement = HashMap::new();
        chain_agreement.insert("beta".to_string(),  1.05);
        chain_agreement.insert("gamma".to_string(), 1.06);
        let cr = ChainResults {
            results: vec![(0, if2_chain0), (1, if2_chain1)],
            best_chain: 1,
            best_loglik: -49.0,
            chain_agreement,
            loglik_eval,
        };

        let theta = cr.winner_theta();
        assert_eq!(theta, &[0.31, 0.41],
            "winner_theta must return clean-eval winner θ̂ \
             (= chain 1's final-iter mean [0.31, 0.41]), NOT chain 1's \
             IF2Result.mle ([0.30, 0.40]). If this fails, \
             mle_params.toml will diverge from final_params.toml — \
             the GH #16 silent-wrong-answer is back.");

        // Pre-fix path for reference: what `&best.mle` of best_chain returns.
        let best = &cr.results.iter().find(|(id, _)| *id == cr.best_chain).unwrap().1;
        assert_eq!(&best.mle, &vec![0.30, 0.40],
            "sanity: chain 1's IF2 mle is [0.30, 0.40] (different \
             from clean-eval winner [0.31, 0.41]) — this is what \
             makes the test discriminate.");
        assert_ne!(theta, best.mle.as_slice(),
            "winner_theta and best.mle must differ in this fixture, \
             else the test isn't catching the bug class");
    }

    /// resolve_prior precedence chain: fit.toml override → model IR → Flat.
    #[test]
    fn resolve_prior_precedence_chain() {
        use ir::parameter::{Parameter, PriorDist, LogNormalPrior, NormalPrior};
        use crate::fit::config_v2::EstimateSpecV2;
        use indexmap::IndexMap;

        let beta_with_ir_prior = Parameter {
            name: "beta".into(), value: None, bounds: Some((0.01, 2.0)),
            prior: Some(PriorDist::LogNormal(LogNormalPrior { mu: -1.0, sigma: 0.5 })),
            transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None,
        };
        let gamma_no_prior = Parameter {
            name: "gamma".into(), value: None, bounds: Some((0.05, 1.0)),
            prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None,
        };
        let model = ir::Model {
            name: "t".into(), version: "0.3".into(), time_unit: "days".into(),
            description: None, origin: None,
            compartments: vec![], transitions: vec![], ode_equations: vec![],
            time_functions: vec![], tables: vec![], interventions: vec![], observations: vec![],
            parameters: vec![beta_with_ir_prior, gamma_no_prior],
            initial_conditions: ir::model::InitialConditions::Explicit(HashMap::new()),
            output: ir::model::OutputConfig {
                times: ir::model::OutputSchedule::AtTimes(vec![]),
                format: "tsv".into(), trajectory: true, observations: false,
            },
            simulation: ir::model::SimulationConfig {
                t_start: 0.0, t_end: 1.0, time_semantics: "continuous".into(),
                dt: None, rng_seed: None,
            },
            presets: vec![], model_structure: None, balance: None,
        };

        let est_with_normal = |name: &str, mean: f64, sd: f64| {
            let mut m: IndexMap<String, EstimateSpecV2> = IndexMap::new();
            m.insert(name.to_string(), EstimateSpecV2 {
                bounds: (0.01, 2.0), transform: None,
                prior: Some(PriorDist::Normal(NormalPrior { mean, sd })),
                ivp: false, rw_sd: None, start: None,
            });
            m
        };

        // (1) fit.toml override beats IR prior
        let estimate_override = est_with_normal("beta", 0.3, 0.1);
        let (p, src) = resolve_prior("beta", &estimate_override, &model);
        assert_eq!(src, "fit.toml", "fit.toml override should take precedence");
        match p {
            Prior::Normal { mean, sd } => {
                assert!((mean - 0.3).abs() < 1e-9);
                assert!((sd - 0.1).abs() < 1e-9);
            }
            other => panic!("expected Normal from fit.toml, got {:?}", other),
        }

        // (2) IR prior used when fit.toml has no override
        let estimate_empty: IndexMap<String, EstimateSpecV2> = IndexMap::new();
        let (p, src) = resolve_prior("beta", &estimate_empty, &model);
        assert_eq!(src, "model", "model IR prior should apply when fit.toml is silent");
        match p {
            Prior::TransformedNormal { mean, sd } => {
                // LogNormal(mu=-1.0, sigma=0.5) in IR → TransformedNormal on log scale
                assert!((mean - (-1.0)).abs() < 1e-9);
                assert!((sd - 0.5).abs() < 1e-9);
            }
            other => panic!("expected TransformedNormal from IR LogNormal, got {:?}", other),
        }

        // (3) Flat fallback when neither fit.toml nor IR provide a prior
        let (p, src) = resolve_prior("gamma", &estimate_empty, &model);
        assert_eq!(src, "flat (default)");
        assert!(matches!(p, Prior::Flat));
    }

    /// Cover every distribution supported in fit.toml `prior = ...` strings.
    /// Regression guard for the asymmetry bug where fit.toml could only override
    /// 4 of the 7 IR distributions.
    /// End-to-end: priors declared in a .camdl file survive compilation to
    /// Regression for the `starts_from = "scout"` bug: when a FitState
    /// (scout's output) is supplied to `FitRunConfig::build`, the
    /// resulting `base_params` must reflect the scout-best values —
    /// NOT the fit.toml `[estimate].*.start` values. The fix for this
    /// was reversing the application order in build. See
    /// docs/dev/incidents/2026-04-18-starts-from-scout-ignored.md.
    ///
    /// IF2 uses `config.base_params` as its starting point for the
    /// particle cloud (if2.rs:338, `current_params = base_params`).
    /// If the priority inversion lets est.start overwrite scout's
    /// best, refine starts from scratch instead of from scout's MLE.
    #[test]
    fn fit_state_overrides_config_start_in_base_params() {
        use crate::fit::state::FitState;
        use crate::fit::config_v2::FitConfigV2;
        use std::collections::HashMap;

        // Tiny v2 fit.toml referencing the seir golden. We set
        // beta's `start = 0.1`; prior_state will supply 0.4. The
        // bug has `start` winning; the fix has `prior_state` winning.
        // Both values must sit within seir's declared beta bounds
        // [0.001, 0.5] so the post-resolution validator (gh#31) lets
        // the build succeed; the precedence test only needs the two
        // values to be distinguishable.
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let ir_path = format!("{}/../../../ocaml/golden/seir_observations.ir.json", manifest);
        let data_dir = std::env::temp_dir().join(format!(
            "camdl_starts_from_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&data_dir).unwrap();
        let data_path = data_dir.join("obs.tsv");
        std::fs::write(&data_path,
            "time\tweekly_cases\n7\t1\n14\t2\n21\t3\n28\t4\n35\t5\n").unwrap();

        // The v2 fit.toml. We use `start = 0.1` on beta and a [stages.scout]
        // section so the config validates; build() doesn't actually consume
        // the stage block (chains/particles come from its own args).
        let fit_toml_path = data_dir.join("fit.toml");
        let toml = format!(r#"
output_dir = "{}"

[model]
camdl = "{}"

[data.observations]
weekly_cases = "{}"

[estimate.beta]
bounds = [0.01, 0.5]
start  = 0.1

[fixed]
sigma    = 0.25
gamma    = 0.3
rho      = 0.5
k        = 10.0
p_detect = 0.5
N0       = 1000
I0       = 1

[stages.scout]
method     = "if2"
chains     = 1
particles  = 100
iterations = 1
cooling    = 0.5

[config]
backend = "gillespie"
dt = 1.0
"#, data_dir.display(), ir_path, data_path.display());
        std::fs::write(&fit_toml_path, &toml).unwrap();
        let fit = FitConfigV2::load(&fit_toml_path.to_string_lossy())
            .expect("v2 fit.toml parse");

        // Scout produced a very different "best" — a clearly
        // distinguishable value so a win/loss is unambiguous.
        // Within [0.001, 0.5] but visibly far from est.start=0.1.
        let mut start_values = HashMap::new();
        start_values.insert("beta".to_string(), 0.4);
        let prior_state = FitState {
            stage: "scout".into(), seed: 1,
            timestamp: "2026-04-18T00:00:00Z".into(),
            input_hash: None, camdl_version: None,
            best_loglik: -100.0, initial_loglik: f64::NEG_INFINITY,
            best_chain: 0, n_chains: 1, n_good_chains: Some(1),
            start_values,
            rw_sd: HashMap::new(),
            loglik_type: Some("if2".into()),
            acceptance_rate: None,
            tail_chain_agreement: HashMap::new(),
            ivp_params: Vec::new(),
            chain_logliks: Vec::new(),
            chain_eval_logliks: Vec::new(),
            chain_eval_ses: Vec::new(),
            resolved_gate: None,
            resolved_loglik_eval: None,
        };

        let config = FitRunConfig::build(
            &fit, Some(&prior_state),
            1, 100, 1, 0.5, 50, 1, false,
        ).expect("build must succeed");

        let beta_idx = config.compiled.param_index.get("beta").copied()
            .expect("beta present");
        assert!((config.base_params[beta_idx] - 0.4).abs() < 1e-9,
            "prior_state must win over est.start — got {}, expected 0.4 \
             (scout's best). 0.1 means est.start overwrote scout — the \
             pre-fix bug is back.",
            config.base_params[beta_idx]);

        std::fs::remove_dir_all(&data_dir).ok();
    }

    // ── IC-free inference: config validation ────────────────────────────

    fn ic_free_fixture(dir: &std::path::Path, ic_free: bool, ivp: bool)
        -> super::super::config_v2::FitConfigV2
    {
        // Minimal v2 fit.toml against the seir_observations golden IR.
        // Toggles ic_free and whether I0 is ivp-flagged independently
        // so all four combinations can be built.
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let ir_path = format!(
            "{}/../../../ocaml/golden/seir_observations.ir.json", manifest);
        let data_path = dir.join("obs.tsv");
        std::fs::write(&data_path,
            "time\tweekly_cases\n7\t1\n14\t2\n21\t3\n28\t4\n35\t5\n").unwrap();
        let ivp_line = if ivp { "ivp    = true\n" } else { "" };
        let fit_toml_path = dir.join("fit.toml");
        let toml_src = format!(r#"
output_dir = "{}"
ic_free = {}

[model]
camdl = "{}"

[data.observations]
weekly_cases = "{}"

[estimate.I0]
bounds = [1, 1000]
start  = 5
{}
[fixed]
sigma    = 0.25
gamma    = 0.3
rho      = 0.5
k        = 10.0
p_detect = 0.5
N0       = 1000
beta     = 0.1

[stages.scout]
method     = "if2"
chains     = 1
particles  = 100
iterations = 1
cooling    = 0.5

[config]
backend = "gillespie"
dt = 1.0
"#, dir.display(), ic_free, ir_path, data_path.display(), ivp_line);
        std::fs::write(&fit_toml_path, toml_src).unwrap();
        super::super::config_v2::FitConfigV2::load(
            &fit_toml_path.to_string_lossy())
            .expect("v2 fit.toml parse")
    }

    fn ic_free_test_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "camdl_icfree_{}_{}_{}", tag, std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// ic_free=true WITHOUT any ivp estimate → build errors with a
    /// helpful message naming the fix.
    #[test]
    fn ic_free_true_requires_ivp() {
        let dir = ic_free_test_dir("requires_ivp");
        let fit = ic_free_fixture(&dir, true, false);
        let err = match FitRunConfig::build(&fit, None, 1, 100, 1, 0.5, 50, 1, false) {
            Ok(_)  => panic!("ic_free=true + no ivp must error"),
            Err(e) => e,
        };
        assert!(err.contains("ic_free") && err.contains("ivp"),
            "error must name both ic_free and ivp: {}", err);
        assert!(err.contains("I0 = {") || err.contains("ivp = true"),
            "error should include a copy-pasteable example: {}", err);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ic_free=true WITH an ivp estimate → build succeeds and
    /// config.ic_free is propagated.
    #[test]
    fn ic_free_true_with_ivp_succeeds() {
        let dir = ic_free_test_dir("with_ivp");
        let fit = ic_free_fixture(&dir, true, true);
        let config = FitRunConfig::build(&fit, None, 1, 100, 1, 0.5, 50, 1, false)
            .expect("ic_free=true + ivp must build");
        assert!(config.ic_free, "FitRunConfig.ic_free must be true");
        // The SMCConfig view also carries the flag — that's what reaches
        // the PF / IF2 loop.
        assert!(config.smc_config().skip_first_obs_from_loglik,
            "smc_config() must thread ic_free into skip_first_obs_from_loglik");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ic_free absent (default false) → build succeeds regardless of
    /// ivp presence, and the SMCConfig view reports ic_free=false.
    /// Regression guard: the new flag must default to OFF so no
    /// existing fit.toml silently changes behaviour.
    #[test]
    fn ic_free_default_off_does_not_require_ivp() {
        let dir = ic_free_test_dir("default_off");
        let fit = ic_free_fixture(&dir, false, false);
        let config = FitRunConfig::build(&fit, None, 1, 100, 1, 0.5, 50, 1, false)
            .expect("ic_free=false + no ivp must build");
        assert!(!config.ic_free);
        assert!(!config.smc_config().skip_first_obs_from_loglik);
        std::fs::remove_dir_all(&dir).ok();
    }
    /// pipeline that pgas.rs / pmmh.rs use to build the Prior vector.
    ///
    /// This is the integration counterpart to resolve_prior_precedence_chain
    /// (which uses a hand-constructed ir::Model). Regression guard for any
    /// serde field rename or IR<->compiler drift.
    #[test]
    fn resolve_prior_end_to_end_from_golden_ir() {
        // sir_priors golden has: beta~LogNormal, gamma~HalfNormal,
        // rho~Beta, N0~LogNormal, I0~Exponential.
        use crate::fit::config_v2::EstimateSpecV2;
        use indexmap::IndexMap;
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let ir_path = format!("{}/../../../ocaml/golden/sir_priors.ir.json", manifest);
        let (model, _) = crate::util::load_model(&ir_path).expect("load golden");

        let empty: IndexMap<String, EstimateSpecV2> = IndexMap::new();

        // beta: LogNormal in IR → TransformedNormal at the Prior layer.
        let (p, src) = resolve_prior("beta", &empty, &model);
        assert_eq!(src, "model", "beta's IR prior should be picked up");
        match p {
            Prior::TransformedNormal { mean, sd } => {
                assert!((mean - (-1.0)).abs() < 1e-9, "mean {}", mean);
                assert!((sd - 0.5).abs() < 1e-9, "sd {}", sd);
            }
            other => panic!("beta expected TransformedNormal, got {:?}", other),
        }

        // gamma: HalfNormal round-trip
        let (p, src) = resolve_prior("gamma", &empty, &model);
        assert_eq!(src, "model");
        assert!(matches!(p, Prior::HalfNormal { .. }), "gamma: {:?}", p);

        // rho: Beta round-trip
        let (p, src) = resolve_prior("rho", &empty, &model);
        assert_eq!(src, "model");
        match p {
            Prior::Beta { alpha, beta } => {
                assert!((alpha - 2.0).abs() < 1e-9);
                assert!((beta - 5.0).abs() < 1e-9);
            }
            other => panic!("rho expected Beta, got {:?}", other),
        }

        // I0: Exponential round-trip
        let (p, src) = resolve_prior("I0", &empty, &model);
        assert_eq!(src, "model");
        assert!(matches!(p, Prior::Exponential { .. }), "I0: {:?}", p);
    }

    /// End-to-end: fit.toml [estimate] prior overrides the model IR prior.
    /// Same golden model, but fit.toml specifies a different distribution
    /// for beta — the override must win over what's in the .camdl.
    #[test]
    fn fit_toml_override_beats_golden_ir_prior() {
        use crate::fit::config_v2::EstimateSpecV2;
        use ir::parameter::{PriorDist, NormalPrior};
        use indexmap::IndexMap;
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let ir_path = format!("{}/../../../ocaml/golden/sir_priors.ir.json", manifest);
        let (model, _) = crate::util::load_model(&ir_path).expect("load golden");

        // Override beta with a much narrower normal prior; leave gamma alone.
        let mut estimate: IndexMap<String, EstimateSpecV2> = IndexMap::new();
        estimate.insert("beta".to_string(), EstimateSpecV2 {
            bounds: (0.01, 5.0), transform: None,
            prior: Some(PriorDist::Normal(NormalPrior { mean: 0.25, sd: 0.05 })),
            ivp: false, rw_sd: None, start: None,
        });

        let (p, src) = resolve_prior("beta", &estimate, &model);
        assert_eq!(src, "fit.toml", "override should take precedence");
        match p {
            Prior::Normal { mean, sd } => {
                assert_eq!(mean, 0.25); assert_eq!(sd, 0.05);
            }
            other => panic!("override should be Normal(0.25, 0.05), got {:?}", other),
        }

        // gamma is not overridden → still uses the IR's HalfNormal.
        let (p, src) = resolve_prior("gamma", &estimate, &model);
        assert_eq!(src, "model");
        assert!(matches!(p, Prior::HalfNormal { .. }));
    }

    /// Replaces the v1-era `parse_prior_covers_all_distributions` +
    /// `parse_prior_rejects_invalid_input` tests. fit.toml carries `prior`
    /// as `ir::PriorDist`; each variant must map onto the correct runtime
    /// `Prior` via `Prior::from_ir`.
    #[test]
    fn prior_dist_to_prior_maps_each_variant() {
        use ir::parameter::{
            PriorDist, LogNormalPrior, NormalPrior, BetaPrior, UniformPrior,
            HalfNormalPrior, GammaPrior, ExponentialPrior,
        };
        match Prior::from_ir(&PriorDist::LogNormal(LogNormalPrior { mu: 1.5, sigma: 0.4 })) {
            Prior::TransformedNormal { mean, sd } => {
                assert_eq!(mean, 1.5); assert_eq!(sd, 0.4);
            }
            other => panic!("LogNormal: {:?}", other),
        }
        match Prior::from_ir(&PriorDist::Normal(NormalPrior { mean: 0.3, sd: 0.1 })) {
            Prior::Normal { mean, sd } => {
                assert_eq!(mean, 0.3); assert_eq!(sd, 0.1);
            }
            other => panic!("Normal: {:?}", other),
        }
        match Prior::from_ir(&PriorDist::Beta(BetaPrior { alpha: 2.0, beta: 5.0 })) {
            Prior::Beta { alpha, beta } => {
                assert_eq!(alpha, 2.0); assert_eq!(beta, 5.0);
            }
            other => panic!("Beta: {:?}", other),
        }
        // Uniform now carries explicit bounds (no silent reduction to Flat
        // on missing fields — that v2 behaviour is intentionally removed).
        match Prior::from_ir(&PriorDist::Uniform(UniformPrior { lower: -1.0, upper: 2.0 })) {
            Prior::Uniform { lower, upper } => {
                assert_eq!(lower, -1.0); assert_eq!(upper, 2.0);
            }
            other => panic!("Uniform: {:?}", other),
        }
        match Prior::from_ir(&PriorDist::HalfNormal(HalfNormalPrior { sigma: 0.3 })) {
            Prior::HalfNormal { sigma } => assert_eq!(sigma, 0.3),
            other => panic!("HalfNormal: {:?}", other),
        }
        match Prior::from_ir(&PriorDist::Gamma(GammaPrior { shape: 3.0, rate: 0.5 })) {
            Prior::Gamma { shape, rate } => {
                assert_eq!(shape, 3.0); assert_eq!(rate, 0.5);
            }
            other => panic!("Gamma: {:?}", other),
        }
        match Prior::from_ir(&PriorDist::Exponential(ExponentialPrior { rate: 2.5 })) {
            Prior::Exponential { rate } => assert_eq!(rate, 2.5),
            other => panic!("Exponential: {:?}", other),
        }
    }

    /// gh#34: when [estimate] entry omits `start =`, the run-config
    /// builder fills in a value automatically. Geometric mean of
    /// bounds for both-positive bounds; arithmetic mean otherwise. No
    /// more "parameter 'foo' has no value" failure for forgetful users.
    #[test]
    fn estimate_without_start_falls_back_to_bounds_midpoint() {
        use crate::fit::config_v2::FitConfigV2;

        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let ir_path = format!("{}/../../../ocaml/golden/seir_observations.ir.json", manifest);
        let data_dir = std::env::temp_dir().join(format!(
            "camdl_gh34_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&data_dir).unwrap();
        let data_path = data_dir.join("obs.tsv");
        std::fs::write(&data_path,
            "time\tweekly_cases\n7\t1\n14\t2\n21\t3\n28\t4\n35\t5\n").unwrap();

        // beta has bounds [0.01, 0.5] and NO `start =`. The geometric
        // mean is sqrt(0.01 * 0.5) ≈ 0.0707. Pre-gh#34 this would fail
        // with "parameter 'beta' has no value".
        let fit_toml_path = data_dir.join("fit.toml");
        let toml = format!(r#"
output_dir = "{}"

[model]
camdl = "{}"

[data.observations]
weekly_cases = "{}"

[estimate.beta]
bounds = [0.01, 0.5]

[fixed]
sigma    = 0.25
gamma    = 0.3
rho      = 0.5
k        = 10.0
p_detect = 0.5
N0       = 1000
I0       = 1

[stages.scout]
method     = "if2"
chains     = 1
particles  = 100
iterations = 1
cooling    = 0.5

[config]
backend = "gillespie"
dt = 1.0
"#, data_dir.display(), ir_path, data_path.display());
        std::fs::write(&fit_toml_path, &toml).unwrap();
        let fit = FitConfigV2::load(&fit_toml_path.to_string_lossy())
            .expect("v2 fit.toml parse");

        let config = FitRunConfig::build(
            &fit, None,
            1, 100, 1, 0.5, 50, 1, false,
        ).expect("build must succeed without explicit start (gh#34)");

        let beta_idx = config.compiled.param_index.get("beta").copied()
            .expect("beta present");
        let expected = (0.01_f64 * 0.5).sqrt();
        assert!((config.base_params[beta_idx] - expected).abs() < 1e-9,
            "missing start should fall back to geometric mean of bounds \
             — got {}, expected {} (sqrt(0.01 * 0.5))",
            config.base_params[beta_idx], expected);

        std::fs::remove_dir_all(&data_dir).ok();
    }
}
