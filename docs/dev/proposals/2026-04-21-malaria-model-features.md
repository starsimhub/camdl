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

Scope: **items #1–#5 from the Tier S / A / B ranking below.** Tier C
items (individual-level heterogeneity, concurrent-infection MOI
bookkeeping, parasite genotype tracking) are explicitly deferred —
they belong in an ABM, not a compartmental DSL.

**Not in scope here:** demographic processes — aging, births, age-
specific mortality. These are not malaria features; they're a
general compartmental-modelling concern that any stratified model
needs, and collapsing them into a malaria-specific "aging block" was
the wrong design. See the companion proposal
`2026-04-21-vital-dynamics.md` for that work. The post-proposal
Garki endpoint below compiles without demographic features — the 2-
age model fits in ≤ 60 lines regardless, and multi-age extensions
wait on the vital-dynamics proposal landing.

---

## What's already in camdl (and therefore not in this proposal)

Three features that come up repeatedly in malaria-modelling wishlists
are already shipped. This section documents them explicitly so
reviewers don't double-count them as gaps.

### Environmental forcing / climate-driven rates — already works

Language spec §12 ships a `forcing {}` block with four primitive
forms: `sinusoidal`, `periodic`, `piecewise`, and `interpolated`. The
last reads a TSV with `time_col` / `value_col` and supports linear,
cubic-spline, or PCHIP interpolation. Forcing values are first-class
references in rate expressions. Climate-driven malaria transmission
(rainfall → vector breeding, temperature → EIP) composes from these
primitives plus `let` bindings:

```camdl
forcing {
  rainfall    = interpolated { data = "rain.tsv"  time_col = t  value_col = mm
                               method = "cubic_spline" }
  temperature = interpolated { data = "temp.tsv"  time_col = t  value_col = C
                               method = "cubic_spline" }
}
parameters {
  beta_base, rain_alpha, temp_opt, temp_width : rate, positive, positive, positive
}

let thermal = exp(-((temperature - temp_opt) / temp_width)^2)
let beta_t  = beta_base * (rainfall / 100)^rain_alpha * thermal

transitions {
  bite[a in age] : X[a] + Iv --> Y1[a] + Iv  @ beta_t * X[a] * Iv / N_h
}
```

No new language feature required. The one small sugar that would
help repeated temperature-dependent rates across multiple transitions
(e.g., `sigma_v(T)` appearing at both EIP and mortality sites) is a
named `functions {}` block — ~2 days of work, generic DSL, not
malaria-specific, not blocking. Tracked separately if it becomes a
friction point.

### Spatial / indexed parameters — already works at one level

The `dimensions {}` mechanism is dimension-agnostic; `age`, `patch`,
`village`, and `district` are all the same primitive. Any parameter
can be indexed by any dimension, and any indexed parameter can carry
a prior. #3 below adds shared-hyperprior syntax (`| patch` for
partial pooling over a single dimension) — that works identically
for spatial and demographic indices.

```camdl
dimensions { patch = read("lga_pop.tsv", column = "patch") }  # 36 LGAs from data
parameters {
  beta[patch] : rate ~ log_normal(mu = -1.0, sigma = 0.5) | patch  # per-patch
}
```

What's **not** in scope for this proposal: multi-level nested
hierarchies (region ⊃ district ⊃ village with pooling at each
level). That needs nested-prior surface area like
`beta[village] ~ LogNormal(mu[district(v)], sigma_v) | village`, with
a `district(v)` lookup function. Non-trivial design; deferred to a
dedicated proposal once single-level spatial pooling has been used
against real data long enough to know what the nested surface should
look like.

### Multi-stage infection dynamics — already expressible

Multi-stage within-host progression (e.g., `I_liver → I_early → I_mid
→ I_late → I_gam` with stage-specific rates and detection) does not
need a new feature. It's a `stage` dimension:

```camdl
dimensions { stage = [liver, blood_early, blood_mid, blood_late, gam] }
compartments { X, I, R }
stratify(by = stage, only = [I])

parameters {
  progression[stage] : rate
  gamma_clear[stage] : rate
  p_gam[stage]       : probability
}

transitions {
  # Sequential progression through stages — one indexed transition,
  # the `where` clause keeps it from generating a past-the-end move.
  progress[s in stage, where s < last(stage)] :
    I[s] --> I[next(stage, s)]  @ progression[s] * I[s]

  # Stage-specific clearance with gametocyte branching (uses #2 below).
  clear[s in stage] :
    I[s] --> {R : 1 - p_gam[s], G : p_gam[s]}  @ gamma_clear[s] * I[s]
}
```

