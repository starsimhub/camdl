---
status: proposal
date: 2026-04-21
authors: camdl core
supersedes: (none)
related: 2026-04-21-malaria-model-features.md (split out from this)
---

# Proposal: Vital dynamics (births, aging, age-specific mortality)

## Goal

Give camdl a correct, first-class demographic layer — births, aging
between age classes, and age-specific mortality — as a single
orthogonal feature, decoupled from any particular disease model.

The malaria proposal (`2026-04-21-malaria-model-features.md`)
previously bundled aging as feature #5. That bundling was wrong: a
demographic sublanguage is not a malaria feature, and the malaria
proposal's sketch (`aging { age : rate }`) encodes age-group
residency as an exponential process, which is biologically wrong in
ways that matter for the long-horizon policy models where
demographics actually bite. This proposal picks up that concern
separately, with the malaria proposal free to land without it.

Scope: *only* the demographic background process. Disease dynamics,
stratification, and observation structure are governed by their own
features and untouched here.

---

## Why this is not just "add an aging block"

Three separate biological processes, entangled if collapsed:

1. **Aging** is deterministic cohort-clock time. Every individual in
   age class `a_i` exits to `a_{i+1}` after exactly `dur(a_i)` years,
   not at exponential rate `1 / dur(a_i)`. Under exponential aging
   the steady-state age pyramid is correct but the transient cohort
   response is wrong — some individuals "age" in 1 year, others in
   30. For disease fits on ≤ 3-year windows this is invisible; for
   multi-decade elimination runs it's not.

2. **Births** are a population-replacement flow into the youngest
   age class. Without an explicit birth process, aging + mortality
   drain the model monotonically: young age classes empty, the
   population ages deterministically toward extinction. This is the
   silent failure mode of "just add aging transitions."

3. **Age-specific mortality** varies by orders of magnitude across
   age classes (U5 mortality vs adult mortality vs elderly
   mortality). Lumping it into a constant background rate distorts
   the age pyramid and thereby every age-stratified fit.

All three must be co-designed. An "aging only" feature is a trap: it
looks useful in a two-age model with a 2-year simulation window, and
quietly produces wrong answers the moment someone adds a third age
class or a 20-year horizon.

---

## Current state

Today the user writes all three processes explicitly as transitions:

```camdl
# For 4 age groups × 4 disease compartments:
# 3 aging boundaries × 4 compartments = 12 aging transitions
# 4 age classes × 4 compartments × 4 mortality rates = 16 death transitions
# 1 birth source-less transition

aging_X_u5_yth   : X[u5]    --> X[youth]    @ X[u5]    / age_dur_u5
aging_X_yth_adl  : X[youth] --> X[adult]    @ X[youth] / age_dur_youth
aging_X_adl_eld  : X[adult] --> X[elderly]  @ X[adult] / age_dur_adult
# ... ×4 compartments = 12 aging lines

death_X_u5       : X[u5]      -->   @ mu_u5      * X[u5]
death_X_youth    : X[youth]   -->   @ mu_youth   * X[youth]
death_X_adult    : X[adult]   -->   @ mu_adult   * X[adult]
death_X_elderly  : X[elderly] -->   @ mu_elderly * X[elderly]
# ... ×4 compartments = 16 death lines

birth            :            --> X[u5]  @ birth_rate * N_total
```

Pain points:

- **29 transitions of pure boilerplate** that encode no epidemiology
  — they're all demographic accounting.
- **Exponential aging**: the shorthand `1 / age_dur` is wrong for
  transient dynamics in a way users rarely notice until a paper
  referee does.
- **Population conservation is manual**: the user has to set
  `birth_rate` such that `births = total_deaths` at steady state, or
  accept population drift. This is a calculation that the language
  could do.
- **No coupling to disease compartments**: if you add a new disease
  compartment `V` (vaccinated), you have to remember to also add its
  four mortality and three aging transitions. Every time.

