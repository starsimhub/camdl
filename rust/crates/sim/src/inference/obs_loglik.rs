//! Observation log-likelihood functions.
//!
//! These evaluate log p(y | projected, θ) for a single observation.
//! No external dependencies — lgamma implemented inline for stability.

use std::f64::consts::PI;

/// Log-gamma function via Stirling's approximation with Lanczos correction.
/// Accurate to ~15 significant digits for x > 0.5.
pub fn lgamma(x: f64) -> f64 {
    // Lanczos approximation (g=7, n=9) — same coefficients as Numerical Recipes.
    const G: f64 = 7.0;
    const COEFFS: [f64; 9] = [
        0.99999999999980993,
        676.5203681218851,
        -1259.1392167224028,
        771.32342877765313,
        -176.61502916214059,
        12.507343278686905,
        -0.13857109526572012,
        9.9843695780195716e-6,
        1.5056327351493116e-7,
    ];

    if x < 0.5 {
        // Reflection formula: Γ(x)Γ(1-x) = π / sin(πx)
        return (PI / (PI * x).sin()).ln() - lgamma(1.0 - x);
    }

    let x = x - 1.0;
    let mut sum = COEFFS[0];
    for i in 1..9 {
        sum += COEFFS[i] / (x + i as f64);
    }
    let t = x + G + 0.5;
    0.5 * (2.0 * PI).ln() + (t.ln() * (x + 0.5)) - t + sum.ln()
}

/// Negative binomial log-PMF.
///
/// Parameterization: mean = mu, size = k (dispersion parameter).
/// As k → ∞, NegBin(mu, k) → Poisson(mu).
///
/// log p(y | mu, k) = lgamma(y+k) - lgamma(y+1) - lgamma(k)
///                   + k·log(k/(k+mu)) + y·log(mu/(k+mu))
pub fn negbin_logpmf(y: f64, mu: f64, k: f64) -> f64 {
    if mu <= 0.0 {
        return if y.round() == 0.0 { 0.0 } else { f64::NEG_INFINITY };
    }
    if k <= 0.0 { return f64::NEG_INFINITY; }

    let y = y.round().max(0.0);
    let p = k / (k + mu);

    lgamma(y + k) - lgamma(y + 1.0) - lgamma(k)
        + k * p.ln()
        + y * (1.0 - p).ln()
}

/// Normal log-PDF.
///
/// log p(y | mu, sigma) = -0.5·((y-mu)/sigma)² - log(sigma) - 0.5·log(2π)
pub fn normal_logpdf(y: f64, mu: f64, sigma: f64) -> f64 {
    if sigma <= 0.0 { return f64::NEG_INFINITY; }
    -0.5 * ((y - mu) / sigma).powi(2) - sigma.ln() - 0.5 * (2.0 * PI).ln()
}

/// Standard normal CDF via the error function.
///
/// Φ(x) = 0.5 × (1 + erf(x / √2))
///
/// Uses a rational approximation to erf (Abramowitz & Stegun 7.1.26,
/// max error < 1.5e-7). No external dependencies.
pub fn normal_cdf(x: f64) -> f64 {
    // Φ(x) = 0.5 × (1 + erf(x / √2))
    // erf approximation: Abramowitz & Stegun 7.1.26, max error < 1.5e-7
    let z = x / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.3275911 * z.abs());
    let poly = t * (0.254829592
        + t * (-0.284496736
        + t * (1.421413741
        + t * (-1.453152027
        + t * 1.061405429))));
    let erf_abs = 1.0 - poly * (-z * z).exp();
    let erf_val = if z >= 0.0 { erf_abs } else { -erf_abs };
    0.5 * (1.0 + erf_val)
}

/// Discretized Normal log-PMF (He et al. 2010 observation model).
///
/// P(y | mean, variance) = Φ((y+0.5-μ)/σ) - Φ((y-0.5-μ)/σ)  for y > 0
///                        = Φ((0.5-μ)/σ)                       for y = 0
///
/// The ±0.5 continuity correction discretizes a continuous Normal
/// onto integer case counts. The variance is typically heteroscedastic:
///
///   variance = ρ·C·(1 - ρ + ψ²·ρ·C)
///
/// where C is the true incidence projection, ρ is reporting probability,
/// and ψ is the overdispersion coefficient. This gives tight observations
/// during inter-epidemic troughs (binomial sampling dominates) and loose
/// observations during peaks (correlated reporting noise dominates).
/// Default likelihood tolerance — matches pomp's `tol` parameter.
/// Exposed as `--tol` on the CLI for models where a different floor is needed.
pub const DEFAULT_TOL: f64 = 1e-18;

