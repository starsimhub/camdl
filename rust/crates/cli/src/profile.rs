//! `camdl profile` — profile likelihood via parallel IF2 runs.
//!
//! For one or more focal parameters, fix them at a grid of values and
//! run IF2 to maximise over the remaining parameters at each grid
//! point. The profile likelihood shows how the MLE changes as you move
//! the focal parameter(s) — revealing identifiability, confidence
//! intervals, and parameter interactions. 2D profiles (two `--sweep`
//! flags) produce a likelihood surface suitable for contour plotting.
//!
//! ## CAS integration (2026-04-24 rewrite)
//!
//! Every (grid_point × start) combination is a cacheable mini-fit.
//! State lives under:
//!
//! ```text
//! <root>/profiles/<stem>-<profile_hash[:8]>/
//!   run.json                                    # RunKind::Profile
//!   profile.tsv                                 # derived rollup
//!   points/
//!     {point_idx:05d}/
//!       focal.toml                              # pinned focal values
//!       start_{start_idx}/
//!         run.json                              # RunKind::FitStage
//!         mle.toml                              # MLE at this start
//! ```
//!
//! Each `start_{k}/run.json` is written atomically (tmp-then-rename);
//! crash mid-IF2 leaves no run.json and the next invocation reruns
//! that start. Completed starts are preserved bit-for-bit. The rollup
//! is rewritten atomically after every completion, so it's always
//! current-as-of-last-finished-start.
//!
//! Design: docs/dev/proposals/2026-04-24-profile-cas-integration.md.
//! Supersedes GH #15's streaming-TSV + --resume approach.

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sim::{
    compiled_model::CompiledModel,
    inference::{
        if2::{run_if2, IF2Config, Observation},
        ParticleState,
        ChainBinomialProcess, MultiStreamObsModel,
        multi_stream_obs::StreamSpec,
        traits::ObservationModel,
    },
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::cas::typed::{
    self, CasInputs, ContentHash, ReplicateSet, hash_canonical,
};
use crate::run_meta::{FitStageMeta, GridAxis, ProfileMeta, Run, RunKind};
use crate::run_paths::{
    output_root, profile_point_dir, profile_point_start_dir,
};

// ─── ProfileInputs ───────────────────────────────────────────────────────────

/// Typed CAS inputs for a single-realization profile run. The struct
/// carries every content-bearing input (model, base params, focal
/// grid, fixed list, IF2 hyperparams, starts_from lineage, seed) plus
/// presentation hints (model_path, stem). Ephemeral inputs (parallel,
/// progress, output mirror) live on `ProfileArgs` and don't appear here.
///
/// `inner_hash` excludes seed and is the umbrella hash for a multi-seed
/// `ReplicateSet`. `content_hash` (the trait method) includes seed via
/// `compose_with_replicate(inner_hash, "seed", seed)` — so a standalone
/// `--seed N` invocation and one child of a `--seeds 1,N,...` set hit
/// the same cache key.
#[derive(Clone, Debug)]
pub struct ProfileInputs {
    /// Display-only model path. Recorded in `ProfileMeta.model`.
    pub model_path: String,
    /// Slugified stem from the model path; used as the `<stem>-<hash>`
    /// directory prefix.
    pub stem: Option<String>,
    /// Full SHA-256 of the IR JSON.
    pub model_hash: String,
    /// Canonical-form hash of the base parameter vector.
    pub base_params_hash: String,
    /// Focal grid (one axis per `--sweep` flag).
    pub focal_grid: Vec<GridAxis>,
    /// Fixed parameters (`--fixed`): excluded from IF2 estimation. Order
    /// doesn't matter; sorted before hashing.
    pub fixed: Vec<String>,
    /// IF2 hyperparameter set.
    pub if2_config: ProfileIf2Config,
    /// Hash of an upstream stage's content this profile starts from.
    /// `None` for standalone profile invocations.
    pub starts_from_lineage: Option<String>,
    /// Per-seed: the actual seed value. `inner_hash` excludes this;
    /// `content_hash` (trait method) includes it.
    pub seed: u64,
}

#[derive(Clone, Debug)]
pub struct ProfileIf2Config {
    pub n_particles:  usize,
    pub n_iterations: usize,
    pub cooling:      f64,
    pub dt:           f64,
    pub n_starts:     usize,
}

impl ProfileInputs {
    /// Hash of all content fields *except* seed. Used as the
    /// inner_hash of a `ReplicateSet` umbrella when running multi-seed.
    pub fn inner_hash(&self) -> ContentHash {
        let grid_canonical = serde_json::to_string(&self.focal_grid).unwrap_or_default();
        let mut fixed_sorted = self.fixed.clone();
        fixed_sorted.sort();
        let if2 = format!(
            "particles={};iterations={};cooling={};dt={};starts={}",
            self.if2_config.n_particles, self.if2_config.n_iterations,
            self.if2_config.cooling, self.if2_config.dt, self.if2_config.n_starts,
        );
        hash_canonical(&[
            ("model",       &self.model_hash),
            ("base_params", &self.base_params_hash),
            ("focal_grid",  &grid_canonical),
            ("fixed",       &fixed_sorted.join(",")),
            ("if2",         &if2),
            ("starts_from", self.starts_from_lineage.as_deref().unwrap_or("")),
        ])
    }
}

impl CasInputs for ProfileInputs {
    fn content_hash(&self) -> ContentHash {
        // Per-seed leaf hash. Composes with `seed` so the same value
        // is obtained whether the run was invoked standalone or as
        // one child of a multi-seed ReplicateSet.
        typed::compose_with_replicate(
            &self.inner_hash(), "seed", &self.seed.to_string(),
        )
    }

    fn cas_path(&self, root: &Path) -> PathBuf {
        let h = self.content_hash();
        let dirname = match &self.stem {
            Some(s) if !s.is_empty() => format!("{}-{}", s, h.short()),
            _ => h.short().to_string(),
        };
        root.join("profiles").join(dirname)
    }

    fn run_kind(&self) -> RunKind {
        let total_jobs = self.focal_grid.iter()
            .map(|g| g.values.len()).product::<usize>()
            * self.if2_config.n_starts;
        // The if2_config_hash and base_params_hash fields on
        // ProfileMeta are diagnostic; ProfileInputs.content_hash() is
        // the authoritative cache key. Keeping the meta fields for
        // human inspection in `camdl show`.
        let if2_canonical = format!(
            "particles={};iterations={};cooling={};dt={};starts={}",
            self.if2_config.n_particles, self.if2_config.n_iterations,
            self.if2_config.cooling, self.if2_config.dt, self.if2_config.n_starts,
        );
        let if2_config_hash = ContentHash::from_bytes(if2_canonical.as_bytes())
            .full().to_string();
        RunKind::Profile(ProfileMeta {
            model:            self.model_path.clone(),
            model_hash:       self.model_hash.clone(),
            focal_params:     self.focal_grid.iter().map(|g| g.param.clone()).collect(),
            grid:             self.focal_grid.clone(),
            n_starts:         self.if2_config.n_starts,
            if2_config_hash,
            base_params_hash: self.base_params_hash.clone(),
            seed_base:        self.seed,
            total_jobs,
        })
    }
}

pub fn cmd_profile(a: &crate::args::ProfileArgs) {
    let ir_path = a.model.to_string_lossy().into_owned();
    let data_path = a.data.to_string_lossy().into_owned();
    let n_particles = a.inference.particles;
    let n_iterations = a.iterations;
    let n_starts = a.starts;
    let cooling = a.cooling;
    let dt = a.inference.dt;
    let seed_base = a.inference.seed;
    let parallel = a.inference.parallel;
    let output_tsv_path: Option<String> = a.output.as_ref().map(|p| p.to_string_lossy().into_owned());
    let scenario_name = a.scenario.scenario.clone();
    let flow_name = a.flow.flow.clone();
    let label_arg: Option<String> = match a.label.as_deref() {
        Some(raw) => match crate::fit::validate_label(raw) {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("error: invalid --label: {}", e);
                std::process::exit(1);
            }
        },
        None => None,
    };
    let overrides: HashMap<String, f64> = a.model_overrides.param.iter()
        .map(|p| (p.name.clone(), p.value))
        .collect();

    let focal_names: Vec<String> = a.sweep.iter().map(|s| s.name.clone()).collect();

    struct FocalGrid { name: String, values: Vec<f64>, param_idx: usize }
    let mut focal_grids: Vec<FocalGrid> = Vec::new();

    let rw_sd = a.rw_sd.as_ref().unwrap_or_else(|| {
        eprintln!("error: --rw-sd required (e.g., --rw-sd \"sigma=0.01\" or --rw-sd auto)");
        std::process::exit(1);
    });
    let rw_sd_auto = matches!(rw_sd, crate::args::types::RwSd::Auto);
    let rw_sd_map_raw: HashMap<String, Option<f64>> = match rw_sd {
        crate::args::types::RwSd::Auto => HashMap::new(),
        crate::args::types::RwSd::Map(m) => m.clone(),
    };

    // Load model
    let (mut model, model_json) = crate::util::load_model(&ir_path)
        .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });

    for pf in &a.model_overrides.params {
        crate::util::apply_params_file(&mut model, &pf.to_string_lossy())
            .unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); });
    }
    if let Some(ref name) = scenario_name {
        if let Some(preset) = model.presets.iter().find(|p| p.name == *name) {
            for p in &mut model.parameters {
                if let Some(&v) = preset.params.get(&p.name) { p.value = Some(v); }
            }
        }
    }
    for p in &mut model.parameters {
        if let Some(&v) = overrides.get(&p.name) { p.value = Some(v); }
    }

    let compiled = Arc::new(CompiledModel::new(model.clone())
        .unwrap_or_else(|e| { eprintln!("{:?}", e); std::process::exit(1); }));
    let base_params = compiled.default_params.clone();

    let observations: Vec<Observation> = crate::pfilter::load_data_tsv_pub(&data_path)
        .unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); })
        .into_iter().map(|o| Observation { time: o.time, value: o.value }).collect();
    let observations = Arc::new(observations);

    let flow_indices = crate::util::resolve_flow_indices(&model, flow_name.as_deref())
        .unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });
    let flow_indices = Arc::new(flow_indices);

    for sw in &a.sweep {
        let idx = compiled.param_index.get(sw.name.as_str()).copied()
            .unwrap_or_else(|| {
                eprintln!("focal parameter '{}' not found", sw.name);
                std::process::exit(1);
            });
        focal_grids.push(FocalGrid {
            name: sw.name.clone(),
            values: sw.grid.expand(),
            param_idx: idx,
        });
    }

    let fixed_names: std::collections::HashSet<String> = a.fixed.iter().cloned().collect();
    let exclude: std::collections::HashSet<String> = focal_names.iter()
        .chain(fixed_names.iter()).cloned().collect();

    let param_names_to_estimate: Vec<String> = if rw_sd_auto {
        model.parameters.iter()
            .filter(|p| !exclude.contains(&p.name))
            .filter(|p| compiled.param_index.contains_key(p.name.as_str()))
            .map(|p| p.name.clone())
            .collect()
    } else {
        rw_sd_map_raw.keys()
            .filter(|name| !exclude.contains(*name))
            .cloned()
            .collect()
    };

    let specs: Vec<crate::fit::runner::ParamSpec> = param_names_to_estimate.iter().map(|name| {
        crate::fit::runner::ParamSpec {
            name: name.clone(),
            rw_sd: rw_sd_map_raw.get(name).and_then(|v| *v),
            transform: None,
            ivp: false,
        }
    }).collect();

    let if2_params = crate::fit::runner::build_if2_params_from_specs(
        &model, &compiled, &base_params, &specs,
    ).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); });
    let if2_params = Arc::new(if2_params);

    let process = Arc::new(ChainBinomialProcess::new(compiled.clone()));
    let obs_model_obj: Arc<dyn ObservationModel<ParticleState> + Send + Sync> = {
        let obs_block = model.observations.first();
        if let Some(obs) = obs_block {
            eprintln!("profile: using observation model '{}' from IR", obs.name);
            let projection = if flow_name.is_some() {
                sim::inference::multi_stream_obs::StreamProjection::FlowSum(flow_indices.to_vec())
            } else {
                sim::inference::multi_stream_obs::StreamProjection::from_ir(
                    &obs.projection, &compiled, &obs.name,
                ).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
            };
            Arc::new(MultiStreamObsModel::new(
                vec![StreamSpec {
                    projection,
                    ir_model: obs.clone(),
                    observations: observations.iter().map(|o| o.value).collect(),
                    obs_times: observations.iter().map(|o| o.time).collect(),
                }],
                compiled.clone(),
            ).unwrap_or_else(|e| {
                eprintln!("error: observation model construction failed: {:?}", e);
                std::process::exit(1);
            }))
        } else {
            eprintln!("error: model has no observations block");
            std::process::exit(1);
        }
    };

    // Build Cartesian product of all focal grids.
    let mut grid_points: Vec<Vec<(usize, f64)>> = vec![vec![]];
    for fg in &focal_grids {
        let mut expanded = Vec::new();
        for existing in &grid_points {
            for &val in &fg.values {
                let mut point = existing.clone();
                point.push((fg.param_idx, val));
                expanded.push(point);
            }
        }
        grid_points = expanded;
    }

    // ── Build typed CAS inputs ─────────────────────────────────────────
    //
    // ProfileInputs encapsulates every content-bearing input. inner_hash
    // (seed-free) drives the multi-seed umbrella; per-seed content_hash
    // = compose_with_replicate(inner, "seed", seed) — same as a
    // standalone --seed N invocation, so cache lookup is uniform.
    let model_hash = crate::hashing::model_hash(&model_json);
    let base_params_hash = {
        let mut lines: Vec<String> = model.parameters.iter()
            .map(|p| format!("{}={}", p.name,
                p.value.unwrap_or(base_params[compiled.param_index[p.name.as_str()]])))
            .collect();
        lines.sort();
        ContentHash::from_bytes(lines.join("\n").as_bytes()).full().to_string()
    };
    let grid_spec: Vec<GridAxis> = focal_grids.iter().map(|fg| GridAxis {
        param: fg.name.clone(),
        values: fg.values.clone(),
    }).collect();

    // Resolve seeds. --seeds wins; default is the single --seed.
    let seeds: Vec<u64> = match &a.seeds {
        Some(spec) => spec.expand(),
        None => vec![seed_base],
    };
    if seeds.is_empty() {
        eprintln!("error: --seeds expanded to empty list");
        std::process::exit(1);
    }

    let argv: Vec<String> = std::env::args().collect();
    let root = output_root(None, None);
    let stem = crate::hashing::path_stem_slug(&ir_path);

    let template_inputs = ProfileInputs {
        model_path: ir_path.clone(),
        stem: stem.clone(),
        model_hash: model_hash.clone(),
        base_params_hash,
        focal_grid: grid_spec,
        fixed: a.fixed.clone(),
        if2_config: ProfileIf2Config {
            n_particles, n_iterations, cooling, dt, n_starts,
        },
        starts_from_lineage: None,
        seed: seeds[0],   // overwritten per-seed below
    };

    // ── Layout: single seed flat, multi-seed under replicates/ ────────
    let multi_seed = seeds.len() > 1;
    let replicate_set: Option<ReplicateSet> = if multi_seed {
        Some(ReplicateSet {
            inner_hash: template_inputs.inner_hash(),
            dim_name:   "seed".to_string(),
            keys:       seeds.iter().map(|s| format!("seed_{}", s)).collect(),
            child_kind: "profile".to_string(),
        })
    } else {
        None
    };
    let umbrella_dir: Option<PathBuf> = if let Some(rset) = &replicate_set {
        let parent_hash = rset.parent_hash();
        let dirname = match &stem {
            Some(s) if !s.is_empty() => format!("{}-{}", s, parent_hash.short()),
            _ => parent_hash.short().to_string(),
        };
        let dir = root.join("profiles").join(dirname);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("error: cannot create {}: {}", dir.display(), e);
            std::process::exit(1);
        }
        let umbrella_run = Run {
            hash:              parent_hash.full().to_string(),
            version:           crate::version::VERSION_SHORT.to_string(),
            created_at:        crate::cas::iso8601_utc(std::time::SystemTime::now()),
            argv:              argv.clone(),
            wall_time_seconds: 0.0,
            label:             label_arg.clone(),
            kind:              rset.run_kind(),
        };
        if let Err(e) = umbrella_run.write(&dir) {
            eprintln!("warning: could not write umbrella run.json: {}", e);
        }
        eprintln!("profile (multi-seed, {} replicates): {}", seeds.len(), dir.display());
        Some(dir)
    } else {
        None
    };

    // Per-seed directories + content hashes (the latter populates
    // FitStageMeta.parent_profile_hash on each leaf start_run.json).
    let mut seed_dirs: Vec<PathBuf> = Vec::with_capacity(seeds.len());
    let mut per_seed_hashes: Vec<String> = Vec::with_capacity(seeds.len());
    for &seed in &seeds {
        let inputs_seed = ProfileInputs { seed, ..template_inputs.clone() };
        let dir = match (&replicate_set, &umbrella_dir) {
            (Some(rset), Some(parent)) => rset.child_dir(parent, &format!("seed_{}", seed)),
            _ => inputs_seed.cas_path(&root),
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("error: cannot create {}: {}", dir.display(), e);
            std::process::exit(1);
        }
        if !multi_seed {
            eprintln!("profile tree: {}", dir.display());
        }
        let profile_run = Run {
            hash:              inputs_seed.content_hash().full().to_string(),
            version:           crate::version::VERSION_SHORT.to_string(),
            created_at:        crate::cas::iso8601_utc(std::time::SystemTime::now()),
            argv:              argv.clone(),
            wall_time_seconds: 0.0,
            label:             if multi_seed { None } else { label_arg.clone() },
            kind:              inputs_seed.run_kind(),
        };
        if let Err(e) = profile_run.write(&dir) {
            eprintln!("warning: could not write profile run.json: {}", e);
        }
        // focal.toml per grid point inside this seed's tree.
        for (gi, point) in grid_points.iter().enumerate() {
            let point_dir = profile_point_dir(&dir, gi);
            if let Err(e) = std::fs::create_dir_all(&point_dir) {
                eprintln!("warning: cannot create {}: {}", point_dir.display(), e);
                continue;
            }
            let focal_toml_path = point_dir.join("focal.toml");
            if focal_toml_path.exists() { continue; }
            let mut body = String::from("# Pinned focal parameter values for this grid point.\n\n");
            for (fg, &(_, val)) in focal_grids.iter().zip(point.iter()) {
                body.push_str(&format!("{} = {}\n", fg.name, val));
            }
            let _ = std::fs::write(&focal_toml_path, body);
        }
        per_seed_hashes.push(inputs_seed.content_hash().full().to_string());
        seed_dirs.push(dir);
    }

    let total_jobs = grid_points.len() * n_starts * seeds.len();
    let dim_str = focal_grids.iter()
        .map(|fg| format!("{}={}", fg.name, fg.values.len()))
        .collect::<Vec<_>>().join(" × ");
    eprintln!("profile: {} grid ({}) × {} starts × {} seeds = {} IF2 runs ({} particles × {} iter each)",
        grid_points.len(), dim_str, n_starts, seeds.len(), total_jobs,
        n_particles, n_iterations);

    // ── Progress + cache scan ─────────────────────────────────────────
    let mp = MultiProgress::with_draw_target(crate::progress::draw_target());
    let overall_style = ProgressStyle::with_template(
        "  {prefix:>12} {bar:40.cyan/dim} {pos:>3}/{len:3} {msg}"
    ).unwrap().progress_chars("━╸─");
    let overall_pb = mp.add(ProgressBar::new(total_jobs as u64));
    overall_pb.set_style(overall_style);
    overall_pb.set_prefix("profile");
    let plain = crate::progress::is_plain();
    let progress_throttle = Mutex::new(crate::progress::Throttle::default());
    if plain {
        log::info!("profile: {} grid points × {} starts × {} seeds = {} jobs",
            grid_points.len(), n_starts, seeds.len(), total_jobs);
    }

    // Job tuple: (seed_idx, grid_idx, start_idx). Cache hit if the
    // start_dir under this seed's profile tree has a parseable run.json.
    let jobs: Vec<(usize, usize, usize)> = (0..seeds.len())
        .flat_map(|seed_idx| (0..grid_points.len())
            .flat_map(move |gi| (0..n_starts).map(move |si| (seed_idx, gi, si))))
        .collect();

    let mut cached: Vec<(usize, usize, usize)> = Vec::new();
    let mut remaining: Vec<(usize, usize, usize)> = Vec::new();
    for &(seed_idx, gi, si) in &jobs {
        let start_dir = profile_point_start_dir(&seed_dirs[seed_idx], gi, si);
        if Run::read(&start_dir).is_ok() {
            cached.push((seed_idx, gi, si));
        } else {
            remaining.push((seed_idx, gi, si));
        }
    }
    if !cached.is_empty() {
        eprintln!("profile: {} of {} starts already cached — resuming",
            cached.len(), total_jobs);
        overall_pb.inc(cached.len() as u64);
    }

    if parallel > 0 {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(parallel)
            .build_global();
    }

    // Throttled rollup rewrites: per-seed profile.tsv (1s throttle) and
    // (multi-seed only) the cross-seed summary.tsv (2s throttle, since
    // it reads N seeds' rollups). Last-completion-wins.
    let rollup_throttle = Mutex::new(std::time::Instant::now()
        - std::time::Duration::from_secs(10));
    let summary_throttle = Mutex::new(std::time::Instant::now()
        - std::time::Duration::from_secs(10));
    let start_time = std::time::Instant::now();

    let focal_names_ordered: Vec<String> =
        focal_grids.iter().map(|fg| fg.name.clone()).collect();

    // ── Run remaining jobs in parallel ──────────────────────────────
    remaining.par_iter().for_each(|&(seed_idx, grid_idx, start_idx)| {
        let process = Arc::clone(&process);
        let obs_model_obj = Arc::clone(&obs_model_obj);
        let if2_params = Arc::clone(&if2_params);
        let focal_values: Vec<f64> = grid_points[grid_idx].iter().map(|&(_, v)| v).collect();
        let seed = seeds[seed_idx];

        // Pin focal parameters
        let mut params = base_params.clone();
        for &(idx, val) in &grid_points[grid_idx] {
            params[idx] = val;
        }

        let config = IF2Config {
            n_particles, n_iterations,
            cooling_fraction: cooling, cooling_target_iters: n_iterations, dt,
            t_start: process.compiled.model.simulation.t_start,
            simplex_groups: vec![],
            skip_first_obs_from_loglik: false,
        };
        // job_seed derives from this REPLICATE's seed, not a global
        // seed_base — different seeds in --seeds drive distinct IF2
        // noise (the whole point of multi-seed sensitivity).
        let job_seed = seed ^ (grid_idx as u64 * 1000 + start_idx as u64);
        let job_t0 = std::time::Instant::now();

        let result = run_if2(
            &*process, &*obs_model_obj, &params, &if2_params, &config, job_seed,
        );
        let elapsed = job_t0.elapsed().as_secs_f64();

        let seed_dir = &seed_dirs[seed_idx];
        let start_dir = profile_point_start_dir(seed_dir, grid_idx, start_idx);
        if let Err(e) = std::fs::create_dir_all(&start_dir) {
            eprintln!("warning: cannot create {}: {}", start_dir.display(), e);
            return;
        }

        let (final_loglik, mle_params): (f64, Vec<f64>) = match result {
            Ok(r) => (r.final_loglik, r.mle),
            Err(_) => (f64::NEG_INFINITY, params.clone()),
        };

        let mle_toml = render_mle_toml(&if2_params, &focal_values,
            &focal_grids.iter().map(|fg| fg.name.as_str()).collect::<Vec<_>>(),
            &mle_params, final_loglik);
        let _ = std::fs::write(start_dir.join("mle.toml"), mle_toml);

        // Per-start run.json. parent_profile_hash references THIS
        // seed's profile content hash (not the umbrella's), so leaves
        // walk back to their per-seed parent regardless of single- vs
        // multi-seed layout.
        let parent_profile_hash = &per_seed_hashes[seed_idx];
        let start_hash_input = format!(
            "{}|point={}|start={}|seed={}",
            parent_profile_hash, grid_idx, start_idx, job_seed,
        );
        let start_hash = ContentHash::from_bytes(start_hash_input.as_bytes())
            .full().to_string();
        let start_run = Run {
            hash: start_hash,
            version: crate::version::VERSION_SHORT.to_string(),
            created_at: crate::cas::iso8601_utc(std::time::SystemTime::now()),
            argv: argv.clone(),
            wall_time_seconds: elapsed,
            label: None,
            kind: RunKind::FitStage(FitStageMeta {
                fit_hash: String::new(),
                stage: "if2".to_string(),
                method: "if2".to_string(),
                seed: job_seed,
                n_chains: 1,
                algorithm: serde_json::json!({
                    "particles":  n_particles,
                    "iterations": n_iterations,
                    "cooling":    cooling,
                    "dt":         dt,
                }),
                best_loglik: if final_loglik.is_finite() { Some(final_loglik) } else { None },
                best_chain:  Some(0),
                starts_from: None,
                derived_from: None,
                parent_profile_hash: Some(parent_profile_hash.clone()),
                profile_point_idx:   Some(grid_idx),
                profile_start_idx:   Some(start_idx),
            }),
        };
        if let Err(e) = start_run.write(&start_dir) {
            eprintln!("warning: could not write {}/run.json: {}",
                start_dir.display(), e);
        }

        // Progress tick.
        overall_pb.inc(1);
        if plain {
            let done = overall_pb.position();
            let ready = progress_throttle.lock()
                .map(|mut t| t.ready()).unwrap_or(true);
            if ready || done == total_jobs as u64 {
                log::info!("profile: {}/{} jobs complete", done, total_jobs);
            }
        }

        // Per-seed rollup (throttled).
        let should_rewrite = {
            let mut last = rollup_throttle.lock().unwrap();
            let now = std::time::Instant::now();
            if now.duration_since(*last) >= std::time::Duration::from_secs(1) {
                *last = now;
                true
            } else { false }
        };
        if should_rewrite {
            if let Err(e) = rewrite_rollup(seed_dir, &focal_names_ordered,
                &if2_params, grid_points.len()) {
                eprintln!("warning: rollup rewrite failed: {}", e);
            }
        }

        // Cross-seed summary (throttled, multi-seed only).
        if multi_seed {
            let should_summary = {
                let mut last = summary_throttle.lock().unwrap();
                let now = std::time::Instant::now();
                if now.duration_since(*last) >= std::time::Duration::from_secs(2) {
                    *last = now;
                    true
                } else { false }
            };
            if should_summary {
                if let Some(parent) = &umbrella_dir {
                    if let Err(e) = write_cross_seed_summary(
                        parent, &seed_dirs, &focal_names_ordered, &if2_params)
                    {
                        eprintln!("warning: summary rewrite failed: {}", e);
                    }
                }
            }
        }
    });

    overall_pb.finish_with_message("done");

    // Final per-seed rollup rewrites + cross-seed summary (unthrottled).
    for seed_dir in &seed_dirs {
        if let Err(e) = rewrite_rollup(seed_dir, &focal_names_ordered,
            &if2_params, grid_points.len())
        {
            eprintln!("warning: final rollup rewrite failed: {}", e);
        }
    }
    if multi_seed {
        if let Some(parent) = &umbrella_dir {
            if let Err(e) = write_cross_seed_summary(
                parent, &seed_dirs, &focal_names_ordered, &if2_params)
            {
                eprintln!("warning: final summary rewrite failed: {}", e);
            }
        }
    }

    // Patch each per-seed (and umbrella) run.json with total wall time.
    let total_wall = start_time.elapsed().as_secs_f64();
    for seed_dir in &seed_dirs {
        if let Ok(mut pr) = Run::read(seed_dir) {
            pr.wall_time_seconds = total_wall;
            let _ = pr.write(seed_dir);
        }
    }
    if let Some(parent) = &umbrella_dir {
        if let Ok(mut pr) = Run::read(parent) {
            pr.wall_time_seconds = total_wall;
            let _ = pr.write(parent);
        }
    }

    // Mirror the user-facing TSV. Multi-seed → cross-seed summary
    // (the artifact you asked for when you ran with --seeds);
    // single-seed → per-seed profile.tsv (legacy behavior).
    let mirror_src: Option<PathBuf> = if multi_seed {
        umbrella_dir.as_ref().map(|d| d.join("summary.tsv"))
    } else {
        Some(seed_dirs[0].join("profile.tsv"))
    };
    if let Some(ref path) = output_tsv_path {
        if let Some(src) = mirror_src.as_ref() {
            if src.exists() {
                match std::fs::copy(src, path) {
                    Ok(_) => eprintln!("written to {}", path),
                    Err(e) => eprintln!("warning: could not copy {} to {}: {}",
                        src.display(), path, e),
                }
            }
        }
    } else if let Some(src) = mirror_src.as_ref() {
        eprintln!("output: {}", src.display());
    }
}

