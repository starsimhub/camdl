---
status: implemented
date: 2026-04-19
authors: upstream + downstream via agent-channel
shipped:
  - e38a845  feat(sim): ancestor-trace primitive + opt-in PF recording
  - bbfa193  feat(cli): --save-paths + --save-filtering flags
  - 9bdab82  test(cli): pfilter integration tests
  - b226ad0  docs(inference): filtering-vs-smoothing + diagnostic plot
---

# Expose PF latent trajectories + clarify the right "does model fit data?" comparison

## TL;DR

Add two output options to the existing `camdl pfilter` subcommand,
with **distinct scientific purposes** (not two ways to dump the
same thing):

- `--save-paths N` — N ancestor-traced samples from the smoothing
  distribution `p(x_{1:T} | y_{1:T}, θ)`. **The modeller's tool.**
  What you plot against data to ask "does the fit explain the
  observations?"
- `--save-filtering` — per-step particle states + log-weights.
  **The PF diagnostic tool.** What you reach for to check particle
  degeneracy, obs-model sanity, filter implementation correctness.
  Explicitly NOT a model-vs-data plotting object; emits a mandatory
  info log on every run clarifying this.

And: document the **two-panel diagnostic** the book chapter should
use for "does the fitted model match the data?" — unconditional
posterior predictive on one panel, smoothing over latent on the
other, data overlaid on both. The *divergence* between the two is
the diagnostic; neither panel alone answers the question.

## Motivation

The immediate trigger: downstream is writing a fitting chapter that
shows a posterior-predictive ribbon from the MLE of a plain-SIR fit to
the boarding-school data. The unconditional forward-sim envelope
**misses the rising limb** — day 4–5 observations sit above the 95%
envelope — yet the PF log-likelihood at those parameters is −61.1.

A 228-nat gap between the PF marginal log-likelihood (−61.1) and the
deterministic Poisson log-likelihood at the same θ (−296) is the
anomaly. The user's instinct: "maybe the data-conditional latent
paths from the PF DO fit the observations, and the right diagnostic
is to compare latent paths to data, not to compare unconditional
predictions to data."

That instinct is half-right, and the half that isn't needs care —
getting it wrong would teach readers of the book a subtly broken
framework for reading fit diagnostics. This proposal writes down the
correct framework and adds the tooling to support it.

## Scientific framing

### Three distinct distributions, three different questions

Given fitted parameters `θ` and observed data `y_{1:T}`:

**(1) Unconditional posterior predictive** — `p(x_{1:T} | θ)`, i.e. the
model's forward-simulation distribution given `θ` alone, no conditioning
on observations. Quantile ribbon over `I(t)` from `camdl simulate
--replicates N` at the MLE.

*Answers:* "Under the fitted model, what trajectories would it produce
a priori?"

**(2) Smoothing distribution over latent state** — `p(x_{1:T} | y_{1:T}, θ)`,
the latent paths conditional on ALL observations. Quantile ribbon over
ancestor-traced sample paths from the PF.

*Answers:* "Given the data, what does the model think the latent
trajectory was?"

**(3) Filtering distribution** — `p(x_t | y_{1:t}, θ)` at each `t`,
conditional only on observations **up to** `t`. Quantile ribbon
over per-step PF particles (before resampling, weighted by filter
weights).

*Answers:* "If we were running the filter online, what would it
believe about `x_t` after seeing `y_{1..t}`?"

All three ribbons exist for the same `(model, θ, data)`. They are
not the same distribution. Plotting any of them and saying "this is
the fit" is ambiguous unless you're specific about which.

### The diagnostic that the book chapter should use

For "does the fitted model match the data?" the canonical plot is
**(1) and (2) side-by-side**, both with the raw observations
overlaid:

- **If (1) and (2) roughly agree and both track the data**: the model
  is well-specified and inference succeeded. The latent the PF
  reconstructs is the same latent the model would produce a priori
  at the MLE — no tension.

- **If (2) tracks the data but (1) misses it**: this is the anomaly
  downstream hit. The model's *unconditional* dynamics at the MLE
  don't reproduce the observed shape, but the PF found *some* latent
  paths (in the tail of the process-noise distribution) that do
  match the data. The PF log-likelihood is high not because the
  model predicts well but because the model is flexible enough to
  thread through any data via process-noise fluctuations.
  **This is diagnostic of over-flexible process noise papering over
  structural mis-specification** — exactly the pathology the 228-nat
  Poisson-vs-PF gap is pointing at.

