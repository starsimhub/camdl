---
status: approved (ship-now)
date: 2026-05-03
authors: camdl-side, prompted by typhoid-vignette agent's LHS-landscape prototype
target: ship in ~1 week — tracks gh#43 (to be filed)
---

# `camdl survey` — likelihood-landscape diagnostic

## TL;DR

A new `camdl survey` subcommand that draws N Latin-hypercube points
across declared parameter bounds, evaluates the marginal log-likelihood
at each point via a particle filter (default) or single deterministic
trajectory (opt-in), and writes a TSV ready for visualisation. Optional
`--render` produces a self-contained interactive HTML pair-plot with a
top-K cutoff slider, color-by selector, axis-scale toggle, hover
tooltips, and panel brushing.

This is a **diagnostic tool**, not a fitting routine. It answers "is
my model identifiable from this data, before I burn six hours of IF2?"
in minutes-to-an-hour. It does not produce an MLE.

## Motivation

The typhoid-vignette agent prototyped this in
`camdl-book/.claude/worktrees/typhoid/_lib/identifiability.py` and
applied it to five fits:

| Fit | Process noise | Single-eval LHS landscape |
|---|---|---|
| Boarding school SIR | none | recovers MLE in single shot |
| NegBin SIR | obs only | tight clusters, k well-identified |
| IC-free SIR | none | surfaces β/R₀ identifiability ridge |
| Typhoid SIRC (8 params, N=10⁶) | small | 2000-point landscape recovers global basin at ll=−14,670 — **better than IF2 single-start ever achieved** (−28,556) |
| He 2010 measles SEIR (σ_se=2.816) | large | "stochastic deceiver" — top samples biologically implausible |

Two findings make this worth shipping as a first-class camdl feature:

1. **Diagnostic value is real.** For three of the five cases the
   landscape pair-plot reveals identifiability structure that's
   either invisible (IC-free SIR's β/R₀ ridge) or expensive to
   reach via IF2 (typhoid's basin). For the failure case (measles)
   it correctly flags "this is hard" rather than fabricating
   confidence.

2. **Single-eval is dangerous default.** The He 2010 failure mode
   is selection bias on a 1-sample Monte Carlo estimator of the
   marginal log-likelihood. Standard PMMH theory (Doucet et al.
   2015, *Biometrika*) gives the bar: per-point `log p̂(y|θ)` SE
   should be ≤ ~1.7 nats for ranks to be trustworthy. A single
   trajectory under chain_binomial with σ_se=2.816 gives SE in
   the 50–500 nats range. Defaulting to particle-filter eval at
   modest particle/replicate counts brings this down enough to
   make the diagnostic meaningful for stochastic models too.

## Naming

`camdl survey` (not `landscape`, not `prescout`). Rationale:

- "Pre-scout" couples the name to one workflow (fit pipeline). The
  primary value is diagnostic, not seeding.
- "Landscape" is technically accurate but more clinical.
- "Survey" reads as broad-but-shallow exploration, accessible to
  the book's audience.

In docs prose we say "the likelihood landscape" / "survey points";
the command is `camdl survey`.

## Two likelihoods, one rule

Same point as the (closed) gh#40 framing applies here: PF-eval
estimates `p(y|θ)` (the chain_binomial marginal likelihood);
simulate-eval estimates `p(y | x_{0:T} ~ p(x|θ))` for one realisation
(noisy MC estimator of the same quantity); ODE-eval would estimate
`p(y | E[x|θ])` (a different statistical object — Jensen's inequality
bias). For low-noise regimes these converge empirically; for
high-noise regimes they don't. **Survey defaults to PF-eval** so the
default behaviour answers the same question the rest of camdl's
inference machinery is targeting.

## CLI surface

```
camdl survey <model> [--fit FIT.toml] [--data DATA.tsv]
    [--estimate "name=lo:hi"]...
    [--fixed "name=value"]...
    [--scenario NAME]
    [--n-points N]
    [--eval simulate|pfilter]
    [--eval-particles N]
    [--eval-replicates K]
    [--seed S]
    [--render]
    [--output DIR]
    [--label TEXT]
    [--force]
```

### Two input modes

**Fit-aware mode** (typical):
```bash
camdl survey model.camdl --fit fit.toml
```
Reads `[estimate]` bounds and `[data]` from fit.toml. The bounds in
`[estimate].bounds` are the LHS span; `[fixed]` and `from_scenario`
populate the rest of the parameter vector; `[data]` provides the
observation TSVs. This is what users will run most of the time.

**Inline mode** (one-off exploration):
```bash
camdl survey model.camdl --data cases.tsv \
  --estimate "beta=0.001:1.0" --estimate "gamma=0.01:0.5" \
  --estimate "rho=0.05:0.95"
```
For users who don't have a fit.toml yet — common when sketching
identifiability for a new model.

### Defaults

| Flag | Default | Rationale |
|---|---|---|
| `--n-points` | 1000 | Balances coverage vs cost for d ≤ 8. |
| `--eval` | `pfilter` | Safe default: handles process noise via PMMH-style MC estimator. |
| `--eval-particles` | 200 | Modest PF; adequate for σ_se ≤ 1 on weekly data. |
| `--eval-replicates` | 3 | logmeanexp combiner; 3 reps cuts SE by ~√3 vs 1 rep. |
| `--seed` | 42 | Same default as the rest of camdl. |
| `--render` | not present | TSV is always written; HTML is opt-in. |
| `--output` | `results/surveys/` | Default CAS root. |

### Per-point cost (rough)

| Model | `simulate` | `pfilter` 200×3 | `pfilter` 500×5 |
|---|---|---|---|
| Typhoid SIRC (T=15 obs) | ~5 ms | ~3 sec | ~15 sec |
| Boarding school SIR (T=14) | ~2 ms | ~1 sec | ~5 sec |
| He measles SEIR (T=1043) | ~50 ms | ~30 sec | ~3 min |

For 1000 points: simulate ≈ 5–50 sec; PF default ≈ 50 min – 8 hr;
PF safe-mode ≈ several hours – multiple days. The cost-correctness
trade is real and surfaced in `--help`.

## TSV output (primary artifact)

`results/surveys/<stem>-<hash[:8]>/landscape.tsv`. One row per LHS
point, columns:

| Column | Source | Notes |
|---|---|---|
| one per estimated param | LHS draw | scale-aware (Log-typed in log space, Logit/None linearly), via `fit::init` module |
| `loglik` | logmeanexp of K replicate evals | `−inf` when sim/PF failed |
| `loglik_se` | replicate variance | per-point Monte Carlo SE; `0` for `--eval simulate` (no replicates) |
| `mean_ess` | average PF ESS across obs times | filter-health diagnostic; column omitted when `--eval simulate` |
| `n_replicates` | K | for traceability across reruns |
| `point_id` | 0..N-1 | stable index for joining to other artifacts |

Sorted by `loglik` descending. Always written; this is the canonical
artifact regardless of whether `--render` is set.

## CAS integration

New `RunKind::Survey(SurveyMeta)` discriminator in
`crates/cli/src/run_meta.rs`. `SurveyMeta` carries:

- `model`, `model_hash` (existing pattern)
- `data_hashes: HashMap<String, String>` (per-stream content hashes,
  same shape as `FitMeta`)
- `bounds: HashMap<String, (f64, f64)>` (the LHS box; canonical-hashed)
- `n_points: usize`
- `eval_method: SurveyEvalMethod` (Pfilter | Simulate)
- `eval_particles: usize`
- `eval_replicates: usize`
- `seed: u64`
- `fixed: HashMap<String, f64>` (resolved fixed params; canonical-hashed)
- `scenario: Option<String>`

Output dir: `results/surveys/<stem>-<hash[:8]>/`. Contents:

```
results/surveys/typhoid_sirc-3a4b5c6d/
  run.json          # Run with kind = Survey(SurveyMeta)
  landscape.tsv     # primary artifact (always)
  summary.json      # SE distribution, top-K stats, dimensionality warnings
  landscape.html    # interactive plot (only when --render)
```

`camdl list` and `camdl show` get a `Survey` arm that prints
relevant stats: n_points, eval method, top loglik, SE distribution
quartiles, and (if rendered) the path to the HTML.

## Default behaviour: TSV-only

`--render` is **not** the default. Users who want a quick iteration
loop just get the TSV and pipe it into their own viz pipeline; users
who want the standalone interactive plot pass `--render`. Two
reasons to gate rendering:

1. The plotly bundle is ~3 MB embedded per HTML file; users who
   produce many surveys (sweeping bound widths, comparing models)
   shouldn't pay that cost involuntarily.
2. TSV is the universal interface — Python, R, Quarto, the user's
   own tools — and we shouldn't force a rendering choice on
   everyone.

`--render` produces `landscape.html` next to `landscape.tsv` in the
same CAS directory.

## Interactive plot (when `--render` set)

Layout copies the visual style of
`camdl-book/.claude/worktrees/typhoid/_lib/identifiability.py`:

- **Diagonals**: marginal histograms with three stacked layers —
  bottom (1−top_pct) gray (`#cccccc`), top *X*% green (`#16a085`),
  top 1% red (`#c0392b`). 40 bins; log bins when the parameter is
  log-scaled.
- **Off-diagonals**: bottom (1−top_pct) as small gray points
  (radius 2, opacity 0.4); top *X*% as larger viridis_r points
  (radius 7, opacity 0.85), color encoding `log|loglik − loglik_max|`
  on a LogNorm-style log scale; top-5 as red stars with black
  edges.
- **Axes**: log scale where the parameter's `Transform` is `Log`,
  linear for `Logit`/`None`. User can override per-axis at view time.
- **Colorbar**: right strip, log-scale "|ll − best| (nats)".
- **Legend**: upper-right margin with proxy artists for each
  marker class.

### Interactive controls (v1)

1. **Top-K cutoff slider.** Drag to change the percentile that
   defines "top" (default 10%, range 0.1%–50%). The diagonal
   layers and off-diagonal viridis vs gray re-bin live.
2. **Color-by selector.** Dropdown that picks the column to drive
   off-diagonal viridis: `loglik` (default), `loglik_se`,
   `mean_ess`. Lets users see "where does the noise concentrate?"
   and "where does filter health degrade?" alongside the headline
   loglik view.
3. **Per-axis scale toggle.** Click an axis label to toggle
   log/linear. Initial state from the parameter's declared
   `Transform`.
4. **Hover tooltip.** Per-point: full parameter coordinates,
   loglik, loglik_se, point_id.
5. **Panel brushing.** Click-drag a region in any panel; matched
   points highlight across all other panels (red outline) and the
   non-matched points dim further. Click-out clears the brush.

Brushing adds modest implementation cost (Plotly's
`selectedpoints` API + a per-axes shared state, ~50 LOC of vanilla
JS) but is high-value for identifying where high-loglik regions in
one pair correspond to in another. Worth shipping in v1.

### HTML packaging

`plotly.min.js` (~3 MB) is **embedded inline** at compile time via
`include_str!` and emitted as a `<script>` tag in the HTML. No CDN
dependency. Reasons:

- HTML works offline / in archives / in restricted networks.
- Version is pinned at build time, not subject to CDN drift.
- Self-contained `rsync` semantics — single file, drops anywhere.

The data block is `<script type="application/json" id="landscape-data">…</script>`
populated from the TSV via Rust serde at run time.

## Runtime warnings

Three warnings emitted at run start or end:

1. **Curse of dimensionality (start).** When `d > 6`: gentle note
   about 2D-marginal interpretation. When `d > 10`: stronger
   warning suggesting `camdl profile` or restricting `[estimate]`
   to a subset. When `n_points / d² < 50`: "consider `--n-points
   N` (recommended ≥ X) for adequate pair-plot resolution."

2. **Per-point loglik SE distribution (end).** After all points
   are evaluated, report the distribution of `loglik_se`. If
   `> 25%` of points exceed Doucet's bar (1.7 nats), warn:
   ```
   warning: 47% of survey points have loglik_se > 1.7 nats —
   ranks for those points are unreliable. Consider:
     --eval-replicates 5  (3× compute, ~√(5/3) variance reduction)
     --eval-particles 500 (2.5× compute, lower per-replicate variance)
   ```
   Doucet's threshold is the published bar for trustworthy
   pseudo-marginal MCMC; we surface it directly.

3. **`--eval simulate` warning (start).** When `--eval simulate`
   is set: print a one-time notice naming the failure mode and
   pointing at PF as the safe alternative. Not a refuse — the
   user may have a known-deterministic model.

## `--help` content

The after_help block is intentionally verbose. Drops at the bottom
of `camdl survey --help`:

```
EXAMPLES

  # Fit-aware: read [estimate] bounds and [data] from fit.toml
  camdl survey model.camdl --fit fit.toml

  # Inline bounds, data file specified directly
  camdl survey model.camdl --data cases.tsv \
      --estimate "beta=0.001:1.0" --estimate "gamma=0.01:0.5"

  # Fast deterministic-only mode (skip PF; not safe for stochastic models)
  camdl survey model.camdl --fit fit.toml --eval simulate

  # Render the interactive HTML alongside the TSV
  camdl survey model.camdl --fit fit.toml --render

WHAT THIS IS

  A diagnostic tool that draws N Latin-hypercube points across the
  declared parameter bounds, evaluates the marginal log-likelihood
  at each, and writes a TSV (and optionally an interactive HTML
  pair-plot) to surface identifiability structure. It is intended
  to be run BEFORE camdl fit, to answer:

    - Is this model identifiable from this data?
    - Are there ridges or multiple basins?
    - Are the high-loglik regions biologically plausible?
    - Where do likely basins concentrate? (informs scout bounds)

  Survey is NOT a fitting routine. It does not produce an MLE.
  The output cannot substitute for camdl fit.

WHEN TO TRUST THE OUTPUT

  Survey works well when:
    - Process noise is small (deterministic-skeleton regime),
      or `--eval pfilter` is used with adequate
      particles/replicates
    - Parameter dimension d ≤ 8 (pair-plots are visually parseable)
    - Bounds reflect informed prior plausibility (not "throw a
      wide net")
    - Dynamics are not strongly chaotic (seasonally-forced SEIR
      with high R₀ may produce intrinsically jagged landscapes)

KNOWN LIMITATIONS

  Stochastic deceiver (mitigated by --eval pfilter):
    Single-trajectory loglik is a 1-sample Monte Carlo estimate of
    p(y|θ) with variance proportional to the model's process
    noise. With high noise (e.g. multiplicative gamma white noise
    on transmission, σ_se > ~1) the rank of N points by
    single-trajectory loglik is biased toward "lucky outliers"
    (Andrieu & Roberts 2009; Doucet et al. 2015, Biometrika). The
    default --eval pfilter substantially mitigates this; survey
    will warn at run end if the per-point loglik SE distribution
    indicates unreliable ranks.

  Chaotic dynamics:
    Seasonally-forced SEIR and similar systems have positive
    Lyapunov exponents in much of parameter space (Earn et al.
    2000; Bauch & Earn 2003). Small Δθ produces wildly divergent
    deterministic trajectories. The landscape will be
    intrinsically jagged regardless of eval method. Interpret
    such surveys cautiously: the diagnostic is correctly
    reporting "this is hard," not "your model is broken."

  Bounds dependence:
    Survey ranks are conditional on the bounds you give. Wide
    bounds dilute (the "top 10%" may be marginally-less-bad
    rather than meaningfully-good). Narrow bounds may exclude
    the true basin entirely with no signal that this happened.
    Bound choice is a load-bearing modelling decision; survey
    cannot rescue a poorly-specified bounds box.

  Curse of dimensionality:
    Pair-plots project 2D marginals from a d-dimensional joint
    distribution. High-loglik points concentrating in a 2D
    pair may reflect tight conditioning on unshown parameters
    not visible in that view. Past d ≈ 8 this becomes hard to
    interpret. Survey emits warnings at d > 6 and d > 10;
    consider camdl profile for higher-dimensional
    identifiability questions.

  Misspecification ≠ identifiability:
    A tight, well-clustered top-K is a necessary but not
    sufficient condition for trusting the resulting fit. A
    misspecified model can have a tight likelihood at a
    wrong-but-best-fitting θ. Posterior predictive checks
    against held-out data are the orthogonal validation;
    survey cannot substitute.

  Silent miss case:
    With N points in d dimensions, LHS may not hit a true basin
    that occupies a small fraction of the bounds box. The
    landscape would then show structure of wrong basins with
    no signal that the right one was missed. If results look
    surprising, increase --n-points and re-run.

CITED REFERENCES

  Andrieu, C. & Roberts, G. O. (2009). The pseudo-marginal
    approach for efficient Monte Carlo computations. Annals of
    Statistics, 37(2), 697–725.
  Doucet, A., Pitt, M. K., Deligiannidis, G. & Kohn, R. (2015).
    Efficient implementation of MCMC when using an unbiased
    likelihood estimator. Biometrika, 102(2), 295–313.
  Earn, D. J. D., Rohani, P., Bolker, B. M. & Grenfell, B. T.
    (2000). A simple model for complex dynamical transitions in
    epidemics. Science, 287(5453), 667–670.
```

## Implementation outline

Files touched / added:

- `rust/crates/cli/src/survey.rs` (new) — `cmd_survey` entry point,
  argument resolution, LHS draw via `fit::init::build_chain_starts`
  (reuses scale-aware sampler), parallel evaluation loop, TSV
  writer, HTML rendering.
- `rust/crates/cli/src/args/mod.rs` — `SurveyArgs` struct + after_help.
- `rust/crates/cli/src/main.rs` — clap subcommand + dispatch.
- `rust/crates/cli/src/run_meta.rs` — `RunKind::Survey(SurveyMeta)`
  variant.
- `rust/crates/cli/src/cas/typed.rs` — `SurveyMeta` content-hash
  builder, mirroring `ProfileMeta`.
- `rust/crates/cli/src/show.rs` / `list.rs` — Survey arms.
- `rust/crates/cli/templates/landscape.html.hbs` (or similar) — the
  HTML template with embedded plotly bundle and data placeholder.
- `rust/crates/cli/src/landscape_html.rs` (new) — render the template
  given the TSV / metadata.
- Integration tests against existing golden models.

Reuse paths:

- `fit::init::build_chain_starts` for scale-aware LHS sampling.
  Already shipped in gh#42; survey calls it with `n_points` and
  the resolved bounds.
- `fit::runner::build_if2_params_from_specs` for bounds resolution
  (model bounds vs fit-toml bounds; gh#42 follow-up just shipped).
- `sim::inference::particle_filter::bootstrap_filter` for PF eval.
- `sim::observation::compute_obs_loglik` (or equivalent) for the
  simulate-eval path.
- `cas::write_run_json` for canonical CAS write semantics.

## Tests

- LHS draws are scale-aware (already covered by `fit::init` tests;
  add survey-level smoke that confirms log bins for log-typed
  params get spread across decades).
- TSV schema invariants: column order, sortedness, presence of
  required columns.
- CAS hash includes bounds (different bounds → different cache key).
- CAS hash includes data file contents (same as gh#39 fix for
  profile).
- Curse-of-dim warnings fire at the right thresholds.
- SE warning fires when synthetic high-noise data produces
  > 25% of points with SE > 1.7.
- Render produces a valid self-contained HTML; smoke test that
  it parses and contains the data block.
- Inline mode: `--estimate` and `--fixed` parse correctly; missing
  bounds error cleanly.
- Fit-aware mode: `--fit fit.toml` reads `[estimate]` and `[data]`.
- Mutual exclusion: `--fit` with redundant `--estimate` /
  `--data` errors with a clear message.

## Out of scope (v1)

- **Stochastic-extinction column.** User-specified watch-compartment
  list adds CLI complexity v1 doesn't need; the column-coloring
  infrastructure can extend later without breaking schema. Defer.
- **`init_method = "survey_top_k"` in fit.toml** (consume a prior
  survey run and seed scout from top-K). Real workflow integration
  but a separate concern; ship as gh#NN-follow-up after we have
  downstream usage of the standalone survey.
- **Parameter brushing across multiple surveys** (compare two
  surveys side-by-side for sensitivity analysis). Useful but adds
  multi-tab HTML complexity.
- **Posterior overlay.** When a fit has run, overlay the chain
  trajectories on the survey landscape. Worthwhile but separate
  feature.

## Acknowledged tradeoffs

- The interactive HTML at ~3 MB per file is the cost of self-
  contained portability. We accept this. Users producing many
  surveys can opt out of `--render` and consume the TSV.
- PF-eval default is the safe-but-not-cheap choice. For
  measles-class models the survey is competitive with IF2-scout
  cost; we don't promise "always fast." The simulate-only escape
  hatch is one CLI flag away.
- Default thresholds (1.7 nats SE bar; d > 6 / d > 10 warnings;
  n_points/d² < 50) are first-pass values from published bars and
  rule-of-thumb. May need re-tuning after downstream usage; should
  be revisited at the 1-month mark.

## Visual style attribution

The pair-plot visual style (color palette, three-layer histograms
on diagonals, viridis_r off-diagonals, top-5 red stars) is ported
from
`camdl-book/.claude/worktrees/typhoid/_lib/identifiability.py`
(`pairplot` function). The Python prototype is the reference; the
camdl Rust+HTML version aims for the same visual language with
interactive controls layered on. Acknowledged in the implementation
commit message.
