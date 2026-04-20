---
status: open
date: 2026-04-19
scope: rust/ sim inference crate — traits, types, linalg, resampling, priors, observation likelihoods (obs_loglik, obs_model, multi_stream_obs), chain_binomial_process adapter, particle_filter, if2
reviewer: external (via `scripts/review-zip.sh engine`, round 5)
---

# Inference code review — 2026-04-19 (batch 1: foundations + PF + IF2)

Companion to the 2026-04-19 compiler and engine reviews. This
pass covered ~2100 lines:

- Foundations: `traits.rs`, `types.rs`, `linalg.rs`,
  `resampling.rs`, `prior.rs`.
- Observation likelihoods: `obs_loglik.rs`, `obs_model.rs`,
  `multi_stream_obs.rs`.
- Particle filter + backend adapter:
  `chain_binomial_process.rs`, `particle_filter.rs`.
- Iterated filtering: `if2.rs`.

**Not yet covered** (separate batch): `correlated_pf.rs`,
`pmmh.rs`, `pgas.rs` (1718 lines — the densest scientific code),
`pgas_grad.rs`, `nuts.rs`, `diagnostic.rs`, and CLI drivers.
The PGAS review noted a pending concern about CSMC-AS ancestor
sampling — flagged for the next batch but not detailed yet.

## Summary

**Strong:** The foundations layer is tidy — `resampling.rs`
implements textbook systematic resampling; `prior.rs` maps IR
prior specs to typed variants cleanly; `obs_loglik.rs` has
legitimate references to He et al. for the discretized-Normal
design; `if2.rs` documents the pomp-style cooling math
explicitly; the IF2 implementation is well factored.

**Weak:** Two likelihood-handling bugs that corrupt inference
silently: `BetaBinomial` returns `-inf` for every observation,
and `Normal` silently coerces continuous observables to integer
counts. Both compile cleanly from the DSL and load cleanly into
the runtime. A panic-on-construction pattern hides
model-construction errors behind a process abort. Per-particle
heap allocation in a hot path contradicts a comment claiming it
doesn't happen.

## Findings

### Critical

**IC1. `BetaBinomial` observation likelihood silently returns
`-inf` for every observation.** `obs_model.rs:108–111`:

```rust
ResolvedLikelihood::BetaBinomial { .. } => {
    log::warn!("BetaBinomial obs_loglik not implemented — returning -inf");
    f64::NEG_INFINITY
}
```

The `sample()` path at lines 193–195 does the same thing
(returns 0). The OCaml expander fully supports emitting
`BetaBinomialLikelihood { n, alpha, beta }`, serde round-trips
it, and `resolve_likelihood` resolves the three subexpressions
without complaint. At eval time every particle gets `-inf`
log-weight. `log_sum_exp` (`types.rs:79–83`) early-returns
`NEG_INFINITY` when `max.is_infinite()`, the likelihood
increment becomes `-inf`, and `particle_filter.rs:172–175`
accumulates that into `total_loglik`. Every IF2 iteration then
reports `-inf`. A CLI user with `RUST_LOG` unset sees no warning
and just a `-inf` fit.

`Binomial` and `Bernoulli` are implemented. Three of six
likelihood types work; one is a `-inf` landmine.

Fix: either implement BetaBinomial log-pmf
(`lgamma(n+1) − lgamma(k+1) − lgamma(n−k+1)
  + log B(k+α, n−k+β) − log B(α, β)` using the existing
`lgamma`), or reject the likelihood at model-construction time
with a clean error. Silent `-inf` is strictly worse than both.

**IC2. `Normal` observation likelihood uses a discretized
integer-count PMF for what the DSL presents as a continuous
distribution.** `obs_model.rs:92–96`:

```rust
ResolvedLikelihood::Normal { mean, sd } => {
    let m = eval_resolved(mean, &ctx(projected));
    let s = eval_resolved(sd, &ctx(projected));
    discretized_normal_logpmf_tol(observed, m, s * s, DEFAULT_TOL)
}
```

`discretized_normal_logpmf` (`obs_loglik.rs:199`) rounds the
observation to a non-negative integer with a continuity
correction — correct for case counts, wrong for genuinely
continuous observables. A user who writes
`likelihood = normal(mean = projected, sd = sigma)` intending to
model log-transformed viral load, antibody titer, or any
real-valued quantity gets their data silently coerced to
non-negative integers before scoring. This is a semantic
mismatch between the DSL and the runtime.