- **If (1) encompasses the data but (2) is way tighter**: inference
  succeeded (PF correctly identified which latent paths are
  consistent with data), model is well-specified, and the data are
  merely informative about the latent. The normal case.

- **If (1) and (2) both miss the data**: fit failed. θ is wrong.

### Why (2) alone is not "the right comparison"

The user asked whether the comparison for "model matches data"
should be "latent paths themselves, not the probability of
observations given θ." Careful answer:

**No — neither alone is sufficient; the DIVERGENCE between (1) and (2)
is the diagnostic.**

(2) alone is a weak test: every flexible enough state-space model has
*some* latent paths consistent with *any* data set, because the PF
will always find them if the process noise is large enough. Plotting
(2) against the data and saying "look, it fits" proves nothing about
the model — it proves the PF found the tube the data lives in, which
it must by construction.

(1) alone, when it misses the data but the model's PF log-likelihood
is high, is what made the chapter's current framing misleading: a
reader sees "the fitted ribbon misses the data" and concludes "the
fit is bad" when the PF log-likelihood says otherwise. The resolution
is not "use (2) instead" — it's "show both (1) and (2), and when they
disagree, TEACH the reader what that disagreement means."

### Implication for the book chapter's current figures

Replace the current one-ribbon figure (unconditional PP) with a
two-panel or overlaid-ribbon figure:

- Panel A: unconditional posterior predictive `(1)` + data.
  Caption: "What the fitted model predicts a priori."
- Panel B: smoothing `(2)` from PF + data.
  Caption: "What the model thinks the latent trajectory was given
  the data."

Then a paragraph of prose that walks the reader through what the
disagreement means: process noise is doing work the deterministic
skeleton isn't. Point forward to the negative-binomial obs model
section as one structural fix, and to the Erlang / time-varying-β
section as the other.

This turns the anomaly from "my ribbon doesn't match the data,
bug?" into "here is a standard diagnostic and what it teaches us"
— a pedagogically better story than either "it fits" or "it
doesn't."

## Feature specification

### Extending `camdl pfilter`

The existing subcommand already has `--obs`, `--output`,
`--save-final-state`. Two new flags:

```
camdl pfilter MODEL.camdl \
    --params mle_params.toml \
    --data boarding_school.tsv \
    --n-particles 5000 \
    --seed 42 \
    --save-paths 200                 # smoothing — recommended default for plots
    --save-filtering filter.tsv      # filtering marginals — online diagnostics only
```

Both flags are optional and independent; a single `camdl pfilter`
run can emit one, the other, or both.

### `--save-paths <N>`: smoothing draws via ancestor tracing

Semantics: at the final observation step, sample `N` particles with
probability proportional to their final weights, walk each selected
particle's ancestor chain back to `t = 0`, emit the resulting
trajectory as a sample path.

Output format (TSV, matches `camdl simulate --replicates N`
convention so downstream polars pipelines compose unchanged):

```
path    time    S       I       R       ...compartments...
1       0       999     1       0
1       1       996     4       0
...
1       14      102     15      883
2       0       999     1       0
2       1       997     3       0
...
```

Each `path ∈ 1..N` is **one sample from `p(x_{1:T} | y_{1:T}, θ)`** —
equally weighted (no log_weight column needed). Quantile ribbons
computed by grouping on `time` and taking quantiles over `I` (or
whatever compartment) are smoothing-marginal quantiles by
construction.

Disk cost for the motivating case: 200 × 14 × 3 × 8 B ≈ 70 KB.
Trivial.

Default value: `--save-paths 200` is a good starting point (matches
the replicate count already used in book-chapter forward sims).

### What `--save-filtering` is actually for

Filtering marginals answer "what did the filter believe about `x_t`
after seeing `y_{1..t}`?" That question is rarely the one a modeller
asks when comparing a fit to data — smoothing paths (§above) are
the right answer to "what does the model think the latent
trajectory was given all the data?"

But filtering-state snapshots are genuinely useful for a separate
class of use cases — all of them **diagnostic or debugging**, none
of them "tell me about the latent":

- **Particle degeneracy / ESS-over-time.** Watch effective sample
  size decay across observation steps to detect the filter
  collapsing into a small number of lineages before the end of the
  series. Symptoms: ESS drops near zero at some `t`, then the
  filter's log-likelihood estimate becomes noisy from that point
  forward.

