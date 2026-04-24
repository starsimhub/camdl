//! Clean-evaluation re-scoring of IF2 candidate parameter points.
//!
//! Closes the ~40-nat extraction bias from argmax-selecting over noisy
//! 500-particle in-run PF evaluations. After IF2 finishes, each chain
//! contributes three candidate parameter points (final iteration mean,
//! tail mean, best-in-run iteration), and each candidate is re-scored
//! with M independent high-particle PF replicates combined via
//! `logmeanexp`. The argmax over the combined scores names the winner.
//!
//! See proposal §Proposal 1
//! (`docs/dev/proposals/2026-04-24-if2-scout-findings-remediation.md`)
//! and the Unit A handoff
//! (`docs/dev/notes/2026-04-24-if2-unit-a-handoff.md`).
//!
//! This module is intentionally split into two layers:
//!
//! - `build_candidates` is pure over `IF2Result` and trivially testable.
//! - `run_clean_eval_with_scorer` is generic over the scoring closure so
//!   tests can inject a deterministic synthetic scorer instead of paying
//!   for real particle-filter calls. `run_clean_eval` is the production
//!   entry point that wires in `runner::run_quick_pfilter`.

use crate::evidence;
use crate::fit::config_v2::{CleanEvalConfig, CombineMode};
use crate::fit::runner::{self, FitRunConfig};
use sim::inference::if2::IF2Result;

/// Which heuristic produced a candidate parameter vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateLabel {
    /// `iterations.last().param_means` — the IF2 final-iteration mean.
    FinalIter,
    /// Mean of `param_means` over the last K iterations (K clamped to
    /// `iterations.len()`). Smooths over per-iteration cooling jitter.
    TailMeanLastK,
    /// `param_means` at the iteration with the largest finite in-run
    /// `loglik`. Falls back to the largest finite `if2_perturbed_loglik`
    /// if no clean PF was run, then to `FinalIter` as a last resort.
    BestInRunIter,
}

impl CandidateLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateLabel::FinalIter      => "final_iter",
            CandidateLabel::TailMeanLastK  => "tail_mean_last_k",
            CandidateLabel::BestInRunIter  => "best_in_run_iter",
        }
    }
}

/// One candidate's clean-PF score: combined log-likelihood plus the
/// per-replicate raw values (kept for diagnostics + downstream gating).
#[derive(Debug, Clone)]
pub struct CandidateScore {
    pub chain_id: usize,
    pub label: CandidateLabel,
    pub theta: Vec<f64>,
    /// Combined score across `per_rep_logliks`. `logmeanexp` by default
    /// (unbiased on the likelihood scale); `mean` when configured.
    pub loglik_combined: f64,
    /// Standard error of the combined score: `sample_sd(per_rep) / √M`.
    pub se: f64,
    pub per_rep_logliks: Vec<f64>,
}

/// Best candidate within a single chain.
#[derive(Debug, Clone)]
pub struct ChainWinner {
    pub chain_id: usize,
    pub label: CandidateLabel,
    pub theta: Vec<f64>,
    pub loglik: f64,
    pub se: f64,
}

/// Full output of clean evaluation: every (chain × candidate) score, a
/// per-chain winner table, and the index into `per_chain_winners` of the
/// overall maximum.
#[derive(Debug, Clone)]
pub struct CleanEvalOutcome {
    /// All 3 × n_chains candidate scores, in deterministic order:
    /// chain-major, label-minor (FinalIter, TailMeanLastK, BestInRunIter).
    pub all_scores: Vec<CandidateScore>,
    pub per_chain_winners: Vec<ChainWinner>,
    pub overall_winner_idx: usize,
}

/// Build the three candidate parameter vectors for one IF2 chain.
///
/// Returns the three `(label, theta)` pairs in the canonical order
/// `[FinalIter, TailMeanLastK, BestInRunIter]`. Errors when
/// `result.iterations` is empty — IF2 always produces at least one
/// iteration, so this is a programmer error rather than a recoverable
/// condition.
pub fn build_candidates(
    result: &IF2Result,
    tail_k: usize,
) -> Result<[(CandidateLabel, Vec<f64>); 3], String> {
    if result.iterations.is_empty() {
        return Err("build_candidates: IF2Result has no iterations".into());
    }

    let n_iter = result.iterations.len();
    let last = result.iterations.last().unwrap();
    let final_theta = last.param_means.clone();

    // Tail mean: arithmetic mean of param_means over the last K iters.
    // Clamp K to n_iter so a fresh chain (n_iter < K) just averages over
    // everything available.
    let k = tail_k.min(n_iter).max(1);
    let n_params = last.param_means.len();
    let mut tail_theta = vec![0.0f64; n_params];
    for it in result.iterations.iter().rev().take(k) {
        for (j, v) in it.param_means.iter().enumerate() {
            tail_theta[j] += *v;
        }
    }
    for v in &mut tail_theta {
        *v /= k as f64;
    }

    // Best-in-run iteration: prefer iters with finite clean `loglik`;
    // fall back to perturbed loglik; final fallback to `FinalIter`.
    let best_theta = pick_best_in_run(&result.iterations).unwrap_or_else(|| final_theta.clone());

    Ok([
        (CandidateLabel::FinalIter,     final_theta),
        (CandidateLabel::TailMeanLastK, tail_theta),
        (CandidateLabel::BestInRunIter, best_theta),
    ])
}

