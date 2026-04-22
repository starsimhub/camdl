---
date: 2026-04-22
status: deferred
related: 2026-04-22-observation-sampler-scratch-state.md (same "escape hatch
  lived too long" pattern), GH #8
---

# Harden `--no-dim-check` with required reason + visible warning

Design note capturing a small hardening intervention for a CLI escape
hatch. Deferred — not blocking any current work, but worth doing
before shipping to real users because the current form masks the kind
of bug reports we want to hear about.

## The problem

`--no-dim-check` currently silences dimensional analysis with no cost
and no signal. It's the right shape of escape hatch — legitimate uses
exist (see below) — but in its current invisible form it tranquillises
bug reports instead of triggering them.

Concrete evidence: the book agent's Ross-Macdonald / He 2010 vignette
work used `--no-dim-check` for weeks as a workaround for GH #8
(interpolated forcings couldn't declare units). Because the flag was
free to flip, the underlying bug didn't surface as a filed issue until
it came up in conversation. If the flag had been noisier, GH #8 would
have been filed day one.

Same failure mode as the observation-sampler scratch-state incident
(2026-04-22-observation-sampler-scratch-state.md): something obviously
wrong happens silently, and the silence extends the bug's lifespan.

## Legitimate uses to preserve

Three classes, in decreasing order of legitimacy:

1. **Compiler bugs in the dim-checker itself.** False positives from
   the checker shouldn't be ship-blockers. Users need an escape until
   a patch lands. Hard requirement for the flag to exist.

2. **Internal test fixtures.** `ocaml/test/test_compiler.ml` sets
   `Compiler.no_dim_check := true` at the top of the file because
   those tests exercise parser / expander / codegen, not dim logic.
   Legitimate but should be library-level, not CLI-visible.

3. **Porting empirically-validated external models.** Someone with a
   pomp model that's been field-tested doesn't need the checker to
   tell them their rate expression is wrong. A flag lets them get
   running today while they decide whether to add annotations.
   Semi-legit — still the common trap path.

## Proposed intervention

Not a redesign, a taxation. Four changes:

### 1. Require a reason string

```
$ camdl simulate model.camdl --no-dim-check
error: --no-dim-check requires a justification; use e.g.
       --no-dim-check="dim-checker bug tracking GH-99"
       --no-dim-check="porting pomp model, unit annotations pending"

$ camdl simulate model.camdl --no-dim-check="GH #8 workaround"
⋯
```

Forces the user to articulate *why*. A reason doesn't fix the problem
but it converts the moment of flipping the flag into a tiny piece of
deliberate design documentation rather than an autopilot unblock.

### 2. Loud warning on every run

```
warning: dimensional analysis disabled for this run
  reason: "GH #8 workaround"
  Models run with --no-dim-check are unverified against dimensional
  consistency. Use this flag only for:
    - dim-checker bugs (please file an issue if this is one)
    - bootstrapping a pre-annotation external model
    - internal test fixtures
  If you're not in one of these cases, add unit annotations instead.
```

Printed to stderr on every invocation, so scripts using the flag emit
the warning to logs — the cost of the escape hatch is visible to
anyone reviewing a run.

### 3. Store reason in run.json provenance

CAS writes already carry the resolved model and seed. Add
`"dim_check_disabled_reason": "GH #8 workaround"` to the run metadata
when the flag is active. Enables auditing: "which runs in this
experiment had dim-check disabled, and why?"

### 4. Split internal vs user-facing

Rename library-level `Compiler.no_dim_check` to
`Compiler.skip_dim_check_for_expansion_testing` or similar —
explicitly scoped to internal test-harness use. CLI flag becomes a
distinct thing that always requires the reason string.

Test-harness code paths shouldn't share the code path that's meant to
be hostile to invocation.

## Explicit kill conditions

The flag is a stepping-stone, not a permanent feature. It should be
deprecated and removed when both:

1. The dim-checker's supported feature set widens enough that
   "bootstrapping external models" is no longer a common need.
2. No GH issue has been filed for a dim-checker false-positive in
   some number of months (say 6).

At that point "add the annotation or open an issue" is the correct
workflow for every user who hits a dim error. The flag's remaining
legitimate use (compiler bugs) gets handled by patch-and-release, not
user-level workaround.

## Implementation cost

~1 hour:

- `rust/crates/cli/src/args.rs`: change `--no-dim-check` from `bool`
  to `Option<String>`; reject empty reason.
- CLI entry points: emit the warning, attach reason to run metadata.
- `fit/runner.rs` + `main.rs` run.json builders: add the field.
- Library split: rename `Compiler.no_dim_check` → internal name.
- Update `camdl-language-spec.md` §2.2 (dim-check) with the new
  invocation syntax and the guidance.
- Tests: one test that the CLI rejects `--no-dim-check` without a
  reason; one that the warning is emitted.

## Why deferred

Not blocking any current correctness work. The GH #8 fix already
closed the specific user-facing gap that motivated this note. Wave 3
(#5 reactive interventions) and the queued hardening audit are the
near-term value targets.

Revisit when:
- Another user-filed issue has the "I hit this, flipped --no-dim-check,
  moved on" shape.
- CLI polish pass happens and multiple small UX hardenings can land
  together.
- External users beyond the book agent start hitting the flag.

## The broader principle

**Never ship a silent escape.** Every escape hatch in camdl should
make its use visible — in the invocation, in the output, in the run
provenance — so that "turned off the checker" shows up when someone
reviews what was run. This note applies the principle specifically to
`--no-dim-check`, but the same stance should shape any future
`--no-*-check` or `--skip-*` or `--force` flag we consider adding.

The Rust `unsafe` block is the right model: the feature exists, it's
the last tool in the box (not the first), and invoking it shows up in
the source. camdl's escape hatches should survive in the invocation
and in the provenance the same way.
