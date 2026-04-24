# Incident: IF2 cooling semantics drifted across four doc surfaces

**Severity:** Moderate (silent wrong-mental-model class — no runtime
error, but inverted user understanding, detectable only by cross-
checking docs against code)
**Discovered:** 2026-04-24 during a conversation about scout vs refine
cooling convention; agent and human both held inconsistent mental
models traceable to the docs, not to either individual's carelessness
**Found in:** `rust/crates/cli/src/if2.rs` module header,
`docs/camdl-run-spec.md:1347`, `docs/camdl-language-spec.md:2832`,
`docs/inference.md:472–481` — and consequently in at least two
vignette `fit.toml` files (`camdl-book/vignettes/he2010/fit_synthetic.toml`,
`camdl-vignettes/he2010-inference/fit_he2010.toml`) that were written
by someone following the (incorrect) spec-doc convention
**Status:** Doc surfaces §6.1–6.4 fixed in commit 62da628
(2026-04-24); `docs/methods/cooling.md` now owns the canonical
description with worked example + empirical iter-by-iter table.
Vignette `fit.toml` configs (§6.5) not yet updated — pending
scientific review (upstream wants to bundle with the next he2010
rerun, rather than silently changing cooling mid-analysis).
Follow-up presentation proposal:
`docs/dev/proposals/2026-04-24-cooling-presentation.md`.
**Related:** cooling-presentation proposal (linked above); book-
agent he2010 analysis depends on the corrected docs landing before
next rerun.

---

# IF2 Cooling Schedule Investigation (audit trail)

**Date:** 2026-04-24
**Trigger:** Uncertainty about whether scout / refine run hot or cold.
Agent flip-flopped twice; upstream asked for definitive evidence from
code, initialisation sites, and an empirical instrumented run.

---

## TL;DR — bottom-line ground truth

