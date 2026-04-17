---
status: proposal
date: 2026-04-17
---

# State-Snapshot Projections in the Particle Filter

## Motivation

The particle filter today supports exactly one observation-projection
mode: **flow accumulators** (`Projection::CumulativeFlow`). At each
observation tick the PF reads the per-stream transition counter
accumulated since the last observation, passes it to the likelihood as
`projected`, and resets the counter. This is the right abstraction for
*incidence* data — daily case notifications, weekly deaths, cumulative
reported hospitalizations — where the observed quantity is an event
count over an interval.

It is the wrong abstraction for everything else. The common pattern
that camdl currently can't fit is **point-in-time state readings**:

- **Boarding school influenza** (the canonical SIR teaching dataset):
  daily count of how many boys are *in bed* on day *t*. That is
  `prevalence(I)`, a state snapshot, not an interval-accumulated flow.
- **Hospital bed occupancy, ICU census, wastewater concentration**:
  prevalence by definition.
- **Seroprevalence surveys, test positivity**: fractions over the state
  vector (`I / (S + I + R)`).
- **Erlang-substage models**: observations like `B1 + B2` — arithmetic
  over several compartments at the same instant.

The simulation side already supports these. The compiler happily
accepts `projected = B1 + B2` in an `observations {}` block and emits a
`Projection::DerivedExpr` node in the IR; `camdl simulate --obs`
evaluates it correctly. The fitting runtime (`runner.rs:591`) hard-errors
with a "DerivedExpr projection not yet supported" message and refuses to
proceed. So the exact same `.camdl` file that generates synthetic data
from prevalence cannot be fit to the data it generated — a spec-level
asymmetry that blocks the book's "Fitting to Data" chapter (SIR /
SIBCR / SEIBCR comparison on boarding-school data, following Wearing
2005, Avilov 2024, Tverskoi 2025).

## Design principle

**The projection is an expression over state at a point in time.** Flow
accumulators are a special case: the "state" for a flow accumulator is a
scalar counter that the simulator updates on each transition and resets
on each observation. State-snapshot projections just read from the
compartment state vector instead of an auxiliary counter.

The two cases should share one evaluation pathway and one startup
diagnostic. A user who writes `projected = incidence(recovery)` and
a user who writes `projected = prevalence(I)` should get symmetric
behavior, symmetric error messages, and symmetric documentation.

## Proposed semantics

Two projection modes, both already present in the IR:

| IR variant | User syntax | What PF does at obs time *t* |
|---|---|---|
| `CumulativeFlow` | `incidence(recovery)`, `incidence(S→E)` | read per-stream counter, reset |
| `DerivedExpr(expr)` | `prevalence(I)`, `B1 + B2`, `I/(S+I+R)` | evaluate `expr` against current state, no reset |

`prevalence(X)` is sugar for `DerivedExpr(X)` where `X` is a
compartment reference. `prevalence(B1, B2)` and equivalent multi-arg
forms desugar to `DerivedExpr(B1 + B2)`. No new IR node is needed.

### Snapshot timing

The snapshot is the value of the projection expression *at the
observation time*, evaluated against the simulation state at that
time. The following rules specify what "the state at time *t*" means
per backend; these need to be added to `camdl-run-spec.md` §3 or a new
§5 ("Observation semantics"):

- **Gillespie SSA (continuous-time):** state is piecewise-constant
  between events. The snapshot reads the state that has been in effect
  since the last event preceding *t*. If an event fires exactly at *t*
  (measure zero in the continuous sim but still possible for
  deterministically-scheduled events and interventions), the snapshot
  reads the **post-event** state. Rationale: matches the existing
  convention for `CumulativeFlow` (which includes events up to and
  including *t*) and matches "what a census taker sees at noon on
  day *t*."
- **Chain-binomial / tau-leap (discrete-time, step `dt`):** the
  snapshot reads the state at the step boundary that lands on, or
  first passes, *t*. For `dt=1` with daily observations this is exact;
  for `dt < 1` with daily observations it is the state at the first
  step boundary ≥ *t*. This matches how the flow accumulator reports
  flows accumulated through the step reaching the observation tick.
- **ODE / hybrid (continuous integrator):** snapshot reads the
  integrator's interpolated state at exactly *t*. Standard
  dense-output evaluation.

### Intervention interaction at observation time

