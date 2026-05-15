# AGENTS.md

Briefing for AI coding agents working in or with camdl. Read this once at the
start of a session; it's denser than a tutorial and covers the things you can't
infer from the source.

For project-internal work in this repo (editing the OCaml compiler, the Rust
runtime, etc.), also read `CLAUDE.md`. This file is for agents *using* camdl —
building models, fitting them, debugging the workflow.

---

## What camdl is

A **DSL plus runtime for stochastic compartmental epidemic models**. You write
the math (compartments, transitions, rate laws, observations); a compiler
expands it into a flat IR; a runtime simulates and fits.

Think of it as the lineage of:

| Tool      | Compiler-DSL? | Stochastic? | Inference?           | Closest analogue                              |
| --------- | ------------- | ----------- | -------------------- | --------------------------------------------- |
| **Stan**  | Yes           | Latent      | NUTS HMC             | Probabilistic-programming DSL with autodiff   |
| **odin**  | Yes           | ODE-only    | External fitting     | Compartmental ODE DSL in R                    |
| **pomp**  | No (R+C)      | Yes         | IF2, PMMH, particle  | Hand-coded SSA + obs models in R              |
| **camdl** | Yes (OCaml)   | Yes         | IF2, PGAS+NUTS, PMMH | DSL + stochastic runtime + autodiff inference |

Models in pretraining data: **lots of Stan and pomp, very little camdl.** When
in doubt, analogize from pomp (closest in problem domain) or Stan (closest in
DSL philosophy), then verify against the camdl spec.

---

## Mental model in one paragraph

A camdl model is a flat declaration: `compartments { ... }`, `transitions { ... }`,
`observations { ... }`, optionally `dimensions { ... } + stratify(...)` for
expansion, `interventions { ... } / events { ... }` for scheduled state changes.
The OCaml compiler (`camdlc`) reads the `.camdl` file, dim-checks every
expression, expands stratification at compile time, emits source-to-source
gradients for every rate, and serialises the result as a versioned IR JSON
envelope. The Rust runtime (`camdl`) consumes that IR and runs simulation
backends (Gillespie, tau-leap, chain-binomial, ODE) plus inference algorithms
(particle filter, IF2, PGAS+NUTS, PMMH). Parameter values are supplied at
runtime — the model file is parameter-free.

---

## Canonical workflow (follow this order)

For most "build me a model and fit it" requests, this is the bring-up
sequence. Skipping steps will burn time.

```
        ┌─────────────────────────────────────────┐
        │  1. WRITE / EDIT MODEL.camdl            │
        └────────────────┬────────────────────────┘
                         │
                         ▼
        ┌─────────────────────────────────────────┐
        │  2. camdl check model.camdl             │   ← compile + dim-check
        │     If errors: see ERROR TABLE below    │
        └────────────────┬────────────────────────┘
                         │ green
                         ▼
        ┌─────────────────────────────────────────┐
        │  3. camdl simulate model.camdl          │   ← sanity check trajectory
        │     --param ... --output traj.tsv       │
        │     Look at traj.tsv — does it look     │
        │     epidemiologically reasonable?       │
        └────────────────┬────────────────────────┘
                         │ ok
                         ▼
        ┌─────────────────────────────────────────┐
        │  4. camdl survey model.camdl            │   ← likelihood landscape
        │     --data cases.tsv --n-points 200     │
        │     --render                            │
        │     Identifies basins, ridges, bound    │
        │     pinning BEFORE you commit to a fit  │
        └────────────────┬────────────────────────┘
                         │ basin visible
                         ▼
        ┌─────────────────────────────────────────┐
        │  5. WRITE fit.toml                      │   ← see FIT.TOML SHAPE
        │     Every estimated param NEEDS a       │
        │     prior block (no implicit Flat!)     │
        └────────────────┬────────────────────────┘
                         │
                         ▼
        ┌─────────────────────────────────────────┐
        │  6. camdl fit run fit.toml              │   ← all stages declared
        │     --stage scout (one stage at a time  │
        │     while iterating)                    │
        └────────────────┬────────────────────────┘
                         │
                         ▼
        ┌─────────────────────────────────────────┐
        │  7. camdl fit summary <fit-dir>         │   ← R̂, ESS, MLE table
        │     If diagnostics fire: see            │
        │     DIAGNOSTICS TABLE below             │
        └─────────────────────────────────────────┘
```

When iterating: `camdl list` to see prior fits, `camdl fit diff old new` to
compare.

