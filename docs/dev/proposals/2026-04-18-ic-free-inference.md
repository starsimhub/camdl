---
status: proposal
date: 2026-04-18
---

# IC-Free Inference: Conditioning on the First Observation

## Motivation

Stochastic compartmental models have an initial-value problem that
no amount of inference technology eliminates: the likelihood surface
in `(β, I₀)` is a ridge. For a short observation window, many
`(β, I₀)` pairs produce trajectories that fit the data equally well —
a large initial seed with modest transmissibility looks the same as
a small seed with aggressive transmissibility, once you've
marginalised out the stochasticity. The standard responses all
commit to a position that is epistemically harder to defend than
practitioners usually admit:

- **Fix `I₀`:** lets `β`'s posterior look tight, but the tightness is
  paid for with a pretence that `I₀` is known. If the fixed value is
  wrong, `β` is biased in a direction the posterior can't signal.
- **Estimate `I₀`:** the posterior on `(β, I₀)` reveals the ridge but
  typically can't resolve along it without a prior on `I₀`. Whatever
  prior is chosen — uniform, Gamma, Poisson — is load-bearing for
  the posterior on `β` and is usually not calibrated against
  anything.
- **Pretend the ridge isn't there:** the most common choice, whether
  or not anyone says so out loud.

The alternative — used throughout the pomp literature and made
explicit in King, Ionides, Nguyen (2016) and King et al.'s cholera
work (2008) — is to **condition the likelihood on the first
observation** and let that observation implicitly pin down the
initial state. You don't commit to a prior on `I₀`; you accept a
wider `β` posterior that honestly reflects the ambiguity. This is a
principled alternative, not a heuristic.

Operationally, in particle-filter terms, this is a change of
likelihood factorisation plus a change of initial-state distribution
for the particles. Three pieces: (i) initialise particles with
spread across the plausible initial state space, (ii) let the first
observation weight and resample the particle cloud (this is the
"pinning"), (iii) start accumulating log-likelihood from the second
observation onward. That's it.

The feature is minimal to implement — existing PF + IVP-perturbation
machinery covers 90 % of it — and is the methodological
demonstration piece the epistemic-laundering whitepaper needs.

## Mathematical formulation

Let `x_{0:T}` be the latent state trajectory, `y_{1:T}` the
observations, `θ` the parameters. A partially-observed Markov
process (POMP) is specified by:

- Initial distribution: `x₀ ~ μ_θ(·)`
- Transition density: `x_t | x_{t-1} ~ f_θ(·|x_{t-1})`
- Measurement density: `y_t | x_t ~ g_θ(·|x_t)`

### Standard likelihood

The full marginal likelihood integrates out the latent trajectory:

```
L(θ) = ∫ μ_θ(x₀) ∏_{t=1}^{T} f_θ(x_t|x_{t-1}) g_θ(y_t|x_t) dx_{0:T}
```

Equivalently, by the sequential factorisation of one-step predictive
densities:

```
L(θ) = ∏_{t=1}^{T} p_θ(y_t | y_{1:t-1})
```

where by convention `p_θ(y_1 | y_{1:0}) ≡ p_θ(y_1)`. The particle
filter provides an unbiased Monte Carlo estimator of each predictive
density:

```
p̂_θ(y_t | y_{1:t-1}) = (1/N) Σ_{i=1}^N w_t^{(i)}
```

with `w_t^{(i)} = g_θ(y_t | x_t^{(i)})` and `x_t^{(i)}` the propagated
particle states.

### Conditional (IC-free) likelihood

Define the likelihood conditional on `y_1`:

```
L_c(θ | y_1) = L(θ) / p_θ(y_1)
            = ∏_{t=2}^{T} p_θ(y_t | y_{1:t-1})
```

Taking logs:

```
log L_c(θ | y_1) = log L(θ) − log p_θ(y_1)
                = Σ_{t=2}^{T} log p_θ(y_t | y_{1:t-1})
```

`L_c` is a valid likelihood function for `θ` — it is the density of
the data `y_{2:T}` given `y_1` and `θ`, treating `y_1` as known and
marginalising over `x_0`. Maximising `L_c` (or sampling from `π(θ) ·
L_c`) gives an inference that does not commit to any particular
`μ_θ`.

### PF estimator of `L_c`

The PF algorithm for `L_c` is identical to the standard PF except in
one place: the log-likelihood accumulation skips `t=1`. The
weight-and-resample step at `t=1` still happens — it is exactly the
mechanism by which `y_1` pins `x_0`, producing a particle
distribution approximating `p(x_0 | y_1, θ)` — but the weight sum is
not added to the running log-likelihood.