A tricky but load-bearing subtlety: **if a scheduled intervention fires
at the same time as an observation**, the snapshot must read the
**post-intervention state**. Rationale: the data was generated in a
world where the intervention had already fired; evaluating the
likelihood against pre-intervention state would deterministically bias
the PF toward rejecting scenarios where the intervention is correctly
represented. This ordering (intervention → snapshot → next step) also
matches `simulate --obs`, so synthetic data and fit-time likelihood are
consistent.

Concretely, the step loop at an observation time becomes:

1. Advance state to *t* (Gillespie: run events until next event > *t*;
   discrete: take the step that lands on/just past *t*).
2. Fire any scheduled interventions at *t* (via
   `apply_interventions_at`).
3. Evaluate the projection expression; pass `projected` to the
   likelihood.
4. Reset `CumulativeFlow` counters.

The chain-binomial `step_one` already fires interventions at `t+dt`
(see the 2026-04-17 double-fire incident); aligning the PF's snapshot
read to happen **after** `step_one` on the step that reaches the
observation tick gives us the correct ordering without any additional
sequencing logic.

### Likelihood-family guidance

Incidence and prevalence need different default likelihoods; users who
copy a NegBin observation model from an incidence fit onto a prevalence
model will silently get wrong results. This should be surfaced in docs
(`camdl-inference-spec.md` §observation-models) and a soft diagnostic:

- **Incidence (CumulativeFlow):** NegativeBinomial or Poisson with
  reporting rate. Support on ℤ≥0; overdispersion natural.
- **Prevalence, single compartment (DerivedExpr):** Binomial(N, p)
  where N is the total population (if known and fixed) and
  p = projected/N; or Poisson for large N. NegBin on prevalence is
  meaningful but the parameters have a different interpretation.
- **Prevalence as a fraction (DerivedExpr returning ∈ [0,1]):** Beta
  or Binomial.

The startup diagnostic (below) can emit a note when a NegBin is paired
with a DerivedExpr projection: "`stream X` uses NegativeBinomial on a
prevalence projection. This is valid but uncommon — Binomial or Poisson
is the typical choice for point-in-time counts. Check §obs-models in
the inference spec."

### Identifiability note for the docs

Prevalence data and incidence data have different identifiability
properties. Pure prevalence data is often weakly identifying for the
recovery rate γ (the infection duration controls how quickly prevalence
decays after the peak, but the peak height is confounded with the
attack rate); pure incidence data directly informs the flow into I at
every tick. Joint prevalence + incidence fits (when both are observed)
are strictly more informative. This belongs in the "Fitting to Data"
chapter of the book but should also be mentioned as a one-liner in the
inference spec so that users don't silently get under-identified fits
from the boarding-school data.

### User-facing terminology

`CumulativeFlow` is an implementation name. User-facing errors, the
startup diagnostic, and the spec should use **incidence projection**
(for `CumulativeFlow`) and **prevalence projection** or **state-snapshot
projection** (for `DerivedExpr`). Rename the error message in
`runner.rs:591` from "DerivedExpr projection not yet supported" to
"state-snapshot / prevalence projections not yet supported" as a
stopgap; after this proposal lands, the error goes away entirely.

### Startup diagnostic for observation streams

`fit run` and `pfilter` already print priors and (after the
intervention/event proposal) active interventions. Extend that summary
with an observation-streams block, printed in the same style:

```
observations (3 streams):
  ✓ cases          incidence(S→E)      NegativeBinomial(μ = ρ·proj, φ)
  ✓ bed_count      prevalence(I)       Binomial(N, proj/N)
  ✓ b_total        B1 + B2  (Erlang)   Poisson(proj)
```

This makes it impossible to silently mis-pair a likelihood with a
projection type; the user sees the pairing at the top of every run.
Skipped if the model has no observations.

## Implementation

### Files touched

| File | Change |
|---|---|
| `rust/crates/sim/src/inference/particle_filter.rs` | branch on `Projection::{CumulativeFlow, DerivedExpr}` in the observation-eval loop; for `DerivedExpr` call `eval_expr` against current state instead of reading the flow counter |
| `rust/crates/sim/src/inference/dmeasure.rs` | same branch in the likelihood-compilation path; `DerivedExpr` needs no per-stream counter |
| `rust/crates/cli/src/fit/runner.rs:591` | drop the hard-error; remove `resolve_flow_indices`'s assumption that every obs stream is a flow |
| `rust/crates/sim/src/obs_loglik.rs` | unchanged — likelihood evaluation is projection-agnostic |
| `rust/crates/sim/src/inference/pgas.rs`, `pgas_grad.rs` | same branch for the likelihood and gradient paths |
| `rust/crates/sim/src/chain_binomial.rs` (PF path) | confirm intervention/snapshot ordering (interventions fire before the snapshot is read) |
| `rust/crates/cli/src/util.rs` | extend `print_scheduled_actions_summary` with the observation-streams block (or add a sibling `print_observations_summary`) |
| `docs/camdl-run-spec.md` | new §5 "Observation semantics" with the snapshot-timing rules above |
| `docs/camdl-inference-spec.md` | likelihood-family guidance + identifiability one-liner |
| `docs/book/src/guide/fitting-to-data.qmd` | rewrite prevalence examples now that they work end-to-end |

