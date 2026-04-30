---
status: open
date: 2026-04-30
scope: full codebase — correctness pass (Critical and High severity)
reviewer: internal (post-v1-cleanup full sweep, HEAD = 5627ef1)
triggered-by: scheduled full-codebase audit following prior-types consolidation and
  v2-native fit-run landing
---

# Correctness review — 2026-04-30

Full pass over `rust/crates/sim/src/inference/`, `rust/crates/cli/src/fit/`,
`ocaml/lib/`, and the IR boundary. Focus: findings that can produce wrong
scientific output with no runtime error. Design and low-severity items are in
the companion `2026-04-30-design.md`.

Verified-against: HEAD `5627ef1` (docs: prior types consolidation).

## Summary

**Strong:** The prior-types consolidation (PriorSpec → `ir::PriorDist`)
eliminates the parallel-type smell that was flagged in the April 20 review. The
`RunStatus` typed enum removes the `wall_time_seconds == 0.0` sentinel. Typed
`Backend` and `MethodKind` enums are in place. All April 20 Major findings
(RdM1–RdM4) are resolved. The inference stack's log-space discipline,
resampling, ESS, and ancestor-sampling implementations are correct. The
`log_sum_exp` IM2 fix is sound.

**Needs work:** One observation likelihood path produces a positive
log-probability for out-of-range Bernoulli probabilities — invalid for an SMC
weight and capable of silently inflating posterior mass. Weight normalization is
duplicated eight times with diverging fallback logic. The seed-range parser
silently returns an empty vector on malformed input; a user who typos a range
gets zero fits with no error. The OCaml deserializer does not enforce the
`prior ⊕ hierarchical` invariant; a hand-crafted IR with both fields set
produces an invalid `Parameter` record that neither the OCaml validator nor the
Rust IR validator catches.

---

## Findings

### Critical

---

**C1. `Bernoulli` observation likelihood produces positive log-probabilities
when `p_val > 1.0`.**

`rust/crates/sim/src/inference/obs_model.rs:165–168`:

```rust
ResolvedLikelihood::Bernoulli { p } => {
    let p_val = eval_resolved(p, &ctx(projected));
    if observed > 0.5 { p_val.max(LOG_PROB_FLOOR).ln() }
    else              { (1.0 - p_val).max(LOG_PROB_FLOOR).ln() }
}
```

When `observed > 0.5` (i.e., the observation is 1) and `p_val > 1.0`,
`p_val.max(LOG_PROB_FLOOR)` is `p_val` (since `p_val` already exceeds
`LOG_PROB_FLOOR`), so `.ln()` returns a **positive number** — e.g.,
`ln(2.0)
≈ 0.693`. This is an invalid log-probability (log-probs are ≤ 0). It
inflates the SMC weight for that particle, pulling the posterior toward
parameter values that predict `p_detect > 1`.

The sampling path has the same problem:

`obs_model.rs:280–283`:

```rust
ResolvedLikelihood::Bernoulli { p } => {
    let p_val = eval_resolved(p, &ctx(projected));
    if rng.uniform() < p_val { 1.0 } else { 0.0 }
}
```

If `p_val > 1.0`, `rng.uniform() ∈ [0,1)` is always less than `p_val`, so the
sampler always returns `1.0`. If `p_val < 0.0`, the sampler always returns
`0.0`. Both silently corrupt synthetic-data generation for model checking.

**Why it matters.** Bernoulli is actively used — the language spec example at
line 1697 of `camdl-language-spec.md` uses
`likelihood = bernoulli(p =
p_detect)`. During PGAS or PMMH exploration, the
parameter `p_detect` can visit values outside [0,1] before the posterior
concentrates. Every such visit produces an artificially inflated weight
(observation-1 case) or a zero-weight (observation-0 case), biasing the
posterior. For low-incidence detection models where most observations are 0, the
bias is especially pernicious: weights on the `p_detect > 1` side are
over-counted, weights on the `p_detect < 0` side are under-counted, and the
posterior shifts.