```
initialise particles x_0^{(i)} from μ̃(·)   # broad, θ-independent
for t in 1..T:
    propagate: x_t^{(i)} ~ f_θ(· | x_{t-1}^{(i)})
    weight:    w_t^{(i)} = g_θ(y_t | x_t^{(i)})
    if t > 1:                               # IC-free guard
        log_L += log((1/N) Σ_i w_t^{(i)})
    resample particles ∝ w_t^{(i)}
```

The robustness claim (rooted in the Bayesian ergodicity results of
Chopin 2004 and extensions in Del Moral 2004) is that as `N → ∞`,
the particle distribution after the `t=1` resample converges to
`p(x_0 | y_1, θ)` regardless of the specific broad prior `μ̃` used,
provided `μ̃` assigns positive density wherever `p(x_0 | y_1, θ)` does.
In practice this means: so long as the initial particle cloud
covers the plausible initial-state space, the fit is insensitive to
the specific shape of the broad prior.

## Related work

- **King, Nguyen, Ionides (2016), "Statistical Inference for
  Partially Observed Markov Processes via the R Package pomp," J.
  Stat. Softw. 69(12).** Sections 4.1–4.3 document the initial-value
  problem in pomp and discuss the three strategies (fixed, estimated,
  IC-free). pomp's `rinit` hook is the user-supplied function that
  defines `μ_θ`; users wanting IC-free inference set `rinit` to
  return a diffuse draw.
- **Ionides, Nguyen, Atchadé, Stoev, King (2015), "Inference for
  dynamic and latent variable models via iterated, perturbed Bayes
  maps," PNAS 112(3):719–724.** IF2 algorithm. The algorithm is
  agnostic to `μ_θ`; supplying a diffuse `rinit` makes it effectively
  IC-free.
- **King, Ionides, Pascual, Bouma (2008), "Inapparent infections and
  cholera dynamics," Nature 454:877–880.** Canonical worked example
  of IC-free compartmental inference — they fit a cholera model
  without committing to initial prevalence, conditioning the
  likelihood on the first observation in each outbreak season.
- **Bretó, He, Ionides, King (2009), "Time series analysis via
  mechanistic models," Ann. Appl. Stat. 3(1):319–348.** Earlier
  treatment of the same idea; equation (3.3) is the conditional
  likelihood form used above.
- **Chopin (2004), "Central limit theorem for sequential Monte Carlo
  methods and its application to Bayesian inference," Ann. Stat.
  32(6):2385–2411.** Theoretical guarantee that the PF's initial
  state distribution after a single resample converges to the
  posterior given `y_1`, independent of the specific broad prior
  `μ̃`.

### Explicitly NOT in scope: renewal-equation / EpiEstim

- **Cori, Ferguson, Fraser, Cauchemez (2013), "A New Framework and
  Software to Estimate Time-Varying Reproduction Numbers During
  Epidemics," Am. J. Epidemiol. 178(9):1505–1512.** EpiEstim paper.
- **Fraser (2007), "Estimating Individual and Household Reproduction
  Numbers in an Emerging Epidemic," PLoS ONE 2(8):e758.**

These replace the compartmental structure during outbreak growth
with a branching process parameterised by `R_t` and a generation
interval distribution. The approach *also* avoids committing to
`I₀`, but does so by changing the process model (no `(S, I, R)`,
just an incidence series), not the likelihood factorisation. It's a
different and larger feature. See "Out of scope" below.

## Design

### User-facing surface

A single new field on `[fit]` in fit.toml:

```toml
[fit]
model     = "boarding_school_sir.camdl"
ic_free   = true        # condition likelihood on y₁
```

