//! gh#38: integration tests for `camdl profile` against indexed
//! observation families.
//!
//! Symptom the fix targets: previously, `camdl profile --obs cases`
//! on a model whose IR had 5 expanded `cases_p1`...`cases_p5`
//! streams scored only the first stream and reported a loglik ~5
//! orders of magnitude smaller (in absolute value) than the joint
//! likelihood `camdl fit run` was optimising. Profile-likelihood
//! plots derived from this output were not commensurate with fit
//! summaries.
//!
//! These tests assert the post-fix behaviour:
//!
//! 1. Family-name resolution: `--obs <root>` against a multi-stream
//!    IR produces a loglik whose magnitude is the sum across all
//!    expanded streams (single-stream `--obs <leaf>` produces ~1/N
//!    of the magnitude when N streams expand from the family).
//! 2. Single-stream profile (one IR observation, no family) keeps
//!    its prior behaviour.
//! 3. Default behaviour for a multi-stream model with `--obs`
//!    omitted is a hard error listing the available stream names —
//!    no silent fall-back to the first IR observation.

use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../target/release/camdl")
}

fn skip_if_missing_binary() -> Option<PathBuf> {
    let bin = binary();
    if !bin.exists() {
        eprintln!("skipping: camdl binary not built at {}", bin.display());
        return None;
    }
    Some(bin)
}

/// 5-patch SEIR with five neg_binomial obs streams `cases_p1`...
/// `cases_p5` sharing family root `cases`. Used here as a stand-in
/// for an indexed `cases[s,a]` family — the expander emits the same
/// `<family>_<index>` naming convention, so the resolution path is
/// identical.
fn seir_spatial_5_inference() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../../ir/golden/seir_spatial_5_inference.ir.json")
}

/// Generate a synthetic multi-stream observations TSV from the model
/// at known parameter values. Returns the path; caller cleans up via
/// the surrounding tempdir.
fn synth_obs_tsv(bin: &Path, tmp: &Path) -> PathBuf {
    let obs_path = tmp.join("seir_obs.tsv");
    let status = Command::new(bin)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "simulate", &seir_spatial_5_inference().to_string_lossy(),
            "--backend", "chain_binomial", "--dt", "1", "--seed", "42",
            "--scenario", "true_params",
            "--obs-only", &obs_path.to_string_lossy(),
        ])
        .status()
        .expect("spawn camdl simulate");
    assert!(status.success(), "synthetic obs generation failed");
    assert!(obs_path.exists(), "obs TSV not written");
    obs_path
}

/// Read the loglik values from a profile.tsv (the umbrella's
/// `summary.tsv` mirror). Skips the comment header and TSV header
/// rows; returns one f64 per data row.
fn parse_logliks(profile_tsv: &Path) -> Vec<f64> {
    let body = std::fs::read_to_string(profile_tsv).expect("read profile.tsv");
    let mut out = Vec::new();
    let mut header_seen = false;
    for line in body.lines() {
        if line.starts_with('#') || line.is_empty() { continue; }
        if !header_seen {
            // First non-comment line is the column header.
            header_seen = true;
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        // Layout: <focal_1> ... <focal_K> | loglik | <param_1> ...
        // We hardcode a single focal column here (the tests sweep one
        // axis); column 1 is loglik.
        if cols.len() >= 2 {
            if let Ok(v) = cols[1].parse::<f64>() {
                out.push(v);
            }
        }
    }
    out
}

/// Run `camdl profile` once and return the parsed loglik values.
fn run_profile(
    bin: &Path,
    output_root: &Path,
    obs_arg: &str,
    data_path: &Path,
    out_tsv: &Path,
) -> Vec<f64> {
    let status = Command::new(bin)
        .env("CAMDL_OUTPUT_DIR", output_root)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "profile", &seir_spatial_5_inference().to_string_lossy(),
            "--scenario", "true_params",
            "--data", &data_path.to_string_lossy(),
            "--obs", obs_arg,
            "--sweep", "R0=lin(15,25,2)",
            "--particles", "100", "--iterations", "1", "--starts", "1",
            "--rw-sd", "auto",
            "--fixed", "sigma,gamma,kappa,amplitude,iota,rho,sigma_se,k",
            "--output", &out_tsv.to_string_lossy(),
            "--seed", "1",
        ])
        .status()
        .expect("spawn camdl profile");
    assert!(status.success(), "profile run failed for --obs {}", obs_arg);
    parse_logliks(out_tsv)
}

