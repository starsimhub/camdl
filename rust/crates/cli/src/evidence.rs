//! Evidence formatting — decibans alongside nats for model-comparison output.
//!
//! See `docs/dev/proposals/2026-04-23-evidence-in-decibans.md` for the full
//! rationale. Short version: model-comparison output should display
//! log-likelihood *differences* in decibans (dB, base-10 log-likelihood ratio
//! × 10) with the Jeffreys qualitative label, alongside the raw nats. This
//! captures the "evidence scale" framing Turing + Good + Jeffreys developed
//! and that Kass & Raftery (1995) use, while preserving nats as the primary
//! machine-readable unit for interop with pomp / Stan / NumPyro pipelines.
//!
//! Rule: dB appears only for *differences* between log-likelihoods (where the
//! Jacobian cancels and the value is a scale-free log-likelihood ratio), not
//! for raw absolute log-likelihoods (whose additive constant is arbitrary).
//!
//! Evidence scale (labels and thresholds). Attribution is split, and the
//! split matters — do not move labels without updating the citations:
//!
//!   0 – 5 dB    indeterminate   (odds 1:1 to 3:1)
//!   5 – 10 dB   substantial     (odds 3:1 to 10:1)
//!   10 – 15 dB  strong          (odds 10:1 to ~30:1)
//!   15 – 20 dB  very strong     (odds ~30:1 to 100:1)
//!   20 – 40 dB  decisive        (odds 100:1 to 10⁴:1)
//!   40+ dB      overwhelming    (odds > 10⁴:1)
//!
//! Tiers 1–5 (0/5/10/15/20 dB breakpoints) are **Jeffreys 1961**,
//! *Theory of Probability*, Appendix B. The Jeffreys original has
//! five tiers and the top one ("decisive") is unbounded: anything
//! above 20 dB is "decisive" in the historical scale.
//!
//! The 20–40 dB / 40+ dB split of Jeffreys' unbounded top tier into
//! "decisive" and "overwhelming" is a **camdl pedagogical
//! extension**, not Jeffreys' or anyone else's. The motivation is
//! that epi model comparisons on multi-year weekly data routinely
//! produce log-likelihood differences in the thousands of decibans —
//! Jeffreys' "decisive, 20 dB and up" qualitatively collapses the
//! "borderline significant" regime with the "10^150 times more
//! likely" regime, which teaches nothing about relative magnitude.
//! The 40 dB break is where cross-validation evidence ratios start
//! exceeding 10⁴:1 (log₁₀ BF > 4), which for typical epi likelihood
//! surfaces marks the point where "you have to work hard to
//! dismiss this" becomes "even an adversarial reviewer can't
//! reasonably dismiss this."
//!
//! Good (1950), *Probability and the Weighing of Evidence*, is the
//! primary source for decibans as a unit and the weight-of-evidence
//! framing; Jaynes (*Probability Theory: The Logic of Science*,
//! ch. 4) uses decibans extensively but does **not** publish a
//! labeled tier scale. Any attribution of "overwhelming" to Jaynes
//! is incorrect — the tier is camdl's own extension of Jeffreys.
//!
//! Kass & Raftery (1995, JASA) is the modern alternative: four
//! tiers at 2 ln(BF) thresholds of 2/6/10, which correspond to
//! 8.7/26.1/43.4 dB — not 5-dB boundaries, so the two scales are
//! not interchangeable without conversion.

/// Nats → decibans: 1 nat ≈ 4.342944819 dB.
pub const NATS_TO_DB: f64 = 10.0 / std::f64::consts::LN_10;