---

## Proposed DSL

One block, three concerns, kept separable:

```camdl
time_unit = 'days

dimensions { age = [u5, youth, adult, elderly] }
compartments { S, E, I, R }
stratify(by = age, all_compartments = true)

vital_dynamics {
  # ── Aging ────────────────────────────────────────────────────────
  # Deterministic cohort clock: everyone in age[i] moves to age[i+1]
  # after exactly dur[i] time units. Implemented as Erlang-k stages
  # under the hood (k ≥ 20 ⇒ coefficient of variation ≤ 5%).
  aging {
    dimension      = age
    durations      = [5 'years, 10 'years, 50 'years, ∞]
    method         = deterministic         # or `exponential` for opt-in wrong-but-simple
    on_exit(last)  = mortality             # last class exits via age-mortality
  }

  # ── Births ──────────────────────────────────────────────────────
  # New individuals enter the youngest age class. The rate can be
  # absolute (rate × N_total) or constrained to conserve population.
  births {
    target = S[u5]
    rate   = crude_birth_rate * N_total    # or: rate = balance  (auto-balance to deaths)
  }

  # ── Age-specific mortality ──────────────────────────────────────
  # Hazard per age class. Applied uniformly across all compartments
  # carrying the `age` dimension.
  mortality {
    dimension = age
    rates     = [mu_u5, mu_youth, mu_adult, mu_elderly]
  }
}

parameters {
  crude_birth_rate : rate                 # per capita
  mu_u5, mu_youth, mu_adult, mu_elderly : rate
}
```

What the compiler generates:

- For **deterministic aging**: insert `k` hidden sub-stages per age
  class, aging them at rate `k / dur`. For `k = 20` the residence
  time is Gamma-distributed with CV ≈ 0.22; for `k = 100`, CV ≈
  0.10. User picks `k` via `method = deterministic(stages = 20)`
  with a documented default.
- For **exponential aging**: one transition per (compartment, age
  boundary), rate `1 / dur[a_i]`. Exactly today's hand-written
  pattern.
- For **mortality**: one `-->` transition per (compartment, age
  class), rate `mu[a]`.
- For **births**: one sourceless transition into the target
  compartment. If `rate = balance`, the compiler emits
  `rate = sum(a in age, mu[a] * N[a])` so the population is
  conserved to numerical precision.

### Interaction with existing features

- `stratify(by = age, only = [...])` continues to decide which
  compartments carry the age dimension. `vital_dynamics { aging }`
  only generates transitions for compartments that *do* carry the
  age dimension — vector compartments (`Sv`, `Ev`, `Iv`) that aren't
  age-stratified stay untouched.
- The `balance {}` block (population conservation constraint)
  continues to apply *after* vital dynamics transitions fire each
  substep.
- `#3 hierarchical priors` from the malaria proposal composes
  orthogonally: `mu_u5 ~ LogNormal(...)` works as any other
  parameter.

### Alternative forms considered

- **Leslie-matrix block**: fertility × survival matrix applied
  annually. Closer to the demography literature, but mismatched with
  camdl's substep-resolved simulation model. Discrete-annual
  projection forces a numerical-integration boundary we don't
  currently impose elsewhere. Rejected; revisit if a user actually
  asks.
- **Per-compartment override**: `vital_dynamics { mortality { S[u5]:
  base * 0.5 } }` for comorbidity-adjusted mortality. Punted to a
  later proposal; 90% of models use uniform-across-compartments
  mortality and the override complicates the expansion rule.

---

## IR impact

Zero. The block desugars to existing transition forms:

- Deterministic aging → Erlang chain (existing feature via
  `consecutive()`), just generated instead of hand-written.
- Mortality → sourceless transitions.
- Births → sourceless transitions with sourceless stoichiometry.

The IR already supports all three shapes. This is a pure expander
feature.

---

## Effort

