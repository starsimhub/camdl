# Incident: IF2 ignored `EstimatedParam::initial`; v2 dispatch didn't build per-chain random starts

**Date:** 2026-04-18
**Status:** Fixed.
**Severity:** High. Every multi-chain IF2 run in the v2 dispatch
(`camdl fit run`) had all chains effectively starting from the same
point, with per-chain divergence coming only from the perturbation
RNG on a shared `base_params`. This made Rhat-across-chains
uninformative — between-chain variance reflected RNG noise, not
genuine independence-of-starts. Scout's v1 subcommand (`camdl fit
scout`) carefully constructed random per-chain `EstimatedParam`
arrays but IF2 never consulted the per-chain `.initial` values, so
even v1 had decorative-only "random starts."

## Fundamental vs. implementation

**Fundamental:** nothing. Running N chains from N random
starting points is the standard way to diagnose independence-of-
starts via Rhat; the documented intent of `chains = N > 1`.

**Implementation:** two stacked bugs.

1. `EstimatedParam::initial` was never read by
   `sim::inference::if2::run_if2_with_progress`. The function
   constructed its starting particle cloud from `base_params.to_vec()`
   (if2.rs:338) and then used `if2_params[i]` only for perturbation
   metadata (rw_sd, transform, bounds, index). So even when a
   caller supplied per-chain `EstimatedParam` arrays with divergent
   `.initial` values — as v1 scout did — IF2 started from the same
   `base_params` in every chain.

2. The v2 `camdl fit run` dispatch in `fit/mod.rs` passed `None` for
   `per_chain_params` to `run_chains_with_per_chain_params`. So even
   the decorative-only per-chain `.initial` values weren't built —
   `chain_starts.tsv` showed identical rows for every chain (which,
   in retrospect, was MORE honest about what IF2 was actually doing
   than v1 scout's divergent-looking file).

Either bug alone would have been survivable. Together they ensured
that (a) v2 never even pretended to randomise chains, and (b) even
if it had, IF2 wouldn't have seen the randomness.

## Summary

Downstream noticed that `chain_starts.tsv` for a v2 SIR fit with 32
chains showed the same `(β, γ, k)` values on every row and flagged
it as "v2 dispatch skips random chain init." That diagnosis was
correct but missed the deeper issue: v1 scout's per-chain random
`.initial` values were also never reaching IF2. Both bugs are fixed
in the same commit.

## Evidence

Before the fix, a two-chain SIR fit with `chains = 2`,
`[estimate] beta = { bounds = [0.01, 5.0], start = 1.0 }`:

```
# chain_starts.tsv
chain   beta    gamma
1       0.903   0.425     # seeded (base_params[beta] via transform)
2       0.903   0.425     # identical — no random build in v2
```

After the fix, same config:

```
chain   beta     gamma
1       0.903    0.425     # seeded start (chain 0 reproducible)
2       3.741    0.515     # random from bounds
```

And crucially the iter-0 rows in `chain_{N}/parameter_traces.tsv`
now reflect those different starts — chain 2 starts from β ≈ 3.68
(post-first-perturbation of β=3.74) with loglik −1417 (different
basin), vs chain 1's β ≈ 0.97 with loglik −29.5. Before the fix,
both chains would have post-perturbation iter-0 β close to ~0.9
with comparable logliks.

## Blast radius

- **Every multi-chain IF2 run via `camdl fit run`** had
  unrealistic Rhat. Apparently-converged fits (low Rhat) had
  genuinely converged in the sense that chains stayed close to
  each other, but the diagnostic couldn't distinguish "converged
  from diverse starts" from "all started close and slowly drifted
  apart."
- **v1 `camdl fit scout`** was intermediate: it built per-chain
  random `.initial` values (so `chain_starts.tsv` looked right) but
  IF2 ignored them (so all chains actually started from
  `base_params`). The visualization lied about the algorithm; the
  algorithm's chain diversity came entirely from the per-chain
  perturbation RNG on a shared base.
- **`starts_from = "scout"`** stage handoffs: unaffected by this
  bug (all chains correctly start from scout's MLE, which was the
  intent). The `starts_from = "scout"` handoff was separately
  broken by a priority-inversion bug fixed earlier the same day
  (see `2026-04-18-starts-from-scout-ignored.md`); that fix is
  independent of this one.
- **PGAS / PMMH** stages: not affected. Their initial-state
  construction doesn't go through `run_if2_with_progress` and has
  its own per-chain init path.
- **Single-chain fits** (`chains = 1`): unaffected. No chain
  diversity was expected; `.initial == base_params[idx]` for the
  sole chain, so the fix is a no-op.

## How we found it

Downstream ran a `camdl fit run` with 32 chains on the SIR model
and expected `chain_starts.tsv` to show 32 distinct rows spanning
the declared bounds (since no `starts_from` was set). The file
showed 32 identical rows. Greping the v2 IF2 dispatch in
`fit/mod.rs` showed `run_chains_with_diagnostics`, which internally
passes `None` for `per_chain_params`.

The deeper bug (IF2 ignoring `.initial`) surfaced when I checked
whether porting the v1 scout's per-chain builder into v2 would
*actually* fix it, or just make `chain_starts.tsv` look correct.
Tracing through `run_if2_with_progress` showed `current_params =
base_params.to_vec()` with no subsequent read of `if2_params[i].initial`.

## The fix

Two changes, both in this commit:

1. **`sim/src/inference/if2.rs`, `run_if2_with_progress`:** after
   initialising `current_params` from `base_params`, overwrite each
   estimated-parameter slot with the corresponding
   `EstimatedParam.initial`. For single-start fits
   `.initial == base_params[idx]` so it's a no-op; for multi-chain
   fits with random per-chain `.initial`, IF2 now starts from each
   chain's declared point.

2. **`cli/src/fit/runner.rs`, new `build_random_chain_starts`:**
   extracts the per-chain random-start policy from v1 scout into a
   callable helper. Chain 0 keeps the seeded start (reproducibility);
   chains 1..N draw uniformly from bounds, or jitter ±50 % of the
   seeded start for unbounded params. `fit/mod.rs`'s IF2 dispatch
   calls this helper when `effective_starts.is_none() && chains > 1`
   and threads the result through `run_chains_with_per_chain_params`.

The `chain_starts.tsv` writer now correctly records the per-chain
random starts (when they exist) or the single seeded start (when
`starts_from` is set or chains == 1).

## Regression test

`rust/crates/cli/tests/synthetic_fit_grid.rs::
v2_if2_chains_diverge_at_iter_0_when_no_starts_from`:

1. Run `camdl fit run` on a toy SIR with `chains = 8`,
   `[estimate] beta = { bounds = [0.01, 5.0], start = 1.0 }`, no
   `starts_from`.
2. Assert `chain_starts.tsv` has 8 rows with β values spanning
   > 1.0 (out of the 4.99-wide bounds).
3. Assert chain 1 and chain 8 iter-0 β in `parameter_traces.tsv`
   differ by > 0.3 — proves IF2 actually saw the different starts,
   not just per-chain RNG noise on a shared base.

The second assertion is the load-bearing one: per-chain RNG on a
shared base would give an iter-0 spread of order `rw_sd ≈ 0.03`,
far below the 0.3 threshold. A spread of 0.3+ is only achievable if
IF2 consulted `.initial`.

## How did this escape

- **`EstimatedParam::initial` looked authoritative.** It's a
  public field, set carefully by every build path, named `initial`,
  documented in comments. Nothing about the API signalled that it
  wasn't consumed. A reader would naturally assume IF2 reads it —
  exactly what happened when I first traced the `starts_from`
  priority-inversion bug earlier the same day and noted "the
  `.initial` override is dead code" as an aside in that fix's
  incident report. At the time I didn't realise the impact was
  "multi-chain IF2 starts are collapsed."
- **v1 scout's `chain_starts.tsv` was plausibly-correct.** It
  showed divergent values per chain because it read
  `per_chain_params[i].initial`, and nobody cross-checked against
  iter-0 trace rows to confirm IF2 had actually used them.
- **The v2 dispatch was a clear miss-port** that would have been
  obvious had anyone looked at `chain_starts.tsv` on a v2 run.
  That file only landed earlier today (commit `f027390`), and the
  same session's review surfaced the bug — which is how it's being
  caught now rather than six months from now.

## Followup actions

- **Done in this fix:** both bugs, regression test, this report.
- **Not done; worth considering:**
  - **Audit `EstimatedParam` field consumption more broadly.** Are
    there other fields that look authoritative but aren't read by
    the algorithm? `rw_sd_auto` for instance — is that consumed or
    informational? A short sweep would give confidence that the
    struct's API matches its reality.
  - **Add a second spread-test on scout v1 specifically** once the
    v1 `camdl fit scout` subcommand is either consolidated into v2
    or deleted. Until then, scout v1 users benefit from the if2.rs
    `.initial`-authoritative fix too, but we don't have a test
    covering the v1 path directly.

## Lessons

- **"Decorative correctness" is the dangerous failure mode.**
  `chain_starts.tsv` showing divergent values in v1 scout was worse
  than it showing identical values would have been, because it
  masked the real algorithm behaviour. The v2 file showing identical
  rows actually surfaced the underlying bug — honest instrumentation
  beats polished instrumentation every time.
- **"Dead code that looks alive" is an escape vector for bugs.**
  `EstimatedParam::initial` had every signal of being consumed
  (named well, set carefully, written by every build path) but was
  never read. When I noted this as an aside in the earlier
  `starts_from` incident, I should have opened a ticket immediately
  rather than moving on. The fix took an hour; deferring it until
  a user complained cost (a) downstream's diagnostic trust in
  camdl's multi-chain machinery, (b) a re-run of every book-chapter
  fit once this lands.
- **Cross-file parity tests matter as much as unit tests.** Scout's
  unit tests checked that random per-chain `.initial` values were
  constructed correctly; IF2's unit tests checked that its
  perturbation logic was correct given starting params. No test
  checked that the values flowed from one to the other. This is
  the third incident in two days where the gap was at the
  composition boundary — see also the SBC seed-mix divergence
  (2026-04-18) and the `starts_from = "scout"` priority inversion
  (also 2026-04-18). Worth internalising as a principle.