Paired with the OCaml-side m19 finding (normal prior uses
`mu`/`sigma`, normal likelihood uses `mean`/`sd`), the Normal
likelihood is the most confusing surface in the model.

Fix options: document the discretization prominently, add a
separate `continuous_normal(...)` likelihood, or rename the
current form `normal_count(...)`.

### Major

**IM1. Per-particle RNG seeding is fragile.**
`particle_filter.rs:84`:

```rust
StatefulRng::new(seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95))
```

XOR of seed with a multiplied index with a single 64-bit
constant. Particles whose indices differ by predictable amounts
(`i` vs `i + 2^k`) have correlated XOR results in low bits.
ChaCha8's `expand_u64_to_seed` (`rng.rs:109–121`) mixes the u64
four times via single multipliers before seeding; correlation
propagates into the first few output blocks before the cipher's
rounds fully mix the state.

Same pattern at `if2.rs:405` and elsewhere for `diag_rngs`,
`resample_rng`. In practice for moderate N this likely tests
statistically clean, but the pattern is wrong — proper
per-stream seeding uses cryptographic key derivation or the
RNG's own stream-rekey (ChaCha has a stream counter that's the
documented way to do this).

Fix: `ChaCha8Rng::from_seed(seed)` then `.set_stream(i as u64)`
on the inner RNG, or a proper KDF. Low priority until someone
uses N in the tens of thousands and suspects particle-diversity
anomalies.

**IM2. Per-particle heap allocation in the multi-stream obs hot
path, contradicting the documented invariant.**
`multi_stream_obs.rs:233, 261, 290, 311`:

```rust
// in log_likelihood (line 290):
let int_s = IntState::new(self.n_int);  // per-stream, per-particle

// in project_stream_with_params (line 233):
let mut scratch = IntState::new(self.n_int);  // per-stream, per-particle, per-obs
scratch.counts.copy_from_slice(counts);
```

`IntState::new(n)` allocates a `Vec<i64>` of `n` zeros. In a PF
pass with N particles × T observations × S streams that's
N·T·S heap allocs per full filter pass. IF2 multiplies by
iterations (×50–100), scout by chains. For a nominal
`10⁴ × 10³ × 3` pass that's 3×10⁷ allocations per PF.

The comment at lines 150–153 explicitly claims "Allocation
happens at observation ticks only — not in the propensity hot
loop" — false in the read code.

Worse: `int_s` is only _read_ by `eval_likelihood_resolved` when
the likelihood expression references `Pop(...)`, which almost
none do (they typically only reference `projected` and params).
So the allocation is done and mostly unread.

Fix: pre-allocated `IntState` scratch per thread (rayon thread
local) or per-stream. Even a single mutable `IntState` field
cleared per call saves the allocation.

**IM3. `resolve_likelihood_from_model` panics on failure instead
of propagating.** `obs_model.rs:68–69`:

```rust
resolve_likelihood(likelihood, &ctx)
    .expect("observation likelihood resolution failed — this is a model construction bug")
```

