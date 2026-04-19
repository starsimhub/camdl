---
status: proposal
date: 2026-04-19
---

# Gate Refine on Scout Convergence

## Motivation

The downstream hit a real pipeline failure while fitting a
time-varying-β SIR (`β(t) = β₀ · exp(-δt)`) against boarding-school
data. Scout found chains across several distinct basins — final
per-chain loglik spread 794 log-units, tail-Rhat on structural
parameters 2–3, on IVP parameters up to 16. Scout's diagnostics
block correctly said "likelihood surface may be multi-modal, do not
continue." The pipeline ran refine anyway. Refine's
`starts_from = "scout"` seeded top-K scout chains into refine, tight
cooling (0.05) collapsed them onto *one* mode, and refine's Rhat
table read `1.05–1.08 ✓` across the board. Downstream reported the
fit as converged.

The key evidence only visible by eyeballing history: refine's
`best_loglik = −76.3`, *worse* than scout's best chain at `−60.2`.
Refine regressed by ~16 log units relative to scout — almost
certain evidence that refine picked up chains from a suboptimal
basin and tight-cooled them there, never revisiting the basin
scout's global best had found.

Every individual diagnostic did its job. Scout honestly reported
multi-modality. Refine honestly reported "my chains agree." The
pipeline-as-a-whole didn't compose those into a decision. The
refine Rhat *laundered* an unconverged scout into a false
"converged" answer.

This is a silent-wrong-answer class bug in the sense of
`CLAUDE.md`: the user sees a confident result; the confidence is
the lie.

## Design

Two gates, firing on different evidence. Either one being violated
blocks refine from reporting a clean result; both together catch the
observed failure mode redundantly.

### Gate 1 — scout tail-Rhat (pre-refine)

Before dispatching refine, read scout's `fit_state.toml` and
inspect the tail-Rhat on every **non-IVP** estimated parameter. If
any exceeds the hard threshold (default 1.10), error.

Soft-warn band: 1.05 < Rhat ≤ 1.10. Refine still runs but the
startup block prints a prominent warning echoing the scout
diagnostic.

IVP parameters (those marked `ivp = true`) are reported in the
Rhat table but not gated. IVP parameters are perturbed only at
`t=0` and their chain-identification is expected to be weaker —
gating on IVP Rhat would block legitimate fits where structural
convergence was fine.

### Gate 2 — loglik regression (post-refine)

After refine's chains complete, compare `refine.best_loglik`
against scout's. If refine regressed by more than ε log units,
error — refine is supposed to polish, not regress. This check is
independent of any Rhat threshold and catches the observed failure
mode regardless of how one answers the Rhat questions.

ε is set conservatively: `ε = max(3.0, 2 · σ_scout_chains)` where
`σ_scout_chains` is the standard deviation of scout chains' final
logliks. Three log units is roughly "noise floor of the PF
log-likelihood estimator at typical particle counts"; the `2σ`
term expands the tolerance when scout itself had high between-chain
loglik variance (i.e. when scout was multi-modal and refine might
legitimately land on one of several good basins).

### Overrides

One CLI flag: `--allow-nonconverged-scout`. Bypasses Gate 1 only.
Scoped per-invocation (not a TOML setting that rots into a
permanent bypass). Typical use: the user accepts that results may
launder multi-modality and wants the fit anyway (exploratory
workflow, debugging).

**Gate 2 is not overridable.** If refine regresses by >ε, something
is wrong with the run itself — the check doesn't reflect user
preference. A failure here indicates a near-certain bug in refine's
handoff or cooling, not a philosophical stance about multi-modality.
Making it bypass-able would re-open the exact failure mode this
proposal exists to prevent.

## Config surface

No new TOML settings. The defaults are the right defaults.

CLI:

```
camdl fit run fit.toml                            # Gate 1 + Gate 2 both active
camdl fit run fit.toml --allow-nonconverged-scout # Gate 1 off, Gate 2 still active
```

Thresholds (1.10 hard / 1.05 soft / ε = max(3, 2σ)) are constants
in the code, not user-tunable. If a future user needs different
thresholds, add them via CLI flags (`--rhat-gate N`, `--loglik-gate
E`) at that point rather than baking per-project knobs into every
fit.toml.

## Error messages

Both gates produce actionable errors that name the failing values.

### Gate 1 error

