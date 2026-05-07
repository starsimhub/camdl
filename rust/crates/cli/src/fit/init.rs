//! Chain / per-cell init strategies.
//!
//! Three modes today, dispatched via the `init_method` field on
//! `[stages.X]` (and the `--init` CLI override):
//!
//! - **Single** — every chain starts at `config.estimated_params[*].initial`
//!   (i.e. `[estimate].start =` or its fallback). Chains differ only by
//!   IF2's per-chain RNG. Useful for refine stages, single-chain runs,
//!   reproducibility-critical tests, and deterministic NLopt at a known
//!   seed point.
//! - **Uniform** — per-chain uniform random draw within natural-scale
//!   bounds. Legacy mode; equivalent to `Lhs` for `Logit`/`None`
//!   parameters but worse for `Log`-typed parameters at low chain count
//!   (clumps in linear space). Kept for reproducibility of pre-LHS results.
//! - **Lhs** — Latin-hypercube stratified sampling, **scale-aware via
//!   `Transform`**. For Log-typed params (rates, positive quantities) LHS
//!   spans `[ln(lo), ln(hi)]` and exponentiates back, so a single LHS pass
//!   covers orders of magnitude rather than concentrating mass near `hi`.
//!   For Logit-typed params (probabilities) LHS spans `[lo, hi]` linearly.
//!   For untransformed params LHS spans `[lo, hi]`. **This is the default**
//!   across IF2 / PGAS / PMMH / NLopt multi-chain stages.
//!
//! Filed as gh#42. Motivation: downstream typhoid SIRC fit found
//! 30 LHS-drawn chains at chain_binomial backend reach a basin
//! 80,542 nats better than 8 uniform-random-start chains, holding
//! everything else equal. Single-point starts (and clumpy uniform
//! starts at low N) miss basins; LHS gives stratified coverage at
//! the same chain count.

use sim::inference::types::{EstimatedParam, Transform};
use sim::rng::StatefulRng;

use crate::util::derive_chain_seed;

/// How chain (or per-cell) starting points are drawn.
///
/// Default is `Lhs` — see the `Default` impl below for rationale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum InitMethod {
    Single,
    Uniform,
    Lhs,
    /// Pull per-chain starts from the top-K rows of a `camdl survey`
    /// landscape. Requires sibling fields `survey_path` (CAS dir) and
    /// `survey_top_k_n` (defaults to `chains`) on the same stage. The
    /// reader cross-checks the survey's `run.json` against the fit's
    /// resolved inputs (model_hash, data_hashes, [fixed] superset,
    /// estimate-set subset) and filters the landscape rows to fit's
    /// bounds before ranking. See gh#51 +
    /// `docs/dev/proposals/2026-05-07-survey-top-k-init.md`.
    #[serde(rename = "survey_top_k")]
    #[clap(name = "survey_top_k")]
    SurveyTopK,
}

impl Default for InitMethod {
    /// LHS — Latin-hypercube stratified sampling, scale-aware via the
    /// parameter's `Transform`. Strictly better basin coverage than
    /// `Uniform` at the chain counts we typically run (gh#42 typhoid
    /// evidence: 30 LHS-drawn chains reach a basin 80,542 nats better
    /// than 8 uniform-random-start chains, holding everything else
    /// equal). The legacy `Uniform` default existed for backward
    /// compat with v1 scout's inline random-start loop; LHS supersedes
    /// it and is now the default across IF2 / PGAS / PMMH / NLopt
    /// multi-chain stages.
    fn default() -> Self { InitMethod::Lhs }
}

impl std::str::FromStr for InitMethod {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "single"        => Ok(InitMethod::Single),
            "uniform"       => Ok(InitMethod::Uniform),
            "lhs"           => Ok(InitMethod::Lhs),
            "survey_top_k"  => Ok(InitMethod::SurveyTopK),
            other => Err(format!(
                "unknown init_method '{}': expected one of \
                 single, uniform, lhs, survey_top_k",
                other)),
        }
    }
}

impl std::fmt::Display for InitMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            InitMethod::Single      => "single",
            InitMethod::Uniform     => "uniform",
            InitMethod::Lhs         => "lhs",
            InitMethod::SurveyTopK  => "survey_top_k",
        })
    }
}

/// Build N chain starts according to `method`. Returns `None` when
/// caller should pass `None` to `run_chains_with_per_chain_params`
/// (i.e. all chains use `config.estimated_params` directly).
///
/// `seed` is the fit's top-level seed; per-chain RNGs derive from it
/// via `derive_chain_seed`. LHS uses one RNG seeded from `seed` for
/// the permutations + per-stratum jitters (so adding a chain reshuffles
/// all stratum assignments — that's the price of stratification).
pub fn build_chain_starts(
    method: InitMethod,
    base: &[EstimatedParam],
    n_chains: usize,
    seed: u64,
) -> Option<Vec<Vec<EstimatedParam>>> {
    match method {
        InitMethod::Single => None,
        InitMethod::Uniform => {
            if n_chains < 2 { return None; }
            Some(build_uniform_chain_starts(base, n_chains, seed))
        }
        InitMethod::Lhs => {
            if n_chains < 2 { return None; }
            Some(build_lhs_chain_starts(base, n_chains, seed))
        }
        InitMethod::SurveyTopK => {
            // Routed through `build_chain_starts_from_survey` at the
            // stage callsite, where the fit-level cross-check context
            // is in scope. Reaching this branch is a wiring bug, not
            // a user-input problem — panic in debug, return None in
            // release so the caller falls back to base specs (rather
            // than mid-fit panicking on a dispatch oversight).
            debug_assert!(false,
                "InitMethod::SurveyTopK reached build_chain_starts; \
                 callsite must dispatch via build_chain_starts_from_survey");
            None
        }
    }
}

