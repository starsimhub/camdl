# Proposal: reactive interventions + EVSI

**Status:** Under consideration — **hold**
**Date:** 2026-05-14
**Motivation:** Add a `trigger` construct for reactive (state-conditional)
vaccination campaigns and use it as the missing piece for a native
`camdl evsi` workflow over PGAS posteriors. Held because the trigger
construct touches inference math (PGAS complete-data density,
trajectory record format) and warrants a careful, post-alpha rollout.

---

## Why

Two related capabilities, neither currently in camdl, both common in
public-health policy work:

**Reactive vaccination.** Real outbreak responses fire when observed
incidence crosses a threshold — ring vaccination in Ebola, ORV in
cholera, polio cVDPV2 SIA after AFP case detection, measles SIA when
reported cases exceed a regional threshold. Today camdl's
`interventions {}` and `events {}` blocks support fixed schedules
(`at_times`, `every`) but not state-conditional triggers. Production
configs (typhoid SIRC, polio cVDPV2) work around this by hand-crafting
SIA event schedules from historical surveillance data — fine for
retrospective fits, useless for prospective scenario analysis ("what if
we'd responded earlier?").

**EVSI.** Expected Value of Sample Information answers "what's the
value of additional surveillance data we could collect, given that we'd
act on it?" Without reactive interventions, every counterfactual
scenario uses a fixed schedule regardless of what the simulated data
turn out to be — you can compute EVPI (perfect parameter information)
and EVPPI (partial perfect information), but EVSI specifically requires
simulating "the policy as a function of observed data," which is what
trigger gives you. EVSI in epidemiology is mostly done today in
deterministic Markov cohort models (TreeAge, Excel, R `voi`) or via
bespoke Python/R glue; a camdl-native workflow over stochastic
compartmental dynamics with full PGAS posteriors would be unusual.

Neither is fundamentally blocked by the math. Stochastic compartmental
models support state-conditional dynamics naturally — the next-step
transition kernel just becomes a piecewise function of the current
state, and the Markov property holds. The work is engineering.

---

## Decision: why hold

The trigger construct touches the inference hot path:

- **Chain-binomial substep loop** (`chain_binomial.rs`) needs to
  evaluate trigger predicates per substep and apply the intervention
  mid-step.
- **PGAS complete-data density** (`pgas.rs`, `pgas_grad.rs`) needs to
  factor reactive intervention transitions into the substep density
  product. Per CLAUDE.md these files are flagged as high-risk; the
  audit-remediation work just landed C1, H3, H8 fixes there.
- **Trajectory record format** (`state.rs`, `pgas.rs`) needs a new
  per-substep field for "interventions fired this substep," touching
  every test that reads trajectories.
- **IR schema bump** (0.4 → 0.5) regenerates every golden — same shape
  of churn as the just-finished C8 envelope work.

Each of these is doable, but stacking them on top of fresh
audit-remediation changes risks compounding interactions that are hard
to bisect. Better to:

1. Let the audit-remediation work bake in production (camdl-book agent
   validates typhoid + polio configs, surfaces any C5/C6 strict-mode
   issues, exercises the C8 envelope handshake against the book's
   vignettes).
2. Ship the alpha so the public surface is anchored.
3. Then design + implement reactive interventions on a clean baseline
   with deliberate care around the PGAS density factoring.

EVSI itself can land mostly without trigger — EVPI and EVPPI work over
existing posteriors with existing forward-sim. The full EVSI workflow
is the second half of this proposal and is bottlenecked on trigger.

Revisit when: alpha has shipped, camdl-book vignettes are stable on the
new strict-mode runtime, and there's a concrete decision-analysis
use case asking for it. The natural pull is the polio cVDPV2 surveillance
chapter in camdl-book — when that chapter wants to ask "should we have
triggered earlier?", trigger becomes load-bearing.

---

## Part I — Reactive interventions

### DSL surface

A new top-level block paralleling `interventions {}` and `events {}`:

```camdl
reactive_interventions {
  ring_sia : trigger(when = weekly_cases > 50, after = 14 'days)
             action  = transfer(fraction = 0.7, from = S, to = V_ring)
             once    = true

  expanded_sia : trigger(when = cumulative(infection) > 1000)
                 action  = transfer(fraction = 0.9, from = S, to = V)
                 once    = true
}
```

Fields:

- **`when`** — boolean expression over current state. Reuses the
  existing `Expr` language; new primitives needed:
  `cumulative(transition_or_event)`, `incidence(transition, window)`.
  The `>`, `<`, `>=`, `<=`, `==`, `!=`, `&&`, `||` operators are
  already in the IR via comparison `BinOp` variants.
- **`after`** — implementation lag from trigger to action firing.
  Optional; defaults to 0.
- **`action`** — same set as existing interventions: `transfer`,
  `add`, `set`, `scale`. Reuses existing `Action` machinery in
  `intervention.rs`.
- **`once`** — fire-and-disable vs fire-every-time-condition-true.
  Default `true` (one-shot) since real reactive campaigns are
  one-shot SIAs. `once = false` is "fire every step where condition
  holds" — useful for ongoing reactive measures (e.g., contact
  tracing intensity scaling with case load).