The inconsistency with every other likelihood is telling: Binomial clamps at
`obs_model.rs:263`, BetaBinomial floors alpha/beta at `obs_model.rs:270–271`,
NegBinomial guards at `obs_model.rs:246`. Bernoulli was missed.

**Fix:**

```rust
ResolvedLikelihood::Bernoulli { p } => {
    let p_val = eval_resolved(p, &ctx(projected)).clamp(0.0, 1.0);
    if observed > 0.5 { p_val.max(LOG_PROB_FLOOR).ln() }
    else              { (1.0 - p_val).max(LOG_PROB_FLOOR).ln() }
}
```

Sampling path:

```rust
let p_val = eval_resolved(p, &ctx(projected)).clamp(0.0, 1.0);
if rng.uniform() < p_val { 1.0 } else { 0.0 }
```

**Severity:** Critical. Silently inflates SMC weights and corrupts synthetic
data under out-of-range parameter proposals. Detection is hard: the run
completes with no error, posteriors shift, and the direction of shift depends on
the proportion of 1-vs-0 observations.

---

### High

---

**H1. Weight normalization duplicated eight times with diverging fallback
logic.**

The log-weight → normalized-probability conversion appears at:

- `particle_filter.rs:342–350` (`weighted_quantiles`)
- `if2.rs:408–416` (per-parameter diagnostics)
- `resampling.rs:22–34` (`systematic_resample`)
- `correlated_pf.rs:420–432` (`sorted_systematic_resample`)
- `ancestor_trace.rs:85–93`
- `pgas.rs:1008–1016` (`sample_categorical_log`)
- `types.rs:294–299` (`ParticleSwarm::ess`)
- `types.rs:310–313` (`log_sum_exp`)

All-weight-degenerate fallback logic differs across sites:

| Site                     | Guard                                 |
| ------------------------ | ------------------------------------- |
| `particle_filter.rs:342` | `max_lw.is_infinite()`                |
| `if2.rs:408`             | `max_lw.is_finite()` (inverted sense) |
| `resampling.rs:22`       | `max_lw.is_infinite()`                |
| `ancestor_trace.rs:85`   | `max_lw.is_finite()`                  |

The `is_finite()` vs `is_infinite()` discrepancy matters when `max_lw` is NaN:
`NaN.is_infinite()` is `false` (so the degenerate-weight branch is _not_ taken,
and the raw `exp` of `NaN - NaN = NaN` propagates as particle weight `NaN`);
`NaN.is_finite()` is also `false` (so the uniform-fallback branch _is_ taken).
The sites that use `is_infinite()` will propagate NaN weights rather than
falling back to uniform — a silent corruption of the particle filter that
produces no diagnostic.

**Why it matters.** NaN log-weights arise under model misspecification: a
parameter combination that makes a rate expression produce NaN (e.g., via a
`0/0` in a `Cond` that is guarded but evaluated on both branches). The correct
behaviour is a uniform-weight fallback (equivalent to a weight-degenerate
filter), not silent NaN propagation. The four sites using `is_infinite()` take
the wrong path.

**Fix:** Extract a single canonical helper in `inference/types.rs`:

```rust
/// Normalize log-weights to a probability vector.
/// All-degenerate (-inf or NaN) falls back to uniform.
pub fn normalize_log_weights(log_weights: &[f64]) -> Vec<f64> {
    let n = log_weights.len();
    let max_lw = log_weights.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !max_lw.is_finite() {
        return vec![1.0 / n as f64; n];
    }
    let raw: Vec<f64> = log_weights.iter().map(|&lw| (lw - max_lw).exp()).collect();
    let sum: f64 = raw.iter().sum();
    if sum <= 0.0 { vec![1.0 / n as f64; n] }
    else { raw.iter().map(|&w| w / sum).collect() }
}
```

Replace all eight sites. Add a unit test:
`normalize_log_weights(&[f64::NAN,
f64::NAN])` returns `[0.5, 0.5]`.

**Severity:** High. The `is_infinite()` vs `is_finite()` divergence is a latent
bug that fires under model misspecification — exactly the condition where the
inference engine's robustness is most needed. Four of eight sites take the wrong
path.