```
error: refine stage requires scout convergence.

  Scout tail-Rhat (last half of iterations):
    ✗ beta_0   Rhat = 3.502   (> 1.10)
    ✗ gamma    Rhat = 2.105   (> 1.10)
    ~ delta    Rhat = 1.194   (> 1.10)
      I0       Rhat = 16.527  (ivp — not gated)
      R_init   Rhat = 5.516   (ivp — not gated)

  Scout loglik spread: 794.4 (max = −60.2, min = −854.6)
  -> likelihood surface is almost certainly multi-modal.

  Pick one:
    - re-run scout with more chains or iterations
    - narrow bounds to the basin scout's best chain found:
        beta_0 ≈ 1.834  (chain 4, ll = −60.2)
        gamma  ≈ 0.512
        delta  ≈ 0.082
      copy into [estimate.*] bounds / start values
    - mark the weak-identification params as `ivp = true`
      (they'll be reported but not gated)

  To run refine anyway (results may launder multi-modality):
    camdl fit run fit.toml --allow-nonconverged-scout
```

### Gate 2 error

```
error: refine regressed below scout.

  scout  best_loglik = −60.2 (chain 4)
  refine best_loglik = −76.3 (chain 1)   delta = −16.1, threshold = ε = 4.2

  Refine landed in a worse basin than scout found. This is a
  pipeline failure, not a user-facing knob — refine is supposed
  to polish scout's best, not regress from it. Possible causes:

    - scout was multi-modal and refine's starts_from filter picked
      top-K chains from the wrong basin (check scout per-chain
      loglik spread and re-run with tighter bounds around scout's
      best chain)
    - refine cooling too aggressive given rw_sd; collapsed on the
      first accessible local maximum
    - the model or data changed between stages (hash mismatch — see
      run.json)

  scout/fit_state.toml is authoritative for "what scout's best
  looked like." Investigate before re-running.
```

## Implementation

### Persist scout's Rhat

`fit_state.toml` currently carries `best_loglik`, `n_chains`,
`n_good_chains`, `start_values`, `rw_sd` — no per-param Rhat. Add
two fields:

```rust
pub struct FitState {
    // ...existing fields...

    /// Per-parameter tail-Rhat (last half of iterations). Populated
    /// by scout and refine after they compute Rhat at end-of-stage.
    /// Loaded by refine (and any other downstream consumer) to gate
    /// on convergence without re-running.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tail_rhat: HashMap<String, f64>,

    /// Names of estimated parameters that were declared `ivp = true`.
    /// Loaded by refine to know which Rhat entries to exempt from
    /// gating. Stored with the stage that produced it so refine
    /// doesn't re-derive from the fit.toml (avoids mismatch if the
    /// fit.toml changed between stages — hash-check covers this but
    /// the list is cheap to persist).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ivp_params: Vec<String>,

    /// Per-chain final log-likelihoods (what scout reports before
    /// picking `best_loglik`). Gate 2 uses the spread to compute
    /// its ε tolerance. Short vector, cheap to serialise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain_logliks: Vec<f64>,
}
```

All three fields are `#[serde(default)]` so existing fit_state.toml
files load without migration. Scout's save-path populates them;
refine's load-path checks them. Absent values mean "legacy stage
output" — in that case refine can't gate and emits a warning
saying so (but doesn't refuse to run; we're not forcing users to
re-run old fits just to get stricter gates on new ones).

### Gate logic

One helper in `fit::runner` or a new `fit::gating` module:

```rust
pub enum ScoutGateVerdict {
    Ok,
    SoftWarn { param_rhats: Vec<(String, f64)> },
    Hard { param_rhats: Vec<(String, f64)>, loglik_spread: f64 },
}

pub fn check_scout_convergence(
    scout_state: &FitState,
    threshold_hard: f64,   // 1.10
    threshold_soft: f64,   // 1.05
) -> ScoutGateVerdict { ... }

pub fn check_loglik_regression(
    scout_best: f64,
    refine_best: f64,
    scout_chain_logliks: &[f64],
) -> Result<(), String> { ... }
```

Refine's entry path (`fit/refine.rs` and the v2 dispatch in
`fit/mod.rs`) calls `check_scout_convergence` after loading scout's
`FitState`, honours `--allow-nonconverged-scout` to downgrade Hard
to SoftWarn, and errors on Hard. After refine's chains complete,
calls `check_loglik_regression` before writing refine's
`fit_state.toml` and errors if violated (without writing the
stale fit_state — we don't want a "refine declared done"
filesystem artefact when refine actually regressed).

