---
status: approved (ship-now)
date: 2026-05-07
target: ship in ~1–2 days — tracks gh#52
issue: https://github.com/vsbuffalo/camdl/issues/52
relates_to: gh#42 (init coverage), gh#51 (survey-seeded init)
---

# Richardson dt-convergence check at θ̂

## TL;DR

Auto-run a halving-ladder pfilter evaluation at each fit's
final-stage θ̂ to detect when the MLE is discretization-dependent.
Two-leg verdict (halving-stability + plateau-width), SE-aware
threshold mirroring the compound gate, backend-specific defaults,
and a "synth-recovery shares this dt and cannot detect dt bias by
itself" warning text. Surfaces in `camdl fit run` terminal output
+ `fit summary` + `fit_state.toml`. Standalone `camdl fit dt-check
<fit_dir>` for re-running on saved fits.

This is the third of three validators that close the "fit landed
somewhere unconvincing and it's hard to tell why" failure modes:
gh#42 picks the init method, gh#51 seeds from a paid survey,
gh#52 audits whether the basin survives finer discretization.

## Motivation — the silent-wrong-answer mode

Boarding-school SIR + Poisson + fixed I(0)=3 on T=14 daily obs,
N=763. Issue #52 has the full reproducer; the load-bearing numbers:

| MLE | ll(dt=1.0) | ll(dt=0.5) | ll(dt=0.25) | ll(dt=0.1) | spread |
|---|---:|---:|---:|---:|---:|
| dt=1.0 fit (β=1.975, γ=0.654, R₀=3.02) | −62.6 | −65.7 | −73.9 | −80.7 | **20.8 nats** |
| dt=0.1 fit (β=1.935, γ=0.500, R₀=3.87) | −76.8 | −60.0 | −58.2 | −58.7 | **1.5 nats** |

Two facts make this dangerous:

1. **Synthetic recovery passes** at the misconfigured dt — same dt
   on the simulator and on the inference loop, so the discretization
   bias cancels in the recovery error metric. The compound gate
   (Â=1.014/1.008, Δ_dB=2.4 against 30 dB) also passes. The bug only
   surfaces under PPC against real data, after the reader has been
   led to interpret the wrong fit. Anderson & May's textbook R₀ for
   this dataset is 3.5–4; the dt=1.0 fit reports 3.02.

2. **Coarse stepping rewards the wrong basin.** At dt=1.0, the
   *misconfigured* MLE is 14 nats *better* than the converged MLE
   evaluated at the same coarse dt. The discretization bias creates
   a fake basin that's only visible from the finer-dt vantage. So
   no in-fit diagnostic at dt=1.0 can detect this — the check has
   to be done after the fit, at finer dt.

