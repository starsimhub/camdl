---
status: proposal
date: 2026-05-04
authors: camdl-side, prompted by endemic-equilibrium age-stratified inference use case
target: three independent phases, each independently shippable; Phase 1 ~1 week, Phase 2 ~3 days, Phase 3 ~1 week
supersedes: docs/dev/proposals/2026-05-02-ode-backend-deterministic-inference.md (closed gh#40)
---

# ODE-Inference Algorithms: NLopt, MH, NUTS

## TL;DR

camdl's three current inference algorithms (IF2, PGAS, PMMH) all wrap a particle
filter and assume **time-series likelihoods** under a stochastic process kernel.
For users fitting deterministic-equilibrium models (endemic age-stratified
incidence, large-population stratified epi models), the particle filter is
structurally redundant — multiple particles give identical trajectories under
ODE — and we burn 100×–1000× more compute than necessary.

This proposal adds three new algorithm-explicit stage methods to fit.toml, each
implicitly running on the ODE backend:

| `algorithm`              | Backend       | Description                                                                | Phase | LOC  |
| ------------------------ | ------------- | -------------------------------------------------------------------------- | ----- | ---- |
| `nl-sbplx` / `nl-bobyqa` | `ode`         | Deterministic MLE via NLopt; Sbplx default, BOBYQA for smooth objectives   | 1     | ~550 |
| `mh`                     | `ode`         | Vanilla Metropolis-Hastings on deterministic marginal likelihood           | 2     | ~300 |
| `nuts`                   | `ode`         | Gradient-based Bayesian via NUTS + forward sensitivity ODE                 | 3     | ~600 |

Existing methods (`if2`, `pgas`, `pmmh`) keep their current chain_binomial PF
semantics unchanged. The two paths coexist; users pick the algorithm that
matches their inference question.

Each phase is independently shippable, validated against a worked typhoid case
as the merge bar, and reuses substantial infrastructure already in tree (LHS
init from gh#42, bounds resolution, symbolic autodiff for Phase 3, the existing
`OdeSim` forward simulator).

**Phase 1 also fixes a latent inaccuracy in `camdl survey`.** Today
`camdl survey --eval simulate` is implemented as a 1-particle bootstrap PF
through `ChainBinomialProcess` — not `OdeSim`. That returns a 1-sample MC
estimate of `p(y|θ)` under the stochastic chain-binomial process kernel, not
the ODE deterministic skeleton likelihood. They differ by Jensen's-inequality
bias plus single-trajectory MC noise. Phase 1 reroutes `--eval simulate`
through the new `compute_ode_loglik` helper so its name matches its
semantics. Documented as a behaviour change with a cache invalidation; see
Phase 1 §"Survey integration" below.

## Motivation

### The endemic-equilibrium use case

Stratified epi models fit to age-stratified incidence in endemic settings
(typhoid SIRC, malaria EIR-by-age, polio age serology, MMC HIV) share a common
likelihood structure:

```
p(observed_age_setting_incidence | θ)
  ≈ Poisson(observed | predicted_equilibrium_incidence(θ) · py_at_risk)
```

The data is a **marginal age-incidence distribution at equilibrium**, not a time
series. The likelihood depends only on the model's steady-state output, not its
trajectory through time. For ergodic models with sufficiently large per-cell
populations (typhoid: ~10⁶ per setting × age bin), process noise washes out at
equilibrium and the deterministic and stochastic likelihoods converge
empirically.

Today camdl forces users through the chain_binomial PF for these fits (because
the PF is the only inference path). Per typhoid vignette: 50-year sim × 200
particles × 50 IF2 iterations × 8 chains = 14 hours wall time for a joint MLE
that the same data could pin down via deterministic optimization in ~30 seconds.

### The deeper observation

"ODE backend" sounds like a backend swap — independent of the inference
algorithm. It isn't. Under ODE the particle filter is structurally redundant:

- Bootstrap PF + ODE: all N particles give identical trajectories per θ. ESS = N
  forever. Resampling no-op. Marginal likelihood is _exact_ (not Monte Carlo) at
  N=1.
- IF2 + ODE: IF2's cooling-perturbation loop assumes per-particle parameter
  perturbations produce per-particle trajectory variance. With ODE,
  perturbations across particles still yield identical trajectories. IF2
  collapses to a noisy gradient-free hill-climber — not what IF2 was designed to
  do.
- PGAS + ODE: degenerate CSMC step (only one trajectory per θ). NUTS-on-θ would
  still work but on the marginal likelihood — i.e. just vanilla NUTS without the
  Gibbs sweep.
- PMMH + ODE: PF-inside-MH does nothing. PMMH becomes vanilla MH on the
  deterministic marginal likelihood.

The right framing: under ODE, each algorithm should be replaced with its
deterministic-likelihood equivalent. There is no coherent "method=if2 +
backend=ode" — that combination just runs IF2's machinery against an objective
the machinery isn't suited for.

### Lessons from the closed gh#40

The previous proposal (`2026-05-02-ode-backend-deterministic-inference.md`,
closed) tried to ship `--backend ode` as a transparent backend swap on profile
only. The PR (#41, closed unmerged) revealed:

1. **Profile-only scope was wrong.** ODE inference is cross-cutting — fit,
   profile, Bayesian routines all want it for the same use cases.
2. **"backend=ode + method=if2" is incoherent naming.** The user is really
   asking for a different algorithm; the backend flag was a misleading
   abstraction.
3. **Multi-start collapse.** Deterministic optimizers from a single starting
   point find one basin; the closed PR didn't draw multi-start LHS, which made
   convergence-gate basin-spread leg non-informative. (gh#42 LHS init has since
   shipped — this barrier is now removed.)
4. **Diagnostic experiment deferred.** The "two likelihoods converge in
   low-noise regimes" empirical check was punted to a handoff doc rather than
   gated on merge.

This proposal incorporates all four corrections explicitly.

## Architecture

### Tuple schema: `algorithm` + `backend` (replaces `method`)

The fit.toml `method = "..."` field smuggled `(algorithm, implicit-backend)`
pairs into a single value — `method = "if2"` always meant chain_binomial,
`method = "pgas"` always meant chain_binomial. With ODE inference adding a
second backend, the smuggling becomes visible and confusing: a user reading
`method = "nl-sbplx"` shouldn't have to remember "wait, does that mean ODE?"

This proposal replaces `method` with two explicit fields:

```toml
[stages.<name>]
algorithm = "..."   # what optimizer/sampler runs
backend   = "..."   # what simulator computes the likelihood
# rest of the algorithm-specific fields...
```

Stage names stay user-chosen — examples below use `scout` / `refine` /
`posterior` to mirror conventional fit.toml structure, but any name works.

```toml
# Existing semantics — unchanged behavior, new schema
[stages.scout]
algorithm = "if2"
backend   = "chain_binomial"
chains      = 8
particles   = 200
iterations  = 50
init_method = "lhs"   # gh#42

# Phase 1: deterministic MLE via NLopt + Sbplx
[stages.scout]
algorithm = "nl-sbplx"            # default; nl-bobyqa for smooth objectives
backend   = "ode"
chains      = 8                    # LHS-drawn multi-start, take best
tolerance   = 1e-6                 # xtol_rel
max_evals   = 5000                 # per-chain budget
init_method = "lhs"

# Phase 2: deterministic Bayesian via vanilla MH
[stages.posterior]
algorithm = "mh"
backend   = "ode"
chains      = 4
iterations  = 50000
burn_in     = 5000
thin        = 5
adapt       = true
adapt_start = 2000
init_method = "lhs"

# Phase 3: deterministic Bayesian via NUTS (gradient-based)
[stages.posterior]
algorithm = "nuts"
backend   = "ode"
chains      = 4
warmup      = 1000
samples     = 1000
dense_mass  = true
max_tree_depth = 10
init_method = "lhs"
```

### Valid `(algorithm, backend)` combinations

The matrix is sparse — algorithms structurally require a specific backend:

| Algorithm    | chain_binomial | ode | Status |
|---|---|---|---|
| `if2`        | ✓              | ✗   | stable |
| `pgas`       | ✓              | ✗   | stable |
| `pmmh`       | ✓              | ✗   | experimental |
| `nl-sbplx`   | ✗              | ✓   | beta (Phase 1) |
| `nl-bobyqa`  | ✗              | ✓   | beta (Phase 1) |
| `mh`         | ✗              | ✓   | beta (Phase 2) |
| `nuts`       | ✗              | ✓   | experimental (Phase 3) |

Why each `✗`:

- `if2`/`pgas`/`pmmh` + `ode`: PF-based algorithms need stochastic process variance to compute their objectives. Under ODE all particles produce identical trajectories per θ; the algorithms collapse to noisy / degenerate variants of themselves.
- `nl-sbplx`/`nl-bobyqa` + `chain_binomial`: deterministic optimizers operating on a stochastic objective (single-trajectory loglik) get a noisy ranking signal — IF2 is the right tool for that case.
- `mh` + `chain_binomial`: vanilla MH on a stochastic likelihood gives biased posteriors (PF wrapping is exactly what makes PMMH unbiased).
- `nuts` + `chain_binomial`: gradients become noisy under PF wrapping; PGAS handles this by integrating NUTS into a Gibbs sweep over trajectories, but vanilla NUTS-on-stochastic isn't a coherent algorithm.

Two completely disjoint subsets — that's not coincidence, it reflects the structural truth that PF-based methods need stochasticity and gradient-/exact-likelihood methods need determinism.

### Single source of truth: `METHODS` registry

The matrix lives once in code, not duplicated across the validator, error messages, `--help` output, docs:

```rust
// rust/crates/cli/src/fit/methods.rs (new module)

#[derive(Debug, Clone, Copy)]
pub enum MethodStatus {
    /// Validated against published / vignette use cases; production-ready.
    Stable,
    /// Shipped and exercised but downstream validation still accumulating.
    /// Surfaced as "BETA"; runtime banner names the caveat.
    Beta,
    /// Known limitations that affect correctness in some regime.
    /// Surfaced as "EXPERIMENTAL"; runtime banner is loud.
    Experimental,
}

pub struct InferenceMethod {
    pub algorithm: &'static str,
    pub backend: &'static str,
    pub status: MethodStatus,
    pub one_liner: &'static str,
    pub description: &'static str,
    pub status_note: &'static str,   // banner text for Beta/Experimental
}

pub const METHODS: &[InferenceMethod] = &[
    InferenceMethod {
        algorithm: "if2",   backend: "chain_binomial",
        status: MethodStatus::Stable,
        one_liner: "Iterated filtering MLE",
        description: "...",
        status_note: "",
    },
    InferenceMethod {
        algorithm: "pmmh",  backend: "chain_binomial",
        status: MethodStatus::Experimental,
        one_liner: "Pseudo-marginal MH",
        description: "...",
        status_note:
            "PMMH acceptance rates degrade for T > 500 observations. \
             Correlated pseudo-marginal (rho config) helps but has \
             limits on discrete-state models. PGAS is the production \
             Bayesian path.",
    },
    // ... 5 more entries — see Phase 1/2/3 sections for exact text
];

/// Validate (algorithm, backend) at config-load time.
pub fn validate_combo(algo: &str, backend: &str)
    -> Result<&'static InferenceMethod, String> { ... }

/// Render the matrix table for `camdl fit methods` and error messages.
pub fn render_matrix() -> String { /* iterates METHODS, formats */ }
```

Adding a new algorithm = one entry in `METHODS` plus its dispatcher arm. The validator, runtime status banner, error messages, and `camdl fit methods` output all read from the same list.

### `camdl fit methods` subcommand

A new top-level subcommand renders the registry as a user-facing reference:

```
$ camdl fit methods

CHAIN_BINOMIAL backend (stochastic process kernel)

  algorithm = "if2"           [stable]
    Iterated filtering MLE — perturbation-and-filter loop.
    Use for: scout/refine pipelines on stochastic models.

  algorithm = "pgas"          [stable]
    Particle Gibbs + NUTS-on-θ; production Bayesian path.

  algorithm = "pmmh"          [experimental]
    Pseudo-marginal MH. ⚠ acceptance degrades for T > 500 obs;
    PGAS is the production Bayesian path.

ODE backend (deterministic skeleton; new in this release)

  algorithm = "nl-sbplx"      [beta — default deterministic MLE]
    Sbplx via NLopt — Nelder-Mead variant, robust to boundary
    non-smoothness. Phase 1 typhoid validation passed; other model
    classes still gathering downstream feedback.

  algorithm = "nl-bobyqa"     [beta]
    BOBYQA via NLopt — quadratic-trust-region. ⚠ requires smooth
    objective in search box; fails at parameter-bound boundaries
    where Sbplx succeeds.

  algorithm = "mh"            [beta]
    Vanilla MH; deterministic Bayesian. Adaptive covariance from
    PMMH; recommend adapt_start = max(1000, 200·d).

  algorithm = "nuts"          [experimental]
    Gradient-based Bayesian via forward-sensitivity ODE. ⚠ reactive
    interventions not supported; hierarchical models with d > 30
    untested (forward-mode AD cost).

Methods compute different statistical objects across backends:
  chain_binomial → p(y|θ) under stochastic process noise
  ode           → p(y|θ, ODE_skeleton) — Jensen's inequality bias
In low-noise regimes these converge empirically. See
docs/inference.md §"Two likelihoods" for guidance.
```

### Invalid-combination error template

When `validate_combo` rejects, the error message names the structural reason and points at the right alternative:

```
error: stage 'scout' has algorithm = "if2" with backend = "ode", which is not
       a supported inference method.

       IF2 (Iterated Filtering 2) is a particle-filter-based MLE algorithm.
       It perturbs parameters across particles and uses the between-particle
       trajectory variance to drive the optimization. Under the ODE backend
       all particles produce identical trajectories per parameter point —
       there is no between-particle variance for IF2 to exploit. The
       algorithm collapses to a noisy gradient-free hill-climber that is
       structurally a worse optimizer than the deterministic alternatives.

       If you want MLE on the ODE backend, use:
         algorithm = "nl-sbplx"   default deterministic MLE; robust to
                                  boundary non-smoothness
         algorithm = "nl-bobyqa"  faster than Sbplx on smooth objectives

       Supported (algorithm, backend) pairs:
         (if2,       chain_binomial)  Iterated filtering MLE (stochastic)
         (pgas,      chain_binomial)  Particle Gibbs Bayesian
         (pmmh,      chain_binomial)  Pseudo-marginal MH; experimental
         (nl-sbplx,  ode)             Deterministic MLE; Sbplx (default)
         (nl-bobyqa, ode)             Deterministic MLE; BOBYQA
         (mh,        ode)             Vanilla MH; deterministic Bayesian
         (nuts,      ode)             Gradient-based Bayesian via NUTS

       Note: camdl computes a different statistical object on each backend
       (chain_binomial → p(y|θ); ode → p(y|θ, ODE_skeleton)). In low-noise
       regimes these converge empirically. See docs/inference.md
       §"Two likelihoods" for guidance.
```

The "specific structural reason" line varies per invalid pair (5–6 distinct rejection reasons total for the 7 valid + N invalid combos), pulled from a small lookup table keyed by `(algorithm, backend)`.

### Mixed-mode pipelines

`starts_from` handoff works across algorithm-mode boundaries:

```toml
[stages.scout]
algorithm = "nl-sbplx"
backend   = "ode"

[stages.refine]
algorithm    = "if2"
backend      = "chain_binomial"
starts_from  = "scout"   # stochastic refinement from deterministic basin

[stages.posterior]
algorithm    = "pgas"
backend      = "chain_binomial"
starts_from  = "refine"
```

or the reverse:

```toml
[stages.scout]
algorithm = "if2"
backend   = "chain_binomial"

[stages.refine]
algorithm    = "nl-sbplx"
backend      = "ode"
starts_from  = "scout"   # deterministic polish of stochastic MLE
```

The handoff consumes prior `fit_state.toml` for `start_values`; the consumer doesn't care what algorithm/backend produced them. Composes naturally; lets users build pipelines that play to each algorithm's strengths.

### CAS / RunKind integration

`Stage` carries the new `algorithm` + `backend` tuple plus algorithm-specific
knobs as flat fields (chains, iterations, tolerance, etc.). `MethodKind` in
`run_meta.rs` becomes `{ algorithm: String, backend: String }` instead of a
single tag. The existing `RunKind::FitStage` discriminator already carries the
method tag — no new top-level RunKind variants needed. Provenance hashing
extends naturally: the canonical `identity_payload()` includes both
`algorithm` and `backend`, so two stages with the same algorithm but different
backends hash to different cache keys (correct — they compute different
likelihoods).

## Phase 1 — `nl-sbplx` and `nl-bobyqa`

### Scope

Deterministic MLE via NLopt over the ODE marginal likelihood. The biggest single
use case (profile likelihood, scout-MLE basin finding, equilibrium fitting).
Validates the cross-cutting tuple-schema architecture against a real worked
example before Phase 2/3 scale it.

### Algorithms shipped

NLopt crate (`nlopt = "0.8"`, MIT-licensed Rust wrapper around Steven Johnson's
C library). Two algorithms surfaced as `algorithm` values in v1:

| `algorithm` value | NLopt name  | Use case                                                                                            |
| ----------------- | ----------- | --------------------------------------------------------------------------------------------------- |
| `nl-sbplx`        | `LN_SBPLX`  | Default deterministic MLE; Nelder-Mead variant robust to boundary non-smoothness                     |
| `nl-bobyqa`       | `LN_BOBYQA` | Quadratic-trust-region; faster than Sbplx on smooth objectives, fails at parameter-bound boundaries  |

Sbplx is the default per the closed gh#40 review's correct point: compartmental
likelihoods are smooth in the interior of the parameter box but non-smooth at
boundaries (degenerate states) and where event timing depends on parameter
values. BOBYQA's quadratic trust region fails badly in those regions.

**Cut from the closed-gh#40 list** (deferred until a real use case shows up):

- `cobyla` — constrained optimization is rare in compartmental models
- `isres` / `crs2` — global multi-modal optimizers; `camdl survey` already
  serves the global-exploration role better (LHS coverage + pair-plot
  diagnostics + filter-health columns). Adding global optimizers as
  `algorithm` values would be redundant with the survey-then-NLopk-from-top-K
  workflow we've been building.

If global optimization is genuinely needed later, `nl-crs2` is a one-line addition
(new `METHODS` entry + `nlopt::Algorithm::Crs2Lm` mapping). Two for v1 keeps the
matrix tight and surfaces only what works.

### Multi-start

`chains = N` draws N LHS-spread starting points via existing
`fit::init::build_chain_starts` (gh#42, scale-aware via `Transform`). Each chain
runs an independent NLopt optimization to convergence. Best-loglik chain is the
winner. Same multi-start machinery already validated under stochastic IF2 — no
new infrastructure.

### Per-eval cost

| Model                           | Per-eval cost |
| ------------------------------- | ------------- |
| Typhoid SIRC (T=15 obs, N=10⁶)  | ~1–5 ms       |
| Boarding school SIR (T=14)      | ~0.5 ms       |
| He measles SEIR (T=1043 weekly) | ~10–50 ms     |

NLopt typically converges in 50–500 evaluations per chain. Total scout cost:
chains × evals × per-eval. For typhoid 8-chain joint MLE: 8 × 200 × 3 ms ≈ 5
seconds vs 14 hours under chain_binomial — ~10000× speedup.

### Two likelihoods — load-bearing framing

User-facing docs and `fit run` log output must surface this clearly:

> When fit with `method = if2` (or any chain_binomial method), camdl computes
> `p(y|θ)` under the stochastic chain-binomial process kernel. When fit with
> `method = nlopt` (or any deterministic method), camdl computes
> `p(y|θ, ODE_skeleton)` — a different statistical object. In low-noise regimes
> (large populations, no overdispersion) these converge empirically. In
> high-noise regimes they don't. The right question is which likelihood matches
> your scientific use case, not which is faster.

This must appear:

- In `fit run` startup banner when a deterministic method is selected
- In `--help` for stages config
- In `docs/inference.md` as its own subsection
- In any chapter docs that introduce the deterministic methods

### Diagnostic experiment as ship gate

Before merge: take the typhoid model at the smallest stratum population in the
SIRC fit (smallest cell ~5,000 — the boundary of the "deterministic equilibrium"
regime). Fit with both `method = if2` and `method = nlopt`. Compare MLEs ±
per-method within-method spread. Three possible outcomes:

1. MLEs agree to within within-method spread → docs say "for stratified
   equilibrium models with population ≥ 5,000 per cell, the two likelihoods
   agree empirically."
2. MLEs diverge meaningfully → docs say something more nuanced (specific
   population threshold, or population-dependent caveat).
3. NLopt fails to converge cleanly on this model → flags an algorithm issue we
   need to solve before merge.

Half a session of work; gates merge.

### Survey integration

`compute_ode_loglik` is the first inference-side consumer of `OdeSim`. To
avoid building two parallel deterministic-eval paths, **Phase 1 also reroutes
`camdl survey --eval simulate` through `compute_ode_loglik`**. Today that
flag uses a 1-particle bootstrap PF on `ChainBinomialProcess` (deceptively
named "simulate" — it's actually 1-sample stochastic chain-binomial). After
Phase 1 it becomes a true ODE deterministic eval, matching its name.

**Behaviour change**: existing survey runs with `--eval simulate` will produce
slightly different `loglik` values after Phase 1 — for typhoid-class N~10⁶
the difference is sub-nat (Jensen bias + single-trajectory MC noise both
~10⁻⁶ relative); for small populations it's larger (PF discrete events vs
ODE continuous trajectories). Documented loudly:

- `survey` CAS hash bumps an internal version tag, invalidating cached
  landscape TSVs from prior versions
- run-start banner names the change so users running fresh surveys know
  what they got
- the diagnostic experiment (Phase 1 merge gate) quantifies the
  magnitude on the typhoid case, putting empirical bounds on "sub-nat"
  for the docs

Net Phase 1 LOC: ~30 LOC for the survey rewiring + ~20 LOC for cache version
bump and run-start banner. Folded into the ~400 LOC Phase 1 estimate; the
diagnostic experiment validates both the new `compute_ode_loglik` helper
*and* the survey behaviour change in one pass.

This is the cleanest place to fix the latent survey inaccuracy: same Phase 1
diagnostic experiment proves the deterministic-eval path is correct;
shipping it once across both consumers ensures we don't accumulate two
deterministic-eval implementations (the survey-1-particle one already there,
and a NLopt-side one that bypasses survey).

### DSL compatibility

`Capabilities::OVERDISPERSION` already structurally rejects overdispersed models
from running on ODE — see `crates/sim/src/lib.rs`. The dispatch layer scans
`CompiledModel::required_capabilities()` and rejects backend-mismatched models
before inference starts. With the new tuple schema, this fires at the
`(algorithm, backend)` validation step:

```
error: stage 'scout' has algorithm = "nl-sbplx" with backend = "ode", which
       is a supported (algorithm, backend) pair — but the model requires
       the OVERDISPERSION capability that the ode backend does not provide.

       The ode backend produces deterministic trajectories and cannot
       represent overdispersed process noise. Two options:

         1. Switch to a stochastic algorithm/backend pair:
              algorithm = "if2"   backend = "chain_binomial"   (MLE)
              algorithm = "pgas"  backend = "chain_binomial"   (Bayesian)

         2. Remove overdispersed() from the rate expressions if the
            process noise is not load-bearing for your inference question
            (i.e. the deterministic skeleton matches your data).

       The two backends compute different statistical objects:
         chain_binomial → p(y | θ) under stochastic process noise
         ode           → p(y | θ, ODE_skeleton) — Jensen's inequality bias
       In low-noise regimes these converge empirically; if your overdispersed
       term has σ² near zero the skeleton path may be the right call.
```

This is one entry in the per-`(algorithm, backend)` rejection-reason lookup
table referenced under §"Invalid-combination error template" in Architecture.
The OVERDISPERSION case is one of ~5 distinct rejection reasons; the table
keys on either the invalid-pair structure or the model-capabilities mismatch
and renders the matching reason text.

### Convergence diagnostics for NLopt chains

IF2's compound gate (chain-agreement Â on iteration trajectories +
decibans-spread across chains) doesn't carry over directly — NLopt chains are
deterministic optimizers, not stochastic chains, so iteration-trajectory Â is
undefined. The intent generalizes:

- **Leg 1: did chains agree on the basin?** Compare final parameter vectors
  across N starts. Two-number gate: relative range vs bound width AND absolute
  range. Refuse only if **both** exceed thresholds.
- **Leg 2: was the agreed basin actually good?** Loglik spread across N
  converged chains (decibans). Same threshold semantics as IF2.

Verdict line UX matches IF2's:

```
chain-agreement: rel range = X% bound | abs range = Y nat. units   ✓/✗
loglik-eval:     Δ = X dB / threshold Y dB                         ✓/✗
```

**First-pass thresholds** (placeholder values; calibrated against the typhoid
diagnostic experiment before merge):

| Constant               | Initial value                                              | Source                                                           |
| ---------------------- | ---------------------------------------------------------- | ---------------------------------------------------------------- |
| `DET_REL_RANGE_THRESH` | 0.05 (5% of bound width)                                   | matches IF2's "tight cluster" intuition                          |
| `DET_ABS_RANGE_FACTOR` | 2× the within-chain xtol_rel × parameter scale             | so absolute spread within numerical noise of the optimizer is OK |
| `DET_DECIBANS_THRESH`  | 30.0 nats (matches IF2's `decibans_thresh`)                | tail-area heuristic                                              |
| Refusal rule           | rel-range > 0.05 **AND** abs-range > 2× xtol-implied scale | both must fire to refuse                                         |

The diagnostic experiment is the primary calibration: run typhoid scout under
NLopt, observe the spread across 8 chains, set thresholds at 2–3× the observed
spread on a known-good fit. If the empirical chain spread on a converged typhoid
scout is e.g. 1.2% of bound width, threshold at 5% leaves comfortable room for
false-negative-tolerance without missing real basin disagreement.

### NLopt success-state semantics

`nlopt::SuccessState` distinguishes `Success`, `XtolReached`, `FtolReached`,
`MaxEvalReached`. Treat:

- `Success | XtolReached | FtolReached` → converged
- `MaxEvalReached` → soft failure (hit budget without converging) — surface and
  report

Spelled out at the dispatch boundary, not lumped under a single `status` string.

### `camdl profile --algorithm` and `--backend`

Profile gets `--algorithm` and `--backend` flags that control per-cell
optimization (mirroring the fit.toml tuple schema):

```bash
# Per-cell IF2 (current default, stochastic)
camdl profile model.camdl --data X.tsv --sweep "omega=log10(1e-5,1e-2,21)"

# Per-cell deterministic NLopt (Phase 1, default Sbplx)
camdl profile model.camdl --data X.tsv --sweep "omega=..." \
    --algorithm nl-sbplx --backend ode
```

Per-cell `(algorithm, backend)` validates through the same `methods.rs`
registry as fit.toml stages — invalid pairs error with the same message
template. Multi-start per cell uses `--starts N` with LHS-drawn starts
(existing infrastructure). Per-cell convergence diagnostics use the
two-number gate above.

### Implementation outline

Files touched:

- `rust/crates/cli/src/fit/methods.rs` (new) — `MethodStatus` enum,
  `InferenceMethod` struct, the `METHODS` const registry covering all 7
  valid `(algorithm, backend)` combos, `validate_combo()`, `render_matrix()`,
  and the per-pair rejection-reason lookup table for invalid combos. ~150 LOC
  including all 7 methods' descriptions and status notes. Single source of
  truth for the validator, error messages, runtime status banners, and
  `camdl fit methods` output.
- `rust/crates/sim/src/inference/deterministic.rs` (new) — `optimize_det()`
  wrapping NLopt; takes the algorithm enum (`NlSbplx | NlBobyqa`), a closure
  for the deterministic forward sim + obs likelihood scoring. Pure function,
  no global state.
- `rust/crates/cli/src/fit/config_v2.rs` — replace `method: String` with
  `algorithm: String, backend: String` in `Stage`. Validate via
  `methods::validate_combo()` at config-load time. Algorithm-specific knobs
  (chains, tolerance, etc.) stay in the same `Stage` struct; the
  `(algorithm, backend)` pair determines which knobs are read. Identity /
  non-identity payload partition extends naturally.
- `rust/crates/cli/src/fit/nlopt_stage.rs` (new) — per-stage runner mirroring
  `pmmh.rs::run_stage`. Dispatches `optimize_det` per chain via rayon, writes
  per-chain `final_params.toml`, aggregates winner, writes `fit_state.toml`.
- `rust/crates/cli/src/fit/mod.rs` — dispatch arm switches on
  `(algorithm, backend)` tuple instead of single method string.
- `rust/crates/cli/src/fit/runner.rs` — add a
  `compute_ode_loglik(config, params)` helper alongside the existing
  `run_quick_pfilter`. Not a refactor of any shared seam:
  `MultiStreamObsModel::log_likelihood(state, obs_idx, params)` is already
  cleanly separated from PF particle indexing — it just needs a `ParticleState`
  (counts + flow_accumulators), and the existing `OdeSim::run` produces
  snapshots that build a `ParticleState` directly via the existing
  `ode.rs::to_states` rounding path. ~30 LOC for the new helper, ~10 LOC for any
  `Trajectory.snapshots_at` glue.
- `rust/crates/cli/src/main.rs` + `args/mod.rs` — new `camdl fit methods`
  subcommand that prints the `render_matrix()` output. ~40 LOC including
  the clap subcommand + dispatch.
- `rust/crates/cli/src/profile.rs` + `args/mod.rs` — add `--algorithm`
  and `--backend` flags (mirroring the fit.toml tuple schema). Per-cell
  dispatch validates `(algorithm, backend)` through the same
  `methods::validate_combo()` as fit stages.
- `rust/crates/cli/src/survey.rs` — reroute `eval_point_simulate` through
  `compute_ode_loglik` instead of the 1-particle bootstrap PF. ~30 LOC
  swap. Bump `SurveyInputs.canonical_hash` version tag so prior
  `--eval simulate` cache entries are invalidated. Update run-start banner
  to name the change so users see what they got.
- `rust/crates/cli/src/run_meta.rs` — `MethodKind` becomes
  `MethodKind { algorithm: String, backend: String }` (was a single tag).
- Existing fit.toml fixtures and golden files: bulk rename `method = "if2"`
  → `algorithm = "if2"\nbackend = "chain_binomial"`. Mechanical, ~5–10
  files, scriptable.
- Tests: per-stage unit tests; a typhoid integration test as the headline
  diagnostic experiment; a survey regression test that confirms
  `--eval simulate` numbers are within the documented sub-nat bound of the
  prior 1-particle PF values for typhoid-class N (i.e. the behaviour change
  doesn't surprise consumers expecting "deterministic eval" — they get a
  *more* deterministic eval, not a wildly different one); a validation test
  that exercises every invalid `(algorithm, backend)` pair through
  `validate_combo()` and confirms error messages name the right alternative.

Reuse paths:

- `OdeSim` — forward simulator, already in sim crate. Phase 1 is its first
  inference-side consumer.
- `MultiStreamObsModel` + `ObservationModel` — obs likelihood scoring (works
  on `ParticleState`, which `OdeSim` snapshots build directly via
  `ode.rs::to_states`)
- `fit::init::build_chain_starts` — LHS multi-start
- `fit::runner::build_if2_params_from_specs` — bounds resolution (fit.toml >
  model)
- `Capabilities::OVERDISPERSION` — auto-reject overdispersed models
- Stage config + provenance + CAS — same patterns

### Estimated cost

~550 LOC across implementation + ~250 LOC tests, ~1.5 weeks including the
diagnostic experiment and docs. Up from the original ~400 LOC estimate after
factoring in:

- `methods.rs` registry (~150 LOC) — single source of truth for the
  `(algorithm, backend)` matrix, status notes, error messages, and
  `camdl fit methods` output. Pays for itself across all three phases.
- Tuple-schema migration of fit.toml fixtures and golden files (~50 LOC of
  test-fixture updates, mostly mechanical).
- `camdl fit methods` subcommand (~40 LOC) — pure rendering of the registry.

## Phase 2 — `mh` on `ode`

### Scope

Vanilla Metropolis-Hastings on the deterministic ODE marginal likelihood. The
Bayesian counterpart to NLopt that doesn't need gradients — useful when:

- Hierarchical priors or other non-conjugate Bayesian structure that NLopt can't
  express
- Phase 3 (NUTS) hasn't shipped yet
- Posterior shape is awkward enough that NUTS misbehaves and we want a robust
  fallback
- Cross-validation against PMMH's stochastic posterior (do they agree in
  low-noise regimes?)

### Algorithm and defaults

Same adaptive Metropolis machinery PMMH already uses (Haario et al. 2001 —
adaptive proposal SD from sample covariance; warm-up phase before adaptation
kicks in). The only difference vs PMMH is the likelihood evaluator:

| Component            | PMMH                                      | MH                             |
| -------------------- | ----------------------------------------- | ------------------------------ |
| Likelihood evaluator | Bootstrap PF (noisy estimator)            | Single ODE forward sim (exact) |
| Proposal             | Gaussian random walk on transformed scale | Same                           |
| Adaptation           | Haario adaptive                           | Same                           |
| Acceptance ratio     | Pseudo-marginal                           | Standard MH                    |

Defaults:

```toml
[stages.posterior]
algorithm   = "mh"
backend     = "ode"
chains      = 4
iterations  = 50000
burn_in     = 5000
thin        = 5
adapt       = true
adapt_start = 2000   # iter where adaptation kicks in
init_method = "lhs"
```

**`adapt_start = 2000` rationale**: Haario adaptation needs enough samples per
dimension to estimate the sample covariance reliably. At d=8 estimated
parameters, 2000 iters → ~250 samples per dimension before the sample covariance
becomes the proposal — enough for a stable Cholesky. PMMH's existing default of
300 was tuned for low-dimensional problems and is too aggressive for d > 5; we
want the new default to behave reasonably at d ~ 10. The user-facing `--help`
text should note that for d > 10 they may want `adapt_start = 200 × d` and that
hierarchical models with many shared hyperparameters should bump this further.
Stan's NUTS gets away with much shorter warmups because its trajectory length
absorbs adaptation noise; vanilla MH random-walk has no such cushion.

### Per-eval cost vs PMMH

PMMH costs `n_particles × per-particle-step × T_obs` per acceptance check; for
typhoid 200×T=15 ≈ 3 ms. MH costs one ODE solve per acceptance check; for
typhoid ≈ 1–5 ms.

Both are roughly comparable per-iteration. The win is acceptance rate: PMMH's
noisy estimator forces conservative step sizes (Doucet's 1.7-nat target). MH on
a deterministic likelihood can take much larger steps because there's no
estimator noise — typically 5–10× higher effective sample size per wall-clock
second.

### Streaming traces

MH inherits the streaming trace infrastructure from PMMH (`TraceWriter` already
supports it). Per-chain `chain_N/trace.tsv` written incrementally during the
run; users `tail -f` for real-time chain monitoring. Same diagnostic
affordances.

### Diagnostic experiment as ship gate

Before merge: same typhoid case as Phase 1. Run `method = mh` posterior.
Compare:

1. MH posterior MAP to NLopt MLE → should agree closely (MAP and MLE coincide
   under flat priors).
2. MH posterior to PMMH posterior on the same model — do they overlap? In
   low-noise regime they should; in high-noise they shouldn't.
3. Â/ESS diagnostics — does MH mix at acceptable rate (~25–35%)?

### Implementation outline

Files touched:

- `rust/crates/sim/src/inference/adaptive_metropolis.rs` (new) — move the
  existing `AdaptiveProposal` struct from `pmmh.rs:117-179` here verbatim. The
  struct is already self-contained (Welford online mean + covariance, Cholesky
  factor, `sample_perturbation` + `update`); zero entanglement with PMMH's
  pseudo-marginal acceptance logic — its two integration points in PMMH are
  `ap.sample_perturbation(...)` for proposal generation and
  `ap.update(&theta_transformed)` after each step. ~10 LOC of mechanical
  refactor (file move + `mod` declaration + import update in PMMH).
- `rust/crates/sim/src/inference/mh_det.rs` (new) — vanilla MH on a
  deterministic-loglik closure, using the relocated `AdaptiveProposal`.
- `rust/crates/cli/src/fit/config_v2.rs` — register `("mh", "ode")` in
  `methods.rs::METHODS` plus the MH-specific knobs (chains, iterations,
  burn_in, thin, adapt, adapt_start) on `Stage`.
- `rust/crates/cli/src/fit/mh_stage.rs` (new) — per-stage runner.
- `rust/crates/cli/src/fit/mod.rs` — dispatch arm.
- `rust/crates/cli/src/run_meta.rs` — `MethodKind::Mh`.
- Tests + integration vs typhoid.

Reuse:

- All of Phase 1's deterministic eval closure
- PMMH's adaptive-covariance code (refactored to shared module)
- `TraceWriter` (already used by PGAS/PMMH)
- All convergence diagnostics (Â, ESS, acceptance rate) — already implemented
  for PMMH

### Estimated cost

~300 LOC implementation + ~150 LOC tests, ~3 days.

## Phase 3 — `nuts` on `ode`

### Scope

Gradient-based Bayesian inference (NUTS) on the deterministic ODE marginal
likelihood. The right algorithm for hierarchical-prior fits, posterior
uncertainty quantification, and any Bayesian inference where MH's random-walk
mixing is too slow for practical wall-clock time.

### Why this is simpler than NUTS-in-PGAS

Counterintuitively, NUTS-on-ODE is statistically and algorithmically simpler
than the existing PGAS-NUTS:

- Under chain_binomial, NUTS lives inside PGAS's Gibbs sweep — sees
  `p(θ | y, x_traj)` conditional on a CSMC-sampled trajectory. Gradient path
  approximates discrete binomial draws as continuous (a known soft spot).
- Under ODE, NUTS sees `p(θ | y) ∝ p(y | θ, ODE) · π(θ)` — a smooth,
  deterministic posterior. Standard textbook NUTS conditions. No CSMC. No
  discrete-event approximation. No coupling to a trajectory-update sweep
  schedule.

The existing `crates/sim/src/inference/nuts.rs` engine takes `log_prob` and
gradient as input closures — it doesn't care where they come from. So we plug in
ODE-based gradients and the NUTS algorithm itself doesn't change.

### Gradient infrastructure via existing symbolic AD

The OCaml compiler (`ocaml/lib/ir/autodiff.ml`) already does source-to-source
symbolic differentiation of rate expressions. Today it emits `rate_grad` =
`∂rate_i/∂θ_j`. For ODE sensitivity equations we also need `∂rate_i/∂Pop(C_k)` —
the same expression set, just a different "with respect to" target.

Forward sensitivity equations:

```
dx/dt = f(x, θ)              (the ODE — already solved)
dS/dt = (∂f/∂x)·S + ∂f/∂θ    (sensitivity ODE — new)
```

where `f = stoichiometry · rates(x, θ)`. So:

- `∂f/∂θ = stoichiometry · ∂rates/∂θ` — direct from existing `rate_grad`.
- `∂f/∂x = stoichiometry · ∂rates/∂x` — needs new emission alongside
  `rate_grad`. Same recursion structure in `autodiff.ml`. **~50–100 LOC OCaml
  depending on how the existing `autodiff.ml` represents its "with respect to"
  target** — if already parameterised over the differentiation target, ~50 LOC
  to add a `Pop(C_k)` case to the recursion; if the target is hardcoded to
  `Param`, ~100+ LOC to thread a generic target through. We'd verify which case
  applies by reading `autodiff.ml` before Phase 3 starts; the LOC estimate
  doesn't change merge readiness either way.

Then chain rule at obs times:

- `∂log p(y_t|x_t) / ∂x_t` — score function of the obs distribution. Closed form
  per distribution; mostly already in `obs_loglik.rs`.
- Multiply: `∂log p / ∂θ = (∂log p/∂x) · S(t)` at each obs time, sum.

### Real-valued obs evaluation (load-bearing for NUTS)

Phase 1 uses the existing obs-eval path which expects
`ParticleState.counts: Vec<i64>` — ODE state gets rounded at snapshot time
before the obs likelihood is evaluated. For Phase 1 (NLopt, gradient-free) and
Phase 2 (MH on the rounded loglik) this is fine: at typhoid-scale Poisson rates
(`λ ≈ 500,000`) the rounding-induced loglik change is `~10⁻⁶`, deep in the
optimizer's numerical noise floor.

For Phase 3 (NUTS), rounding is **not** acceptable. The gradient sees a
piecewise-constant function of the continuous ODE state wherever rounding snaps
to the next integer, which is undefined-derivative at every integer boundary.
NUTS will not handle this gracefully — expect spurious divergences clustering at
integer boundaries, especially in low-count regimes.

**Design decision: Phase 3 uses continuous obs evaluation, no rounding.** The
obs likelihood expressions natively accept real-valued state — Poisson
`log p(y|λ)` works for any positive `λ`, NegBin `(μ, k)` for positive `μ`,
Normal `(μ, σ)` for any real `μ`. The path forward is to extend the obs eval
entry point so it can take `f64`-valued compartment counts, then bypass the
rounding step that the Phase 1/2 path uses.

camdl already has the right infrastructure pattern for this:
`EvalCtx.int_float_override` (`crates/sim/src/ode.rs:64`). The ODE solver uses
this to evaluate rate expressions at full f64 compartment values during substeps
without rounding through `IntState`. Phase 3 extends the same pattern to obs
eval: a parallel
`MultiStreamObsModel::log_likelihood_continuous(real_counts: &[f64], ...)` entry
point that uses `int_float_override` to evaluate the obs expressions at the
unrounded ODE state.

**Cost**: ~100–150 LOC to plumb the override through `eval_likelihood_resolved`
and the projection helpers, plus a parallel `with_scratch_real_from_counts`
helper. Not surfaced in the original LOC estimate; revising Phase 3 cost upward
by ~150 LOC (now ~600–650 LOC total).

For Phase 1 / Phase 2, the rounded path stays — it's correct for those
algorithms and reuses the existing `ParticleState`-based obs eval verbatim. No
work needed there.

### Sensitivity-ODE solver

`OdeSim` is already generic over dimension — state is `Vec<f64>` for both
`int_vals` and `real_vals` (`crates/sim/src/ode.rs:95-96, 160-161`), with the
RK4 stepper iterating over Vec lengths. **No type surgery needed** to support an
augmented (n + n·d)-state system; we just allocate a larger Vec.

**Architecture decision: Option A (wrapper) for v1.** Two viable architectures:

- **Option A (wrapper)** — Run the existing state-only `OdeSim` step, then run a
  separate sensitivity-ODE step with the just-computed state. The sensitivity
  ODE is linear in S given the state and Jacobians, so it's cheaper than the
  state ODE; total per-step cost ~2× state-only. Cleaner module boundary; the
  gradient-correctness validation (finite-diff comparison) can independently
  inspect state and sensitivity paths. **This is what we ship in v1.**
- **Option B (stacked)** — Augment state to `[x; S_flat]` length `n + n·d`,
  single RK4 step over the joint system. Faster (one set of stage evaluations
  instead of two), but couples state and sensitivity at the stepper level —
  harder to validate piecewise.

The Option A speed cost is ~10–20% slower at d=8; negligible vs the gradient
cost itself. Wrapper boundary lets us swap to Option B later without touching
`OdeSim` internals.

State dim under Option A: `n` for the state ODE step + `n·d` for the sensitivity
ODE step (run separately). For typhoid n=15, d=8: 15 + 120 = 135 ODEs total per
likelihood eval. The sensitivity-specific code is the right-hand-side
construction:

```rust
fn rhs_with_sensitivity(t: f64, y: &[f64], dy: &mut [f64], ...) {
    // y = [x; S_flat]   length n + n*d
    let (x, s_flat) = y.split_at(n);
    // 1. Compute rates and dx = stoich · rates
    let rates = eval_rates(x, theta, t);
    for i in 0..n { dy[i] = stoichiometry · rates [i]; }
    // 2. Compute dS = J_x · S + J_θ
    let j_x = eval_state_jacobian(x, theta, t);   // uses new state_grad
    let j_theta = eval_param_jacobian(x, theta, t); // uses existing rate_grad
    let s_mat = matrix_view(s_flat, n, d);
    let ds_mat = matrix_view_mut(&mut dy[n..], n, d);
    matmul_into(&j_x, &s_mat, &mut ds_mat);
    add_into(&mut ds_mat, &j_theta);
}
```

~150 LOC including matrix utilities.

### Events / interventions

Two cases for v1:

- **Parameter-independent events** (e.g., `add(I, 100)` at fixed time) —
  sensitivity propagates trivially: `S(t_e+) = S(t_e-)`. The discontinuity in
  `x` doesn't change `S` because the modification isn't `θ`-sensitive.
  Event-time matching uses the existing event-time machinery in OdeSim.
- **Parameter-dependent events** (e.g., `add(I, N0 · θ_seed)`) — symbolic AD
  over the event expression handles this:
  `S(t_e+) = S(t_e-) + ∂(modification)/∂θ`. Same machinery as rate-expression
  AD.

**Out of scope for v1: reactive interventions** (event time depends on θ via
implicit-function condition like "fire when I > threshold(θ)"). These need
event-time sensitivities via the implicit-function theorem — solvable but adds
significant complexity. The endemic-fitting use cases that motivate this work
don't have reactive interventions; we ship without and add later if needed.

### Cost vs alternatives

Per-eval cost for typhoid SIRC (n=15, d=8):

| Method                         | Sim cost                | Gradient cost     | Total per eval |
| ------------------------------ | ----------------------- | ----------------- | -------------- |
| chain_binomial PF (PMMH/PGAS)  | 200 particles × T=15    | N/A (no gradient) | ~3 ms          |
| ODE state-only (NLopt, MH)     | 1 sim × T=15            | N/A               | ~3 ms          |
| ODE state + sensitivity (NUTS) | 1 sim with 9× state dim | included          | ~10 ms         |

NUTS pays ~3× per-eval over MH for the gradient information. But NUTS typically
gets 100–1000× higher effective sample size per wall-clock second on smooth
posteriors than MH or PMMH, because it can take long trajectories that
random-walk methods cannot. Net: NUTS is the right algorithm for posterior
inference on smooth deterministic likelihoods.

### Diagnostic experiment as ship gate

Before merge:

1. Validate gradient correctness vs finite differences on the typhoid model:
   `‖grad_symbolic - grad_finitediff‖_∞ < 1e-4` across all estimated params.
2. Run NUTS posterior on the typhoid case. Compare to MH posterior (Phase 2) on
   the same model — should agree on posterior shape; NUTS should achieve higher
   ESS per wall-clock second.
3. Validate against PGAS posterior on the same data in low-noise regime — should
   agree (since the deterministic and stochastic likelihoods converge there).

### Implementation outline

Files touched:

- `ocaml/lib/ir/autodiff.ml` — emit `state_grad` alongside `rate_grad`. Mirror
  the recursion. ~50 LOC.
- `ocaml/lib/ir/serialize.ml` + `deserialize.ml` — IR schema additions for
  `state_grad`. ~30 LOC.
- `ir/schema.json` + version bump. Backward-compat: missing `state_grad` means
  "no NUTS available" — non-NUTS methods unaffected.
- `rust/crates/ir/src/` — Rust types matching OCaml additions.
- `rust/crates/sim/src/ode/sensitivity.rs` (new) — sensitivity-ODE solver. ~150
  LOC.
- `rust/crates/sim/src/inference/det_grad.rs` (new) — assembles
  `(log_prob, gradient)` from sensitivity-solver output + obs likelihood scores.
  ~100 LOC.
- `rust/crates/cli/src/fit/config_v2.rs` — register `("nuts", "ode")` in
  `methods.rs::METHODS` plus NUTS-specific knobs (chains, warmup, samples,
  dense_mass, max_tree_depth) on `Stage`.
- `rust/crates/cli/src/fit/nuts_stage.rs` (new) — wraps `nuts.rs` engine with
  the new `det_grad` source. ~150 LOC.
- `rust/crates/cli/src/fit/mod.rs` — dispatch arm.
- Tests including the gradient-vs-finite-diff validation suite.

Reuse:

- Existing `nuts.rs` engine (PGAS already uses it)
- All of Phase 1/2's deterministic eval closure
- Existing `TraceWriter` for streaming output
- Existing Â/ESS/divergence diagnostics

### Estimated cost

~600–650 LOC implementation + ~200 LOC tests, ~1.5 weeks. Up from the original
~500 LOC estimate after factoring in the real-valued obs eval path (~100–150
LOC) needed to avoid NUTS divergences at integer boundaries.

## What gets reused vs built new

| Component                                         | Source                  | Phase 1                                | Phase 2     | Phase 3                        |
| ------------------------------------------------- | ----------------------- | -------------------------------------- | ----------- | ------------------------------ |
| `OdeSim` (forward sim)                            | sim crate, existing     | reuse (first inference consumer)       | reuse       | reuse                          |
| `MultiStreamObsModel`                             | sim crate, existing     | reuse                                  | reuse       | reuse                          |
| `ObservationModel` trait                          | sim crate, existing     | reuse                                  | reuse       | reuse                          |
| `compute_obs_loglik`                              | sim crate, existing     | reuse                                  | reuse       | reuse                          |
| `fit::init::build_chain_starts` (LHS)             | gh#42, shipped          | reuse                                  | reuse       | reuse                          |
| `build_if2_params_from_specs` (bounds resolution) | gh#42-followup, shipped | reuse                                  | reuse       | reuse                          |
| `Capabilities` dispatch                           | sim crate, existing     | reuse                                  | reuse       | reuse                          |
| Stage config infrastructure                       | existing                | tuple-schema migration + new fields    | extend      | extend                         |
| `methods.rs` registry (METHODS, validate_combo)   | n/a                     | new (~150 LOC)                         | extend (1 entry) | extend (1 entry)          |
| `camdl fit methods` subcommand                    | n/a                     | new (~40 LOC)                          | reuse       | reuse                          |
| Provenance + CAS hashing                          | existing                | extend                                 | extend      | extend                         |
| `TraceWriter` (streaming)                         | existing                | n/a (NLopt has no per-iter trace need) | reuse       | reuse                          |
| Â / ESS / acceptance diagnostics                  | PMMH, existing          | new (det. variant)                     | reuse       | reuse                          |
| `nuts.rs` engine                                  | PGAS, existing          | n/a                                    | n/a         | reuse                          |
| Symbolic autodiff                                 | OCaml, existing         | n/a                                    | n/a         | extend (`state_grad` emission) |
| `nlopt` crate                                     | new dep                 | new                                    | n/a         | n/a                            |
| Sensitivity-ODE solver                            | n/a                     | n/a                                    | n/a         | new                            |

Net new infrastructure across all three phases: NLopt crate dep,
sensitivity-ODE solver, the `methods.rs` registry (single source of truth for
algorithm + backend combos), the `camdl fit methods` subcommand, and the
tuple-schema migration of `Stage`. Everything else is reuse or extension.

## Speedup estimates (rough)

Per typical workflow. **chain_binomial timings marked O are observed** (from
typhoid SIRC vignette at typhoid-issues.md, gh#40 reproducer); deterministic
timings marked P are projected from per-eval cost × estimated optimizer
convergence count and have not yet been observed end-to-end (that's the
diagnostic experiment).

| Workflow                                              | chain_binomial (O)                                | ODE deterministic (P)  | Speedup        |
| ----------------------------------------------------- | ------------------------------------------------- | ---------------------- | -------------- |
| 1-D profile likelihood (21 cells × per-cell MLE)      | ~6 h (typhoid ω profile, observed)                | ~5 s                   | ~4300×         |
| 2-D profile (11×11 cells × per-cell MLE)              | ~25 h (typhoid β-pair, observed)                  | ~2 min                 | ~750×          |
| Joint 8-param scout MLE                               | ~14 h (typhoid SIRC scout, observed)              | ~30 s                  | ~1700×         |
| Bayesian posterior (4 chains, 1000 effective samples) | PGAS ~24 h (projected from PMMH typhoid attempts) | NUTS ~30 s, MH ~10 min | ~2800× / ~140× |

Order of magnitude: **100×–1000× per workflow**. The biggest wins are workflows
that iterate the optimizer many times (profile likelihood) or that need long
chains (Bayesian posterior with NUTS).

The deterministic projections will be replaced with observed numbers in the
proposal's update once the Phase 1 diagnostic experiment runs.

## Two-likelihoods framing — required user-facing

This must appear in:

1. **`fit run` startup banner** when a deterministic method is selected — naming
   the likelihood being computed.
2. **`--help`** for `[stages.X]` and for `--method` on profile.
3. **`docs/inference.md`** as a dedicated subsection with the
   verify-don't-assume rule.
4. **chapter docs** that introduce these methods, with the diagnostic experiment
   results from the merge gate as the empirical evidence.

The non-negotiable framing: **camdl computes a different statistical object
under deterministic vs stochastic methods**. Empirically they converge in
low-noise regimes; the user should verify, not assume.

## Out of scope for v1 across all phases

- **Reactive interventions under ODE inference** (parameter-dependent event
  times). Not in typhoid-class endemic-equilibrium use cases; deferable until
  malaria/reactive-intervention modeling lands.
- **Hierarchical priors specifically for NLopt MLE.** NLopt is point-estimate
  inference; hierarchical priors only make sense under MH or NUTS.
- **Adjoint-mode autodiff for gradients.** Forward-mode sensitivity is `O(d)`
  extra solves; adjoint is `O(1)`. For `d ~ 10` the difference is small enough
  to defer; reach for adjoint when `d > 30`. **Note for future hierarchical
  models**: a typhoid-style model with shared hyperparameters across many strata
  (50 settings × 8 params = `d = 400`) makes forward-mode prohibitive — adjoint
  becomes mandatory at that scale, not optional. Worth pre-flagging so the
  future-Phase-4 implementer doesn't rediscover this constraint.
- **Stiff ODE solvers for Phase 3.** Existing OdeSim's RK integrator is fine for
  typical compartmental models. Stiff solvers add complexity; defer until a real
  model needs one.
- **Mixed PF-and-ODE chains.** A scout=ODE/refine=chain_binomial pipeline works
  via `starts_from`; we don't try to interleave at finer granularity than stage
  boundaries.
- **`--method auto`** that picks between deterministic and stochastic algorithms
  based on model capabilities. The user should make this choice explicitly per
  stage; auto-selection on inference algorithm is too high-stakes for silent
  magic. (Compare to `camdl survey --eval auto` which is fine because survey is
  diagnostic.)

## Risks and tradeoffs

### Diagnostic experiment may invalidate the framing

If the typhoid case shows MLEs disagree significantly between `method = if2` and
`method = nlopt` even at large populations, the "two likelihoods converge" claim
doesn't hold and the docs guidance becomes much more nuanced. Mitigation: this
is the merge gate; we run it before declaring Phase 1 done.

### NLopt C-FFI build cost

`nlopt = "0.8"` wraps libnlopt via C-FFI. Linux/macOS-arm64 verified to work
(used briefly in the closed gh#40 PR before revert). Windows CI status uncertain
— needs check before Phase 1 ships. If Windows breaks, gate behind
`--features nlopt` (default-on for Linux/macOS, opt-in for Windows users) and
document the build-from-source path in install instructions.

### Mixed-mode pipelines may behave surprisingly

A user combining deterministic scout with stochastic refine sees scout finding a
different basin than refine because they're optimizing different likelihoods.
The runtime banner when a stage's `starts_from` crosses a likelihood boundary
must say so explicitly:

```
warning: stage 'refine' (algorithm=if2, backend=chain_binomial) starts from
  stage 'scout' (algorithm=nl-sbplx, backend=ode). These methods compute
  different statistical objects (p(y|θ) vs p(y|θ, ODE_skeleton)). The
  starting point is taken as-is; this stage's convergence diagnostics
  (Â, decibans-spread) are independent and must be evaluated from
  scratch — do NOT treat scout's convergence verdict as evidence that
  this stage starts in a good basin for the chain_binomial likelihood.
```

The downstream stage is required to run its own convergence diagnostics
regardless of the upstream stage's verdict. The closed-gh#40 lesson generalises
here: convergence on one likelihood is not convergence on another, even when
they agree empirically in the bulk of parameter space.

### Phase 3 OCaml IR schema bump

Adding `state_grad` is backward-compat (missing field means "no NUTS
available"), but it does require a schema version bump and golden-file
regeneration. Standard procedure per CLAUDE.md (atomic commit: schema + both
language changes + golden files together).

### Phase 3 NUTS divergences at integer boundaries

If we accidentally ship Phase 3 against the rounded obs-eval path (Phase 1's),
NUTS will diverge at integer boundaries because the gradient is undefined at
those discontinuities. Observable as: divergent transitions clustered at
small-integer-count regions of parameter space, or systematic underestimation of
posterior mass near low-count basins. Mitigation: the Phase 3 design decision
(real-valued obs eval, no rounding) explicitly addresses this; the
gradient-vs-finite-difference validation in Phase 3's diagnostic experiment will
catch any regression to the rounded path. Worth naming as a class of failure to
test for explicitly during Phase 3 work.

## Phasing rationale

Each phase ships independently and validates the architecture before scaling:

- **Phase 1 (NLopt)** delivers immediate value (equilibrium-fitting use case,
  profile-likelihood speedup) and validates the cross-cutting
  algorithm-replacement architecture against typhoid. If Phase 1's diagnostic
  experiment fails, we learn that before sinking time into Phase 2/3.
- **Phase 2 (MH)** is small relative payoff but very low risk — pure reuse of
  existing PMMH machinery with a different likelihood evaluator. Ships in ~3
  days. Provides a Bayesian path that doesn't depend on Phase 3 being ready.
- **Phase 3 (NUTS)** is the highest-payoff Bayesian path but depends on Phase
  1's deterministic eval infrastructure being in place. Reasonable to defer
  until Phase 1 ships and we have downstream validation.

Recommended order: Phase 1 → Phase 2 (parallel possible) → Phase 3 after Phase 1
validation.

## What lands first

`gh#NN` filed against this proposal as a tracking issue covering all three
phases. Phase 1 is the first PR; Phase 2 and Phase 3 follow as independent PRs.
Each phase's PR includes:

1. Implementation
2. Per-stage unit tests
3. Typhoid integration test (the diagnostic experiment)
4. Docs update (`docs/inference.md` + relevant chapters)
5. Two-likelihoods framing in `fit run` banner

No phase ships without its diagnostic experiment passing.

## Prior work attribution

This proposal incorporates the closed gh#40's correct technical observations
(Sbplx default, two-likelihoods framing, NLopt success-state semantics,
multi-start convergence gate replacement) while correcting its scope mistake
(profile-only) and integration mistake (backend-flag vs algorithm-explicit). The
closed gh#40 author and reviewer get implicit credit; the substantive design
improvements come from the post-merge review, the gh#42 LHS work, and the closed
gh#40's PR review thread. The pressure-test review (Q1/Q2/Q3 coupling-point
checks, the rounding-discontinuity observation for NUTS, the `adapt_start`
calibration for d > 8) confirmed the approach is buildable as proposed and
surfaced the Phase 3 real-valued obs eval requirement that this iteration
documents explicitly.