- **Sequential-tightening plots.** Show how the filter's marginal
  belief about `x_t` narrows as observations accumulate. Useful for
  teaching what "data-conditional inference" means in the online
  case; not the same as "does the fit match the data" (which is
  the smoothing question).

- **Debugging PF implementation correctness.** Does the filter
  produce the right weights at step 1 under a known obs model? Are
  resampling indices sensible? Is there numerical underflow in the
  log-weight accumulator? Having raw per-step particle states +
  weights is how you answer these; smoothing paths abstract the
  machinery away.

- **Observation-model sanity checks.** Plot each particle's
  `log p(y_t | x_t^i, θ)` distribution at step `t`; a bimodal
  distribution may indicate the obs model is picking out a small
  subset of particles as plausible (filter working) or that the
  obs model is too sharp / too flat (obs model mis-specified).

None of these are what the book chapter wants. The chapter wants
smoothing paths, hence `--save-paths`. `--save-filtering` is the
tool you reach for when something is *wrong* with the PF and you
want to see inside it.

### `--save-filtering <PATH>`: filtering marginals with a warning

Semantics: at each observation step `t`, before resampling, emit a
row per particle with the particle's current state and the un-
normalised log-weight. Downstream consumers must re-normalise within
each `time` group to get filtering marginal densities.

Output format:

```
time    particle    S       I       R       ...    log_weight
0       1           999     1       0              -0.0
0       2           999     1       0              -0.0
...
1       1           996     4       0              -1.23
1       2           997     3       0              -0.87
...
```

`particle` is the **in-step** index (not a persistent identity across
`time`). Linking particles across `time` does NOT give sample paths —
it traces genealogies through resampling, not draws from the
smoothing distribution.

On **every** invocation with `--save-filtering`, emit this info log
to stderr:

```
[info] --save-filtering emits filtering marginals p(x_t | y_{1..t}),
       not smoothing paths. Joining particles across time by index
       does NOT yield trajectory samples from the posterior.
       For coherent sample paths use --save-paths N. For the
       right diagnostic against data see
       docs/dev/proposals/2026-04-19-pf-latent-trajectories.md
       §"Scientific framing".
```

The log fires unconditionally (not a `--quiet`-able warning, not
behind a verbose flag) because the failure mode is silent and the
class of misuse is exactly "use filtering marginals where smoothing
was meant." Paying one stderr line per invocation to prevent a
confidently-wrong book chapter is the right trade.

### What neither flag does

Neither flag caches PF output under the unified `Run` ADT. A
`RunKind::Pfilter` variant would let `camdl list` surface PF runs
alongside sims and fits, with hash-aware reuse. **Not doing this in
this proposal.** The PF is cheap enough that re-running is rarely
the bottleneck, and adding a new `RunKind` variant deserves its own
design pass (directory shape, hash inputs, cache-staleness rules).
Revisit when a concrete workflow asks for it.

## UX notes

### Default recommendation in `--help`

The help text for `camdl pfilter` should state the distinct
purposes of each flag rather than listing them neutrally:

```
--save-paths N        Draw N trajectory samples from the smoothing
                      distribution (ancestor tracing). For
                      model-vs-data plots, this is what you want.

--save-filtering PATH Dump per-step particle states + weights.
                      For PF diagnostics (particle degeneracy, obs
                      model sanity, implementation debugging). NOT
                      a substitute for --save-paths when plotting
                      against data.
```

Asymmetric framing is deliberate. The two flags look superficially
interchangeable ("both dump PF particles!") but serve non-
overlapping purposes; help text is where users actually read, so
the distinction belongs there.

### `camdl fit pfilter <fit_dir>` convenience wrapper

