# Incident: `camdl simulate` and `camdl fit` have incompatible default backends, causing silent PPC miscomparison

**Severity:** Critical
**Discovered:** 2026-04-19, during book-chapter two-panel diagnostic validation
**Found by:** downstream (Vince + Claude on the camdl-book side)
**Status:** Not a regression — likely latent since `chain_binomial` was added as the default fit backend. Silent miscomparison in every PPC plot the book has produced.

---

## Summary

`camdl simulate` defaults to the **Gillespie** backend. `camdl fit run` defaults
to **chain-binomial** with dt=1. These are different dynamical systems at the
same parameters. When a user fits a model with `camdl fit` and then simulates
forward from the MLE with `camdl simulate` to produce a PPC, **the simulation
runs under a different dynamical model than the one the MLE was computed for**.
There is no warning, error, or visual indicator that this has happened. The
resulting PPC plot is, in effect, "evaluate the chain-binomial MLE under
Gillespie dynamics," which is a category error.

## Concrete reproducer

Using `boarding_school_sir.camdl` on the 1978 English boarding-school flu
dataset (14 observations of `in_bed`, N=763), fit with chain-binomial
default:

```bash
camdl fit run fits/real.toml --seed 42
# MLE: β=1.9058652207, γ=0.6559714166, I(0)=5, N0=763
# ll = -61.1, Rhat < 1.05
```

Forward-simulate at this MLE with the `camdl simulate` default backend
(Gillespie):

```bash
camdl simulate boarding_school_sir.camdl --params MLE.toml \
  --replicates 200 --seed 12345 --scenario baseline -o gill.tsv
```

Forward-simulate at the same MLE with chain-binomial (fit default):

```bash
camdl simulate boarding_school_sir.camdl --params MLE.toml \
  --replicates 200 --seed 12345 --scenario baseline \
  --backend chain_binomial --obs cbin_obs.tsv
```

Latent I(t), median across 200 replicates:

| day | Gillespie median | Chain-binomial median | ODE | observed |
|---|---|---|---|---|
| 3 | 110 | 64 | 128 | 76 |
| 4 | 198 | 135 | 207 | 225 |
| 5 | 210 | 230 | 215 | 298 |
| 6 | 176 | 293 | 168 | 258 |
| 7 | 121 | 274 | 114 | 233 |

- Gillespie peaks day 4-5 at I≈210, closely matching the ODE (peak day 5 at 215).
- Chain-binomial peaks day 6 at I≈293 — a full day later and 80 boys higher.
- These trajectories come from *identical* parameter values under *different*
  simulators. The chain-binomial at dt=1 is a coarse discrete-time
  approximation of the SIR CTMC, and in this regime (β=1.91, γ=0.66, finite
  population) it diverges materially from Gillespie.

## Impact on the book chapter

Every PPC figure produced by `analyze.py::simulate_at` and
`analyze.py::simulate_obs_at` without an explicit `--backend` flag used
the `camdl simulate` default (Gillespie) to evaluate a chain-binomial MLE.
Affected figures include (non-exhaustive):

- `figures/ppc_real.png` (the primary plain-SIR PPC)
- `figures/ppc_scout_vs_refine.png`
- `figures/ppc_sir_vs_od.png`
- every other `figures/ppc_*.png` produced via those helpers

These figures systematically show the fit looking **worse than it actually is**
under its own backend, because Gillespie trajectories at the chain-binomial
MLE don't reproduce the rising-limb dynamics that the PF MLE was optimizing
for. The book's narrative — "simple SIR + Poisson obs fails to match the
peak, motivating Γ-noise / Erlang / tvbeta as structural fixes" — is partly
built on this artifact. We spent two days and about a thousand lines of
analysis investigating the mismatch; the real answer is that the forward
sim was on the wrong backend.

When we finally generated the two-panel diagnostic using `camdl simulate
--backend chain_binomial` (by explicit flag, because upstream recommended
the two-panel format and I manually picked the matching backend), the fit
looked dramatically better. Vince (correctly) noticed the inconsistency
and pushed until we diagnosed it.

Separately: `analyze.py::simulate_obs_at` uses `--obs` and gets the
observation-level `in_bed` TSV, so when the simulator's default is
Gillespie, the Poisson-obs output is drawn from Gillespie trajectories.
Same backend-mismatch problem, just at the observation level.

## Root cause

Two independent defaults were picked to be "the right default for this
subcommand":

- `crates/cli/src/fit/config_v2.rs` → `FitBackendConfig::default()` →
  `backend = "chain_binomial"`, `dt = 1.0`. Makes sense for fitting because
  chain-binomial is the cheapest PF-compatible backend.
- `camdl simulate` CLI → default `gillespie`. Makes sense for simulation
  because Gillespie is exact and is what a textbook SIR user wants.

Both defaults are individually defensible. The defect is that **no layer of
the system detects or warns when these are mixed.** An MLE produced under
one dynamical model is silently forward-simulated under a different one.

## Why this is severe

1. **Silent**: no error, no warning, no metadata disagreement that any
   existing tooling checks.
2. **Category-level**: it's not parameter drift or numerical error; it's
   simulating the MLE under a different stochastic process than it was
   estimated for. The resulting plot is not "the fit is bad" nor "the fit
   is good" — it's answering a different question than the user thought
   they were asking.
