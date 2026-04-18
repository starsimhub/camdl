# camdl Value of Information Specification

**Version:** 0.1-draft\
**Date:** 2026-03-25\
**Depends on:** camdl Experiment Spec v0.5 (§7 designs, §8 output structure)

---

## 1. Purpose

This document specifies how to compute the Expected Value of Sample Information
(EVSI) for epidemiological decision problems using camdl. It is a **consumer**
of the experiment system — it reads experiment outputs and computes
decision-theoretic quantities. It does not modify the experiment runner, the
simulation engine, or the IR.

If something in this spec cannot be expressed using the experiment spec's output
schemas, that is a bug in the experiment spec, not a reason to extend this one.

---

## 2. The Decision Problem

Every EVSI computation requires four ingredients:

**1. Prior π(θ)** — current beliefs about model parameters.

**2. Actions A** — the set of decisions available. In epi: "do 2 SIA rounds,"
"do 4 rounds," "do nothing."

**3. Utility U(a, θ)** — the value of taking action a when the true parameters
are θ. In epi: cases averted minus cost, QALYs gained, probability of
elimination.

**4. Study S** — a possible information acquisition. A coverage survey, a
transmission study, a serosurvey. The study produces data d that updates
beliefs: π(θ) → π(θ|d).

**EVSI** answers: "what is the expected improvement in my decision outcome if I
conduct study S before deciding?"

```
EVSI(S) = E_d[ max_a E[U(a, θ) | d] ] − max_a E[U(a, θ)]
            ↑                                ↑
      decide AFTER study              decide NOW
```

The first term is the expected utility when you observe the study result and
then choose optimally. The second is the utility when you choose optimally with
current beliefs. The difference is what the study is worth.

---

## 3. Connection to the Experiment System

The experiment spec provides the raw material:

| Experiment concept                     | VOI role                         |
| -------------------------------------- | -------------------------------- |
| `[design.*]` with `prior = {...}`      | Generates θ samples from π(θ)    |
| `[[scenario]]`                         | Actions A                        |
| `[compare.derived]` utility expression | U(a, θ)                          |
| `parameter_points.tsv`                 | θ samples with prior weights     |
| `outputs.tsv`                          | U(a, θ) evaluated at each sample |

The VOI computation is **post-processing** on these outputs. No new simulations
are needed — EVSI is computed by reweighting existing simulation results.

---

## 4. The VOI File

A TOML file that references a completed experiment and defines the decision
problem:

```toml
# voi.toml — value of a coverage survey

[voi]
experiment = "experiment.toml" # path to completed experiment
design = "prior" # which design's outputs to use

# ── The decision ───────────────────────────────────────

[decision]
actions = ["no_sia", "two_rounds", "four_rounds"]
reference = "no_sia" # baseline for utility computation
utility = "cases_averted" # derived quantity from experiment

# ── Studies to evaluate ────────────────────────────────

[study.coverage_survey]
parameter = "vacc_eff"
likelihood = "beta_binomial"
sample_sizes = [50, 200, 500, 2000]

[study.transmission_study]
parameter = "R0"
likelihood = "log_normal"
observation_sd = [0.5, 0.2, 0.1] # sd of log(observed R0)

[study.serosurvey]
parameter = "vacc_eff"
likelihood = "binomial"
sample_sizes = [100, 500, 1000]
```

### 4.1 `[decision]` Fields

| Field       | Type         | Description                                    |
| ----------- | ------------ | ---------------------------------------------- |
| `actions`   | list[string] | Scenario names (must exist in experiment)      |
| `reference` | string       | Baseline scenario for utility computation      |
| `utility`   | string       | Derived quantity name from `[compare.derived]` |

The utility is a scalar per (action, θ, seed). For a given θ and seed, the
optimal action is `argmax_a U(a, θ)`.

### 4.2 `[study.*]` Fields

Each study block defines one possible information acquisition:

| Field            | Type        | Description                                              |
| ---------------- | ----------- | -------------------------------------------------------- |
| `parameter`      | string      | Which parameter the study informs                        |
| `likelihood`     | string      | Data-generating model (see §5)                           |
| `sample_sizes`   | list[int]   | Evaluate EVSI at multiple study sizes                    |
| `observation_sd` | list[float] | For continuous likelihoods (alternative to sample_sizes) |

Multiple studies can be defined. EVSI is computed independently for each study
at each sample size / observation precision.

---

## 5. Study Likelihood Models

