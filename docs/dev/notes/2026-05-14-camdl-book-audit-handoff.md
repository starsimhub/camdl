# Camdl-book agent handoff: 2026-05-12 audit remediation

Date: 2026-05-14
Audience: camdl-book agent
Source branch: `compartmental@main`, commits 777a17f..65fc167 (17 fixes)
Audit: `docs/dev/reviews/2026-05-12-full-audit.md`
Proposal: `docs/dev/proposals/2026-05-13-pre-alpha-audit-remediation.md`

---

## Self-contained briefing

The camdl pre-alpha audit (8 Critical / 14 High / 21 Medium findings)
has been substantially remediated on `main`. 17 fixes landed across
six sprints over a single session. This note hands off to camdl-book
the work *the book agent owns*: pulling the changes, re-rendering,
validating production configs (typhoid SIRC + polio cVDPV2) against
the new strict-mode behaviours, and updating any chapters whose code
samples assume the old IR shape or silent-fallback semantics.

**Read the proposal at `docs/dev/proposals/2026-05-13-pre-alpha-audit-remediation.md`
before starting.** It's 1000+ lines but explains *why* each fix landed
the way it did. The audit is the *what*; the proposal is the *why*.

---

## What changed (grouped by impact on book content)

### IR contract bumped 0.3 → 0.4 (commit 80e0221, audit C8)

**The big one for the book.** Every IR JSON file is now wrapped:

```json
{
  "ir_version": "0.4",
  "validated_by": "ocaml-compiler-v0.4",
  "model": { /* the existing model body */ }
}
```

Rust's `ir::from_str` enforces the envelope — bare-Model JSON now
errors with `IrError::VersionMismatch { expected, found }`. OCaml emits
the envelope automatically.

**Book impact:**
- Any chapter that shows raw IR JSON (probably in the architecture /
  internals chapter) needs the envelope wrapper added to the example.
- Any code example that does `serde_json::from_str::<Model>(&json)`
  needs to switch to `ir::from_str(&json)`.
- Hand-curated `.ir.json` fixtures referenced by the book need the
  envelope wrapper. Use `jq '{ir_version:"0.4", validated_by:"hand-curated", model:.}' in.json > out.json`.
- The book's earlier "the IR schema is the contract" claim is now
  *true* — call this out explicitly. There's now a real version
  handshake; previously it was aspirational.

### Numerical-collapse paths now error by default (commits 363d7ba + 3270ef7, audit C5/C6/S1/S2)

Production models with empty-stratum-divisor patterns (e.g.
`beta * I[a] / N_local[a]` when `N_local[a] = 0`) used to silently
return 0; now they return `SimError::NumericalCollapse`. Negative
compartment counts (binomial overshoot) used to silently clamp to 0;
now `SimError::NegativeCount`.

**Book impact:**
- **Run the typhoid SIRC config.** If it produces non-zero
  `degenerate_step_count` in the new EvalStats summary (printed at
  end of every `cmd_*`), the model has empty-stratum issues that the
  silent-zero was hiding. Either:
  - Add explicit `Cond` guards: `cond(N > 0, I/N, 0)`, OR
  - Pass `--allow-degenerate-rates` (mirrors the legacy behaviour).
- **Run the polio cVDPV2 spatial config.** Spatial models with empty
  patches are exactly the case the audit warns about. Same triage.
- For inference fits (PGAS / IF2), per-particle recovery in the
  particle filter catches `NumericalCollapse` and converts to −Inf
  for the offending particle. Fits keep running, but the
  EvalStats summary now reports how often the fallback fired.
- The `--allow-degenerate-rates` flag is documented in the proposal §1
  C6 and is available on `simulate`, `pfilter`, `if2`, `fit run`.

### PGAS now refuses to run without explicit priors (commit 3f4bbd3, audit C4)

`[estimate.beta]` with no `[estimate.beta.prior]` block and no `~`
prior in the model used to silently use `Prior::Flat` (improper
uniform). Now PGAS errors at startup, listing the offending parameter
names.