/// Resolve `method` to per-chain full parameter vectors, for routines
/// (PGAS, PMMH) that work with `Vec<f64>` directly rather than the
/// IF2-shaped `Vec<EstimatedParam>`. Returns one full param-vector
/// per chain, with each `EstimatedParam`-listed index overwritten by
/// the per-chain draw and all other slots taken from `base_params`.
///
/// Returns `Ok(None)` when the caller should treat every chain as
/// starting from `base_params` directly (i.e. `Single`, or
/// `n_chains < 2`). Returns `Err` for `InitMethod::SurveyTopK` on
/// stages that don't yet plumb the survey cross-check context — v1
/// supports SurveyTopK on `Stage::IF2` only; PGAS/PMMH/NLopt/profile
/// are deferred to v2 (see proposal §"Stage scope — v1 vs v2").
pub fn build_chain_param_vecs(
    method: InitMethod,
    base_specs: &[EstimatedParam],
    base_params: &[f64],
    n_chains: usize,
    seed: u64,
) -> Result<Option<Vec<Vec<f64>>>, String> {
    if method == InitMethod::SurveyTopK {
        return Err(
            "init_method = \"survey_top_k\" is not yet supported on this \
             stage type; v1 supports it on IF2 only. PGAS / PMMH / NLopt / \
             profile support is deferred to v2 (see gh#51 §\"Stage scope \
             — v1 vs v2\"). Workaround: use init_method = \"lhs\" on this \
             stage, or run an IF2 scout first and chain via \
             starts_from = \"<scout>\".".to_string());
    }
    let per_chain = build_chain_starts(method, base_specs, n_chains, seed);
    Ok(per_chain.map(|chains| chains.iter().map(|chain| {
        let mut params = base_params.to_vec();
        for spec in chain { params[spec.index] = spec.initial; }
        params
    }).collect()))
}

/// Per-chain uniform random draw within natural-scale bounds. Chain 0
/// keeps the seeded start (reproducibility); chains 1..N draw fresh.
/// Equivalent to the previous `runner::build_random_chain_starts`
/// (kept as a free function here so the runner doesn't grow more init
/// strategies inline).
fn build_uniform_chain_starts(
    base: &[EstimatedParam],
    n_chains: usize,
    seed: u64,
) -> Vec<Vec<EstimatedParam>> {
    (0..n_chains).map(|chain_id| {
        let mut rng = StatefulRng::new(derive_chain_seed(seed, chain_id));
        base.iter().map(|spec| {
            let initial = if chain_id == 0 {
                spec.initial
            } else if spec.lower.is_finite() && spec.upper.is_finite() {
                spec.lower + rng.uniform() * (spec.upper - spec.lower)
            } else {
                spec.initial * (0.5 + rng.uniform())
            };
            EstimatedParam { initial, ..spec.clone() }
        }).collect()
    }).collect()
}

/// Latin-hypercube stratified starts, scale-aware via `Transform`.
///
/// Algorithm (textbook stratified LHS):
/// 1. For each parameter dim d, draw a random permutation π_d of `[0..n_chains)`.
/// 2. For chain k, dim d: `u_{k,d} = (π_d[k] + jitter) / n_chains`, with
///    `jitter ~ Uniform(0, 1)` — a uniform draw within stratum k's cell.
/// 3. Map `u_{k,d}` to natural-scale θ via the parameter's transform:
///    - `Transform::Log` and both bounds positive → exponential mapping
///      `θ = lo · (hi/lo)^u`. Equivalent to LHS in `[ln lo, ln hi]`.
///    - Otherwise (Logit, None, or pathological log bounds) → linear
///      `θ = lo + u · (hi - lo)`.
///
/// Unbounded params (lower or upper non-finite) fall back to a
/// `±50%` jitter around `spec.initial` — same fallback as
/// `build_uniform_chain_starts` for parity. LHS without finite bounds
/// is meaningless; flag with the validator if this matters in practice.
fn build_lhs_chain_starts(
    base: &[EstimatedParam],
    n_chains: usize,
    seed: u64,
) -> Vec<Vec<EstimatedParam>> {
    let n_params = base.len();
    let mut rng = StatefulRng::new(seed ^ 0x1f5_beef_u64);

    // Step 1+2: per-dim permutation, jitter within each stratum.
    // u[chain_id][param_id] is the [0,1] LHS coordinate.
    let mut u: Vec<Vec<f64>> = vec![vec![0.0; n_params]; n_chains];
    for d in 0..n_params {
        let mut perm: Vec<usize> = (0..n_chains).collect();
        // Fisher-Yates using the same RNG (deterministic given seed).
        for i in (1..n_chains).rev() {
            let j = (rng.uniform() * (i as f64 + 1.0)).floor() as usize;
            perm.swap(i, j.min(i));
        }
        for k in 0..n_chains {
            let jitter = rng.uniform();
            u[k][d] = (perm[k] as f64 + jitter) / n_chains as f64;
        }
    }

    // Step 3: map [0,1] LHS coord to natural-scale θ per Transform.
    (0..n_chains).map(|chain_id| {
        base.iter().enumerate().map(|(d, spec)| {
            let initial = lhs_map_to_natural(spec, u[chain_id][d]);
            EstimatedParam { initial, ..spec.clone() }
        }).collect()
    }).collect()
}