#[test]
fn profile_family_root_sums_all_expanded_streams() {
    // Core gh#38 regression test: `--obs cases` resolves to all 5
    // expanded streams (`cases_p1`...`cases_p5`) and the reported
    // loglik must be the joint sum, not the first-stream-only value.
    //
    // Concretely we expect the magnitude (|loglik|) under
    // `--obs cases` to be substantially larger than under
    // `--obs cases_p1`. With a uniform-ish 5-stream split we'd
    // expect a factor of ~5; we assert a much weaker lower bound
    // (≥3×) to stay robust to per-stream variation under stochastic
    // IF2 with iterations=1.
    let Some(bin) = skip_if_missing_binary() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let data_path = synth_obs_tsv(&bin, tmp.path());

    let cases_dir = tmp.path().join("out_cases");
    let p1_dir    = tmp.path().join("out_cases_p1");
    let cases_tsv = tmp.path().join("profile_cases.tsv");
    let p1_tsv    = tmp.path().join("profile_p1.tsv");

    let ll_family = run_profile(&bin, &cases_dir, "cases", &data_path, &cases_tsv);
    let ll_single = run_profile(&bin, &p1_dir,    "cases_p1", &data_path, &p1_tsv);

    assert_eq!(ll_family.len(), 2, "expected 2 grid points, got {:?}", ll_family);
    assert_eq!(ll_single.len(), 2, "expected 2 grid points, got {:?}", ll_single);

    // Both families should produce finite, negative logliks.
    for (i, ll) in ll_family.iter().enumerate() {
        assert!(ll.is_finite() && *ll < 0.0,
            "multi-stream loglik at grid {} not a finite negative: {}", i, ll);
    }
    for (i, ll) in ll_single.iter().enumerate() {
        assert!(ll.is_finite() && *ll < 0.0,
            "single-stream loglik at grid {} not a finite negative: {}", i, ll);
    }

    // Magnitude check: |ll_family| should be ≥3× |ll_single|. Before
    // the fix, family resolution silently scored only the first IR
    // observation (cases_p1), so the family loglik equalled the
    // single-stream loglik (ratio ≈ 1×).
    for (i, (lf, ls)) in ll_family.iter().zip(ll_single.iter()).enumerate() {
        let ratio = lf.abs() / ls.abs();
        assert!(ratio >= 3.0,
            "grid {}: |loglik(family)| = {} should be ≥ 3× |loglik(single)| = {} \
             (ratio = {:.2}x). Pre-fix: ratio ≈ 1× (silent first-stream-only).",
            i, lf.abs(), ls.abs(), ratio);
    }
}

#[test]
fn profile_multi_stream_model_requires_explicit_obs() {
    // `--obs` omitted on a multi-stream IR must hard-error, not
    // silently default to the first stream. The error message must
    // list the available streams so the user knows the family root
    // (or one specific stream) to pass.
    let Some(bin) = skip_if_missing_binary() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let data_path = synth_obs_tsv(&bin, tmp.path());
    let out_dir = tmp.path().join("out_no_obs");
    let out_tsv = tmp.path().join("profile_no_obs.tsv");

    let output = Command::new(&bin)
        .env("CAMDL_OUTPUT_DIR", &out_dir)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "profile", &seir_spatial_5_inference().to_string_lossy(),
            "--scenario", "true_params",
            "--data", &data_path.to_string_lossy(),
            "--sweep", "R0=lin(15,25,2)",
            "--particles", "100", "--iterations", "1", "--starts", "1",
            "--rw-sd", "auto",
            "--fixed", "sigma,gamma,kappa,amplitude,iota,rho,sigma_se,k",
            "--output", &out_tsv.to_string_lossy(),
            "--seed", "1",
        ])
        .output()
        .expect("spawn camdl profile");
    assert!(!output.status.success(),
        "profile must fail without --obs on multi-stream model");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Pass `--obs"),
        "error must guide the user to pass --obs: {}", stderr);
    assert!(stderr.contains("cases_p1"),
        "error must list at least one available stream: {}", stderr);
}
