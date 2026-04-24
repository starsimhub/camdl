# IF2 Cooling Schedule — Presentation Without Semantic Change

**Status:** Proposed
**Author:** upstream analysis (2026-04-24), captured + lightly edited
**Date:** 2026-04-24
**Related:**
- `docs/dev/incidents/2026-04-24-cooling-semantics-docs-drift.md` (the
  doc-drift incident that motivated this)
- `docs/methods/cooling.md` (canonical description of the cf50 formula
  and the scout-vs-refine design intent; the recipe table in §4 here
  is the natural next iteration of that doc)

---

## Thesis

The cooling-schedule semantics camdl inherits from pomp's `mif2` are
fine. The presentation is what's confusing. Three concrete changes —
runtime display of the endpoint reduction, an optional alternative
TOML spelling, a cooking-recipe table in the docs — get most of the
ergonomic win without breaking pomp-convention compatibility and
without touching any of the cooling math.

No semantic redesign. No new parameter name replacing
`cooling_fraction`. Pure UX/UI on top of what already runs.

## Why not redesign the semantics

Three arguments for keeping pomp's cf50 convention in place even
though it has real ergonomic flaws:

1. **Cross-tool interoperability with pomp is a user asset.** Anyone
   coming to camdl from the mif2 world (measles, cholera, COVID
   modelling literature, every Ionides-lab publication, every pomp
   tutorial on Stack Overflow or in supplementary code) has
   `cooling.fraction.50` as their mental currency. A private-to-
   camdl renaming creates a persistent translation burden that lands
   hardest on students and new practitioners — the audience camdl's
   teaching framing explicitly targets.

2. **The "support both conventions" path is worse than either
   alone.** If we introduced `cooling_sd_ratio` (final SD / initial
   SD) as a new parameter, users who consult pomp docs for
   cooling.fraction.50 advice would need to translate it; users who
   think in endpoints would use the new knob; and config files
   across the repo would silently mix the two. The failure mode
   "which convention is this TOML using?" is worse than the current
   "why is lower more aggressive?"

3. **Scope. We are mid-investigation.** The he2010 analysis is mid-
   stream, the decibans proposal is queued, the IF2 findings doc
   needs the cooling-math corrections from this week's work folded
   in. Piling a semantic redesign onto that stack doubles the
   remediation scope with zero scientific payoff for the current
   analyses.

## Three concrete presentation changes

### Change 1 — Display endpoint reduction everywhere cooling appears

Every time `cooling_fraction` is surfaced (fit.toml validation
output, IF2 run headers, stage-progress lines, trace-plot captions,
error messages), annotate with the consequence the user cares about.
Current:

```
running 36 chains × 2000 particles × 200 iterations, cooling=0.9, dt=1
```

Proposed:

```
running 36 chains × 2000 particles × 200 iterations, cooling=0.9 (SD 1.00 → 0.90 at halfway → 0.81 at end), dt=1
```

For `fit.toml` load-time validation output:

```
stages.scout  : cooling = 0.70   (SD: 1.00 → 0.70 @ halfway → 0.49 @ end)
stages.refine : cooling = 0.05   (SD: 1.00 → 0.05 @ halfway → 0.0025 @ end)
```

Users never have to recompute the endpoint in their head. The
parameter itself stays what it is; the display makes the consequence
legible.

**Implementation scope.** There is already a "Cooling schedule
preview" block in `rust/crates/cli/src/fit/runner.rs` (grep for
`rw_at`) that prints SD at iter 1, halfway, and end. It just needs
to be:
  - Enabled by default on every stage start (currently printed once
    per stage; keep that).
  - Condensed to a single line matching the format above.
  - Echoed in fit.toml-load-time validation output (so users see
    the reduction before any computation starts, not just at stage
    launch).

Estimated cost: half a session. One-file change in
`fit/runner.rs` plus a small echo in `fit/mod.rs`'s validation path.

### Change 2 — Optional alternative spelling in `fit.toml`

Accept either `cooling` (pomp convention) or `cooling_final_sd_ratio`
(endpoint convention) in a `[stages.<name>]` block. Internally
canonicalize to the pomp convention; under the hood there's still
one schedule, one formula.

```toml
[stages.scout]
cooling = 0.70                 # pomp cooling.fraction.50

# OR equivalently (both forms produce the same runtime behaviour):
cooling_final_sd_ratio = 0.49  # final SD / initial SD
```

Rules:

- Exactly one of the two keys allowed per stage. Both-set is a
  validation error with an explicit message pointing at
  `docs/methods/cooling.md`.
- When `cooling_final_sd_ratio = r` is specified, internally set
  `cooling = sqrt(r)` (since final = cooling²).
- Validation output reports **both** forms regardless of which was
  used:
  ```
  stages.scout: cooling = 0.70 (= cooling_final_sd_ratio 0.49)
  ```
- Parse-time error messages name both spellings when cooling is
  mis-typed or missing: "expected `cooling = FLOAT` or
  `cooling_final_sd_ratio = FLOAT`".

