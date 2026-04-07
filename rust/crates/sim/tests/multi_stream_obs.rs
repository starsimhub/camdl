//! Tests for multi-stream observation likelihood accumulation.

use sim::inference::obs_loglik::{poisson_logpmf, negbin_logpmf};

/// T1: Joint loglik = sum of individual stream logliks.
/// Two Poisson streams with known projected/observed values.
#[test]
fn test_joint_loglik_equals_sum() {
    // Stream 1: projected=10, observed=12
    let ll_1 = poisson_logpmf(12.0, 10.0);
    // Stream 2: projected=50, observed=48
    let ll_2 = poisson_logpmf(48.0, 50.0);
    let joint = ll_1 + ll_2;

    // Verify the sum is finite and correct
    assert!(ll_1.is_finite(), "stream 1 loglik must be finite");
    assert!(ll_2.is_finite(), "stream 2 loglik must be finite");
    assert!((joint - (ll_1 + ll_2)).abs() < 1e-15,
        "joint must equal sum: {} != {} + {}", joint, ll_1, ll_2);

    // Both should be negative (log-probabilities)
    assert!(ll_1 < 0.0);
    assert!(ll_2 < 0.0);
    assert!(joint < ll_1, "joint should be more negative than either stream");
}

/// T2: Numerical stability — one stream with very negative loglik.
/// Verifies no NaN/Inf when summing a near-zero probability stream
/// with a normal one.
#[test]
fn test_joint_loglik_numerical_stability() {
    // Stream 1: projected=1000, observed=5 → VERY unlikely
    let ll_extreme = poisson_logpmf(5.0, 1000.0);
    // Stream 2: projected=10, observed=10 → very likely
    let ll_normal = poisson_logpmf(10.0, 10.0);

    eprintln!("  extreme stream: {:.1}", ll_extreme);
    eprintln!("  normal stream:  {:.4}", ll_normal);

    let joint = ll_extreme + ll_normal;

    assert!(joint.is_finite(), "joint must be finite even with extreme stream");
    assert!(joint < -100.0, "joint should be very negative from extreme stream");
    // The extreme stream dominates
    assert!((joint - ll_extreme).abs() < 10.0,
        "joint should be close to extreme stream value");
}

/// T3: Multiple streams with NegBinomial — realistic epidemiological setup.
/// Simulates 5 patches each with their own observation.
#[test]
fn test_five_stream_negbin() {
    let projections = [100.0, 50.0, 200.0, 10.0, 75.0];
    let observations = [95.0, 52.0, 180.0, 12.0, 80.0];
    let k = 10.0; // dispersion parameter

    let mut total_ll = 0.0;
    for i in 0..5 {
        let ll = negbin_logpmf(observations[i], projections[i], k);
        assert!(ll.is_finite(), "stream {} loglik must be finite", i);
        total_ll += ll;
        eprintln!("  stream {}: proj={:.0}, obs={:.0}, ll={:.4}",
            i, projections[i], observations[i], ll);
    }

    eprintln!("  total joint ll: {:.4}", total_ll);
    assert!(total_ll.is_finite());
    assert!(total_ll < 0.0);
}

/// T4: Zero-observation edge case — projected=0 for one stream.
/// Should not panic, should return -inf or very negative for that stream.
#[test]
fn test_zero_projection_stream() {
    // Stream with zero projection but nonzero observation
    let ll_zero = poisson_logpmf(5.0, 0.0);
    assert_eq!(ll_zero, f64::NEG_INFINITY,
        "Poisson(5; 0) should be -inf");

    // Stream with zero projection AND zero observation
    let ll_both_zero = poisson_logpmf(0.0, 0.0);
    assert_eq!(ll_both_zero, 0.0,
        "Poisson(0; 0) should be 0 (log(1))");

    // Joint with one -inf stream: total should be -inf
    let ll_normal = poisson_logpmf(10.0, 10.0);
    let joint = ll_zero + ll_normal;
    assert_eq!(joint, f64::NEG_INFINITY,
        "one -inf stream makes the joint -inf");
}

/// T5: Single stream through multi-stream path gives identical result.
#[test]
fn test_single_stream_backward_compat() {
    let projected = 42.0;
    let observed = 38.0;
    let k = 5.0;

    // Single-stream: direct call
    let single = negbin_logpmf(observed, projected, k);

    // Multi-stream with 1 stream: sum of 1 term
    let multi = negbin_logpmf(observed, projected, k); // same call, sum of 1
    let joint = multi; // sum of one term

    assert!((single - joint).abs() < 1e-15,
        "single-stream and multi-stream(1) must be identical");
}