---

**H2. `SeedsSpec::to_vec()` silently returns an empty vector on malformed seed
ranges.**

`rust/crates/cli/src/fit/config_v2.rs:277–284`:

```rust
impl SeedsSpec {
    pub fn to_vec(&self) -> Vec<u64> {
        match self {
            SeedsSpec::List(xs) => xs.clone(),
            SeedsSpec::Range(s) => parse_seed_range(s).unwrap_or_default(),
        }
    }
```

`parse_seed_range` returns `None` for malformed ranges (non-integer tokens,
inverted bounds like `"20:1"`, missing colon). `unwrap_or_default()` returns
`vec![]`. The downstream code at `fit run` iterates over this vector and
launches zero fit replicates — the command exits successfully with no output, no
error, and a `RunKind::ReplicateSet` umbrella containing zero children.

`validate_no_duplicates()` at line 286 catches the empty-vector case, but only
when `validate()` is called. Callers that construct `SeedsSpec` from TOML and
call `to_vec()` before `validate()` (or call `to_vec()` at any point outside the
validation path) get a silent zero-length slice.

**Why it matters.** A `fit_seeds = "1:20"` that is mistyped as
`fit_seeds = "1-20"` (hyphen vs colon) produces a fit run with zero replicates.
The user sees no error and gets an empty results directory. Any downstream
analysis that consumes those results — a summary table, a model comparison —
processes zero rows silently.

**Fix:** Change `to_vec()` to return `Result<Vec<u64>, String>` and propagate
the error to callers. Alternatively, make the `unwrap_or_default()` a hard
error:

```rust
SeedsSpec::Range(s) => parse_seed_range(s)
    .ok_or_else(|| format!(
        "malformed seed range '{}' — use 'start:end' with start ≤ end", s)),
```

Move the duplicate-check into the same `Result` chain so validation is
structurally enforced at construction, not deferred.

**Severity:** High. Produces zero-replicate fits with no diagnostic under a
plausible typo. Downstream analyses process empty results without error.

---

**H3. OCaml deserializer does not enforce `prior ⊕ hierarchical` invariant; Rust
IR validator is silent on the same.**

`ocaml/lib/ir/serde.ml:736–737` (deserializer):

```ocaml
prior        = (match member_opt "prior" j with
                | Some `Null | None -> None | Some p -> Some (prior_dist_of_json p));