/// Draw a single Transform-aware value within `[lo, hi]`, suitable for
/// the gh#34 start-fallback path: when an `[estimate]` entry has neither
/// `start =` nor a model-declared parameter `value`, we still need a
/// scalar to seed `model.parameters[i].value` with so that compile +
/// `validate_parameter_values` succeed. Downstream chain init can then
/// perturb from this base.
///
/// "Transform-aware" means: for `Log`-typed parameters with both bounds
/// strictly positive, draw uniformly in *log space* and exponentiate;
/// otherwise draw linearly in `[lo, hi]`. Replaces the legacy
/// bounds-midpoint heuristic (`(lo*hi).sqrt()` or `(lo+hi)/2`), which
/// was geometric-shape-aware via a positive-bounds proxy but ignored
/// the parameter's declared transform and gave the same point at every
/// seed.
///
/// Reproducibility: the per-parameter `u ∈ [0, 1]` is derived from
/// `(seed, param_name)` via a 64-bit hash, so re-running with the same
/// `seed` gives the same start, and two estimate entries with the same
/// bounds at the same seed get *different* draws (their names hash
/// differently). Same seed across runs ⇒ same fallback start; different
/// seeds ⇒ different fallback starts within `[lo, hi]`.
pub fn draw_start_in_bounds(
    lo: f64,
    hi: f64,
    log_scale: bool,
    seed: u64,
    param_name: &str,
) -> f64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    param_name.hash(&mut h);
    // Map the 64-bit hash into u ∈ (0, 1) — open interval, so the
    // log-scale branch's `(hi/lo).powf(u)` never lands exactly on a
    // bound. 53-bit mantissa is plenty.
    let u = ((h.finish() >> 11) as f64 + 0.5) / (1u64 << 53) as f64;

    if log_scale && lo > 0.0 && hi > 0.0 {
        lo * (hi / lo).powf(u)
    } else {
        lo + u * (hi - lo)
    }
}

// ── survey_top_k chain init (gh#51) ──────────────────────────────────

/// Fit-level context required to validate a survey artifact and resolve
/// fallbacks. Constructed at the runner side (where the fit's resolved
/// inputs are in scope) and passed to `build_chain_starts_from_survey`.
///
/// The borrows are short-lived — this struct exists only for the
/// duration of a single chain-init resolution.
pub struct SurveyFitContext<'a> {
    /// Full SHA-256 of the fit's resolved IR JSON. Must match the
    /// survey's `model_hash` exactly.
    pub model_hash: &'a str,
    /// Per-stream content hashes of the fit's data files. Each stream
    /// the fit consumes must appear with a matching hash in the
    /// survey's `data_hashes`. Survey may reference *more* streams
    /// than the fit (e.g. survey held a covariate fixed and the fit
    /// drops it); those are ignored.
    pub data_hashes: &'a std::collections::HashMap<String, String>,
    /// Resolved `[fixed]` block from the fit. Survey's `[fixed]` must
    /// be a superset; differing-value at any shared key refuses.
    pub fixed: &'a std::collections::HashMap<String, f64>,
    /// Estimated-param names from the fit, in any order. Each must
    /// either appear in the survey's estimated-param column set, or
    /// fall back to the row's `base.initial` (typically the user's
    /// `[estimate].start` or its gh#34 uniform draw).
    pub estimate_names: &'a [String],
}

/// SHA-256 every data file referenced by `effective_obs`, returning the
/// stream-name → hex-hash map shape that `SurveyFitContext.data_hashes`
/// (and `SurveyMeta.data_hashes`) use for the cross-check. Centralised
/// here so the four-or-five fit-stage dispatch sites compute it
/// identically.
pub fn compute_data_hashes(
    effective_obs: &indexmap::IndexMap<String, String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let mut out = std::collections::HashMap::with_capacity(effective_obs.len());
    for (stream, path) in effective_obs {
        let bytes = std::fs::read(path).map_err(|e| format!(
            "cannot read data file `{}` for stream `{}`: {}", path, stream, e))?;
        out.insert(stream.clone(), crate::hashing::sha256_hex(&bytes));
    }
    Ok(out)
}

/// Result of `build_chain_starts_from_survey` — bundles the per-chain
/// `EstimatedParam` overrides with provenance info the caller needs
/// to populate `fit_state.toml.chain_init_source` and
/// `chain_starts.tsv`. Each chain's rank in the survey is its index
/// in `chains` plus 1 (chain 0 = rank-1, chain 1 = rank-2, ...).
pub struct SurveyTopKResult {
    pub chains: Vec<Vec<EstimatedParam>>,
    /// Full content hash of the survey CAS dir (`run.json.hash`).
    /// Embedded into provenance strings as
    /// `survey:<survey_hash>:top-<K>` (one per fit) and
    /// `survey:<survey_hash>:rank-<N>` (one per chain in
    /// `chain_starts.tsv`). Full hash, not short — short hashes
    /// collide and audit-survivable links must point at exactly one
    /// CAS dir.
    pub survey_hash: String,
}

