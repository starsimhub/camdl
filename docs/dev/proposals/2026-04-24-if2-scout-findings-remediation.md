# camdl IF2 Scout: Findings and Remediation Proposal

**Status:** Proposed (pending upstream review); findings established via
clean-PF evaluation on the he2010 synthetic recovery vignette
**Author:** upstream analysis (captured 2026-04-24)
**Date:** 2026-04-24
**Context:** he2010 synthetic recovery vignette (21-year, seed 42)
**Related:**
- Supersedes parts of `docs/dev/proposals/2026-04-19-refine-gates-scout-convergence.md`
  (the single-diagnostic Rhat gate it proposed is extended into the
  compound gate in Proposal 3 below). Other elements of that proposal
  — the Rhat/best_loglik regression check, the
  `--allow-nonconverged-scout` override — remain complementary.
- Builds on `docs/methods/cooling.md` (cooling schedule semantics as of
  2026-04-24; this proposal's Finding 4 assumes cf50 semantics)
- Uses log-likelihood differences expressed in decibans per
  `docs/dev/proposals/2026-04-23-evidence-in-decibans.md`
- `docs/dev/proposals/INFLIGHT.md` tracks this proposal's staging
  relative to other in-flight work

**Scope:** This document targets camdl platform-wide — `fit/runner.rs`,
the `fit.toml` schema, the CLI surface, and the IF2/refine/validate
stage semantics. The he2010 vignette is the empirical test case that
surfaced the issues, but the fixes are not vignette-specific; they
change default behavior and the public interface of camdl's MLE
pipeline. The vignette will be updated downstream to reflect the new
pipeline behavior.

> **The core pathology in one sentence:** parameter $\widehat{R}$ was
> $\approx 1.000$ across all 36 chains of the 6-parameter scout, while
> those chains had converged in consensus on a point $10^{155}$ times
> less likely than truth under the fitted model. The current pipeline
> can silently report mathematically-agreed convergence on a
> catastrophically wrong answer. Every proposal in this document
> follows from that observation.

## Background

The he2010 synthetic recovery vignette generates 21 years of weekly
measles notifications at the He et al. 2010 MLE (seed = 42,
$N_0$ = 2,462,500), estimates six parameters via IF2 scout, and asks
whether truth is recovered. Initial analysis reported biased parameter
estimates:

- **6-parameter scout** (estimating $s_0$ with bounds $[0.001, 0.5]$)
  pinned $s_0$ at its upper bound 0.5 (truth: 0.0297), with
  $R_0 = 73.4$ (truth 56.8), $\gamma = 0.0994$ (truth 0.083).
- **Fixed-$s_0$ scout** ($s_0$ pinned at 0.025 = profile-likelihood
  peak) gave $R_0 = 73.69$ (truth 56.8), $\gamma = 0.148$ (truth
  0.083), $\alpha = 0.926$ (truth 0.976).

All parameter $\widehat{R}$ values were $\approx 1.000$ across 36
chains. The working narrative interpreted the biased estimates as
evidence of identifiability ridges: an $(s_0, \alpha)$ ridge derived
analytically from the FOI $\propto \beta s_0 I^\alpha$, and a
hypothesized $(R_0, \gamma)$ ridge to be tested via 2D profile
likelihood.

This document records the results of a systematic investigation that
revealed the identifiability-ridge framing was partially correct but
masked a more consequential pipeline issue.

## Findings

### Finding 1: Catastrophic basin failure in the 6-parameter scout

Clean PF log-likelihood evaluations at high particle count (16,000
particles × 8 replicates, combined via `logmeanexp`):

| Parameter point | $\hat\ell_\text{PF}$ (16k, 8 reps) | SE | $\Delta\ell$ from truth | Decibans |
|-----------------|--------------------------------------|------|--------------------------|----------|
| $\theta_\text{truth}$ | $-5982.7$ | 0.6 | — | — |
| $\theta_\text{MLE}$ (6-param scout, $s_0$ at 0.5 bound) | $-6340.7$ | 2.4 | $-358$ nats | $-1554$ dB |
| $\theta_\text{MLE}$ (fixed-$s_0$ scout, $s_0 = 0.025$) | $-6047.6$ | 1.8 | $-65$ nats | $-282$ dB |

The 6-parameter scout's reported MLE is not merely biased — it sits
358 nats below truth. In weight-of-evidence terms, truth is $10^{155}$
times more likely than the scout's reported MLE under the fitted
model. This is not ridge geometry; it is a hard basin failure at the
$s_0 = 0.5$ boundary.

The parameter $\widehat{R}$ diagnostic did not flag the failure. All
36 chains fell into the same bad basin, producing
$\widehat{R} \approx 1.000$ with perfect cross-chain agreement on a
point 358 nats below truth. This is a generic failure mode:
$\widehat{R}$ tests whether chains agree on where they are; it is
silent on whether where-they-are is a good place.

### Finding 2: The 65-nat fixed-$s_0$ gap decomposes into three distinct contributions

$$\Delta\ell = \underbrace{\ell(\theta_\text{truth}) - \ell(\theta^{*}_{s_0 = 0.025})}_{\text{constraint cost}} \;+\; \underbrace{\ell(\theta^{*}_{s_0 = 0.025}) - \ell(\theta_\text{scout}^{\text{best-iter}})}_{\text{IF2 convergence gap}} \;+\; \underbrace{\ell(\theta_\text{scout}^{\text{best-iter}}) - \ell(\theta_\text{scout}^{\text{reported}})}_{\text{extraction bias}}$$

where $\theta^{*}_{s_0=0.025}$ is the (unknown) true conditional MLE at
the constrained $s_0$. Experimental decomposition:

- **Constraint cost: 7.0 nats.** Measured by the "truth-swap" —
  substitute $s_0 \to 0.025$ into the truth vector, leave the other
  five parameters at truth, evaluate cleanly. This is a *lower bound*
  on the true constraint cost because $\theta^{*}_{s_0=0.025}$ may
  differ from truth in parameters other than $s_0$, so the true
  conditional MLE at $s_0 = 0.025$ may have higher likelihood than
  truth-swap.
- **Extraction bias: 40.2 nats.** Measured by re-evaluating the best
  iter of the best chain at 16k particles × 8 reps.
- **Residual IF2 gap: 17.7 nats.** Computed as
  $65 - 7 - 40.2 \approx 17.8$ nats. This is an *upper bound* on the
  true IF2 gap, since the constraint cost is bounded below.

Best-iter extraction identifies chain 20, iter 170:
$\hat\ell = -6007.4 \pm 0.5$ at 16k × 8 reps. The in-run 500-particle
eval at the same iter reported $-6007.1$ — a coincidence of a
favorable PF noise draw at that iteration, as discussed in
Finding 3. Parameter values at chain 20, iter 170:

| Parameter | Truth | Chain 20, iter 170 | % error |
|-----------|-------|---------------------|---------|
| $R_0$ | 56.8 | 78.8 | 38.7% |
| $\sigma$ | 0.0791 | 0.077 | 2.7% |
| $\gamma$ | 0.0832 | 0.121 | 45.4% |
| $\alpha$ | 0.976 | $\approx$ truth | $\approx 0$ |
| amplitude | 0.554 | $\approx$ truth | $\approx 0$ |

$\sigma$ is recovered to 3% of truth. The remaining bias concentrates
in $R_0$ and $\gamma$, consistent with a $(R_0, \gamma)$ coupling
inherited from $\beta = R_0(\gamma + \mu)$ — though the 17.7-nat gap
is too small to adjudicate whether this is a flat ridge, a curved
ridge whose true conditional MLE is near chain-20-iter-170, or further
IF2 underconvergence.

### Finding 3: Root cause of the 40.2-nat extraction bias

The camdl pipeline at `fit/runner.rs:755-766` performs the following
selection:

```
every 10 iterations:
    ll_iter[i] = clean_pf(iter.param_means, n_eval_particles)
    where n_eval_particles = min(n_particles, 500)

final_loglik = ll_iter[last iteration]
best_chain = argmax_i final_loglik[i]
final_params.toml = param_means[best_chain, last iteration]
```

Three pathologies interact:

**(a) 500-particle PF evaluation noise on a 1096-observation dataset.**
Empirical PF single-evaluation SD at 500 particles is approximately 30
nats on this problem, estimated from the observed 16k × 8-rep SE of
0.5 via

$$\sigma_\text{PF, N particles} \approx \sigma_\text{PF, 16k}\sqrt{16{,}000 / N} \cdot \sqrt{8} \;\approx\; 30 \text{ nats at } N = 500.$$

**(b) Argmax selection bias.** For $N$ independent evaluations each
with SD $\sigma$, the expected maximum lies above the true value by
approximately $\sigma \sqrt{2 \ln N}$. With 36 chains and
$\sigma \approx 30$ nats:

$$\mathbb{E}[\max_i \hat\ell_i] - \ell_\text{true} \;\lesssim\; 30 \sqrt{2 \ln 36} \;\approx\; 80 \text{ nats}.$$

The observed 40-nat gap is below this ceiling, consistent with chains
not being fully independent by iter 200 (they have converged toward
overlapping basins, reducing effective $N$).

**(c) Last-iter versus trajectory-best selection.** Even at zero PF
noise, IF2 traces oscillate in the tail. Under camdl's cf50 cooling
semantics (see `docs/methods/cooling.md`), scout with
`cooling_fraction = 0.9` over 200 iterations ends at
$\sigma_{200}/\sigma_0 = 0.9^2 = 0.81$ — perturbations are still
alive, but the PF-noise contribution to trace variance dominates any
parameter-motion contribution at SD≈0.81×initial because the
underlying likelihood surface in a converged basin is locally smooth.
Reporting the last iter specifically — rather than a tail summary —
commits the final estimate to whichever point the trajectory happened
to land on, with no averaging over the ergodic tail.

Concrete illustration: for the reported chain (chain 27, iter 200),
the 500-particle in-run evaluation reported $\hat\ell = -6007$; the
same parameter vector re-evaluated at 16k particles gave
$\hat\ell = -6047$. The 40-nat gap is entirely selection on PF noise —
the chain's parameters did not suddenly become worse between 500 and
16k particles, only the evaluation got more accurate.

### Finding 4: In-run convergence traces are dominated by PF noise

Scout diagnostic traces show 30–100 nat iter-to-iter swings across all
36 chains in the last half of iterations. At 500-particle eval SE of
~30 nats, iter-to-iter ll differences on the order of 40–80 nats are
fully explained by PF evaluation noise alone, with no parameter motion
required. Parameter perturbations *are* still alive at iter 200 under
scout's actual cooling schedule (SD≈0.81×initial at cf50
semantics — see `docs/methods/cooling.md`) — but they can contribute at
most a few nats to iter-to-iter ll variation when restricted to that
regime, because the underlying likelihood surface in a converged basin
is locally smooth in parameter space. The trace variance is
PF-noise-dominated, not because perturbations are dead, but because PF
noise is large relative to the ll-change any plausible remaining
parameter motion could produce.