pub fn discretized_normal_logpmf(y: f64, mean: f64, variance: f64) -> f64 {
    discretized_normal_logpmf_tol(y, mean, variance, DEFAULT_TOL)
}

/// Discretized Normal log-PMF with configurable tolerance floor.
///
/// `tol` is the minimum probability before taking log. At 1e-18 (pomp's
/// default), particles that predict ~0 when data shows 80 get log-weight
/// ≈ -41 regardless of exactly how wrong they are. At 1e-300, the gap
/// between "zero" and "nearly zero" is 650 log-units, which collapses ESS.
///
/// For large-population models (London measles), 1e-18 is correct.
/// For small-population models where observing 3 vs 0 is informative,
/// a tighter tolerance (e.g., 1e-30) preserves that signal.
pub fn discretized_normal_logpmf_tol(y: f64, mean: f64, variance: f64, tol: f64) -> f64 {
    let sd = variance.max(1e-30).sqrt();
    let y = y.round().max(0.0);

    let prob = if y > 0.0 {
        let upper = normal_cdf((y + 0.5 - mean) / sd);
        let lower = normal_cdf((y - 0.5 - mean) / sd);
        (upper - lower).max(tol)
    } else {
        normal_cdf((0.5 - mean) / sd).max(tol)
    };

    prob.ln()
}

/// Binomial log-PMF: log P(X = k) where X ~ Binom(n, p).
///
/// log p(k | n, p) = lgamma(n+1) - lgamma(k+1) - lgamma(n-k+1)
///                  + k·log(p) + (n-k)·log(1-p)
///
/// Used by PGAS for transition density evaluation.
pub fn binom_logpmf(k: u64, n: u64, p: f64) -> f64 {
    if k > n { return f64::NEG_INFINITY; }
    if p <= 0.0 { return if k == 0 { 0.0 } else { f64::NEG_INFINITY }; }
    if p >= 1.0 { return if k == n { 0.0 } else { f64::NEG_INFINITY }; }
    lgamma(n as f64 + 1.0) - lgamma(k as f64 + 1.0) - lgamma((n - k) as f64 + 1.0)
        + k as f64 * p.ln() + (n - k) as f64 * (1.0 - p).ln()
}