The study likelihood p(d | θ) defines what data the study would produce given
the true parameter value. This is NOT a prior — it's a forward model of the
measurement process.

### 5.1 `beta_binomial`

For coverage surveys: observe k successes in n trials.

```
d = k ~ Binomial(n, θ)
```

Where θ is the true parameter value (e.g., vacc_eff). The posterior update for a
Beta prior is analytic:

```
π(θ) = Beta(α, β)
π(θ|k, n) = Beta(α + k, β + n - k)
```

For non-conjugate priors, use importance sampling (§6.2).

### 5.2 `binomial`

Same as beta_binomial. Convenience alias.

### 5.3 `log_normal`

For transmission studies: observe a noisy estimate of a rate.

```
d = log(θ_observed) ~ Normal(log(θ_true), σ²)
```

Where σ is the observation standard deviation in log space, specified via
`observation_sd`.

### 5.4 `normal`

For direct measurements with known noise:

```
d = θ_observed ~ Normal(θ_true, σ²)
```

### 5.5 Custom (future)

User-defined likelihood functions for complex study designs (multi-parameter
studies, hierarchical observations).

---

## 6. EVSI Computation

### 6.1 The Algorithm

Given N prior samples θ₁, ..., θ_N with corresponding utilities U(a, θᵢ) for
each action a:

```
For each study S at sample size n:
  1. Current optimal:
     EU_current(a) = (1/N) Σᵢ U(a, θᵢ)
     a*_current = argmax_a EU_current(a)
     V_current = EU_current(a*_current)

  2. For j = 1, ..., M (Monte Carlo over study outcomes):
     a. Draw a "true" parameter θ* from the prior samples
     b. Simulate study data: d_j ~ p(d | θ*, n)
     c. Compute importance weights:
        w_i = p(d_j | θᵢ, n) for each prior sample
        (how likely is this study outcome under each θᵢ)
     d. Posterior-weighted utility:
        EU_posterior(a | d_j) = Σᵢ w_i U(a, θᵢ) / Σᵢ w_i
     e. Posterior-optimal action:
        a*_j = argmax_a EU_posterior(a | d_j)
        V_j = EU_posterior(a*_j | d_j)

  3. EVSI(S, n) = (1/M) Σⱼ V_j − V_current
```

This is the **preposterior analysis** framework (Raiffa & Schlaifer 1961,
updated by Strong et al. 2014 for health economics).

### 6.2 Importance Sampling Details

The key insight: we don't re-run the model. The utilities U(a, θᵢ) are already
computed from the experiment's `outputs.tsv`. The importance weights w_i = p(d_j
| θᵢ) reweight these existing samples to approximate the posterior expectation.

For conjugate models (Beta-Binomial for coverage surveys), the posterior is
analytic and the importance weights are:

```
w_i = p(d_j | θᵢ) = Binomial(k_j | n, θᵢ)
```

Where k_j is the simulated number of survey successes and θᵢ is the i-th prior
sample's value of the study parameter.

**Effective sample size check:** after computing weights, check ESS = (Σ wᵢ)² /
Σ wᵢ². If ESS < N/5, the importance sampling is unreliable — the study outcome
is too informative for the prior sample to approximate the posterior. Flag in
output.

### 6.3 Handling Stochastic Seeds

The experiment runs each θ-point at multiple seeds. For the EVSI computation,
average U(a, θᵢ) over seeds first:

```
Ū(a, θᵢ) = (1/|seeds|) Σ_s U(a, θᵢ, s)
```

Then use Ū in the importance-weighted expectation. This removes stochastic noise
from the utility estimates.

### 6.4 Computational Cost

No new simulations. The EVSI computation is O(N × M × |A|) floating point
operations, where N = prior samples (~1000-4000), M = study outcome Monte Carlo
draws (~1000), |A| = number of actions (~3-5). This runs in milliseconds.

The expensive part was the original experiment run. EVSI is free
post-processing.

---

## 7. Output

### 7.1 `evsi.tsv`

```tsv
study               sample_size  EVSI     EVSI_se  current_EU  optimal_action_current
coverage_survey      50          1234     156      45000       two_rounds
coverage_survey      200         2890     203      45000       two_rounds
coverage_survey      500         3401     189      45000       two_rounds
coverage_survey      2000        3650     175      45000       two_rounds
transmission_study   50          456      98       45000       two_rounds
transmission_study   200         1230     145      45000       two_rounds
```

