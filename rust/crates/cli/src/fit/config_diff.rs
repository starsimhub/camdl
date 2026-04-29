//! Structured fit-config diff engine.
//!
//! Computes a typed diff between two `FitConfigV2` instances —
//! ([estimate], [fixed], bounds, priors, data hashes, stages) — for
//! the `table_row.config_diff_from_baseline` field. JSON consumers
//! need a structured shape (free-form text loses information); the
//! text renderer in `fit table` projects the same struct
//! deterministically.
//!
//! See `docs/dev/proposals/2026-04-28-fit-experiment-management.md` §4.
//!
//! **Parser reuse.** This module never re-implements fit.toml parsing;
//! it always loads via [`config_v2::FitConfigV2::load`]. Two parsers
//! diverging silently on edge cases (transform aliases, prior syntax,
//! default filling) is exactly the drift class this proposal exists to
//! prevent.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::fit::config_v2::{FitConfigV2, PriorSpec, Stage};
use crate::run_meta::FitMeta;

/// Structured diff of one fit's config relative to a baseline. Map
/// fields use `BTreeMap` end-to-end so JSON serialization is
/// lex-ordered (load-bearing for the `summary ⊆ table` byte-equality
/// test in Deliverable C).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConfigDiff {
    /// Hash of the baseline fit. `None` only when no baseline was
    /// available (e.g. computing the diff for the baseline against
    /// itself produces all-empty fields and the baseline_hash equals
    /// the fit's own hash; the explicit `None` case is reserved for
    /// callers that don't pick a baseline at all).
    pub baseline_hash: Option<String>,
    /// True iff the underlying camdl model IR hash differs. When true,
    /// scalar comparisons (best_loglik, R0, etc.) are misleading; the
    /// text renderer says `(model changed; comparison limited)`.
    pub model_changed: bool,
    /// Parameters that moved into `[estimate]` (or appeared new).
    pub estimate_added: Vec<String>,
    /// Parameters that left `[estimate]`.
    pub estimate_removed: Vec<String>,
    /// Parameters that moved into `[fixed]` (or appeared new).
    pub fixed_added: Vec<String>,
    /// Parameters that left `[fixed]`.
    pub fixed_removed: Vec<String>,
    /// Parameters whose `[estimate.<name>].bounds` tuple changed.
    pub bounds_changed: Vec<BoundsChange>,
    /// Parameters whose declared `prior` differs (after canonical
    /// rendering — see [`format_prior`]). Add ↔ remove ↔ retype all
    /// fall under this collection.
    pub priors_changed: Vec<PriorChange>,
    /// Per-stream data file hash differences.
    pub data_hashes: DataHashesDiff,
    /// Stage-level diff (added / removed names plus per-stage settings
    /// changes).
    pub stages_changed: StagesChanged,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BoundsChange {
    pub param: String,
    pub from: (f64, f64),
    pub to: (f64, f64),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PriorChange {
    pub param: String,
    /// Canonical prior string (output of [`format_prior`]). `None`
    /// means no prior was declared (only meaningful for MLE-only
    /// parameters; Bayesian fits require a prior).
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DataHashesDiff {
    /// Streams present in this fit but not in the baseline.
    pub added: Vec<String>,
    /// Streams present in the baseline but not in this fit.
    pub removed: Vec<String>,
    /// Streams in both fits whose content hashes differ.
    pub modified: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct StagesChanged {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Per-stage settings changes — one entry per (stage, key) tuple
    /// whose value changed. The shape is intentionally flat (a list of
    /// settings deltas) rather than nested maps; renderers project
    /// however they want.
    pub settings_changed: Vec<StageSettingsChange>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StageSettingsChange {
    pub stage: String,
    pub key: String,
    /// Pre-image as a JSON value (numbers stay numeric, strings stay
    /// stringy). Stored as `serde_json::Value` so consumers can
    /// type-introspect.
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

impl ConfigDiff {
    /// Diff of a fit against itself — every `_added`/`_removed`
    /// vector empty, no scalar changes, baseline_hash = self's hash.
    /// Used by `fit summary` (single-fit) and by `fit table` when the
    /// baseline-selection policy resolves to the same row (e.g.
    /// `--hash <h>` filtering to one row).
    pub fn identity(self_hash: &str) -> Self {
        ConfigDiff {
            baseline_hash: Some(self_hash.to_string()),
            model_changed: false,
            estimate_added: Vec::new(),
            estimate_removed: Vec::new(),
            fixed_added: Vec::new(),
            fixed_removed: Vec::new(),
            bounds_changed: Vec::new(),
            priors_changed: Vec::new(),
            data_hashes: DataHashesDiff::default(),
            stages_changed: StagesChanged::default(),
        }
    }

    /// Compare `this` against `baseline`. Both arguments are already
    /// parsed via [`FitConfigV2::load`] — callers that have only paths
    /// should call [`compare_paths`] instead, which loads then
    /// dispatches here.
    ///
    /// `model_changed` requires the caller to supply each fit's
    /// `model_hash` from `FitMeta` (the `FitConfigV2` itself only
    /// references the model file; the canonical hash lives on
    /// `FitMeta`).
    pub fn compare(
        this: &FitConfigV2,
        baseline: &FitConfigV2,
        this_meta: &FitMeta,
        baseline_meta: &FitMeta,
    ) -> Self {
        let this_est: BTreeSet<&str> =
            this.estimate.keys().map(|s| s.as_str()).collect();
        let base_est: BTreeSet<&str> =
            baseline.estimate.keys().map(|s| s.as_str()).collect();

        let this_fix = this.fixed.resolve().unwrap_or_default();
        let base_fix = baseline.fixed.resolve().unwrap_or_default();
        let this_fix_keys: BTreeSet<&str> =
            this_fix.keys().map(|s| s.as_str()).collect();
        let base_fix_keys: BTreeSet<&str> =
            base_fix.keys().map(|s| s.as_str()).collect();

        let estimate_added: Vec<String> = this_est
            .difference(&base_est)
            .map(|s| s.to_string())
            .collect();
        let estimate_removed: Vec<String> = base_est
            .difference(&this_est)
            .map(|s| s.to_string())
            .collect();
        let fixed_added: Vec<String> = this_fix_keys
            .difference(&base_fix_keys)
            .map(|s| s.to_string())
            .collect();
        let fixed_removed: Vec<String> = base_fix_keys
            .difference(&this_fix_keys)
            .map(|s| s.to_string())
            .collect();

        let mut bounds_changed = Vec::new();
        for name in this_est.intersection(&base_est) {
            let tb = this.estimate[*name].bounds;
            let bb = baseline.estimate[*name].bounds;
            if (tb.0 - bb.0).abs() > 0.0 || (tb.1 - bb.1).abs() > 0.0 {
                bounds_changed.push(BoundsChange {
                    param: (*name).to_string(),
                    from: bb,
                    to: tb,
                });
            }
        }

        let mut priors_changed = Vec::new();
        let estimate_union: BTreeSet<&str> =
            this_est.union(&base_est).copied().collect();
        for name in &estimate_union {
            let tp = this.estimate.get(*name).and_then(|e| e.prior.as_ref());
            let bp = baseline.estimate.get(*name).and_then(|e| e.prior.as_ref());
            let tp_str = tp.map(format_prior);
            let bp_str = bp.map(format_prior);
            if tp_str != bp_str {
                priors_changed.push(PriorChange {
                    param: (*name).to_string(),
                    from: bp_str,
                    to: tp_str,
                });
            }
        }

        let data_hashes =
            diff_data_hashes(&this_meta.data_hashes, &baseline_meta.data_hashes);
        let stages_changed = diff_stages(&this.stages, &baseline.stages);

        ConfigDiff {
            baseline_hash: Some(baseline_meta_hash(baseline_meta)),
            model_changed: this_meta.model_hash != baseline_meta.model_hash,
            estimate_added,
            estimate_removed,
            fixed_added,
            fixed_removed,
            bounds_changed,
            priors_changed,
            data_hashes,
            stages_changed,
        }
    }
}

/// Canonical string projection for a [`PriorSpec`]. Intentionally
/// terse — `log_normal(mu=..., sigma=...)`, `normal(mu=..., sigma=...)`,
/// `beta(alpha=..., beta=...)`, `uniform`, `half_normal(sigma=...)`.
/// The format is stable across versions because `priors_changed`
/// equality compares strings.
pub fn format_prior(p: &PriorSpec) -> String {
    match p {
        PriorSpec::LogNormal { mu, sigma } =>
            format!("log_normal(mu={}, sigma={})", mu, sigma),
        PriorSpec::Normal { mu, sigma } =>
            format!("normal(mu={}, sigma={})", mu, sigma),
        PriorSpec::Beta { alpha, beta } =>
            format!("beta(alpha={}, beta={})", alpha, beta),
        PriorSpec::Uniform => "uniform".to_string(),
        PriorSpec::HalfNormal { sigma } =>
            format!("half_normal(sigma={})", sigma),
        PriorSpec::Gamma { shape, rate } =>
            format!("gamma(shape={}, rate={})", shape, rate),
        PriorSpec::Exponential { rate } =>
            format!("exponential(rate={})", rate),
    }
}

fn baseline_meta_hash(meta: &FitMeta) -> String {
    // FitMeta carries the fit's input hash via fit_toml_hash (the
    // canonical hash for a v2 fit.toml); the *fit* hash itself lives on
    // the enclosing Run.hash. Callers that have the Run pass that
    // through; here we only have FitMeta, so fall back to the toml
    // hash. In practice the consumer (`fit table`) passes a
    // pre-resolved baseline hash via `ConfigDiff::with_baseline_hash`
    // when it has a Run handy.
    meta.fit_toml_hash.clone()
}

impl ConfigDiff {
    /// Override the `baseline_hash` field after construction. Used by
    /// `fit table` to put the actual fit_hash (Run.hash) on the diff
    /// rather than the fit_toml_hash that [`compare`] defaults to.
    pub fn with_baseline_hash(mut self, hash: String) -> Self {
        self.baseline_hash = Some(hash);
        self
    }
}

fn diff_data_hashes(
    this: &std::collections::HashMap<String, String>,
    baseline: &std::collections::HashMap<String, String>,
) -> DataHashesDiff {
    let this_keys: BTreeSet<&str> = this.keys().map(|s| s.as_str()).collect();
    let base_keys: BTreeSet<&str> = baseline.keys().map(|s| s.as_str()).collect();

    let added: Vec<String> = this_keys
        .difference(&base_keys)
        .map(|s| s.to_string())
        .collect();
    let removed: Vec<String> = base_keys
        .difference(&this_keys)
        .map(|s| s.to_string())
        .collect();
    let mut modified = Vec::new();
    for name in this_keys.intersection(&base_keys) {
        if this.get(*name) != baseline.get(*name) {
            modified.push((*name).to_string());
        }
    }
    DataHashesDiff {
        added,
        removed,
        modified,
    }
}

fn diff_stages(
    this: &indexmap::IndexMap<String, Stage>,
    baseline: &indexmap::IndexMap<String, Stage>,
) -> StagesChanged {
    let this_keys: BTreeSet<&str> = this.keys().map(|s| s.as_str()).collect();
    let base_keys: BTreeSet<&str> = baseline.keys().map(|s| s.as_str()).collect();

    let added: Vec<String> = this_keys
        .difference(&base_keys)
        .map(|s| s.to_string())
        .collect();
    let removed: Vec<String> = base_keys
        .difference(&this_keys)
        .map(|s| s.to_string())
        .collect();

    let mut settings_changed = Vec::new();
    for name in this_keys.intersection(&base_keys) {
        let ts = stage_settings_map(&this[*name]);
        let bs = stage_settings_map(&baseline[*name]);
        let key_union: BTreeSet<&str> =
            ts.keys().chain(bs.keys()).map(|s| s.as_str()).collect();
        for key in key_union {
            let from_v = bs.get(key).cloned().unwrap_or(serde_json::Value::Null);
            let to_v = ts.get(key).cloned().unwrap_or(serde_json::Value::Null);
            if from_v != to_v {
                settings_changed.push(StageSettingsChange {
                    stage: (*name).to_string(),
                    key: key.to_string(),
                    from: from_v,
                    to: to_v,
                });
            }
        }
    }
    StagesChanged {
        added,
        removed,
        settings_changed,
    }
}

/// Project a `Stage` into a flat key→value settings map. Method-aware:
/// keys differ across IF2/PGAS/PMMH/PFilter, and the `method` itself
/// becomes a key so a stage that swapped methods produces a clean
/// `method` row in `settings_changed`.
fn stage_settings_map(stage: &Stage) -> BTreeMap<String, serde_json::Value> {
    let mut m = BTreeMap::new();
    m.insert("method".into(), serde_json::Value::String(stage.method_name().into()));
    match stage {
        Stage::IF2 {
            chains,
            particles,
            iterations,
            cooling,
            loglik_eval,
            gate,
            ..
        } => {
            m.insert("chains".into(), serde_json::json!(chains));
            m.insert("particles".into(), serde_json::json!(particles));
            m.insert("iterations".into(), serde_json::json!(iterations));
            m.insert("cooling".into(), serde_json::json!(cooling));
            m.insert(
                "loglik_eval.n_particles".into(),
                serde_json::json!(loglik_eval.n_particles),
            );
            m.insert(
                "loglik_eval.n_replicates".into(),
                serde_json::json!(loglik_eval.n_replicates),
            );
            m.insert("gate.a_thresh".into(), serde_json::json!(gate.a_thresh));
            m.insert(
                "gate.decibans_thresh".into(),
                serde_json::json!(gate.decibans_thresh),
            );
        }
        Stage::PGAS {
            chains,
            particles,
            sweeps,
            burn_in,
            thin,
            ..
        } => {
            m.insert("chains".into(), serde_json::json!(chains));
            m.insert("particles".into(), serde_json::json!(particles));
            m.insert("sweeps".into(), serde_json::json!(sweeps));
            m.insert("burn_in".into(), serde_json::json!(burn_in));
            m.insert("thin".into(), serde_json::json!(thin));
        }
        Stage::PMMH {
            chains,
            particles,
            iterations,
            burn_in,
            thin,
            ..
        } => {
            m.insert("chains".into(), serde_json::json!(chains));
            m.insert("particles".into(), serde_json::json!(particles));
            m.insert("iterations".into(), serde_json::json!(iterations));
            m.insert("burn_in".into(), serde_json::json!(burn_in));
            m.insert("thin".into(), serde_json::json!(thin));
        }
        Stage::PFilter {
            particles,
            replicates,
            ..
        } => {
            m.insert("particles".into(), serde_json::json!(particles));
            m.insert("replicates".into(), serde_json::json!(replicates));
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit::config_v2::FitConfigV2;
    use std::collections::HashMap;

    fn fitmeta(model_hash: &str) -> FitMeta {
        FitMeta {
            model: "sir.camdl".into(),
            model_hash: model_hash.into(),
            fit_toml_path: "fit.toml".into(),
            fit_toml_hash: "h".repeat(64),
            data_hashes: HashMap::new(),
            estimated: Vec::new(),
            fixed: HashMap::new(),
            stages_declared: Vec::new(),
            ic_free: false,
            label: None,
        }
    }

    fn parse(s: &str) -> FitConfigV2 {
        toml::from_str(s).expect("toml parse")
    }

    const BASELINE_TOML: &str = r#"
        [model]
        camdl = "sir.camdl"

        [data]
        observations = { cases = "cases.tsv" }

        [estimate.R0]
        bounds = [1.0, 100.0]

        [estimate.sigma]
        bounds = [0.01, 0.5]
        prior = { dist = "log_normal", mu = -2.0, sigma = 0.5 }

        [fixed]
        N0 = 1000.0

        [stages.scout]
        method = "if2"
        chains = 4
        particles = 500
        iterations = 50
        cooling = 0.7

        [stages.refine]
        method = "if2"
        chains = 4
        particles = 1000
        iterations = 100
        cooling = 0.5
    "#;

    #[test]
    fn identity_diff_is_all_empty() {
        let diff = ConfigDiff::identity("abcd1234");
        assert_eq!(diff.baseline_hash.as_deref(), Some("abcd1234"));
        assert!(!diff.model_changed);
        assert!(diff.estimate_added.is_empty());
        assert!(diff.estimate_removed.is_empty());
        assert!(diff.bounds_changed.is_empty());
        assert!(diff.priors_changed.is_empty());
    }

    #[test]
    fn detects_estimate_to_fixed_move() {
        let baseline = parse(BASELINE_TOML);
        let mut variant_str = BASELINE_TOML.replace(
            "[fixed]\n        N0 = 1000.0",
            "[fixed]\n        N0 = 1000.0\n        sigma = 0.08",
        );
        variant_str = variant_str.replace(
            "[estimate.sigma]\n        bounds = [0.01, 0.5]\n        prior = { dist = \"log_normal\", mu = -2.0, sigma = 0.5 }\n",
            "",
        );
        let variant = parse(&variant_str);
        let diff = ConfigDiff::compare(
            &variant,
            &baseline,
            &fitmeta("modelA"),
            &fitmeta("modelA"),
        );
        assert_eq!(diff.estimate_removed, vec!["sigma".to_string()]);
        assert_eq!(diff.fixed_added, vec!["sigma".to_string()]);
        assert!(diff.estimate_added.is_empty());
    }

    #[test]
    fn detects_bounds_change() {
        let baseline = parse(BASELINE_TOML);
        let variant_str = BASELINE_TOML.replace("[1.0, 100.0]", "[40.0, 80.0]");
        let variant = parse(&variant_str);
        let diff = ConfigDiff::compare(
            &variant,
            &baseline,
            &fitmeta("modelA"),
            &fitmeta("modelA"),
        );
        assert_eq!(diff.bounds_changed.len(), 1);
        let bc = &diff.bounds_changed[0];
        assert_eq!(bc.param, "R0");
        assert_eq!(bc.from, (1.0, 100.0));
        assert_eq!(bc.to, (40.0, 80.0));
    }

    #[test]
    fn detects_prior_change() {
        let baseline = parse(BASELINE_TOML);
        // Add a prior on R0 (was none).
        let variant_str = BASELINE_TOML.replace(
            "[estimate.R0]\n        bounds = [1.0, 100.0]",
            "[estimate.R0]\n        bounds = [1.0, 100.0]\n        prior = { dist = \"log_normal\", mu = 4.0, sigma = 0.4 }",
        );
        let variant = parse(&variant_str);
        let diff = ConfigDiff::compare(
            &variant,
            &baseline,
            &fitmeta("modelA"),
            &fitmeta("modelA"),
        );
        assert_eq!(diff.priors_changed.len(), 1);
        let pc = &diff.priors_changed[0];
        assert_eq!(pc.param, "R0");
        assert_eq!(pc.from, None);
        assert_eq!(pc.to.as_deref(), Some("log_normal(mu=4, sigma=0.4)"));
    }

    #[test]
    fn detects_stage_added_and_settings_changed() {
        let baseline = parse(BASELINE_TOML);
        let variant_str = BASELINE_TOML.replace(
            "[stages.refine]\n        method = \"if2\"\n        chains = 4\n        particles = 1000\n        iterations = 100\n        cooling = 0.5",
            "[stages.refine]\n        method = \"if2\"\n        chains = 8\n        particles = 1000\n        iterations = 100\n        cooling = 0.5\n\n        [stages.validate]\n        method = \"if2\"\n        chains = 4\n        particles = 5000\n        iterations = 20\n        cooling = 0.9",
        );
        let variant = parse(&variant_str);
        let diff = ConfigDiff::compare(
            &variant,
            &baseline,
            &fitmeta("modelA"),
            &fitmeta("modelA"),
        );
        assert_eq!(diff.stages_changed.added, vec!["validate".to_string()]);
        assert!(diff.stages_changed.removed.is_empty());
        // refine.chains: 4 → 8
        let chains_chg = diff
            .stages_changed
            .settings_changed
            .iter()
            .find(|s| s.stage == "refine" && s.key == "chains")
            .expect("refine.chains delta missing");
        assert_eq!(chains_chg.from, serde_json::json!(4));
        assert_eq!(chains_chg.to, serde_json::json!(8));
    }

    #[test]
    fn model_changed_requires_distinct_model_hashes() {
        let baseline = parse(BASELINE_TOML);
        let diff_same = ConfigDiff::compare(
            &baseline,
            &baseline,
            &fitmeta("modelA"),
            &fitmeta("modelA"),
        );
        assert!(!diff_same.model_changed);
        let diff_changed = ConfigDiff::compare(
            &baseline,
            &baseline,
            &fitmeta("modelB"),
            &fitmeta("modelA"),
        );
        assert!(diff_changed.model_changed);
    }

    #[test]
    fn data_hashes_added_removed_modified() {
        let mut base = fitmeta("modelA");
        base.data_hashes.insert("cases".into(), "h1".into());
        base.data_hashes.insert("deaths".into(), "h2".into());
        let mut this = fitmeta("modelA");
        this.data_hashes.insert("cases".into(), "h1prime".into()); // modified
        this.data_hashes.insert("hospital".into(), "h3".into());   // added
        // "deaths" removed.
        let cfg = parse(BASELINE_TOML);
        let diff = ConfigDiff::compare(&cfg, &cfg, &this, &base);
        assert_eq!(diff.data_hashes.added, vec!["hospital".to_string()]);
        assert_eq!(diff.data_hashes.removed, vec!["deaths".to_string()]);
        assert_eq!(diff.data_hashes.modified, vec!["cases".to_string()]);
    }

    /// Identity diff serializes deterministically — empty vectors and
    /// `model_changed: false`. The test exists so any later schema
    /// change visible to consumers fails loudly.
    #[test]
    fn identity_diff_json_shape_is_stable() {
        let diff = ConfigDiff::identity("abc");
        let json = serde_json::to_value(&diff).unwrap();
        assert_eq!(json["baseline_hash"], "abc");
        assert_eq!(json["model_changed"], false);
        assert_eq!(json["estimate_added"], serde_json::json!([]));
        assert_eq!(json["data_hashes"]["modified"], serde_json::json!([]));
        assert_eq!(json["stages_changed"]["settings_changed"], serde_json::json!([]));
    }
}