### CLI

One new flag in `parse_args_fit_run` (and the legacy `fit refine`
subcommand):

```
--allow-nonconverged-scout   Skip the tail-Rhat gate on scout's
                             output. Does NOT skip the post-refine
                             loglik-regression check.
```

## Test plan

Seven tests:

- **`refine_errors_on_unconverged_scout`** — construct a `FitState`
  with `tail_rhat["beta"] = 1.5`, call the gate, assert Hard verdict
  naming `beta`. Pure unit test on the check function, doesn't
  require a full fit run.

- **`refine_ignores_ivp_rhat_in_gate`** — same but `tail_rhat["I0"]
  = 16.5` with `ivp_params = ["I0"]` and structural params
  converged. Assert Ok.

- **`refine_soft_warns_between_thresholds`** — Rhat = 1.07, assert
  SoftWarn with the param named, not Hard.

- **`refine_override_bypasses_rhat_gate`** — end-to-end CLI test:
  construct a fit config where scout's tail-Rhat is high, run
  `fit run ... --allow-nonconverged-scout`, assert completion.

- **`refine_errors_on_worse_loglik_than_scout`** — construct scout
  and refine `best_loglik` values with refine > 10 units worse than
  scout, call the regression check, assert error naming both values
  and the delta.

- **`refine_override_does_not_bypass_loglik_check`** — end-to-end:
  scout converged but refine regressed (simulate by writing a
  degenerate refine fit_state), with `--allow-nonconverged-scout`
  set, assert the refine-regression error fires regardless. This
  is the load-bearing test — the override must NOT bypass Gate 2.

- **`legacy_scout_fit_state_warns_but_proceeds`** — if
  `scout/fit_state.toml` has no `tail_rhat` field (old camdl
  version), refine warns but doesn't refuse to run.

## Out of scope

- **Per-basin refine (Option C from the downstream's proposal).**
  When scout is multi-modal, cluster scout chains by final
  parameter values and run refine once per cluster, reporting all
  resulting MLEs. A legitimate workflow the downstream identified
  ("run multiple refine passes from different scout basins") but
  deserves its own proposal. One data point (the tvbeta fit) isn't
  enough to design the clustering + reporting UX, and making it a
  follow-up is cheaper than getting it wrong the first time. File
  when a second independent ask arrives.

- **Tunable thresholds.** The defaults (Rhat = 1.10 hard / 1.05
  soft, ε = max(3, 2σ)) are encoded as constants. If users start
  asking for per-project overrides, add CLI flags (`--rhat-gate N`,
  `--loglik-epsilon E`); don't bake per-project knobs into every
  fit.toml.

- **Gating stages other than refine.** Validate could in principle
  also gate (it's another stage handoff), but validate's input is
  refine, and refine is already gated by this proposal. Layering
  a third gate at validate is cheap to add later if a separate
  failure mode appears.

## Why this design

- **Two independent gates.** Gate 1 catches the "you don't know
  which basin" case before wasting refine compute. Gate 2 catches
  "refine demonstrably regressed" regardless of Rhat, which is
  the cheapest and most certain check and fires on different
  evidence. Either alone would have caught the downstream's bug,
  but having both is robust to future variants of the same pattern.

- **Override only for Gate 1.** The user has legitimate reasons to
  run refine despite non-convergence (exploratory work, multi-basin
  investigation). They have no legitimate reason to accept a
  regressed refine — that's not a statistical choice, it's a sign
  the run went wrong.

- **No new TOML settings.** Adding config knobs would invite the
  same "user changes it once, forgets, accidentally bypasses gate
  on every future fit" failure mode that this proposal exists to
  prevent. CLI flags are conscious per-invocation gestures.

- **Actionable error messages.** Both gates print the failing
  numbers AND the concrete fixes. The downstream's scout diagnostic
  already names the three remedies; Gate 1 re-uses that text so
  users see consistent guidance.

- **IVP exemption.** Users who declare `ivp = true` have already
  acknowledged the identification is weaker. Gating on IVP Rhat
  would block fits where the structural params are fine, creating
  pressure to mis-configure IVP params just to pass gating — the
  opposite of the principle.