Observed per-chain iter-to-iter swings of ~40–80 nats match ±1 to 2
PF-noise-SD fluctuations at 500 particles (30 nats SD), confirming the
PF-noise-dominated diagnosis.

This has two consequences. First, user-visible convergence trace plots
are mostly reporting noise, not convergence state, which obscures
whether IF2 has finished mixing. Second, any argmax over iter-level
traces — including the naive proposed fix "argmax over (chain, iter)
pairs" — inherits this noise. With 36 chains × 20 iteration
checkpoints = 720 candidates:

$$\mathbb{E}[\max_{i,t} \hat\ell_{i,t}] - \ell_\text{true} \;\lesssim\; 30\sqrt{2 \ln 720} \;\approx\; 110 \text{ nats}.$$

Naive best-iter argmax makes selection bias *worse*, not better. This
is the key reason the fix must target selection *evidence* (cleaner
evaluations), not selection *candidates* (more iters to choose from).

### Finding 5: Parameter-$\widehat{R}$ is blind to basin quality

The 6-parameter scout had $\widehat{R} \approx 1.000$ on all 36
chains. Per the vignette's own decibans analysis, chain
log-likelihoods spanned ~18,000 nats (~78,000 dB). Both statements
were true simultaneously: chains agreed on where they were (within
each basin), and basins differed by factors up to $10^{7800}$ in
posterior mass under the fitted model.

