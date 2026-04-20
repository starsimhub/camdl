---
status: batch-1 addressed (pending PGAS + CLI review)
date: 2026-04-19
scope: rust/ sim inference crate — traits, types, linalg, resampling, priors, observation likelihoods (obs_loglik, obs_model, multi_stream_obs), chain_binomial_process adapter, particle_filter, if2
reviewer: external (via `scripts/review-zip.sh engine`, round 5)
---

## Resolution status

**Addressed (behavior fixes):**
- IC1 — BetaBinomial obs likelihood implemented
  (`beta_binomial_logpmf` + sample + mean).
- IC2 — Normal-likelihood count discretization documented in the
  IR spec; runtime emits a once-per-process `log::warn!` on
  fractional observations so silent coercion is visible.
- IM1 — per-particle RNGs now use ChaCha8's stream counter via
  new `StatefulRng::new_stream(seed, stream)`. Four call sites
  in particle_filter / if2 / pgas / correlated_pf.
- IM2 — thread-local scratch `IntState` eliminates
  per-particle/per-stream/per-obs heap allocation in
  `multi_stream_obs.rs`. Two helpers cover the common cases.
- IM3 — `MultiStreamObsModel::new` and
  `resolve_likelihood_from_model` now return `Result<_, SimError>`.
  CLI call sites convert to a clean `exit 1` with diagnostic.
- IM4 — IF2 skips non-finite ll_inc with a per-iteration skip
  counter instead of poisoning `total_loglik`.

**Addressed (docs + guards + tests):**
- IM5 — cooling-approximation gap (small n_obs) documented at
  the formula site.
- Im3 — multi-stream `obs_times` consistency now validated at
  construction (same as IM3 fix).
- Im5 — `reset_flows` semantic invariant documented with a
  canary note for future per-stream-cadence features.
- In4 — NegBin(μ=0, k=0, y=0) = 0 contract test.
- IC1 regression — `beta_binomial_logpmf` known-value test.

**Deferred (not yet addressed):**
- Im1 — reject `PriorDist::LogNormal` with `Transform::None` at
  fit-config time. CLI-level; revisit in the CLI review batch.
- Im2 — `log_sum_exp` +∞ handling. Defensive; low risk.
- Im4 — systematic resampling `<` vs `<=`. Stylistic nit.
- Im6 — SIGINT handling in IF2 loop. CLI-level.
- Im7 — per-parameter progress callback payload. UX.
- In1, In2, In3, In5 — naming/doc/perf nits.

**Still unread:** PGAS (1718 lines), pgas_grad, nuts, pmmh,
correlated_pf, diagnostic, CLI drivers. The PGAS review flagged
a CSMC-AS ancestor-sampling concern that wasn't detailed in this
batch — highest remaining risk surface for the next review.


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

## What's left (as of batch 1)

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

---

# Inference code review — batch 2: NUTS, pgas_grad, correlated PF, PMMH prior path

Covers `nuts.rs` (428 lines, end-to-end), `pgas_grad.rs` (250 of
344 covering density-gradient core), `correlated_pf.rs` (407
lines, CPM random-state machinery + filter loop), and the PMMH
initialization + acceptance code (~360 of 525). Still outstanding
from this side: `diagnostic.rs` (R-hat/ESS), the last third of
`pgas_grad.rs` (gamma-density gradient, currently `dead_code`),
the non-resume PMMH tail, and CLI drivers.

## Findings

### Major

**IM7. `pgas_grad.rs` advances `gamma_idx` once per source group;
`pgas.rs` and `step_one` advance once per overdispersed
transition. Silent wrong NUTS gradient for any model with
multiple overdispersed transitions in one source group.**

Three places agree on the canonical convention:
- `chain_binomial.rs:280` — `step_one` pushes one gamma per
  overdispersed transition with `rate > RATE_EPSILON`.
- `pgas.rs:316` — density increments `gamma_idx` per such transition.
- `pgas.rs:511` — gamma-density loop increments per such transition.

`pgas_grad.rs:110` breaks the pattern:

```rust
for &tr_idx in group {
    // ...
    let g = if gamma_idx < gammas.len() { gammas[gamma_idx] } else { 1.0 };
    // uses same gamma_idx for every transition in this group
}
if group_has_overdispersion { gamma_idx += 1; }  // advances once per group
```