Out of scope for this proposal. Trivial five-line addition later:
resolve fit_dir → MLE params toml → call `camdl pfilter` with
those params + the fit's data. Deferred because `camdl pfilter
--params <MLE> --data <data>` already covers the workflow without
coupling to fit-dir provenance.

## Interaction with existing systems

### Relationship to PGAS

PGAS already produces ancestor-traced trajectories as part of its
sampling loop. `--save-paths` duplicates that plumbing in a
bootstrap-filter context — not a redundancy, because the two
procedures answer different questions (`camdl pfilter --save-paths`
gives paths at a point estimate; PGAS integrates over both paths
AND parameters). Implementation should share the ancestor-walk
utility across both call sites in a `sim::inference::ancestor_trace`
module if the logic duplicates more than trivially.

### Relationship to `camdl simulate --replicates`

`simulate --replicates N` produces N unconditional forward paths.
`pfilter --save-paths N` produces N data-conditional paths at the
same parameters. **Pairing them in a figure is the canonical "does
this model match data?" diagnostic** per the framing above.

To make the pairing easy: identical TSV schema is a feature, not an
accident. Downstream code should be able to load both with the same
reader and plot them with the same polars pipeline.

## Implementation plan

Commit 1 — shared ancestor-trace primitive.
Extract the backward-walk from `pgas.rs` into
`sim::inference::ancestor_trace::sample_paths(filter_state, n)`.
Pure function, easy to test (synthetic ancestor arrays → known
paths).

Commit 2 — extend `cli::pfilter`.
Add the two flags, wire to the primitive. Emit the info log on
`--save-filtering`. Update `--help`.

Commit 3 — integration tests.
End-to-end: run on a known model with known obs, assert that
save-paths output has N unique paths, each with T timesteps, and
that the marginal at each t is consistent with the filtering
state recorded by the existing pfilter log.

Commit 4 — docs.
`docs/inference.md`: §"Filtering vs smoothing", §"Choosing a
diagnostic plot". Book chapter picks this up in a follow-up PR.

## Test plan

- Unit: `ancestor_trace::sample_paths` with a hand-built ancestor
  matrix and a uniform weight vector — N sampled paths should equal
  the N distinct lineages walking back from the final step.
- Unit: on a deterministic model (no process noise), ALL ancestor-
  traced paths should be identical — degenerate but correct.
- Integration: a 2-compartment SIR with known truth, fit PF to
  data, sample paths, assert the smoothing ribbon contains the
  truth at every `t` with at least the nominal coverage.
- Integration: assert the info log fires on `--save-filtering` and
  doesn't fire on `--save-paths`.
- Golden: TSV format stability for one known run; locks the schema
  so downstream polars pipelines don't break silently on reorder.

## Out of scope (noted for future)

- **`RunKind::Pfilter` cache-aware runs.** Natural fit in the
  unified output tree. Deferred pending a concrete workflow that
  actually wants PF caching (likely: repeated PF-at-MLE calls for
  different bootstrap replicates of an uncertainty-quantification
  script).

- **Per-IF2-iteration trajectory dumps.** Originally option (3) in
  the agent-channel request. Rejected — the *final* PF pass is the
  diagnostic one; per-iteration dumps are disk-wasteful and the
  pedagogical value is low.

- **Two-filter smoothing.** Kitagawa-style combined forward/backward
  smoother. Ancestor tracing gives smoothing draws from a bootstrap
  filter at no extra cost; explicit two-filter machinery is only
  worth adding if someone needs the full smoothing marginal density,
  not samples.

## Open questions

1. Should `--save-paths` take a path argument for the TSV, mirroring
   `--save-filtering`, or is emitting to `--output`-scoped locations
   sufficient? Leaning explicit: `--save-paths N paths.tsv`. Makes
   the output location visible at the call site.

2. Should the filtering-marginals info log be downgradable in
   automated pipelines (CI, batch)? Leaning no — the log is the
   point. A script author who knows what they're doing can pipe
   stderr to /dev/null. An automation path that *silences the
   warning by default* defeats the purpose.

3. For the book chapter: do we write the "(1) vs (2) diagnostic"
   section in this proposal, or in the book chapter itself? This
   proposal is the technical landing; the book chapter is the
   teaching landing. The pedagogy belongs in the book; the
   reference explanation belongs in `docs/inference.md`. This
   proposal is the bridge.

**ACTION FOR downstream:** does this framing match what you want to
teach in the chapter? Specifically (a) the "divergence is the
diagnostic" framing over "pick one ribbon and show it", and
(b) committing to the two-panel figure (unconditional + smoothing)
rather than replacing one with the other. Confirm before commit 2
lands, because it's the shape of the API that downstream will
consume.

**ACTION FOR upstream (me, later):** wait for downstream
confirmation on the framing; then ship commits 1–4 as proposed;
then open the follow-up for `camdl fit pfilter` convenience
wrapper once the primitives settle.
