# Clean-eval strip: 3-candidate construction → FinalIter + clean re-eval

Date: 2026-04-27
Project: camdl
Tags: inference, if2, clean-eval, pomp, citations
Verified-against: HEAD = `08bdc3c` (post-strip)
Closes: code-review concern raised over `clean_eval.rs`'s uncited
3-candidate construction and the SE-formula / combine-mode mismatch.

## TL;DR

Stripped `crates/cli/src/fit/clean_eval.rs` from a 3-candidate
construction (FinalIter / TailMeanLastK / BestInRunIter) re-scored
with M PF replicates and argmaxed-after-rescoring, back to the
pomp-canonical workflow: take each chain's IF2 final-iteration
parameter means as θ̂, re-score with M high-particle clean PF
replicates, combine via `logmeanexp` on the likelihood scale.

Commit: `20d48fe refactor(fit): strip clean-eval to FinalIter + clean re-evaluation`.

## Why we did this

The prior implementation, introduced by
`docs/dev/proposals/2026-04-24-if2-scout-findings-remediation.md`
(§Proposal 1), constructed three candidate parameter vectors per
chain:

1. **FinalIter** — `iterations.last().param_means` (the IF2
   estimator).
2. **TailMeanLastK** — arithmetic mean of `param_means` over the
   last K=50 iterations (smooths cooling jitter).
3. **BestInRunIter** — `param_means` from the iteration with the
   highest finite in-run clean loglik (fallback to perturbed
   loglik, then to FinalIter).

After re-scoring with M=8 PF replicates each, the per-chain winner
was the candidate with the highest `logmeanexp` of its replicate
logliks. Two structural problems with this:

### 1. Uncited

There is no published precedent we could find for picking the
highest-loglik *candidate construction* among (final-iter /
tail-mean / best-in-run-iter) after a clean PF re-evaluation. The
proposal that introduced it offered an empirical motivation
("closes the ~40-nat extraction bias from argmax-selecting over
noisy 500-particle in-run PF evaluations") but no theoretical
justification for the three-candidate shape, no comparison to the
pomp-style single-candidate workflow, and no reference to any
prior work using this construction.

For an inference module on the path between IF2 and the reported
MLE, "no citation, no benchmark" is the wrong shape — especially
in a tool intended to inform public-health decisions.

### 2. Bias-on-top-of-bias-fix

The module's docstring claimed to be fixing the ~40-nat bias from
"argmax-selecting over noisy in-run PF evaluations." The
**`BestInRunIter` candidate** is exactly that: it picks the
iteration with the highest noisy in-run clean loglik. Re-scoring
the winner cleanly with high-particle PF doesn't undo the selection
bias in the candidate pool — you've just argmaxed over noise once,
then evaluated the winner honestly. The honest evaluation gives an
unbiased loglik *for that θ̂*, but the θ̂ itself was selected by a
noisy max, so the reported MLE (the maximum across chains and
candidates) is upward-biased relative to the population MLE.

Symbolically: if `L̂_in_run(θ_i)` is a noisy estimator of `L(θ_i)`
with mean L and noise σ, then `θ̂ = arg max_i L̂_in_run(θ_i)` is
selection-biased toward `θ_i` whose `L̂` realizations were
favorable. `L_clean(θ̂)` is then unbiased *given θ̂*, but the joint
distribution of `(θ̂, L_clean(θ̂))` is biased. The re-evaluation
catches the *vertical* bias (bias in the loglik at fixed θ̂) but
not the *horizontal* bias (bias in which θ̂ was chosen).

### What pomp does

The canonical pomp workflow (King, Ionides, Bretó 2016 JSS;
Ionides et al. 2015 PNAS supplementary materials; pomp manual on
`pfilter` and `logmeanexp`) is:

1. Run `mif2()` to convergence (the IF2 algorithm itself).
2. Take `coef(mif2_out)` as the point estimate — the IF2 final-
   iteration mean.
3. Re-evaluate the log-likelihood at that θ̂ with multiple
   high-particle `pfilter` replicates.
4. Combine via `logmeanexp` of the per-replicate logliks. Report
   that as the MLE loglik with the delta-method SE.
5. Multi-start to characterize chain-to-chain variability — *not*
   within-chain candidate selection.

The within-chain selection is anchored by Ionides et al. 2015's
proof that IF2 converges to the MLE in the final-iter mean. There
is no within-chain argmax over candidate constructions; the
between-chain argmax is the only selection mechanism.

This is the workflow the strip restores. The point estimate is
the IF2 final-iter mean; the loglik is the clean re-evaluation;
the cross-chain selection is argmax over per-chain clean logliks.
**Within-chain there is no candidate selection.**

## What we kept

- **High-particle M-replicate clean re-evaluation.** This is the
  load-bearing piece — it removes the ~40-nat bias from
  in-run-PF-noise *without* introducing a candidate-pool bias.
- **`logmeanexp` combining** of the M per-replicate logliks.
  Standard practice (Pitt-Silva-Giordani-Kohn 2012-style for
  unbiased likelihood-scale combining); matches pomp's
  `logmeanexp` convention.
- **Filter-health stats** (`FilterStats` struct, ESS-at-θ̂
  surfacing) — useful regardless of candidate construction;
  carried over verbatim.
- **Pure / scorer separation** — `run_clean_eval_with_scorer` is
  generic over the scorer closure so unit tests can use a
  deterministic synthetic scorer without paying for real PF calls.
- **Stable seed scheme** for replicate independence.
- **`CombineMode::{LogMeanExp, Mean}` knob** in `CleanEvalConfig`.
  Default remains `LogMeanExp`.

## What we dropped

| dropped | replacement |
|---|---|
| `CandidateLabel` enum | n/a — only one candidate now |
| `CandidateScore` struct (per-candidate score) | `ChainScore` (per-chain score) |
| `ChainWinner` struct | folded into `ChainScore` |
| `CleanEvalOutcome.all_scores` (3N rows) | `per_chain` (N rows) |
| `CleanEvalOutcome.per_chain_winners` | n/a — `per_chain` is per-chain by construction |
| `build_candidates` | inline `if2.iterations.last().unwrap().param_means.clone()` |
| `pick_best_in_run` | n/a |
| `TAIL_K = 50` const | n/a |
| `winning_candidate_label` field in `final_params.toml` `[provenance]` and `chain_evaluations.tsv` `candidate` column | n/a |
| Per-rep SE formula `sd/√M` for `LogMeanExp` (mathematically wrong) | `evidence::logmeanexp_with_se` (delta-method on log scale; matches pomp) |
| Inner-argmax NaN-first-sticks bug | n/a — no inner argmax anymore |

The cross-chain argmax (which chain's clean θ̂ to report as the
overall MLE) explicitly skips NaN chains and errors if every chain
produced NaN, instead of silently picking chain 0.

## Schema changes (alpha; back-compat is a non-goal)

- `final_params.toml` `[provenance]` table: drops
  `winning_candidate_label`. Keeps `loglik`, `se`, `chain`.
  Per-chain final_params header line drops the `candidate = ...`
  fragment.
- `chain_evaluations.tsv`: schema changes from
  `chain | candidate | loglik | se | ess_* | <params>`
  (3N rows)
  to
  `chain | loglik | se | ess_* | <params>`
  (N rows). One row per chain.

The agent's `read_ess_at_mle` in `method_result.rs` (commit
`d45c932`) uses header-name-based column lookup with graceful
degradation on missing columns — it survives the column drop
without changes.