/// Pull per-chain starts from the top-K rows of a `camdl survey`
/// landscape. See gh#51 +
/// `docs/dev/proposals/2026-05-07-survey-top-k-init.md` for the
/// design rationale.
///
/// Steps:
/// 1. Load `<survey_path>/run.json`; refuse unless `RunKind::Survey`.
/// 2. Cross-check `model_hash`, `data_hashes`, `[fixed]` superset,
///    estimate-set subset against `ctx`. Refuse on any mismatch with
///    a diagnostic naming the offending field.
/// 3. Read `<survey_path>/landscape.tsv` (skipping `#` comment lines).
/// 4. **Filter** rows: keep only those whose every parameter value
///    lies within the corresponding `base[i].lower / .upper` bound.
///    No clipping. Refuse if filtered count < `n_chains`. Warn if
///    filtered drops > 50% of original.
/// 5. **Rank** filtered rows by `loglik` desc; take top-`top_k`
///    (defaults to `n_chains` when `top_k_n` is `None`). v1 enforces
///    `top_k == n_chains` (strict K=chains; K > chains is v2).
/// 6. **SE-aware warn**: if the top-K decibans-spread is below
///    `max(30.0, 8 · σ_max · NATS_TO_DB)`, warn that the rank
///    ordering is uncertain at this measurement budget. Never refuse
///    on this — fits with noisy seeds still work.
///
/// For each top-K row, build a chain by cloning `base` and
/// overriding `initial` with the row's column value for every
/// estimated param the survey carried. Estimated params present in
/// the fit but absent from the survey (fit estimates ρ, survey held
/// it fixed) keep `base.initial` as the per-chain start.
pub fn build_chain_starts_from_survey(
    survey_path: &std::path::Path,
    top_k_n: Option<usize>,
    n_chains: usize,
    base: &[EstimatedParam],
    ctx: &SurveyFitContext,
) -> Result<SurveyTopKResult, String> {
    use crate::run_meta::{Run, RunKind};

    let top_k = top_k_n.unwrap_or(n_chains);
    if top_k != n_chains {
        return Err(format!(
            "init_method = \"survey_top_k\": v1 requires \
             survey_top_k_n == chains (got top_k_n = {}, chains = {}). \
             K > chains with stratified sub-sampling is deferred to v2 \
             — see gh#51 §\"Out of scope for v1\".",
            top_k, n_chains));
    }

    // Step 1: load run.json.
    let run = Run::read(survey_path).map_err(|e| format!(
        "init_method = \"survey_top_k\": cannot read run.json from {:?}: {}",
        survey_path, e))?;
    let survey_meta = match &run.kind {
        RunKind::Survey(m) => m,
        other => return Err(format!(
            "init_method = \"survey_top_k\": {:?} is a {:?} run, not a Survey run. \
             survey_path must point at a `camdl survey` CAS directory.",
            survey_path, std::mem::discriminant(other))),
    };

    // Step 2: cross-check.
    cross_check_survey(survey_meta, ctx)?;

    // Step 3: read + parse landscape.tsv.
    let landscape_path = survey_path.join("landscape.tsv");
    let raw = std::fs::read_to_string(&landscape_path).map_err(|e| format!(
        "init_method = \"survey_top_k\": cannot read {:?}: {}",
        landscape_path, e))?;
    let rows = parse_landscape_tsv(&raw, &survey_meta.estimated)
        .map_err(|e| format!("init_method = \"survey_top_k\": {}", e))?;
    let total_rows = rows.len();

    // Step 4: filter by fit bounds.
    let filtered: Vec<&LandscapeRow> = rows.iter().filter(|row| {
        base.iter().all(|spec| {
            match row.params.get(&spec.name) {
                Some(&v) => v >= spec.lower && v <= spec.upper,
                // Param not in survey → not a filter criterion (it'll
                // fall back to base.initial in step 6).
                None => true,
            }
        })
    }).collect();

    if filtered.len() < n_chains {
        return Err(format!(
            "init_method = \"survey_top_k\": survey has {} rows but only {} \
             fall within fit bounds, and chains = {}. Either widen fit's \
             bounds toward the surveyed region, or re-run the survey on \
             the narrower box.",
            total_rows, filtered.len(), n_chains));
    }
    if (filtered.len() as f64) < 0.5 * (total_rows as f64) {
        eprintln!("\x1b[33mwarning:\x1b[0m init_method = \"survey_top_k\" \
            discards {} of {} survey rows as outside fit bounds. The \
            fit will use the top-{} of the {} that remain, but most of \
            the survey's measurement budget is being thrown away. \
            Consider widening fit bounds or re-running the survey.",
            total_rows - filtered.len(), total_rows, top_k, filtered.len());
    }

    // Step 5: rank + take top-K.
    let mut ranked: Vec<&LandscapeRow> = filtered;
    ranked.sort_by(|a, b| {
        b.loglik.partial_cmp(&a.loglik).unwrap_or(std::cmp::Ordering::Equal)
    });
    let selected: &[&LandscapeRow] = &ranked[..top_k];

    // Step 6: SE-aware warn on rank noise.
    emit_top_k_se_warning(selected);

    // Step 7: assemble per-chain EstimatedParam vectors.
    let chains: Vec<Vec<EstimatedParam>> = selected.iter().map(|row| {
        base.iter().map(|spec| {
            let initial = row.params.get(&spec.name)
                .copied()
                .unwrap_or(spec.initial);
            EstimatedParam { initial, ..spec.clone() }
        }).collect()
    }).collect();

    Ok(SurveyTopKResult {
        chains,
        survey_hash: run.hash,
    })
}

/// One row of a survey landscape.tsv, parsed.
#[derive(Debug, Clone)]
struct LandscapeRow {
    params: std::collections::HashMap<String, f64>,
    loglik: f64,
    loglik_se: f64,
}