The literature has the components (Roache 1998 on Richardson
extrapolation; Higham 2001 on weak/strong convergence checks for
SDE solvers) but no shipped tool for stochastic compartmental fit
pipelines does this automatically. pomp's documentation discusses
dt selection as a heuristic ("small enough that per-step transition
probability stays small," King et al. 2016 §4.2 of the JSS paper);
the He2010 vignette uses dt = 2/365 by convention; mif2 has no
post-fit convergence audit. odin/odin.dust same. Filling this gap
in camdl ships actual new validation, not a re-implementation of
existing tooling.

## Surface

### Auto-run, with optional override block

`camdl fit run` runs the dt-check automatically at the end of the
**final stage** (after the compound gate; before exit), so every
fit gets the audit by default. Per-stage override block in
fit.toml when defaults need tuning:

```toml
[stages.refine.dt_check]
n_halvings     = 2                  # default — eval at dt_fit/2 and dt_fit/4
n_particles    = 2000               # default = stage's loglik_eval n_particles
n_replicates   = 8                  # default = stage's loglik_eval n_replicates
threshold_nats = 2.0                # warn τ; --strict drops to 0.5 (chain_binomial); 0.5 default for ode_rk4
combine        = "log_mean_exp"     # match loglik_eval

[stages.refine.dt_check.disable]    # opt out for CI smoke fits / known-converged dt
enabled = false
```

CLI flags on `camdl fit run` (require `--stage` like other gh#42
overrides):

- `--dt-check / --no-dt-check` — toggle the auto-run.
- `--dt-check-halvings N` — override n_halvings.
- `--dt-check-strict` — drop threshold to 0.5 nats (chain_binomial)
  / 0.1 nats (ode_rk4).

### Standalone subcommand

`camdl fit dt-check <fit_dir>` re-runs the check on a saved fit
without redoing the inference. Useful for:

- Auditing fits produced before this feature shipped.
- Sweeping `--strict` after a routine pass to qualify research-
  quality fits.
- `--extended` to stretch n_halvings = 3 (cost 15× the loglik_eval).

### Terminal output

```
dt-convergence at θ̂ (Richardson check, Np=2000 × Nreps=8):
  dt = 1.000   ll = -62.56 ± 0.07   (fit)
  dt = 0.500   ll = -65.73 ± 0.24   Δ = -3.17 nats
  dt = 0.250   ll = -73.93 ± 0.63   Δ = -8.20 nats
  ⚠ FAIL: loglik shifted -11.4 nats from dt_fit to dt_fit/4
    (threshold τ = 2.0 nats; SE-aware floor 4·σ_max = 2.5 nats).
    MLE is discretization-dependent. Re-fit at dt ≤ 0.25 before
    interpreting θ̂.

    Note: synthetic recovery at the same dt cannot detect this
    bias — the simulator and inference loop share the same dt and
    the discretization error cancels in the recovery metric. This
    Richardson check is the supplementary validator for that
    failure mode.
```

Pass case is one-line:

```
dt-convergence at θ̂: PASS  (|Δ_leg1|=0.6, |Δ_leg2|=0.6 nats vs τ=2.0)
```

## Two-leg verdict

1. **Halving stability** — `|ll(dt_fit) − ll(dt_fit/2)| < τ`. Catches
   the first-order tail of dt error — if the loglik is still moving
   at dt_fit/2, it'll keep moving at dt_fit/4.
2. **Plateau width** — `|ll(dt_fit) − ll(dt_min)| < τ`. Catches the
   case where adjacent halvings each shift sub-τ but the cumulative
   drift exceeds τ. (Vince's reproducer: leg-1 fires at dt_fit→dt/2
   already, but plateau width is the cleaner diagnostic for the
   "small per-halving step, big total drift" case.)

Verdict states (`fit_state.toml.dt_check.verdict`):

- `"pass"` — both legs pass.
- `"marginal"` — halving-stability passes (next halving would be
  small) but plateau-width is in `[τ, 2τ]`. Soft warning, not error.
- `"fail"` — either leg exceeds τ. Hard warning; user should re-fit.
- `"skipped"` — backend has no dt parameter (Gillespie, ODE-via-
  exact-jump). The check is structurally inapplicable.

The verdict is a warning, not a blocker — the fit completes and
writes its outputs. The user reads the verdict and decides whether
to re-fit. This matches camdl's broader pattern: warn loudly, refuse
only when output would be silently wrong (e.g. the compound gate's
Â/decibans hard-fail).

## SE-aware threshold

The user-set `threshold_nats` is a *floor*, not the absolute. The
effective threshold mirrors the compound gate's
`8·σ_max·NATS_TO_DB` shape but at half the multiplier (4× rather
than 8×) because this is a per-evaluation comparison, not a chain-
level spread:

```
τ_effective = max(threshold_nats, 4 · σ_max)
```

where `σ_max = max(se(dt) for dt in ladder)`. At default
`Np=2000 × Nreps=8` and well-conditioned fits, σ ≈ 0.5 nats and
4·σ_max ≈ 2 nats — the floor and the SE-aware bound are about
equal, so the SE-awareness costs nothing in routine cases. At low
Np or PF-degeneracy regimes, σ can inflate to 1–2 nats and the
floor bumps up automatically, preventing spurious trips. The
verdict line prints both the floor and the SE-aware bound for
transparency.

## Auxiliary: PF-SE inflation signal

A dt-misconfigured fit's particle filter is structurally degenerate
at finer dt — the trajectories the coarse-dt MLE was tuned to
explain become low-probability under finer-grained dynamics, ESS
collapses, and the per-replicate loglik variance balloons. From
Vince's reproducer:

| MLE | σ(dt=0.1) | σ(dt=0.5) |
|---|---:|---:|
| dt=1.0 (misconfigured) | 1.4 nats | 0.24 nats |
| dt=0.1 (converged) | 0.03 nats | 0.06 nats |

47× SE ratio at dt=0.1 between the two MLEs. Worth surfacing as a
secondary warning because:

- The data is already computed (the ladder already records
  `se(dt)` per row).
- Costs nothing additional.
- Discriminates orthogonally: a fit where ll changes little but σ
  inflates is a different failure mode from one where ll drifts
  but σ stays small (the latter is "discretization shifts the
  expected loglik," the former is "discretization drives the PF
  off-manifold").

Trigger: warn when `σ(dt_fine) > 2 · σ(dt_fit)` non-monotonically
in the ladder. One auxiliary line in the terminal output, one
field in `fit_state.toml.dt_check`:

```
  ⚠ PF-SE inflation: σ went 0.07 → 0.24 → 0.63 nats as dt halved.
    Often co-occurs with dt-bias; the misconfigured MLE's
    trajectories are improbable under finer dynamics.
```

## Backend-specific τ defaults

Different backend convergence orders need different thresholds:

| Backend | Convergence order | τ default |
|---|---|---:|
| `chain_binomial` | O(dt) weak | 2.0 nats |
| `tau_leap` | O(dt) weak | 2.0 nats |
| `euler_sde` | O(√dt) strong / O(dt) weak | 2.0 nats |
| `ode_euler` | O(dt) | 2.0 nats |
| `ode_rk4` | O(dt⁴) | 0.5 nats |
| `gillespie` | exact (no dt) | (skipped) |

The `--strict` flag drops chain-binomial-class to 0.5 nats and
ode_rk4-class to 0.1 nats, targeting research-quality fits where
sub-nat differences matter for paper-grade conclusions.

Calibration of the ode_rk4 default needs a measles-scale repro
(He2010 measles SEIR at the published θ̂); Vince's boarding-school
reproducer is chain_binomial-only. **TODO before merge**: run the
Richardson check against He2010 at dt = 1/52 weeks (the published
choice) and confirm the converged threshold lands sub-0.5 nats.
This becomes an L9 external-validation case (`tests/external/`).

## Provenance — `fit_state.toml`

```toml
[dt_check]
verdict        = "fail"           # pass | marginal | fail | skipped
n_halvings     = 2
threshold_nats = 2.0
threshold_se_aware_nats = 2.5    # max(threshold_nats, 4·σ_max)
leg1_delta_nats = -3.17           # ll(dt_fit) - ll(dt_fit/2)
leg2_delta_nats = -11.4           # ll(dt_fit) - ll(dt_min)
pf_se_inflation = true            # auxiliary signal fired
notes = "leg-1 |Δ| = 3.17 > τ = 2.0 (1.6×); leg-2 |Δ| = 11.4 > τ (5.7×)"

[[dt_check.ladder]]
dt     = 1.0
loglik = -62.56
se     = 0.07
[[dt_check.ladder]]
dt     = 0.5
loglik = -65.73
se     = 0.24
[[dt_check.ladder]]
dt     = 0.25
loglik = -73.93
se     = 0.63
```

Mirrors the existing `[loglik_eval]` / `[gate]` block conventions.
`fit summary` reads this and renders the verdict line. Schema is
versioned implicitly via fit_state's existing
`#[serde(default)]` fields — pre-this-proposal fit_state.toml
files have no `[dt_check]` block; readers treat absence as
"unknown, not run."

## Implementation plan

| File | Change | LOC |
|---|---|---|
| `rust/crates/cli/src/fit/dt_check.rs` | New module — Richardson runner, verdict computation, terminal-output renderer | ~200 |
| `rust/crates/cli/src/fit/config_v2.rs` | New `DtCheckConfig` struct, optional `dt_check: DtCheckConfig` field on `Stage::IF2` / NLopt | ~40 |
| `rust/crates/cli/src/fit/state.rs` | New `DtCheckBlock` field on `FitState` (Option, serde-default) | ~30 |
| `rust/crates/cli/src/fit/mod.rs` | Wire auto-run after compound gate at end-of-final-stage | ~50 |
| `rust/crates/cli/src/fit/fit_summary.rs` | Render the dt_check verdict line from FitState | ~20 |
| `rust/crates/cli/src/main.rs` + `args/mod.rs` | New subcommand `camdl fit dt-check`; CLI flags `--dt-check`/`--no-dt-check`/`--dt-check-strict`/`--dt-check-halvings` | ~60 |
| Tests | Verdict logic unit tests; SE-aware threshold; PF-SE inflation; end-to-end on a tiny model | ~120 |

Total ~520 LOC. Issue estimate was 200–300; the proposal expands the
test surface and the fit_summary integration. Still 1–2 days.

The Richardson runner is mostly plumbing of existing primitives:
`runner::run_quick_pfilter_full(config, theta, n_particles, seed)`
returns `(loglik, FilterStats)`; the only new wrinkle is overriding
`SMCConfig.dt` per ladder rung. `loglik_eval::combine_with_se` does
the per-rung loglik combination already.

## Tests worth adding

1. **Verdict logic** (unit, no I/O):
   - Synthetic ladder where leg-1 trips, leg-2 passes → verdict = "fail".
   - Synthetic ladder where both legs are sub-τ → verdict = "pass".
   - Synthetic ladder where leg-1 ≈ τ but leg-2 is in [τ, 2τ] → verdict = "marginal".
   - Skipped backend → verdict = "skipped" with no ladder.

2. **SE-aware threshold** (unit):
   - Bare `threshold_nats = 2.0` with σ_max = 1.0 → effective τ = 4.0.
   - Bare `threshold_nats = 2.0` with σ_max = 0.1 → effective τ = 2.0 (floor wins).

3. **PF-SE inflation detection** (unit):
   - σ ladder [0.05, 0.10, 0.30] → fires (3× ratio at last rung).
   - σ ladder [0.20, 0.21, 0.19] → doesn't fire (no inflation).
   - σ ladder [0.10, 0.30, 0.10] → doesn't fire (non-monotonic dip back).

4. **End-to-end** (integration):
   - Tiny SIR + Poisson model, fit at dt=1.0, run dt-check at
     halvings=2, verify ladder length=3, verdict struct serialises
     to fit_state.toml round-trip.

5. **L9 external** (cross-validation, deferred to a follow-up
   commit so v1 ships cleanly):
   - He2010 measles SEIR at the published θ̂ at dt = 2/365 — verify
     verdict = "pass" at default τ (calibrates the chain_binomial
     default).
   - Boarding-school SIR at dt=1.0 — verify verdict = "fail"; same
     fit at dt=0.1 → verdict = "pass".

## v1 ship status (post-implementation)

Shipped 2026-05-07 across three commits:

- **Foundation + IF2 wiring** (`df30e9f`): module + types + verdict
  logic + Richardson runner + dispatch wiring at IF2 stages +
  `FitState.dt_check` field + 17 unit tests.
- **CLI flag overrides** (`5e4eaae`): `--no-dt-check`,
  `--dt-check-strict`, `--dt-check-halvings`.
- **`fit summary` integration** (`e36b348`): verdict block in IF2
  stage formatter; pass case is one line, fail/marginal includes
  ladder + synth-recovery warning text. 2 new unit tests.

Deferred to follow-up issues (do not block alpha):

- **Standalone `camdl fit dt-check <fit_dir>` subcommand**.
  Reconstructing a `FitRunConfig` from a saved fit dir without the
  original `fit.toml` path requires plumbing the fit.toml
  reference through the run.json provenance, which is its own
  surface change. Auto-run on every IF2 fit covers the primary
  use case for new fits; legacy fits can re-run by re-invoking
  `camdl fit run` against the same fit.toml. File a follow-up
  issue when a real consumer hits the gap.
- **L9 external-validation case** (He2010 measles SEIR at the
  published dt = 2/365 weeks → verdict = "pass" at default τ).
  Calibration of the ode_rk4 default needs a real measles-scale
  fit; this lands when `camdl-book/vignettes/he2010-pomp/` runs
  the check end-to-end.
- **End-to-end integration test** at the binary level (real fit
  at coarse dt → assert `fit_state.toml.dt_check.verdict ==
  "fail"`). Same dependency on the L9 fixture; defer alongside.
- **PGAS / PMMH / NLopt stage support**. Each launcher would need
  the FitRunConfig + backend in scope at the post-fit point;
  in-scope plumbing is mechanical but multi-file. The marginal
  value is real but smaller (PGAS/PMMH posteriors mix past
  burn-in regardless of seed → dt-bias mostly bounds the
  point-estimate accuracy, not the posterior shape). v2 issue.

## Out of scope for v1

Per the issue's "Out of scope" section + my recommendations:

- **Full Richardson extrapolation** (estimating ll(dt → 0) by
  fitting `f(0) + c · dt^p` to the ladder). Diagnostic-only v1
  doesn't need it; v2 if researchers want bias estimates.
- **Adaptive auto-refit** at finer dt until convergence. v1
  diagnoses, v2 could auto-fix. Note the basin-tracking concern:
  the fit at dt/2 might land in a *different* basin than dt_fit
  (gh#52's 14-nat crossover is exactly that), so auto-refit needs
  basin coupling. Non-trivial.
- **Per-parameter dt sensitivity decomposition** (γ is dt-sensitive,
  β isn't, in Vince's reproducer). v1 reports total loglik shifts.
- **Pre-fit early warning** at the user's seed values. Catches
  obvious cases (rates × dt > 1 in bounds) before burning IF2
  hours, but the dt-bias often manifests only at the MLE — false-
  pass rate would erode trust. Post-fit is the right primary
  surface; pre-fit can be a v2 supplement.

## v3+ thought (note for future-self)

dt-bias and basin-finding (gh#42 / gh#51) interact: coarse dt creates
*fake basins* that the user's IF2 cleanly converges to but that
dissolve under finer dt. A "principled" basin-finder would re-run
survey or scout-style search at finer dt to check whether the dt-
fictive basin survives. That's basically a second fit; expensive
but the principled completion of the validator trio. Don't ship
now; future-issue when a real-data PPC shows we need it.
