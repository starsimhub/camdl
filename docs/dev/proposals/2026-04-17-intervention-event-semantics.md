---
status: proposal
date: 2026-04-17
---

# Intervention and Event Semantics for Simulation and Inference

## Motivation

The language spec (`camdl-language-spec.md:1550`, §14) is unambiguous:
`interventions {}` are **inactive by default** and must be enabled via
scenarios or CLI; `events {}` are **always active** and cannot be toggled
by scenarios. The two-block distinction exists precisely to separate
toggleable policy levers from always-on structural processes.

Today the code diverges from the spec in three places:

- `camdl simulate` with no scenario: correctly clears toggleable
  interventions — but `util.rs:448`'s `model.interventions.clear()`
  also nukes events, because events and interventions live in one `Vec`
  distinguished by a bool. Latent bug; not yet triggered because no
  golden uses an `events {}` block.
- `camdl pfilter` with no scenario: keeps all interventions active —
  violates spec.
- `camdl fit run`: has no scenario/enable support at all; all
  interventions from the `.camdl` fire during inference — violates spec.

Beyond fixing the spec violations, the user-facing question is **how a
modeler decides where a scheduled action belongs.** A model of polio
transmission with 20 historical SIA rounds is one natural place to look:
the rounds happened, the data was shaped by them, omitting them from
inference is wrong. Should the modeler (a) declare them in
`interventions {}` and remember to enable the right scenario in fit.toml,
or (b) declare them in `events {}` and pay zero config cost?

This proposal nails down the answer, fixes the latent bugs, and adds
the small UX affordances that make the distinction easy to use correctly.

## Design principle

The two blocks share one mechanism (same action syntax, same scheduling,
same indexing, same table-driven dates) and differ only in the default:

| Block          | Default      | Purpose                                        | Fit config needed? |
|----------------|--------------|------------------------------------------------|--------------------|
| `events {}`    | always on    | Things that happened / structural processes   | No                 |
| `interventions {}` | **off**  | Hypothetical policy levers for counterfactuals | Yes (scenario or `enable`) |

The user's choice of block encodes intent, not mechanics:

- "This happened" → `events {}`
- "What if this happened?" → `interventions {}`

This is ontological, not syntactic. Moving a scheduled action from one
block to the other is a one-line edit; it changes the default, not the
semantics of the action itself.

## Usage cases

### Case 1 — fitting under historical SIAs (the common case)

Polio transmission with 20 historical SIA rounds. Facts of the data-
generating process. Zero config in fit.toml:

```camdl
tables {
  sia_schedule : round × patch = read("data/historical_sias.tsv")
}

events {
  historical_sia[r in round, p in patch] :
    transfer(fraction = sia_cov[r], from = S[p], to = V[p])
    at [sia_schedule[r, p]]
}
```

```toml
# fit.toml — no scenario, no enable, nothing.
[fit]
model = "polio.camdl"
output_dir = "fits/01"
```

```bash
camdl fit run fit.toml    # events fire at every PF step
```

Startup diagnostic confirms:

```
events (20 declared, always active):
    historical_sia_r1_north   transfer 80% S→V at [t=120]
    historical_sia_r1_south   transfer 80% S→V at [t=125]
    ...
```

### Case 2 — counterfactual (with_sia vs no_sia)

Move to `interventions {}` + named scenarios:

```camdl
interventions {
  historical_sia[r in round, p in patch] : ... at [sia_schedule[r, p]]
}

scenarios {
  with_sia  { enable = [historical_sia] }   # one name → all 20 expanded
  no_sia    { }                              # counterfactual: none fire
}
```

```toml
[fit]
model    = "polio.camdl"
scenario = "with_sia"
```

```bash
camdl simulate polio.camdl --draws posterior.tsv --scenario no_sia
```

One-line move between blocks changes the default, not the mechanics.

### Case 3 — structural events plus hypothetical policy