/// Numerically-stable `log(mean(exp(xs)))` = `logsumexp(xs) - ln(len)`.
///
/// Used to combine M replicate particle-filter log-likelihood estimates
/// into a single unbiased (on the likelihood scale) summary. The raw PF
/// log-lik estimator is *downward*-biased by `Var(ℓ)/2` on the log
/// scale even though `exp(ℓ)` is unbiased; combining on the likelihood
/// scale via `log(mean(exp(ℓ_k)))` recovers the unbiased combined
/// estimate. See proposal 2026-04-24-if2-scout-findings-remediation.md
/// §Proposal 1.
///
/// Returns `NEG_INFINITY` on empty input; `NaN` propagates through.
pub fn logmeanexp(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return f64::NEG_INFINITY;
    }
    let m = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !m.is_finite() {
        return m;
    }
    let sum_exp: f64 = xs.iter().map(|&x| (x - m).exp()).sum();
    m + (sum_exp / xs.len() as f64).ln()
}

/// Sample standard deviation (N-1 denominator). Returns 0.0 for len<2.
pub fn sample_sd(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    var.sqrt()
}

/// Jeffreys qualitative label for an evidence magnitude in decibans. The
/// input is the *signed* Δlog-lik in dB; the label describes the magnitude,
/// and the caller decides whether to prefix with "for" / "against" based on
/// the sign.
pub fn jeffreys_label(db: f64) -> &'static str {
    let m = db.abs();
    if      m <  5.0 { "indeterminate"  }
    else if m < 10.0 { "substantial"    }
    else if m < 15.0 { "strong"         }
    else if m < 20.0 { "very strong"    }
    else if m < 40.0 { "decisive"       }
    else             { "overwhelming"   }
}