For a source group with two overdispersed transitions (spatial
S→E_local and S→E_import; two-strain S→I₁ and S→I₂; competing
overdispersed reporting streams): the gradient reads `gammas[k]`
for both transitions when the density and simulator used
`gammas[k]` and `gammas[k+1]`, AND every subsequent source
group's `gamma_idx` is off by one. Log-density computed by the
gradient machinery disagrees with `log_transition_density_substep`
and `complete_data_loglik`. NUTS leapfrog takes steps using a
stale/wrong gradient against the actual target density — MH
acceptance rate stays OK because the correction is
self-consistent, but the sampler doesn't explore the correct
geometry and converges slower / to a biased region.

Not triggered by: He et al. measles SEIR, standard
SIR/SEIR/SIRS/SIRD with single-outflow overdispersion.
Triggered by: multi-strain models with per-strain overdispersion;
spatial models with overdispersed local+import infection (the
polio cVDPV2 work); competing overdispersed reporting streams
sharing a source.

Fix: mirror `pgas.rs:311–317` exactly — inside the
`Overdispersed` arm, increment `gamma_idx += 1` per transition;
delete the `group_has_overdispersion` bookkeeping. Regression
test: two-overdispersed-transition model, finite-difference vs
`complete_data_loglik_grad` at ~1e-6.

**IM8. `correlated_pf.rs` CPM only handles one overdispersed
transition per substep, and uses the first transition's σ² for
all subsequent ones.** `correlated_pf.rs:283–289`:

```rust
let noise_idx = i * steps_per_obs + substep;
if noise_idx < gamma_row.len() {
    let z = gamma_row[noise_idx];
    let g = normal_to_gamma(z, gamma_shape, gamma_scale);
    scratch.gamma_override = Some(g);
}
```

`scratch.gamma_override` is a single `Option<f64>` that
`step_one` consumes for the first overdispersed transition only
(`chain_binomial.rs:278` calls `.take()`). Subsequent
overdispersed transitions in the same substep fall through to
`rng.gamma_multiplier(...)` — uncorrelated.

Compounding issue: `sigma_sq` at `correlated_pf.rs:248–260`
picks the **first** overdispersed transition's `sigma_sq` via
`find_map(...)` and uses it for every gamma draw in the run.
If different overdispersed transitions have different σ²
(infection overdispersion ≠ reporting overdispersion, the normal
case), CPM transforms normals using the wrong shape/scale for
all but the first.

For single-overdispersion models (common case) both issues are
moot. For multi-overdispersion models, PMMH with
`rho = Some(0.99)` produces uncorrelated or mis-transformed
gamma draws and loses the CPM variance reduction — acceptance
rate drops to vanilla PMMH levels with no error.

Fix: either promote `gamma_override` to `Vec<f64>` with a
matching pop/index, or reject multi-overdispersion models at the
CPM preflight (smaller patch, matches existing pattern at
`correlated_pf.rs:237–247`).

**IM9. `pgas_grad.rs:79` rate-threshold inconsistency with
density and simulator.** Line 79:

```rust
if rate <= 0.0 || matches!(...::Deterministic) { handled[tr_idx] = true; continue; }
```

Density (`pgas.rs:284`) and simulator (`chain_binomial.rs:270`)
both use `rate <= RATE_EPSILON`. Density additionally has a
branch (`pgas.rs:294–301`) for `0 < rate ≤ RATE_EPSILON` with
nonzero flow — includes the transition in the multinomial with
its tiny rate rather than skipping. The gradient unconditionally
skips.

For a near-zero-rate transition with nonzero flow, the density
is finite and depends on the rate through `total_rate` /
`p_total`; the gradient computes zero. Hamiltonian is not
conserved (density shifts without gradient shift), NUTS
divergences accumulate, step size adapts down. Shows up as
nonspecific divergences in `pgas.rs:1635–1643`.

Fix: align threshold and "near-zero with flow" handling to match
`pgas.rs:284–304` exactly. Same regression test as IM7.

### Minor

**Im14. `nuts.rs:281` acceptance detection via byte-equal Vec.**
`let accepted = z_proposal != current_z;` uses bit-pattern Vec
equality. A multinomial move that coincidentally returns the
same float vector is flagged "rejected." Vanishingly unlikely;
worth a comment.

**Im15. `correlated_pf.rs:346–348` dead allocation.**

```rust
let _resample_rng = StatefulRng::new(
    seed.wrapping_add(0xdeadbeef).wrapping_add(obs_idx as u64)
);
```

`_`-prefixed, never read; `sorted_systematic_resample` (line 350)
uses `base_uniform` from correlated noise and no RNG. Delete.