fn pick_best_in_run(iters: &[sim::inference::if2::IF2IterResult]) -> Option<Vec<f64>> {
    // First pass: argmax over finite `loglik` (clean-PF evaluations).
    let by_clean = iters.iter()
        .filter(|it| it.loglik.is_finite())
        .max_by(|a, b| a.loglik.partial_cmp(&b.loglik).unwrap());
    if let Some(it) = by_clean {
        return Some(it.param_means.clone());
    }
    // Fallback: perturbed loglik (always populated by IF2 engine).
    iters.iter()
        .filter(|it| it.if2_perturbed_loglik.is_finite())
        .max_by(|a, b| a.if2_perturbed_loglik.partial_cmp(&b.if2_perturbed_loglik).unwrap())
        .map(|it| it.param_means.clone())
}

/// Production entry point: clean-evaluate every (chain, candidate)
/// using `runner::run_quick_pfilter` as the scorer.
pub fn run_clean_eval(
    run_config: &FitRunConfig,
    results: &[(usize, IF2Result)],
    cfg: &CleanEvalConfig,
    seed: u64,
) -> Result<CleanEvalOutcome, String> {
    run_clean_eval_with_scorer(
        results,
        cfg,
        seed,
        |theta, n_particles, pf_seed| {
            runner::run_quick_pfilter(run_config, theta, n_particles, pf_seed)
        },
    )
}

/// Test-friendly inner core. `scorer(theta, n_particles, seed) -> ll`
/// is called `n_chains × 3 × n_replicates` times.
///
/// Seed scheme matches the handoff:
/// `pf_seed = seed + chain_id*10_000 + cand_ix*1000 + rep_k`.
pub fn run_clean_eval_with_scorer<F>(
    results: &[(usize, IF2Result)],
    cfg: &CleanEvalConfig,
    seed: u64,
    scorer: F,
) -> Result<CleanEvalOutcome, String>
where
    F: Fn(&[f64], usize, u64) -> f64,
{
    if results.is_empty() {
        return Err("run_clean_eval: no chain results to score".into());
    }
    if cfg.n_replicates == 0 {
        return Err("run_clean_eval: n_replicates must be ≥ 1".into());
    }
    if cfg.n_particles == 0 {
        return Err("run_clean_eval: n_particles must be ≥ 1".into());
    }

    // Tail-K = 50 per the handoff. Clamped inside build_candidates.
    const TAIL_K: usize = 50;

    let mut all_scores: Vec<CandidateScore> = Vec::with_capacity(results.len() * 3);
    let mut per_chain_winners: Vec<ChainWinner> = Vec::with_capacity(results.len());

    for (chain_id, if2) in results.iter() {
        let cands = build_candidates(if2, TAIL_K)?;

        let mut chain_best: Option<usize> = None; // index into all_scores
        for (cand_ix, (label, theta)) in cands.iter().enumerate() {
            let mut per_rep = Vec::with_capacity(cfg.n_replicates);
            for k in 0..cfg.n_replicates {
                let pf_seed = seed
                    .wrapping_add((*chain_id as u64).wrapping_mul(10_000))
                    .wrapping_add((cand_ix as u64).wrapping_mul(1000))
                    .wrapping_add(k as u64);
                per_rep.push(scorer(theta, cfg.n_particles, pf_seed));
            }
            let combined = combine(&per_rep, cfg.combine);
            let se = evidence::sample_sd(&per_rep) / (cfg.n_replicates as f64).sqrt();

            let score = CandidateScore {
                chain_id: *chain_id,
                label: *label,
                theta: theta.clone(),
                loglik_combined: combined,
                se,
                per_rep_logliks: per_rep,
            };

            let this_idx = all_scores.len();
            // Winner-of-chain: argmax over combined ll, NaN-safe (NaN
            // never wins). Ties broken by first-seen (FinalIter wins
            // over TailMean wins over BestInRun) — matches canonical
            // candidate order and gives a stable, predictable choice.
            chain_best = Some(match chain_best {
                None => this_idx,
                Some(prev) => {
                    let prev_ll = all_scores[prev].loglik_combined;
                    if score.loglik_combined > prev_ll { this_idx } else { prev }
                }
            });
            all_scores.push(score);
        }

        let winner_idx = chain_best.expect("≥1 candidate per chain by construction");
        let w = &all_scores[winner_idx];
        per_chain_winners.push(ChainWinner {
            chain_id: w.chain_id,
            label: w.label,
            theta: w.theta.clone(),
            loglik: w.loglik_combined,
            se: w.se,
        });
    }

    // Overall winner across chains. Same NaN-safe argmax.
    let overall_winner_idx = per_chain_winners.iter().enumerate()
        .fold(0usize, |best, (i, w)| {
            if w.loglik > per_chain_winners[best].loglik { i } else { best }
        });

    Ok(CleanEvalOutcome { all_scores, per_chain_winners, overall_winner_idx })
}

