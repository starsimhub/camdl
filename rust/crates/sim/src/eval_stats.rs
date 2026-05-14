//! Tallies for silent fallback paths in expression evaluation.
//!
//! RM2 in 2026-04-19 engine review: eval_resolved, eval_expr, and
//! rng all have silent fallback paths (Div-by-zero → 0, Pow-NaN → 0,
//! NegBinomial degenerate → Poisson, etc.). Logging on each hit is
//! either ignored (default log level) or a firehose for inference
//! runs with millions of steps. Atomic counters give a cheap summary
//! the caller can check at sim end: if the count is non-zero, the
//! model hit a degenerate regime the user should know about.
//!
//! Counters are process-global. Callers that care about per-sim
//! isolation should snapshot at start and diff at end.

use std::sync::atomic::{AtomicU64, Ordering};

pub static DIV_BY_ZERO:       AtomicU64 = AtomicU64::new(0);
pub static POW_NAN_INF:       AtomicU64 = AtomicU64::new(0);
pub static UNOP_NAN:          AtomicU64 = AtomicU64::new(0);
pub static NEG_BINOMIAL_POIS: AtomicU64 = AtomicU64::new(0);
pub static BINOMIAL_FALLBACK: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EvalStats {
    pub div_by_zero:       u64,
    pub pow_nan_inf:       u64,
    pub unop_nan:          u64,
    pub neg_binomial_pois: u64,
    pub binomial_fallback: u64,
}

impl EvalStats {
    pub fn snapshot() -> Self {
        EvalStats {
            div_by_zero:       DIV_BY_ZERO.load(Ordering::Relaxed),
            pow_nan_inf:       POW_NAN_INF.load(Ordering::Relaxed),
            unop_nan:          UNOP_NAN.load(Ordering::Relaxed),
            neg_binomial_pois: NEG_BINOMIAL_POIS.load(Ordering::Relaxed),
            binomial_fallback: BINOMIAL_FALLBACK.load(Ordering::Relaxed),
        }
    }

    pub fn diff_since(&self, earlier: &Self) -> Self {
        EvalStats {
            div_by_zero:       self.div_by_zero.saturating_sub(earlier.div_by_zero),
            pow_nan_inf:       self.pow_nan_inf.saturating_sub(earlier.pow_nan_inf),
            unop_nan:          self.unop_nan.saturating_sub(earlier.unop_nan),
            neg_binomial_pois: self.neg_binomial_pois.saturating_sub(earlier.neg_binomial_pois),
            binomial_fallback: self.binomial_fallback.saturating_sub(earlier.binomial_fallback),
        }
    }

    pub fn total(&self) -> u64 {
        self.div_by_zero + self.pow_nan_inf + self.unop_nan
            + self.neg_binomial_pois + self.binomial_fallback
    }
}

#[inline]
pub fn inc_div_by_zero()       { DIV_BY_ZERO.fetch_add(1, Ordering::Relaxed); }
#[inline]
pub fn inc_pow_nan_inf()       { POW_NAN_INF.fetch_add(1, Ordering::Relaxed); }
#[inline]
pub fn inc_unop_nan()          { UNOP_NAN.fetch_add(1, Ordering::Relaxed); }
#[inline]
pub fn inc_neg_binomial_pois() { NEG_BINOMIAL_POIS.fetch_add(1, Ordering::Relaxed); }
#[inline]
pub fn inc_binomial_fallback() { BINOMIAL_FALLBACK.fetch_add(1, Ordering::Relaxed); }

impl std::fmt::Display for EvalStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "eval-stats summary (counts of fallback paths hit during this run):")?;
        if self.div_by_zero       > 0 { writeln!(f, "  div_by_zero:       {}", self.div_by_zero)?; }
        if self.pow_nan_inf       > 0 { writeln!(f, "  pow_nan_inf:       {}", self.pow_nan_inf)?; }
        if self.unop_nan          > 0 { writeln!(f, "  unop_nan:          {}", self.unop_nan)?; }
        if self.neg_binomial_pois > 0 { writeln!(f, "  neg_binomial_pois: {}", self.neg_binomial_pois)?; }
        if self.binomial_fallback > 0 { writeln!(f, "  binomial_fallback: {}", self.binomial_fallback)?; }
        Ok(())
    }
}

/// gh#audit-H5. Convenience helper used by every CLI entry point that
/// runs simulation or inference. Snapshot at the start of `cmd_*`, call
/// `report_if_nonzero(start)` at the end. Prints a compact summary to
/// stderr if any counter incremented during the run; silent otherwise.
/// Does not write JSON — `eval_stats.json` was the audit's recommendation
/// for fit runs with a results dir; left as future work for now.
pub fn report_if_nonzero(start: &EvalStats) {
    let end  = EvalStats::snapshot();
    let diff = end.diff_since(start);
    if diff.total() > 0 {
        eprintln!();
        eprint!("{}", diff);
    }
}
