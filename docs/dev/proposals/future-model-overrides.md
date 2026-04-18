---
status: stub
date: 2026-04-18
---

# Model-Field Overrides from fit.toml (and batch.toml)

## Status

**Stub — deferred, not blocking.** Captured to page the design out of
active memory. Revisit when the second or third instance of the
underlying pattern shows up.

## The concrete ask (today)

The book's "Fitting to Data" chapter wants to compare observation
cadence: fit the same SIR to synthetic data sampled every 1, 2, 7,
and 14 days. The `every` field lives in the `.camdl` model's
`observations { }` block, so a frequency sweep currently requires
four `.camdl` files (or four fit.toml files pointing at four
near-identical models). Workable, but duplicated.

## The generalized pattern

"Frequency sweep" is the visible tip of a larger class: let fit.toml
(and batch.toml) override a small, named subset of fields on the
`.camdl` model. Candidate overrides seen so far or reasonably
anticipated:

- `observations[N].every` / `observations[N].times` — observation
  cadence and explicit schedule (book chapter today).
- `simulate.t_end` — fit a truncated window of a longer trajectory.
- `observations[N].likelihood` — compare Poisson vs NegBin vs
  Binomial on the same model + data.
- `init.S`, `init.I` — sweep initial conditions for start-of-epidemic
  sensitivity analysis.
- `compartments` — swap SIR ↔ SIRS by adding a waning rate;
  structural comparison (this one is probably too big — better done
  with separate `.camdl` files even long-term).
- Prior overrides — comparing informative vs diffuse priors on the
  same fit.

## Why it's deferred

- **The workaround is trivial and readable.** N fit.toml files +
  three lines of shell is not worth an abstraction today. Users who
  hit the pattern once don't need to re-derive it; those who hit it
  three times will ask.
- **The right shape isn't obvious.** Options span from a narrow
  `[synthetic] observation_frequencies = [...]` sugar (solves today's
  case, doesn't generalize) to a full
  `[overrides]` block on fit.toml (solves everything, opens every
  model-field as tuneable from outside, needs provenance-hash
  discipline so sweeps don't silently share caches). Between those
  sit several workable middles. Designing without enough use-cases
  means picking one and living with it.
- **Provenance complexity.** Every override participates in the
  per-cell content hash. Scenarios already patch scenario-visible
  fields; `[overrides]` would patch other fields. The composition
  rule — what wins, and how hashes reflect both — is non-trivial.
- **Scope creep risk.** Every user who wants their one favourite
  field overridable will ask for it. The design choice isn't "do we
  allow overrides" but "what's the minimum set, and what's the
  justification for excluding the rest." Needs a clear principle.

## Design questions to resolve (when the time comes)

1. **What's the surface?** `[overrides]` at the top of fit.toml?
   Sub-blocks like `[overrides.observations.cases]`? Or per-axis
   sugar (`[synthetic] observation_frequencies = [...]`) that
   covers the most-asked cases only?
2. **Which fields are overridable?** Curated whitelist (ship the
   first 3–4 we've seen asked for, reject others) or generic patch
   over the IR JSON? The former is safer; the latter is more
   general but forces us to commit to IR stability.
3. **Interaction with `[synthetic]`.** An override to `every`
   needs to affect both data generation AND the fit's likelihood
   eval — they must agree, or the fit is nonsense. The
   generalization has to thread overrides through both halves.
4. **Interaction with scenarios.** A scenario is already a
   model-field override (enable/disable, param set/scale).
   `[overrides]` vs `scenario` precedence needs a rule —
   probably "scenarios first, overrides last, overrides error
   if they conflict with a scenario."
5. **CAS / provenance hash.** The per-cell hash must include the
   override payload. Straightforward once the surface is decided.
6. **Is sweeping an override a third axis in the
   synthetic-fit-replicate grid?** E.g.
   `overrides.observation_frequencies = [1, 7, 14]` × `sim_seeds`
   × `fit_seeds`. If yes, the grid runner needs a third loop
   layer; the TOML shape needs to distinguish "apply this single
   override" from "sweep this override across these values".

## Signal to act

Track this list. When a *second* independent ask comes in for a
different override field — not the book's frequency case — upgrade
the stub to a proposal. Two data points are enough to reveal the
general shape; one isn't.

## Pointers back

- The book chapter's frequency sweep, once written, should include
  a brief pedagogical note: "camdl doesn't have a
  frequency-sweep built-in — this is the kind of workflow you drive
  with a shell loop for now." See
  `docs/dev/proposals/2026-04-17-synthetic-fit-replicates.md` for
  the related synthetic-data machinery.
- If an override layer lands, `camdl-run-spec.md §6` (FitConfig)
  gains the new block and `camdl-inference-spec.md §3.7` gains a
  mention alongside `fit_seeds` / `[synthetic]`.