**Im16. `correlated_pf.rs:329–335` sort-by-Σ-flows heuristic is
fragile with mixed streams.** Deligiannidis-Doucet-Pitt
sort-by-scalar-projection is a general trick but projection
choice matters. For incidence-only streams, reasonable; for
mixed prevalence/incidence with different scales, noisy. Document
rather than fix.

**Im17. `pgas_grad.rs:117` boundary clamping gives finite-but-huge
gradient at `p_total ∈ {0, 1}`.** The clamp to `[1e-15, 1-1e-15]`
at line 115 ensures `dbinom_dp` is never NaN — it's ~±1e15.
NUTS divergences at the boundary are expected. Worth a comment:
"clamp ensures finite gradient at the cost of accuracy at
boundary; divergences here are normal."

**Im18. `pgas.rs:1303–1305` resume state only updates cold rung.**
Same as the previously-flagged Im13 — heated rungs re-warmup on
every resume. Either persist all rungs or make cold-rung-only
opt-in.

### Validated as correct

- **NUTS mass-matrix algebra** (`nuts.rs`). Diagonal stores
  `Σ_ii` (named `m_inv`, i.e. per-component inverse-mass =
  variance). Dense stores `Cholesky(Σ)`. Momentum draw
  `p = L⁻ᵀ z` gives `Cov(p) = M`. Kinetic `0.5 ‖Lᵀp‖²` equals
  `0.5 pᵀ Σ p`. `m_inv_times` correctly returns `Σp = L Lᵀ p`.
  The `m_inv` naming is confusing but the algebra is consistent.
- **NUTS slice sampling.** `log_slice = −h₀ − Exp(1)` is the
  correct log-uniform (Hoffman & Gelman Algorithm 3). Multinomial
  `n'/(n_valid + n')` is H&G-original — not Betancourt's improved
  variant, but mathematically valid.
- **NUTS dual averaging.** Matches Nesterov:
  `log_eps = µ − h̄·√m/γ`, polynomially-decaying averaging,
  `.final_step_size()`. Defaults `γ=0.05, t₀=10, κ=0.75,
  µ=log(10·initial)` are H&G's.
- **NUTS U-turn.** `(z⁺ − z⁻) · M⁻¹ p_endpoint < 0` for either
  endpoint — correct criterion.
- **PMMH correlated-PF validation.** Preflight rejects
  state-dependent σ² and non-uniform observation spacing. Both
  are the right pattern to emulate for IM8's multi-overdispersion
  case.
- **PMMH accumulation.** Prior + Jacobian additions in the MH
  ratio (`pmmh.rs:419–420`) are the standard transform-scale MH
  form. The only issue is the underlying
  `Prior::TransformedNormal::log_density` semantics — same double-
  Jacobian as IC3 from an earlier batch.

### Notes propagating from earlier batches

- The IC3 double-Jacobian on `log_normal` priors also lives in
  PMMH at `pmmh.rs:419–420`. Verified: `current_log_prior` at
  334–337 uses `prior.log_density(natural, z)` (z-scale density
  for `TransformedNormal`, already absorbing the Jacobian);
  `current_log_jacobian` at 356–359 adds `log_jacobian(z)` on top.
  Same fix, same three callers.
- `pmmh.rs:340` tracks `map_log_posterior = current_ll +
  current_log_prior` without the Jacobian. For Normal/Beta/Gamma
  priors this correctly tracks `LL + log p(θ)` on the natural
  scale. For `TransformedNormal`, `current_log_prior` is the
  z-scale density, so the tracked "MAP posterior" is a different
  quantity than for the other priors — non-comparable MAPs across
  parameters using different priors, in the same fit. Fix
  alongside IC3.

## Batch 2 resolution status

**Addressed:**
- IM7 — `pgas_grad.rs` now advances `gamma_idx` per overdispersed
  transition (mirrors `pgas.rs`), drops the
  `group_has_overdispersion` flag.
- IM9 — `pgas_grad.rs` rate threshold aligned to `RATE_EPSILON`
  via `pub const` from `chain_binomial`.
- IM8 — `correlated_pf.rs` preflight rejects multi-overdispersion
  per source group AND distinct σ² values across overdispersed
  transitions. Matches the existing state-dependent-σ² preflight
  pattern.