### Test plan

Four new tests, all in `rust/crates/sim/tests/`:

- **`pf_prevalence_sir_recovers_params`** — synthetic SIR, generate
  daily-prevalence data with known (β, γ), fit via PF + NUTS,
  posterior means within 2σ of truth. The minimum viable regression
  test.
- **`pf_prevalence_and_incidence_agree_on_params`** *(cross-check, the
  important test)* — generate a single ground-truth SIR trajectory
  long enough to be well-identified on its own (toy SIR with `t_end ≥
  100`, ≥50 observation points, moderate epidemic so both peak and
  decay are observed); derive two synthetic observation streams from
  it, one incidence (`incidence(S→E)`), one prevalence
  (`prevalence(I)`); fit each independently. **Assertion:** the 90%
  credible intervals for β and γ overlap between the two fits.
  Prevalence and incidence have genuinely different Fisher information
  about the parameters (prevalence is more informative about γ via
  decay shape; incidence is more informative about β via the flow into
  I), so posterior means will differ and posterior widths will differ —
  but both posteriors must be compatible with the same DGP. CI overlap
  is the right arithmetic-correctness guarantee; a means-within-ε test
  would either be too loose (hides a bug) or too tight (rejects
  legitimate posterior-width differences). A failure here means one
  projection path is arithmetically wrong, and the direction of the
  bias tells us which.
- **`pf_snapshot_reads_post_intervention_state`** — SIR with a
  scheduled `FractionTransfer(0.5)` at `t = 10`, prevalence
  observation at `t = 10`. Assert the likelihood sees the
  post-intervention S. A failure here means the intervention ordering
  regressed.

Plus an integration test via the existing `tests/test_ocaml_to_rust.sh`
path once the boarding-school fixtures are added (follow-up).

## Out of scope

- ~~**Implicit sum for Erlang-stratified compartments**
  (`prevalence(B)` auto-summing over `B1..Bk`).~~ **Shipped after
  initial review** — the compartment-grouping semantics turned out to
  already be settled: rate expressions apply the same "omitted
  dimension sums over it" rule (language spec §5.1), so
  `prevalence(B)` on a stratified `B` just follows the same rule and
  emits `CurrentPopSum` over all expansions. Partial indexing
  (`prevalence(B[patch1])` on a `B × patch × age` compartment) is
  still deferred; the common case of "Erlang substages with no other
  stratification" and the fully-indexed case both work today.
- **Period-average prevalence** (mean of `I` over the interval
  `[t_{k-1}, t_k]`). A legitimate observation mode for weekly ICU
  census with daily dynamics, but requires a trapezoidal / running-sum
  integrator in the step loop. Defer; the snapshot semantics cover the
  common case, and daily-observation daily-dt models don't distinguish.
- **Reporting delays / lag kernels** on prevalence. Modeling-layer
  concern; users can compose with `time_functions` today.
- **Auto-scoring likelihood defaults** ("if DerivedExpr + count data,
  default to Binomial"). Flagging via diagnostic is enough; do not
  silently rewrite the user's model.

## Why this is low-risk

- IR is already expressive enough; no schema bump, no golden-file churn.
- `simulate --obs` already evaluates `DerivedExpr` projections — we
  are importing a working evaluator into the inference runtime, not
  designing a new one.
- Intervention ordering piggybacks on `step_one`'s existing firing
  semantics (just freshly audited by the 2026-04-17 double-fire
  incident); no new ordering edge case is introduced.
- The failure mode of the old code was an explicit hard-error at fit
  start, not a silent wrong answer. Enabling the new path cannot make
  previously-working fits worse.
- Cross-check test (prevalence fit vs incidence fit on the same
  trajectory recover the same posterior) gives us an arithmetic
  correctness guarantee on the critical path before any book / tutorial
  material depends on it.
