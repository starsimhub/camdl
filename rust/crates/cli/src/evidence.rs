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
/// handles framing / column layout.
///
/// Example (5.5 nats ≈ 23.9 dB, lands in the "decisive" tier):
/// ```ignore
/// # use cli::evidence::fmt_evidence;
/// assert_eq!(fmt_evidence("Δlogℒ", 5.5),
///            "Δlogℒ = +5.500 nats (+23.9 dB, decisive)");
/// ```
#[allow(dead_code)]  // used by Unit A compound gate (IF2 scout remediation), fit/diff output; landed here first so the API is stable
pub fn fmt_evidence(label: &str, delta_nats: f64) -> String {
    if !delta_nats.is_finite() {
        return format!("{} = {} nats (non-finite, evidence undefined)",
            label, delta_nats);
    }
    let db = delta_nats * NATS_TO_DB;
    let tag = jeffreys_label(db);
    format!("{} = {:+.3} nats ({:+.1} dB, {})", label, delta_nats, db, tag)
}

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

/// Format a Δlog-likelihood with an associated Monte Carlo SE (nats),
/// displaying the interval in both units. Used when the comparison has a
/// known MC uncertainty (e.g., clean-eval SE from the IF2 scout remediation's
/// compound gate, per proposal 2026-04-24-if2-scout-findings-remediation.md).
///
/// Example:
/// ```ignore
/// # use cli::evidence::fmt_evidence_with_se;
/// assert_eq!(fmt_evidence_with_se("Δlogℒ", 5.5, 0.7),
///            "Δlogℒ = +5.500 ± 0.700 nats (+23.9 ± 3.0 dB, decisive)");
/// ```
#[allow(dead_code)]  // used by Unit A compound gate when MC SE is available
pub fn fmt_evidence_with_se(label: &str, delta_nats: f64, se_nats: f64) -> String {
    if !delta_nats.is_finite() {
        return format!("{} = {} nats (non-finite, evidence undefined)",
            label, delta_nats);
    }
    let db = delta_nats * NATS_TO_DB;
    let db_se = se_nats * NATS_TO_DB;
    let tag = jeffreys_label(db);
    format!("{} = {:+.3} ± {:.3} nats ({:+.1} ± {:.1} dB, {})",
        label, delta_nats, se_nats, db, db_se, tag)
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
    fn fmt_evidence_formats_positive_and_negative() {
        // 5.5 nats × 4.343 = 23.9 dB → "decisive" tier (20–40 dB).
        assert_eq!(fmt_evidence("Δlogℒ", 5.5),
            "Δlogℒ = +5.500 nats (+23.9 dB, decisive)");
        // Negative Δ (alternative model is worse than baseline).
        assert_eq!(fmt_evidence("Δlogℒ", -5.5),
            "Δlogℒ = -5.500 nats (-23.9 dB, decisive)");
        // Zero — indeterminate, within Turing's ~1 dB JND of evidence.
        assert_eq!(fmt_evidence("Δlogℒ", 0.0),
            "Δlogℒ = +0.000 nats (+0.0 dB, indeterminate)");
        // Large Δ (a realistic epi scale — 25 nats ≈ 108 dB, overwhelming).
        let s = fmt_evidence("Δlogℒ", 25.0);
        assert!(s.contains("overwhelming"), "got {}", s);
    }

    #[test]
    fn fmt_evidence_with_se_preserves_both_units() {
        let s = fmt_evidence_with_se("Δlogℒ", 5.5, 0.7);
        assert!(s.contains("5.500"));
        assert!(s.contains("0.700"));
        assert!(s.contains("23.9"));
        // 0.7 nats × 4.343 dB/nat ≈ 3.0 dB
        assert!(s.contains("3.0"));
        assert!(s.contains("decisive"));
    }

    #[test]
    fn evidence_cells_compact_form() {
        let (nats, db_label) = evidence_cells(5.5);
        assert_eq!(nats, "+5.500");
        assert_eq!(db_label, "+23.9 dB, decisive");
    }

    #[test]
    fn non_finite_handled_gracefully() {
        // NaN / ±∞ → non-finite message, not a panic, not a spurious label.
        let s = fmt_evidence("Δlogℒ", f64::NAN);
        assert!(s.contains("non-finite"), "got {}", s);
        let s = fmt_evidence("Δlogℒ", f64::INFINITY);
        assert!(s.contains("non-finite"));
        let (n, db) = evidence_cells(f64::NAN);
        assert!(n.contains("NaN"));
        assert_eq!(db, "—");
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