### Triggering on observed (not true) state

Real reactive campaigns trigger on *observed* incidence with reporting
noise and lag, not the latent compartment count. Two paths:

**A. Accumulator compartment** (works today, no new construct).
Add an `observed_cases` real compartment with an ODE that integrates
the observation model's mean. `weekly_cases` becomes a windowed
accumulator. `trigger(when = observed_cases > 50)` reads the
deterministic accumulator. Underestimates uncertainty (no obs noise
in the trigger), but tractable.

**B. New `observed(stream_name)` primitive** (clean but more work).
A new `Expr` variant that returns the most recent observed value of an
observation stream (drawn from the obs model with its noise). Requires
the runtime to thread the observation history into `EvalCtx`, and
PGAS to factor in the joint distribution over latent + observed +
trigger.

Path A first; path B if real cases need it.

### IR shape

```rust
// rust/crates/ir/src/reactive.rs (new)
pub struct ReactiveIntervention {
    pub name: String,
    pub when: Expr,                       // boolean, dim-checked at compile time
    pub after: Option<f64>,               // lag in time_unit; None = 0
    pub action: Action,                   // reuses ir::intervention::Action
    pub once: bool,                       // default true
    pub source_loc: Option<SourceLoc>,    // for diagnostics
}

// in Model:
pub reactive_interventions: Vec<ReactiveIntervention>,
```

### Runtime mechanics

**Per-substep loop** (`chain_binomial.rs`):

1. Evaluate the substep's stochastic transitions as today.
2. Apply scheduled interventions / events at substep boundary as today.
3. **New:** evaluate each `reactive_interventions[i].when` predicate.
   For each that fires:
   - If `once = true` and already fired in this trajectory → skip.
   - Else if `after > 0` → push to a deferred-action queue keyed on
     fire-time `t + after`.
   - Else → apply action immediately (in this substep, after dynamics).
4. Process any deferred actions whose fire-time is in `(t, t + dt]`.
5. Record fired actions in the substep trajectory record.

Per-particle tracking:

```rust
pub struct ReactiveState {
    /// Per-reactive-intervention: has it fired in this trajectory yet?
    /// (only relevant when once = true)
    pub fired: Vec<bool>,
    /// Pending actions: (fire_time, intervention_idx).
    pub deferred: Vec<(f64, usize)>,
}
```

This is per-particle in PGAS / PF — different particles can have
different reactive-firing histories, which is the whole point.

### PGAS complete-data density

The hard part. PGAS computes:

```
log p(X | θ) = Σ_substeps log p(flows_s | x_{s-1}, θ)
              + Σ_observations log p(y_t | x_t, θ)
```

When a reactive intervention fires inside substep `s`, the substep
density factors:

```
log p(flows_s, intervention_fired_s | x_{s-1}, θ)
  = log p(flows_part1 | x_{s-1}, θ)              # stochastic, t to t+δ
  + log P(intervention.fires at t+δ | x_{t+δ})   # deterministic given state
  + log p(flows_part2 | x_intervened, θ)         # stochastic, t+δ to t+dt
```

The middle term is `0` (deterministic firing — δ-function with mass 1)
when `when` is a function of latent state alone. With observed-state
triggers (path B), it becomes the observation density at `t+δ`
restricted to the threshold-crossing region — non-trivial.

**Risk concentration:** this density factoring is the single most
delicate part of the proposal. PGAS depends on the density being
correct to ULP for the gradient identity to hold; getting the
intervention-firing density wrong silently biases the posterior. The
existing complete-data-density tests (`spatial_density.rs::
test_density_matches_step_one_*`) need extension to cover reactive
firings.

