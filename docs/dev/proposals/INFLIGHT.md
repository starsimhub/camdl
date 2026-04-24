# In-flight proposals — staging to unblock the book agent

**Date:** 2026-04-24
**Purpose:** Snapshot which proposals in `docs/dev/proposals/` are
shipped, queued, deferred, or discussion-only, so that downstream
agents (primarily the camdl-book author working on the he2010
analysis) can see what's safe to rely on and what's still in motion.

Not a proposal itself; this file is an index. Replace / update in
place as proposals move between states.

---

## Shipped (fully landed, safe to build on)

These are done. The book agent can use any of these without caveat.

| Proposal | Commit(s) | What it gives the book author |
|---|---|---|
| `2026-04-23-external-validation-harness.md` | 426f272, e630fd7, 672540a, others | L9 regression layer catching camdl-vs-pomp divergence. Means a refined fit can be sanity-checked against pomp at matched config; any drift in forward-simulation or pfilter log-lik shows up in `cargo test --test external_validation`. |
| `2026-04-24-profile-cas-integration.md` (v1) | ea91535, 6b2bed4, 5f57260, 8e9e271, 06f565b | `camdl profile` writes to the CAS tree, resume-on-crash, mid-run plotting from `profile.tsv`. The 2D profile-likelihood surface for he2010 can now survive the 12-hour wall-time risk. |
| `2026-04-11-dimensional-analysis.md` (Phase 1 + 2) | earlier | `'per_year` / `'days` / etc. on forcings and tables, checked at compile time. Model authoring is safer. |
| Progress flag (`--progress plain`) | 75230d7, 153044d | Agent-readable per-chain progress in long fits; `plain` mode emits `log::info!` lines after the auto-bump. |
| Cooling docs corrected (incident fix) | 62da628 | All four doc surfaces agree; `docs/methods/cooling.md` is canonical. Scout = cooling 0.70 (mild), refine = 0.05 (aggressive). |

## Queued — approved, ready to implement

The book agent should plan around these as landing imminently, but
not yet available.

### `2026-04-23-evidence-in-decibans.md` (evidence in decibans)

**State:** Approved for implementation; minimal single-session
change per the proposal's §Implementation sketch.

**What lands:** `Δlog-lik` in nats alongside decibans + Jeffreys
qualitative label in every model-comparison surface (`camdl compare`,
fit-stage summaries, external-harness failure messages with log-lik
stats). Roughly 60-line helper at `cli/src/evidence.rs`, 5–8 call
sites, tests on label boundaries. Raw absolute log-likelihoods stay
nats-only (they don't carry evidential meaning on their own).

**Why the book agent wants this:** Gives every `Δlog-lik` in the
he2010 analysis narrative an interpretive layer — "+27 nats (+118
dB, decisive)" reads directly as weight-of-evidence, no separate
footnote about Jeffreys scales needed.

**Blockers:** None.

**Plan:** Ship this next. Aim for one focused session: helper +
call sites + tests + one-paragraph stub in the book's model-
comparison chapter pointing at the proposal. Full book chapter on
decibans is a follow-up.

### `2026-04-20-prequential-evaluation.md` (prequential / preq)

**State:** Partially implemented (`camdl pfilter --save-prequential`
exists). The narrative machinery — `camdl compare` with preq scores,
Δpreq decibans, book chapter framing — is the remainder.

**What lands:** Uniform prequential log-score / CRPS / PIT-coverage
output from pfilter; comparison of two fits on held-out tail via
Δpreq at the per-step + aggregate level.

**Why the book agent wants this:** The He et al. analysis will want
out-of-sample validation; preq is the right framework. Also pairs
naturally with decibans (Δpreq in dB is directly interpretable).

**Blockers:** Evidence-in-decibans proposal should land first so
preq uses the same dB display convention.

## Queued — presentation polish, no lift to scientific pipeline

### `2026-04-24-cooling-presentation.md` (just authored)

**State:** Proposal in review. Three changes, prioritised internally.