Reading: "a coverage survey of 500 people is expected to improve the decision
outcome by ~3400 cases averted (SE = 189). A transmission study of 50 sites
improves it by only ~456. The coverage survey is ~7x more valuable."

### 7.2 `study_comparison.tsv`

```tsv
study_a              n_a   study_b              n_b   EVSI_a  EVSI_b  ratio
coverage_survey      500   transmission_study   200   3401    1230    2.77
coverage_survey      200   serosurvey           500   2890    2100    1.38
```

Pairwise comparison of studies at specified sample sizes.

### 7.3 `action_sensitivity.tsv`

How often the optimal action changes after observing the study:

```tsv
study               sample_size  P_switch  most_common_switch
coverage_survey      50          0.12      two_rounds → four_rounds
coverage_survey      500         0.31      two_rounds → four_rounds
transmission_study   200         0.05      two_rounds → two_rounds
```

If the study never changes the optimal action (P_switch ≈ 0), it has no decision
value regardless of EVSI magnitude. This table catches the case where EVSI is
nonzero but the decision is insensitive.

### 7.4 `diminishing_returns.tsv`

EVSI as a function of sample size for each study:

```tsv
study               sample_size  EVSI     marginal_EVSI
coverage_survey      50          1234     1234
coverage_survey      100         2100     866
coverage_survey      200         2890     790
coverage_survey      500         3401     511
coverage_survey      1000        3580     179
coverage_survey      2000        3650     70
```

`marginal_EVSI` = EVSI(n) − EVSI(n_prev). Where this drops below the cost of
additional sampling, stop collecting data.

### 7.5 `assumptions.txt`

Auto-generated, non-negotiable:

```
VOI analysis assumptions:

Decision:
  Actions: no_sia, two_rounds, four_rounds
  Utility: cases_averted (from experiment compare.derived)
  Reference: no_sia

Prior:
  N = 2048 samples from design "prior"
  vacc_eff ~ Beta(4.0, 6.0) over [0.1, 0.9]
  R0 ~ LogNormal(1.0, 0.5) over [1.0, 5.0]

Studies:
  coverage_survey: Binomial(n, vacc_eff), n ∈ {50, 200, 500, 2000}
  transmission_study: LogNormal(log(R0), σ), σ ∈ {0.5, 0.2, 0.1}

Method:
  Preposterior importance sampling (Strong et al. 2014)
  M = 1000 Monte Carlo draws per study configuration
  Seed averaging: U(a, θ) averaged over 100 seeds per θ-point

Effective sample size:
  All study configurations had ESS > N/5 (importance sampling reliable)
  Minimum ESS: 412 (coverage_survey, n=2000)

Limitations:
  - EVSI assumes the study informs exactly one parameter
  - Prior is approximated by the design sample, not the analytic form
  - Importance sampling degrades when the study is very informative
    (posterior far from prior)
  - Utility averaging over seeds assumes the decision criterion is
    the expected value; risk-averse criteria need different treatment
```

---

## 8. Experiment Spec Requirements

For VOI to work, the experiment must provide:

**Required from experiment outputs:**

- `parameter_points.tsv` with prior sample values (from `[design.*]`)
- `outputs.tsv` with per-point, per-scenario, per-seed summaries
- `[compare.derived]` defining the utility quantity

**Required NEW in experiment spec (§7 extension):**

- `prior = { dist = "beta", alpha = 4.0, beta = 6.0 }` on design parameters.
  Currently the experiment spec has `range` and `transform` but no
  distributional specification. For VOI, we need the prior to compute importance
  weights. Without it, we can only assume uniform over the range.

This is the one extension the VOI spec requires of the experiment spec.
Everything else is already there.

```toml
# In experiment.toml — the prior field is what VOI needs
[design.prior.parameters.vacc_eff]
range = { min = 0.1, max = 0.9 }
prior = { dist = "beta", alpha = 4.0, beta = 6.0 }

[design.prior.parameters.R0]
range = { min = 1.0, max = 5.0 }
prior = { dist = "log_normal", mu = 1.0, sigma = 0.5 }
```

If `prior` is omitted, the design samples uniformly (current behavior) and VOI
uses a uniform prior — which is a valid but less informative analysis.

---

## 9. CLI

```bash
# Compute EVSI for all studies defined in voi.toml
camdl voi run voi.toml

# Output to specific directory
camdl voi run voi.toml --output analysis/voi/

# JSON output alongside TSV
camdl voi run voi.toml --json
```

Implementation: Rust. ~200 lines. Reads TSVs, computes importance weights,
evaluates nested expectation. No simulation.