```camdl
events {
  cohort_entry : add(S, cohort_size) every 365 'days from 0 to 10 'years
  births       : add(S, birth_rate * N) every 1 'days from 0 to 10 'years
}

interventions {
  proposed_sia : transfer(fraction = 0.8, from = S, to = V) at [730, 1095]
}

scenarios {
  baseline          { }                      # events on, intervention off
  with_proposed_sia { enable = [proposed_sia] }
}
```

Fitting the baseline needs no scenario; events fire automatically.

### Case 4 — disabling events

Rare but legitimate: "what if there were no births?" `--disable` applies
to events too, as the explicit override:

```bash
camdl simulate model.camdl --disable cohort_entry
```

Diagnostic:

```
events (2 declared, 1 active):
    ✓ births           every 1 days
    ✗ cohort_entry     (disabled via --disable)
```

## Implementation

All filter logic consolidates into a single helper so `simulate`,
`pfilter`, and `fit` can't drift again:

```rust
// rust/crates/cli/src/util.rs
pub fn apply_scenario_filter(
    model: &mut ir::Model,
    scenario: Option<&str>,
    enable: &[String],
    disable: &[String],
) -> Result<(), String> {
    let scenario_enables = /* resolve preset.enable if scenario set */;
    let scenario_disables = /* resolve preset.disable */;
    let all_enable  = chain(enable, scenario_enables);
    let all_disable = chain(disable, scenario_disables);

    model.interventions.retain(|iv| {
        let dom = iv.base_name.as_deref().unwrap_or(&iv.name);

        // Explicit disable wins over everything, including always_active.
        if list_matches(&all_disable, dom) { return false; }

        // Events stay on unless explicitly disabled.
        if iv.always_active { return true; }

        // Toggleable interventions fire only if enabled.
        list_matches(&all_enable, dom)
    });

    // Scenario's set/scale params still applied here.
    Ok(())
}
```

`list_matches` handles wildcard `"*"`, exact name, and `base_name`
matching for indexed expansions.

### Fixes bundled into this change

1. **Event-clearing latent bug** in `util.rs:448`. `retain(|iv| iv.always_active)`
   instead of `.clear()`.
2. **`pfilter.rs` spec violation.** Replace inline filter with
   `apply_scenario_filter(...)`. Adds the missing "no-scenario → clear
   toggleable" path that simulate has.
3. **`fit` spec violation.** Add `scenario` / `enable` / `disable` to
   `FitToml`, call `apply_scenario_filter(...)` after `load_model` in
   `fit/runner.rs:69`.
4. **`--disable` on events.** Explicit disable wins even when
   `always_active = true`. Previously events could only be disabled by
   deleting them from the source.
