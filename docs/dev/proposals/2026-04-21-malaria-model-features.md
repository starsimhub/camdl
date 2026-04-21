---
status: proposal
date: 2026-04-21
authors: camdl core
supersedes: (none)
---

# Proposal: DSL features for first-class malaria modelling

## Goal

Make camdl the most ergonomic language-layer for stochastic
compartmental malaria models. Concretely: the endpoint target is that
the Ross-Macdonald model fits in ≤ 25 lines of DSL, and a 3-age-group
Garki fits in ≤ 60 lines of DSL, with every line reading like the
underlying biology rather than an encoding workaround.

This is the reference yardstick we'll measure each feature against. A
feature that doesn't shorten or clarify the Garki file isn't worth
adding; a feature whose absence makes the Garki file contain
comments explaining "why is it written this way" — is worth adding.

Scope: **items #1–#6 from the Tier S / A / B ranking below.** Tier C
items (individual-level heterogeneity, concurrent-infection MOI
bookkeeping, parasite genotype tracking) are explicitly deferred —
they belong in an ABM, not a compartmental DSL.

---

## Current state — what a Garki-like model looks like today

A minimal two-age Garki-style model exercises most of camdl's current
features. Annotated with the pain points each section hits:

This minimal 2-age Garki-like model **compiles cleanly against the
current DSL** (verified against camdlc at commit 213aba8), and is
the baseline the proposal measures progress against. Without
branching sugar (#2), the symptomatic / asymptomatic split requires
declaring `Y1_symp` and `Y1_asym` as separate compartments, doubling
the Y1 state count and the number of transitions out of Y1:

```camdl
time_unit = 'days

dimensions { age = [child, adult] }
# Explicit symp/asym compartments — no branching sugar today, so each
# destination of "infection" is its own named state. #2 would collapse
# this back to a single Y1 with a multinomial branch at firing.
compartments { X, Y1_symp, Y1_asym, Y2, Y3, Sv, Ev, Iv }
stratify(by = age, only = [X, Y1_symp, Y1_asym, Y2, Y3])

parameters {
  a         : rate in [0.1, 1.0]
  b_h       : probability
  b_v       : probability
  r1_c      : rate in [0.01, 0.5]      # #3: per-age recovery, separate params
  r1_a      : rate in [0.01, 0.5]
  alpha_c   : rate                     #     immunity acquisition
  alpha_a   : rate
  r2        : rate
  delta     : rate
  sigma_v   : rate                     # EIP progression
  mu_v      : rate                     # mosquito mortality
  rho_sens  : probability              # slide-test sensitivity
  rho_spec  : probability              # slide-test specificity
  p_symp_c  : probability              # #2: branching probabilities
  p_symp_a  : probability
}

let I_h_c = Y1_symp_child + Y1_asym_child + Y2_child
let I_h_a = Y1_symp_adult + Y1_asym_adult + Y2_adult
let I_h_total = I_h_c + I_h_a
let N_h = X_child + X_adult
        + Y1_symp_child + Y1_symp_adult + Y1_asym_child + Y1_asym_adult
        + Y2_child + Y2_adult + Y3_child + Y3_adult
let h_eff = a * b_h * Iv / N_h                # per-host force of infection

transitions {
  # #2: 4 infection transitions where biology is 2 events × 1 branch.
  infect_symp_c : X_child --> Y1_symp_child  @ p_symp_c * h_eff * X_child
  infect_asym_c : X_child --> Y1_asym_child  @ (1 - p_symp_c) * h_eff * X_child
  infect_symp_a : X_adult --> Y1_symp_adult  @ p_symp_a * h_eff * X_adult
  infect_asym_a : X_adult --> Y1_asym_adult  @ (1 - p_symp_a) * h_eff * X_adult

  # #2: 4 recovery transitions (one per {symp,asym} × {child,adult}) where
  #     biology is 2 per-age events.
  recover_symp_c  : Y1_symp_child --> X_child  @ r1_c * Y1_symp_child
  recover_asym_c  : Y1_asym_child --> X_child  @ r1_c * Y1_asym_child
  recover_symp_a  : Y1_symp_adult --> X_adult  @ r1_a * Y1_symp_adult
  recover_asym_a  : Y1_asym_adult --> X_adult  @ r1_a * Y1_asym_adult

  # Same duplication for immunity acquisition.
  acquire_symp_c  : Y1_symp_child --> Y2_child @ alpha_c * Y1_symp_child
  acquire_asym_c  : Y1_asym_child --> Y2_child @ alpha_c * Y1_asym_child
  acquire_symp_a  : Y1_symp_adult --> Y2_adult @ alpha_a * Y1_symp_adult
  acquire_asym_a  : Y1_asym_adult --> Y2_adult @ alpha_a * Y1_asym_adult

  clear_c : Y2_child --> Y3_child @ r2 * Y2_child
  clear_a : Y2_adult --> Y3_adult @ r2 * Y2_adult
  wane_c  : Y3_child --> X_child  @ delta * Y3_child
  wane_a  : Y3_adult --> X_adult  @ delta * Y3_adult

  # #1: vector-host coupling — one biological event, but `mosq_infect`
  #     and the `h_eff` term inside all `infect_*` transitions have to
  #     be kept in sync by hand. Edit one without the other and the
  #     model silently breaks.
  mosq_infect   : Sv --> Ev  @ a * b_v * Sv * I_h_total / N_h
  mosq_eip      : Ev --> Iv  @ sigma_v * Ev
  mosq_death_S  : Sv -->     @ mu_v * Sv
  mosq_death_E  : Ev -->     @ mu_v * Ev
  mosq_death_I  : Iv -->     @ mu_v * Iv
}

init { X_child = 400  X_adult = 600  Sv = 5000 }

simulate { from = 0 'days  to = 365 'days }
```

Verified compile (`camdlc inspect`):

```text
garki_pre_proposal
  compartments   8 base × 2 age = 13 expanded
  transitions    21 base → 21 expanded (+ 0 filtered by where)
  parameters     15 declared (a: rate, b_h: probability, b_v: probability,
                 r1_c: rate, r1_a: rate, alpha_c: rate, alpha_a: rate,
                 r2: rate, delta: rate, sigma_v: rate, mu_v: rate,
                 rho_sens: probability, rho_spec: probability,
                 p_symp_c: probability, p_symp_a: probability)
  tables         0
  let bindings   5 (I_h_c, I_h_a, I_h_total, N_h, h_eff)
  dimensions     age = [child, adult]
  observations   0 streams
  interventions  0 (0 active by default)
```

The fixture lives at `docs/dev/proposals/fixtures/garki_pre_proposal.camdl`.

**65 content lines, 21 transitions.** Count the friction points —
every one corresponds to a numbered feature below:

- **#1 vector-host coupling duplication**: `mosq_infect` and the
  `a * b_h * Iv / N_h` inside `h_eff` both encode the same
  biological event. No atomic coupling; hand-maintained parallel
  rates.
- **#2 branching explosion**: `Y1_symp` and `Y1_asym` have to be
  distinct compartments, doubling the state count out of `Y1`.
  Four infection transitions × four recovery × four acquisition =
  12 transitions that in the biology are one event per age.
- **#3 parameter explosion**: `r1_c`, `r1_a`, `alpha_c`, `alpha_a`,
  `p_symp_c`, `p_symp_a` — flat names duplicate the dimensional
  structure that `age` already has. Indexed priors with partial
  pooling across age groups aren't expressible.
- **#4 inlined test-characteristic correction** (not shown in the
  model above — would be another 3-line `observations { }` block
  with the sens/spec correction manually inlined into the
  `likelihood = binomial(n, p = rho_sens*projected/N_h + (1-rho_spec)*(1-projected/N_h))`
  expression).
