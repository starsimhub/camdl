---
status: implemented
date: 2026-04-04
implemented: "commit c43cc8a, 2026-04-04"
note: DSL primitive for scheduled discrete state modifications (cohort pulses, importation).
---

# Proposal: `events {}` block and action type taxonomy
**Motivation:** The He et al. 2000-nat loglik gap was caused by a
cohort pulse implemented as a rate spike via `mod(t, 365.25)` arithmetic
in a transition. The DSL lacked a primitive for scheduled discrete state
modifications, forcing modelers to express amounts as momentary rates —
a dimensional hack that silently drifted by 0.25 days/year.

## The problem

The cohort birth pulse injects ~20,000 susceptibles into S once per
year. This is an **amount** (people), not a **rate** (people/time). But
the only way to express it in the current DSL is as a transition rate:

```camdl
# BAD: amount disguised as a rate. The * 365.25 is a dimensional hack.
birth : --> S @ deterministic(
  (1.0 - cohort) * daily_births
  + is_cohort_day * cohort * daily_births * 365.25
)
```

The `* 365.25` converts a stock change into a flow rate. The
`is_cohort_day` flag uses `mod(t, 365.25)` with a comparison window.
With integer `dt = 1`, the non-integer period causes the window to
capture two timesteps in 75% of years, doubling the injection. This
single bug produced a 2000-nat loglik gap vs pomp.

## State modification taxonomy

Six mechanisms modify model state. Each has different semantics for
when it fires, what it changes, and how it's activated.

| Block | Modeler writes | Engine decides | Timing | Activation | Stochastic? |
|-------|---------------|----------------|--------|------------|-------------|
| `transitions {}` | rate (per time) | count (draws) | every substep | always | yes |
| `forcing {}` | time function | rate value at t | continuous | always | no |
| `balance {}` | constraint expr | compartment value | every substep | always | no |
| `events {}` | amount (count) | nothing — executes exactly | scheduled | always | no |
| `interventions {}` | amount (count) | nothing — executes exactly | scheduled | scenario-toggled | no |
| `ode {}` | derivative | state via integration | continuous | always | no |

`events {}` and `interventions {}` share the same action grammar and
scheduling syntax. The only difference is activation: events are always
active (structural), interventions are scenario-toggled (policy).

## Decision rule for modelers

- **Does this happen continuously (every substep)?** Use `transitions {}`.
  Infection, recovery, birth, death, aging, migration.
- **Does this happen at specific scheduled times?** Use `events {}` or
  `interventions {}`.
  - **Is it always part of the model?** Use `events {}`. Cohort entry,
    seasonal migration, importation seeding.
  - **Is it a policy choice to compare against a counterfactual?** Use
    `interventions {}`. SIA campaign, school closure, travel ban.

The key question is **every substep vs. scheduled**, not rate vs. amount.
Deterministic transitions (`deterministic(expr)`) are per-substep
operations where the engine executes exactly what you write — but they
still run every substep, which is what makes them transitions.

**Signal that you're using the wrong primitive:** a `* 365.25` or
`* period` scaling factor in a rate expression, or a comparison chain
producing a 0/1 flag multiplied into a rate. These are scheduled amounts
disguised as per-substep rates — use `events {}` instead.

## Syntax

### Actions (shared by events and interventions)

```camdl
# Add people to a compartment (sourceless inflow or outflow)
add(COMPARTMENT, EXPR)

# Transfer between compartments
transfer(fraction = EXPR, from = COMP, to = COMP)
transfer(count = EXPR, from = COMP, to = COMP)

# Set a compartment to an exact value
set(COMPARTMENT, EXPR)
```

`add` accepts negative values (e.g., emigration, culling). If the
result makes the compartment negative, the engine warns but does not
crash — the particle gets a bad trajectory and is resampled away.

### Scheduling

```camdl
# At specific times
ACTION at [t1, t2, t3]

# Recurring with period and phase
ACTION every PERIOD at_day DAY

# Recurring with explicit start/end
ACTION every PERIOD at_day DAY from T0 to T1
```

The engine fires scheduled actions on the single timestep closest to
the target time, using `|t - target| < 0.5 * dt`. This guarantees
exactly one fire per period regardless of dt or fractional-day drift.

