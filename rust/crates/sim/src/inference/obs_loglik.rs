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
}