/// Render a per-start MLE TOML file. Human-readable; also the format
/// `rewrite_rollup` reads back to reconstruct the rollup.
fn render_mle_toml(
    if2_params: &[sim::inference::if2::EstimatedParam],
    focal_values: &[f64],
    focal_names: &[&str],
    mle: &[f64],
    final_loglik: f64,
) -> String {
    let mut body = String::new();
    body.push_str("# Per-start MLE for one profile grid point.\n\n");
    body.push_str(&format!("final_loglik = {}\n\n", final_loglik));
    body.push_str("[focal]\n");
    for (name, v) in focal_names.iter().zip(focal_values.iter()) {
        body.push_str(&format!("{} = {}\n", name, v));
    }
    body.push_str("\n[mle]\n");
    for spec in if2_params.iter() {
        body.push_str(&format!("{} = {}\n", spec.name, mle[spec.index]));
    }
    body
}

/// Scan the per-start CAS tree and rewrite `profile.tsv` as the
/// derived rollup. One row per grid point, each row the winning start
/// (max final_loglik) across `n_starts`. Written atomically via
/// tmp-then-rename so concurrent rollups (from racing threads) never
/// expose a truncated intermediate.
fn rewrite_rollup(
    profile_dir: &Path,
    focal_names: &[String],
    if2_params: &[sim::inference::if2::EstimatedParam],
    n_grid_points: usize,
) -> std::io::Result<()> {
    // For each grid point, find the winning start by scanning its
    // start_{k}/ subdirs for mle.toml. If no starts have finished yet
    // for this point, skip the row (partial rollup — consumers see
    // only completed points).
    let mut rows: Vec<RollupRow> = Vec::new();
    for gi in 0..n_grid_points {
        let point_dir = profile_point_dir(profile_dir, gi);
        let Ok(dir_iter) = std::fs::read_dir(&point_dir) else { continue; };

        let mut best: Option<ParsedMle> = None;
        let mut wall_time_sum: f64 = 0.0;
        let mut best_start: Option<usize> = None;
        for entry in dir_iter.flatten() {
            let fname = entry.file_name();
            let name = fname.to_string_lossy();
            let Some(start_idx_str) = name.strip_prefix("start_") else { continue; };
            let Ok(start_idx) = start_idx_str.parse::<usize>() else { continue; };
            let start_dir = entry.path();

            // Use run.json's wall_time_seconds for summation. Skip
            // starts with missing/broken run.json — they're incomplete.
            let Ok(start_run) = Run::read(&start_dir) else { continue; };
            wall_time_sum += start_run.wall_time_seconds;

            let mle_path = start_dir.join("mle.toml");
            let Ok(mle_text) = std::fs::read_to_string(&mle_path) else { continue; };
            let Some(parsed) = parse_mle_toml(&mle_text, if2_params, focal_names) else { continue; };
            match &best {
                Some(b) if parsed.final_loglik <= b.final_loglik => {}
                _ => {
                    best = Some(parsed);
                    best_start = Some(start_idx);
                }
            }
        }

        if let (Some(best), Some(best_start)) = (best, best_start) {
            let _ = gi;  // order preserved by outer loop; field elided.
            rows.push(RollupRow {
                focal_values: best.focal_values,
                best_loglik: best.final_loglik,
                best_start_idx: best_start,
                mle: best.mle,
                wall_time_sum,
            });
        }
    }

    // Render.
    let mut body = String::new();
    body.push_str(&format!("# {}\n", crate::version::VERSION));
    body.push_str(&format!("# total_points={} completed={}\n",
        n_grid_points, rows.len()));
    for name in focal_names { body.push_str(&format!("{}\t", name)); }
    body.push_str("best_loglik\tbest_start_idx\twall_time_seconds");
    for spec in if2_params.iter() { body.push_str(&format!("\t{}", spec.name)); }
    body.push('\n');
    for row in &rows {
        for v in &row.focal_values { body.push_str(&format!("{:.4}\t", v)); }
        body.push_str(&format!("{:.4}\t{}\t{:.3}",
            row.best_loglik, row.best_start_idx, row.wall_time_sum));
        for spec in if2_params.iter() {
            body.push_str(&format!("\t{:.6}", row.mle[spec.index]));
        }
        body.push('\n');
    }

    // Atomic write.
    let final_path = profile_dir.join("profile.tsv");
    let tmp_path = profile_dir.join("profile.tsv.tmp");
    std::fs::write(&tmp_path, body)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

struct RollupRow {
    focal_values: Vec<f64>,
    best_loglik: f64,
    best_start_idx: usize,
    mle: Vec<f64>,
    wall_time_sum: f64,
}

struct ParsedMle {
    final_loglik: f64,
    focal_values: Vec<f64>,
    mle: Vec<f64>,
}

fn parse_mle_toml(
    text: &str,
    if2_params: &[sim::inference::if2::EstimatedParam],
    focal_names: &[String],
) -> Option<ParsedMle> {
    let doc: toml::Value = toml::from_str(text).ok()?;
    let final_loglik = toml_as_f64(doc.get("final_loglik")?)?;
    let focal = doc.get("focal")?.as_table()?;
    let mle = doc.get("mle")?.as_table()?;

    // Extract focal values in the caller's declared order (the column
    // order of the rollup TSV header), not in TOML key order.
    let mut focal_values: Vec<f64> = Vec::with_capacity(focal_names.len());
    for name in focal_names {
        let v = focal.get(name).and_then(toml_as_f64)?;
        focal_values.push(v);
    }

    let mle_len = if2_params.iter().map(|s| s.index).max().unwrap_or(0) + 1;
    let mut mle_values: Vec<f64> = vec![0.0; mle_len];
    for spec in if2_params.iter() {
        if let Some(v) = mle.get(&spec.name).and_then(toml_as_f64) {
            if mle_values.len() <= spec.index {
                mle_values.resize(spec.index + 1, 0.0);
            }
            mle_values[spec.index] = v;
        }
    }

    Some(ParsedMle { final_loglik, focal_values, mle: mle_values })
}

/// Accept TOML numeric values whether they serialised as Integer
/// (`R0 = 50`) or Float (`R0 = 50.0`). `toml::Value::as_float()`
/// returns `None` for Integers, which would silently drop any focal
/// value that happened to be a whole number.
fn toml_as_f64(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Float(f)   => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// Cross-seed aggregator. Reads each per-seed `profile.tsv` and emits
/// `summary.tsv` at the umbrella directory: one row per grid point
/// with mean / sd / min / max of `best_loglik` across seeds plus
/// per-MLE-column mean/sd. High `sd_loglik` at a grid point flags
/// stochastic IF2 instability — that cell's MLE is not trustworthy
/// from a single chain.
///
/// Atomic write (tmp-then-rename) so concurrent throttled rewrites
/// from the rayon pool can't expose a half-written summary.
fn write_cross_seed_summary(
    umbrella_dir: &Path,
    seed_dirs: &[PathBuf],
    focal_names: &[String],
    if2_params: &[sim::inference::if2::EstimatedParam],
) -> std::io::Result<()> {
    use std::collections::BTreeMap;

    // Map: focal-values key (as the canonical TSV-column strings, so
    // grid points with identical floats group together regardless of
    // formatting) → list of (best_loglik, mle_vec) per seed.
    let mut by_grid: BTreeMap<Vec<String>, Vec<(f64, Vec<f64>)>> = BTreeMap::new();
    let mle_len = if2_params.iter().map(|s| s.index).max().unwrap_or(0) + 1;

    for seed_dir in seed_dirs {
        let path = seed_dir.join("profile.tsv");
        let Ok(text) = std::fs::read_to_string(&path) else { continue; };
        for line in text.lines() {
            if line.starts_with('#') { continue; }
            let cols: Vec<&str> = line.split('\t').collect();
            // Header row uses literal column names; skip it.
            if cols.get(focal_names.len()).map(|s| *s) == Some("best_loglik") {
                continue;
            }
            // Layout: focal_1 ... focal_N | best_loglik | best_start_idx |
            //         wall_time_seconds | mle_param_1 ... mle_param_M
            if cols.len() < focal_names.len() + 3 + if2_params.len() { continue; }

            let focal_key: Vec<String> = cols[..focal_names.len()]
                .iter().map(|s| s.trim().to_string()).collect();
            let Ok(best_loglik) = cols[focal_names.len()].parse::<f64>() else { continue; };

            let mle_start = focal_names.len() + 3;
            let mut mle = vec![f64::NAN; mle_len];
            for (i, spec) in if2_params.iter().enumerate() {
                if let Some(s) = cols.get(mle_start + i) {
                    if let Ok(v) = s.parse::<f64>() {
                        if spec.index < mle.len() {
                            mle[spec.index] = v;
                        }
                    }
                }
            }
            by_grid.entry(focal_key).or_default().push((best_loglik, mle));
        }
    }

    let mut body = String::new();
    body.push_str(&format!("# {} cross-seed summary across {} seeds\n",
        crate::version::VERSION, seed_dirs.len()));
    body.push_str(&format!("# n_grid_points={} n_seeds={}\n",
        by_grid.len(), seed_dirs.len()));
    for name in focal_names { body.push_str(&format!("{}\t", name)); }
    body.push_str("n_seeds\tmean_loglik\tsd_loglik\tmin_loglik\tmax_loglik");
    for spec in if2_params.iter() {
        body.push_str(&format!("\t{}_mean\t{}_sd", spec.name, spec.name));
    }
    body.push('\n');

    for (focal_key, samples) in &by_grid {
        for v in focal_key { body.push_str(&format!("{}\t", v)); }
        let n = samples.len();
        let logliks: Vec<f64> = samples.iter().map(|(ll, _)| *ll)
            .filter(|x| x.is_finite()).collect();
        let n_finite = logliks.len();
        let (mean_ll, sd_ll, min_ll, max_ll) = summary_stats(&logliks);
        body.push_str(&format!("{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}",
            n_finite.max(n), mean_ll, sd_ll, min_ll, max_ll));
        for spec in if2_params.iter() {
            let vals: Vec<f64> = samples.iter()
                .filter_map(|(_, mle)| mle.get(spec.index).copied())
                .filter(|x| x.is_finite()).collect();
            let (m, s, _, _) = summary_stats(&vals);
            body.push_str(&format!("\t{:.6}\t{:.6}", m, s));
        }
        body.push('\n');
    }

    let final_path = umbrella_dir.join("summary.tsv");
    let tmp_path = umbrella_dir.join("summary.tsv.tmp");
    std::fs::write(&tmp_path, body)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// (mean, sd, min, max) of a slice. Empty input returns NaN/0/inf/-inf;
/// callers should treat NaN as "no data" not "zero data."
fn summary_stats(xs: &[f64]) -> (f64, f64, f64, f64) {
    if xs.is_empty() {
        return (f64::NAN, 0.0, f64::INFINITY, f64::NEG_INFINITY);
    }
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let sd = if xs.len() > 1 {
        (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
    } else { 0.0 };
    let min = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mean, sd, min, max)
}