/// Parse `landscape.tsv` body. Skips `#` comment lines, reads the
/// header, then each data row. Recognised column-set: `<param>...
/// loglik loglik_se [mean_ess] n_replicates point_id`. Param columns
/// are matched against `survey_estimated`; remaining named columns
/// (loglik / loglik_se) are extracted explicitly.
fn parse_landscape_tsv(
    raw: &str,
    survey_estimated: &[String],
) -> Result<Vec<LandscapeRow>, String> {
    let mut lines = raw.lines().filter(|l| !l.trim_start().starts_with('#'));
    let header = lines.next()
        .ok_or_else(|| "landscape.tsv has no header row (only comments?)".to_string())?;
    let cols: Vec<&str> = header.split('\t').collect();
    let loglik_idx = cols.iter().position(|c| *c == "loglik")
        .ok_or_else(|| "landscape.tsv header missing `loglik` column".to_string())?;
    let loglik_se_idx = cols.iter().position(|c| *c == "loglik_se")
        .ok_or_else(|| "landscape.tsv header missing `loglik_se` column".to_string())?;
    // Param columns are the leading run of columns whose name matches
    // an entry in survey_estimated (their order matters for the survey
    // writer; we use their names for the lookup).
    let param_indices: Vec<(String, usize)> = survey_estimated.iter()
        .map(|name| {
            cols.iter().position(|c| *c == name)
                .map(|i| (name.clone(), i))
                .ok_or_else(|| format!(
                    "landscape.tsv header missing param column `{}` \
                     (declared in run.json `estimated`)", name))
        })
        .collect::<Result<_, _>>()?;

    let mut rows = Vec::new();
    for (line_no, line) in lines.enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != cols.len() {
            return Err(format!(
                "landscape.tsv data row {} has {} fields, expected {}",
                line_no + 1, fields.len(), cols.len()));
        }
        let parse = |i: usize, name: &str| -> Result<f64, String> {
            fields[i].parse::<f64>().map_err(|e| format!(
                "landscape.tsv data row {}: cannot parse `{}` field {:?}: {}",
                line_no + 1, name, fields[i], e))
        };
        let loglik = parse(loglik_idx, "loglik")?;
        let loglik_se = parse(loglik_se_idx, "loglik_se")?;
        let mut params = std::collections::HashMap::with_capacity(param_indices.len());
        for (name, idx) in &param_indices {
            params.insert(name.clone(), parse(*idx, name)?);
        }
        rows.push(LandscapeRow { params, loglik, loglik_se });
    }
    Ok(rows)
}

fn cross_check_survey(
    meta: &crate::run_meta::SurveyMeta,
    ctx: &SurveyFitContext<'_>,
) -> Result<(), String> {
    if meta.model_hash != ctx.model_hash {
        return Err(format!(
            "init_method = \"survey_top_k\": model_hash mismatch.\n  \
             survey: {}\n     fit: {}\nA model edit between survey and \
             fit invalidates the cross-check; re-run the survey on the \
             current model.",
            meta.model_hash, ctx.model_hash));
    }
    for (stream, fit_hash) in ctx.data_hashes {
        match meta.data_hashes.get(stream) {
            Some(survey_hash) if survey_hash == fit_hash => {}
            Some(survey_hash) => return Err(format!(
                "init_method = \"survey_top_k\": data_hashes mismatch on \
                 stream `{}`.\n  survey: {}\n     fit: {}",
                stream, survey_hash, fit_hash)),
            None => return Err(format!(
                "init_method = \"survey_top_k\": fit consumes data stream \
                 `{}` which the survey did not score against. Re-run the \
                 survey with this stream included.", stream)),
        }
    }
    for (name, &fit_value) in ctx.fixed {
        match meta.fixed.get(name) {
            Some(&survey_value) if (survey_value - fit_value).abs() < 1e-12 => {}
            Some(&survey_value) => return Err(format!(
                "init_method = \"survey_top_k\": [fixed].{} disagrees.\n  \
                 survey: {}\n     fit: {}\nFixed-value drift between \
                 survey and fit invalidates the seeded starts.",
                name, survey_value, fit_value)),
            None => return Err(format!(
                "init_method = \"survey_top_k\": fit's [fixed] must be a \
                 subset of survey's [fixed]; survey did not pin `{}` (the \
                 survey estimated it or left it free). Pin it in the \
                 survey, or remove it from fit's [fixed].", name)),
        }
    }
    let survey_estimated: std::collections::HashSet<&str> =
        meta.estimated.iter().map(|s| s.as_str()).collect();
    for name in ctx.estimate_names {
        // Fit-estimate params absent from the survey are fine (fall
        // back to base.initial). Fit-estimate params *fixed* by the
        // survey at a value that equals fit's expected start would
        // also be fine, but that's a degenerate case we don't need to
        // optimise for. The hard refusal is when the survey neither
        // estimated nor fixed the param — meaning it has no value at
        // all in the survey's parameter space. That can't happen for
        // a model the survey actually ran (every model parameter is
        // either estimated or fixed at survey time), so this loop is
        // mostly defensive.
        let _ = survey_estimated; // keep the set for future cross-checks
        let _ = name;
    }
    Ok(())
}

fn emit_top_k_se_warning(top_k: &[&LandscapeRow]) {
    use crate::evidence::NATS_TO_DB;
    if top_k.len() < 2 { return; }
    let logliks: Vec<f64> = top_k.iter().map(|r| r.loglik).collect();
    let ses: Vec<f64> = top_k.iter().map(|r| r.loglik_se).collect();
    let hi = logliks.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let lo = logliks.iter().copied().fold(f64::INFINITY, f64::min);
    let delta_db = (hi - lo) * NATS_TO_DB;
    let sigma_max = ses.iter().copied().fold(0.0_f64, f64::max);
    // Mirror the IF2 convergence-gate floor: rank ordering is
    // meaningful only when the spread exceeds the SE-aware threshold.
    // 30 dB matches `GateConfig::default().decibans_thresh`.
    let threshold_db = (30.0_f64).max(8.0 * sigma_max * NATS_TO_DB);
    if delta_db < threshold_db {
        eprintln!("\x1b[33mwarning:\x1b[0m init_method = \"survey_top_k\": \
            top-{} loglik spread = {:.1} dB is below the SE-aware threshold \
            ({:.1} dB; σ_max = {:.2} nats). Rank ordering is uncertain at \
            this measurement budget — chains seeded from rank-1 vs rank-{} \
            may not be in genuinely-different basins. Consider re-running \
            the survey with higher --eval-replicates.",
            top_k.len(), delta_db, threshold_db, sigma_max, top_k.len());
    }
}