## Open follow-ups

- **Integration test asserting bias reduction.** The original
  strip motivation was "the prior 3-candidate construction was
  bias-on-top-of-bias-fix." We didn't write a test that
  *measures* the bias reduction relative to a known reference.
  The right shape: a synthetic model with a known true MLE and a
  controlled in-run PF noise level, run IF2, then assert the
  clean-eval reported loglik is within tolerance of the true
  loglik. One test, takes a day to write, gives the empirical
  anchor the original module lacked. Worth scheduling alongside
  the experiment-management proposal's step 4 (foundation +
  integration tests) work.
- **Sweep stale references.** Internal docstrings still mention
  "candidate" or "winning candidate" in places. The audit at
  `docs/dev/notes/2026-04-27-fit-experiment-management-audit.md`
  and the proposal at `docs/dev/proposals/2026-04-28-fit-
  experiment-management.md` were written before this strip and
  may have stale phrasing. Quick grep + sweep when convenient.
- **`docs/methods/`.** The earlier `particle-methods.md` listed
  clean-eval as a project-specific construct deserving its own
  methods note. Post-strip, clean-eval is much smaller — a
  paragraph inside a future `if2.md` is the right home, not its
  own file. Update `particle-methods.md`'s §7 cross-cutting
  pointers when next sweeping methods/.

## Citation list

- Ionides, E. L., Nguyen, D., Atchadé, Y., Stoev, S., & King, A. A.
  (2015). Inference for dynamic and latent variable models via
  iterated, perturbed Bayes maps. *PNAS*, 112(3), 719–724.
  Source for IF2 itself; the supplementary materials describe the
  post-mif2 `pfilter` re-evaluation pattern.
- King, A. A., Nguyen, D., & Ionides, E. L. (2016). Statistical
  inference for partially observed Markov processes via the R
  package pomp. *Journal of Statistical Software*, 69(12), 1–43.
  Documents the pomp idiom of `coef(mif2_out)` + multi-replicate
  `pfilter` + `logmeanexp` combining.
- pomp manual entries for `mif2`, `pfilter`, `logmeanexp` (King &
  Ionides, ongoing). The reference convention for the workflow we
  match.

## Commit references

- `20d48fe refactor(fit): strip clean-eval to FinalIter + clean
  re-evaluation` — the strip itself.
- `c8733f0 feat(evidence): add logmeanexp_with_se` (rewritten as
  `65d75e2`) — delta-method SE foundation; supports the strip's
  SE-formula fix.
- `08bdc3c fix(test): repo_root path + brace-aware path truncator
  + stage rename` — incidental fixes to the agent's foundation
  test file surfaced when running the workspace test suite after
  the strip.

The original 3-candidate proposal at
`docs/dev/proposals/2026-04-24-if2-scout-findings-remediation.md`
is preserved as the historical design record. It no longer
describes the implementation; this note documents the deviation.