The stage dimension plus #2 (branching) covers the 4-6 stage
structure without new language surface.

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
- **#5 no reactive interventions**: interventions must have static
  schedules; state-triggered response isn't expressible.
- **(aging, births, age-specific mortality)**: this fixture omits
  demographic dynamics entirely. Covered by the separate
  `2026-04-21-vital-dynamics.md` proposal — orthogonal to malaria
  and needed equally by any stratified compartmental model.

Proposal target (post-#1, #2, #4): this model compresses to ≈ 55
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

**Proposed DSL.** Everything stays in `parameters { }`. A
hyperparameter is nothing but a parameter that another parameter's
prior references — there's no language-level distinction, only a
reference-graph property the compiler can infer. A trailing
`| <dim>` clause on an indexed parameter's prior marks which
dimension is partially pooled.

```camdl
parameters {
  # Hyperparameters: ordinary scalars with ordinary priors.
  mu_alpha    : rate     ~ normal(mu = 0, sigma = 1)
  sigma_alpha : positive ~ half_normal(sigma = 0.5)

  # Leaf: prior references the hyperparameters above, one draw per age.
  alpha[age]  : rate ~ normal(mu = mu_alpha, sigma = sigma_alpha) | age
}
```

Semantically: `mu_alpha` and `sigma_alpha` are themselves
parameters (estimated from data) that govern a group-level
distribution from which each `alpha[a]` is drawn. During inference,
both levels are updated.

**Why one block, not a `hyper { }` block.** Hyperparameters and
leaves are semantically identical — both get priors, both are
sampled, both support `--params` / `fit.toml [fixed]` overrides.
Splitting them into two blocks (a) forces every downstream consumer
(transform defaults, scenario machinery, `run.json` provenance,
`camdl inspect` output) to handle two cases instead of one, (b)
introduces a language-level distinction that dissolves under the
slightest reparameterisation, and (c) diverges from the one-block
convention in Stan, PyMC, brms, rstanarm. The engineering upside of
a separate block — knowing which parameters need non-centered
reparameterisation in NUTS — is already derivable from the prior
reference graph; no grammar change required.

`camdl inspect --hierarchy` renders the graph explicitly for users
who want to see hyper vs leaf structure:

```
parameters:
  mu_alpha    scalar    hyper (referenced by alpha)
  sigma_alpha scalar    hyper (referenced by alpha)
  alpha       [age]     leaf, pools over age, parent = (mu_alpha, sigma_alpha)
```

**IR impact.** No new block, no new top-level variant. The existing
`Prior` node gains an optional `pool_over: Option<DimName>` field
(from the `| age` clause) and the existing prior-argument slot
already supports parameter references via `Expr::Param`. The
compiler derives the hyper/leaf partition from the reference graph
during a single post-parse walk, storing it in the IR
`Parameter::role: enum { Hyper, Leaf { parent: Vec<Name> }, Plain }`.

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

### #5 — Reactive / conditional interventions

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

#### #5b — Intervention state-read (decay dynamics)

**Motivation.** Real ITN / IRS / drug interventions don't just turn
on or off — efficacy decays after deployment. ITN efficacy falls
with net ageing; IRS efficacy decays with residual insecticide life;
repeated drug deployment raises resistance. Today the only way to
express "ITN efficacy is 95% on the day of distribution and decays
exponentially with 2-year half-life" is to write a `piecewise`
forcing with hand-set deploy times, which breaks the moment
distribution becomes `when`-triggered rather than scheduled.

**Proposed DSL.** Expose the intervention's firing history as
readable scalars inside rate expressions (and inside `when`
predicates):

```camdl
parameters {
  itn_eff_init  : probability in [0.5, 0.95]
  itn_halflife  : time        in [180 'days, 1000 'days]
  outbreak_th   : probability in [0.02, 0.15]
}

interventions {
  itn_distribution :
    transfer(fraction = 0.8, from = Sv, to = Sv_protected)
    when   prevalence > outbreak_th
    cooldown = 2 'years
}

# `itn_distribution.t_last_fired` and `itn_distribution.times_fired`
# are readable anywhere in the rate DSL.
let itn_age = t - itn_distribution.t_last_fired
let itn_eff = if itn_distribution.times_fired > 0
              then itn_eff_init * exp(- log(2) * itn_age / itn_halflife)
              else 0.0

transitions {
  bite[a in age] : X[a] + Iv --> Y1[a] + Iv
    @ (1 - itn_eff) * a * b_h * X[a] * Iv / N_h
}
```

**Surface area.** Two new scalar expressions per intervention:

- `<iv>.t_last_fired` — model time of last firing; `-∞` before first
  firing. Dimension `T`.
- `<iv>.times_fired` — integer count; dimension `[1]`.

No new blocks, no new distributions, no new backends. Trigger
bookkeeping is already required by the cooldown machinery in #5
above; this just exposes it to the expression language.

**IR impact.** One new IR expression node variant:
`InterventionState { intervention: String, field: TLastFired | TimesFired }`.
Propensity evaluator reads from the intervention registry.

**Test plan.** One-intervention model where efficacy decays
predictably: schedule a single firing at t=0, integrate with a
known decay half-life, assert the intervention effect at
t = halflife is exactly half of the initial effect.

**Effort.** ~3 days on top of #5's cooldown bookkeeping. Parser
addition for the `<iv>.field` syntax, IR + propensity additions,
one test.

**Unlocks.** ITN decay, IRS residual efficacy, post-campaign
case-management reactivation windows, resistance build-up models
(`resistance_frac = 1 - exp(-k * drug.times_fired)`).

**What this deliberately does not cover.** Multi-state intervention
*efficacy* tracked as its own compartment (e.g., "netted vs
un-netted households" as populations that migrate between each
other). That's just compartments and transitions — no new feature
needed, write it directly.

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

**Multi-level nested hierarchical priors** (region ⊃ district ⊃
village partial pooling at each level). #3 gives single-level
pooling within one dimension — enough for "villages within a
region," "age effects within a population," or "strains within a
serotype." Nested surface (`beta[village] ~ LogNormal(mu[district(v)],
...)` with a `district(v)` parent-lookup) is a real gap for
multi-country DHS-scale fits but is a dedicated design problem: it
wants lookup functions into the dimension hierarchy, a multi-level
prior distribution in the IR, and careful NUTS reparameterization.
Worth its own proposal once single-level spatial pooling has been
exercised against real data.

**Module system / model composition.** Big DX win but architecturally
heavy. Revisit after the above features shipped; users will have
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

# --- #3: age-indexed leaves + hyperparameters, all in one block.
# The `| age` clause marks the pooling dimension; the compiler detects
# that r1, alpha, p_symp are leaves (their priors reference other
# parameters) and applies non-centered reparameterization in NUTS.
parameters {
  a         : rate
  b_h       : probability
  b_v       : probability

  # Hyperparameters for the per-age-group partially-pooled rates.
  mu_r1     : rate     ~ half_normal(sigma = 0.1)
  sigma_r1  : positive ~ half_normal(sigma = 0.05)
  mu_alpha  : rate     ~ half_normal(sigma = 0.05)
  sigma_alpha : positive ~ half_normal(sigma = 0.02)
  alpha_psymp : positive ~ gamma(shape = 2, rate = 1)
  beta_psymp  : positive ~ gamma(shape = 2, rate = 1)

  # Leaves: priors reference hyperparameters above.
  r1[age]     : rate        ~ log_normal(mu = mu_r1, sigma = sigma_r1) | age
  alpha[age]  : rate        ~ log_normal(mu = mu_alpha, sigma = sigma_alpha) | age
  p_symp[age] : probability ~ beta(alpha = alpha_psymp, beta = beta_psymp) | age

  r2          : rate
  delta       : rate
  sigma_v     : rate
  mu_v        : rate
  rho_sens    : probability
  rho_spec    : probability
  outbreak_th : probability in [0.02, 0.15]
  irs_eff     : probability in [0.5, 0.95]
}

let I_h       = sum(a in age, Y1[a] + Y2[a])
let N_h       = sum(a in age, X[a] + Y1[a] + Y2[a] + Y3[a])
let prev      = I_h / N_h

# Aging / births / age-specific mortality deliberately omitted —
# see 2026-04-21-vital-dynamics.md. For a 1-year fit against Garki
# surveillance data the demographic drift is negligible; multi-decade
# runs would add a `vital_dynamics {}` block per that proposal.

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

# --- #5: reactive outbreak-triggered IRS, not a static schedule ----
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

Three waves, each ending with a runnable and fittable malaria model
so that every wave has an independent demo. Checkboxes track
landed work; update in place as each item ships.

### Wave 1 — "Ross-Macdonald fittable" (~2 weeks)

Goal: a 15-line Ross-Macdonald model that simulates correctly under
Gillespie/tau-leap and fits real slide-positivity data.

- [x] **#1 multi-source transitions** (landed `d88eb7f`, 2026-04-21)
  - [x] Parser: `+` separator on both sides of `-->`
  - [x] Expander: multi-source stoich generation with catalyst collapse
  - [x] Propensity: verified via Ross-Macdonald host-conservation test
        (no code change — IR already general)
  - [x] Error code E310 (fully-catalytic / no-net-effect)
  - [x] Tests: 3 compiler tests + 1 Rust runtime conservation test
  - [x] Spec §9.1.1 updated
  - [x] Golden: `ocaml/golden/ross_macdonald.camdl` + `.ir.json`
- [x] **#4 diagnostic-test likelihood** (landed `674c451`, 2026-04-21)
  - [x] Parser: `diagnostic_test(base = …, sens = …, spec = …)`
  - [x] Pure compile-time sugar — no IR change needed (rewrites
        inner `p` to `sens·π + (1−spec)·(1−π)`, emits base Binomial
        or Bernoulli); scoring + sampling + gradient paths unchanged
  - [x] Error codes E253 (bad base), E254 (missing kwarg)
  - [x] Equivalence test: sugar IR byte-identical to hand-inlined
  - [x] Spec §13.2.1 updated
  - [x] Ross-Macdonald golden extended with slide_positivity
        observation using the sugar
- [ ] **Wave 1 demo**: `docs/vignettes/ross_macdonald_fit.qmd`
  PMMH recovers `a, b_h, b_v` within 2σ on synthetic data.

### Wave 2 — "Garki fittable" (~3 weeks)

Goal: the 55-line post-proposal Garki from §"Endpoint" above
compiles and fits age-specific partial-pooling parameters.

- [x] **#2 probabilistic branching** (landed `9c6530a`, 2026-04-21)
  - [x] Parser: `{ dest : weight, ... }` on transition destination
  - [x] Pure compile-time sugar — no IR change needed (desugars to
        one transition per branch with rate = weight_i × original_rate;
        existing chain-binomial / tau-leap source-grouping does the
        multinomial split). Zero new runtime code paths.
  - [x] Atomicity + correct-split runtime tests on Gillespie, tau-leap,
        AND chain-binomial. 200 seeds × ~900 draws, 9σ tolerance —
        would catch any bias in the multinomial weights.
- [ ] **#3 hierarchical priors (single-level)** (~2 weeks)
  - [ ] Parser: `| <dim>` pooling clause; `~` with parameter refs
  - [ ] Semantic: reference-graph walk → hyper/leaf classification
  - [ ] IR: `Parameter::role` enum; `Prior::pool_over` field
  - [ ] Inference: non-centered reparameterization in NUTS; joint
        updates in PGAS + IF2 + PMMH
  - [ ] `camdl inspect --hierarchy` visualiser
  - [ ] Shrinkage regression test: fitted leaves between per-group
        MLE and grand mean
  - [ ] Posterior-coverage test on synthetic two-level data (≥ 90%)
- [ ] **Wave 2 demo**: `docs/vignettes/garki_2age_fit.qmd`
  Recovers age-specific `p_symp` with pooled-sigma shrinkage.

### Wave 3 — "Policy-grade interventions" (~2 weeks)

Goal: reactive, decay-aware interventions; counterfactual policy
simulation with proper uncertainty.

- [ ] **#5 reactive interventions** (~1 week)
  - [ ] Parser: `when <predicate>` + `cooldown = <duration>`
  - [ ] IR: `Intervention::trigger: Conditional { predicate, cooldown }`
  - [ ] Runtime: substep predicate eval + cooldown tracking +
        last-fired bookkeeping (state survives inference restarts)
  - [ ] Decide eval cadence default (observation cadence vs substep)
  - [ ] Reproducibility: identical seed → identical firing times
- [ ] **#5b intervention state-read** (~3 days)
  - [ ] Parser: `<iv>.t_last_fired`, `<iv>.times_fired`
  - [ ] IR: `Expr::InterventionState { iv, field }`
  - [ ] Propensity: read from intervention registry
  - [ ] Sentinel semantics at t=0 before any firing
  - [ ] Decay test: single firing, efficacy half at t = halflife
- [ ] **Wave 3 demo**: `docs/vignettes/reactive_irs.qmd`
  Scheduled vs reactive vs no-IRS trajectories with CIs.

### Parallel track — Vital dynamics (~2 weeks)

See `2026-04-21-vital-dynamics.md`. Independent of the waves above;
ship anytime. No inter-track dependencies in either direction.

### Cross-cutting hygiene

Every wave follows the discipline in `docs/dev/testing.md`:

- Failing TDD test asserting the documented claim **before** code.
- Error-code fixture in `ocaml/test/errors/` for every new diagnostic.
- Spec update merges **before** implementation.
- Each wave ends with a book chapter, not just code — the vignettes
  are the reality check on DSL ergonomics.

**Total**: ~7 weeks end-to-end for a fittable, documented, state-of-
the-art malaria DSL. First Ross-Macdonald demo at ~2 weeks.

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