**`at_day` semantics:** `at_day` is an absolute phase within the
period, not relative to `start`. Fire times are `at_day + k * period`
for k = 0, 1, 2, .... The first fire is the smallest
`at_day + k * period >= t_start`. The `start` field controls when
the recurrence begins, not the phase.

Example: `every 365.25 'days at_day 251`, simulation starts at `t = 100`.
First fire at `t = 251` (not `t = 351`). Next at
`t = 251 + 365.25 = 616.25`, rounded to the nearest timestep.

The `at_day` scheduling works for both `events {}` and
`interventions {}`. SIA campaigns with per-period timing
(e.g., "day 180 of each year") use the same mechanism.

### Events block

```camdl
events {
  cohort_entry : add(S, cohort * birthrate(t) * pop(t))
    every 365.25 'days at_day 251

  importation : add(I, 10)
    at [30]
}
```

Events are always active. They cannot be disabled via scenarios.

### Indexed events (spatial/age-structured models)

Events support index variables, matching intervention syntax:

```camdl
events {
  sia_campaign[p in patch] : add(V[p], sia_coverage * S[p])
    at [sia_day[p]]
}
```

The expander generates one event per index value. The `at` expression
can reference index-dependent values from tables (e.g., per-patch
campaign timing).

### Interventions block (unchanged)

```camdl
interventions {
  sia_round_1 : transfer(fraction = vacc_frac, from = S, to = V)
    at [180, 545, 910]
}

scenarios {
  with_sia { enable = [sia_round_1] }
}
```

Interventions are inactive by default and toggled via scenarios.

## Balance block

`balance {}` remains a separate top-level block. It runs every substep
(not on a schedule) and always runs LAST — after transitions, events,
and interventions. Its semantics are unique: overwrite a compartment so
a constraint holds.

```camdl
balance {
  R = pop(t) - S - E - I
}
```

**Balance + events interaction:** Events can change the total population
within a step (e.g., `add(S, 20000)` creates people from nowhere).
Balance restores the constraint by adjusting R. Without balance, the
total drifts. Models with sourceless `add` events typically need a
balance block to maintain demographic consistency. This matches pomp's
`R = nearbyint(pop) - S - E - I` which absorbs the cohort injection.

## Execution semantics

### Step ordering

Within each simulation substep:

1. **Snapshot** start-of-step state (frozen copy for expression evaluation)
2. **Evaluate propensities** from the snapshot
3. **Draw transition counts** (multinomial for source groups, Poisson/deterministic for inflows)
4. **Apply all transition deltas** atomically to the live state
5. **Clamp** non-negative (skip balance target)
6. **Apply events and interventions** scheduled for this timestep
7. **Apply balance** constraint

### Expression evaluation context

Event and intervention action expressions (`add(S, EXPR)`) are
evaluated using **start-of-step compartment state** (the snapshot from
step 1) but **current-time covariates** (t = t_end of this substep).
This means:

- `add(S, S * 0.1)` — S comes from the snapshot (pre-transition value)
- `add(S, pop(t))` — pop(t) evaluates at end-of-step time
- `add(S, cohort * birthrate(t) * pop(t))` — cohort is a parameter
  (constant), birthrate and pop are covariates at current time, no
  compartment references

This matches pomp, where the cohort birth count is computed at the
current step time but from start-of-step state.

The snapshot is already captured for propensity evaluation. Events
reuse it.

### Multiple events at the same timestep

When multiple events (or events and interventions) fire on the same
timestep, they execute in **declaration order**. The modeler controls
ordering by ordering the block.

**Expressions** are evaluated from the start-of-step snapshot.
**Actions** are applied to the live state sequentially. This means:

- If two events both reference S in their expressions, they both see
  the start-of-step value (from the snapshot), regardless of what
  earlier events did to S.
- But the state modifications (add, set, transfer) apply to the live
  state in order — so `add(S, 100)` then `set(S, 0)` gives S = 0,
  while `set(S, 0)` then `add(S, 100)` gives S = 100.

Example: snapshot S = 500. Event 1: `add(S, 100)` — evaluates from
snapshot, applies to live state: S = 600. Event 2: `add(S, S * 0.1)`
— evaluates `S * 0.1` from snapshot (= 50), applies: S = 650. Not 660.

Balance always runs last (step 7), after all events and interventions.

## He et al. model after this proposal