- Im14 — comment added at `nuts.rs:281` on Vec-equality semantics.
- Im15 — dead `_resample_rng` in `correlated_pf.rs` deleted.
- Im17 — comment at `pgas_grad.rs:131` documenting the clamp
  behavior at `p_total ∈ {0, 1}`.

**Deferred:**
- Im16 — `correlated_pf.rs` sort-by-Σ-flows projection note;
  document rather than fix.
- Im18 — `pgas.rs` resume-only-cold-rung; revisit with the
  tempering resume refactor.
- IC3 double-Jacobian (`log_normal` priors) + `map_log_posterior`
  non-comparability — cross-cutting fix spanning `prior.rs`,
  `pmmh.rs`, `pgas.rs`; track as its own work item.

## What's left after batch 2

- `diagnostic.rs` (419 lines) — R-hat, ESS, multi-chain handling.
- Last 90 lines of `pgas_grad.rs` — `log_gamma_density_grad_substep`
  is marked `dead_code` (gamma density is routed through
  `complete_data_loglik` instead). Quick skim for future
  re-enablement.
- PMMH lines ~360–525 — adaptive proposal covariance, non-correlated
  path, MAP updates, resume tail.
- CLI drivers: `cli/src/if2.rs`, `cli/src/pfilter.rs`,
  `cli/src/sampling.rs`, `cli/src/fit/{state,refine,mod,status}.rs`.
  Especially the Prior↔Transform binding (Im1 from batch 1).

**Cross-cutting pattern:** PGAS, PMMH, `pgas_grad`, and
`correlated_pf` all share conventions with chain-binomial's
`step_one` that have to be hand-maintained; three of four have
drifted slightly. A dedicated "audit the gamma_idx convention
across all four" sweep after IM7 would be worth it.

---

# Inference code review — batch 3: diagnostics + CLI wiring

Covers `diagnostic.rs` (full), `runner.rs` R-hat/ESS +
`derive_transform` + `build_if2_params_from_specs` +
`resolve_prior` + `compute_config_hash`, `pfilter.rs` (full), and
slices of `fit/mod.rs`, `fit/config.rs`, `fit/pgas.rs`.
Outstanding after this batch: tail of `fit/pmmh.rs`, `cli/if2.rs`
specifics (mostly mechanical), `batch.rs` (orchestration, not
correctness), `util.rs`, tests. Inference-algorithm code is now
covered end-to-end.

## Findings

### Critical

**IC4. Prior ↔ Transform incompatibility is silently accepted.**
`derive_transform` (`runner.rs:474–495`) picks the transform from
the IR's `param_kind` (or bounds fallback) without ever
consulting the prior. `validate_bounds` (`fit/config.rs:298–315`)
checks only that fit bounds sit within model bounds — no
prior/transform compatibility check. Silent failure modes:

1. **`log_normal` prior on `Transform::None` parameter**
   (`param_kind = "real"`, or negative lower bound):
   `Prior::TransformedNormal::log_density(natural, z)` is called
   with `z == natural` (identity transform). The Normal density
   evaluates at the natural-scale value —
   `log_normal(mu=0, sigma=1)` silently becomes
   `Normal(mean=0, sd=1)`. No warning.
2. **`log_normal` prior on `Transform::Logit` parameter**
   (probability parameter with mis-specified prior): same silent
   coercion — `z = logit(natural)`, density evaluated as
   `log N(logit(θ); mu, sigma)`, i.e. logit-normal not log-normal.
3. **`half_normal` prior on `Transform::Log`**: correct by
   construction but nothing validates that HalfNormal's support
   `[0, ∞)` matches the user's bounds.

Combined with IC3 (double-Jacobian), the subset of prior ×
transform combinations that actually do what the user intends is
far smaller than what the CLI accepts. `fit/pgas.rs:96–99`
reports `LogNormal(mu=X, sigma=Y) → median=exp(X)` — the
*intended* median — while the sampler targets a different
quantity after double-Jacobian. CLI reports one prior, sampler
uses another.

Fix: add `validate_prior_transform_compat` and call from every
fit-stage entry point. Enforce:
- `Prior::TransformedNormal` (log_normal) requires `Transform::Log`.
- `Prior::Beta` requires `Transform::Logit` and bounds `[0, 1]`.
- `Prior::HalfNormal`/`Gamma`/`Exponential` require `Transform::Log`.
- `Prior::Uniform`/`Normal` compatible with any transform.
Reject, don't warn.

### Major