---

## When to stop and ask the human

camdl outputs feed real public-health decisions. The asymmetry matters: a fit
that takes an extra day because the agent paused for confirmation costs
roughly nothing; a posterior that's silently miscalibrated because the agent
bypassed an error costs much more. Default to pausing.

**Always pause and ask before:**

- **Reaching for an escape-hatch flag** (`--allow-degenerate-rates`,
  `CAMDL_SKIP_VERSION_CHECK=1`, `--no-nuts`, `--force` on a fit re-run).
  Each of these bypasses a check that exists for a reason. If a flag is
  the obvious fix to make an error go away, that's the signal to stop.
- **Bumping `ir/VERSION` or editing `ocaml/lib/ir/serde.ml` / `rust/crates/ir/src/`.**
  IR schema changes break every golden file and require atomic OCaml + Rust
  + golden-regen commits. Per CLAUDE.md, these are explicit human-loop
  changes.
- **Editing inference math files** (`pgas.rs`, `pgas_grad.rs`, `nuts.rs`,
  `if2.rs`, `obs_loglik.rs`, `obs_model.rs`, `particle_filter.rs`). Per
  CLAUDE.md these are flagged as high-risk regardless of how mechanical the
  edit looks; ask before touching.
- **Loosening a convergence gate** because scout failed it. The gate exists
  to fail loudly rather than pass a bad fit through. The right move when
  scout's gate fires is to diagnose *why* (widen bounds? more chains? more
  iterations?), not lower the threshold.
- **Choosing prior shape for a parameter you don't have domain context for.**
  Picking `Normal(0, 1)` "to make PGAS run" is the worst-case communication
  failure the audit's C4 fix was designed to prevent. Priors show up in the
  posterior. If the model author's prior intent isn't documented, ask.
- **Anything that publishes / shares a fit hash** as a result. Before a fit's
  output goes into a paper, brief, or policy artefact, a human should sign
  off on the diagnostics, the priors, and the model assumptions.