Parameter $\widehat{R}$ tests intra-chain vs. between-chain variance
in parameter space. It is blind to the likelihood values attained in
each basin. A gate built only on parameter $\widehat{R}$ cannot catch
"all chains agree on one bad basin" nor "chains are in different
basins with different qualities." Something that summarizes the
likelihood values directly — the cross-chain decibans spread — is
required.

## Proposal

A single structural change inside scout, plus diagnostic-hygiene
changes, a new resume capability, and a rename of a misleading
diagnostic, address all five findings without altering the
scout → refine → validate architecture.

### Philosophy: MLE workflow diagnostics are not MCMC diagnostics

Before the concrete proposals, one UX principle worth uplifting as
camdl policy: the diagnostics and labels used in MLE workflows (IF2
scout, IF2 refine) must not borrow names or conventions from MCMC
workflows where the mathematical object being measured is different.
Right now the scout reports `Rhat` across chains, borrowing the
Gelman–Rubin potential scale reduction factor. That statistic is
defined for *posterior samples* under a stationary distribution:
between-chain vs. within-chain variance answers "has the chain mixed
to its target distribution?" IF2 chains are not samples from a
target — they are independent stochastic optimizers, each annealing a
perturbation kernel toward zero.

The naming mismatch is not merely cosmetic; the statistic is
**structurally biased toward 1 by construction** in the IF2 setting.
Under IF2, within-chain parameter variance shrinks as
$a^{2m} \sigma_0^2$ from the cooling kernel regardless of whether the
chain is converging to a good basin, a bad basin, or a saddle point.
Between-chain variance shrinks at a problem-dependent rate as chains
descend into basins. By late iterations, both numerator and
denominator of the Gelman–Rubin statistic are small, and the ratio
converges toward 1 as a property of the cooling schedule, not as
evidence that anything has been correctly identified. The he2010
6-parameter scout's "$\widehat{R} \approx 1.000$ across all 36 chains
in one catastrophically bad basin" is not a pathology — it is the
expected behavior of the statistic when run on cooled optimizer
chains. The name has to go not because users will misread a
well-behaved diagnostic, but because the statistic genuinely measures
something different here from what $\widehat{R}$ measures in MCMC.

Concrete consequences:

- Users trained on Stan/PyMC read `Rhat < 1.01` as "the posterior has
  been correctly sampled." In the IF2 context the same number means
  at most "independent cooled optimizers agreed on an endpoint,"
  which is necessary but not sufficient for basin quality (Finding 5:
  all 36 chains agreed on a point 358 nats below truth with
  `Rhat ≈ 1.000`).
- Shipping the name `Rhat` in MLE output invites users to apply MCMC
  intuitions (effective sample size, posterior width, HDI coverage)
  that don't apply to optimizer output at all.

This is one instance of a broader principle: camdl's MLE and Bayesian
pipelines should share architecture where the math allows (stages,
gates, caching, clean-eval patterns) and diverge in naming wherever
the interpretation diverges. When a diagnostic computation happens to
match across both pipelines, it still deserves separate names where
the interpretation is different.

### Proposal 1: Clean-evaluation selection sub-step inside scout *and* refine

Replace the current final-iter + 500-particle + argmax selection with
a post-IF2 clean evaluation pass. This sub-step runs at the end of
**both** scout and refine stages: scout uses it to pick the winner
handed to refine; refine uses it to report the final MLE with a
defensible SE. (Validate remains a distinct third pass for
higher-particle-count final evaluation and optional profile-likelihood
scans.)

**Multi-candidate per chain.** IF2 tails oscillate (Finding 4), and
`final_iter.param_means` is an arbitrary stopping point. Clean-evaluate
three candidates per chain rather than just the final-iter point:

- `final_iter` — the IF2 estimator's theoretical endpoint (what the
  Ionides et al. convergence results apply to).
- `tail_mean_last_K` — mean of `param_means` over the last $K$
  iterations (default $K = 50$). Averages out cooled-tail PF noise
  without discarding IF2's late-stage refinement.
- `best_in_run_iter` — the iteration whose in-run eval was highest.
  Caveat: this candidate is *selected* on the noisy 500-particle
  in-run eval, so its inclusion re-introduces a selection step — but
  the final winner is still chosen on the clean re-score of all
  candidates, so selection bias is bounded by clean-eval SE, not
  in-run SE.

Score each candidate via the clean-eval procedure below, take the
chain's reported score as the max of its three candidates' clean
scores, and select the winner across chains on that. The full
selection-bias analysis is at the end of this proposal; the short
version is that going from 36 to 108 candidates only changes the
ceiling from ~1.9 to ~2.1 nats.

**Per-candidate clean-eval procedure:**

```
scout stage:
  (existing) IF2 runs N chains to final iteration
  (new) clean-eval pass:
    for chain in chains:
      candidates = {
        final_iter:        chain.iterations[-1].param_means,
        tail_mean_last_K:  mean(chain.iterations[-K:].param_means),
        best_in_run_iter:  chain.iterations[argmax(in_run_ll)].param_means,
      }
      for (label, θ_c) in candidates:
        {ℓ_k}_{k=1..M} = pfilter(θ_c, n_particles_clean, seed=seed_k)
        ℓ_c = logmeanexp({ℓ_k})
        SE_c = sd({ℓ_k}) / sqrt(M)
      (ℓ_chain, winning_label) = argmax_c (ℓ_c, label)
      θ_chain = candidates[winning_label]
  (new) winner = argmax_chain ℓ_chain
  (new) write final_params.toml with θ_winner, ℓ_winner, SE_winner, winning_candidate_label
  (new) write chain_evaluations.tsv with (chain, candidate, ℓ, SE, param_means) for all (chain, candidate) pairs
```

Recommended parameters for the clean-eval pass:

- **$n_\text{particles\_clean}$**: 4,000 to 16,000 (problem-dependent).
  he2010 at 1096 observations and $N_0 \approx 2.5\text{M}$ needs at
  least 4,000; 16,000 gives single-rep SE ≈ 2 nats.
- **$M$ replicates**: 5–10. With $M = 8$ the reported SE becomes
  $\text{SE}_\text{single-rep}/\sqrt{M}$, approximately 0.7 nats at
  16k particles.
