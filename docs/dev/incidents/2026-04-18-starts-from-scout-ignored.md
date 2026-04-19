# Incident: `starts_from = "scout"` silently discarded scout's best params

**Date:** 2026-04-18
**Status:** Fixed.
**Severity:** Medium. Refine stages ran from the config's
`[estimate].*.start` values instead of scout's MLE. On easy likelihood
surfaces refine lucked into the right basin anyway; on harder or
multimodal surfaces it wasted compute and sometimes produced fits
that were worse than scout's.

## Fundamental vs. implementation

**Fundamental:** nothing. The intended priority
`prior_state > estimate.start > fixed > model default` is the obvious
one for a staged pipeline where refine continues from scout.

**Implementation:** two blocks in `FitRunConfig::build` applied
values to `base_params` in the wrong order. The block that wrote
`prior_state.start_values` ran before the block that wrote
`estimate.*.start`, so `est.start` overwrote whatever scout had
produced. A comment directly above the first block claimed the
priority correctly — nobody noticed the code did the opposite.

Compounding this: `EstimatedParam::initial` WAS set to the right
(scout-best) value by a later override in `build_if2_params`, but IF2
never reads `initial`. `sim::inference::if2::run_if2_with_progress`
constructs the per-particle cloud from its `base_params: &[f64]`
argument (`current_params = base_params.to_vec()` at if2.rs:338);
`EstimatedParam` supplies the perturbation metadata (rw_sd,
transform, bounds, ivp flag) but not the starting value. So the
"correct-looking" `p.initial = scout_best` line in
`build_if2_params` was dead code for IF2's starting point, and no
path other than `base_params` reached the algorithm.

## Evidence

Downstream reported (boarding-school SIR / Erlang-2):

- Scout `fit_state.toml` best: β = 1.457, γ = 1.195, k = 84.4.
- Refine chain 1, iter 0:       β = 2.008, γ = 0.552, k = 28.1.

The iter-0 values match the fit.toml `[estimate].*.start` lines
(`start = 2.0`, `start = 0.5`, `start = 28.1`), not scout's output.

## Blast radius

- **Any `starts_from = "scout"` (or any stage-to-stage handoff via
  FitState) in an IF2-only pipeline**: refine started from config
  defaults every time. When scout landed in the right basin, refine
  sometimes re-found it anyway and produced a sane MLE — just with
  more wasted compute than advertised. When scout landed off-MLE
  and refine was supposed to polish, refine *restarted from scratch*
  and the chain of stages was a lie.
- **`starts_from = <path-to-external-dir>`**: same bug, same
  silent-discard.
- **PGAS / PMMH stages following IF2**: unaffected on the starting-
  point dimension; they use the MLE via a different path (`config.
  base_params` is constructed differently in PGAS/PMMH entry points).
  Their per-chain starts come through a separate code path that
  reads the scout MLE correctly.
- **Non-estimated (fixed) parameters**: unaffected. They're written
  by the `[fixed]` block after est.start, and the fix doesn't
  change their priority (prior_state → est.start → fixed was the
  intent; the bug made it est.start → prior_state → fixed; the fix
  makes it est.start → fixed → prior_state). Prior_state now wins
  over both, which is correct because scout produces values for
  every parameter including fixed ones (scout's `fit_state.toml`
  `start_values` is `collect_all_params(...)` over the full set).
- **Scout itself**: unaffected. Scout doesn't consume a
  `prior_state`; it generates one.

## How we found it

Downstream was comparing refine iter-0 parameters across two
fits (SIR and Erlang-2) and noticed that iter-0 values always
matched the fit.toml `start` lines, regardless of what scout
produced. The pattern was suspicious — iter-0 should vary with
scout's output, not be fixed at config defaults. A grep for
`start_values` through the runner showed the ordering inversion.

## The fix

`rust/crates/cli/src/fit/runner.rs`, `FitRunConfig::build`:

Before:

```rust
// Apply start overrides from fit_state if provided (overrides model defaults)
if let Some(state) = prior_state { ... base_params[idx] = scout_best ... }
// Apply estimate start values to base_params (may override model defaults)
for spec in &fit.estimate { ... base_params[idx] = est.start ... }
// Apply fixed numeric values
for (name, val) in &fit.fixed { ... base_params[idx] = fixed ... }
```

After:

```rust
// 1. est.start
for spec in &fit.estimate { ... }
// 2. fixed
for (name, val) in &fit.fixed { ... }
// 3. prior_state last — wins over both.
if let Some(state) = prior_state { ... }
```

A doc comment at the start of the block now states the priority
and cross-references this incident, so the next reader sees the
invariant the ordering enforces.

## Regression test

`rust/crates/cli/src/fit/runner.rs::tests::
fit_state_overrides_config_start_in_base_params` builds a
`FitRunConfig` with a `FitState` that has `start_values["beta"] =
9.9` and a fit.toml with `[estimate] beta = { start = 1.5 }`. After
build, asserts `config.base_params[beta_idx] == 9.9`. Verified by
reverting the fix: the test fails with a clear message naming the
observed and expected values.

## How did this escape

Three things:

1. **The comment at the top of the block was correct.** "Priority:
   fit_state start_values > estimate start > fixed value > model
   default" — a reviewer reading the comment and believing the code
   matches would not catch it. The comment was last-touched when
   the intended priority was set, not when the order was changed.
2. **Tests for `starts_from = "scout"` asserted the FitState file
   was *read*, not that its values *won*.** There was a
   resolve_prior_precedence_chain test (for priors) and a scout-
   writes-fit-state test, but no test that the values in
   fit_state.toml actually drove refine's iter-0. The missing test
   is the one that lands now.
3. **On easy surfaces, refine converges anyway.** SIR's basin is
   wide enough that a re-start from config defaults still lands at
   roughly the same MLE as scout's output — the bug was a waste of
   compute, not a visibly wrong answer. On harder surfaces (spatial
   models, Erlang substages) the symptom would have shown up as
   refine's "best" being worse than scout's, which is a weird enough
   signal that it should have been caught earlier but wasn't.

## Followup actions

- **Done in this fix:** priority inversion corrected; comment
  rewritten to match; regression test; this report.
- **Not done; worth considering:**
  - **Remove the dead `EstimatedParam::initial` override in
    `build_if2_params` lines 319–327.** That block looks like it's
    doing something useful (preferring prior_state over est.start
    for p.initial) but IF2 never reads p.initial, so it's
    misleading. Either delete it, or teach IF2 to read it (safer:
    use it as the starting point explicitly, eliminating
    base_params as the sole carrier). Low priority but each reader
    wastes time chasing the meaning.
  - **Audit other stage handoffs** (PGAS, PMMH start values) for
    similar ordering bugs. They go through a different code path
    and appear fine, but the same class of "priority inversion with
    a correct comment" could exist.

## Lessons

- **The comment is not the contract; the code is.** A correct
  comment above wrong code is worse than no comment — readers
  trust it and skim the implementation. The fix swaps the order
  so the comment matches; the next time this kind of inversion
  appears, the compile-step won't catch it but a regression test on
  the *effect* (not just the parse) will.
- **"Scout feeds refine" is a claim that needs end-to-end testing,
  not just unit testing of each side.** Scout writes fit_state.toml
  and refine reads fit_state.toml — both tested. The missing test
  was "the scout value in fit_state.toml appears at refine iter 0,"
  i.e., the composition. Same shape as the 2026-04-18 SBC
  seed-mix bug: feature-in-isolation tests don't catch cross-feature
  parity bugs.
- **`EstimatedParam::initial` is a footgun.** It looks authoritative
  but isn't consulted by IF2. The data-carrying field is
  `base_params[idx]`. Either the field should be removed or IF2
  should consult it. Having both without the first one being read
  is a recipe for exactly this confusion.