### Trajectory record

```rust
// state.rs additions
pub struct SubstepRecord {
    // existing fields...
    pub counts_before: Vec<i64>,
    pub counts_after:  Vec<i64>,
    pub flows:         Vec<u64>,
    pub gammas:        Vec<f64>,

    /// gh#future-trigger: which reactive interventions fired this
    /// substep, and any deferred actions executed this substep.
    pub reactive_fired:    Vec<usize>,           // by intervention idx
    pub reactive_deferred: Vec<(f64, usize)>,    // (fire_time, idx)
}
```

Touches: serialization, golden trajectory diffs, `pgas_resume`,
`csmc_as`, `apply_interventions_at`. Mechanical but fan-out is
significant.

### Backend support matrix

| Backend         | Initial support | Notes                                    |
| --------------- | --------------- | ---------------------------------------- |
| chain-binomial  | Yes             | Substep loop is the natural shape.       |
| tau-leap        | Yes             | Same substep shape.                      |
| Gillespie       | Deferred        | No notion of substep; needs design — fire at next event time? Insert synthetic events? |
| ODE             | Deferred        | Continuous-time triggers via root-finding (event detection in RK4). Standard ODE-event-handling work. |

Declare a `REACTIVE_INTERVENTIONS` capability; chain-binomial and
tau-leap declare it; Gillespie/ODE refuse models that use it (until
their support lands).

### DSL examples

**Polio cVDPV2 SIA on AFP case detection:**

```camdl
# Observe AFP cases via existing observation model
observations {
  weekly_afp : { projected = incidence(paralysis), every = 7 'days,
                 likelihood = neg_binomial(mean = rho * projected, r = k) }
}

# Accumulator for the trigger (path A)
let recent_afp = window(observed(weekly_afp), span = 28 'days)

reactive_interventions {
  ring_sia : trigger(when = recent_afp > 5, after = 21 'days)
             action  = transfer(fraction = 0.7, from = S, to = V)
             once    = true
}
```

**Cholera ORV with logistical cap:**

```camdl
parameters {
  daily_dose_capacity : count = 10000
  trigger_threshold   : count = 100
}

reactive_interventions {
  orv : trigger(when = weekly_cholera_cases > trigger_threshold)
        action  = transfer(fraction = min(daily_dose_capacity / S, 0.95),
                           from = S, to = V)
        once    = false   # ongoing as long as cases stay above threshold
}
```

### Implementation phases

| Phase | Scope | LOC est | Risk |
|---|---|---|---|
| 1. IR + parser | Add `reactive_interventions` block, AST, dim-check, IR types both sides, IR_VERSION 0.4 → 0.5, regenerate goldens | ~250 | Low (additive) |
| 2. Chain-binomial runtime | Per-substep evaluation, deferred queue, per-particle ReactiveState, trajectory record extension | ~200 | Medium (touches state.rs + every backend test) |
| 3. PGAS density | Factor reactive transitions into complete_data_loglik; extend density-vs-step_one tests | ~150 | **HIGH** (inference math) |
| 4. Tau-leap support | Mirror phase 2 in tau_leap.rs | ~80 | Low |
| 5. Capability gate | Gillespie/ODE refuse `REACTIVE_INTERVENTIONS`-requiring models | ~30 | Low |
| 6. Path B (`observed()` primitive) | New Expr variant, runtime threading, density factoring | ~200 | Medium-High |

Total: ~900 LOC, with phase 3 being the load-bearing risk.

---

## Part II — EVSI

Builds directly on Part I. The math:

```
EVSI(D) = E_θ[ E_D|θ[ U(δ*(θ, D), θ) ] ]  −  E_θ[ U(δ*(θ), θ) ]
              └─────────┬───────────┘          └────────┬────────┘
              expected utility under              expected utility
              optimal data-informed policy        under fixed policy
```

Each `δ*(θ, D)` requires simulating the policy *as a function of the
observation stream D*. With trigger, that's a forward sim; without it,
the inner expectation collapses.

### DSL: outcome block

EVSI requires a scalar metric per simulated trajectory. Today camdl
has expression evaluation over compartments and time; what's missing
is an `outcome { metric = expr }` block that aggregates over the full
trajectory:

```camdl
outcome {
  averted_infections = baseline_cumulative_I - cumulative(infection)
  vaccine_doses      = cumulative(action.ring_sia.transfers)
  net_benefit        = wtp_per_infection_averted * averted_infections
                     - cost_per_dose * vaccine_doses
}
```

New aggregator primitives over trajectories:
`cumulative(transition_or_event)`, `peak(compartment)`,
`time_to_peak(compartment)`, `area_under(compartment)`. Each is a
thin wrapper over the trajectory record. Pure post-processing — no
runtime hot-path impact.

### CLI: camdl evsi

```bash
# Posterior already exists from a prior fit
camdl fit run fit.toml          # → fits/<hash>/pgas/

# Define candidate data + policy + outcome in evsi.toml
camdl evsi run evsi.toml --posterior fits/<hash>/pgas/
```

```toml
# evsi.toml
[posterior]
fit = "fits/he2010-abc1234/pgas"

[candidate_data]
stream      = "weekly_cases_arm_C"   # an obs stream defined in the model
horizon     = 26                     # weeks of new data
n_outer     = 200                    # outer-MC draws of D|θ
n_posterior = 500                    # posterior θ samples

[policy.reactive_sia]
# Reactive intervention defined in the model file (Part I).
# Either zero-parameter or with free knobs to optimize.
trigger      = "weekly_cases_arm_C > threshold"
threshold    = { optimize = "outcome", grid = [10, 20, 50, 100] }
implementation_lag = "14 days"

[outcome]
maximize = "averted_infections - cost_per_dose * vaccine_doses"

[output]
write = "evsi_results.tsv"
emit  = ["evsi", "evppi", "evpi", "policy_optimum", "value_of_perfect_data"]
```

### Algorithm: Strong et al. (2015) regression-based EVSI

The canonical nested-MC formulation requires re-running PGAS for each
inner sample, which is intractable. Strong et al. (2015) showed you can
skip the inner posterior update by:

1. Simulate many `(θ, D, outcome)` tuples by sampling θ from the
   posterior, simulating future data D | θ, and computing the outcome
   under each candidate policy.
2. Regress outcome on D (any flexible regressor — GAMs, splines, RFs).
3. Estimate `var_D[E[outcome | D]]` from the fitted regressor; this is
   the EVSI up to a known additive constant.

Computationally: 500 outer × 200 inner = 100K trajectories, embarrassingly
parallel over `(θ, D)` pairs. Reuses the existing `camdl batch`
infrastructure for the simulation phase. The regression step is pure
post-processing in Rust (or shell out to R for stress-testing against
the `voi` package).

Two alternatives, mentioned for completeness:

- **Heath et al. (2020) moment-matching EVSI** — middle ground;
  treats EVSI as a function of data precision. Faster than regression
  for some structured cases.
- **Brute-force nested MCMC** — re-run PGAS for each `(θ, D)` pair.
  Honest but ~10⁵ chains. Only feasible for small models; useful as a
  validation oracle for the regression approach.

Strong et al. is the right default; expose Heath as `--method moment`
and brute-force as `--method nested-mcmc` for validation.

### Outcome utility surface

EVSI requires a scalar. Health-economic conventions:

- **Averted cases / hospitalizations / deaths** — natural-units.
- **QALYs / DALYs averted** — standard HE outcomes; need
  per-compartment-per-time disutility weights from the user.
- **Net monetary benefit** — `WTP × QALY − cost`. Standard for HTA.
- **Cost per case averted (ICER)** — ratio rather than scalar; not
  directly EVSI-compatible, but exposable as a derived output.

The `outcome { metric = expr }` block takes any expression over
trajectory aggregators + parameters, so users compose whichever they
need. No need to bake HE conventions into camdl's IR.

### Implementation phases

| Phase | Scope | LOC est | Risk |
|---|---|---|---|
| 1. Outcome block | IR field, parser, aggregator primitives (`cumulative`, `peak`, etc.), trajectory post-processor | ~200 | Low |
| 2. EVPI / EVPPI subcommand | Posterior consumer, simulate counterfactuals across draws, variance decomposition. Doesn't need trigger | ~150 | Low |
| 3. EVSI subcommand (regression) | Outer/inner MC orchestration, Strong et al. regression, optional R `voi` validation | ~250 | Medium (statistical correctness) |
| 4. Heath moment-matching | Alternative method | ~150 | Medium |
| 5. Vignette | Worked cVDPV2 example: surveillance frequency vs averted cases under various policy thresholds | ~doc only | Low |