**Book impact:**
- Any fit.toml example in the book that estimates a parameter without
  declaring a prior will now fail PGAS. Either add `prior = { ... }`
  blocks (preferred — explicit Bayesian) or call out the missing-
  prior error and explain it's intentional.
- For tutorial purposes where uniform-on-bounds is the right answer:
  use `prior = { uniform = { lower = ..., upper = ... } }` to make
  the choice explicit.

### IF2 / PMMH MLE selection / cooling / acceptance changes (commits 645d8fb + 65fc167, audit C2/M2/M4)

- IF2 `result.mle` is now the *last iteration's* filter mean (correct
  per IF2 cooling theory), not the iteration that maximised the
  perturbed loglik (audit C2). Anything that consumed `IF2Result.mle`
  directly was previously biased; the CLI's `winner_theta` workaround
  via clean-eval re-scoring already routed around this.
- IF2 cooling formula: `(1 + n_obs)` instead of `n_obs` (audit M2).
  Cooling now matches the documented half-life behaviour exactly. For
  models with small `n_obs` (e.g. weekly data over 1 year, n_obs=52),
  effective cooling is now ~2% slower than before — negligible. For
  `n_obs = 1` (toy examples), cooling is now half as fast as before.
- PMMH acceptance_rate now reports post-burn-in acceptance (audit M4).
  Numbers in the book's PMMH chapter for production fits will shift
  slightly.

**Book impact:** mostly invisible. Re-render and check that any
explicit acceptance-rate / cooling-rate text matches new outputs.

### Diagnostics now actually fire (commits f364a46 + 024c400 + 1d48fc4, audit C7/H4/H5/M18)

Seven `DiagnosticKind` variants previously had full render plumbing
but were never constructed. Now wired:
- `DivergentTransitions` (PGAS, post-burn-in)
- `MaxTreeDepthHits` (PGAS, > 5% post-burn-in)
- `LowSwapRate` (tempering pairs < 10%)
- `DegenerateAncestorSampling` (CSMC > 10% degenerate)
- `LowTrajectoryRenewal` (mean post-burn renewal < 10%)
- `LowESSAtMLE` (clean-eval ESS < 5% of particles)
- `ParamNearBound` (chain θ̂ within 1% of estimated bounds)

`EvalStats` counters surface at the end of every `cmd_*` run when
non-zero.

**Book impact:** the diagnostics chapter (if any) should mention
these, or be lightly re-validated. Re-rendering example outputs may
now include diagnostic warnings that weren't there before.

### CLI flag enforcement (commit f27b73f, audit H12/H13)

- `--record-prequential` / `--record-ancestry` now error if used with
  a non-PFilter stage (previously silently no-op).
- `--parallel N` now actually parallelises `camdl pfilter --replicates`
  (previously silently single-threaded).

**Book impact:** any CLI invocation in the book that combined these
flags incorrectly was already broken (silently). Re-rendering will
either succeed (if the combination was valid) or surface the new
error message.

### `balance{}` now errors on tau_leap / gillespie / ode (commit 1eff142, audit C3)

A model with `balance { ... }` is now rejected at dispatch time on
those backends with the standard capability-mismatch error. Previously
silent drop.

**Book impact:** if any book example simulates a `balance{}` model on
tau_leap or gillespie expecting it to work, it'll now error. Switch
to chain_binomial (the only backend that supports balance) or remove
the balance block.

### NUTS Algorithm 6 (commit de91b44, audit H1)

Outer-tree combine now matches Hoffman & Gelman 2014 Algorithm 6
exactly. Previously a hybrid form (close to Algorithm 3) without
documented justification.

**Book impact:** PGAS chapter mentions of "we follow H&G NUTS" are
now accurate. No behavioural change for typical fits.

### Discretized-normal CDF (commit 20087da, audit H2)

Replaced A&S 7.1.26 erf approximation (~1.5e-7 max abs error) with
`libm::erfc` (full f64 precision). Tail observations (rare-event
surveillance like AFP) used to be dominated by 1e-7 noise; now
properly resolved.

**Book impact:** likely improves polio inference numbers slightly.
Re-render and check whether AFP chapters' headline numbers shift.

### CSMC ancestor sampling (commit 3691b31, audit H8)

