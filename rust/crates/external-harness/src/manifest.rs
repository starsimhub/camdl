//! Manifest types for the external-validation harness.
//!
//! Three files per case:
//! - `case.toml`        — what to run (camdl command, reference driver, summary spec)
//! - `expected.toml`    — what counts as passing (checks with rationales)
//! - `fixtures/MANIFEST.toml` — what produced the cached fixture (hashes + provenance)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─── case.toml ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CaseManifest {
    pub name: String,
    pub description: String,
    pub category: CaseCategory,
    pub camdl: CamdlSpec,
    pub reference: ReferenceSpec,
    pub summary: SummarySpec,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CaseCategory {
    ForwardSimulation,
    Pfilter,
    If2,
    Pmmh,
    Analytical,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CamdlSpec {
    pub model: PathBuf,
    pub params: Option<PathBuf>,
    /// Command template. Placeholders:
    /// - `@model` / `@params` resolve to the case's `model` / `params`.
    /// - `@seed` — per-run seed (per-seed mode only).
    /// - `@obs_out` — per-seed output TSV path (per-seed mode only).
    /// - `@n_seeds` / `@seed_base` — batch-replicated mode only.
    /// - `@batch_out` — single output TSV path for batch-replicated mode.
    pub command: Vec<String>,
    pub n_seeds: usize,
    pub seed_base: u64,
    /// Where camdl writes output per seed — read by the summariser.
    /// Placeholders: `@seed_dir` for the per-seed directory.
    #[serde(default = "default_output_spec")]
    pub output: String,
    /// Execution mode.
    ///
    /// - `per-seed` (default): the harness invokes camdl `n_seeds`
    ///   times with different seeds, each producing its own TSV under
    ///   `seeds/<seed>/obs.tsv`. Natural for forward simulation.
    /// - `batch-replicated`: the harness invokes camdl **once** with
    ///   the full replicate count; camdl writes a long-format TSV
    ///   with one row per replicate. Natural for pfilter log-lik
    ///   (`camdl pfilter --replicates N --output ...`), PMMH traces,
    ///   etc. Reads through the same long-TSV summariser as the
    ///   reference side, so stat specs look identical either way.
    #[serde(default)]
    pub mode: CamdlMode,
    /// For `batch-replicated` mode: name of the seed column in the
    /// output TSV that camdl produces (e.g. `seed` for `camdl pfilter`).
    #[serde(default = "default_batch_seed_col")]
    pub batch_seed_col: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CamdlMode {
    #[default]
    PerSeed,
    BatchReplicated,
}

fn default_output_spec() -> String { "@seed_dir/obs.tsv".to_string() }

fn default_seed_col() -> String { "sim".to_string() }

fn default_batch_seed_col() -> String { "seed".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ReferenceSpec {
    /// No external tool; fixture is derived from a mathematical derivation
    /// committed alongside the case (for dogfood/unit-test cases).
    Analytical {
        /// Hashed for staleness detection.
        derivation: PathBuf,
    },
    /// Shell-out to an R + pomp driver. `run` is invoked on regen.
    RPomp {
        run: PathBuf,
        /// Directory whose recursive hash is the staleness fingerprint.
        /// Typically the `reference/` directory itself.
        #[serde(default)]
        fingerprint_dir: Option<PathBuf>,
        /// Path to the long-format ensemble TSV the reference script
        /// writes. Relative to the case directory. The harness reads
        /// this on regen to compute the fresh summary.
        ensemble_tsv: PathBuf,
        /// Name of the seed column in `ensemble_tsv`. Common values:
        /// pomp's `simulate` uses `sim`; numpyro typically uses `chain`
        /// or `sample`.
        #[serde(default = "default_seed_col")]
        seed_col: String,
    },
    /// Shell-out to a Python (NumPyro, etc.) driver. Same semantics.
    PyNumpyro {
        run: PathBuf,
        #[serde(default)]
        fingerprint_dir: Option<PathBuf>,
    },
    /// Shell-out to a Stan driver.
    Stan {
        run: PathBuf,
        #[serde(default)]
        fingerprint_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SummarySpec {
    /// For analytical cases: summary is read directly from a pre-computed
    /// TSV committed to `fixtures/summary.tsv`; nothing to aggregate.
    Prebaked,
    /// Aggregate per-seed output TSVs into per-stat summary rows.
    EnsembleStats { stats: Vec<StatSpec> },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatSpec {
    pub name: String,
    pub aggregate: AggregateOp,
    /// Column name in the per-seed output TSV.
    pub over: String,
    /// Optional scope hint (e.g., "last-year-per-seed"). Interpreted by
    /// the summariser; unknown values are ignored for now.
    #[serde(default)]
    pub scope: Option<String>,
    /// For `aggregate = "frac"`: count the fraction of per-seed totals
    /// that exceed `threshold`.
    #[serde(default)]
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AggregateOp {
    Sum,
    Max,
    Mean,
    Frac,
}

// ─── expected.toml ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpectedManifest {
    pub checks: std::collections::BTreeMap<String, Check>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Check {
    #[serde(flatten)]
    pub kind: CheckKind,
    /// Human-readable justification including the MC power statement.
    /// Required (enforced by the harness at load time).
    pub rationale: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "compare", rename_all = "kebab-case")]
pub enum CheckKind {
    Mean {
        #[serde(default)]
        tol_abs: Option<f64>,
        #[serde(default)]
        tol_rel: Option<f64>,
    },
    Quantiles {
        q: Vec<f64>,
        #[serde(default)]
        tol_abs: Option<f64>,
        #[serde(default)]
        tol_rel: Option<f64>,
    },
    Value {
        #[serde(default)]
        tol_abs: Option<f64>,
        #[serde(default)]
        tol_rel: Option<f64>,
    },
    ProportionTest {
        alpha: f64,
    },
    KsTest {
        alpha: f64,
    },
}

// ─── fixtures/MANIFEST.toml ───────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FixtureManifest {
    /// sha256 over the reference fingerprint (directory for r-pomp / py /
    /// stan; single file for analytical). Enforced strictly.
    pub reference_sha: String,
    /// sha256 over model + params + case.toml + expected.toml. Enforced
    /// strictly: a model/params change without a reference regen is a test
    /// bug.
    pub case_sha: String,
    /// Version of the harness that produced this fixture. Enforced
    /// strictly: summariser changes invalidate existing fixtures.
    pub harness_version: String,
    /// sha256 over fixtures/summary.tsv. Informational only —
    /// byte-reproducibility is not a design goal (principle #3).
    pub fixture_sha: String,

    // Provenance (informational)
    #[serde(default)]
    pub pomp_version: Option<String>,
    #[serde(default)]
    pub r_version: Option<String>,
    #[serde(default)]
    pub python_version: Option<String>,
    pub generated_at: String,
    #[serde(default)]
    pub generated_on: Option<String>,
    #[serde(default)]
    pub generated_command: Option<String>,
    #[serde(default)]
    pub generated_in_docker: bool,
    pub n_seeds_reference: usize,
    pub seed_base: u64,
}