- **`logmeanexp` for combining replicates.** The PF log-likelihood
  estimator $\hat\ell$ is *downward*-biased on the log scale by
  $\text{Var}(\hat\ell)/2$ (Jensen's inequality), even though the PF
  likelihood $\exp(\hat\ell)$ is unbiased. `logmeanexp` — which
  combines on the likelihood scale via
  $\ell_\text{combined} = \log\left(\frac{1}{M}\sum_k e^{\hat\ell_k}\right)$ —
  recovers the unbiased combined estimate.

**Selection bias ceiling** on clean scores for 36 chains × 3
candidates:

$$\mathbb{E}[\max_{i,c} \hat\ell_{i,c}^\text{clean}] - \ell_\text{true} \;\lesssim\; 0.7 \sqrt{2 \ln 108} \;\approx\; 2.1 \text{ nats}.$$

This is a ~40× reduction in selection bias from the 80-nat ceiling of
the current pipeline. The reduction comes from shrinking $\sigma$ (the
inner factor), not from shrinking $N$ (the candidate count). Scaling
to many more chains is therefore safe:

$$\mathbb{E}[\max] \;\lesssim\; 0.7 \sqrt{2 \ln (3 \cdot 1000)} \;\approx\; 2.8 \text{ nats at } N = 1000 \text{ chains}.$$

**Cost.** For 36 chains × 3 candidates × 8 replicates × 16k particles
on the he2010 problem, this is ~864 PF evaluations. At 16k particles
this is roughly 30% of the IF2 wall-time at current scout defaults
(36 chains × 200 iters of 2000-particle IF2 internally), dropping to
roughly 7% at $n_\text{particles\_clean} = 4000$. Not free, but
bounded, predictable, and a strict upgrade on selection quality. Users
running very large problems should scale `n_particles_clean` down
first before reducing the candidate set.

This is the substantive fix. It addresses:

- **Finding 1** — the 6-param scout would not have reported $s_0 = 0.5$
  as the winner if other chains' final-iter params, under clean eval,
  scored better. If no such chains exist and all 36 really fell into
  the $s_0 = 0.5$ basin, Finding 5's decibans-spread gate (Proposal 3)
  catches it at the scout → refine gate.
- **Finding 3** — extraction bias is eliminated at the root by using
  clean scores for selection.
- **Finding 5 (partial)** — winner selection no longer rests on
  parameter-$\widehat{R}$ alone.

### Proposal 2: Denoised in-run diagnostics, **additive to raw traces**

Independent of selection, in-run trace log-likelihood plots are
dominated by PF noise and do not communicate convergence state to
users. Two hygiene changes, both strictly additive — the raw
per-iteration PF evaluation trace is always retained and remains the
primary ground-truth diagnostic:

**(a) Raise `n_eval_particles` in the in-run trace from 500 to
2,000–4,000.** Changes single-eval SD from ~30 nats to ~10–15 nats,
making the raw trace readable as a convergence signal rather than a
noise floor. Cost: one longer PF call every 10 iterations (the trace
cadence). This is a default change, not a new optional feature.

**(b) Add a rolling-mean log-likelihood overlay to trace diagnostic
plots, alongside the raw trace.** Rolling mean over the last $k$ eval
checkpoints ($k = 5$ giving a 50-iteration window) reduces display
noise by $\sqrt{k}$ without modifying what IF2 is doing. This is
**additive**: the raw eval track is always displayed, and the
rolling-mean overlay is drawn on top so users can compare the two.
Under no circumstance does the rolling mean replace the raw trace —
users need the raw trace to diagnose PF instability, impoverishment,
and single-eval outliers that the rolling mean would smooth away.

Neither of these tracks feed selection. Selection uses the clean-eval
pass from Proposal 1 exclusively. The in-run tracks are
diagnostic-only.

### Proposal 3: Compound gate criterion for scout → refine, with renamed diagnostic

**Rename first.** Per the philosophy section above, the current
cross-chain parameter dispersion diagnostic should not be labeled
`Rhat` in MLE output. Proposed rename: **$\widehat{A}$** ("agreement
statistic") or `chain_agreement` in output columns and TOML.
Alternatives considered and rejected: $\widehat{X}$ (placeholder,
users will still read it as Rhat-adjacent); $\widehat{G}$ for Gelman
(gives credit but also invites the MCMC interpretation we're trying
to avoid). $\widehat{A}$ is computed identically to Gelman–Rubin
$\widehat{R}$ on parameter traces; only the name and interpretation
text change. A transition period can alias the TOML key `rhat` to
`chain_agreement` with a deprecation warning for one or two releases.

**Extend the gate.** Current gate: $\widehat{R} < R_\text{thresh}$
(soon-to-be: $\widehat{A} < A_\text{thresh}$). Extend to:

$$\text{gate:} \quad \widehat{A}_\text{param} < A_\text{thresh} \;\;\wedge\;\; \Delta_\text{dB}(\text{clean chain } \hat\ell\text{'s}) < \Delta_\text{thresh}$$

where the decibans spread is

$$\Delta_\text{dB} = \frac{10}{\ln 10} \cdot \left(\hat\ell_\text{best} - \hat\ell_\text{worst}\right)$$

computed over the clean-eval log-likelihoods produced by Proposal 1.

**Adaptive threshold.** A fixed 30 dB threshold is right for problems
where clean-eval SE is ~1 nat per chain (the he2010 regime at
16k × 8 reps). For harder problems — longer series, larger $N_0$,
tougher filter — clean-eval SE can reach 2–3 nats per chain, and then
cross-chain Monte Carlo variance alone can hit 30 dB without meaning
multimodality. For easier problems (shorter data, tighter priors),
clean-eval SE can be much smaller, and 30 dB may be too loose to
catch genuine basin disagreement at smaller scale.

Set the threshold to scale with the problem-specific clean-eval noise
floor:

$$\Delta_\text{thresh} = \max\left(30 \text{ dB}, \;\; k \cdot \text{SE}_\text{clean,max} \cdot \frac{10}{\ln 10}\right)$$

with $k \approx 6$–$10$ (recommended default: 8), and
$\text{SE}_\text{clean,max}$ computed as the maximum SE across chains
from the current clean-eval pass (so the threshold adapts to problem
difficulty naturally — harder problems produce noisier clean evals
and automatically widen the gate). The floor at 30 dB covers
well-behaved problems where SE is sub-nat and raw sampling variance
never reaches 30 dB in practice. The SE-proportional term covers
harder problems where it could.

On he2010 fixed-$s_0$ at SE $\approx 1.8$ nats, $k = 8$:
$8 \cdot 1.8 \cdot 10/\ln 10 \approx 63$ dB. The floor at 30 dB is not
active here; the effective threshold is 63 dB. On an easier problem
with SE $\approx 0.3$ nats: $8 \cdot 0.3 \cdot 10/\ln 10 \approx 10$
dB, below the 30 dB floor, so the floor applies. Both regimes work
without manual tuning.

Both conditions are necessary. $\widehat{A}$ catches "chains are
wandering in the same basin but haven't converged." Decibans-spread
catches "chains have all converged individually but to
different-quality basins." Neither diagnostic can substitute for the
other.

Applied to the he2010 6-parameter scout: $\widehat{A}$ passes
($\approx 1.000$); decibans gate fails catastrophically (~78,000 dB
$\gg$ 30 dB). The pipeline would refuse to hand refine a starting
point, and the user would be informed that the scout did not produce
a single basin — much more actionable than the current silent handoff
into a tight local sampler.

### Proposal 4: `--resume` and `--extend-iterations` for MLE stages

camdl's Bayesian stages (PGAS, PMMH) support resuming posterior
sampling if a user decides more iterations are needed after inspecting
preliminary results. MLE stages currently do not, which creates an
asymmetry: a user who runs a 200-iteration scout, inspects the trace,
and sees log-likelihood still trending toward a target must rerun from
scratch with 400 iterations rather than extending the existing run.

Add resume/extend capability to IF2 (scout and refine). Two modes are
necessary, because IF2's cooling schedule makes the naive "resume"
behavior pedagogically subtle:

**Mode A: `--continue`.** Pick up at iteration $M+1$ with the cooling
schedule continuing uninterrupted. Under cf50 semantics (see
`docs/methods/cooling.md`), if the original run used
`cooling_fraction = 0.9` and `target_iters = 200`, extending by
another 200 iterations at the same per-step factor (with
`target_iters` unchanged at 200) drives the schedule further: SD at
iter 400 = `0.9^(2·400/200) = 0.9^4 = 0.656 × initial`. If instead
`target_iters` is updated to 400 on extension, the per-step factor
shifts and the schedule lengthens rather than continues. Document
which behavior `--continue` implements (recommend: keep `target_iters`
fixed so the schedule is a strict continuation, not a rewrite) and
surface the projected SD at the end of the extended run in startup
output so users can see what they're getting.

**Mode B: `--warm-restart`.** Reset perturbation SD to some fraction
of $\sigma_0$ (default: $0.1 \sigma_0$, configurable) and resume
cooling from that point over the new iteration budget. This is the
mode that matches the pedagogical use case: user sees the ll trace has
not plateaued, wants to continue exploring parameter space rather
than continuing to anneal an already-cooled kernel. Equivalent to
starting a fresh IF2 run from the current param_means with a reduced
initial perturbation — but with all chain histories, trace data, and
RNG state preserved for analysis continuity.

**Warm-restart covariance structure** (subsidiary choice). "Reset
$\sigma$" is underspecified: a diagonal from parameter bounds throws
away information IF2 earned about parameter correlations during the
original run; the empirical chain covariance at the resume point
preserves learned ridge structure but risks locking in the wrong
structure if the chain had descended into a bad basin. Default to
**diagonal-from-bounds** (conservative — re-explores the full local
neighborhood around current params) and expose
`--warm-restart-cov={diagonal,empirical}` for users who want to
preserve learned structure. The safe default matches what a user who
thinks "I want to explore more" expects; the `empirical` option is
for users who explicitly know they want to preserve the directional
information from the original run.

**Persistent state required per chain.** To support resume, each IF2
chain's final state must be serialized: `param_means`, perturbation
covariance $\sigma_m$ at iteration $M$ (for continue mode) or
$\sigma_0$ (for warm-restart mode recomputation), particle cloud (or
a resampled representative subset if memory-bound), RNG state,
iteration count, and config hash (to reject resume attempts against a
changed model or dataset). Serialization format should match what the
Bayesian stages already use for their resume implementation, so the
codepath is shared.

**Vignette teaching value.** This feature enables a concrete
pedagogical flow in he2010-synthetic.qmd: run 200-iter scout, show the
log-likelihood still climbing toward truth, invoke
`camdl fit --resume --warm-restart --extend-iterations 200`, show that
the additional iterations close the gap. This makes the "IF2 iteration
budget is a hyperparameter, not a ritual" lesson explicit in a way
that re-running from scratch cannot.

**Gate interaction.** The scout → refine gate is re-evaluated after
the extended run. If the original 200-iter scout failed the gate
(e.g. decibans spread above threshold),
`--warm-restart --extend-iterations` is the correct user-facing
remediation — try more iterations before widening bounds or adding
chains.

### Defaults and public interface changes

Summary of defaults and interface changes introduced by Proposals 1–4.
Primary configuration surface is `fit.toml`; CLI flags override for
ad-hoc runs.

**Default changes (behavior change for existing users; all justified
by findings above):**

| Parameter | Old default | New default | Reason |
|-----------|-------------|-------------|--------|
| `trace.n_eval_particles` (in-run PF eval for trace display) | `min(n_particles, 500)` | `min(n_particles, 2000)` | 500 particles gives ~30 nat per-eval SD on 1k-obs problems, overwhelming trace signal (Finding 4). |
| Scout winner selection | argmax on in-run final-iter ll | argmax on clean-eval ll | 500-particle argmax selection bias $\sim 80$ nats on 36 chains; clean-eval bias $\sim 2$ nats (Finding 3, Proposal 1). |
| Scout → refine gate | $\widehat{R} < R_\text{thresh}$ only | $\widehat{A} < A_\text{thresh}$ AND $\Delta_\text{dB} < 30$ dB | $\widehat{R}$ alone is blind to basin-quality failures (Finding 5, Proposal 3). |
| Diagnostic label `rhat` | `rhat` | `chain_agreement` ($\widehat{A}$) | MCMC-specific interpretation does not apply to IF2 optimizers (Philosophy, Proposal 3). |

**New `fit.toml` keys (all with sensible defaults, user rarely needs
to set):**

```toml
[stages.scout.clean_eval]
n_particles = 4000          # particles for clean-eval winner selection
n_replicates = 8            # logmeanexp over this many seed replicates
combine = "logmeanexp"      # "logmeanexp" | "mean" (logmeanexp is the default, bias-corrected)

[stages.scout.gate]
a_thresh = 1.01             # chain_agreement threshold (was rhat_thresh)
decibans_thresh = 30.0      # cross-chain clean-ll spread threshold, in decibans

[stages.scout.trace]
n_eval_particles = 2000     # new default (was 500)
rolling_window = 5          # eval checkpoints in the rolling-mean overlay (0 to disable overlay)
```

The same `clean_eval` / `gate` / `trace` blocks are valid under
`[stages.refine]` with analogous semantics.

**New CLI flags:**

```
camdl fit <config.toml>
    # existing flags...
    [--resume]                    # resume a previous fit run from its cached state
    [--extend-iterations N]       # add N iterations to the current stage on resume
    [--warm-restart]              # reset perturbation SD to σ_warm × σ₀ before extending (IF2 only)
    [--warm-restart-sigma X]      # override σ_warm (default 0.1)
    [--warm-restart-cov MODE]     # diagonal | empirical (default diagonal)
    [--clean-eval-particles N]    # override stage's clean_eval.n_particles
    [--clean-eval-reps M]         # override stage's clean_eval.n_replicates
    [--decibans-thresh X]         # override gate's decibans_thresh
    [--trace-particles N]         # override trace.n_eval_particles
```

The `--resume` / `--warm-restart` flags apply to both MLE stages
(scout, refine) and Bayesian stages (PGAS, PMMH); the underlying
resume infrastructure already exists for Bayesian stages and should be
extended rather than duplicated for IF2. Config hash validation
(model + data + stage config) should refuse resume attempts where the
config changed in ways that invalidate cached state; trivial changes
(e.g. adjusting `decibans_thresh`) should not invalidate cache.

## Expected Impact on he2010 Results

Applying Proposals 1 and 3 to the existing he2010 scouts, without
rerunning IF2 (only re-processing the stored chain final-iter params
with the clean-eval pass):

**6-parameter scout.** Clean-eval of all 36 chains' final-iter params
surfaces two possibilities. Either (i) some chains were not pinned at
$s_0 = 0.5$ but were out-selected by a noise-inflated eval of an
$s_0 = 0.5$ chain — in which case the reported MLE changes and the
358-nat gap shrinks substantially; or (ii) all 36 chains genuinely
fell into the $s_0 = 0.5$ basin — in which case the decibans gate
flags the scout as failed and the user is forced to widen bounds, add
chains, or investigate identifiability before refine. Either outcome
is strictly better than the current silent-reporting behavior.

**Fixed-$s_0$ scout.** Clean-eval produces chain 20 (or whichever
chain best-iter is near truth) as winner with reported
$\hat\ell \approx -6007 \pm 1$ nat. The 40.2-nat extraction bias is
closed immediately. The remaining 17.7 nats is addressable by the
refine stage (tighter cooling, more particles, more iterations) and
would be correctly labeled as a local-optimum / IF2-tuning question
rather than an architecture question.

## Remaining Open Questions

The proposal does not resolve the following, which require additional
experimental work:

1. **Is the residual 17.7 nats constraint cost or IF2 gap?**
   Truth-swap gives a lower bound of 7.0 for constraint cost but not
   an upper bound. A refine run from the chain-20-iter-170 starting
   point will either close or plateau the remaining gap; the outcome
   determines whether the fixed-$s_0$ approach has an inherent 10+
   nat cost or whether better optimization closes it.

2. **Is the $(R_0, \gamma)$ bias at chain 20 iter 170 a ridge or a
   local optimum?** $R_0 = 78.8$ vs. truth 56.8 and $\gamma = 0.121$
   vs. truth 0.083 are both biased by $\sim 40\%$, but the likelihood
   gap to truth is small. The 2D profile figure on matched-length
   (21-yr) data, with $s_0$ fixed at truth, is needed to characterize
   the local likelihood geometry around chain 20 iter 170 and around
   truth.

3. **Sampling-distribution characterization.** The current analysis
   is on seed 42. Regenerating synthetic data at truth with 20+ seeds
   and re-running the new (post-Proposal-1) pipeline converts
   "seed-42 gave these numbers" into a sampling-distribution statement
   about where the MLE lands on this DGP. This test was proposed
   earlier and deprioritized once the extraction bias became the
   dominant finding; with Proposal 1 in place it returns to the top
   of the list as the chapter's headline claim for recovery.

## Implementation Sequencing

The proposals decouple cleanly into three shipping units with distinct
scopes:

**Unit A (issue): Proposal 1 + Proposal 3.** These are coupled — the
gate (Proposal 3) reads clean-eval output produced by Proposal 1, and
the $\widehat{A}$ rename touches the same output schema. Ship together
as one issue / PR. This is the heart of the fix and should land first,
since it closes the 40-nat extraction bias and upgrades the gate to
catch basin-agreement failures.

**Unit B (separate issue): Proposal 2.** Independent of Units A and
C; touches only in-run trace display. Small, self-contained UX win.
Could land before Unit A as a quick improvement, or bundled with Unit
A for release cohesion. Either order works.

**Unit C (larger issue): Proposal 4.** The largest piece by
implementation scope — serialization schema, config hash, cache
invalidation logic, interaction with the Bayesian resume
infrastructure. Weeks of work rather than days. File as its own issue
with the "share the Bayesian-stage resume codepath" point called out
explicitly so the agent evaluates whether that sharing is actually
achievable or just superficial. Landing Unit C unblocks the vignette's
pedagogical use case but is not required for the correctness fixes in
Units A and B.

Rerunning the he2010 scouts under the new pipeline (action item 7
below) requires only Unit A to have landed.

## Actionable Items

1. Implement **Proposal 1** in `fit/runner.rs`: clean-eval block
   after the IF2 loop, before `final_params.toml` write. Applies to
   both scout and refine stages. Export `chain_evaluations.tsv` for
   transparency.
2. Implement **Proposal 2 (a, b)** in the same commit or adjacent:
   raise default `n_eval_particles` in the in-run trace; add
   rolling-mean overlay to trace diagnostics while retaining raw
   track as primary.
3. Implement **Proposal 3**: rename `rhat` → `chain_agreement`
   ($\widehat{A}$) in output/TOML with deprecation alias; extend the
   scout → refine gate with decibans-spread criterion reading
   clean-eval output from Proposal 1.
4. Implement **Proposal 4**: `--resume`, `--extend-iterations`,
   `--warm-restart` for IF2 stages, sharing the serialization format
   with the existing Bayesian-stage resume infrastructure. Config
   hash validation on resume.
5. Add the new `fit.toml` schema keys under `[stages.scout]` /
   `[stages.refine]` — `clean_eval`, `gate`, `trace` blocks — with
   defaults from the defaults table.
6. Update the `camdl fit` CLI help and shell completions for the new
   flags.
7. Rerun he2010 6-parameter and fixed-$s_0$ scouts under the new
   pipeline. Expected reduction in reported gaps: ~40 nats on
   fixed-$s_0$ (certain); unknown on 6-parameter (depends on what
   chains exist in the stored run — Proposal 1 may surface a
   non-$s_0 = 0.5$ winner, or Proposal 3's decibans gate will flag
   the scout as failed).
8. Update `he2010-synthetic.qmd` to reflect the corrected findings
   and the new pipeline: the scouts had an extraction bias masking
   ~40 nats of recoverable MLE quality; the $(s_0, \alpha)$ ridge
   analysis remains correct; the $(R_0, \gamma)$ second-ridge claim
   needs either demonstration on matched-length data or retraction
   pending further investigation. Add a pedagogical subsection
   demonstrating `--resume --warm-restart` when a scout trace has
   not plateaued.