**Recommendation:** Hold for now (the docs were what actually
unblocked cooling; the presentation improvements are polish).
Reconsider after evidence-in-decibans lands. If picking one change
to ship alongside other work, pick Change 1 (endpoint-reduction
display next to cooling in validation/runtime output) — half a
session, highest persistent payoff.

**Why the book agent doesn't need this immediately:** The doc fix
in commit 62da628 already gave them correct scout/refine cooling
semantics. Presentation polish doesn't move the pipeline.

## Deferred — explicit hold, decision pending

### `2026-04-23-nightly-external-regen.md` (nightly external regen)

**State:** Proposal written, deliberately not implemented. Design
decisions captured so whoever picks it up next session isn't
re-deriving them.

**Why deferred:** No nightly CI infrastructure yet; the L9 fast path
already catches regressions on every PR. Nightly regen is for
detecting reference-tool (pomp) drift, which is a slow concern
worth one evening when a nightly CI slot is free.

**Not blocking anything.**

### Vignette fit.toml cooling updates (§6.5 of the cooling incident)

**State:** Not yet done. Scientific change, not doc cleanup.

Files with inverted cooling direction:
- `camdl-book/vignettes/he2010/fit_synthetic.toml` (scout=0.9,
  refine=0.97)
- `camdl-book/vignettes/he2010/fit_synthetic_fixed_s0.toml` (same)
- `camdl-vignettes/he2010-inference/fit_he2010*.toml` (scout=0.9,
  refine=0.95)

**What the book agent needs to decide:** Whether to bundle the
cooling correction into the next he2010 rerun (recommended — the
decibans proposal lands, cooling corrected, rerun once, all
analyses benefit) or treat as separate cleanup. Changing cooling
mid-analysis confounds any comparison to earlier results.

**Empirical sanity check before committing to the code-default
values:** If the current vignette refine (`cooling=0.95/0.97`) is
merely gentle-but-functional rather than catastrophically under-
converged, the rerun may find minimal difference. Evidence to
gather: in a current fit_he2010 run, is refine's `best_loglik`
meaningfully better than scout's? If yes, cooling is functional.
If within a few nats, it's too gentle and the correction will
materially improve convergence.

### `2026-04-18-ic-free-inference.md` (IC-free inference)

**State:** Implemented and documented. The flag is wired.

**Why it's on this list:** So the book agent knows the option
exists if they need to model he2010 without committing to a
specific initial state — helpful when the Bayesian story wants to
marginalise over IC uncertainty.

## Discussion-only — no code, no timeline

### `2026-04-21-malaria-model-features.md`, `2026-04-21-vital-dynamics.md`

Feature roadmaps for future DSL extensions (malaria-specific,
vital-dynamics patterns). Not a he2010 concern.

---

## What the book agent should treat as the actionable stack

Two queued items, in order:

1. **evidence-in-decibans** — next up, single session. After this, `camdl compare` and every Δlog-lik surface have dB framing. Book chapter on model comparison can write directly against the shipped UX.

2. **vignette fit.toml cooling updates** — bundle with the next he2010 rerun. Before that, spend one run confirming whether the current refine actually converges (check `best_loglik` differential between scout and refine).

Everything else (cooling presentation, nightly regen, further
profile work) is polish or deferred and doesn't need to block the
book agent.

## Index of incidents (for pattern-matching)

Silent wrong-answer incidents documented this session — useful for
the book agent to recognise if they see analogous symptoms:

- `docs/dev/incidents/2026-04-23-iota-toml-unit-silent-miscast.md` — param TOMLs are untyped floats, unit conversion mistake undetectable until cross-validation against pomp
- `docs/dev/incidents/2026-04-23-forcing-rescale-double-conversion.md` — `'per_year` forcings auto-rescaled, explicit user `/ 365.25` applied a second time silently
- `docs/dev/incidents/2026-04-24-cooling-semantics-docs-drift.md` — four doc surfaces disagreed with each other and with the code on cooling direction; inverted mental model propagated into vignette configs
