---
status: proposal
date: 2026-05-04
authors: camdl-side, prompted by endemic-equilibrium age-stratified inference use case
target: three independent phases, each independently shippable; Phase 1 ~1 week, Phase 2 ~3 days, Phase 3 ~1 week
supersedes: docs/dev/proposals/2026-05-02-ode-backend-deterministic-inference.md (closed gh#40)
---

# ODE-Inference Algorithms: NLopt, MH, NUTS

## TL;DR

camdl's three current inference algorithms (IF2, PGAS, PMMH) all wrap a particle filter and assume **time-series likelihoods** under a stochastic process kernel. For users fitting deterministic-equilibrium models (endemic age-stratified incidence, large-population stratified epi models), the particle filter is structurally redundant — multiple particles give identical trajectories under ODE — and we burn 100×–1000× more compute than necessary.

This proposal adds three new algorithm-explicit stage methods to fit.toml, each implicitly running on the ODE backend:

| Method | Algorithm | Phase | LOC |
|---|---|---|---|
| `Stage::NLopt` | Deterministic MLE via NLopt (Sbplx default) | 1 | ~400 |
| `Stage::MH` | Vanilla Metropolis-Hastings on deterministic marginal likelihood | 2 | ~300 |
| `Stage::NUTS` | Gradient-based Bayesian via NUTS + forward sensitivity ODE | 3 | ~500 |

Existing methods (`if2`, `pgas`, `pmmh`) keep their current chain_binomial PF semantics unchanged. The two paths coexist; users pick the algorithm that matches their inference question.

Each phase is independently shippable, validated against a worked typhoid case as the merge bar, and reuses substantial infrastructure already in tree (LHS init from gh#42, bounds resolution, survey diagnostics, symbolic autodiff for Phase 3).

## Motivation

### The endemic-equilibrium use case

Stratified epi models fit to age-stratified incidence in endemic settings (typhoid SIRC, malaria EIR-by-age, polio age serology, MMC HIV) share a common likelihood structure:

```
p(observed_age_setting_incidence | θ)
  ≈ Poisson(observed | predicted_equilibrium_incidence(θ) · py_at_risk)
```

The data is a **marginal age-incidence distribution at equilibrium**, not a time series. The likelihood depends only on the model's steady-state output, not its trajectory through time. For ergodic models with sufficiently large per-cell populations (typhoid: ~10⁶ per setting × age bin), process noise washes out at equilibrium and the deterministic and stochastic likelihoods converge empirically.

Today camdl forces users through the chain_binomial PF for these fits (because the PF is the only inference path). Per typhoid vignette: 50-year sim × 200 particles × 50 IF2 iterations × 8 chains = 14 hours wall time for a joint MLE that the same data could pin down via deterministic optimization in ~30 seconds.

### The deeper observation

"ODE backend" sounds like a backend swap — independent of the inference algorithm. It isn't. Under ODE the particle filter is structurally redundant:

- Bootstrap PF + ODE: all N particles give identical trajectories per θ. ESS = N forever. Resampling no-op. Marginal likelihood is *exact* (not Monte Carlo) at N=1.
- IF2 + ODE: IF2's cooling-perturbation loop assumes per-particle parameter perturbations produce per-particle trajectory variance. With ODE, perturbations across particles still yield identical trajectories. IF2 collapses to a noisy gradient-free hill-climber — not what IF2 was designed to do.
- PGAS + ODE: degenerate CSMC step (only one trajectory per θ). NUTS-on-θ would still work but on the marginal likelihood — i.e. just vanilla NUTS without the Gibbs sweep.
- PMMH + ODE: PF-inside-MH does nothing. PMMH becomes vanilla MH on the deterministic marginal likelihood.

The right framing: under ODE, each algorithm should be replaced with its deterministic-likelihood equivalent. There is no coherent "method=if2 + backend=ode" — that combination just runs IF2's machinery against an objective the machinery isn't suited for.

### Lessons from the closed gh#40

The previous proposal (`2026-05-02-ode-backend-deterministic-inference.md`, closed) tried to ship `--backend ode` as a transparent backend swap on profile only. The PR (#41, closed unmerged) revealed:

1. **Profile-only scope was wrong.** ODE inference is cross-cutting — fit, profile, Bayesian routines all want it for the same use cases.
2. **"backend=ode + method=if2" is incoherent naming.** The user is really asking for a different algorithm; the backend flag was a misleading abstraction.
3. **Multi-start collapse.** Deterministic optimizers from a single starting point find one basin; the closed PR didn't draw multi-start LHS, which made convergence-gate basin-spread leg non-informative. (gh#42 LHS init has since shipped — this barrier is now removed.)
4. **Diagnostic experiment deferred.** The "two likelihoods converge in low-noise regimes" empirical check was punted to a handoff doc rather than gated on merge.

This proposal incorporates all four corrections explicitly.

## Architecture

### Three new stage methods, additive

```toml
# Existing — unchanged, stochastic chain_binomial PF
[stages.scout_stochastic]
method      = "if2"
chains      = 8
particles   = 200
iterations  = 50
init_method = "lhs"   # gh#42

# NEW — Phase 1: deterministic MLE via NLopt
[stages.scout_det]
method     = "nlopt"
algorithm  = "sbplx"          # bobyqa | sbplx | cobyla | isres | crs2
chains     = 8                 # LHS-drawn multi-start, take best
tolerance  = 1e-6              # xtol_rel
max_evals  = 5000              # per-chain budget
init_method = "lhs"            # default; same dispatcher as gh#42

# NEW — Phase 2: deterministic Bayesian via vanilla MH
[stages.posterior_mh]
method      = "mh"
chains      = 4
iterations  = 50000
burn_in     = 5000
thin        = 5
adapt       = true             # adaptive Metropolis (Haario et al. 2001)
adapt_start = 500
init_method = "lhs"

# NEW — Phase 3: deterministic Bayesian via NUTS (gradient-based)
[stages.posterior_nuts]
method      = "nuts"
chains      = 4
warmup      = 1000
samples     = 1000
dense_mass  = true             # full covariance preconditioner
max_tree_depth = 10
init_method = "lhs"
```

### Why naming the algorithm rather than the backend

The backend choice is **structurally bound** to the algorithm — there's no chain_binomial-MH that makes inference sense (you'd get an unbiased noisy estimator of `log p(y|θ)`, but then PMMH already does that better with the PF). Treating ODE as implicit-with-method makes the user-facing decision tree cleaner:

> "Which algorithm matches my inference question?"

rather than the matrix:

> "Which method × which backend × which gradient strategy?"

### Mixed-mode pipelines

`starts_from` handoff works across algorithm-mode boundaries:

```toml
[stages.scout]   method = "nlopt" ...    # fast deterministic MLE for basin-finding
[stages.refine]  method = "if2"   ...    starts_from = "scout"  # stochastic refinement
[stages.posterior] method = "pgas" ...   starts_from = "refine"  # stochastic Bayesian
```

or the reverse:

```toml
[stages.scout]   method = "if2"    ...
[stages.refine]  method = "nlopt"  ...   starts_from = "scout"   # deterministic polish
```

The handoff just consumes prior `fit_state.toml` for `start_values`; the consumer doesn't care what algorithm produced them. This composes naturally and lets users build pipelines that play to each algorithm's strengths.

### CAS / RunKind integration

Each new method gets a `Stage::*` enum variant in `config_v2.rs` and a `MethodKind::*` in `run_meta.rs`. The existing `RunKind::FitStage` discriminator already carries the method tag — no new top-level RunKind variants needed. Provenance hashing extends naturally (each new stage variant gets its own canonical `identity_payload()` / `non_identity_payload()` partition).

## Phase 1 — `Stage::NLopt`

### Scope

Deterministic MLE via NLopt over the ODE marginal likelihood. The biggest single use case (profile likelihood, scout-MLE basin finding, equilibrium fitting). Validates the cross-cutting algorithm-replacement architecture against a real worked example before Phase 2/3 scale it.

### Algorithm and defaults

NLopt crate (`nlopt = "0.8"`, MIT-licensed Rust wrapper around Steven Johnson's C library). Five algorithms exposed in v1:

| Flag | NLopt name | Use case | Default? |
|---|---|---|---|
| `sbplx` | `LN_SBPLX` | Robust to boundary non-smoothness; safe default for compartmental likelihoods | ✓ |
| `bobyqa` | `LN_BOBYQA` | When objective is known smooth — faster on those |  |
| `cobyla` | `LN_COBYLA` | Active linear-inequality constraints (rare) |  |
| `isres` | `GN_ISRES` | Global multi-modal "is this the basin?" pass |  |
| `crs2` | `GN_CRS2_LM` | Global controlled random search; faster than ISRES |  |

Sbplx default per the closed gh#40 review's correct point: compartmental likelihoods are smooth in the interior of the parameter box but non-smooth at boundaries (degenerate states) and where event timing depends on parameter values. BOBYQA's quadratic trust region fails badly in those regions; Sbplx (Nelder-Mead variant) doesn't.

### Multi-start

`chains = N` draws N LHS-spread starting points via existing `fit::init::build_chain_starts` (gh#42, scale-aware via `Transform`). Each chain runs an independent NLopt optimization to convergence. Best-loglik chain is the winner. Same multi-start machinery already validated under stochastic IF2 — no new infrastructure.

### Per-eval cost

| Model | Per-eval cost |
|---|---|
| Typhoid SIRC (T=15 obs, N=10⁶) | ~1–5 ms |
| Boarding school SIR (T=14) | ~0.5 ms |
| He measles SEIR (T=1043 weekly) | ~10–50 ms |

NLopt typically converges in 50–500 evaluations per chain. Total scout cost: chains × evals × per-eval. For typhoid 8-chain joint MLE: 8 × 200 × 3 ms ≈ 5 seconds vs 14 hours under chain_binomial — ~10000× speedup.

### Two likelihoods — load-bearing framing

User-facing docs and `fit run` log output must surface this clearly:

> When fit with `method = if2` (or any chain_binomial method), camdl computes `p(y|θ)` under the stochastic chain-binomial process kernel. When fit with `method = nlopt` (or any deterministic method), camdl computes `p(y|θ, ODE_skeleton)` — a different statistical object. In low-noise regimes (large populations, no overdispersion) these converge empirically. In high-noise regimes they don't. The right question is which likelihood matches your scientific use case, not which is faster.

This must appear:
- In `fit run` startup banner when a deterministic method is selected
- In `--help` for stages config
- In `docs/inference.md` as its own subsection
- In any chapter docs that introduce the deterministic methods

### Diagnostic experiment as ship gate

Before merge: take the typhoid model at the smallest stratum population in the SIRC fit (smallest cell ~5,000 — the boundary of the "deterministic equilibrium" regime). Fit with both `method = if2` and `method = nlopt`. Compare MLEs ± per-method within-method spread. Three possible outcomes:

1. MLEs agree to within within-method spread → docs say "for stratified equilibrium models with population ≥ 5,000 per cell, the two likelihoods agree empirically."
2. MLEs diverge meaningfully → docs say something more nuanced (specific population threshold, or population-dependent caveat).
3. NLopt fails to converge cleanly on this model → flags an algorithm issue we need to solve before merge.

Half a session of work; gates merge.

### DSL compatibility

`Capabilities::OVERDISPERSION` already structurally rejects overdispersed models from running on ODE — see `crates/sim/src/lib.rs`. The dispatch layer scans `CompiledModel::required_capabilities()` and rejects backend-mismatched models before inference starts. So a model using `overdispersed(rate, σ²)` will hard-error at fit time with `method = nlopt`. Clear error message:

```
error: stage 'scout' uses method=nlopt (deterministic ODE) but the model
       requires the OVERDISPERSION capability. Methods nlopt/mh/nuts run
       on the ODE backend, which is not compatible with overdispersed()
       process noise. Use method=if2/pgas/pmmh for stochastic-process
       inference, or remove overdispersed() from the rate expressions
       if the noise is not load-bearing for your inference question.
```

### Convergence diagnostics for NLopt chains

IF2's compound gate (chain-agreement Â on iteration trajectories + decibans-spread across chains) doesn't carry over directly — NLopt chains are deterministic optimizers, not stochastic chains, so iteration-trajectory Â is undefined. The intent generalizes:

- **Leg 1: did chains agree on the basin?** Compare final parameter vectors across N starts. Two-number gate: relative range vs bound width AND absolute range. Refuse only if both exceed thresholds.
- **Leg 2: was the agreed basin actually good?** Loglik spread across N converged chains (decibans). Same threshold semantics as IF2.

Verdict line UX matches IF2's:

```
chain-agreement: rel range = X% bound | abs range = Y nat. units   ✓/✗
loglik-eval:     Δ = X dB / threshold Y dB                         ✓/✗
```

### NLopt success-state semantics

`nlopt::SuccessState` distinguishes `Success`, `XtolReached`, `FtolReached`, `MaxEvalReached`. Treat:
- `Success | XtolReached | FtolReached` → converged
- `MaxEvalReached` → soft failure (hit budget without converging) — surface and report

Spelled out at the dispatch boundary, not lumped under a single `status` string.

### `camdl profile --method nlopt`

Profile gets a `--method` flag that controls per-cell optimization:

```bash
# Per-cell IF2 (current default, stochastic)
camdl profile model.camdl --data X.tsv --sweep "omega=log10(1e-5,1e-2,21)"

# Per-cell deterministic NLopt (new)
camdl profile model.camdl --data X.tsv --sweep "omega=..." --method nlopt
```

Multi-start per cell uses `--starts N` with LHS-drawn starts (existing infrastructure). Per-cell convergence diagnostics use the two-number gate above.

### Implementation outline

Files touched:
- `rust/crates/sim/src/inference/deterministic.rs` (new) — `optimize_det()` wrapping NLopt; takes a closure for the deterministic forward sim + obs likelihood scoring. Pure function, no global state.
- `rust/crates/cli/src/fit/config_v2.rs` — add `Stage::NLopt { ... }` variant, defaults, validation, identity/non-identity payload partition.
- `rust/crates/cli/src/fit/nlopt_stage.rs` (new) — per-stage runner mirroring `pmmh.rs::run_stage`. Dispatches `optimize_det` per chain via rayon, writes per-chain `final_params.toml`, aggregates winner, writes `fit_state.toml`.
- `rust/crates/cli/src/fit/mod.rs` — `Stage::NLopt { .. }` arm in dispatch.
- `rust/crates/cli/src/fit/runner.rs` — minor: factor out `build_obs_eval_closure()` so deterministic-stage code can reuse it.
- `rust/crates/cli/src/profile.rs` + `args/mod.rs` — `--method` flag, per-cell dispatch.
- `rust/crates/cli/src/run_meta.rs` — `MethodKind::Nlopt` variant.
- Tests: per-stage unit tests + a typhoid integration test as the headline diagnostic experiment.

Reuse paths (zero new infrastructure):
- `OdeSim` — forward simulator, already in sim crate
- `MultiStreamObsModel` + `ObservationModel` — obs likelihood scoring
- `compute_obs_loglik` — same as survey uses
- `fit::init::build_chain_starts` — LHS multi-start
- `fit::runner::build_if2_params_from_specs` — bounds resolution (fit.toml > model)
- `Capabilities::OVERDISPERSION` — auto-reject overdispersed models
- Stage config + provenance + CAS — same patterns

### Estimated cost

~400 LOC across implementation + ~200 LOC tests, ~1 week including the diagnostic experiment and docs.

## Phase 2 — `Stage::MH`

### Scope

Vanilla Metropolis-Hastings on the deterministic ODE marginal likelihood. The Bayesian counterpart to NLopt that doesn't need gradients — useful when:

- Hierarchical priors or other non-conjugate Bayesian structure that NLopt can't express
- Phase 3 (NUTS) hasn't shipped yet
- Posterior shape is awkward enough that NUTS misbehaves and we want a robust fallback
- Cross-validation against PMMH's stochastic posterior (do they agree in low-noise regimes?)

### Algorithm and defaults

Same adaptive Metropolis machinery PMMH already uses (Haario et al. 2001 — adaptive proposal SD from sample covariance; warm-up phase before adaptation kicks in). The only difference vs PMMH is the likelihood evaluator:

| Component | PMMH | MH |
|---|---|---|
| Likelihood evaluator | Bootstrap PF (noisy estimator) | Single ODE forward sim (exact) |
| Proposal | Gaussian random walk on transformed scale | Same |
| Adaptation | Haario adaptive | Same |
| Acceptance ratio | Pseudo-marginal | Standard MH |

Defaults:

```toml
[stages.posterior_mh]
method      = "mh"
chains      = 4
iterations  = 50000
burn_in     = 5000
thin        = 5
adapt       = true
adapt_start = 500   # iter where adaptation kicks in
init_method = "lhs"
```

### Per-eval cost vs PMMH

PMMH costs `n_particles × per-particle-step × T_obs` per acceptance check; for typhoid 200×T=15 ≈ 3 ms.
MH costs one ODE solve per acceptance check; for typhoid ≈ 1–5 ms.

Both are roughly comparable per-iteration. The win is acceptance rate: PMMH's noisy estimator forces conservative step sizes (Doucet's 1.7-nat target). MH on a deterministic likelihood can take much larger steps because there's no estimator noise — typically 5–10× higher effective sample size per wall-clock second.

### Streaming traces

MH inherits the streaming trace infrastructure from PMMH (`TraceWriter` already supports it). Per-chain `chain_N/trace.tsv` written incrementally during the run; users `tail -f` for real-time chain monitoring. Same diagnostic affordances.

### Diagnostic experiment as ship gate

Before merge: same typhoid case as Phase 1. Run `method = mh` posterior. Compare:

1. MH posterior MAP to NLopt MLE → should agree closely (MAP and MLE coincide under flat priors).
2. MH posterior to PMMH posterior on the same model — do they overlap? In low-noise regime they should; in high-noise they shouldn't.
3. Â/ESS diagnostics — does MH mix at acceptable rate (~25–35%)?

### Implementation outline

Files touched:
- `rust/crates/sim/src/inference/mh_det.rs` (new) — vanilla MH on a deterministic-loglik closure. Reuses Haario adaptive-covariance machinery from `pmmh.rs::adaptive` (factor it out into shared module).
- `rust/crates/cli/src/fit/config_v2.rs` — `Stage::MH { ... }` variant.
- `rust/crates/cli/src/fit/mh_stage.rs` (new) — per-stage runner.
- `rust/crates/cli/src/fit/mod.rs` — dispatch arm.
- `rust/crates/cli/src/run_meta.rs` — `MethodKind::Mh`.
- Tests + integration vs typhoid.

Reuse:
- All of Phase 1's deterministic eval closure
- PMMH's adaptive-covariance code (refactored to shared module)
- `TraceWriter` (already used by PGAS/PMMH)
- All convergence diagnostics (Â, ESS, acceptance rate) — already implemented for PMMH

### Estimated cost

~300 LOC implementation + ~150 LOC tests, ~3 days.

## Phase 3 — `Stage::NUTS`

### Scope

Gradient-based Bayesian inference (NUTS) on the deterministic ODE marginal likelihood. The right algorithm for hierarchical-prior fits, posterior uncertainty quantification, and any Bayesian inference where MH's random-walk mixing is too slow for practical wall-clock time.

### Why this is simpler than NUTS-in-PGAS

Counterintuitively, NUTS-on-ODE is statistically and algorithmically simpler than the existing PGAS-NUTS:

- Under chain_binomial, NUTS lives inside PGAS's Gibbs sweep — sees `p(θ | y, x_traj)` conditional on a CSMC-sampled trajectory. Gradient path approximates discrete binomial draws as continuous (a known soft spot).
- Under ODE, NUTS sees `p(θ | y) ∝ p(y | θ, ODE) · π(θ)` — a smooth, deterministic posterior. Standard textbook NUTS conditions. No CSMC. No discrete-event approximation. No coupling to a trajectory-update sweep schedule.

The existing `crates/sim/src/inference/nuts.rs` engine takes `log_prob` and gradient as input closures — it doesn't care where they come from. So we plug in ODE-based gradients and the NUTS algorithm itself doesn't change.

### Gradient infrastructure via existing symbolic AD

The OCaml compiler (`ocaml/lib/ir/autodiff.ml`) already does source-to-source symbolic differentiation of rate expressions. Today it emits `rate_grad` = `∂rate_i/∂θ_j`. For ODE sensitivity equations we also need `∂rate_i/∂Pop(C_k)` — the same expression set, just a different "with respect to" target.

Forward sensitivity equations:

```
dx/dt = f(x, θ)              (the ODE — already solved)
dS/dt = (∂f/∂x)·S + ∂f/∂θ    (sensitivity ODE — new)
```

where `f = stoichiometry · rates(x, θ)`. So:

- `∂f/∂θ = stoichiometry · ∂rates/∂θ` — direct from existing `rate_grad`.
- `∂f/∂x = stoichiometry · ∂rates/∂x` — needs new emission alongside `rate_grad`. Same recursion structure in `autodiff.ml`. ~50 LOC OCaml.

Then chain rule at obs times:

- `∂log p(y_t|x_t) / ∂x_t` — score function of the obs distribution. Closed form per distribution; mostly already in `obs_loglik.rs`.
- Multiply: `∂log p / ∂θ = (∂log p/∂x) · S(t)` at each obs time, sum.

### Sensitivity-ODE solver

State dim grows from `n` (compartments) to `n + n·d` (compartments + sensitivity matrix). For typhoid n=15 compartments, d=8 estimated params: 135 ODEs. Same RK integrator, same step size adaptation. The only sensitivity-specific code is the right-hand-side construction:

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

- **Parameter-independent events** (e.g., `add(I, 100)` at fixed time) — sensitivity propagates trivially: `S(t_e+) = S(t_e-)`. The discontinuity in `x` doesn't change `S` because the modification isn't `θ`-sensitive. Event-time matching uses the existing event-time machinery in OdeSim.
- **Parameter-dependent events** (e.g., `add(I, N0 · θ_seed)`) — symbolic AD over the event expression handles this: `S(t_e+) = S(t_e-) + ∂(modification)/∂θ`. Same machinery as rate-expression AD.

**Out of scope for v1: reactive interventions** (event time depends on θ via implicit-function condition like "fire when I > threshold(θ)"). These need event-time sensitivities via the implicit-function theorem — solvable but adds significant complexity. The endemic-fitting use cases that motivate this work don't have reactive interventions; we ship without and add later if needed.

### Cost vs alternatives

Per-eval cost for typhoid SIRC (n=15, d=8):

| Method | Sim cost | Gradient cost | Total per eval |
|---|---|---|---|
| chain_binomial PF (PMMH/PGAS) | 200 particles × T=15 | N/A (no gradient) | ~3 ms |
| ODE state-only (NLopt, MH) | 1 sim × T=15 | N/A | ~3 ms |
| ODE state + sensitivity (NUTS) | 1 sim with 9× state dim | included | ~10 ms |

NUTS pays ~3× per-eval over MH for the gradient information. But NUTS typically gets 100–1000× higher effective sample size per wall-clock second on smooth posteriors than MH or PMMH, because it can take long trajectories that random-walk methods cannot. Net: NUTS is the right algorithm for posterior inference on smooth deterministic likelihoods.

### Diagnostic experiment as ship gate

Before merge:

1. Validate gradient correctness vs finite differences on the typhoid model: `‖grad_symbolic - grad_finitediff‖_∞ < 1e-4` across all estimated params.
2. Run NUTS posterior on the typhoid case. Compare to MH posterior (Phase 2) on the same model — should agree on posterior shape; NUTS should achieve higher ESS per wall-clock second.
3. Validate against PGAS posterior on the same data in low-noise regime — should agree (since the deterministic and stochastic likelihoods converge there).

### Implementation outline

Files touched:
- `ocaml/lib/ir/autodiff.ml` — emit `state_grad` alongside `rate_grad`. Mirror the recursion. ~50 LOC.
- `ocaml/lib/ir/serialize.ml` + `deserialize.ml` — IR schema additions for `state_grad`. ~30 LOC.
- `ir/schema.json` + version bump. Backward-compat: missing `state_grad` means "no NUTS available" — non-NUTS methods unaffected.
- `rust/crates/ir/src/` — Rust types matching OCaml additions.
- `rust/crates/sim/src/ode/sensitivity.rs` (new) — sensitivity-ODE solver. ~150 LOC.
- `rust/crates/sim/src/inference/det_grad.rs` (new) — assembles `(log_prob, gradient)` from sensitivity-solver output + obs likelihood scores. ~100 LOC.
- `rust/crates/cli/src/fit/config_v2.rs` — `Stage::NUTS` variant.
- `rust/crates/cli/src/fit/nuts_stage.rs` (new) — wraps `nuts.rs` engine with the new `det_grad` source. ~150 LOC.
- `rust/crates/cli/src/fit/mod.rs` — dispatch arm.
- Tests including the gradient-vs-finite-diff validation suite.

Reuse:
- Existing `nuts.rs` engine (PGAS already uses it)
- All of Phase 1/2's deterministic eval closure
- Existing `TraceWriter` for streaming output
- Existing Â/ESS/divergence diagnostics

### Estimated cost

~500 LOC implementation + ~200 LOC tests, ~1 week.

## What gets reused vs built new

| Component | Source | Phase 1 | Phase 2 | Phase 3 |
|---|---|---|---|---|
| `OdeSim` (forward sim) | sim crate, existing | reuse | reuse | reuse |
| `MultiStreamObsModel` | sim crate, existing | reuse | reuse | reuse |
| `ObservationModel` trait | sim crate, existing | reuse | reuse | reuse |
| `compute_obs_loglik` | sim crate, existing | reuse | reuse | reuse |
| `fit::init::build_chain_starts` (LHS) | gh#42, shipped | reuse | reuse | reuse |
| `build_if2_params_from_specs` (bounds resolution) | gh#42-followup, shipped | reuse | reuse | reuse |
| `Capabilities` dispatch | sim crate, existing | reuse | reuse | reuse |
| Stage config infrastructure | existing | new variant | new variant | new variant |
| Provenance + CAS hashing | existing | extend | extend | extend |
| `TraceWriter` (streaming) | existing | n/a (NLopt has no per-iter trace need) | reuse | reuse |
| Â / ESS / acceptance diagnostics | PMMH, existing | new (det. variant) | reuse | reuse |
| `nuts.rs` engine | PGAS, existing | n/a | n/a | reuse |
| Symbolic autodiff | OCaml, existing | n/a | n/a | extend (`state_grad` emission) |
| `nlopt` crate | new dep | new | n/a | n/a |
| Sensitivity-ODE solver | n/a | n/a | n/a | new |

Net new infrastructure across all three phases: NLopt crate dep, sensitivity-ODE solver, three new stage variants. Everything else is reuse or extension.

## Speedup estimates (rough)

Per typical workflow:

| Workflow | chain_binomial | ODE deterministic | Speedup |
|---|---|---|---|
| 1-D profile likelihood (21 cells × per-cell MLE) | ~10 h | ~5 s | ~7000× |
| 2-D profile (11×11 cells × per-cell MLE) | ~25 h | ~2 min | ~750× |
| Joint 8-param scout MLE | ~14 h | ~30 s | ~1700× |
| Bayesian posterior (4 chains, 1000 effective samples) | PGAS ~24 h | NUTS ~30 s, MH ~10 min | ~2800× / ~140× |

Order of magnitude: **100×–1000× per workflow**. The biggest wins are workflows that iterate the optimizer many times (profile likelihood) or that need long chains (Bayesian posterior with NUTS).

## Two-likelihoods framing — required user-facing

This must appear in:

1. **`fit run` startup banner** when a deterministic method is selected — naming the likelihood being computed.
2. **`--help`** for `[stages.X]` and for `--method` on profile.
3. **`docs/inference.md`** as a dedicated subsection with the verify-don't-assume rule.
4. **chapter docs** that introduce these methods, with the diagnostic experiment results from the merge gate as the empirical evidence.

The non-negotiable framing: **camdl computes a different statistical object under deterministic vs stochastic methods**. Empirically they converge in low-noise regimes; the user should verify, not assume.

## Out of scope for v1 across all phases

- **Reactive interventions under ODE inference** (parameter-dependent event times). Not in typhoid-class endemic-equilibrium use cases; deferable until malaria/reactive-intervention modeling lands.
- **Hierarchical priors specifically for NLopt MLE.** NLopt is point-estimate inference; hierarchical priors only make sense under MH or NUTS.
- **Adjoint-mode autodiff for gradients.** Forward-mode sensitivity is `O(d)` extra solves; adjoint is `O(1)`. For `d ~ 10` the difference is small enough to defer; reach for adjoint when `d > 30`.
- **Stiff ODE solvers for Phase 3.** Existing OdeSim's RK integrator is fine for typical compartmental models. Stiff solvers add complexity; defer until a real model needs one.
- **Mixed PF-and-ODE chains.** A scout=ODE/refine=chain_binomial pipeline works via `starts_from`; we don't try to interleave at finer granularity than stage boundaries.
- **`--method auto`** that picks between deterministic and stochastic algorithms based on model capabilities. The user should make this choice explicitly per stage; auto-selection on inference algorithm is too high-stakes for silent magic. (Compare to `camdl survey --eval auto` which is fine because survey is diagnostic.)

## Risks and tradeoffs

### Diagnostic experiment may invalidate the framing

If the typhoid case shows MLEs disagree significantly between `method = if2` and `method = nlopt` even at large populations, the "two likelihoods converge" claim doesn't hold and the docs guidance becomes much more nuanced. Mitigation: this is the merge gate; we run it before declaring Phase 1 done.

### NLopt C-FFI build cost

`nlopt = "0.8"` wraps libnlopt via C-FFI. Linux/macOS-arm64 verified to work (used briefly in the closed gh#40 PR before revert). Windows CI status uncertain — needs check before Phase 1 ships. If Windows breaks, gate behind `--features nlopt` (default-on for Linux/macOS, opt-in for Windows users) and document the build-from-source path in install instructions.

### Mixed-mode pipelines may behave surprisingly

A user combining deterministic scout with stochastic refine sees scout finding a different basin than refine because they're optimizing different likelihoods. Document this loudly in the `starts_from` cross-mode handoff path: "the prior stage used a different statistical object; this stage starts from there but optimizes its own likelihood — expect drift if the two likelihoods disagree."

### Phase 3 OCaml IR schema bump

Adding `state_grad` is backward-compat (missing field means "no NUTS available"), but it does require a schema version bump and golden-file regeneration. Standard procedure per CLAUDE.md (atomic commit: schema + both language changes + golden files together).

## Phasing rationale

Each phase ships independently and validates the architecture before scaling:

- **Phase 1 (NLopt)** delivers immediate value (equilibrium-fitting use case, profile-likelihood speedup) and validates the cross-cutting algorithm-replacement architecture against typhoid. If Phase 1's diagnostic experiment fails, we learn that before sinking time into Phase 2/3.
- **Phase 2 (MH)** is small relative payoff but very low risk — pure reuse of existing PMMH machinery with a different likelihood evaluator. Ships in ~3 days. Provides a Bayesian path that doesn't depend on Phase 3 being ready.
- **Phase 3 (NUTS)** is the highest-payoff Bayesian path but depends on Phase 1's deterministic eval infrastructure being in place. Reasonable to defer until Phase 1 ships and we have downstream validation.

Recommended order: Phase 1 → Phase 2 (parallel possible) → Phase 3 after Phase 1 validation.

## What lands first

`gh#NN` filed against this proposal as a tracking issue covering all three phases. Phase 1 is the first PR; Phase 2 and Phase 3 follow as independent PRs. Each phase's PR includes:

1. Implementation
2. Per-stage unit tests
3. Typhoid integration test (the diagnostic experiment)
4. Docs update (`docs/inference.md` + relevant chapters)
5. Two-likelihoods framing in `fit run` banner

No phase ships without its diagnostic experiment passing.

## Visual style attribution

This proposal incorporates the closed gh#40's correct technical observations (Sbplx default, two-likelihoods framing, NLopt success-state semantics, multi-start convergence gate replacement) while correcting its scope mistake (profile-only) and integration mistake (backend-flag vs algorithm-explicit). The closed gh#40 author and reviewer get implicit credit; the substantive design improvements come from the post-merge review, the gh#42 LHS work, and the closed gh#40's PR review thread.