/// Format `chain_init_source` for `fit_state.toml` — one line of
/// provenance describing where this stage's chain starts came from.
/// `lhs` / `single` / `uniform` for the in-process samplers,
/// `survey:<full-hash>:top-<K>` for the survey reader.
pub fn format_chain_init_source(
    method: InitMethod,
    survey_top_k: Option<&SurveyTopKResult>,
) -> String {
    if let Some(res) = survey_top_k {
        return format!("survey:{}:top-{}", res.survey_hash, res.chains.len());
    }
    match method {
        InitMethod::Single => "single".into(),
        InitMethod::Uniform => "uniform".into(),
        InitMethod::Lhs => "lhs".into(),
        InitMethod::SurveyTopK => {
            // SurveyTopKResult should have been provided. Defensive
            // fallback so a wiring bug doesn't write a corrupt
            // provenance string into fit_state.toml.
            "survey:<missing-result>:top-?".into()
        }
    }
}

/// Write `chain_starts.tsv` — sidecar audit-only artifact recording
/// the per-chain starting parameter vector and its provenance source
/// (e.g. `survey:<hash>:rank-1`, `lhs:chain-0`). Lives next to
/// `chain_evaluations.tsv` at the stage root. Emitted for every
/// init mode so an auditor can re-derive any chain's exact start
/// from a single TSV without reading inference-engine internals.
///
/// `per_chain_starts` is `None` for `InitMethod::Single` (every
/// chain at `base`) — that case writes one row per chain with the
/// base values and `source = "single"`. For ranked-survey mode the
/// caller supplies `survey_top_k` so each chain's source carries
/// `:rank-N` (1-indexed).
pub fn write_chain_starts_tsv(
    stage_dir: &std::path::Path,
    base: &[EstimatedParam],
    per_chain_starts: Option<&[Vec<EstimatedParam>]>,
    n_chains: usize,
    method: InitMethod,
    survey_top_k: Option<&SurveyTopKResult>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    let path = stage_dir.join("chain_starts.tsv");
    let tmp = path.with_extension("tsv.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        // Comment header — stable, machine-parseable.
        writeln!(f, "# camdl chain_starts; method={}; chains={}",
            method, n_chains)?;
        if let Some(res) = survey_top_k {
            writeln!(f, "# survey_hash={}", res.survey_hash)?;
        }
        // Header row.
        let mut cols = vec!["chain_id".to_string(), "source".to_string()];
        for spec in base { cols.push(spec.name.clone()); }
        writeln!(f, "{}", cols.join("\t"))?;
        for chain_id in 0..n_chains {
            let source = match (method, survey_top_k) {
                (InitMethod::SurveyTopK, Some(res)) =>
                    format!("survey:{}:rank-{}", res.survey_hash, chain_id + 1),
                _ => format!("{}:chain-{}", method, chain_id),
            };
            let mut fields = vec![chain_id.to_string(), source];
            for (i, spec) in base.iter().enumerate() {
                let initial = per_chain_starts
                    .and_then(|chains| chains.get(chain_id))
                    .and_then(|c| c.get(i))
                    .map(|s| s.initial)
                    .unwrap_or(spec.initial);
                fields.push(format_float_for_tsv(initial));
            }
            writeln!(f, "{}", fields.join("\t"))?;
        }
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn format_float_for_tsv(v: f64) -> String {
    if v.is_nan() { "NaN".into() }
    else if v == f64::INFINITY  { "Inf".into() }
    else if v == f64::NEG_INFINITY { "-Inf".into() }
    else { format!("{}", v) }
}

/// Map an LHS coordinate `u ∈ [0, 1]` to the natural-scale parameter
/// value, respecting the parameter's transform.
fn lhs_map_to_natural(spec: &EstimatedParam, u: f64) -> f64 {
    if !spec.lower.is_finite() || !spec.upper.is_finite() {
        // Unbounded: ±50% jitter around the seeded start. LHS is meaningless
        // here but we don't want to fail — the upstream validator should
        // refuse fits with unbounded estimated params; until that lands,
        // fall back gracefully.
        return spec.initial * (0.5 + u);
    }
    match &spec.transform {
        Transform::Log { .. } if spec.lower > 0.0 && spec.upper > 0.0 => {
            // LHS in log space: θ = lo · (hi/lo)^u
            spec.lower * (spec.upper / spec.lower).powf(u)
        }
        _ => {
            // Linear LHS in [lo, hi]
            spec.lower + u * (spec.upper - spec.lower)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::inference::types::Transform;

    fn ep(name: &str, lower: f64, upper: f64, transform: Transform, initial: f64) -> EstimatedParam {
        EstimatedParam {
            name: name.into(),
            index: 0,
            initial,
            rw_sd: 0.1,
            transform,
            lower,
            upper,
            rw_sd_auto: false,
            ivp: false,
        }
    }

    #[test]
    fn init_method_default_is_lhs() {
        // LHS by default for all multi-chain stages — see
        // Default impl in init.rs for the rationale (gh#42 typhoid
        // evidence + the supersession of the legacy Uniform default).
        assert_eq!(InitMethod::default(), InitMethod::Lhs);
    }

    #[test]
    fn init_method_from_str_round_trip() {
        for m in [
            InitMethod::Single,
            InitMethod::Uniform,
            InitMethod::Lhs,
            InitMethod::SurveyTopK,
        ] {
            let s = m.to_string();
            let parsed: InitMethod = s.parse().unwrap();
            assert_eq!(parsed, m);
        }
        assert!("unknown".parse::<InitMethod>().is_err());
        // The TOML-on-the-wire form is the snake_case variant, not
        // hyphenated — survey_top_k, not survey-top-k.
        assert!("survey-top-k".parse::<InitMethod>().is_err());
    }

    #[test]
    fn single_returns_none_so_caller_uses_base_params() {
        let base = vec![ep("a", 0.0, 1.0, Transform::None, 0.5)];
        let out = build_chain_starts(InitMethod::Single, &base, 8, 42);
        assert!(out.is_none());
    }

    #[test]
    fn uniform_n1_returns_none() {
        let base = vec![ep("a", 0.0, 1.0, Transform::None, 0.5)];
        assert!(build_chain_starts(InitMethod::Uniform, &base, 1, 42).is_none());
        assert!(build_chain_starts(InitMethod::Lhs, &base, 1, 42).is_none());
    }

    #[test]
    fn lhs_strata_cover_range_uniformly() {
        // 100 chains × 1 param ∈ [0, 1] linear: every decile should
        // contain ~10 starts (LHS guarantee at this resolution).
        let base = vec![ep("a", 0.0, 1.0, Transform::None, 0.5)];
        let starts = build_chain_starts(InitMethod::Lhs, &base, 100, 42).unwrap();
        let values: Vec<f64> = starts.iter().map(|c| c[0].initial).collect();

        let mut bin_counts = vec![0usize; 10];
        for &v in &values {
            let bin = ((v * 10.0) as usize).min(9);
            bin_counts[bin] += 1;
        }
        // LHS guarantees exactly one sample per stratum at the dim level.
        // With 100 chains and 10 bins, each stratum aligns 10:1 with bins.
        for &c in &bin_counts {
            assert!(c >= 8 && c <= 12,
                "LHS strata uneven: counts = {:?}", bin_counts);
        }
    }

    #[test]
    fn lhs_log_param_spans_orders_of_magnitude() {
        // Log-typed param with bounds [1e-5, 1e-2] should LHS in log space.
        // The geomean of all draws should be near sqrt(1e-5 * 1e-2) = 1e-3.5
        // and the spread should be the full range — not concentrated near 1e-2.
        let base = vec![ep("rate", 1e-5, 1e-2, Transform::Log { lo: 1e-5, hi: 1e-2 }, 1e-3)];
        let starts = build_chain_starts(InitMethod::Lhs, &base, 50, 42).unwrap();
        let values: Vec<f64> = starts.iter().map(|c| c[0].initial).collect();

        // Distribute roughly evenly across each decade.
        let log_vals: Vec<f64> = values.iter().map(|v| v.log10()).collect();
        let mean = log_vals.iter().sum::<f64>() / log_vals.len() as f64;
        // log10(1e-5) = -5, log10(1e-2) = -2, midpoint = -3.5
        assert!((mean - (-3.5)).abs() < 0.3,
            "log-LHS mean = {} (expected ~−3.5)", mean);

        let lo_count = values.iter().filter(|&&v| v < 1e-4).count();
        let hi_count = values.iter().filter(|&&v| v > 1e-3).count();
        // With LHS in log space, mass spreads across decades; uniform
        // (linear) sampling would cluster near 1e-2 with very few < 1e-4.
        assert!(lo_count >= 5 && hi_count >= 5,
            "log-LHS clusters: lo<1e-4={} hi>1e-3={} (linear sampling would skew here)",
            lo_count, hi_count);
    }

    #[test]
    fn lhs_deterministic_given_seed() {
        let base = vec![
            ep("a", 0.0, 1.0, Transform::None, 0.5),
            ep("b", 1e-3, 1.0, Transform::Log { lo: 1e-3, hi: 1.0 }, 0.1),
        ];
        let s1 = build_chain_starts(InitMethod::Lhs, &base, 16, 42).unwrap();
        let s2 = build_chain_starts(InitMethod::Lhs, &base, 16, 42).unwrap();
        for (c1, c2) in s1.iter().zip(s2.iter()) {
            for (p1, p2) in c1.iter().zip(c2.iter()) {
                assert_eq!(p1.initial, p2.initial);
            }
        }
    }

    #[test]
    fn lhs_different_seed_gives_different_draws() {
        let base = vec![ep("a", 0.0, 1.0, Transform::None, 0.5)];
        let s1 = build_chain_starts(InitMethod::Lhs, &base, 16, 42).unwrap();
        let s2 = build_chain_starts(InitMethod::Lhs, &base, 16, 43).unwrap();
        let differs = s1.iter().zip(s2.iter())
            .any(|(c1, c2)| c1[0].initial != c2[0].initial);
        assert!(differs, "LHS with different seeds returned identical draws");
    }

    #[test]
    fn lhs_within_bounds() {
        let base = vec![
            ep("rate",  1e-5, 1.0, Transform::Log   { lo: 1e-5, hi: 1.0 }, 0.01),
            ep("prob",  0.05, 0.95, Transform::Logit { lo: 0.05, hi: 0.95 }, 0.5),
            ep("real", -10.0, 10.0, Transform::None,                          0.0),
        ];
        let starts = build_chain_starts(InitMethod::Lhs, &base, 32, 7).unwrap();
        for chain in &starts {
            for spec in chain {
                assert!(spec.initial >= spec.lower && spec.initial <= spec.upper,
                    "{} out of bounds: {} not in [{}, {}]",
                    spec.name, spec.initial, spec.lower, spec.upper);
            }
        }
    }

    // ── draw_start_in_bounds (gh#34 fallback) ────────────────────────

    #[test]
    fn draw_start_log_scale_lands_inside_positive_bounds() {
        // Log-scale draw across six orders of magnitude: result must
        // be strictly inside (lo, hi) and stay positive.
        let v = draw_start_in_bounds(1e-6, 1.0, true, 42, "beta");
        assert!(v > 1e-6 && v < 1.0, "{} not in (1e-6, 1.0)", v);
        assert!(v.is_finite() && v > 0.0);
    }

    #[test]
    fn draw_start_linear_scale_lands_inside_bounds() {
        // Linear draw on a real-valued parameter (Logit/None analogue):
        // negative-to-positive bounds, no log-scale possible.
        let v = draw_start_in_bounds(-10.0, 10.0, false, 42, "drift");
        assert!(v > -10.0 && v < 10.0, "{} not in (-10, 10)", v);
    }

    #[test]
    fn draw_start_log_falls_back_to_linear_when_lo_nonpositive() {
        // log_scale=true but lo=0 — helper must NOT call powf on zero
        // (would yield 0 always or NaN); falls back to linear.
        let v = draw_start_in_bounds(0.0, 1.0, true, 42, "p");
        assert!(v > 0.0 && v < 1.0, "{} not in (0, 1)", v);
    }

    #[test]
    fn draw_start_deterministic_per_seed_and_name() {
        // Same (seed, name) ⇒ same draw.
        let a = draw_start_in_bounds(1e-3, 1.0, true, 7, "beta");
        let b = draw_start_in_bounds(1e-3, 1.0, true, 7, "beta");
        assert_eq!(a, b);
    }

    #[test]
    fn draw_start_different_names_give_different_draws() {
        // Two parameters with identical bounds at the same seed must
        // not collide (would defeat the point of the per-name hash).
        let a = draw_start_in_bounds(1e-3, 1.0, true, 7, "beta");
        let b = draw_start_in_bounds(1e-3, 1.0, true, 7, "gamma");
        assert_ne!(a, b);
    }

    #[test]
    fn draw_start_different_seeds_give_different_draws() {
        // Reseeding the run shifts the fallback (so users get spread
        // across seed sweeps, unlike the old midpoint heuristic which
        // gave the same point at every seed).
        let a = draw_start_in_bounds(1e-3, 1.0, true, 1, "beta");
        let b = draw_start_in_bounds(1e-3, 1.0, true, 2, "beta");
        assert_ne!(a, b);
    }

    #[test]
    fn draw_start_log_scale_spans_orders_of_magnitude() {
        // Across many seeds, log-scale draws on (1e-6, 1.0) should
        // populate at least three different decade buckets — the prior
        // midpoint would have given 1e-3 at every seed.
        use std::collections::HashSet;
        let mut decades: HashSet<i32> = HashSet::new();
        for seed in 0..64u64 {
            let v = draw_start_in_bounds(1e-6, 1.0, true, seed, "beta");
            decades.insert(v.log10().floor() as i32);
        }
        assert!(decades.len() >= 3,
            "expected ≥3 decades populated across 64 seeds, got {}: {:?}",
            decades.len(), decades);
    }

    // ── parse_landscape_tsv (gh#51) ──────────────────────────────────

    #[test]
    fn parse_landscape_tsv_pfilter_columns() {
        // Real shape: comments, header, 3 data rows. Param order in
        // header matches survey_estimated. Pfilter eval includes
        // mean_ess column (we ignore it but the parser must tolerate
        // the wider row).
        let raw = "\
# camdl survey landscape; run_hash=abc; version=0.1\n\
# eval=pfilter; n_points=3\n\
beta\tgamma\tloglik\tloglik_se\tmean_ess\tn_replicates\tpoint_id\n\
0.3\t0.1\t-100.5\t1.2\t0.8\t8\t0\n\
0.4\t0.2\t-95.0\t0.9\t0.85\t8\t1\n\
0.5\t0.15\t-110.2\t2.0\t0.75\t8\t2\n";
        let estimated = vec!["beta".to_string(), "gamma".to_string()];
        let rows = parse_landscape_tsv(raw, &estimated).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].params.get("beta"), Some(&0.3));
        assert_eq!(rows[0].params.get("gamma"), Some(&0.1));
        assert_eq!(rows[0].loglik, -100.5);
        assert_eq!(rows[0].loglik_se, 1.2);
        // Best-loglik row is index 1 (loglik = -95.0).
        let best = rows.iter().max_by(|a, b|
            a.loglik.partial_cmp(&b.loglik).unwrap()).unwrap();
        assert_eq!(best.loglik, -95.0);
    }

    #[test]
    fn parse_landscape_tsv_simulate_columns() {
        // Simulate eval omits mean_ess column.
        let raw = "\
# survey\n\
beta\tgamma\tloglik\tloglik_se\tn_replicates\tpoint_id\n\
0.3\t0.1\t-100.5\t1.2\t1\t0\n";
        let estimated = vec!["beta".to_string(), "gamma".to_string()];
        let rows = parse_landscape_tsv(raw, &estimated).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].params.get("beta"), Some(&0.3));
        assert_eq!(rows[0].loglik, -100.5);
    }

    #[test]
    fn parse_landscape_tsv_missing_param_column_errors() {
        // Survey claims `beta` is estimated but the header doesn't
        // have a `beta` column. Should error with a clear message
        // naming the missing param.
        let raw = "\
gamma\tloglik\tloglik_se\tn_replicates\tpoint_id\n\
0.1\t-100.5\t1.2\t1\t0\n";
        let estimated = vec!["beta".to_string(), "gamma".to_string()];
        let err = parse_landscape_tsv(raw, &estimated).unwrap_err();
        assert!(err.contains("beta"), "error should name missing param: {}", err);
    }

    #[test]
    fn parse_landscape_tsv_missing_loglik_errors() {
        let raw = "\
beta\tgamma\tn_replicates\tpoint_id\n\
0.3\t0.1\t1\t0\n";
        let estimated = vec!["beta".to_string(), "gamma".to_string()];
        let err = parse_landscape_tsv(raw, &estimated).unwrap_err();
        assert!(err.contains("loglik"));
    }
}