Absent: likelihood is computed over `y_{1:T}` (today's behaviour).
Present and true: PF initialises particles with spread, weights and
resamples at `t=1` without accumulating that step into the
log-likelihood, and accumulates normally from `t=2`.

### Why fit.toml (not CLI, not .camdl)

- **Not the CLI.** Whether the inference conditions on `y_1` is a
  statistical-model choice, not an operational knob. CLI flags
  belong to operational concerns (`--parallel`, `--force`, `--seed`).
  Routing this through a CLI flag would make the same fit.toml mean
  two different things depending on invocation — the exact
  provenance failure camdl's design rejects.
- **Not the .camdl model file.** The `.camdl` defines what the
  model *is* — structure, compartments, transitions, observation
  blocks. The same model can legitimately be fit in IC-free and
  non-IC-free modes depending on which question is being asked
  (full-epidemic mechanistic fit vs growth-phase `β` estimation).
  Baking the IC-free choice into the model would force users to
  maintain two `.camdl` files that differ only in inference setup.
- **fit.toml is the right home** because (per `camdl-run-spec.md`
  §1.2) it owns "how inference RUNS": which params to estimate,
  which algorithm, which data, what priors, what seeds. `ic_free` is
  one more inference-setup choice of the same kind.

### Required precondition: particle spread at `t=0`

IC-free only works if the initial particle cloud has *spread* across
plausible initial states. Otherwise the `t=1` reweight step has
nothing to discriminate between — all particles get the same weight,
resampling is a no-op, and IC-free degenerates to "the fit without
the first observation." This is silently wrong.

camdl's existing machinery provides the spread via **IVP
perturbation** (`ivp = true` on an estimated parameter, used today
by IF2 / PGAS to inject stochasticity into initial states):

```toml
[estimate]
I0   = { bounds = [1, 500], ivp = true }   # ← required for ic_free
beta = { bounds = [0.1, 5.0] }
```

Validation rule: when `ic_free = true`, the fit config must have at
least one `[estimate]` entry with `ivp = true` *or* a `.camdl`-level
declaration of a stochastic initial distribution (future work —
today only IVP covers this). A fit config with `ic_free = true` but
no ivp entry errors at validation:

```
error: ic_free = true requires at least one [estimate.*] entry with
       ivp = true. Without per-particle variation at t=0, the first
       observation cannot discriminate between particles and ic_free
       degenerates to dropping the first data point.

       Example: mark your initial-state parameter as ivp:

           [estimate]
           I0 = { bounds = [1, 500], ivp = true }
```

### Diagnostic output

Already there are startup blocks for interventions, observations,
and priors. Add a one-liner when `ic_free = true`:

```
  ic-free inference: conditioning on y₁
    - initial state spread from ivp params: [I0]
    - log-likelihood accumulation from t = 2 (y₁ used for reweighting only)
```

Silent when `ic_free = false` or absent.

### Output schema

The `fit_state.toml` gains one optional field:

```toml
[fit_state]
# ...existing fields...
ic_free    = true        # only set when the fit used ic_free
ic_pin_obs = 7           # the y₁ time used to pin, in model time units
                         # (first obs time in the data file)
```

`summary.tsv` and `coverage.tsv` schemas unchanged — they already
expose `loglik` and whatever the log-likelihood objective was for a
given fit. The docstring on `loglik` is clarified to note that
ic_free fits report the conditional log-likelihood `log L_c`, which
is NOT directly comparable to non-ic_free `log L` on the same data
(they differ by `log p_θ(y_1)`).

## Implementation

### Files touched

| File | Change |
|------|--------|
| `rust/crates/sim/src/inference/traits.rs` | Add `skip_first_obs_from_loglik: bool` to `SMCConfig`; default false. |
| `rust/crates/sim/src/inference/particle_filter.rs` | Guard loglik accumulation with `if obs_idx > 0 \|\| !config.skip_first_obs_from_loglik`. Resampling unchanged. |
| `rust/crates/sim/src/inference/if2.rs` | Same guard in the obs loop. |
| `rust/crates/sim/src/inference/pgas.rs` | Same guard — conditional likelihood applies symmetrically for Bayesian stages. |
| `rust/crates/cli/src/fit/config_v2.rs` | Add `ic_free: Option<bool>` on `FitConfigV2`. Validation: if true, require at least one `ivp = true` in `[estimate]`. |
| `rust/crates/cli/src/fit/runner.rs` | Thread `ic_free` into `SMCConfig.skip_first_obs_from_loglik` in `FitRunConfig::smc_config()`. |
| `rust/crates/cli/src/util.rs` | Extend the startup diagnostic functions with the ic_free block. |
| `docs/camdl-inference-spec.md` | Add `§3.8 IC-Free Inference` with the equations, precondition, and worked example. |
| `docs/camdl-run-spec.md` | Add `ic_free: Option<bool>` to the `FitConfig` type in §6.2. |

Approximate LOC:
- Core PF / IF2 / PGAS change: ~15 lines (three `if` guards plus config plumbing).
- Validation + diagnostic: ~30 lines.
- Documentation: ~50 lines.
- Tests: ~100 lines.

Total: ~200 lines, no architectural change. The existing IVP
machinery does all the heavy lifting on the initial-state-spread
side.

### SMCConfig change in detail

```rust
// rust/crates/sim/src/inference/traits.rs
pub struct SMCConfig {
    // ...existing fields...

    /// When true, the particle filter still weights and resamples at
    /// the first observation (pinning the initial state via Bayesian
    /// update) but does NOT accumulate that step's log-sum-exp into
    /// the returned log-likelihood. This is the IC-free / conditional-
    /// likelihood formulation from King et al. 2008, Bretó et al. 2009.
    ///
    /// Requires particles to have spread at t=0 — typically via at
    /// least one `ivp = true` estimated parameter. Validation is at
    /// the fit-config layer, not here.
    #[serde(default)]
    pub skip_first_obs_from_loglik: bool,
}
```

### PF guard in detail

```rust
// rust/crates/sim/src/inference/particle_filter.rs

for obs_idx in 0..n_obs {
    // ...existing propagate step...
    // ...existing weight step...

    // Log-likelihood accumulation: skip t=1 for IC-free.
    // The reweight-and-resample happens either way — that's what
    // pins x_0 | y_1.
    if !(config.skip_first_obs_from_loglik && obs_idx == 0) {
        log_likelihood += log_sum_exp_minus_log_n(&log_weights);
    }

    // ...existing resample step...
}
```

Three places total (PF, IF2, PGAS) with identical shape.

## UX

### fit.toml

The canonical home. Minimal example (boarding-school SIR fit,
IC-free with β and I₀ both estimated):

```toml
[fit]
model      = "boarding_school_sir.camdl"
ic_free    = true

[data.observations]
cases = "data/boarding_school.tsv"

[estimate]
beta = { bounds = [0.1, 5.0], start = 2.0 }
I0   = { bounds = [1, 500],   start = 5.0, ivp = true }

[fixed]
gamma = 0.5
N0    = 763

[stages.mle]
algorithm     = "if2"
backend     = "chain_binomial"
chains     = 8
particles  = 1000
iterations = 100
cooling    = 0.9
```

### CLI

No new CLI flag. Quick A/B comparison via two fit.toml files:

```bash
camdl fit run fit_commit.toml     # fixes I0, reports tight beta posterior
camdl fit run fit_ic_free.toml    # ic_free=true, wider beta posterior
```

The downstream result-processing tools (`camdl list`, `camdl show`,
plus book-chapter R/Python) can then diff the posteriors. This is
the whitepaper demonstration: side-by-side posteriors from two
fit.toml files that differ by one line.

### Startup output

Abbreviated example of the diagnostic block when `ic_free = true`:

```
fit: fit_ic_free.toml (1 stage)
  model:    boarding_school_sir.camdl
  estimate: beta, I0 (ivp)
  fixed:    gamma, N0
  output:   results/fits/fit_ic_free/real/fit_1/

  ic-free inference: conditioning on y₁
    - initial state spread from ivp params: [I0]
    - log-likelihood accumulation from t = 2

  observations (1 stream):
    ✓ cases  incidence(S→I)  NegativeBinomial

── stage: mle (method=if2) ──
  ...
```

When `ic_free = false` (absent), the `ic-free` block is silent.

### Error path

Attempting to set `ic_free = true` without any ivp-flagged estimate:

```
$ camdl fit run fit_broken.toml
error: ic_free = true requires at least one [estimate.*] entry with
       ivp = true. Without per-particle variation at t=0, the first
       observation cannot discriminate between particles and ic_free
       degenerates to dropping the first data point.

       Example: mark your initial-state parameter as ivp:

           [estimate]
           I0 = { bounds = [1, 500], ivp = true }
```

## Test plan

Five tests:

1. **`ic_free_fit_requires_ivp`** — config validation: `ic_free =
   true` with no ivp entries errors cleanly with the guidance above.

2. **`ic_free_loglik_skips_first_obs`** — unit test against
   `bootstrap_filter` on a tiny synthetic dataset. Build two PF
   configs identical except for `skip_first_obs_from_loglik`.
   Assert: `loglik_standard - loglik_ic_free ≈ log p̂(y_1)` where
   `p̂(y_1)` is the log-sum-exp of the t=1 weights, computed
   independently. This is the direct check that `log L - log L_c =
   log p(y_1)`.

3. **`ic_free_passes_through_if2`** — end-to-end: run a toy SIR fit
   with `ic_free = true`, assert it completes, and assert
   `fit_state.toml` has `ic_free = true` recorded.

4. **`ic_free_widens_beta_posterior_vs_committed`** — the
   whitepaper demonstration as a test. Toy SIR dataset where the
   (β, I₀) ridge is real. Run two fits: (a) I₀ fixed at the truth
   exactly, (b) ic_free with I₀ ivp. Assert the posterior SD on β
   is strictly wider under (b) than (a) by a factor of ≥ 1.5. This
   test encodes the methodological claim and will fail red if a
   future refactor accidentally breaks the t=1 guard.

5. **`ic_free_startup_diagnostic_names_ivp_params`** — smoke test
   on the stderr output: when ic_free is active, the startup block
   names the ivp params contributing to the initial-state spread.
   Captures regressions in the user-facing communication of what
   the fit is actually doing.

## Worked example (for the whitepaper)

Boarding-school influenza. 14 daily observations. True `(β, γ) =
(2.07, 0.65)` (from He et al. 2010's MLE), `I₀ = 1`, `N₀ = 763`.

**Committed fit** — I₀ fixed at 1:

```toml
[fixed]
I0    = 1
gamma = 0.5
N0    = 763

[estimate]
beta = { bounds = [0.1, 5.0] }
```

**IC-free fit** — I₀ estimated with broad bounds, ic_free true:

```toml
ic_free = true

[fixed]
gamma = 0.5
N0    = 763

[estimate]
beta = { bounds = [0.1, 5.0] }
I0   = { bounds = [1, 100], start = 5, ivp = true }
```

Expected outcome (to be validated once implemented):

- Committed `β` posterior: narrow around the MLE, maybe SD ≈ 0.1.
- IC-free `β` posterior: visibly wider, SD ≈ 0.3–0.5, covering the
  range of `β` values consistent with `y_1` under plausible `I₀`.
- Median `β` similar under both (ridge is along `(β · I₀) = const`
  roughly, so fixing `I₀` at truth doesn't bias the location — just
  narrows the uncertainty).

The whitepaper argument is the width difference: the committed fit's
confidence is paid for with a pretence; the IC-free fit's width is
what's actually known about `β` from 14 days of cases.

## Out of scope

- **Per-observation-stream `ic_free`.** Multi-stream fits (e.g.
  cases + hospitalisations) might reasonably want to condition on
  the first observation in one stream but not the other. Punt until
  we see the need; the fit-level flag covers single-stream fits,
  which are the targeted use case.
- **Renewal-equation / EpiEstim.** Replaces the process model with a
  branching process driven by `R_t` and a generation interval
  distribution. Different backend, new IR fields for the GI kernel,
  roughly 500–1000 lines plus spec work. Worth having a separate
  proposal when someone asks for growth-phase `R_t` estimation on
  data not warranting a full mechanistic fit.
- **Automatic broad-prior selection for `x_0`.** IVP perturbation +
  broad `[estimate].*.bounds` is the current mechanism for spread.
  A dedicated "stochastic initial distribution" declaration in
  `.camdl` (e.g. `init { I ~ Poisson(5) }`) would be more ergonomic
  but is a language change; defer.
- **Reporting both `log L` and `log L_c`.** When `ic_free = true` we
  report `log L_c`. Reporting both would let users quantify
  `log p(y_1)` directly, which is the information-theoretic "cost"
  of the first observation. Cheap to add later if useful; not
  required for the initial landing.

## Why this design is clean

- **Tiny implementation.** One conditional in three PF-family
  functions; one Option<bool> on FitConfigV2; one validation rule;
  one diagnostic line. No new machinery, no schema expansion beyond
  a single boolean.
- **Reuses existing infrastructure.** IVP perturbation already
  injects t=0 spread for exactly this kind of use case. The "where
  does the initial particle spread come from?" question is already
  answered — we just need to require the answer.
- **Inference-setup choice lives in the inference-setup file.**
  fit.toml owns how inference runs; ic_free joins priors, transforms,
  seeds, and stages as one of those levers. No philosophical
  renegotiation needed.
- **Error paths catch the silent-wrong-answer case.** The degenerate
  "ic_free without spread" configuration is rejected at config load,
  not silently accepted and producing nonsense. Matches the project's
  "no silent wrong answers" stance.
- **Directly enables a whitepaper demonstration.** Two fit.toml
  files differing by one line, run side by side, produce posteriors
  that illustrate the epistemic-laundering argument concretely.
  That's the point.
