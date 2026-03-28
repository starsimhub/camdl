//! Validate CubicSpline against scipy.interpolate.CubicSpline(bc_type='natural').
//!
//! Reference values generated with:
//!   from scipy.interpolate import CubicSpline
//!   xs = [0.0, 1.0, 3.0, 5.0, 8.0, 10.0]
//!   ys = [2.0, 3.5, 2.8, 4.2, 3.1, 5.0]
//!   cs = CubicSpline(xs, ys, bc_type='natural')

use sim::compiled_model::CubicSpline;

#[test]
fn test_cubic_spline_matches_scipy() {
    let xs = vec![0.0, 1.0, 3.0, 5.0, 8.0, 10.0];
    let ys = vec![2.0, 3.5, 2.8, 4.2, 3.1, 5.0];
    let spline = CubicSpline::new(&xs, &ys);

    // (t, expected from scipy) — tolerance 1e-6
    let reference: Vec<(f64, f64)> = vec![
        (0.0,  2.0000000000),
        (0.5,  2.9016704304),
        (1.0,  3.5000000000),
        (2.0,  3.3241365569),
        (3.0,  2.8000000000),
        (4.0,  3.4034537726),
        (5.0,  4.2000000000),
        (6.5,  3.7348233262),
        (8.0,  3.1000000000),
        (9.0,  3.7517003188),
        (10.0, 5.0000000000),
    ];

    for (t, expected) in &reference {
        let actual = spline.eval(*t);
        assert!(
            (actual - expected).abs() < 1e-6,
            "spline({}) = {}, expected {} (scipy), diff = {:.2e}",
            t, actual, expected, (actual - expected).abs()
        );
    }
}

#[test]
fn test_cubic_spline_interpolates_knots_exactly() {
    let xs = vec![0.0, 1.0, 3.0, 5.0, 8.0, 10.0];
    let ys = vec![2.0, 3.5, 2.8, 4.2, 3.1, 5.0];
    let spline = CubicSpline::new(&xs, &ys);

    for (x, y) in xs.iter().zip(&ys) {
        let actual = spline.eval(*x);
        assert!(
            (actual - y).abs() < 1e-12,
            "spline({}) = {}, expected {} (knot)", x, actual, y
        );
    }
}

#[test]
fn test_cubic_spline_two_points_is_linear() {
    let xs = vec![0.0, 10.0];
    let ys = vec![1.0, 5.0];
    let spline = CubicSpline::new(&xs, &ys);

    // Linear: y = 1.0 + 0.4 * t
    for t in [0.0, 2.5, 5.0, 7.5, 10.0] {
        let expected = 1.0 + 0.4 * t;
        let actual = spline.eval(t);
        assert!(
            (actual - expected).abs() < 1e-12,
            "spline({}) = {}, expected {} (linear)", t, actual, expected
        );
    }
}

#[test]
fn test_cubic_spline_boundary_clamping() {
    let xs = vec![1.0, 3.0, 5.0];
    let ys = vec![10.0, 20.0, 15.0];
    let spline = CubicSpline::new(&xs, &ys);

    // Before first knot → first y value
    assert_eq!(spline.eval(0.0), 10.0);
    assert_eq!(spline.eval(-100.0), 10.0);

    // After last knot → last y value
    assert_eq!(spline.eval(6.0), 15.0);
    assert_eq!(spline.eval(1000.0), 15.0);
}

#[test]
#[should_panic(expected = "strictly increasing")]
fn test_cubic_spline_rejects_duplicate_x() {
    let xs = vec![0.0, 1.0, 1.0, 3.0];
    let ys = vec![1.0, 2.0, 2.0, 4.0];
    CubicSpline::new(&xs, &ys);
}

#[test]
#[should_panic(expected = "strictly increasing")]
fn test_cubic_spline_rejects_decreasing_x() {
    let xs = vec![0.0, 3.0, 2.0, 5.0];
    let ys = vec![1.0, 2.0, 3.0, 4.0];
    CubicSpline::new(&xs, &ys);
}