Pre-resample state cache used for ancestor weights. Previously the
post-resample shuffle silently relabelled the ensemble that ancestor
sampling categoricalised over. On observation-tight steps with
heterogeneous patch prevalences, the wrong ancestor index could be
selected.

**Book impact:** spatial-model fits in the book may produce slightly
different posteriors. Re-render and check whether chapter numbers
need updating.

---

## Action items for the book agent

In priority order:

1. **Pull `compartmental@main`** to your worktree and re-render the
   full book. The first re-render will surface anywhere the new
   strict-mode behaviour breaks an example.

2. **Audit production configs.** Run typhoid SIRC and polio cVDPV2
   end-to-end (the `experiments.qmd` chapter is the canonical
   entry point per the existing book convention). Check the
   EvalStats summary at the end of each run for non-zero counters.
   Document any model that legitimately needs `--allow-degenerate-
   rates` and call it out in the chapter; fix any model that has
   a real bug (most likely an empty-stratum divisor missing a
   `Cond` guard).

3. **Update IR JSON examples.** Any chapter showing raw IR (likely
   in the architecture / IR-internals chapter) needs the envelope
   wrapper. The OCaml frontend emits it automatically; only
   hand-curated examples need updating.

4. **Update fit.toml examples missing priors.** If any tutorial
   `[estimate.X]` has no prior, either add one (preferred) or add
   a sidebar explaining the new "no prior, no run" behaviour.

5. **Run the diagnostic experiment** referenced in CLAUDE.md
   (typhoid SIRC: nl-sbplx vs if2 MLE on the smallest stratum
   cell). The C1 preflight gate now blocks PGAS from running with
   estimated obs-likelihood / σ² parameters; if the typhoid fit
   uses `rho` or `psi` as estimated params, the preflight will
   reject the run. Workaround: move them to fixed params or fit
   them with IF2 first (non-gradient method).

6. **Surface the new `--allow-degenerate-rates` flag** in the
   troubleshooting chapter (if any) — it's the escape hatch for
   the C5/C6 strict-mode behaviour.

7. **Cite the audit and proposal** in the changelog / release-notes
   chapter once the book references the new behaviour. Files:
   - `docs/dev/reviews/2026-05-12-full-audit.md`
   - `docs/dev/proposals/2026-05-13-pre-alpha-audit-remediation.md`

---

## What didn't land (book doesn't need to worry yet)

These items are documented in the proposal as deferred to follow-up
sessions. The book can ignore for now:

- **C1 full** — extending OCaml `autodiff.ml` to emit obs-likelihood
  derivatives. The preflight gate (C1 partial) is in place as a
  backstop, so the silent-bias hole is closed; the full gradient
  threading is a week of compiler work. PGAS users who need to
  estimate `rho`/`psi` use the preflight gate's hint to switch to
  IF2 / PMMH.
- **H6 / H7 / M12 / M13 / M14 / S3** — Sprint 4 follow-ups: enum
  tightening, dead-field deletion, init validation. Quality-of-life
  improvements; defer.
- **H9 / H10 / H11** — DRY cleanup + selective newtypes. Refactor
  with no user-facing surface change.
- **M1, M3, M5, M7, M15, M17, M19, M20, M21** — small numerical /
  testing items; defer.

---

## Surface back if you find

1. A book example that breaks under the new strict-mode behaviour
   in a way that *isn't* covered by `--allow-degenerate-rates` or
   an obvious `Cond` guard fix.
2. A typhoid or polio production config that produces large
   `degenerate_step_count` (> 1% of substeps) — that's a model
   issue worth investigating.
3. A diagnostic that fires unexpectedly often — the thresholds
   (5% post-burn `MaxTreeDepthHits`, 10% `DegenerateAncestorSampling`,
   etc.) were chosen from Stan / pomp conventions; if production
   models routinely cross them, the thresholds may need tuning.
4. Any IR JSON file in the book's tree that I missed (the C8
   handoff regenerated the compartmental repo's goldens, but the
   book may have its own copies).

If anything blocks the book re-render and isn't documented in the
proposal, file an issue or write back.
