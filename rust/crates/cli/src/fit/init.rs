//! Chain / per-cell init strategies.
//!
//! Three modes today, dispatched via the `init_method` field on
//! `[stages.X]` (and the `--init` CLI override):
//!
//! - **Single** — every chain starts at `config.estimated_params[*].initial`
//!   (i.e. `[estimate].start =` or its fallback). Chains differ only by
//!   IF2's per-chain RNG. Useful for refine stages and reproducibility.
//! - **Uniform** — per-chain uniform random draw within natural-scale bounds.
//!   Today's default for scout (`effective_starts.is_none() && chains > 1`).
//!   Adequate when the parameter scale is linear and the basin is not too
//!   pathological.
//! - **Lhs** — Latin-hypercube stratified sampling, **scale-aware via
//!   `Transform`**. For Log-typed params (rates, positive quantities) LHS
//!   spans `[ln(lo), ln(hi)]` and exponentiates back, so a single LHS pass
//!   covers orders of magnitude rather than concentrating mass near `hi`.
//!   For Logit-typed params (probabilities) LHS spans `[lo, hi]` linearly.
//!   For untransformed params LHS spans `[lo, hi]`.
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
/// Default `Uniform` matches today's scout behaviour (per-chain uniform
/// random within natural-scale bounds), so existing fit.toml files
/// without `init_method` set keep their current semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum InitMethod {
    Single,
    Uniform,
    Lhs,
}

impl Default for InitMethod {
    fn default() -> Self { InitMethod::Uniform }
}

impl std::str::FromStr for InitMethod {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "single"  => Ok(InitMethod::Single),
            "uniform" => Ok(InitMethod::Uniform),
            "lhs"     => Ok(InitMethod::Lhs),
            other => Err(format!(
                "unknown init_method '{}': expected one of single, uniform, lhs",
                other)),
        }
    }
}

impl std::fmt::Display for InitMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            InitMethod::Single  => "single",
            InitMethod::Uniform => "uniform",
            InitMethod::Lhs     => "lhs",
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
    }
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
    fn init_method_default_is_uniform() {
        assert_eq!(InitMethod::default(), InitMethod::Uniform);
    }

    #[test]
    fn init_method_from_str_round_trip() {
        for m in [InitMethod::Single, InitMethod::Uniform, InitMethod::Lhs] {
            let s = m.to_string();
            let parsed: InitMethod = s.parse().unwrap();
            assert_eq!(parsed, m);
        }
        assert!("unknown".parse::<InitMethod>().is_err());
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
}