| Piece                                    | Estimate |
|------------------------------------------|----------|
| Parser: `vital_dynamics {}` block         | 2 days   |
| Expander: aging (deterministic + exp)     | 3–4 days |
| Expander: births + balance auto-rate      | 2 days   |
| Expander: age-specific mortality          | 1 day    |
| Tests: conservation, cohort clock CV, balance-rate match | 3 days |
| Spec update + golden fixture              | 2 days   |
| **Total**                                 | **~2 weeks** |

---

## What this unlocks

- **Long-horizon policy models**: vaccination-coverage buildup over
  20 years, elimination feasibility studies, maternal-antibody
  cohort effects — all of which break under exponential aging.
- **Multi-age malaria without boilerplate**: the Garki model
  extended to the standard 5-age Dietz-1974 partition drops 29
  boilerplate transitions to 6 lines of DSL.
- **Demographic calibration**: fit `crude_birth_rate` and age-
  specific `mu[a]` against census data as any other parameter. Not
  possible today without naming each mortality rate individually.
- **Orthogonal to disease dynamics**: adding a new disease
  compartment requires zero demographic bookkeeping — `stratify(by =
  age)` pulls it in automatically.

---

## Target model sketch

A demographic-plus-SEIR model (no malaria-specific features needed):

```camdl
time_unit = 'days

dimensions { age = [u5, youth, adult, elderly] }
compartments { S, E, I, R }
stratify(by = age, all_compartments = true)

parameters {
  beta, sigma, gamma : rate, rate, rate
  crude_birth_rate   : rate
  mu_u5, mu_youth, mu_adult, mu_elderly : rate
}

vital_dynamics {
  aging {
    dimension = age
    durations = [5 'years, 10 'years, 50 'years, ∞]
    method    = deterministic(stages = 20)
  }
  births    { target = S[u5]  rate = balance }
  mortality { dimension = age  rates = [mu_u5, mu_youth, mu_adult, mu_elderly] }
}

let N = sum(a in age, S[a] + E[a] + I[a] + R[a])
let lambda[a] = beta * sum(b in age, I[b]) / N

transitions {
  infect[a in age]   : S[a] --> E[a]   @ lambda[a] * S[a]
  progress[a in age] : E[a] --> I[a]   @ sigma * E[a]
  recover[a in age]  : I[a] --> R[a]   @ gamma * I[a]
}

init  { S[u5] = 500  S[youth] = 1000  S[adult] = 3000  S[elderly] = 500  I[adult] = 10 }
simulate { from = 0 'days  to = 30 'years }
```

**23 content lines** for a 4-age SEIR with full demographic replacement.

---

## Testing discipline

1. **Cohort-clock CV test**: simulate a closed cohort (no births, no
   disease), verify that the residence-time CV in each age class
   matches the `stages` parameter analytically (`CV = 1/√k`).
2. **Population-conservation test**: `rate = balance` form holds
   `N_total` constant to ≤ 1e-3 relative drift over a 100-year
   simulation.
3. **Exponential-aging regression**: explicit hand-written exp-aging
   transitions produce byte-identical trajectories to
   `method = exponential` under the same seed.
4. **Dimension-subset test**: vector compartments without the `age`
   dimension receive no aging/mortality/birth transitions (regression
   guard against "vital dynamics leaked onto `Iv`").

---

## Sequencing

This proposal is orthogonal to the malaria feature set. It lands in
parallel with no dependencies on or from features #1–#5 of
`2026-04-21-malaria-model-features.md`. For models that need both
(multi-age Garki with full demography), both proposals' features
compose without interaction — demographic transitions fire every
substep like any other transition; disease transitions see the
resulting state.

Recommended order: land this *after* the core malaria features (#1,
#2, #4) so that the demographic test-bed can use the bimolecular and
branching shapes it needs. The 2-week budget slots cleanly between
the malaria phase-1 and phase-2 blocks.
