//! Validate all `InterpMethod` variants against numpy / scipy references.
//!
//! Cubic spline has its own suite (`cubic_spline.rs`). This file covers
//! linear and constant interpolation, comparing `eval_time_func` output
//! to `np.interp` and `scipy.interpolate.interp1d(kind="previous")`
//! respectively. Closes an audit gap — only Spline had reference tests
//! pre-2026-04-21. See
//! `docs/dev/reviews/2026-04-21-spec-claims-vs-tests.md`.
//!
//! Reference values generated with:
//!   import numpy as np
//!   xs = np.array([0.0, 1.0, 3.0, 5.0, 8.0, 10.0])
//!   ys = np.array([2.0, 3.5, 2.8, 4.2, 3.1, 5.0])
//!   np.interp(t, xs, ys)             # linear
//!   # previous-value step: np.searchsorted + ys[i-1]

use sim::compiled_model::CompiledTimeFuncKind;
use sim::propensity::eval_time_func;

fn fixture_linear() -> CompiledTimeFuncKind {
    CompiledTimeFuncKind::Interpolated {
        times:  vec![0.0, 1.0, 3.0, 5.0, 8.0, 10.0],
        values: vec![2.0, 3.5, 2.8, 4.2, 3.1, 5.0],
    }
}

fn fixture_constant() -> CompiledTimeFuncKind {
    CompiledTimeFuncKind::Constant {
        times:  vec![0.0, 1.0, 3.0, 5.0, 8.0, 10.0],
        values: vec![2.0, 3.5, 2.8, 4.2, 3.1, 5.0],
    }
}

#[test]
fn linear_matches_numpy_interp() {
    let kind = fixture_linear();
    // (t, np.interp(t, xs, ys))
    let reference: Vec<(f64, f64)> = vec![
        ( 0.0, 2.0000000000),
        ( 0.5, 2.7500000000),
        ( 1.0, 3.5000000000),
        ( 2.0, 3.1500000000),
        ( 3.0, 2.8000000000),
        ( 4.0, 3.5000000000),
        ( 5.0, 4.2000000000),
        ( 6.5, 3.6500000000),
        ( 8.0, 3.1000000000),
        ( 9.0, 4.0500000000),
        (10.0, 5.0000000000),
    ];
    for (t, expected) in reference {
        let actual = eval_time_func(&kind, t);
        assert!(
            (actual - expected).abs() < 1e-9,
            "linear({}) = {}, expected {} (numpy); diff = {:.2e}",
            t, actual, expected, (actual - expected).abs()
        );
    }
}

#[test]
fn linear_clamps_to_endpoints_out_of_bounds() {
    // np.interp default: left-clamp to ys[0], right-clamp to ys[-1].
    let kind = fixture_linear();
    assert!((eval_time_func(&kind, -1.0) - 2.0).abs() < 1e-12);
    assert!((eval_time_func(&kind, 11.0) - 5.0).abs() < 1e-12);
    assert!((eval_time_func(&kind, -1e9) - 2.0).abs() < 1e-12);
    assert!((eval_time_func(&kind,  1e9) - 5.0).abs() < 1e-12);
}

#[test]
fn linear_passes_through_knots_exactly() {
    // At each knot t = xs[i], result must equal ys[i] with zero floating
    // drift — linear interp is piecewise affine; no numerical error at grid.
    let kind = fixture_linear();
    let xs = [0.0, 1.0, 3.0, 5.0, 8.0, 10.0];
    let ys = [2.0, 3.5, 2.8, 4.2, 3.1, 5.0];
    for (x, y) in xs.iter().zip(&ys) {
        let actual = eval_time_func(&kind, *x);
        assert_eq!(actual, *y, "linear at knot x={}: got {}, expected {}",
            x, actual, y);
    }
}

#[test]
fn constant_matches_previous_value_step() {
    let kind = fixture_constant();
    // Previous-value step: at t, return ys[i] for largest i with xs[i] <= t.
    // Ties at knots: t = xs[i] returns ys[i] (step UP at the knot).
    let reference: Vec<(f64, f64)> = vec![
        ( 0.0, 2.0),
        ( 0.5, 2.0),  // before xs[1]=1: value at xs[0]
        ( 1.0, 3.5),  // at knot 1: steps to ys[1]
        ( 2.0, 3.5),  // (1, 3) → ys[1]
        ( 3.0, 2.8),  // at knot 3: steps to ys[2]
        ( 4.0, 2.8),
        ( 5.0, 4.2),
        ( 6.5, 4.2),
        ( 8.0, 3.1),
        ( 9.0, 3.1),
        (10.0, 5.0),
    ];
    for (t, expected) in reference {
        let actual = eval_time_func(&kind, t);
        assert_eq!(actual, expected,
            "constant({}) = {}, expected {}", t, actual, expected);
    }
}

#[test]
fn constant_clamps_to_endpoints_out_of_bounds() {
    let kind = fixture_constant();
    assert_eq!(eval_time_func(&kind, -1.0), 2.0);
    assert_eq!(eval_time_func(&kind, 11.0), 5.0);
}

#[test]
fn degenerate_single_knot() {
    // Single knot: linear and constant should both return that value everywhere.
    let lin = CompiledTimeFuncKind::Interpolated {
        times: vec![5.0], values: vec![42.0],
    };
    let con = CompiledTimeFuncKind::Constant {
        times: vec![5.0], values: vec![42.0],
    };
    for &t in &[-10.0, 0.0, 5.0, 10.0, 100.0] {
        assert_eq!(eval_time_func(&lin, t), 42.0);
        assert_eq!(eval_time_func(&con, t), 42.0);
    }
}

#[test]
fn empty_returns_zero() {
    let lin = CompiledTimeFuncKind::Interpolated {
        times: vec![], values: vec![],
    };
    let con = CompiledTimeFuncKind::Constant {
        times: vec![], values: vec![],
    };
    assert_eq!(eval_time_func(&lin, 5.0), 0.0);
    assert_eq!(eval_time_func(&con, 5.0), 0.0);
}
