//! Time-to-step conversion — the single entrypoint for mapping
//! continuous time to integer step indices.
//!
//! This module exists because the alternative — inlining
//! `(t / dt).round() as i64` at every site that needs the conversion
//! — produced gh#53: `compiled_model.rs:561` baked fire-step indices
//! at compile time using `model.simulation.dt`, but the runtime
//! integrator's dt could differ (every `camdl pfilter --dt 0.5` run on
//! a model declared at `dt = 1.0`). The result was a sub-day-step bug
//! invisible to synth-recovery and single-dt benchmarks but visible
//! against pomp on He et al. 2010 measles (5862 nat divergence at the
//! literature MLE; gh#52 Richardson ladder caught it).
//!
//! Funnel every continuous-time → step-index conversion through
//! [`time_to_step`]. The conversion is trivial; consolidating it
//! gives one place to invariant-test, one place to fix if the
//! semantics ever change, and one place agents and reviewers know to
//! audit.

/// Map continuous time `t` (in the model's `time_unit`, typically days
/// or years) to the integer step index for an integrator running at
/// step size `dt` (same unit). Rounds to the nearest step — interventions
/// fire in whichever step contains them.
///
/// Panics on non-finite `t` or non-positive `dt`. Both are caller bugs:
/// non-finite `t` came from somewhere that should have validated;
/// non-positive `dt` would put the integrator in an infinite loop
/// regardless of this function.
#[inline]
pub fn time_to_step(t: f64, dt: f64) -> i64 {
    debug_assert!(t.is_finite(), "time_to_step: non-finite t = {}", t);
    debug_assert!(dt > 0.0, "time_to_step: non-positive dt = {}", dt);
    (t / dt).round() as i64
}

/// Map a list of continuous fire times to a sorted, deduplicated
/// `BTreeSet` of step indices for integrator step `dt`. Used by
/// [`crate::compiled_model::CompiledModel::resolve_fire_steps`] to
/// derive the runtime view of a (compile-time, dt-invariant) fire-time
/// schedule.
pub fn fire_times_to_steps(times: &[f64], dt: f64) -> std::collections::BTreeSet<i64> {
    times.iter().map(|&t| time_to_step(t, dt)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_to_step_at_dt_1_is_identity_on_integers() {
        assert_eq!(time_to_step(0.0, 1.0), 0);
        assert_eq!(time_to_step(1.0, 1.0), 1);
        assert_eq!(time_to_step(258.0, 1.0), 258);
    }

    #[test]
    fn time_to_step_at_sub_day_dt_scales_correctly() {
        // The bug-fingerprint test: at dt=0.5, day 258 should map to
        // step 516, NOT step 258 (the gh#53 bug). The compile-time
        // pre-baked fire_steps had 258 as a step index and the
        // runtime walked at dt=0.5, so the impulse fired at wall
        // time 129 (= 258 * 0.5). With this helper used uniformly,
        // that confusion can't happen.
        assert_eq!(time_to_step(258.0, 0.5), 516);
        assert_eq!(time_to_step(258.0, 0.25), 1032);
        assert_eq!(time_to_step(258.0, 0.125), 2064);
    }

    #[test]
    fn time_to_step_rounds_to_nearest() {
        // 0.5*dt below a step boundary rounds up; at-or-above a step
        // boundary stays. The choice (round vs floor) matches pomp's
        // convention (`fabs(t - target) < 0.5*dt`) where the firing
        // step is the one that *contains* the target.
        assert_eq!(time_to_step(7.4, 1.0), 7);
        assert_eq!(time_to_step(7.5, 1.0), 8);  // banker's rounding ties to even? rust f64::round → 8
        assert_eq!(time_to_step(7.6, 1.0), 8);
    }

    #[test]
    fn time_to_step_at_zero_dt_panics_in_debug() {
        // Defensive: dt = 0 would put the integrator in an infinite
        // loop. Catching at the conversion point gives a clearer
        // panic site than a stuck simulator.
        let result = std::panic::catch_unwind(|| time_to_step(1.0, 0.0));
        assert!(result.is_err());
    }

    #[test]
    fn time_to_step_at_negative_dt_panics_in_debug() {
        let result = std::panic::catch_unwind(|| time_to_step(1.0, -0.5));
        assert!(result.is_err());
    }

    #[test]
    fn time_to_step_at_nan_t_panics_in_debug() {
        // Rm4 in 2026-04-19 engine review: NaN as i64 is 0 on
        // current rustc, which would silently match step 0 in any
        // fire-step-checking code. The debug_assert catches it.
        let result = std::panic::catch_unwind(|| time_to_step(f64::NAN, 1.0));
        assert!(result.is_err());
    }

    // ── fire_times_to_steps ─────────────────────────────────────────

    #[test]
    fn fire_times_to_steps_resolves_periodic_schedule_dt_invariantly() {
        // Cohort entry at day 258 every 365.25 days. Three fires:
        // days 258, 623.25, 988.5. At any dt that divides into 258
        // cleanly, all three fires should map to distinct step
        // indices.
        let fire_times = vec![258.0, 623.25, 988.5];

        let steps_dt1   = fire_times_to_steps(&fire_times, 1.0);
        let steps_dt05  = fire_times_to_steps(&fire_times, 0.5);
        let steps_dt025 = fire_times_to_steps(&fire_times, 0.25);

        // Each ladder produces the same number of distinct fires —
        // exactly 3 — regardless of dt. (The gh#53 bug was that
        // these would alias incorrectly under finer dt.)
        assert_eq!(steps_dt1.len(), 3);
        assert_eq!(steps_dt05.len(), 3);
        assert_eq!(steps_dt025.len(), 3);

        // The wall times recovered from the steps must equal the
        // input times (within rounding) at every dt.
        for (steps, dt) in [(&steps_dt1, 1.0), (&steps_dt05, 0.5), (&steps_dt025, 0.25)] {
            let recovered: Vec<f64> = steps.iter().map(|&s| s as f64 * dt).collect();
            for (orig, rec) in fire_times.iter().zip(&recovered) {
                assert!((orig - rec).abs() <= 0.5 * dt,
                    "dt={}: fire time {} → step → wall {} drifted >0.5*dt",
                    dt, orig, rec);
            }
        }
    }

    #[test]
    fn fire_times_to_steps_dedups_collisions() {
        // Two fire times that round to the same step at coarse dt
        // collapse to a single entry in the BTreeSet. Documented
        // behaviour: the set semantics inherently dedup. No fire
        // is "lost" because BTreeSet membership is what the runtime
        // checks, not a count — one fire per step.
        let fire_times = vec![100.0, 100.3];  // both round to step 100 at dt=1
        let steps = fire_times_to_steps(&fire_times, 1.0);
        assert_eq!(steps.len(), 1);
        assert!(steps.contains(&100));
    }
}