**IM10. `compute_config_hash` omits `starts_from` and the
data-loading stream-to-file mapping.** `runner.rs:1869–1911`.
Hash covers: model IR JSON, data file bytes, estimate specs,
fixed values, n_particles, dt, runtime version. It does NOT
cover:

- `fit.fit.starts_from` — previous stage's path. Running PGAS
  with resume after `starts_from = "scout_v1"` vs `"scout_v2"`
  hashes identically. Resume reuses stale state from a wrong
  starting point; the `config hash mismatch` check at
  `fit/pgas.rs:134–135` passes but the chain continues from the
  wrong initial point.
- Stream → file mapping. Hash includes file bytes but not the
  mapping from stream name to file. Renaming a stream in the IR
  while file contents are identical hashes the same but models a
  different problem.

Fix: include `fit.fit.starts_from` (as path or transitively via
its own hash) and normalize the stream→file mapping into the
hash.

**IM11. ESS estimator uses a simplified Geyer rule that
overestimates ESS for chains with oscillating autocorrelation.**
`pmmh.rs:503–525`. The "initial positive sequence" truncation at
line 520 stops on the first negative *single-lag*
autocorrelation. Geyer (1992) stops on the first negative *pair
sum* (ρ_{2k} + ρ_{2k+1}) — strictly more conservative. Stan,
PyMC, BDA3 all use pair-sum. For chains with non-monotonic
autocorrelation (NUTS during tuning, PMMH near a mode boundary),
single-lag overestimates ESS by 2–5×. The user's reported ESS is
higher than actual effective sample size; posterior-quantile
uncertainty calibration is off.

Fix: pair-sum truncation (~8 lines), or FFT autocorrelation +
pair-sum in one function.

