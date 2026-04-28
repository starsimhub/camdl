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
| `2026-04-23-evidence-in-decibans.md` | this commit | `camdl compare` now emits an `evidence` column alongside `Δelpd`: dB + Jeffreys label (`substantial`/`strong`/`decisive`/`overwhelming`) so reviewers can parse evidence magnitude without computing exp(Δelpd) in their head. JSON output gains `delta_elpd_db` + `evidence_label` fields. Helper `cli/src/evidence.rs` is the single source of truth for the conversion + Jeffreys scale; Unit A compound gate (next) reuses it for cross-chain spread reporting. |
| `2026-04-24-profile-cas-integration.md` (v1) | ea91535, 6b2bed4, 5f57260, 8e9e271, 06f565b | `camdl profile` writes to the CAS tree, resume-on-crash, mid-run plotting from `profile.tsv`. The 2D profile-likelihood surface for he2010 can now survive the 12-hour wall-time risk. |
| `2026-04-11-dimensional-analysis.md` (Phase 1 + 2) | earlier | `'per_year` / `'days` / etc. on forcings and tables, checked at compile time. Model authoring is safer. |
| Progress flag (`--progress plain`) | 75230d7, 153044d | Agent-readable per-chain progress in long fits; `plain` mode emits `log::info!` lines after the auto-bump. |
| Cooling docs corrected (incident fix) | 62da628 | All four doc surfaces agree; `docs/methods/cooling.md` is canonical. Scout = cooling 0.70 (mild), refine = 0.05 (aggressive). |

## Queued — approved, ready to implement

The book agent should plan around these as landing imminently, but
not yet available.

### `2026-04-24-if2-scout-findings-remediation.md` (IF2 scout fix — biggest item) 🔴

**State:** Pending upstream review; Unit A is the critical piece for
anyone continuing the he2010 analysis.

**What it addresses:** Five findings established empirically on the
he2010 synthetic recovery vignette:
1. 6-parameter scout reported a point 358 nats below truth with
   $\widehat{R} \approx 1.000$ across all 36 chains — classic silent
   wrong-answer: parameter-level agreement on a catastrophically bad
   basin.
2. Fixed-$s_0$ scout had a 40.2-nat extraction bias in the selection
   step (500-particle argmax on 36 chains → $\mathbb{E}[\max]$ ceiling
   ≈ 80 nats).
3. In-run trace log-likelihoods are PF-noise-dominated at 500
   particles (SD ≈ 30 nats), not convergence signal.
4. Parameter $\widehat{R}$ is **structurally biased toward 1** in the
   IF2 setting by construction of the cooling kernel — it's a
   genuinely different statistic from MCMC's Gelman–Rubin, deserves a
   different name ($\widehat{A}$ / `chain_agreement`).
5. Basin-quality failure is invisible to parameter-only diagnostics.

**Proposed fix, three units:**

- **Unit A (ship first):** Proposal 1 (loglik-eval selection in scout
  AND refine, multi-candidate per chain) + Proposal 3 (compound gate
  with $\widehat{A}$ + decibans-spread, with the Rhat → $\widehat{A}$
  rename). Target file: `rust/crates/cli/src/fit/runner.rs`. Days of
  work, not weeks.
- **Unit B (independent, small):** Proposal 2 — raise in-run trace
  particles 500→2000, add rolling-mean overlay to trace plots (raw
  track always retained). One-file change.
- **Unit C (separate large issue):** Proposal 4 — `--resume` and
  `--warm-restart` for MLE stages, sharing serialization with the
  existing Bayesian-stage resume infrastructure. Weeks of work; file
  separately.

**Why the book agent wants this:** This is the actual fix for the
he2010 analysis's reported biases. The current vignette shows scouts
with 40-nat extraction bias and one with a 358-nat basin failure that
the pipeline silently reports as converged. Unit A alone makes the
he2010 rerun a different (and defensible) analysis. Unit C makes the
vignette's pedagogical "iteration budget is a hyperparameter, not a
ritual" lesson reproducible with a `--resume --warm-restart` demo.