**Implementation scope.** Touches the fit.toml schema types (one
additional field on the stage config), the TOML → internal config
mapping (compute the canonical `cooling` value), and the error-
message surface. Estimated cost: 1–2 sessions, primarily in
`rust/crates/cli/src/fit/config.rs` or equivalent.

**Risk.** Minor: adds one schema field, slight breaking change for
anything programmatically parsing stage configs. Fit.toml files
using the current convention continue to work unchanged.

### Change 3 — Cooking-recipe doc in `docs/methods/cooling.md`

`docs/methods/cooling.md` already covers the formula and the
derivation. What it doesn't have yet is a *prescriptive* recipe
table — a menu of cooling values with labels and use-cases that
users can copy from without computing anything:

| Goal | `cooling` value | SD at end | Use for |
|---|---|---|---|
| Exploration — chains stay mobile, find basins | **0.70** | 0.49 × initial | scout stage (default) |
| Moderate convergence — polish without aggressive collapse | **0.20** | 0.04 × initial | refine on shaky scouts, or when scout's Rhat was marginal |
| Aggressive concentration — final MLE collapse | **0.05** | 0.0025 × initial | refine, validate (default) |
| Very aggressive — use only if you trust scout completely | **0.01** | 0.0001 × initial | rare; final validate pass on well-identified models |

And a corresponding "don't do this" table:

| `cooling` value | SD at end | Why it's probably wrong |
|---|---|---|
| 0.95 | 0.90 × initial | Barely cools at all. Fine for scout exploration if paired with many iterations; wrong for refine (chains won't concentrate onto scout's point). |
| 0.0001 | 10⁻⁸ × initial | Over-aggressive. Collapses before particle filter noise has been averaged; particle cloud locks onto a noisy PF-eval local point. |

Users rarely want to pick a cooling value from first principles —
they want to pick from a short menu and understand the consequences.
Put the menu in the doc. This is where camdl's teaching-oriented
framing pays off: we can be more prescriptive than pomp's docs
because we're teaching, not just providing a library.

**Implementation scope.** Edit to `docs/methods/cooling.md`. Half
an hour.

## Where to draw the line

If after living with these presentation fixes we still see users
tripping on "why does lower mean more cooling" or "what does the 50
mean" in practice — not the presentation but the actual concept —
then a semantic redesign becomes worth revisiting. Check: after the
doc fixes land and the endpoint-reduction display is shipped, do new
users (book readers, vignette contributors) still ask those
questions?

If yes, the semantics was the problem all along. If no, the
presentation was the problem and we're done. The upstream
recommendation (and the author's) is: presentation is the entire
problem, and the evidence is that once the doc surfaces all agreed
on cf50 + the design intent (scout hot, refine cold), the
conceptual confusion evaporated. The lingering inconvenience is
purely display-level.

## Prioritisation against other in-flight work

This proposal is **not blocking** anything. The doc fixes from the
2026-04-24 incident already landed (commit 62da628), which is what
actually unblocks the book agent on he2010 cooling configuration.
The presentation changes here are polish that pays off persistently
but doesn't fire on any urgent pipeline.

Recommended ordering:

1. **Evidence-in-decibans proposal**
   (`docs/dev/proposals/2026-04-23-evidence-in-decibans.md`) first.
   It's approved, the book chapter wants it, and it closes a
   different interpretive gap (Δlog-lik in dB with Jeffreys labels)
   that the book agent needs for model-comparison narrative.

2. **This proposal (change 1 — endpoint display)** second. Half a
   session. Lands the single most-impactful piece of this proposal
   at low cost. Changes 2 and 3 can follow or defer.

3. **This proposal (change 2 — alternative spelling)** third, if at
   all. Ship it if users report confusion on TOML authoring; defer
   otherwise.

4. **This proposal (change 3 — recipe table)** concurrent with 2 or
   whenever docs/methods/cooling.md gets its next edit pass.

## Out of scope for this proposal

- Any change to the underlying cooling formula, the `per_step_cooling`
  computation at `if2.rs:250-251`, or the scout/refine `_COOLING`
  defaults at `fit/scout.rs:14` and `fit/refine.rs:15`.
- Auto-migration of existing vignette `fit.toml` files to the new
  alternative spelling. Those stay in pomp-convention form unless
  the author chooses to rewrite them.
- A deprecation path for the pomp-convention key. `cooling` remains
  the primary, authoritative name; `cooling_final_sd_ratio` is an
  alternative surface, not a replacement.

## Recommendation

**Hold for now.** The doc fixes are what unblocked the immediate
pipeline. The endpoint-display change is small enough to ship
alongside the decibans proposal work or to slot in as a utility
improvement whenever fit/runner.rs is next touched; the alternative-
spelling change is worth doing only if user feedback indicates
authoring friction, which we don't have evidence of yet (the docs
bugs that caused this investigation were about inversion, not about
authoring friction). The recipe-table edit is a 30-minute addition
to `docs/methods/cooling.md` and can ship any time.

If you want to land *something* from this proposal alongside
decibans, pick **Change 1** (endpoint display). It's the most user-
facing, the most persistent payoff, and the lowest implementation
risk. Skip Changes 2 and 3 for now unless they come up organically
later.