**Flag and proceed (don't block, but surface):**

- **Diagnostics fired in `camdl fit summary`.** Report which fired (R̂,
  ParamNearBound, DivergentTransitions, etc.), with the interpretation from
  the diagnostics table below. Don't decide unilaterally that "R̂ = 1.12 is
  fine."
- **`degenerate_step_count > 0`** in the eval-stats summary. Even with
  per-particle recovery handling it, the user should see the count and
  decide whether the model needs a `Cond` guard.
- **Profile likelihood non-monotonicity** or wide CIs from
  `camdl profile`. Identifiability problems are model-design issues, not
  fitting bugs.
- **`camdl survey` results.** The HTML pair-plot is a visual artefact
  (parameter-pair scatter coloured by loglik). Agent vision is unreliable on
  scatter geometry — what looks like a "clear basin" or a "ridge" or "bound
  pinning" to an agent is often partially wrong about location, extent, or
  whether multiple basins are present. Surface the rendered HTML path and a
  one-line summary ("survey rendered to `survey.html`; my read is X but
  please confirm before I seed scout"), don't act on the survey
  unilaterally. The numerical TSV next to the HTML is reliable for
  argmax-loglik points; the geometry interpretation is not.
- **External-oracle disagreements**: if a model is meant to reproduce a
  published result (He et al. 2010 measles, K-McK final-size, etc.) and
  the numbers don't match within the expected tolerance, surface
  immediately rather than tweaking until they match.

**Safe to do autonomously:**

- Run `camdl check`, `camdl simulate`, `camdl survey`, `camdl pfilter`,
  `camdl fit run` (single stage, not committing the fit dir), `camdl fit
  summary`, `camdl list`, `camdl show`, `camdl fit diff`.
- Edit a `.camdl` model file in response to a compile error from the
  error table (typo fix, missing declaration, dim-correction).
- Edit a `fit.toml` to widen bounds in response to `ParamNearBound`, or
  add a missing `[estimate.X.prior]` block (asking what shape the user
  wants if not obvious).
- Add `Cond` guards to rate expressions in response to
  `NumericalCollapse{DivByZero}` (this is the *correct* fix, not a bypass).
- Run `make build && make install` after pulling.

**The general principle:** agents are good at running the workflow; humans
are needed for *modeling decisions* and *interpreting calibration*.
Calibration is the half of compartmental modelling that's actually
identifiability and prior-belief judgement, not engineering. Don't pretend
otherwise to "make progress."

---

## Error → cause → fix table

Compile-time errors (from `camdlc`):

| Code   | What it says                              | What it usually means                                                 | What to do                                                                                                                       |
| ------ | ----------------------------------------- | --------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `E100` | undeclared name 'X'                       | Typo, or use of a name not declared in compartments/parameters/let    | Add the declaration. Don't introduce a new symbol just to make the error go away.                                                |
| `E107` | ambiguous unit literal after '/'          | `20 / 100_000 'per_year` — unit binds to the adjacent number          | Parenthesise: `(20 / 100_000) 'per_year`, or pre-compute a single literal.                                                       |
| `E300` | transition rate has wrong dimension       | Per-capita rate where population-level was needed (or vice versa)     | The rate must have dim `P·T⁻¹` (population per time). If you have a per-capita rate `T⁻¹`, multiply by `S` (the source pop).     |
| `E302` | dimension mismatch in addition            | Adding incompatible quantities                                        | Check units of both sides; usually a missing `* N` or `/ N`.                                                                     |
| `E303` | conflicting dimensions for parameter X    | Same parameter inferred to be different dims in different transitions | Pick the right dim for the parameter and fix the transition that's wrong.                                                        |
| `L401` | rate expression `(1 - exp(-rate * 1 'days))` not dt-invariant | Discretization-correction shape that's only correct at dt=1 day | Use the `dt` primitive: `(1 - exp(-rate * dt))/dt` — invariant across integrator steps. |

Run-time errors from `camdl simulate` / `pfilter` / `if2` / `fit`:

| Error                                                         | What it usually means                                                                                                              | What to do                                                                                                                                                                                                                          |
| ------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `IR version mismatch`                                         | Stale `camdlc` binary vs `camdl` binary. The IR envelope's `ir_version` doesn't match what the runtime expects.                    | `make build && make install`. The runtime checks the on-PATH `camdlc` hash against its own.                                                                                                                                         |
| `SimError::NumericalCollapse { kind: DivByZero }`             | A rate expression hit `0/0` or similar (e.g. `beta * I[a] / N_local[a]` when stratum `a` is empty)                                 | Add a `Cond` guard: `cond(N_local[a] > 0, beta * I[a] / N_local[a], 0)`. **Do not** reach for `--allow-degenerate-rates` unless you've decided the silent-zero is the modeling intent.                                              |
| `SimError::NumericalCollapse { kind: PowNanInf / SqrtNegative }` | Negative base raised to fractional power, or sqrt of negative                                                                       | Domain bug in the rate expression. Add a guard or fix the formula.                                                                                                                                                                  |
| `SimError::NegativeCount { cause: BinomialOvershoot }`        | Binomial split overshot (rate × dt → 1 for some particle). Common in inference exploration                                         | If during `simulate`: reduce `--dt`. If during `fit`: per-particle recovery handles it (the offending particle gets `−Inf` log-likelihood and is killed in resampling). Watch the `eval-stats` summary for how often this fires.    |
| `SimError::NegativeCount { cause: InterventionAddNegative }` | An `Action::Add` expression resolved to a negative value                                                                            | Config bug. There's no inference scenario where `Add` should remove individuals. Fix the expression or use `transfer` instead of `add`.                                                                                             |
| `requires capabilities: BALANCE` (on tau_leap/gillespie/ode)  | Model uses `balance { ... }`; only chain-binomial supports it                                                                      | Use `--backend chain_binomial`. Don't try to translate `balance` to a manual transition — its semantics are chain-binomial-specific (the residual-compartment fix).                                                                 |
| `--record-prequential requires --stage <pfilter-stage>`       | Flag used with a non-PFilter stage                                                                                                 | Pass `--stage` with a PFilter stage from your fit.toml. The error message lists available PFilter stages.                                                                                                                          |
| `pgas refuses to run with implicit improper-uniform priors`   | `[estimate.X]` block exists with no `[estimate.X.prior]`                                                                           | Add an explicit prior. For uniform-on-bounds: `prior = { uniform = { lower = ..., upper = ... } }`. **Do not** add a wide normal "to make it shut up" — the prior shows up in the posterior.                                       |
| `PGAS gradient does not yet include obs-likelihood ... derivatives` | Estimating `rho`, `psi`, `k`, or any param appearing in the obs-likelihood / overdispersion expression                       | Move that param to fixed (`[fixed.rho] value = ...`) and either grid-search it, or fit it with IF2 first (gradient-free). Full obs-likelihood gradient threading is on the roadmap (audit C1 follow-up).                            |
| `IR JSON parse error: missing field 'ir_version'`              | Loading a bare-Model JSON; runtime requires the envelope wrapper                                                                   | Re-emit with `camdlc` (it always wraps). For hand-curated JSON: wrap with `jq '{ir_version: "0.4", validated_by: "manual", model: .}' in.json > out.json`.                                                                          |

---

## Diagnostics → interpretation table

After `camdl fit run`, `camdl fit summary <fit-dir>` shows diagnostics. They're
not all "the fit failed"; they're typed signals.

| Diagnostic                       | Threshold                            | What it means                                                                                                                                                                  | What to do                                                                                                                                          |
| -------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `RhatHigh`                       | R̂ > 1.1 (warn), > 1.5 (error)       | Chains haven't agreed on this parameter's posterior                                                                                                                            | More sweeps; check for multimodality with `survey`. R̂ > 1.5 → almost certainly a real basin problem.                                              |
| `LowESSAtMLE`                    | ESS < 5% × n_particles               | Particle filter is struggling at the point estimate. Loglik estimate has wide variance.                                                                                        | Increase `n_particles` in the validate stage, or check for model misspecification at the MLE.                                                       |
| `ParamNearBound`                 | within 1% of natural-scale bound     | Posterior pile-up at a bound                                                                                                                                                   | Almost always: widen the bound. The data is telling you the parameter wants to live outside your prior support.                                     |
| `DivergentTransitions`           | any post-burn-in divergence          | NUTS hit a divergent trajectory (high curvature in posterior geometry)                                                                                                         | Reparameterise (log/logit transforms), shrink the step size, or check for funnel geometry. Stan-canonical: any post-burn divergence is suspicious. |
| `MaxTreeDepthHits`               | > 5% of post-burn-in sweeps          | NUTS trees not finishing — step size too small or posterior too elongated                                                                                                      | Increase `max_tree_depth`, or reparameterise.                                                                                                       |
| `LowSwapRate`                    | adjacent-rung pair < 10%             | Tempering ladder too sparse — chains don't mix across rungs                                                                                                                    | Add intermediate β values to the ladder.                                                                                                            |
| `DegenerateAncestorSampling`     | > 10% of post-burn-in CSMC substeps  | Reference trajectory too far from particle cloud                                                                                                                               | More particles, or smaller PGAS proposal SDs (let scout run longer first).                                                                          |
| `LowTrajectoryRenewal`           | mean post-burn renewal < 10%         | PGAS reference trajectory not getting refreshed — possibly stuck                                                                                                               | More particles. Check that CSMC is actually proposing diverse trajectories (run with `RUST_LOG=camdl_sim::inference::pgas=debug`).                  |
| `MultimodalLikelihood`           | ll spread > 50 nats with R̂ > 1.5    | Different chains are in different basins                                                                                                                                       | Run `camdl survey` to map the landscape. Likely need more chains or different initialisation.                                                       |
| `ConvergenceIncomplete`          | max R̂ > 1.1 with finite agreements  | Some parameters haven't converged                                                                                                                                              | More sweeps; check the per-parameter R̂ table to see which.                                                                                         |
| `AcceptanceRateUnhealthy`        | < 10% or > 50%                       | MH proposal SD is too big (low accept) or too small (high accept)                                                                                                              | Let burn-in run longer; the Robbins-Monro adapter usually fixes this. If not, fix the proposal SD manually in fit.toml.                             |

`eval-stats` summary at end of run (separate from diagnostics) shows
counter increments: `div_by_zero`, `pow_nan_inf`, `binomial_fallback`, etc.
Non-zero counts mean the model hit a degenerate path during this run.
Cross-reference with `camdl fit summary` for context.

---

## Idioms / anti-idioms

**Backend choice for fits.** Use `chain_binomial`. Gillespie is for
forward-simulation sanity checks, not fits (too slow). Tau-leap is fine but
chain-binomial is the production fit backend.

**Always `camdl survey` before `camdl fit run`.** Surveying is the cheapest
hour of compute in the pipeline; fitting a model you haven't surveyed is the
single most common way to spend a week producing a wrong answer.

**One stage at a time when iterating.** `camdl fit run fit.toml --stage scout`
gives you one stage's output to inspect before committing to refine + validate.
Run all stages only when the fit.toml is stable.

**Explicit priors, always.** PGAS now refuses implicit-Flat priors. A wide
uniform is fine if that's actually your belief; a wide normal is fine for log
parameters; but the choice has to be in the file. "No prior" is no longer an
option.

**Cond guards on rate expressions with potentially-zero divisors.** Spatial
and stratified models are the common case (an empty patch's force-of-infection
is `0 / 0`). Write `cond(N > 0, beta * I / N, 0)` rather than relying on
silent-zero (the runtime no longer silently zeros — it errors).

**Reparameterise to natural support.** Use `transform = "log"` for rates and
positives, `transform = "logit"` for probabilities. The MCMC moves on the
transformed scale; bounds are enforced by construction.

**Reach for `camdl fit summary`, not eyeballed traces.** The summary already
extracts R̂, ESS, the MLE table, and any fired diagnostics. Eyeballing
trace TSVs is for debugging the summary, not for routine inspection.

**Don't reach for these escape hatches without understanding them:**

- `--allow-degenerate-rates` — restores legacy silent-zero on `Div by zero`
  etc. Use only when the model legitimately means "rate is 0 when divisor is
  0" (e.g. force of infection in a patch with no people). Default is hard
  error, which is correct for almost every model.
- `CAMDL_SKIP_VERSION_CHECK=1` — bypasses the camdl/camdlc version
  handshake. Almost always means you should `make install` instead.
- `--no-nuts` (PGAS) — falls back to MH-within-Gibbs. For posterior
  geometries where NUTS struggles, but verify with a small run before
  committing.

---

## fit.toml shape (canonical)

```toml
[model]
camdl = "model.camdl"

[data.observations]
weekly_cases = "data/cases.tsv"

# Optional: holdout for out-of-sample validation
[holdout]
weekly_cases = "data/cases_holdout.tsv"

# Every estimated param needs an explicit prior (PGAS refuses Flat)
[estimate.beta]
bounds = [0.001, 2.0]
prior  = { log_normal = { mu = -1.0, sigma = 0.5 } }

[estimate.gamma]
bounds = [0.01, 1.0]
prior  = { log_normal = { mu = -2.3, sigma = 0.3 } }

[estimate.rho]
bounds = [0.3, 0.6]
prior  = { beta = { alpha = 5, beta = 5 } }

# Fixed (not estimated) parameters
[fixed]
N0 = 1_000_000
mu = 0.000027

# Stages — declared by name, run in declaration order by default
[stages.scout]
algorithm   = "if2"
backend     = "chain_binomial"
chains      = 16
particles   = 2000
iterations  = 200
init_method = "lhs"           # latin-hypercube; "survey_top_k" if you ran survey

[stages.refine]
algorithm   = "pgas"
backend     = "chain_binomial"
chains      = 4
particles   = 2000
sweeps      = 5000
burn_in     = 500
starts_from = "scout"

[stages.validate]
algorithm  = "pfilter"
backend    = "chain_binomial"
particles  = 4000
replicates = 8
starts_from = "refine"
```

`camdl fit methods` lists the supported `(algorithm, backend)` pairs.
`camdl fit new --from base.toml variant.toml` derives a new fit.toml from an
existing one (renames model file, keeps stage structure).

---

## Reproducibility primitives — use them

Every fit run is content-addressed: hash of `(model IR, params, seed, data,
algorithm config, tool version)`. Same inputs → same hash → cache hit (no
re-run).

```bash
camdl fit where fit.toml          # output dir for this fit
camdl fit status fit.toml         # which stages have completed
camdl list                        # all cached fits in the project
camdl show <hash>                 # full metadata for one fit
camdl cat <hash>                  # emit trajectory or observations
camdl fit diff <hash1> <hash2>    # compare two fits
```

The iterative model-building loop:

```bash
prev_hash=$(camdl fit where fit.toml)
# edit fit.toml — say, widen a prior
camdl fit run fit.toml
new_hash=$(camdl fit where fit.toml)
camdl fit diff $prev_hash $new_hash
```

Cite a fit hash in writeups — paste it into a methods section and any reader
with the source can reproduce the result bit-for-bit.

---

## Where the docs live

### If you're working inside the camdl repo

| For                                          | Read                                                                          |
| -------------------------------------------- | ----------------------------------------------------------------------------- |
| DSL syntax reference                         | `docs/camdl-language-spec.md`                                                 |
| IR schema (the OCaml↔Rust contract)          | `docs/camdl-data-spec.md`, `docs/compartmental-ir-spec.md`, `ir/schema.json`  |
| Inference workflow (fit.toml fields, stages) | `docs/camdl-inference-spec.md`, `docs/inference.md`                           |
| Batch / sweep system                         | `docs/camdl-run-spec.md`                                                      |
| Simulation backends                          | `docs/runtimes.md`                                                            |
| Feature catalogue with pomp comparison       | `docs/user-features.md`                                                       |
| Tutorial                                     | `docs/intro.md`                                                               |
| Debugging via `camdl eval`                   | `docs/debugging.md`                                                           |
| Recent breaking changes (audit remediation)  | `docs/dev/reviews/2026-05-12-full-audit.md`, `docs/dev/proposals/2026-05-13-pre-alpha-audit-remediation.md` |
| In-flight design proposals                   | `docs/dev/proposals/`                                                         |

For deeper reading: `docs/methods/particle-methods.md`, `docs/methods/cooling.md`.

### If you're working in a downstream project that uses camdl

The docs aren't on the local filesystem by default. Three options, in order
of preference:

1. **Shallow-clone the docs into the project** (recommended for any
   non-trivial camdl work — version-pinned, offline, fast):
   ```bash
   git clone --depth 1 --filter=blob:none --sparse \
       https://github.com/vsbuffalo/camdl .camdl-source
   cd .camdl-source && git sparse-checkout set docs ocaml/golden && cd ..
   ```
   This pins ~5 MB of `docs/` + `ocaml/golden/` (working examples for every
   language feature) into `.camdl-source/`. Add `.camdl-source/` to
   `.gitignore`. Re-run `git -C .camdl-source pull` to sync to upstream.
   Reference `.camdl-source/docs/camdl-language-spec.md` etc. just as you
   would inside the repo.

2. **Fetch from the hosted docs** at
   [vincebuffalo.com/camdl](https://vincebuffalo.com/camdl/docs/) via
   `WebFetch` (or the agent's equivalent). Slower per query, online-only,
   but no setup. Use for one-off lookups; switch to (1) for sustained work.

3. **Read the binary's `--help` output.** `camdl --help`, `camdl fit run
   --help`, `camdl fit methods`, etc. cover the CLI surface authoritatively
   — the help text is part of CI. This catches the *what flags exist* and
   *what stages are supported* questions, not the *what does the language
   look like* questions.

Working examples are often faster than reading the spec: `.camdl-source/
ocaml/golden/sir_basic.camdl` is the canonical SIR; `seir_age.camdl` covers
stratification; `polio_spatial_5.camdl` covers spatial coupling; the rest of
the directory exercises every language feature at least once.

A future `camdl docs <topic>` subcommand would emit relevant sections to
stdout (proposed; not yet implemented). Until then, options 1–3 above.

---

## What the runtime can validate that you can't see in source

Things to remember the compiler is doing for you, so you can lean on them:

- **Unit conversion** between `'days`, `'weeks`, `'years`, `'per_year`, etc.
  to the model's `time_unit`. Don't pre-convert; the compiler handles it.
- **Dimensional analysis** on every expression. Per-capita vs
  population-level rate confusion → `E300` at compile time.
- **Source-to-source autodiff** on every rate expression. PGAS+NUTS gets
  exact gradients; you don't need to hand-write Jacobians.
- **Identifiability checks** in `camdl survey` (informally) and in
  `camdl profile` (formally — 1D and 2D profile likelihoods).
- **Backend-capability gate**: requesting a backend that can't run a model
  (e.g. `--backend gillespie` on a model with `overdispersed()`) errors at
  dispatch time with a hint.
- **Per-particle recovery** in PGAS / particle filter: a particle that hits
  `NumericalCollapse` is killed in resampling (its log-weight goes to `−Inf`).
  The chain continues. The `degenerate_step_count` in the eval-stats summary
  tells you how often this fired.

---

## When to write back

If the agent gets stuck in a way the tables above don't cover:

1. Run `camdl check` on the current model file — the compile error is almost
   always the right entry point.
2. Run `camdl --help` and `camdl <subcommand> --help` — the help text is
   maintained as part of CI.
3. Look at `docs/dev/reviews/` for recent design discussions.
4. Look at `ocaml/golden/*.camdl` for working examples of every language
   feature; corresponding `*.params.toml` shows runtime parameter values.
5. As a last resort, the source is in `ocaml/lib/compiler/` (parser,
   expander) and `rust/crates/sim/src/inference/` (inference math).