```camdl
transitions {
  infection : S --> E  @ overdispersed(beta * seas * S * ((I + iota) ^ alpha) / pop(t), sigma_se)
  latency   : E --> I  @ sigma * E
  recovery  : I --> R  @ gamma * I

  # Continuous births only — no cohort pulse hack
  birth   : --> S  @ deterministic((1.0 - cohort) * birthrate(t) * pop(t) / 365.25)
  death_S : S -->  @ mu * S
  death_E : E -->  @ mu * E
  death_I : I -->  @ mu * I
  death_R : R -->  @ mu * R
}

events {
  # Cohort: children enter school once per year on day 251 (September)
  cohort_entry : add(S, cohort * birthrate(t) * pop(t))
    every 365.25 'days at_day 251
}

balance {
  R = pop(t) - S - E - I
}
```

No `mod()`. No `is_cohort_day`. No `* 365.25` magnitude hack. The
cohort is what it is: a scheduled addition of people to S. The engine
handles the timing. The 2000-nat bug class is eliminated.

## IR changes

### New action type

```json
{
  "add": {
    "compartment": "S",
    "count": { "bin_op": { "op": "mul", "left": ..., "right": ... } }
  }
}
```

Added to the existing `Action` enum alongside `fraction_transfer`,
`absolute_transfer`, and `set`.

### Events as always-active interventions

In the IR, events and interventions share the same struct (currently
`Intervention`, to be renamed `ScheduledAction` or kept as-is). Events
are distinguished by a new field:

```json
{
  "name": "cohort_entry",
  "always_active": true,
  "schedule": { "recurring": { "start": 0, "period": 365.25, "end": 7672, "at_day": 251.0 } },
  "actions": [{ "add": { "compartment": "S", "count": ... } }]
}
```

The `always_active` field (default `false` for backward compatibility)
means the event fires regardless of scenario enable/disable settings.
The runtime processes both identically — the only difference is whether
the scenario system can toggle them.

### Recurring schedule at_day

The existing `RecurringSchedule { start, period, end }` is extended
with an optional `at_day` field:

```json
{
  "recurring": {
    "start": 0,
    "period": 365.25,
    "end": 7672,
    "at_day": 251.0
  }
}
```

Fire times: `target_k = at_day + k * period` for k = 0, 1, 2, ....
The first fire is the smallest `target_k >= start`. The engine fires
at the unique timestep where `|t - target_k| < 0.5 * dt`. The `start`
field controls when the recurrence begins; `at_day` is the absolute
phase within each period.

## Runtime changes

### Action::Add

In `intervention.rs`, add to the `Action` enum:

```rust
Add(AddAction),
```

```rust
pub struct AddAction {
    pub compartment: String,
    pub count: Expr,
}
```

In `apply_intervention`, handle `Action::Add`:

```rust
Action::Add(aa) => {
    let n = eval_expr(&aa.count, &ctx)?.round() as i64;
    if n < 0 {
        log::warn!("event '{}' adding negative count ({}) to '{}'",
            iv.name, n, aa.compartment);
    }
    let global = *model.comp_index.get(aa.compartment.as_str())
        .ok_or_else(|| SimError::UnknownCompartment(aa.compartment.clone()))?;
    if let Some(local) = model.global_to_int[global] {
        int_s.counts[local] += n;
    }
}
```

Note: the `ctx` for expression evaluation uses the **start-of-step
snapshot**, not the post-transition state. The runtime must pass the
snapshot `EvalCtx` to event evaluation, not the live state.

### Event expression evaluation from snapshot

Currently, `apply_interventions_at` constructs its `EvalCtx` from the
live `int_s`/`real_s` passed to it. For events to evaluate from the
start-of-step snapshot, `step_one` must pass the snapshot separately:

```rust
// In step_one, after transitions but before events:
let snapshot_ctx = EvalCtx {
    model, int_s: &scratch.int_s, // start-of-step copy
    real_s: &scratch.real_s, params, t: t + dt, projected: None,
};
apply_events_at(t + dt, model, counts, &snapshot_ctx, dt * 0.5)?;
```

This is the only non-trivial runtime change.

## Compiler warnings

When the compiler detects a transition rate expression containing a
pulse-like pattern, warn:

```
warning: transition 'birth' rate contains a pulse pattern
  (is_cohort_day * ... * 365.25). This may be a discrete injection
  modeled as a rate spike.
  Consider using events { } for scheduled state modifications.
  See: docs/camdl-language-spec.md#events-vs-transitions
```

Detection heuristic: a multiplication chain where one factor is a
comparison-derived 0/1 flag and another is a large constant (> 100).

## Implementation plan

1. Add `Action::Add` to IR and runtime (~20 lines)
2. Add `always_active` field to `Intervention` IR struct
3. Modify event expression evaluation to use start-of-step snapshot
4. Add `events {}` block to DSL parser (reuse intervention grammar)
5. Add `at_day` to `RecurringSchedule`
6. Support indexed events (`[p in patch]`)
7. Update He et al. model to use `events {}`
8. Add compiler warning for pulse-pattern rate expressions (stretch)

Steps 1-2 are mechanical. Step 3 is the key semantic change. Steps 4-6
reuse existing infrastructure. Step 7 is validation. Step 8 is polish.

## Test plan

### IR / deserialization tests (Rust, `crates/ir/`)

**T1. Add action deserializes.** JSON with `"add": {"compartment": "S", "count": ...}`
round-trips through serde. Verify field values.

**T2. always_active field defaults to false.** Existing IR JSON without
`always_active` deserializes with `always_active: false` (backward compat).

**T3. at_day field in RecurringSchedule.** JSON with and without `at_day`
both deserialize correctly. Without `at_day`, fire times are `start + k*period`
as before.

**T4. Golden IR round-trip.** All existing golden `.ir.json` files still
deserialize and re-serialize identically (no breakage from new optional fields).

### Compiled model tests (Rust, `crates/sim/`)

**T5. Add action applies correctly.** Build a minimal 2-compartment model
with one event `add(S, 100) at [10]`. Run for 20 steps. Verify S increases
by 100 at t=10 and only at t=10.

**T6. Negative add warns but doesn't crash.** Event `add(S, -50) at [5]`
with S=30 at t=5. Verify S=-20 after the event (no clamp), and that
`log::warn!` was emitted. The simulation continues.

**T7. always_active events fire without scenario enable.** Model with
one always_active event and one non-always_active intervention at the
same time. Run without enabling the intervention. Verify: event fires,
intervention does not.

**T8. Recurring schedule with at_day fires exactly once per period.**
Event with `period=10, at_day=3`. Run for 50 steps (dt=1). Verify fires
at t=3, 13, 23, 33, 43. Exactly 5 fires. No double-fires.

**T9. Recurring at_day with fractional period fires exactly once.**
Event with `period=7.5, at_day=2`. Run for 30 steps (dt=1). Verify
fires at t=2, 9 or 10 (nearest to 9.5), 17, 24 or 25 (nearest to 24.5).
Exactly 4 fires. This is the test that catches the `mod(t, 365.25)` bug
class — fractional period with integer dt.

**T10. Multiple events same timestep: declaration order.** Two events
at t=5: first `add(S, 100)`, second `set(S, 0)`. After t=5, S=0 (set
overwrites the add). Reverse the declaration order: S=100 (add after set).
Confirms declaration-order execution.

### Snapshot evaluation tests (Rust, `crates/sim/`)

**T11. Event expression evaluates from start-of-step state.** Model:
S=1000, one transition `S --> E @ 0.1 * S` (removes ~100 from S per step),
one event `add(E, S) at [1]` (add current S to E). After step 1:
transition removes ~100 from S (S≈900), then event adds start-of-step
S=1000 to E (not post-transition S≈900). Verify E receives 1000, not 900.

This is the critical semantic test. If it fails, the event saw
post-transition state instead of the snapshot.

**T12. Multiple events all see the same snapshot.** Two events at t=5:
`add(S, -I)` then `add(E, I)`. Starting state: S=1000, E=0, I=50. Both
should see I=50 from the snapshot, regardless of what the first event did
to S. After t=5: S=950, E=50. Not S=950, E=some-other-value.

### Balance interaction tests (Rust, `crates/sim/`)

**T13. Balance runs after events.** Model with S+E+I+R=1000, balance
`R = 1000 - S - E - I`, and event `add(S, 200) at [5]`. At t=5: event
adds 200 to S, then balance sets R = 1000 - (S+200) - E - I. Total
population stays 1000. R decreases by 200.

