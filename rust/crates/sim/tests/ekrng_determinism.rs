//! EKRNG determinism and order-independence tests (§A.3).

use sim::ekrng::EkRng;

#[test]
fn test_same_key_same_value() {
    let ekrng = EkRng::new(42);
    let key = "infection_child:1337";
    let v1 = ekrng.exp_keyed(key, 0, 1.0);
    let v2 = ekrng.exp_keyed(key, 0, 1.0);
    assert_eq!(v1, v2, "same (seed, key, counter) must produce identical draw");
}

#[test]
fn test_different_keys_different_values() {
    let ekrng = EkRng::new(42);
    let v1 = ekrng.exp_keyed("infection:0", 0, 1.0);
    let v2 = ekrng.exp_keyed("recovery:0", 0, 1.0);
    assert_ne!(v1, v2, "different keys should (almost surely) produce different draws");
}

#[test]
fn test_different_seeds_different_values() {
    let e1 = EkRng::new(42);
    let e2 = EkRng::new(43);
    let key = "infection:0";
    assert_ne!(
        e1.exp_keyed(key, 0, 1.0),
        e2.exp_keyed(key, 0, 1.0),
        "different seeds must produce different draws"
    );
}

#[test]
fn test_order_independence() {
    // Drawing keys in two different orders must produce identical per-key values.
    let seed = 42u64;
    let keys = ["infection:0", "recovery:0", "birth:0", "infection:1"];
    let ekrng = EkRng::new(seed);

    // Order A: natural order
    let values_a: Vec<f64> = keys.iter().map(|k| ekrng.exp_keyed(k, 0, 1.0)).collect();

    // Order B: reversed
    let mut values_b: Vec<f64> = keys.iter().rev().map(|k| ekrng.exp_keyed(k, 0, 1.0)).collect();
    values_b.reverse();

    assert_eq!(values_a, values_b, "EKRNG draws must be order-independent");
}

#[test]
fn test_poisson_keyed_deterministic() {
    let ekrng = EkRng::new(99);
    let v1 = ekrng.poisson_keyed("infection:5", 3, 10.0);
    let v2 = ekrng.poisson_keyed("infection:5", 3, 10.0);
    assert_eq!(v1, v2);
}

#[test]
fn test_zero_lambda_returns_zero() {
    let ekrng = EkRng::new(1);
    assert_eq!(ekrng.poisson_keyed("x", 0, 0.0), 0);
    assert_eq!(ekrng.poisson_keyed("x", 0, -1.0), 0);
}

#[test]
fn test_zero_rate_returns_infinity() {
    let ekrng = EkRng::new(1);
    assert_eq!(ekrng.exp_keyed("x", 0, 0.0), f64::INFINITY);
    assert_eq!(ekrng.exp_keyed("x", 0, -1.0), f64::INFINITY);
}

#[test]
fn test_counter_distinguishes_draws() {
    // Same key, different counter → different values
    let ekrng = EkRng::new(42);
    let key = "infection:0";
    let v0 = ekrng.exp_keyed(key, 0, 1.0);
    let v1 = ekrng.exp_keyed(key, 1, 1.0);
    assert_ne!(v0, v1, "different counters must produce different draws for same key");
}