**Blockers:** None technically. Relationship to other proposals:
- Uses decibans framing for the cross-chain spread gate → pairs
  naturally with `2026-04-23-evidence-in-decibans.md` (can ship in
  either order; if decibans ships first, the gate's threshold text
  reads natively in dB without a conversion footnote).
- Cooling semantics already clarified via `62da628` + `docs/methods/cooling.md`;
  Finding 4's reasoning about PF-noise-dominated traces cites the
  corrected cf50 interpretation.
- Supersedes parts of `2026-04-19-refine-gates-scout-convergence.md`
  (Rhat-only gate) with the compound $\widehat{A}$ + decibans gate.

**Plan:** Review Unit A's proposed `fit.toml` schema additions
(`loglik_eval`, `gate`, `trace` blocks) and CLI flags before any
implementation lands. Then ship Unit A as one PR and Unit B as a
second (or bundled). Unit C files as its own issue with the shared-
resume-codepath question flagged explicitly.

### ~~`2026-04-23-evidence-in-decibans.md`~~ (shipped)

Moved to the "Shipped" table above. `camdl compare` + JSON both
carry dB + Jeffreys labels alongside the existing nats column.

Earlier proposal state (now obsolete, preserved for context):

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

Four queued items, in order. The first two are approximately equal
priority and can ship in either order; items 3 and 4 unblock the
he2010 rerun together.

1. ~~**evidence-in-decibans** — shipped in this commit.~~ `camdl
   compare` surfaces dB + Jeffreys label alongside Δelpd/nats.
   JSON output has `delta_elpd_db` + `evidence_label`. Helper
   (`cli/src/evidence.rs`) is the single source of truth — Unit A's
   compound gate reuses it for cross-chain spread formatting.

2. **IF2 scout remediation, Unit A** — Proposal 1 (loglik-eval
   selection) + Proposal 3 (compound gate with $\widehat{A}$ +
   decibans-spread, rename `rhat` → `chain_agreement`). Days of
   work; this is the load-bearing fix for the he2010 analysis. After
   this lands, the scout's reported MLE is no longer selection-biased
   on PF noise, and the gate catches "all chains agreed on one bad
   basin" which parameter-$\widehat{R}$ misses. **This is up next.**

3. **IF2 scout remediation, Unit B** — Proposal 2 (raise in-run
   trace particles to 2000; add rolling-mean overlay). Small
   self-contained UX win; ship independently or bundle with Unit A.

4. **vignette fit.toml cooling updates + he2010 rerun** — bundle with
   Unit A landing so the rerun exercises both the corrected cooling
   (0.70 scout, 0.05 refine — was 0.9/0.95 in the vignette configs)
   AND the corrected selection pipeline. Doing both in one pass
   avoids two reruns and gives a single clean baseline for the book
   chapter. Before committing: check `best_loglik` differential
   between scout and refine in the current (pre-fix) vignette run to
   quantify how much of the bias comes from cooling vs. selection.

**Deferred until after 1–4**: cooling presentation (pure polish),
nightly regen (waiting for CI slot), Unit C resume/warm-restart
(weeks of work, separate issue).

**Not in the book agent's path at all**: malaria-model-features,
vital-dynamics proposals (future DSL work, not he2010-relevant).

## Index of incidents (for pattern-matching)

Silent wrong-answer incidents documented this session — useful for
the book agent to recognise if they see analogous symptoms:

- `docs/dev/incidents/2026-04-23-iota-toml-unit-silent-miscast.md` — param TOMLs are untyped floats, unit conversion mistake undetectable until cross-validation against pomp
- `docs/dev/incidents/2026-04-23-forcing-rescale-double-conversion.md` — `'per_year` forcings auto-rescaled, explicit user `/ 365.25` applied a second time silently
- `docs/dev/incidents/2026-04-24-cooling-semantics-docs-drift.md` — four doc surfaces disagreed with each other and with the code on cooling direction; inverted mental model propagated into vignette configs