**Your intuition was right and my earlier flips were wrong.** Scout
uses mild cooling (chains stay hot to explore); refine uses aggressive
cooling (chains collapse onto scout's basin). Code defaults:

```
SCOUT_COOLING  = 0.70    // rust/crates/cli/src/fit/scout.rs:14
REFINE_COOLING = 0.05    // rust/crates/cli/src/fit/refine.rs:15
```

At the scout default, the perturbation SD at the end of a stage is
**49% of initial**. At the refine default, **0.25% of initial**.
Scout barely cools; refine collapses by 3½ orders of magnitude.

Three documentation surfaces state the opposite or state numbers that
disagree with the code. Every fit.toml in the vignettes directory that
sets cooling explicitly uses a different convention again (even
gentler cooling on both stages, with the relative ordering sometimes
inverted relative to code defaults). The docs and some configs are
wrong; the code is right; the user's intuition was correct. Full
catalogue and fix plan below.

---

## Part 1 — The exact per-step cooling formula, as compiled

### 1.1 The cooling factor computation

File: `rust/crates/sim/src/inference/if2.rs`, lines 234–251. Quoted
verbatim:

```rust
    // Compute per-filtering-step cooling factor.
    // Matches pomp's cooling.fraction.50 semantics: the fraction is reached
    // at the HALFWAY point of the target iterations. After the full run,
    // rw_sd = cooling_fraction² × initial.
    //
    // Per-step factor: c = cooling_fraction ^ (2 / (target_iters × n_obs))
    // The "2" makes the fraction apply at the midpoint, not the endpoint.
    //
    // IM5 in 2026-04-19 inference review: each iteration actually
    // consumes (1 + n_obs) `global_step` ticks — one for the t=0
    // perturbation and one per observation. The formula approximates
    // this by `n_obs`, which is tight for n_obs ≳ 10 and loose for
    // small n_obs. Rule of thumb: for n_obs = 1 the effective cooling
    // is doubled, for n_obs = 10 it is ~10% stronger than advertised.
    // If this matters for a particular fit, bump `cooling_target_iters`
    // accordingly.
    let total_target_steps = config.cooling_target_iters as f64 * n_obs as f64;
    let per_step_cooling = config.cooling_fraction.powf(2.0 / total_target_steps);
```

### 1.2 The per-step factor is raised to `global_step` at each perturbation

The computed `per_step_cooling` is applied via `.powf(global_step as f64)`
at every perturbation site. Three occurrences:

- Line 310: initial t=0 perturbation in each iteration
- Line 374: per-observation perturbation during the filtering sweep
- Line 485: end-of-iteration diagnostic, for recording effective SD

```rust
let cooling_now = per_step_cooling.powf(global_step as f64);
```

### 1.3 The formula as it actually runs

Given `cooling_fraction`, `cooling_target_iters`, `n_obs`, and a global
step counter that increments once per t=0 perturbation + once per
observation within each iteration:

```
per_step_cooling = cooling_fraction ^ (2 / (cooling_target_iters × n_obs))

effective_SD(global_step) = rw_sd × per_step_cooling^global_step
```

Substituting: at iteration *m* of a stage (approximately m × n_obs
global steps, ignoring the +1 for t=0 ticks which is a ~0.1% correction
for realistic n_obs):

```
SD(iter m) / SD(initial) ≈ cooling_fraction ^ (2m / cooling_target_iters)
```

**This is the cf50 convention** (pomp's `cooling.fraction.50`):

- At iter = target_iters/2 (halfway): factor = `cooling_fraction¹` (the
  "50% mark" name).
- At iter = target_iters (end): factor = `cooling_fraction²`.

For `cooling_fraction = 0.9, target_iters = 30`:
- Halfway (iter 15): SD = 0.9 × initial (factor `0.9¹`)
- End (iter 30): SD = 0.81 × initial (factor `0.9²`)

For `cooling_fraction = 0.70, target_iters = 30` (SCOUT default):
- Halfway (iter 15): SD = 0.70 × initial
- End (iter 30): SD = 0.49 × initial

For `cooling_fraction = 0.05, target_iters = 50` (REFINE default):
- Halfway (iter 25): SD = 0.05 × initial
- End (iter 50): SD = 0.0025 × initial

---

## Part 2 — Where `cooling_target_iters` gets set

Grep across the codebase for every assignment:

```
rust/crates/cli/src/profile.rs:394:   cooling_target_iters: n_iterations
rust/crates/cli/src/if2.rs:299:       cooling_target_iters: n_iterations
rust/crates/cli/src/fit/runner.rs:254: cooling_target_iters: n_iterations
rust/crates/cli/src/fit/validate.rs:493: cooling_target_iters: 30
```

**Answer:** in every production path (camdl if2, camdl fit run, camdl
profile), `cooling_target_iters` is set to **the stage's own
`n_iterations`**. It is **not** a fixed 50 (despite the module-level
comment at `if2.rs:87` saying "cooling.fraction.50 when
cooling_target_iters = 50" as an illustrative reference). Only
`fit/validate.rs:493` hard-codes `cooling_target_iters = 30` for the
validation stage; everywhere else follows the pattern.

Consequence: `cooling_fraction` always expresses "where does the SD end
up at the halfway point of *this stage's* iterations," regardless of
stage length. A scout with `cooling_fraction = 0.9` over 30 iterations
and another with `cooling_fraction = 0.9` over 200 iterations both
produce the same final SD (`0.81 × initial`) — they just take longer
to get there.

---

## Part 3 — Empirical per-iteration SD table

### 3.1 Instrumentation (temporary)

Added one `log::info!` after the existing `cooling_at_iter`
computation in the IF2 per-iteration diagnostic block (line 485), emitting
`iter`, `cooling_at_iter`, `per_step_cooling`, `global_step`,
`cooling_target_iters`, `cooling_fraction`, `n_obs`. Built `camdl`,
ran a 30-iteration scout, captured the log, reverted the
instrumentation, rebuilt to clean state. The instrumentation is no
longer in the tree.

### 3.2 Run parameters

To mirror `vignettes/he2010/fit_synthetic.toml`'s `[stages.scout]`
block, which uses `cooling = 0.9`, 200 iterations — but shortened to
30 iterations and 100 particles for runtime-of-investigation (the
cooling curve shape is determined by the formula, not the particle
count):

```
cd /Users/vsb/projects/work/camdl-book/vignettes/he2010
camdl --progress plain --verbosity info if2 \
    models/he2010_london.camdl \
    --params params/he2010_london.toml \
    --data data/he2010_synthetic_obs.tsv \
    --particles 100 --iterations 30 --chains 1 \
    --cooling 0.9 --seed 1 \
    --rw-sd "R0=5,sigma=0.01,gamma=0.01" \
    --flow recovery
```

Effective settings at runtime: `cooling_fraction=0.9`, `target_iters=30`
(set to `n_iterations`), `n_obs=1096` (weekly observations of He et al.
London), `per_step_cooling=0.999993591230`.

### 3.3 Observed SD per iteration

```
  iter   0: SD = 1.0000 × initial
  iter   1: SD = 0.9930 × initial
  iter   2: SD = 0.9861 × initial
  iter   3: SD = 0.9791 × initial
  iter   4: SD = 0.9723 × initial
  iter   5: SD = 0.9655 × initial
  iter   6: SD = 0.9587 × initial
  iter   7: SD = 0.9520 × initial
  iter   8: SD = 0.9454 × initial
  iter   9: SD = 0.9387 × initial
  iter  10: SD = 0.9322 × initial
  iter  11: SD = 0.9256 × initial
  iter  12: SD = 0.9192 × initial
  iter  13: SD = 0.9127 × initial
  iter  14: SD = 0.9063 × initial
  iter  15: SD = 0.9000 × initial    ← halfway — matches cooling_fraction = 0.9
  iter  16: SD = 0.8937 × initial
  iter  17: SD = 0.8874 × initial
  iter  18: SD = 0.8812 × initial
  iter  19: SD = 0.8751 × initial
  iter  20: SD = 0.8689 × initial
  iter  21: SD = 0.8629 × initial
  iter  22: SD = 0.8568 × initial
  iter  23: SD = 0.8508 × initial
  iter  24: SD = 0.8449 × initial
  iter  25: SD = 0.8390 × initial
  iter  26: SD = 0.8331 × initial
  iter  27: SD = 0.8272 × initial
  iter  28: SD = 0.8215 × initial
  iter  29: SD = 0.8157 × initial    ← end — approaches 0.81 = cooling_fraction²
```

### 3.4 What this shows

- **At iter 15 (halfway of 30 iterations), SD = 0.9000 × initial**,
  exactly equal to `cooling_fraction = 0.9`. This is the "cf50" tell:
  the halfway SD *is* the cooling_fraction parameter by construction.
- **At iter 29 (end), SD = 0.816 × initial**, approaching
  `0.9² = 0.81`. The final-iter value is slightly above `0.81`
  because the global_step counter hasn't yet reached its end-of-stage
  tick — the iter 30 value would land on `0.81` exactly.
- **This is mild cooling**. Over 30 iterations the perturbation SD
  shrinks by only 18%. Chains never truly concentrate on a single
  point; they keep moving around.

This definitively confirms the `cf50` semantics in the module comment
at `if2.rs:235–240` is what actually runs. The "per-iter factor"
alternative reading (where `cooling_fraction` would be applied once
per iteration, giving final factor `0.9^30 ≈ 0.04` — 96% reduction)
is **not** what the code does.

---

## Part 4 — Ground truth for scout vs refine

### 4.1 Code constants (authoritative)

```
File: rust/crates/cli/src/fit/scout.rs
const SCOUT_COOLING: f64 = 0.70; // cf50: 70% at halfway, 49% at end — find basins

File: rust/crates/cli/src/fit/refine.rs
const REFINE_COOLING: f64 = 0.05; // cf50: 5% at halfway, 0.25% at end — converge to MLE
```

### 4.2 What each stage looks like in action

Plugging the code defaults into the formula (verified against the
empirical iter-by-iter run above):

**Scout** (cooling_fraction = 0.70, 30 iterations):
| iter | SD / initial |
|---|---|
| 0 | 1.000 |
| 15 | 0.700 |
| 30 | 0.490 |

Scout halves the perturbation SD over its stage. Chains stay mostly
hot, with meaningful per-iteration jitter even at the end. A scout
chain's final particle cloud is **not** concentrated on a point — it's
a moderately-spread distribution around whichever basin the chain's
initial position led it toward. This is why scout uses multiple
chains with dispersed starts: each one explores semi-independently,
and the cross-chain Rhat at the end tells you whether they agreed.

**Refine** (cooling_fraction = 0.05, 50 iterations):
| iter | SD / initial |
|---|---|
| 0 | 1.000 |
| 25 | 0.050 |
| 50 | 0.0025 |

Refine collapses SD by a factor of ~400 over its stage. Chains start
from scout's best parameters and progressively quench toward those
coordinates. The final particle cloud is tightly concentrated near
the local MLE.

### 4.3 The pattern is classical simulated annealing

Scout is the exploration phase (stays hot to find basins); refine is
the quenching phase (cools sharply to lock onto the MLE scout
identified). This is exactly the "run hot, then freeze" schedule from
simulated annealing textbooks. The user's intuition was tracking this
correctly from the start.

---

## Part 5 — Documentation bug catalogue

Every user-facing statement I could find about scout/refine cooling,
cross-referenced against the code:

| Source | Lines | Statement | Accurate? |
|---|---|---|---|
| `rust/crates/cli/src/fit/scout.rs` | 14 | `SCOUT_COOLING = 0.70` | ✓ authoritative |
| `rust/crates/cli/src/fit/refine.rs` | 15 | `REFINE_COOLING = 0.05` | ✓ authoritative |
| `rust/crates/cli/src/if2.rs` (module header) | 8–10 | "scout: 200 particles, 20 iters, no cooling" | **partially wrong**: particles/iters stale (current 500/30); "no cooling" is an oversimplification of 0.70 |
| `rust/crates/cli/src/if2.rs` (module header) | 9 | "refine: cooling=0.95" | **wrong**: code has 0.05 |
| `docs/camdl-run-spec.md` | 1347 | "Lower values = more aggressive cooling (better for exploration/scout); higher values = gentler cooling (better for refinement near an optimum)" | **direction reversed**: scout uses *higher* cooling_fraction (gentler), refine uses *lower* (aggressive) |
| `docs/camdl-language-spec.md` | 2832 | "scout (cooling=0.5), refine (cooling=0.95)" | **both wrong**: code has 0.70 and 0.05 respectively |
| `docs/inference.md` | 474 | "scout ... no cooling" | loosely defensible (0.70 ≈ "barely cools"); better phrasing: "mild cooling" |
| `docs/inference.md` | 478 | "refine ... cooling=0.95" | **wrong**: code has 0.05 |
| `camdl-book/vignettes/he2010/fit_synthetic.toml` | scout block | `cooling = 0.9` | works but **ignores code default** (0.70); slightly gentler than default |
| `camdl-book/vignettes/he2010/fit_synthetic.toml` | refine block | `cooling = 0.97` | **dramatically wrong direction**: default is 0.05; this value gives final SD = 0.94 × initial, barely any cooling at all, so refine won't converge tightly |
| `camdl-vignettes/he2010-inference/fit_he2010.toml` | scout | `cooling = 0.9` | gentler than default |
| `camdl-vignettes/he2010-inference/fit_he2010.toml` | refine | `cooling = 0.95` | **wrong direction**: 0.05 is the default |

### 5.1 Why the docs and configs drifted

Tracing the pattern: the `docs/camdl-run-spec.md:1347` statement
reads "lower values = aggressive cooling, better for scout." In
isolation, this description of `cooling_fraction` is grammatically
correct — a *lower* value *does* produce more aggressive cooling. The
error is in the trailing advice clause ("better for exploration/scout"):
that's backwards given what IF2 scout is actually doing with
cooling_fraction=0.70 (mild cooling, deliberately).

It looks like this line was written by someone who internalised the
simulated-annealing textbook rule ("explore first, cool slowly") and
applied it to pick the clause direction — without cross-checking
against what the scout stage is actually tuned for in camdl's
particular design. The `camdl-language-spec.md` preset values appear
to have been copied from that same mental model: "scout cooling=0.5
(aggressive, because exploration)" — which matches the wrong mental
model rather than the SCOUT_COOLING=0.70 constant in the code.

The vignette fit.toml files were then written by someone following the
wrong spec-doc convention. They don't reproduce the *exact* spec-doc
numbers (scout=0.9, not 0.5; refine=0.95 or 0.97, not 0.95), but they
follow the spec-doc *direction* (scout with a smaller cooling_fraction
than refine, or both in the "gentle cooling" range).

Empirically these configs still produce usable fits because the IF2
algorithm is forgiving of imperfect cooling schedules. But they
under-converge: a refine stage with cooling=0.97 produces chains
whose final SDs are still 94% of initial — barely any tightening
around scout's estimate. The run cost is there; the concentration
benefit isn't.

---

## Part 6 — Proposed fixes

**None of these should be checked in automatically. Each is a small,
localised edit that a reviewer should eyeball.**

### 6.1 `rust/crates/sim/src/inference/if2.rs` module header (lines 7–10)

Current:
```
//!   scout:    8 chains, 200 particles, 20 iters, no cooling, random starts
//!   refine:   4 chains, 1000 particles, 50 iters, cooling=0.95
//!   validate: 4 chains, 5000 particles, 100 iters, cooling=0.95
```

Replace with (mirroring the authoritative constants in
`fit/scout.rs` and `fit/refine.rs`):
```
//!   scout:    8 chains, 500 particles, 30 iters, cooling=0.70 (mild — find basins)
//!   refine:   4 chains, 1000 particles, 50 iters, cooling=0.05 (aggressive — converge to MLE)
//!   validate: 4 chains, 5000 particles, 100 iters, cooling=0.05 (aggressive — final polish)
```

### 6.2 `docs/camdl-run-spec.md:1347`

Current (reversed):
```
/// Lower values = more aggressive cooling (better for exploration/scout);
/// higher values = gentler cooling (better for refinement near an optimum).
```

Replace with:
```
/// Cooling fraction is pomp's cooling.fraction.50 convention: at the
/// halfway point of the stage, the perturbation SD is `cooling_fraction`
/// times its initial value; at the end, it is `cooling_fraction²` times
/// initial.
///
/// Higher values (closer to 1) = gentler cooling, SD stays large, chains
/// keep exploring — used for **scout** (default 0.70 → 49% final SD).
///
/// Lower values (closer to 0) = aggressive cooling, SD collapses, chains
/// concentrate on the MLE — used for **refine** (default 0.05 → 0.25%
/// final SD) and **validate**.
```

### 6.3 `docs/camdl-language-spec.md:2832`

Current:
```
`scout` (8 chains, 500 particles, cooling=0.5),
`refine` (4 chains, 1000 particles, cooling=0.95),
`validate` (4 chains, 5000 particles, cooling=0.95).
```

Replace with:
```
`scout` (8 chains, 500 particles, 30 iters, cooling=0.70 — mild cooling
for basin exploration),
`refine` (4 chains, 1000 particles, 50 iters, cooling=0.05 — aggressive
cooling to converge onto scout's best),
`validate` (4 chains, 5000 particles, 100 iters, cooling=0.05 — final
polish at scout's converged point).
```

### 6.4 `docs/inference.md` lines 472–481

Current:
```
**Scout** (`--regime scout`): 8 chains, 200 particles, 20 iterations, no
cooling. Pure exploration — chains wander freely to map the likelihood surface.

**Refine** (`--regime refine`): 4 chains, 1000 particles, 50 iterations,
cooling=0.95. Converge to the MLE from the best scout endpoints.
```

Replace with:
```
**Scout** (`--regime scout`): 8 chains, 500 particles, 30 iterations,
cooling=0.70 (mild — SD drops to 49% of initial across the stage).
Exploration: chains stay hot enough to wander across basins rather than
quenching onto the first local optimum. Cross-chain Rhat at the end
diagnoses multi-modality.

**Refine** (`--regime refine`): 4 chains, 1000 particles, 50 iterations,
cooling=0.05 (aggressive — SD drops to 0.25% of initial). Starts from
the best scout chain's parameters and collapses chains tightly onto
the local MLE.
```

### 6.5 Vignette configs — recommended audit (not required)

The following fit.toml files have cooling values that produce
sub-optimal convergence behaviour relative to the code defaults. They
still work but under-converge in refine:

- `camdl-book/vignettes/he2010/fit_synthetic.toml`:
  change scout `cooling = 0.9` → `0.70` (or remove, falling through to
  SCOUT_COOLING default); change refine `cooling = 0.97` → `0.05`
- `camdl-book/vignettes/he2010/fit_synthetic_fixed_s0.toml`: same edit
- `camdl-vignettes/he2010-inference/fit_he2010*.toml`: same edit

Before applying: rerun any existing scout/refine results that depend
on these configs, because the new cooling schedule will produce
different (more tightly converged) MLEs. This is a scientific change,
not just a documentation one — should be flagged to downstream users.

### 6.6 One worked numerical example for the docs

I'd recommend adding this worked example to whichever doc becomes the
single canonical source on cooling (probably `camdl-run-spec.md`):

> **Worked example: scout with `cooling=0.70`, `iterations=30`, 1096
> weekly observations.**
>
> The formula at `rust/crates/sim/src/inference/if2.rs:250-251`
> computes
>
> ```
> per_step_cooling = 0.70 ^ (2 / (30 × 1096)) ≈ 0.999978
> ```
>
> After iteration *m*, the effective perturbation SD is
>
> ```
> SD(m) / initial_SD = per_step_cooling^(m × 1096)
> ```
>
> giving a predictable cooling curve:
>
> | iter | SD / initial |
> |---|---|
> | 0 | 1.000 |
> | 15 (halfway) | 0.700 |
> | 30 (end) | 0.490 |
>
> The "halfway SD equals cooling_fraction" property is what makes
> this pomp's `cooling.fraction.50` convention — named because at
> 50% of the target iterations, you're at the configured fraction.

---

## Part 7 — What to do with this report

Order of operations:

1. **Land the doc fixes** (`camdl-run-spec.md`, `camdl-language-spec.md`,
   `docs/inference.md`, `if2.rs` module header). These are the priority
   because every contributor who reads the docs currently gets a wrong
   mental model, which has already caused bugs in the vignette configs
   and wasted debugging time.
2. **Audit the vignette configs** with the corrected cooling values,
   rerunning the fits so downstream artifacts reflect actually-converged
   refine stages.
3. **Then** resume whatever code change prompted the question. No
   cooling-adjacent refactor should go in until the docs are truth.

I'll wait for explicit instruction before touching any of these files.
This report is the evidence; the edits are mechanical once approved.

---

## Appendix A — Full empirical log (iter 0–29, SD per iteration)

From the instrumented run described in §3.2; the agent-internal log
file was `/tmp/scout_cooling.log` on the run host and has since been
discarded. The reformatted table in §3.3 is the durable artifact.

The `per_step_cooling` value logged was `0.999993591230` across all
iterations (as expected — it's a stage-level constant). The
`global_step` counter incremented by 1097 per iteration (1 t=0 tick +
1096 per-obs ticks), which is the same formula used to produce the
analytical predictions in Parts 1.3 and 4.2.

## Appendix B — Why the two "correct" answers earlier were inconsistent

Record of reasoning failures, for future reference:

1. **First answer** ("scout uses aggressive cooling"): was correct,
   recalled from memory of the simulated-annealing convention but
   without checking the code. Happened to match the user's
   spec-doc-informed mental model at the time — no pushback, no
   verification.
2. **Second answer** ("scout is slow cooling"): after user pushback,
   reasoned from first principles ("exploration = stay hot") and
   concluded the first answer was wrong. Still didn't check the code.
3. **Third answer** (back to "scout is aggressive"): finally read the
   docs (`camdl-run-spec.md`, `camdl-language-spec.md`), took them at
   face value — which re-validated the first answer. But the docs
   were themselves wrong, and the third answer was wrong too.
4. **Fourth answer** (back to "scout is mild, refine is aggressive"):
   read the actual code constants `SCOUT_COOLING = 0.70` and
   `REFINE_COOLING = 0.05`, which are unambiguous. Cross-validated
   with an empirical instrumented run. This is the correct answer.

The lesson: on a question of "what does this tool do," code >
comments > docs > memory. I skipped to memory, then to docs, only
ending at code after the user insisted. The cost was four
messages of flip-flopping before converging on a two-minute
verification. `CLAUDE.md` flags this as "don't silently comply with
pushback if the evidence points otherwise" — but the dual failure
here was *also* "check the code before speaking," which I elided
until forced. Both of us would have saved time if I'd opened
`fit/scout.rs` on your first question.