Called from `MultiStreamObsModel::new` at lines 192–195. A
likelihood expression referencing an unknown parameter or
compartment (possible when the OCaml compiler's silent-fallback
patterns get past its own validate — compiler review C2 — and
before RC1's `ir::validate` wiring) panics instead of returning
`SimError::UnknownParameter`.

Fix: thread `Result<...>` through `MultiStreamObsModel::new` and
up to the CLI. One-line change per call site.

**IM4. `-inf` loglik increments accumulate without guard.**
`if2.rs:561`:

```rust
let ll_inc = log_sum_exp(&log_weights) - (n as f64).ln();
if !(config.skip_first_obs_from_loglik && obs_idx == 0) {
    total_loglik += ll_inc;
}
```

If a single `ll_inc` returns `-inf` (e.g., all N particles hit
IC1's BetaBinomial path, or a binomial-constraint violation, or
early-exploration param combinations push particles to
impossible states), `total_loglik` becomes `-inf` for the rest
of the iteration. The best-iteration search at lines 622–625
correctly filters `is_finite()`, so the damage is bounded to
one iteration. Not a correctness bug but a robustness gap: one
bad obs kills N−obs_idx useful data points.

Fix: `if ll_inc.is_finite() { total_loglik += ll_inc } else { n_skipped += 1 }`,
report `n_skipped` in the iteration diagnostic so the user knows
their exploration space hit dead zones.

**IM5. IF2 cooling approximation diverges from pomp semantics
for small n_obs.** `if2.rs:425–483`.

`per_step_cooling = fraction^(2 / (target_iters × n_obs))`
reaches `fraction` at iteration `target/2` when the t=0
perturbation's extra `global_step += 1` is a small fraction of
`n_obs`. For `n_obs = 1`, effective cooling is doubled; for
`n_obs = 10, target = 50` it's ~10% stronger than advertised.

Fix: document the approximation where the formula is defined
("pomp-style; exact fraction-reached-at-midpoint holds for
n_obs ≳ 10"), or account for the +1 per iteration in
`per_step_cooling`.

### Minor

**Im1. `PriorDist::LogNormal` always maps to `TransformedNormal`
regardless of the parameter's transform.** `prior.rs:102`:

```rust
PriorDist::LogNormal(p) => Prior::TransformedNormal { mean: p.mu, sd: p.sigma },
```

Correct iff the parameter's `Transform` is `Log`. If a user
mis-configures and a `log_normal` prior is put on a
`Transform::None` parameter, `log_density` is computed on the
transformed (identity) scale with no Jacobian. The CLI should
reject this combination at fit-config time.

**Im2. `log_sum_exp` early-returns `NEG_INFINITY` on
`max.is_infinite()`.** `types.rs:79–83`. Correct for all-`−inf`
but also catches `+inf` (shouldn't happen; defensive code should
distinguish).

**Im3. `MultiStreamObsModel::new` uses `stream_specs[0].obs_times`
for all streams.** `multi_stream_obs.rs:199`. If streams have
different schedules (weekly + monthly), the second schedule is
silently discarded. Add an assertion that all streams share
`obs_times`, or support heterogeneous schedules.

**Im4. `resampling.rs:44` uses `<` rather than `<=`.** Stylistic
nit — exact boundary equality is measure zero with float
weights, so behavior is identical in practice. Systematic
resampling literature mostly uses `<`; fine.

**Im5. `resets_after_observation` precondition undocumented.**
`multi_stream_obs.rs:51–52` correctly identifies that only
`FlowSum` projections reset, but
`particle_filter.rs:188`'s `state.reset_flows()` resets all flow
accumulators. For a model with multiple disjoint/overlapping
flow-sum streams, the global reset still works. Flag as
"document the invariant" rather than a bug.

**Im6. `if2.rs:388` iteration loop ignores SIGINT.** A
200-iteration × 10k-particle fit can't be Ctrl-C'd; `kill -9`
is the only escape. CLI-level fix (signal handler).

**Im7. `if2.rs:317–327` callback is loglik-only.** The `&dyn Fn(usize, f64)`
callback reports iteration number and loglik, but per-parameter
diagnostics (`clamp_fraction`, etc. already computed in
`ParamIterDiag`) aren't exposed. Progress UIs that want to show
"clamp fraction is 0.5 — your rw_sd is too large" can't. Minor
UX.

### Nits

**In1. `HALF_LN_2PI` constant in `prior.rs:21`** shadowed by a
`PI` import elsewhere — fine.

**In2. `if2.rs:405` seed combination overwrites upper seed bits
when `seed > 2^32`.** XOR with `iter << 32`. Never in practice.

**In3. `multi_stream_obs.rs:70–71` flow-name prefix match**
shares the OCaml M15 prefix-ambiguity concern: `infection`
matches `infection_wild_p1` as a flow-family member. For indexed
transitions with expanded names that's correct; for ambiguous
base names it silently conflates. Worth a regression test.

**In4. `negbin_logpmf(y=0, mu=0, k=0)` returns 0.**
`obs_loglik.rs:139–143`: the `mu == 0` branch triggers before
the `k == 0` check. A `NegBin(μ=0, k=0)` is ill-defined; the
current path effectively treats "no cases expected and none
observed" as log-prob 0, which is defensible for inference
stability but hides the degenerate-parameter case. Worth a test
fixing the contract.

**In5. `weighted_quantiles` linear scan.** `particle_filter.rs:207–241`.
Binary search over cumulative weights would be faster for large
N. Low priority.

## What's left

1. `pgas.rs` (1718 lines) — conditional SMC + ancestor sampling.
   Densest scientific code in the repo. Initial concern noted
   about CSMC-AS construction not obviously respecting the
   conditional-path marginal — to detail in next batch.
2. `pgas_grad.rs` (344).
3. `nuts.rs` (428).
4. `pmmh.rs` (525).
5. `correlated_pf.rs` (406).
6. `diagnostic.rs` (419).
7. CLI drivers (`cli/src/{if2,pfilter,fit/*}.rs`).