---

## 10. Python Figures

The `camdl-analysis` Python package provides visualization:

```bash
# EVSI diminishing returns curves (one line per study)
camdl-analysis voi-curves voi.toml --output figures/

# Study comparison bar chart
camdl-analysis voi-compare voi.toml --output figures/

# Action sensitivity heatmap
camdl-analysis voi-actions voi.toml --output figures/
```

**Diminishing returns plot:** X-axis = sample size (or 1/σ for continuous
studies). Y-axis = EVSI. One curve per study. Shows where each curve flattens —
the point where additional data isn't worth collecting. This is the most
decision-relevant figure.

**Study comparison bar chart:** Horizontal bars, one per study at a fixed sample
size. Length = EVSI. Directly answers "which study should I fund?"

**Action sensitivity heatmap:** For each study × sample size, color by P(action
switches). Dark = study frequently changes the decision. Light = study is
informative but decision-insensitive.

---

## 11. Worked Example: SIA Campaign Design

"Should we fund a coverage survey or a transmission study before deciding how
many SIA rounds to conduct in northern Nigeria?"

**Step 1: Run the experiment**

```bash
camdl simulate batch experiment.toml --parallel 16
camdl list results/simulate/
```

The experiment has `[design.prior]` with 2048 LHS samples from informative
priors on vacc_eff, R0, gamma. Three scenarios: no_sia, two_rounds, four_rounds.
100 seeds each. Total: 2048 × 3 × 100 = 614,400 runs.

**Step 2: Define the decision problem**

```toml
# voi.toml
[voi]
experiment = "experiment.toml"
design = "prior"

[decision]
actions = ["no_sia", "two_rounds", "four_rounds"]
reference = "no_sia"
utility = "cases_averted"

[study.coverage_survey]
parameter = "vacc_eff"
likelihood = "beta_binomial"
sample_sizes = [50, 100, 200, 500, 1000, 2000]

[study.transmission_study]
parameter = "R0"
likelihood = "log_normal"
observation_sd = [0.5, 0.3, 0.2, 0.1]
```

**Step 3: Compute EVSI**

```bash
camdl voi run voi.toml
camdl-analysis voi-curves voi.toml --output figures/
```

**Step 4: Read the results**

The diminishing returns plot shows: coverage survey EVSI rises steeply to n=500,
then flattens. Transmission study EVSI rises slowly and never exceeds coverage
survey at any precision. The coverage survey at n=500 is the optimal study
design — it's where EVSI per additional sample drops below the cost threshold.

The action sensitivity table shows: at n=500, P_switch = 0.31, mostly from
"two_rounds → four_rounds." The study frequently changes the decision,
confirming it has real decision value — not just variance reduction.

---

## 12. Relationship to Other Specs

```
┌──────────────────────────┐
│  .camdl model            │ structure, scenarios
│  params.toml             │ parameter values
└────────────┬─────────────┘
             │ compiled + simulated by
             ▼
┌──────────────────────────┐
│  experiment.toml         │ designs, sweeps, seeds, comparisons
│  Experiment Spec v0.5    │ → parameter_points.tsv, outputs.tsv
└────────────┬─────────────┘
             │ consumed by
             ▼
┌──────────────────────────┐
│  voi.toml                │ decision, studies, EVSI
│  VOI Spec v0.1           │ → evsi.tsv, diminishing_returns.tsv
└──────────────────────────┘
```

The dependency is one-way. The experiment spec never mentions VOI. The VOI spec
references experiment output schemas. If VOI needs something the experiment
doesn't provide, the experiment spec is extended — the VOI spec stays a pure
consumer.

---

## 13. Implementation Phases

### v0.3-core: Basic EVSI

- `[decision]` and `[study.*]` parsing
- Importance sampling with conjugate likelihoods (Beta-Binomial,
  LogNormal-Normal)
- `evsi.tsv`, `diminishing_returns.tsv`, `assumptions.txt`
- ESS diagnostic
- ~200 lines Rust

### v0.3-figures: Python visualization

- Diminishing returns curves
- Study comparison bars
- Action sensitivity heatmap
- ~150 lines Python (matplotlib)

### v0.3-multi: Multi-parameter studies

- Studies that jointly inform 2+ parameters
- Multivariate importance weights
- Joint posterior updates

### v0.4: Risk-averse criteria

- CVaR (conditional value at risk) instead of expected utility
- Minimax regret
- Robust decision making under deep uncertainty