hierarchical = (match member_opt "hierarchical" j with
                | Some `Null | None -> None | Some h -> Some (hierarchical_prior_of_json h));
```

Both fields are independently deserialized with no mutual-exclusion check. A
JSON `parameter` object with both `"prior"` and `"hierarchical"` set to non-null
values produces an `Ir.parameter` record violating the invariant documented at
`ocaml/lib/ir/ir.ml:228`:

```ocaml
(* mutually exclusive with prior *)
hierarchical: hierarchical_prior option;
```

`ocaml/lib/ir/validate.ml` does not check this invariant.
`rust/crates/ir/src/validate.rs` has no mention of `prior` or `hierarchical`.

The OCaml _compiler_ correctly enforces mutual exclusion at line 1914–1916 of
`expander.ml` via a `prior_classification` variant. But the deserializer is a
separate code path — used when loading an existing `.ir.json` for simulation or
when a user hand-crafts the IR. That path has no guard.

**Why it matters.** The Rust backend's `resolve_prior()` (in `sampling.rs`) does
not check which field is populated and would use whichever it encounters first.
A parameter with both fields set could resolve using the wrong prior, producing
posterior inference under a prior the user did not intend and no error message.

**Fix (OCaml serde):** After deserializing both fields, add:

```ocaml
let () =
  (match prior, hierarchical with
   | Some _, Some _ ->
     failwith (Printf.sprintf
       "parameter '%s': prior and hierarchical are mutually exclusive"
       name)
   | _ -> ())
```

**Fix (Rust IR validate):** In `rust/crates/ir/src/validate.rs`, add a check
over all parameters:

```rust
if p.prior.is_some() && p.hierarchical.is_some() {
    return Err(format!(
        "parameter '{}': prior and hierarchical are mutually exclusive", p.name));
}
```

**Severity:** High. A hand-crafted or externally generated IR with both fields
set produces wrong inference under the wrong prior with no diagnostic.

---

**H4. Tempering ladder validation accepts `β > 1` and `β ≤ 0` entries.**

`rust/crates/cli/src/fit/pgas.rs:52–56`:

```rust
if tempering.is_empty() || (tempering[0] - 1.0).abs() > 1e-9 {
    return Err(format!(
        "stage tempering ladder must start with β=1.0 (cold chain). Got: {:?}",
        tempering));
}
```

This checks that `tempering` is non-empty and that `tempering[0] ≈ 1.0`. It does
not check that the remaining entries are in `(0, 1]`. A user who writes
`tempering = [1.0, 1.5, 0.4]` (entry > 1) or `tempering = [1.0, -0.2]` (negative
entry) passes validation. Both configurations are physically nonsensical: a
temperature `β > 1` concentrates the likelihood (sharper than the posterior),
and a negative temperature inverts it.

In the PGAS implementation, each rung scales its log-likelihood by `β`:
`ll_rung = β × ll_cold`. A `β = -0.2` rung would accept proposals that decrease
the likelihood, effectively running an anti-annealing chain. No runtime error
fires; the chain converges to the wrong target.

**Fix:** Extend the validation:

```rust
for (i, &beta) in tempering.iter().enumerate() {
    if !(beta > 0.0 && beta <= 1.0) {
        return Err(format!(
            "tempering[{}] = {} is out of range (0, 1]; all β must be \
             positive and ≤ 1.0", i, beta));
    }
}
```

Also check that the ladder is non-increasing (a strictly decreasing ladder from
1.0 is conventional, though not required for correctness).

**Severity:** High. An invalid tempering ladder runs without error and converges
to the wrong posterior target.

---

## Resolved findings (April 20 open items now closed)

| ID    | Item                                                     | Resolved in                                                                       |
| ----- | -------------------------------------------------------- | --------------------------------------------------------------------------------- |
| RdM1  | `EstimatedParam`/`Transform` in `if2.rs`                 | `inference/types.rs` (2026-04-20)                                                 |
| RdM2  | `rate_grads` name-keyed linear scan                      | `rate_grads_indexed` in `CompiledModel` (2026-04-20)                              |
| RdM3  | `ChainResumeState` / `PMMHResumeState` shared fields     | `restore_z_values` in `types.rs`; struct layout unchanged for bincode compat      |
| RdM4  | `log_transition_density_substep` 162-line monolith       | `compute_source_group_probs`, `exit_and_split_log_density` extracted (2026-04-20) |
| Rdm1  | `InferenceConfig` trait absent                           | `traits.rs` (2026-04-20)                                                          |
| Rdm2  | `1e-300` bare literal × 8 sites                          | `LOG_PROB_FLOOR` constant in `types.rs` (2026-04-20)                              |
| Rdm3  | RNG init DRY                                             | `init_particle_rngs` in `types.rs` (2026-04-20)                                   |
| Rdm4  | `n_obs`/`steps_per_obs` in `PMMHConfig`                  | Removed; `run_pmmh` derives them internally (2026-04-20)                          |
| —     | `wall_time_seconds == 0.0` sentinel                      | `RunStatus` typed enum (2026-04-28)                                               |
| —     | Stringly-typed `Backend`/`MethodKind`                    | Typed enums in `args/types.rs` (2026-04-28)                                       |
| —     | `PriorSpec` parallel type                                | Re-exported as `ir::PriorDist`; `prior_spec_to_prior` deleted (2026-04-29)        |
| IM2   | `log_sum_exp` returning finite value for all-`-∞` inputs | Fixed; returns `NEG_INFINITY` correctly                                           |
| IM6   | PGAS ancestor sampling weight bug                        | Fixed; stale pre-resample weight removed from ancestor computation                |
| IM7/9 | Gradient evaluator skipping overdispersion groups        | Fixed; mirrors `pgas.rs` exactly                                                  |