**T14. Balance runs after interventions too.** Same as T13 but with an
intervention instead of an event (scenario-enabled). Same result.

### OCaml compiler tests

**T15. events block parses.** `events { foo : add(S, 100) at [10] }`
compiles to IR with `always_active: true` and `Action::Add`.

**T16. interventions block still works.** Existing intervention syntax
compiles unchanged. `always_active` defaults to false.

**T17. Indexed events expand correctly.**
`events { e[p in patch] : add(S[p], 100) at [10] }` with 3 patches
produces 3 events in the IR, each targeting S_patch_1, S_patch_2, etc.

**T18. at_day in recurring schedule.**
`events { e : add(S, 100) every 365 'days at_day 251 }` compiles to
RecurringSchedule with period=365, at_day=251.

**T19. Golden IR round-trip unchanged.** All existing golden `.camdl`
files compile to the same `.ir.json` as before (no `events` block = no
change). Run `make update-golden` and verify no diff.

### Integration / end-to-end tests

**T20. He et al. cohort as event matches pomp.** The definitive test.
Rewrite the He et al. model to use `events { cohort_entry: add(S, ...) }`.
Run pfilter at MLE with 5000 particles, 10 seeds. Compare against pomp's
loglik. Gap should be < 20 nats (the cohort-disabled baseline was 5 nats;
20 allows headroom for MC noise but catches double-fires which produce
~2000 nats). Also verify fire count = exactly 21 (combine with T23).

**T21. Existing intervention models unchanged.** Run `seir_vaccine.camdl`
and `polio_spatial_5.camdl` through the full compile+simulate pipeline.
Verify output matches pre-change golden expected values.

**T22. Event + pfilter interaction.** Run pfilter on a model with an
event. Verify that all particles receive the event at the same timestep,
that flow accumulators are not affected by the event (only by transitions),
and that the event doesn't corrupt particle RNG streams.

**T23. Event firing count diagnostic.** Run a 21-year simulation with
a yearly cohort event. Count how many times the event fired. Should be
exactly 21 (±0 for `at_day` schedule, regardless of fractional period).

### Regression tests (prevent the original bug)

**T24. mod(t, non-integer-period) with pulse pattern warns.** Compile
a model with `let flag = (mod(t, 365.25) > 250) * (mod(t, 365.25) < 252)`
used in a rate expression multiplied by a constant > 100. Verify the
compiler emits a warning about pulse patterns.

**T25. Fractional-period double-fire regression.** Model with
`add(S, 1) every 365.25 'days at_day 251`, dt=1, run for 21 years.
Count fires. Must be exactly 21. If the engine used the old
`mod(t, 365.25)` approach internally, this would produce 24-27 fires.

### IF2 interaction tests

**T26. Event with per-particle parameters in IF2.** Model with event
`add(S, param_a)` where `param_a` is an estimated parameter. Run IF2
with 2 particles that have different `param_a` values (e.g., 100 and
200). Verify each particle's S increases by its own `param_a`, not by
the base value. This catches the most dangerous IF2 integration bug —
events that accidentally use base params instead of particle params.

### Scheduling edge cases

**T27. Simulation starts mid-period.** Event with `every 365.25 'days
at_day 251`, simulation starts at `t = 300`. First fire should be at
`t ≈ 616` (day 251 of the second period), NOT at `t ≈ 551` (300 + 251).
The `at_day` is an absolute phase, not relative to start.

**T28. at_day works for interventions too.** Intervention with
`every 30 'days at_day 15`, enabled via scenario. Verify fires at
t=15, 45, 75, .... Same `at_day` scheduling as events.

### Test matrix summary

| Category | Tests | Key property verified |
|----------|-------|----------------------|
| IR/serde | T1-T4 | Backward compatibility, new fields |
| Core actions | T5-T10 | Add, negative add, always_active, at_day, ordering |
| Snapshot semantics | T11-T12 | Start-of-step evaluation (THE critical semantic) |
| Balance interaction | T13-T14 | Events → balance ordering |
| OCaml compiler | T15-T19 | Parsing, expansion, golden stability |
| Integration | T20-T23 | He et al. match, pfilter, fire count |
| Regression | T24-T25 | Prevent the original bug class |
| IF2 interaction | T26 | Per-particle params in events |
| Scheduling edges | T27-T28 | Mid-period start, at_day for interventions |