Total: ~750 LOC + a vignette.

EVSI without trigger (Phases 1+2 only) lands EVPI / EVPPI on existing
PGAS posteriors. That's a meaningful capability on its own and worth
considering as a Phase-1.5 deliverable independent of the trigger
decision.

---

## Risk register

What this proposal could break:

1. **PGAS posterior validity.** Phase 3 of Part I (complete-data
   density factoring) is where silent bias would enter. Mitigation:
   extend `spatial_density.rs::test_density_matches_step_one_*` to
   exercise reactive firings; add a gradient-check denial test
   (`gradient_check.rs::reactive_intervention_gradient`) that
   compares analytical vs finite-difference gradients on a model with
   reactive firings.

2. **Trajectory format churn.** Adding `reactive_fired` to
   `SubstepRecord` touches every test that reads trajectories
   (~30 files). Mitigation: land the field as `#[serde(default)]`
   with `Vec::new()` so old golden trajectories deserialize without
   regeneration; only the new tests assert non-empty `reactive_fired`.

3. **IR schema 0.5 bump.** Same shape of churn as the just-finished
   0.3 → 0.4 envelope work. Mitigation: bundle with any other queued
   IR changes (H6 `param_kind` enum, M14 init validation) so we pay
   the regeneration cost once.

4. **Backend asymmetry.** Chain-binomial + tau-leap support reactive;
   Gillespie + ODE don't (initially). Mitigation: capability gate so
   the user gets a clean error with a hint to switch backends, rather
   than a silent drop. Same pattern as the C3 `BALANCE` capability fix
   from this audit cycle.

5. **Inference + reactive interactions.** Reactive interventions
   make the latent trajectory's distribution depend on the observed
   data history (under path B observed-state triggers). This
   complicates the PGAS Markov assumption. Mitigation: ship path A
   first (latent-state triggers only), validate against synthetic
   recovery, defer path B until the simpler case is proven.

6. **Convergence-gate sensitivity.** The Richardson dt-convergence
   check evaluates the loglik on (dt, dt/2, dt/4). Reactive triggers
   that fire based on accumulators integrating over time will
   produce slightly different fire-times under different dt — this is
   a real numerical issue (the trigger threshold-crossing time is
   rate-of-state-change-dependent), and the dt-check will surface it.
   Probably a feature rather than a bug — surfaces models whose
   reactive policy is sensitive to integrator step.

7. **Camdl-book vignette breakage.** Existing vignettes that use
   hand-crafted SIA event schedules continue to work unchanged. New
   reactive vignettes (cVDPV2 surveillance, ORV ring) are additive.
   No regression risk for the book on Part I; Part II's `camdl evsi`
   is entirely new surface.

---

## What's NOT in scope

- **Continuous-time event detection in ODE backend.** Standard
  ODE-event-handling (root-finding in RK4) is its own design;
  defer. ODE refuses reactive models until that lands.
- **Optimal stopping / dynamic programming over trigger thresholds.**
  We optimize via grid search over candidate thresholds; full DP
  policy optimization is a separate project (and probably out of scope
  for camdl entirely — refer users to dedicated DP / RL tools).
- **Multi-arm reactive trials.** Modelling the surveillance policy as
  an A/B-tested intervention itself (rather than a fixed policy
  evaluated for value) is a research direction, not a Phase-1
  feature.
- **Reactive interventions in PMMH.** Phase 1 focuses on PGAS because
  PGAS is the production Bayesian method per CLAUDE.md. PMMH support
  is a follow-up if anyone reaches for it.

---

## Decision

**Hold.** Revisit when:

1. Audit-remediation work has baked in production for ~2 weeks, with
   the camdl-book agent's typhoid + polio config validation complete.
2. Alpha has shipped, anchoring the public surface.
3. A concrete decision-analysis use case — most naturally the polio
   cVDPV2 surveillance chapter in camdl-book — asks for it. The pull
   from a real chapter writeup is what should trigger this work
   (pun intended).

In the meantime, EVPI / EVPPI (Part II Phase 1+2) could land
independently as a self-contained `camdl evsi` subcommand that
operates on existing PGAS posteriors with fixed-policy
counterfactuals. Lower risk, immediate value, doesn't block on
trigger. Worth considering as a separate proposal if the use case
arises.