**IM12. `compute_rhat_ess` reports total ESS across chains
regardless of R-hat convergence.** `runner.rs:829–857`. If R-hat
is 3.0 (chains haven't converged), per-chain ESS estimates are
meaningful for each chain's own stationary distribution, but the
sum is meaningless — chains sample different distributions.
Returned unconditionally; displayed as "total ESS" in reports,
making a non-converged run look adequately-sampled.

Fix: return `NaN` (or `None`) for `total_ess` when R-hat exceeds
threshold.

**IM13. `compute_rhat` uses 1992 Gelman-Rubin without
split-chains or rank-normalization.** `runner.rs:782–823`. For
IF2 each "chain" is a per-iteration parameter-means trajectory,
not a posterior sample — classic R-hat is conservatively OK there.
But the code uses the name `Rhat` without qualification and users
from Stan/PyMC expect Vehtari et al. 2021. Either rename to
`gelman_rubin_1992` or implement split-chains + rank-
normalization.

### Minor

**Im19. Default seed is 20-bit.** `fit/mod.rs:1700`:
`dur.as_nanos() % 1_000_000`. Birthday bound: ~1000 parallel
runs → ≈50% collision. Fix: drop the modulo, use full u64.

**Im20. Replicate seeding is additive.** `pfilter.rs:221`:
`seed + rep as u64`. Same weak-KDF pattern as IM1 from batch 1.
Fix: multiplicative with golden-ratio constant, or use
`StatefulRng::new_stream(seed, rep as u64)`.

**Im21. `DiagnosticCollector::push` locks a Mutex per call.**
`diagnostic.rs:330–341`. Low-frequency so fine; note only.

**Im22. `pfilter` CLI is single-observation-only.**
`pfilter.rs:149–158`. Runtime supports multi-stream; CLI doesn't.
Documentation concern.

**Im23. `diagnostic.rs` has a hand-rolled date formatter.**
Lines 390–419 implement Hinnant's civil_from_days. Correct; add
a link comment for future readers.

**Im24. `compute_rhat_ess` doesn't check all chains have the
same length.** `runner.rs:835`. Uses `chains[0].len()` in the
between-chain variance formula; if chains have different lengths
(one resumed from longer run), the formula is wrong.
Fix: assert equal lengths or use per-chain lengths.

**Im25. `compute_rhat` skip amount is identical across chains
but absolute iteration indices differ for resumed chains.**
`runner.rs:796`. Same class as Im24.

### Notes on existing good patterns

- `DiagnosticKind` is well-designed: typed variants, stable
  serialization identifiers, severity-in-type, hints attached to
  the kind.
- `CompressedLogitPosition` diagnostic catches real user
  mistakes.
- `derive_transform` + `ParamSpec` + `build_if2_params_from_specs`
  is a clean three-layer decomposition; the comment at
  `runner.rs:505-509` attributes it to "three bugs in one session"
  — visibly lessons-learned.
- `compute_config_hash` including the runtime version is the
  right call — code changes affecting inference semantics
  invalidate cached state.

### Cross-cutting consolidated critical bugs list

With inference review now substantively complete:

**OCaml compiler** (addressed in earlier batches):
- C1, C2, C5, C6, C8 — all fixed.

**Rust runtime** (addressed):
- RC3 (`ir::validate`) wired.
- RM1 (tau_leap competing-risks) ported.

**Inference:**
- IC1 (BetaBinomial -inf) — **addressed batch 1**.
- IC2 (Normal discretized) — **addressed batch 1** (doc + warn).
- IC3 (`log_normal` double-Jacobian) — **open**. Cross-cutting
  across `prior.rs`, `pmmh.rs`, `pgas.rs`, `fit/pgas.rs`.
- IC4 (prior × transform mismatch) — **open**. New in this
  batch. Combined with IC3: log_normal priors are the highest-
  risk Bayesian workflow in the codebase.

Plus majors: IM6 (CSMC-AS — open, pending), IM7/IM8/IM9
(addressed batch 2), IM10/IM11/IM12/IM13 (this batch — open).

IC3 and IC4 together are the highest-priority unblock: they
contaminate every Bayesian inference result using a log_normal
prior. Bayesian modelers reach for log_normal rate priors
reflexively. Fix IC3 (natural-scale `log_density` for
TransformedNormal + Jacobian once), add IC4's validator,
regenerate any PMMH/PGAS posterior that used log_normal priors.
The He et al. measles benchmark wouldn't have caught IC3 because
pomp's published MLE is tested against IF2 (no priors), not
posterior means from PMMH/PGAS.

## Batch 3 resolution status

**Addressed:**
- **IC3** — `TransformedNormal::log_density` now returns natural-
  scale density. Caller-added `log_jacobian(z) = z` recovers the
  correct z-scale density without double-counting. Two regression
  tests (natural-scale integrates to 1; natural + jacobian = z-
  scale normal). Posterior-mean bias for log_normal priors gone.
  Any PMMH/PGAS posterior over log_normal-priored params should be
  regenerated.
- **IC4** — `validate_prior_transform_compat` in `runner.rs`,
  called from the v2 fit entry point after config validation and
  sweep-value substitution. Rejects incompatible combinations
  (log_normal × not-Log, beta × not-Logit, half_normal × not-Log,
  gamma × not-Log, exponential × not-Log). Error names the
  parameter, prior, transform, prior source.
- **IM10** — legacy `compute_config_hash` now hashes
  `pgas.starts_from` and `pmmh.proposal_from`. v2 already covered
  these via serde_json on the full Stage struct.
- **IM11** — `mcmc_ess` now uses Geyer pair-sum truncation
  (ρ_{2k−1} + ρ_{2k} ≤ 0) instead of single-lag negative. 2–5×
  more conservative on non-monotonic chains.
- **IM12** — `compute_rhat_ess` returns `NaN` for ESS when R-hat
  > 1.1 (BDA3 threshold), so non-converged runs don't look
  adequately-sampled in reports.
- **IM13** — `compute_rhat` documented as 1992 G-R formula (no
  split-chains, no rank-norm). Exported alias
  `gelman_rubin_1992` for explicit naming.
- **Im19** — default seed uses full u64 nanoseconds (was 20 bits).
- **Im20** — pfilter replicate seeding uses golden-ratio stride
  instead of `+ rep`.
- **Im24** — `compute_rhat_ess` asserts equal chain lengths;
  returns `(NaN, NaN)` when violated.

**Deferred:**
- Im21 — Mutex per push in DiagnosticCollector (low-frequency).

**Addressed in follow-up pass (2026-04-20):**
- IM6 — CSMC-AS ancestor-sampling weight drops stale `log_weights[j]`
  term (post-resample slots carry uniform weight). PGAS posteriors
  over models with >1 observation previously skewed toward
  high-pre-resample-weight slots regardless of whether the
  ancestor-source state was a good precursor; now the ancestor
  categorical is driven by transition density alone.
- Im18 — heated-rung re-warmup on resume documented + logged.
- Im22/Im23 — pfilter single-stream doc comment + Hinnant
  algorithms link comment.
- Im25 — `compute_rhat` now uses each chain's own last-half window;
  G-R formula uses the shortest chain's tail length.
- Im2 — `log_sum_exp` distinguishes +∞ from −∞.