fn combine(xs: &[f64], mode: CombineMode) -> f64 {
    match mode {
        CombineMode::LogMeanExp => evidence::logmeanexp(xs),
        CombineMode::Mean => {
            if xs.is_empty() { f64::NEG_INFINITY }
            else { xs.iter().sum::<f64>() / xs.len() as f64 }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sim::inference::if2::{IF2IterResult, IF2Result};

    fn iter(n: usize, loglik: f64, perturbed: f64, params: Vec<f64>) -> IF2IterResult {
        IF2IterResult {
            iteration: n,
            loglik,
            if2_perturbed_loglik: perturbed,
            param_means: params,
            param_diag: vec![],
        }
    }

    fn synthetic_result(iters: Vec<IF2IterResult>) -> IF2Result {
        IF2Result {
            mle: iters.last().map(|it| it.param_means.clone()).unwrap_or_default(),
            final_loglik: iters.last().map(|it| it.loglik).unwrap_or(f64::NAN),
            last_loglik: iters.last().map(|it| it.if2_perturbed_loglik).unwrap_or(f64::NAN),
            iterations: iters,
        }
    }

    #[test]
    fn build_candidates_picks_final_tail_and_best_iter() {
        // 5 iterations; iter 2 has the highest finite clean loglik.
        let r = synthetic_result(vec![
            iter(0, f64::NAN, -100.0, vec![0.0, 0.0]),
            iter(1, -90.0,    -85.0, vec![1.0, 2.0]),
            iter(2, -50.0,    -55.0, vec![5.0, 6.0]), // best clean
            iter(3, -80.0,    -70.0, vec![3.0, 4.0]),
            iter(4, -75.0,    -65.0, vec![4.0, 5.0]), // final
        ]);
        let cands = build_candidates(&r, 3).unwrap();

        assert_eq!(cands[0].0, CandidateLabel::FinalIter);
        assert_eq!(cands[0].1, vec![4.0, 5.0]);

        assert_eq!(cands[1].0, CandidateLabel::TailMeanLastK);
        // Mean of last 3 iters' params: (5+3+4)/3, (6+4+5)/3 = 4.0, 5.0.
        assert!((cands[1].1[0] - 4.0).abs() < 1e-12);
        assert!((cands[1].1[1] - 5.0).abs() < 1e-12);

        assert_eq!(cands[2].0, CandidateLabel::BestInRunIter);
        assert_eq!(cands[2].1, vec![5.0, 6.0]);
    }

    #[test]
    fn build_candidates_falls_back_to_perturbed_when_clean_loglik_all_nan() {
        // No clean loglik populated → fall back to perturbed-loglik argmax.
        let r = synthetic_result(vec![
            iter(0, f64::NAN, -100.0, vec![0.0]),
            iter(1, f64::NAN,  -50.0, vec![7.0]), // best perturbed
            iter(2, f64::NAN,  -75.0, vec![3.0]),
        ]);
        let cands = build_candidates(&r, 50).unwrap();
        assert_eq!(cands[2].0, CandidateLabel::BestInRunIter);
        assert_eq!(cands[2].1, vec![7.0]);
    }

    #[test]
    fn build_candidates_clamps_tail_k_to_iter_count() {
        let r = synthetic_result(vec![
            iter(0, -10.0, -10.0, vec![1.0, 2.0]),
        ]);
        let cands = build_candidates(&r, 100).unwrap();
        // Tail mean over a single iter == that iter's params.
        assert_eq!(cands[1].1, vec![1.0, 2.0]);
    }

    #[test]
    fn build_candidates_errors_on_empty_iterations() {
        let r = synthetic_result(vec![]);
        assert!(build_candidates(&r, 50).is_err());
    }

    #[test]
    fn run_clean_eval_argmax_picks_higher_combined_chain() {
        // Two chains, identical IF2Result structure but a deterministic
        // scorer that returns -10.0 for chain-0's thetas and -5.0 for
        // chain-1's thetas. Expect chain-1 to win regardless of in-run
        // numbers (which is the whole point of clean eval).
        let mk_chain = |a: f64| synthetic_result(vec![
            iter(0, f64::NAN, -1000.0, vec![a, a + 1.0]),
            iter(1, f64::NAN, -1000.0, vec![a + 2.0, a + 3.0]),
        ]);
        let results = vec![
            (0usize, mk_chain(0.0)),  // thetas around 0
            (1usize, mk_chain(10.0)), // thetas around 10
        ];

        // Scorer: -10 for small thetas, -5 for large thetas (no noise).
        let scorer = |theta: &[f64], _n: usize, _seed: u64| {
            if theta[0] < 5.0 { -10.0 } else { -5.0 }
        };

        let cfg = CleanEvalConfig {
            n_particles: 1,
            n_replicates: 4,
            combine: CombineMode::LogMeanExp,
        };

        let out = run_clean_eval_with_scorer(&results, &cfg, 42, scorer).unwrap();

        // 2 chains × 3 candidates each = 6 scores, deterministic order.
        assert_eq!(out.all_scores.len(), 6);
        assert_eq!(out.per_chain_winners.len(), 2);

        // Per-chain winners' loglik values reflect the scorer.
        assert!((out.per_chain_winners[0].loglik - (-10.0)).abs() < 1e-12);
        assert!((out.per_chain_winners[1].loglik - (-5.0)).abs() < 1e-12);

        // SE is 0 because all replicates returned the same value.
        for w in &out.per_chain_winners {
            assert!(w.se.abs() < 1e-12);
        }

        // Overall winner is chain 1.
        assert_eq!(out.overall_winner_idx, 1);
        assert_eq!(out.per_chain_winners[out.overall_winner_idx].chain_id, 1);
    }

    #[test]
    fn run_clean_eval_se_reflects_replicate_spread() {
        // One chain, one candidate vector, scorer returns rep-dependent
        // values so we can verify SE = sample_sd / √M.
        let r = synthetic_result(vec![iter(0, -5.0, -5.0, vec![1.0])]);
        let results = vec![(7usize, r)];

        // M = 4 replicates; values 1, 2, 3, 4 → sd ≈ 1.2909944487
        // SE = 1.2909944.../√4 ≈ 0.6454972
        let scorer = |_t: &[f64], _n: usize, seed: u64| {
            // seed = 0 + 7*10_000 + cand_ix*1000 + rep_k.
            // Map rep_k = seed mod 1000 from {0..3} to {1.0..4.0}.
            let rep_k = (seed % 1000) as f64;
            rep_k + 1.0
        };
        let cfg = CleanEvalConfig {
            n_particles: 1, n_replicates: 4, combine: CombineMode::Mean,
        };
        let out = run_clean_eval_with_scorer(&results, &cfg, 0, scorer).unwrap();

        // All 3 candidates have the same theta (only one iter), so all
        // get the same per-rep values → same SE.
        let sum: f64    = 1.0 + 2.0 + 3.0 + 4.0;
        let sum_sq: f64 = 1.0 + 4.0 + 9.0 + 16.0;
        let expected_se = ((sum_sq - sum.powi(2) / 4.0) / 3.0).sqrt() / 2.0;
        for s in &out.all_scores {
            assert!((s.se - expected_se).abs() < 1e-9,
                "expected SE ≈ {}, got {}", expected_se, s.se);
        }
    }

    #[test]
    fn run_clean_eval_combine_mean_vs_logmeanexp_differ() {
        // For non-degenerate replicates, mean(ℓ) < logmeanexp(ℓ).
        let r = synthetic_result(vec![iter(0, -5.0, -5.0, vec![0.0])]);
        let results = vec![(0usize, r)];
        let scorer = |_t: &[f64], _n: usize, seed: u64| {
            let rep_k = (seed % 1000) as f64;
            -10.0 + rep_k
        };
        let cfg_mean = CleanEvalConfig {
            n_particles: 1, n_replicates: 4, combine: CombineMode::Mean,
        };
        let cfg_lme = CleanEvalConfig {
            n_particles: 1, n_replicates: 4, combine: CombineMode::LogMeanExp,
        };
        let mean_out = run_clean_eval_with_scorer(&results, &cfg_mean, 0, scorer).unwrap();
        let lme_out  = run_clean_eval_with_scorer(&results, &cfg_lme,  0, scorer).unwrap();
        // Same theta selected; the scores differ in the expected direction.
        assert!(lme_out.per_chain_winners[0].loglik > mean_out.per_chain_winners[0].loglik);
    }
}