/// Format a Δlog-likelihood (nats) as a single-line string with nats,
/// decibans, and the Jeffreys qualitative label. Intended for human-readable
/// output in `camdl compare`, fit-stage summaries, external-harness failure
/// messages, and any other context where a scale-free log-lik difference
/// carries evidential meaning.
///
/// The `label` arg is the metric name (e.g. "Δlogℒ", "Δelpd", "Δpreq");
/// returned string does not include leading/trailing whitespace. Caller
/// Compact two-column form for use inside tables or tight displays:
/// returns `(nats_str, db_with_label_str)` for a Δlog-lik.
///
/// Example: `("+27.300", "+118.6 dB, decisive")`.
pub fn evidence_cells(delta_nats: f64) -> (String, String) {
    if !delta_nats.is_finite() {
        return (format!("{}", delta_nats), "—".into());
    }
    let db = delta_nats * NATS_TO_DB;
    let tag = jeffreys_label(db);
    (format!("{:+.3}", delta_nats), format!("{:+.1} dB, {}", db, tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_factor_matches_10_over_ln10() {
        // 1 nat = 10 / ln(10) ≈ 4.342944819 dB.
        assert!((NATS_TO_DB - 4.342944819).abs() < 1e-9);
    }

    #[test]
    fn jeffreys_boundaries_symmetric() {
        // Each boundary is open on the lower side, closed on the upper:
        // < 5 → indeterminate; [5, 10) → substantial; etc.
        assert_eq!(jeffreys_label(0.0),     "indeterminate");
        assert_eq!(jeffreys_label(4.99),    "indeterminate");
        assert_eq!(jeffreys_label(5.0),     "substantial");
        assert_eq!(jeffreys_label(9.99),    "substantial");
        assert_eq!(jeffreys_label(10.0),    "strong");
        assert_eq!(jeffreys_label(14.99),   "strong");
        assert_eq!(jeffreys_label(15.0),    "very strong");
        assert_eq!(jeffreys_label(19.99),   "very strong");
        assert_eq!(jeffreys_label(20.0),    "decisive");
        assert_eq!(jeffreys_label(39.99),   "decisive");
        assert_eq!(jeffreys_label(40.0),    "overwhelming");
        assert_eq!(jeffreys_label(1000.0),  "overwhelming");
    }

    #[test]
    fn jeffreys_label_is_symmetric_in_sign() {
        // The label names magnitude only; the sign tells you which direction.
        for db in [6.0, 12.0, 17.0, 25.0, 50.0] {
            assert_eq!(jeffreys_label(db), jeffreys_label(-db),
                "label must be sign-symmetric at {} dB", db);
        }
    }

    #[test]
    fn evidence_cells_compact_form() {
        let (nats, db_label) = evidence_cells(5.5);
        assert_eq!(nats, "+5.500");
        assert_eq!(db_label, "+23.9 dB, decisive");
    }

    #[test]
    fn evidence_cells_handles_non_finite() {
        // NaN / ±∞ → "—" in the dB column, not a panic.
        let (n, db) = evidence_cells(f64::NAN);
        assert!(n.contains("NaN"));
        assert_eq!(db, "—");
        let (_, db) = evidence_cells(f64::INFINITY);
        assert_eq!(db, "—");
    }

    #[test]
    fn logmeanexp_constant_is_identity() {
        // mean of identical values equals the value itself.
        assert!((logmeanexp(&[0.0, 0.0, 0.0]) - 0.0).abs() < 1e-12);
        assert!((logmeanexp(&[-7.3, -7.3, -7.3, -7.3]) - (-7.3)).abs() < 1e-12);
    }

    #[test]
    fn logmeanexp_is_numerically_stable() {
        // Naive exp of -1e6 underflows to 0; stable form must not.
        // log((e^-1e6 + e^0)/2) = log(1/2) ≈ -0.6931 (the -1e6 term is
        // negligible). Naive implementation would return -inf or NaN.
        let got = logmeanexp(&[-1e6, 0.0]);
        assert!((got - (-(2.0f64).ln())).abs() < 1e-9, "got {}", got);
    }

    #[test]
    fn logmeanexp_matches_pf_bias_example() {
        // Two PF reps ℓ_1 = -100, ℓ_2 = -98. logmeanexp > mean because
        // the likelihoods combine on the likelihood scale:
        // log((e^-100 + e^-98)/2) = -98 + log((e^-2 + 1)/2)
        let xs = [-100.0, -98.0];
        let lme = logmeanexp(&xs);
        let arith = (xs[0] + xs[1]) / 2.0;
        assert!(lme > arith, "logmeanexp {} should exceed arithmetic mean {}", lme, arith);
        // Analytic value: -98 + ln((1 + e^-2) / 2) ≈ -98.566
        let expected = -98.0 + ((1.0 + (-2f64).exp()) / 2.0).ln();
        assert!((lme - expected).abs() < 1e-9, "got {}, expected {}", lme, expected);
    }

    #[test]
    fn logmeanexp_edge_cases() {
        assert_eq!(logmeanexp(&[]), f64::NEG_INFINITY);
        assert_eq!(logmeanexp(&[f64::NEG_INFINITY]), f64::NEG_INFINITY);
        assert!(logmeanexp(&[f64::NAN, 0.0]).is_nan());
    }

    #[test]
    fn sample_sd_basic() {
        assert_eq!(sample_sd(&[]), 0.0);
        assert_eq!(sample_sd(&[5.0]), 0.0);
        // sd of [1, 2, 3, 4, 5] is sqrt(2.5) ≈ 1.5811 with N-1 denom.
        let got = sample_sd(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!((got - 2.5f64.sqrt()).abs() < 1e-9, "got {}", got);
    }

    #[test]
    fn boundary_evidence_from_proposal() {
        // Spot-check the Jeffreys table against the proposal's worked values.
        // 20 nats × 4.343 ≈ 86.9 dB, "overwhelming" (above 40 dB).
        let (_, tag) = evidence_cells(20.0);
        assert!(tag.contains("overwhelming"));
        // Small gap (0.5 nats ≈ 2.2 dB) should be indeterminate — noise floor.
        let (_, tag) = evidence_cells(0.5);
        assert!(tag.contains("indeterminate"));
        // 3-nat gap (~13 dB) should be "strong" — a few nats of difference
        // on a weekly-obs fit is already beyond anecdotal.
        let (_, tag) = evidence_cells(3.0);
        assert!(tag.contains("strong"));
    }
}