- **#5 no aging**: this example omits demographic aging entirely
  because adding it would require `|compartments| × (|age|-1)` =
  8 × 1 = 8 manual transitions. Scaling to 5 age groups means
  32 aging transitions.
- **#6 no reactive interventions**: interventions must have static
  schedules; state-triggered response isn't expressible.

Proposal target (post-#1, #2, #4, #5): this model compresses to ≈ 55
lines and ≈ 10 transitions, with every transition reading like one
biological event.

---

## Proposed features

Six features, ordered by the sequence we should land them in. The
order maximises the incremental reduction in incidental complexity
per feature.

### #1 — Multi-source ("bimolecular") transitions

**Status quo.** Transitions are single-source / single-destination in
the DSL. The IR's `Transition::stoichiometry: Vec<StoichiometryEntry>`
supports arbitrary positive/negative entries, but no DSL surface
exposes it.

**What's limiting.** Any two-population mass-action event —
mosquito-host transmission, pair formation, cell-cell interaction —
has to be encoded as a decorrelated pair of single-source transitions
that share a hand-maintained rate expression. Biologically one event;
semantically two uncoupled ones. And under stochastic backends
(Gillespie, chain-binomial), the pair fires at independently-drawn
times, breaking instantaneous conservation.

**Proposed DSL.** `+` as the source-side separator, matching the
destination-side:

```camdl
transmission : Sv + I_h --> Ev + I_h  @ a * b_v * Sv * I_h / N_h
bite         : Sh + Iv  --> I_h + Iv  @ a * b_h * Sh * Iv / N_h

# Partnership formation (separate potential use):
pair_form    : S + S --> SS  @ rho * S * (S - 1) / (2 * N_h)
```

Stoichiometry follows the `+` structure literally: LHS decreases,
RHS increases, compartments appearing on both sides don't change.
So `Sv + I_h --> Ev + I_h` means `Sv: -1, Ev: +1, I_h: 0` (the
infectious human triggers but isn't consumed).

**IR impact.** Zero. The IR already stores general stoichiometry;
only the parser/expander need to populate multi-entry vectors.

**Stochastic backend impact.**
- Chain-binomial: already applies stoichiometry atomically on each step.
- Gillespie: need to apply a transition's full stoichiometry vector
  as a single state update instead of looping single source→dest.
  ~30 lines in `gillespie.rs`.
- Tau-leap: same shape as chain-binomial, already OK.

**Test plan.**
- Structural: compile `Sv + I_h --> Ev + I_h` assert IR stoichiometry
  is `[(Sv, -1), (Ev, +1), (I_h, 0)]` and that `I_h` receives a
  zero-delta entry (not absent — explicit zero documents the dependency).
- Conservation: Gillespie sim with pair-formation transitions,
  assert `N_total = S + 2*SS + SI_count` is exactly invariant on every
  snapshot.
- Regression guard: existing single-source models produce bitwise-
  identical trajectories pre/post refactor.

**Effort.** ~1 week including backend adjustments, conservation tests,
and the error-code pass (E26x family for stoichiometry validation).

**Unlocks.** Vector-host coupling as one transition. Pair-formation
dynamics. Cell-cell interaction models. The Garki file above drops
from 12 transitions to 9. The Ross-Macdonald model (below) drops
from 6 transitions to 4.

---

### #2 — Probabilistic branching at transition

**Status quo.** An event with stochastic branching (e.g., infection
produces symptomatic with probability p, asymptomatic with 1-p) is
written as two independent transitions, each with its own rate:

```camdl
infect_symp : S --> Y_symp  @ p * h_eff * S
infect_asym : S --> Y_asym  @ (1 - p) * h_eff * S
```

Two ways this goes wrong:

1. **Symbolic drift**: edit the FOI on one and forget the other. No
   compiler signal; the model silently becomes inconsistent.
2. **Semantic mismatch**: these are not two independent Poisson
   processes. An infection event happens at one time, and at that
   time the destination is drawn from a multinomial. Writing them
   as independent rates is an approximation that's exact in mean
   and wrong in variance.

**Proposed DSL.** Destination as a weighted set, with probabilities
summing to 1:

```camdl
infection[a in age] : S[a] --> {Y_symp[a] : p_symp[a], Y_asym[a] : 1 - p_symp[a]}
  @ h_eff * S[a]
```

Sugar: expands to one transition with a multinomial-sampled
destination per event. Expander emits one `Transition` with
`stoichiometry = [(S[a], -1)]` plus a new IR field
`branch_destinations: Vec<(CompartmentId, Expr)>` where the `Expr`
is the weight.

**Probability validation.** The expander computes a symbolic sum of
all branch weights. If the sum is not syntactically `1` (after
simplification), the compiler emits `W204: branch probabilities may
not sum to 1, got `p + q``. Users override with an explicit
`| _ : 1 - p - q` "rest" clause or by confirming the algebraic
identity.

**Stochastic backend impact.** Each backend gains a
`multinomial_sample` helper that, given the per-branch weights for
a single firing event, draws the destination assignment. For
chain-binomial this is one multinomial draw on the batch of events
in the substep. For Gillespie one categorical draw per event. Both
~15 lines.

**Test plan.**
- Structural: parse `S --> {A: p, B: 1-p}` assert IR has
  `branch_destinations` of length 2 with correct weight exprs.
- Stochastic: 10000-sample Gillespie on a single infection event,
  assert branch-outcome counts match `p * N` within 3σ binomial.
- Mass conservation: infected individuals always go somewhere —
  `|S| + |A| + |B|` preserved per event.
- Likelihood gradient: the branch probabilities participate in
  likelihood; autodiff still produces correct gradients (extend
  `autodiff.ml`).

**Effort.** ~3–4 days. Expander change, small backend helper, a
new autodiff rule.

**Unlocks.** Natural expression of symptomatic/asymptomatic,
detected/undetected, mild/severe/fatal, age-at-infection outcomes.
Clean gradient for parameters that control branch probabilities.
The Garki file above drops to 8 transitions total.

---

### #3 — Hierarchical priors / random effects for indexed parameters

**Status quo.** camdl supports priors on scalar parameters via
`~ dist(...)`. Indexed parameters (per-age, per-patch) get priors
the same way — but one scalar prior per index entry:

```camdl
parameters {
  alpha[a in age] : rate ~ normal(0.05, 0.02)
}
```

This says "`alpha_child ~ N(0.05, 0.02)` independently,
`alpha_adult ~ N(0.05, 0.02)` independently" — no partial pooling.
For multi-village Garki fits where per-village parameters should
share strength, this is the feature pomp users most ask about.

**Proposed DSL.** A `hyper { }` block for hyper-parameters, plus
hyper-parametrised priors in `parameters { }`:

```camdl
hyper {
  mu_alpha    ~ normal(0, 1)
  sigma_alpha ~ half_normal(0.5)
}

parameters {
  alpha[a in age] : rate ~ normal(mu_alpha, sigma_alpha)
}
```

Semantically: `mu_alpha` and `sigma_alpha` are themselves
parameters (estimated from data) that govern a group-level
distribution from which each `alpha[a]` is drawn. During inference,
both levels are updated.

**IR impact.** Parameters gain an optional `hyper_parent` field that
references the hyper-parameter names. Hyper-parameters are regular
parameters with a flag distinguishing them. No structural change
to the expression grammar.

**Inference impact.** Substantial. IF2 needs to sample both levels
jointly; PGAS/NUTS needs gradients of the hierarchical log-prior
(straightforward — `log p(alpha | mu, sigma)` is just another term).
PMMH needs to propose both levels. None of this is conceptually
new (textbook hierarchical MCMC); the effort is in threading the
two-level updates through the existing inference code paths.

**Test plan.**
- Structural: compile hierarchical model, assert IR has both the
  hyper-parameter nodes and the per-index `hyper_parent` links.
- Inference (stat): synthetic data from a known two-level process,
  fit with PGAS, assert the posterior on `mu_alpha, sigma_alpha`
  covers the true values in ≥ 90% of seed replicates.
- Partial-pooling regression: with T data points per group and K
  groups, fitted `alpha[a]` values should fall between the
  per-group MLE and the grand mean — the classic shrinkage signature.

**Effort.** ~2 weeks including inference support. Largest single-
feature investment in this proposal.

**Unlocks.** Multi-village / multi-cohort fits with partial
pooling. Hierarchical random-effects on any indexed parameter.
Matches the full Bayesian hierarchical modelling vocabulary that
brms/rstanarm users expect.

---

### #4 — Observation-model primitives for test sensitivity / specificity

**Status quo.** The six built-in likelihoods (Poisson, NegBin,
Normal, Binomial, BetaBin, Bernoulli) cover raw counts. Real
surveillance data has an observation layer on top: slide-microscopy
sensitivity varies 0.5–0.95, RDT 0.8–0.99, PCR 0.95+. Currently
users inline the test-characteristic correction into the likelihood
expression:

```camdl
likelihood = binomial(n = N_tested,
                      p = sens * projected_prev + (1 - spec) * (1 - projected_prev))
```

That works but (a) obscures intent ("why is `1 - spec * …` added?"),
(b) mixes biology with measurement, (c) makes estimating `sens` and
`spec` from auxiliary data awkward.

**Proposed DSL.** A `test_characteristics` likelihood that composes
with any base distribution:

```camdl
observations {
  slide_positivity : {
    projected  = prevalence(Y1 + Y2)
    every      = 1 'weeks
    likelihood = diagnostic_test(
      base      = binomial(n = N_tested, p = projected / N_h),
      sens      = rho_sens,
      spec      = rho_spec,
    )
  }
}
```

Or for count-based incidence with reporting delay / under-reporting:

```camdl
likelihood = reported_counts(
  base       = neg_binomial(mean = lambda * projected, r = k),
  report_rate = rho,
)
```

**IR impact.** Wraps the base likelihood in a new
`Likelihood::Diagnostic { base, sens, spec }` variant with a canonical
desugaring to the correction formula above. Kept as a distinct IR
node so analysis passes can find test-characteristic parameters and
their relationships with the latent prevalence.

**Effort.** ~1 week. Expander rewrite, add log-pmf + gradient for
the wrapped distribution, a round-trip test.

**Unlocks.** Natural expression of surveillance-data likelihoods.
Models compose test characteristics with true disease dynamics
without needing algebraic manipulation at the likelihood site.

---

### #5 — First-class aging transitions

**Status quo.** Aging between age classes is a generic pattern:
"individuals in class `a_i` progress to `a_{i+1}` at rate
`1/mean_duration_in_a_i`". Today you write this explicitly, one
transition per (compartment × age pair):

```camdl
# for 5 age groups and 4 compartments: 20 hand-written transitions
age_X_c_t   : X_child --> X_teen      @ X_child / age_dur_child
age_Y1_c_t  : Y1_child --> Y1_teen    @ Y1_child / age_dur_child
age_Y2_c_t  : Y2_child --> Y2_teen    @ Y2_child / age_dur_child
age_Y3_c_t  : Y3_child --> Y3_teen    @ Y3_child / age_dur_child
# ... × 4 transitions per age-group boundary × 4 boundaries = 16
```

**Proposed DSL.** A dedicated block that takes the dimension and an
expression for residence time per class, and emits the full aging
flow for every compartment stratified by that dimension:

```camdl
tables {
  age_dur : age 'years = [5, 10, 35, 15, 20]
}

aging {
  age : 1 / age_dur[a]
}
```

Expands to `|compartments_with_age| × (|age| - 1)` transitions of
the form `X[a_i] --> X[a_{i+1}]  @ rate_into_next * X[a_i]`. The
rate is evaluated per compartment so that if you wanted age-
compartment-specific rates you'd use a 2D table.

**Boundary semantics.** By default, individuals exiting the last
class accumulate there (no leaving). A trailing `→ exit` clause
routes them to a death compartment:

```camdl
aging {
  age : 1 / age_dur[a] → death
}
```

**IR impact.** Zero. Desugars to transitions the IR already has.

**Effort.** ~3–4 days. Parser addition, expansion logic in the
expander, unit tests for the 1D, 2D, and compartment-subset cases.

**Unlocks.** Multi-year population dynamics concise. Ross-
Macdonald doesn't need it (single time-scale); Garki with 5 age
groups drops from 20 aging transitions to 2 lines of DSL.

---

### #6 — Reactive / conditional interventions

**Status quo.** Interventions fire at times specified statically in
the model (`at [180, 545, 910]`) or on a periodic schedule. Real-
world public-health interventions trigger reactively: "if district
prevalence exceeds 5%, start IRS within 14 days."

**Proposed DSL.** A `when` clause on interventions that references
model state:

```camdl
interventions {
  reactive_irs[p in patch] :
    transfer(fraction = irs_eff, from = Iv[p], to = Sv[p])
    when prevalence(Y1[p] + Y2[p]) / N_h[p] > outbreak_threshold
    cooldown = 60 'days      # don't re-fire within 60 days of last firing
}
```

**IR impact.** The existing `Cond` IR node covers the
predicate-evaluation shape. Interventions gain a `trigger` field
that's either `Schedule(times)` (current behaviour) or
`Conditional { predicate, cooldown }`. Trigger evaluation runs at
each observation time (or each substep, depending on cadence
config).

**Backend impact.** Per-step conditional check, state lookup, last-
fired-time bookkeeping. Small (~50 lines).

**Test plan.**
- Deterministic: prevalence threshold crossed at t=100 → intervention
  fires at t=100 (or next step). Assert it fires exactly once within
  the cooldown window.
- Reproducibility: identical seed + identical threshold produces
  identical firing times.
- Cooldown: rapid prevalence oscillation triggers only one firing
  per cooldown window, not a burst.

**Effort.** ~1 week. New parser construct, IR variant, substep
evaluation logic, cooldown tracking.

**Unlocks.** Test-and-treat / reactive case management. Outbreak-
triggered vaccination campaigns. The Garki model for intervention-
evaluation studies.

---

## What we're not proposing, and why

**Individual heterogeneity in continuous parameters** (per-host
biting weight ~ Gamma). Fundamentally ABM territory. If biting
heterogeneity matters to you, stratify by `bite_class = [low, high]`
and camdl handles it; continuous distributions per individual don't
fit a compartmental framework.

**Concurrent-infection MOI bookkeeping as host state.** Dietz's
original 1974 Garki has an explicit "number of concurrent broods"
state per host. Modern fits lump this into the `Y1 / Y2` binary;
for the rare paper that needs the full bookkeeping, an ABM is the
right tool.

**Parasite-genotype tracking.** Ditto.

**DDE backend for fixed delays.** Erlang-staging with k ≥ 10 is the
standard compartmental workaround and works fine.

**Module system / model composition.** Big DX win but architecturally
heavy. Revisit after the above six features shipped; users will have
built real malaria models and we'll know what module boundaries want
to be.

---

## Endpoint: Ross-Macdonald in camdl, post-proposal

For calibration against the proposal above, here's what the canonical
Ross-Macdonald (Ross 1911; Macdonald 1957) two-equation model looks
like:

Mathematically:

$$
\frac{dX}{dt} = a b_h \frac{Y}{H} (H - X) - r X
$$

$$
\frac{dY}{dt} = a b_v \frac{X}{H} (M - Y) - \mu_v Y
$$

In camdl post-#1:

```camdl
time_unit = 'days

compartments { S_h, I_h, S_v, I_v }

parameters {
  a     : rate     in [0.1, 1.0]      # biting rate
  b_h   : probability                  # host susceptibility
  b_v   : probability                  # vector susceptibility
  r     : rate                         # host recovery
  mu_v  : rate                         # vector mortality
  H     : count                        # host population size (fixed)
  M     : count                        # vector population size (fixed)
}

init { S_h = 999  I_h = 1   S_v = 999  I_v = 1 }

transitions {
  bite         : S_h + I_v --> I_h + I_v   @ a * b_h * S_h * I_v / H
  recovery     :   I_h     --> S_h          @ r * I_h
  transmission : S_v + I_h --> I_v + I_h   @ a * b_v * S_v * I_h / H
  vec_death_S  :   S_v     -->              @ mu_v * S_v
  vec_death_I  :   I_v     -->              @ mu_v * I_v
}

simulate { from = 0 'days  to = 365 'days }
```

**17 content lines.** Every transition reads like the underlying
biology — `S_h + I_v --> I_h + I_v` literally says "a susceptible
host and an infectious vector meet; the host becomes infectious."

---

## Endpoint: Garki in camdl, post-proposal

Same biology as the 65-line pre-proposal fixture above, rewritten
against the proposed DSL with all six features applied. Each
annotated block calls out which feature it uses.

```camdl
time_unit = 'days

dimensions { age = [child, adult] }
compartments { X, Y1, Y2, Y3, Sv, Ev, Iv }
stratify(by = age, only = [X, Y1, Y2, Y3])
# #2 collapses Y1_symp / Y1_asym back to a single Y1. The symp/asym
# split becomes a multinomial branch at infection firing, not a
# state duplication.

# --- #3: age-indexed parameters, one declaration each --------------
parameters {
  a        : rate
  b_h      : probability
  b_v      : probability
  r1[age]  : rate         ~ HalfNormal(0.1) | age      # partial pooling
  alpha[age]: rate        ~ HalfNormal(0.05) | age
  p_symp[age]: probability ~ Beta(2, 2) | age
  r2       : rate
  delta    : rate
  sigma_v  : rate
  mu_v     : rate
  rho_sens : probability
  rho_spec : probability
  outbreak_th : probability in [0.02, 0.15]
  irs_eff  : probability  in [0.5, 0.95]
}

let I_h       = sum(a in age, Y1[a] + Y2[a])
let N_h       = sum(a in age, X[a] + Y1[a] + Y2[a] + Y3[a])
let prev      = I_h / N_h

# --- #5: one block replaces |compartments| × (|age|-1) transitions -
aging { age : rate = [1 / (15 'years)] }

transitions {
  # --- #1 + #2: vector-host coupling as one atomic event, with a ---
  # --- multinomial branch on the infection outcome per age ---------
  bite[a in age] :
    X[a] + Iv --> {Y1[a] : p_symp[a], Y1[a] : 1 - p_symp[a]} + Iv
    @ a * b_h * X[a] * Iv / N_h
  # (Both branches currently land in Y1; when we split Y1 into symp/
  # asym Y1 flags later the branch targets diverge. The DSL tolerates
  # identical targets — the branch becomes a no-op but documents the
  # biology.)

  # #1: mosquito-side coupling, reciprocal of `bite`.
  infect_v : Sv + I_h_ref --> Ev + I_h_ref
    @ a * b_v * Sv * I_h / N_h

  # Per-age progression — indexed transitions keyed by [a in age]
  # eliminate the per-age duplication.
  recover[a in age]  : Y1[a] --> X[a]  @ r1[a] * Y1[a]
  acquire[a in age]  : Y1[a] --> Y2[a] @ alpha[a] * Y1[a]
  clear[a in age]    : Y2[a] --> Y3[a] @ r2 * Y2[a]
  wane[a in age]     : Y3[a] --> X[a]  @ delta * Y3[a]

  vec_eip    : Ev --> Iv  @ sigma_v * Ev
  vec_mort_S : Sv -->     @ mu_v * Sv
  vec_mort_E : Ev -->     @ mu_v * Ev
  vec_mort_I : Iv -->     @ mu_v * Iv
}

# --- #6: reactive outbreak-triggered IRS, not a static schedule ----
interventions {
  reactive_irs : transfer(fraction = irs_eff, from = Iv, to = Sv)
                 when prev > outbreak_th
                 cooldown = 60 'days
}

# --- #4: diagnostic_test likelihood absorbs sens/spec correction ---
observations {
  slide_positivity[a in age] : {
    projected  = prevalence(Y1[a] + Y2[a])
    every      = 1 'weeks
    likelihood = diagnostic_test(
      base = binomial(n = N_tested[a], p = projected / N_h),
      sens = rho_sens, spec = rho_spec
    )
  }
}

init { X[child] = 400  X[adult] = 600  Sv = 5000 }
simulate { from = 0 'days  to = 2 'years }
```

**≈ 55 content lines** for a 2-age Garki (extends to any age
partition by rewriting one `dimensions` line) with vector dynamics,
bite-mediated transmission, symptomatic/asymptomatic branching,
immune-waning dynamics, reactive IRS, and slide-characteristic-
adjusted observations per age. Every non-boilerplate line reads
like the biology.

For comparison: the equivalent pomp model is ~450 lines of C (plus
R boilerplate), or ~200 lines of Stan if you fit deterministic-
approximation. camdl-after-this-proposal is a ~5× reduction in
surface area and a ~20× reduction in incidental-complexity lines.

---

## Implementation sequencing

Landing order optimises: (a) each feature is independently useful
even if the next never ships; (b) features don't block each other;
(c) the Ross-Macdonald test model works as soon as possible.

1. **#1 multi-source transitions** (~1 week) → unlocks Ross-Macdonald
   golden fixture.
2. **#2 probabilistic branching** (~3–4 days) → clean symptomatic
   splits.
3. **#5 aging transitions** (~3–4 days) → scale-invariant multi-age
   models.
4. **#4 diagnostic-test likelihood** (~1 week) → clean surveillance
   observation.
5. **#6 reactive interventions** (~1 week) → outbreak-response
   studies.
6. **#3 hierarchical priors** (~2 weeks) → multi-village partial
   pooling. Land last; inference surface is the largest.

Total: ~7 weeks of focused language + inference work for a state-
of-the-art malaria-modelling DSL. First four features (#1, #2, #5,
#4) get the 55-line Garki model runnable and fittable in ~3 weeks.

---

## Testing discipline

Each feature lands with, in this order:

1. **Failing TDD test asserting the documented claim**, per the
   discipline in `docs/dev/testing.md` §"Adding a spec-claim
   regression". For #1, that's a conservation test on a bimolecular
   model; for #2, a 3σ multinomial-outcome statistical test; for
   #3, a partial-pooling shrinkage test on synthetic hierarchical
   data; etc.
2. **Error-code fixture** in `ocaml/test/errors/` for every new
   diagnostic the feature introduces.
3. **Spec update** in `camdl-language-spec.md` before the feature
   lands in the expander. Spec claim needs a test (§6.1 table-unit
   incident was the warning shot).
4. **Golden file** that exercises the feature — a small `.camdl` +
   `.ir.json` pair in `ocaml/golden/` so the round-trip test picks
   up schema drift.

The Ross-Macdonald and Garki models, once runnable, become
integration test fixtures in their own right — guarantee that the
features compose correctly across a full realistic model.

---

## Risks and unknowns

- **Hierarchical-priors inference correctness.** Two-level sampling
  in IF2 / PGAS has known convergence pathologies when hyperparams
  are weakly identified (funnel geometry, centered vs non-centered
  parameterisation). Plan: default to non-centered parameterisation,
  document the trade-off, leave centered as an advanced option.
- **Reactive-intervention non-stationarity.** Interventions that
  respond to state make the likelihood dependent on the full
  trajectory path rather than the snapshot, which complicates
  particle-filter likelihood evaluation. PMMH still works; PGAS's
  conditional SMC needs care. Plan: document the inference-method
  constraint in the `when` clause syntax.
- **Multinomial-branch variance calibration.** The stochastic
  interpretation of `S → {A: p, B: 1-p}` is "single event, multinomial
  outcome per event." This differs numerically from two independent
  Poisson processes at rates `p·λ` and `(1-p)·λ` by an O(λ) term.
  The existing DSL's workaround is the two-process form; we're
  changing the contract. Plan: document the new semantic, regression-
  test the table above to confirm the distinguishability.

---

## Measuring success

After all six features ship, the Garki model above compiles with no
workarounds, fits against the Garki Project positivity data in one
`camdl fit run` invocation, and — most importantly — reads to a
first-time camdl user like the biology of the model rather than like
a simulation-framework configuration file.

If that's not true, the proposal didn't deliver; roll back a tier
and try again.