5. **Wildcard `enable = ["*"]`.** Matches every toggleable intervention.
   Useful when every declared `interventions {}` entry is historical and
   the user doesn't want to enumerate names. Does not apply to events
   (they're already on).

### Startup diagnostic

`fit run` and `pfilter` both print, right before sampling, a listing
that matches the priors block style. Shows every declared intervention
and event, marks which are firing, cites the reason (scenario / enable /
default / disable). Skipped if both lists are empty.

```
scenario: historical

interventions (3 active of 5 declared):
  ✓ school_closure     transfer 40% S→I_q at [t=62, 90, 118]
  ✓ sia_2021_r1        transfer 80% S→V   at [120]
  ✓ sia_2021_r2        transfer 80% S→V   at [180]
  ✗ lockdown_2022      (off — not in scenario)
  ✗ sia_2022           (off — not in scenario)

events (2 declared, always active):
    cohort_entry       every 365 days
    births             every year

priors:
  ...
```

## Decision flowchart (for docs and book)

Short prose snippet that goes in `camdl-run-spec.md` §12 and in the
book's counterfactuals chapter:

> **Where does this scheduled action belong?**
>
> - Did it happen in the data you're fitting to? → `events {}`
>   (always on, no fit config needed)
> - Are you evaluating whether to do it? → `interventions {}`
>   (off by default, enable via scenario)
> - Did it happen, but you also want "what if it hadn't?" —
>   start with `events {}` and use `disable` in the counterfactual
>   scenario, OR move to `interventions {}` with a `historical`
>   scenario. Either works; the first has less boilerplate when the
>   counterfactuals are rare.

## Tests

Explicit default-behavior tests at both layers, so the contract is
enforced by CI.

### Simulate-side (OCaml / Rust integration)

- `simulate_no_scenario_events_fire`: model with one event, one
  intervention. `camdl simulate` with no flags. Event fires; intervention
  does not. Regression guard for the `clear()`-kills-events bug.
- `simulate_enable_activates_intervention`: same model, `--enable sia`.
  Both event and intervention fire.
- `simulate_disable_silences_event`: `--disable cohort_entry`. Event
  does not fire; intervention (not enabled) also does not fire.
- `simulate_wildcard_enable`: `--enable "*"` activates all toggleable
  interventions.

### Fit-side

- `fit_no_scenario_events_fire`: fit.toml with no scenario/enable. Model
  has one event and one intervention. Particle filter log-likelihood
  reflects the event's state change at its fire time; intervention does
  not fire.
- `fit_scenario_enables_interventions`: fit.toml with
  `scenario = "historical"`. Interventions in that scenario's enable
  list fire during PF.
- `fit_startup_diagnostic_format`: capture stderr on fit startup,
  assert the "N active of M declared" block matches the documented
  layout.

### Pfilter-side

- `pfilter_no_scenario_events_fire`: same as fit case; guards the bug
  fix in `pfilter.rs`.

### Shared

- `scenario_filter_preserves_events_on_clear`: unit test for the helper.
  Given a Vec with events and interventions, calling with no scenario /
  no enable clears only toggleable.
- `scenario_filter_disable_beats_always_active`: unit test. Explicit
  disable removes events.

## Why this is the cleanest option

- **Single source of truth.** One helper for scenario filtering; all
  three CLI entry points share it. Can't drift.
- **Matches the spec exactly.** Every entry point conforms. Net
  reduction of undocumented behavior.
- **No DSL break.** `events {}` and `interventions {}` already exist;
  users just need clearer guidance on which to use. Vocabulary clarified
  in docs, not in grammar.
- **Historical-intervention UX is zero-config.** The common case (20 SIA
  rounds that all happened) lives in `events {}` and fits with no scenario
  at all. Only counterfactual-evaluation modelers pay the scenario cost.
- **Safe migration.** Users who have interventions declared today and
  rely on them firing in fit will see a loud diagnostic on startup
  ("0 active of N declared") the first time they run post-change. The
  fix is either adding a scenario or moving to `events {}` — both
  documented.

## Files

| File | Change |
|------|--------|
| `rust/crates/cli/src/util.rs` | `apply_scenario_filter` helper; wildcard + family-name matching; `disable` applies to events |
| `rust/crates/cli/src/pfilter.rs` | replace inline filter with helper |
| `rust/crates/cli/src/fit/config.rs` | add optional `scenario` / `enable` / `disable` to `[fit]` |
| `rust/crates/cli/src/fit/runner.rs` | call helper after `load_model`; emit active/inactive diagnostic |
| `rust/crates/sim/tests/intervention_event_defaults.rs` | new — all the explicit default tests listed above |
| `rust/crates/cli/tests/cas_integration.rs` | new cases for simulate / fit / pfilter defaults |
| `docs/camdl-run-spec.md` | new "Interventions in Inference" subsection with the decision flowchart |
| `docs/camdl-language-spec.md` | §14 clarified: events = "things that happened", not just "structural" |

## Out of scope

- DSL unification (one block with an `active: always | scenario`
  modifier). Cleaner but a breaking change; defer.
- Per-intervention `--enable` override from the `fit run` CLI beyond
  fit.toml. Easy add if requested.
- Tags / categories for grouping interventions across naming schemes
  (e.g., `@historical`). Can revisit if the wildcard + family-name
  pattern proves insufficient.