/// Poisson log-PMF.
///
/// log p(y | lambda) = y·log(lambda) - lambda - lgamma(y+1)
pub fn poisson_logpmf(y: f64, lambda: f64) -> f64 {
    if lambda <= 0.0 {
        return if y.round() == 0.0 { 0.0 } else { f64::NEG_INFINITY };
    }
    let y = y.round().max(0.0);
    y * lambda.ln() - lambda - lgamma(y + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lgamma_known_values() {
        // lgamma(1) = 0, lgamma(2) = 0, lgamma(5) = log(24) = 3.178...
        assert!((lgamma(1.0) - 0.0).abs() < 1e-10);
        assert!((lgamma(2.0) - 0.0).abs() < 1e-10);
        assert!((lgamma(5.0) - 24.0_f64.ln()).abs() < 1e-10);
        assert!((lgamma(0.5) - (PI.sqrt().ln())).abs() < 1e-10);
    }

    #[test]
    fn test_negbin_logpmf_known_values() {
        // Reference: Python math.lgamma-based computation
        // negbin(10, mu=20, k=5) = -3.369870
        let ll = negbin_logpmf(10.0, 20.0, 5.0);
        assert!((ll - (-3.369870)).abs() < 1e-4,
            "negbin_logpmf(10, 20, 5) = {}, expected -3.370", ll);

        // negbin(0, mu=5, k=2): p = 2/7, ll = lgamma(2)-lgamma(1)-lgamma(2) + 2*ln(2/7)
        let ll = negbin_logpmf(0.0, 5.0, 2.0);
        let expected = 2.0 * (2.0_f64 / 7.0).ln();
        assert!((ll - expected).abs() < 1e-4,
            "negbin_logpmf(0, 5, 2) = {}, expected {}", ll, expected);
    }

    #[test]
    fn test_normal_logpdf_known() {
        // N(0, 1): log p(0) = -0.5*log(2π) = -0.9189
        let ll = normal_logpdf(0.0, 0.0, 1.0);
        assert!((ll - (-0.9189385)).abs() < 1e-5);

        // N(5, 2): log p(5) = -log(2) - 0.5*log(2π) = -1.612
        let ll = normal_logpdf(5.0, 5.0, 2.0);
        assert!((ll - (-1.612086)).abs() < 1e-4);
    }

    #[test]
    fn test_poisson_logpmf_known() {
        // poisson(5, lambda=3): 5*ln(3) - 3 - lgamma(6) = -2.2944
        let ll = poisson_logpmf(5.0, 3.0);
        assert!((ll - (-2.2944)).abs() < 1e-3,
            "poisson_logpmf(5, 3) = {}, expected -2.294", ll);
    }

    #[test]
    fn test_negbin_mu_zero_y_zero() {
        assert_eq!(negbin_logpmf(0.0, 0.0, 5.0), 0.0);
    }

    #[test]
    fn test_negbin_mu_zero_y_nonzero() {
        assert_eq!(negbin_logpmf(10.0, 0.0, 5.0), f64::NEG_INFINITY);
    }

    #[test]
    fn test_normal_cdf_known() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-6);
        assert!((normal_cdf(1.96) - 0.975).abs() < 1e-3);
        assert!((normal_cdf(-1.96) - 0.025).abs() < 1e-3);
        assert!(normal_cdf(10.0) > 0.9999);
        assert!(normal_cdf(-10.0) < 0.0001);
    }

    #[test]
    fn test_discretized_normal_matches_scipy() {
        // Reference: scipy.stats.norm.cdf with He et al. variance formula
        // variance = rho * C * (1 - rho + psi^2 * rho * C)
        // rho=0.488, psi=0.116

        // y=100, C=200: mean=97.6, var=178.1 (trough — tight observation)
        assert!((discretized_normal_logpmf(100.0, 97.6, 178.1) - (-3.526643)).abs() < 1e-2,
            "y=100, C=200");

        // y=0, C=5: mean=2.4, var=1.3 (near-zero — very tight, tail of CDF)
        assert!((discretized_normal_logpmf(0.0, 2.4, 1.3) - (-3.074161)).abs() < 0.05,
            "y=0, C=5: got {}", discretized_normal_logpmf(0.0, 2.4, 1.3));

        // y=500, C=1000: mean=488.0, var=3454.3 (moderate incidence)
        assert!((discretized_normal_logpmf(500.0, 488.0, 3454.3) - (-5.013484)).abs() < 1e-2,
            "y=500, C=1000");

        // y=10, C=20: mean=9.8, var=6.3 (low count, binomial regime)
        assert!((discretized_normal_logpmf(10.0, 9.8, 6.3) - (-1.848681)).abs() < 1e-2,
            "y=10, C=20");

        // y=2000, C=4000: mean=1952.0, var=52270.9 (peak — loose observation)
        assert!((discretized_normal_logpmf(2000.0, 1952.0, 52270.9) - (-6.373076)).abs() < 1e-2,
            "y=2000, C=4000");
    }

    #[test]
    fn test_discretized_normal_zero_variance_safe() {
        // Should not panic or return NaN
        let ll = discretized_normal_logpmf(5.0, 5.0, 0.0);
        assert!(ll.is_finite());
    }

    #[test]
    fn test_binom_logpmf_known() {
        // Binom(5, 10, 0.3): lgamma-based = -2.2738
        let ll = binom_logpmf(5, 10, 0.3);
        assert!((ll - (-2.2738)).abs() < 1e-3,
            "binom_logpmf(5, 10, 0.3) = {}, expected -2.274", ll);
    }

    #[test]
    fn test_binom_logpmf_boundaries() {
        assert_eq!(binom_logpmf(0, 10, 0.0), 0.0);
        assert_eq!(binom_logpmf(5, 10, 0.0), f64::NEG_INFINITY);
        assert_eq!(binom_logpmf(10, 10, 1.0), 0.0);
        assert_eq!(binom_logpmf(5, 10, 1.0), f64::NEG_INFINITY);
        assert_eq!(binom_logpmf(11, 10, 0.5), f64::NEG_INFINITY);
        // Binom(0, 0, p) = 1 for any p (within floating point tolerance)
        assert!((binom_logpmf(0, 0, 0.5)).abs() < 1e-14);
    }
}