3. **Affects every PPC**: anyone writing an analysis script that follows
   the natural pattern `(fit → read MLE → camdl simulate)` falls into this
   trap by default. There is no documentation, example, or guardrail that
   tells them they need `--backend chain_binomial` on the simulate step.
4. **Hard to catch by eye**: the PPC "looks wrong" in a way that's easy
   to attribute to model mis-specification (the whole reason this chapter
   was investigating structural fixes). The bug presents as a modeling
   artifact, not a bug. It took a two-panel diagnostic with upstream's
   newly-shipped `pfilter --save-paths` to make the inconsistency visually
   jarring.
5. **Contaminates cached artifacts**: any PPC image, TSV, or cached
   observation file written by `camdl simulate` at a fit's MLE is a
   backend-mismatched artifact. The book has ~30 such files. They're
   not just wrong-looking; they encode the wrong dynamics.

## Recommended fixes (in order of thoroughness)

### 1. Emit a warning on any backend mismatch, immediately

When `camdl simulate --params P.toml` is invoked and `P.toml` is a
`mle_params.toml` (i.e., came from a fit), read the backend the fit used
(it's already in `fit_state.toml` + the fit-level `run.json`). If the
simulate backend differs, emit:

```
warning: backend mismatch.
  This params file was produced by a fit that used backend=chain_binomial
  (dt=1.0). `camdl simulate` is currently running with backend=gillespie.
  These are different dynamical systems at the same parameter values and
  will produce different trajectories. For a consistent PPC, re-run with
  --backend chain_binomial (and --dt 1.0).
```

Simple, localized, and catches the exact failure mode.

### 2. Auto-match the backend when simulating from a fit

If `camdl simulate --params FIT/mle_params.toml` is invoked and
`--backend` is not explicitly passed, **default to the fit's backend**,
not the simulate-CLI default. The MLE's provenance already carries the
backend; use it. Only use the Gillespie default when `--params` is a
standalone TOML with no fit provenance.

This is a behavior change but the right one. No sensible user ever
actually wants "simulate the MLE under a different backend than the fit"
— if they do, they can pass `--backend` explicitly. Making the
provenance-aware behavior the default is consistent with the CAS
unification's goal of "the fit is a first-class artifact, not just
parameter values."

### 3. Flag in `mle_params.toml` and require simulator to read it

Add a field to the MLE TOML header:

```toml
# Backend: chain_binomial (dt=1.0)
```

(It's already in the fit config and `run.json`.) `camdl simulate`
refuses to run without `--backend` matching, or warns loudly if they
differ. Verbose but unambiguous.

### 4. Per-backend provenance-hash comparison

Since the CAS unification hashes fits with backend included
(`sim_hash` takes backend as input, `hashing.rs:44`), any downstream
`camdl simulate` that produces artifacts *could* check whether a
previously-hashed simulate-artifact exists at the fit's backend and
warn if not. Heavier; probably unnecessary if (1) or (2) is in place.

## Suggested incident actions

1. **Immediate**: land fix (1) — a warning — in a patch release. This
   is a few lines in `camdl simulate`'s arg-parse path.
2. **Within a week**: land fix (2) — auto-match-on-fit-params — as the
   default. Announce as a behavior change in the release notes.
3. **Audit**: grep downstream repos (`camdl-book`, `camdl-vignettes`) for
   `camdl simulate` invocations at a fit's MLE that don't pass `--backend`.
   Every one is suspect. We'll do this on the book side; offer a mechanical
   checker if there's interest.
4. **Regression test**: integration test that fits a model with chain-binomial,
   runs `camdl simulate --params MLE.toml --replicates 50` *without* `--backend`,
   and asserts the simulation is under the same backend as the fit (either via
   warning emission or via behavior change).
5. **Documentation**: `docs/inference.md` should have a "PPC at MLE" section
   that walks through the consistency requirement explicitly. Don't let
   anyone read the book chapter as an example to emulate until this is
   clarified — right now it's a worked example of the bug.

## What we (downstream) are doing

- Abandoning the existing `.fit-scratch/` analysis tree. Starting a fresh
  clean boarding-school SIR subdirectory with backend-matched forward sims
  throughout. The old report has too many backend-mismatched figures to
  cleanly correct in-place.
- Before writing any new PPC, verify the backend chain: fit.toml backend
  → mle_params.toml record → simulate --backend. All three must agree.
- Flagging this in the book chapter's methods section as a hazard readers
  should know about.

## Attribution

Caught by Vince pushing repeatedly on the question "why does this figure
look so different from the earlier one when the MLE is the same?" — three
separate times across the conversation, against my initial attempts to
explain it away with seed variation. The crosstalk cost about an hour of
analysis time; the actual miscalibration has been latent across the
entire book chapter for two days of work. Thanks for pushing.

---

**File location:** `/tmp/camdl-incidents/2026-04-19-backend-default-mismatch.md`

Upstream: copy this file into `docs/dev/incidents/` under the camdl repo
(same naming convention as the `2026-04-18-if2-ignored-per-chain-initial.md`
incident), open a blocker issue, and prioritize fix (1) in the next patch
release.
