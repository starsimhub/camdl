Good to know on EKRNG — that downgrades RC2 substantially. What I'll land on is: it's a docs/comments cleanup (delete the "ekrng.rs is available if needed" comment in gillespie.rs:78 and the "different derivation than EkRng" comment in rng.rs:13, remove the IR spec section 10, audit CLAUDE.md). The scenario-coupling mechanism stays what it is: shared stateful seed, fragile to RNG-order changes but fine for the reference tests where both legs are bitwise-equal until the intervention fires. The rest stands. Let me pull up the inference code.Also noticed: the zip has `cli/src/if2.rs`, `cli/src/pfilter.rs`, `cli/src/fit/` — CLI driver code for the inference paths. Worth a glance at `sampling.rs` too. Let me plan the read order by scientific risk, cheapest-to-highest:

1. **Foundations**: `traits.rs` (174), `types.rs` (83), `mod.rs`, `linalg.rs` (32), `resampling.rs` (98), `prior.rs` (218). ~600 lines. Tells me how params/state flow.
2. **Particle filter core**: `particle_filter.rs` (241), `chain_binomial_process.rs` (90), `obs_loglik.rs` (444), `obs_model.rs` (242), `multi_stream_obs.rs` (349). ~1400 lines. The bootstrap PF is the basis for everything.
3. **Backends that build on PF**: `correlated_pf.rs` (406), `pmmh.rs` (525), `if2.rs` (634). ~1550 lines.
4. **PGAS (the monster)**: `pgas.rs` (1718), `pgas_grad.rs` (344), `nuts.rs` (428). ~2500 lines. Deepest risk for silent wrong posteriors.
5. **Diagnostic + CLI drivers if time permits**: `diagnostic.rs` (419), `cli/src/{if2,pfilter,fit/*}.rs`.A few concerns already:

1. `particle_filter.rs:137` — `while t < obs_time - 1e-10 { t += dt.min(obs_time - t); }` — this recomputes the same advancement the particles did, purely for updating the global `t`. OK as bookkeeping, but fragile: if any particle's advancement path differs from this (e.g., a particle hits an error and returns early — wait, `errors.collect()` fails the whole call), `t` still advances. Minor.

2. Line 84: `seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95)` — XOR with a constant-multiplied index. For particle indices 0..N, this produces *very* correlated seeds. Two particles with indices that share many low-bits of the multiplication will have highly correlated early state. ChaCha8 "washes out" this correlation but it's still sketchy. A proper per-particle stream would use a counter-based PRNG or a cryptographic key derivation, but for ChaCha8 at least you'd want `ChaCha8Rng::seed_from_u64(seed).then(set_stream(i))` or similar. Worth checking statistical tests later.

3. Line 173: `skip_first_obs_from_loglik` — the IC-free inference path. Comment says the reweight still happens, just not accumulated into total_loglik. Correct per the linked design doc. Fine.

4. Line 82–84: `rngs` is initialized deterministically from seed. Good for reproducibility. But line 87–91 creates `diag_rngs` with a different constant — **both ChaCha8Rng instances are initialized from the XOR-with-small-hash of seed**, which is extraordinarily weak key derivation. `seed.wrapping_add(0xdeadbeef)` on line 116 for the resample RNG is the same. These are three independent streams seeded by 16-byte-different rng states. ChaCha8 will probably make them uncorrelated in practice but it's not the right pattern.

Let me keep reading.Big finding here already: **`Normal` observation likelihood uses `discretized_normal_logpmf_tol`** (line 95) — it treats continuous Normal as discretized on integer counts. That's pomp-style, intentional if the data is case counts. But if a user writes `likelihood = normal(mean = projected, sd = sigma)` for a genuinely continuous observable (e.g., log-transformed viral load), they get a discretized integer PMF instead of a continuous Normal PDF. No documentation I've seen flags this.

Also: `ResolvedLikelihood::BetaBinomial` returns `-inf` at line 110 and warns. The OCaml expander fully supports emitting `BetaBinomialLikelihood` with n/alpha/beta fields — and serde round-trips it. So a user who writes `beta_binomial(...)` in their DSL will compile to IR cleanly, load into the Rust runtime cleanly, then silently get -inf likelihood for every particle. All particles collapse to uniform weights, the filter runs but gives meaningless estimates. This is a critical silent-wrong-answer bug.

Let me keep going.A clear perf issue: `log_likelihood` / `sample` / `mean` in `MultiStreamObsModel` all heap-allocate `IntState::new(self.n_int)` per stream per call (lines 290, 311, 327). Plus `project_stream_with_params` allocates another `IntState::new` per `Expr` projection per call (line 233). These are called per-particle per-observation in the PF inner loop. For 10k particles × 1000 obs × 3 streams, that's 30M+ allocations that the comment on line 152 claims don't happen.## Summary — inference foundations + IF2

This batch read ~2100 lines: the traits layer, types, linalg, resampling, priors, observation log-likelihoods, multi-stream observation model, chain-binomial process adapter, and the full IF2 implementation. Still ahead: `correlated_pf.rs`, `pmmh.rs`, `pgas.rs` (the 1718-line monster), `pgas_grad.rs`, `nuts.rs`, `diagnostic.rs`, and the CLI drivers (`if2.rs`, `pfilter.rs`, `fit/*`). I'll write these up as batch 5a so we can keep the critical ones at the top.

## Findings

### Critical

**IC1. `BetaBinomial` observation likelihood silently returns `-inf` for every observation.** `obs_model.rs:108–111`:
```rust
ResolvedLikelihood::BetaBinomial { .. } => {
    log::warn!("BetaBinomial obs_loglik not implemented — returning -inf");
    f64::NEG_INFINITY
}
```
The `sample()` path does the same thing returning 0 at `obs_model.rs:193–195`. The OCaml expander fully supports emitting `BetaBinomialLikelihood { n, alpha, beta }` (I covered this in round 2: `expander.ml:2214–2219` builds this variant). The IR round-trips it. The Rust `resolve_likelihood` resolves its three subexpressions without complaint. Then at eval time every particle gets `-inf` log-weight. `log_sum_exp` in `types.rs:79–83` hits the `max.is_infinite() → NEG_INFINITY` early return, the likelihood increment becomes `-inf`, and per `particle_filter.rs:172–175` `total_loglik += -inf`. Every IF2 iteration then reports `-inf`. The user's fit fails loudly with a log message, not silently — but the warning is `log::warn!` which may or may not be visible depending on env, and it fires every observation (so at worst it's drowned in its own repetition). A CLI user with `RUST_LOG` unset sees no warnings at all and just a `-inf` fit.

`Binomial` is implemented (lines 101–107) and `Bernoulli` is implemented (lines 112–115). So three of the six likelihood types work and one is a `-inf` landmine. Either implement BetaBinomial log-pmf (straightforward: `lgamma(n+1) - lgamma(k+1) - lgamma(n-k+1) + log B(k+α, n-k+β) - log B(α, β)` using the existing `lgamma`), or reject the likelihood in `resolve_likelihood` at model-construction time with a hard error ("BetaBinomial observation is not yet implemented"). Silent `-inf` is strictly worse than both.

**IC2. `Normal` observation likelihood uses a discretized integer-count PMF for what the DSL presents as a continuous distribution.** `obs_model.rs:92–96`:
```rust
ResolvedLikelihood::Normal { mean, sd } => {
    let m = eval_resolved(mean, &ctx(projected));
    let s = eval_resolved(sd, &ctx(projected));
    discretized_normal_logpmf_tol(observed, m, s * s, DEFAULT_TOL)
}
```
`discretized_normal_logpmf` (`obs_loglik.rs:199`) rounds the observation to an integer with a continuity correction — correct for case counts, wrong for genuinely continuous observables. A user who writes `likelihood = normal(mean = projected, sd = sigma)` intending to model log-transformed viral load, antibody titer, or any real-valued quantity gets their data silently coerced to nonneg integers before scoring. The log-Jacobian is also missing (it would be 0 for identity, but the discretization introduces its own). This is a semantic mismatch between the DSL and the runtime. Either document the discretization prominently (the IR comment at lines 181–194 explains the He et al. heteroscedastic motivation, but the language spec surface `normal(...)` gives no hint), or add a separate `likelihood = continuous_normal(...)` variant, or rename the current one `normal_count(...)`.

Paired with the OCaml-side m19 finding (normal prior uses `mu`/`sigma`, normal observation uses `mean`/`sd`) — the Normal likelihood is the most confusing surface in the model.

### Major

**IM1. Per-particle RNG seeding is fragile.** `particle_filter.rs:84`:
```rust
StatefulRng::new(seed ^ (i as u64).wrapping_mul(0x517cc1b727220a95))
```
XOR of seed with a multiplied index is OK-ish but the multiplier is a single 64-bit constant. Particles whose indices differ by predictable amounts (e.g. `i` and `i + 2^k`) will have highly correlated XOR results in low bits. ChaCha8's initial state expansion (see `rng.rs:109–121`'s `expand_u64_to_seed` — it only mixes the u64 four times via single multipliers) propagates this correlation into the first few output blocks before the cipher's rounds fully mix the state. In practice, for moderate N this probably tests statistically clean, but there's a reason proper per-stream seeding uses cryptographic key derivation or the RNG's own stream-rekey (ChaCha has a stream counter that's the documented way to do this). Same pattern appears in `if2.rs:405`:
```rust
StatefulRng::new(seed ^ ((iter as u64) << 32) ^ (i as u64).wrapping_mul(0x517cc1b727220a95))
```
and `diag_rngs`, `resample_rng` elsewhere. If anyone ever uses N in the tens of thousands and finds "weird correlations in particle diversity," this is the first place to audit. Fix: use `ChaCha8Rng::from_seed` then call `.set_stream(i as u64)` on the inner, or use a proper KDF.

**IM2. Per-particle heap allocation in multi-stream obs hot path.** `multi_stream_obs.rs:290, 311, 327` and the per-stream `project_stream_with_params`:
```rust
// In log_likelihood:
let int_s = IntState::new(self.n_int);                          // per-stream, per-particle
// In project_stream_with_params's Expr arm:
let mut scratch = IntState::new(self.n_int);                    // per-stream, per-particle, per-obs
scratch.counts.copy_from_slice(counts);
```
`IntState::new(n)` allocates a `Vec<i64>` of `n` zeros. In a PF inner loop with N particles × T obs × S streams, this is N·T·S heap allocs per full filter pass. IF2 multiplies that by iterations (~50–100) and scout by chains (~64). For a nominal `10⁴ × 10³ × 3` pass, that's 3·10⁷ allocs per PF; IF2 with 100 iterations hits 3·10⁹. The comment at `multi_stream_obs.rs:150–153` specifically claims "Allocation happens at observation ticks only — not in the propensity hot loop" — but that's false in the read code. The `int_s` is used by `eval_likelihood_resolved` only if the likelihood expression references `Pop(...)` or similar, which almost none do (they typically just reference `projected` and params). So the allocation is done and largely unread.

Fix: keep a pre-allocated `IntState` inside `Stream` (or one shared in `MultiStreamObsModel` behind a thread-local / scratch buffer since PF/IF2 parallelize with rayon). Even just storing one mut `IntState` field and `clear`ing it per call saves the allocation. Given this is claimed to be a hot path, the fix is small and the impact may be measurable in benchmarks.

**IM3. `resolve_likelihood_from_model` panics on failure instead of propagating.** `obs_model.rs:68–69`:
```rust
resolve_likelihood(likelihood, &ctx)
    .expect("observation likelihood resolution failed — this is a model construction bug")
```
This is called from `MultiStreamObsModel::new` at lines 192–195. If any stream's likelihood expression references an unknown parameter or compartment, the process panics instead of returning a user-facing error. The OCaml compiler should have caught this, but per the earlier rounds, the OCaml silent-fallback pattern means a typo in a stratum index can produce an IR that references a nonexistent parameter (`C2` in round 2). Rust then panics rather than emitting a `SimError::UnknownParameter`. Fix: thread `Result<...>` through `MultiStreamObsModel::new` and up to the CLI. One-line change per call site.

**IM4. `if2.rs:561` log-likelihood accumulation uses the naive `total_loglik += ll_inc` which has no numerical guards.** If a single `ll_inc` returns `-inf` (which happens when all N particles get `f64::NEG_INFINITY` log-weight — e.g., when the binomial constraint is violated, or when BetaBinomial hits IC1), `total_loglik` becomes `-inf` for the rest of the run. The next iteration's best-iteration search at lines 622–625 uses `.filter(|it| it.if2_perturbed_loglik.is_finite())` to skip these — good. But no per-iteration recovery: if the first observation step hits `-inf` (which is easy during early exploration when perturbed params push a particle to an impossible state), the entire iteration's loglik is `-inf` and unusable. For IF2 this is probably fine because iterations are cheap; for scout with many chains, one bad chain can kill an iteration count. Small gain: `if ll_inc.is_finite() { total_loglik += ll_inc }` with a counter of skipped observations, reported at end. Not a correctness bug, a robustness one.

**IM5. IF2 parameter perturbation at `t=0` is global-step-indexed, but the "at t=0" perturbation (`if2.rs:425–451`) uses `global_step` that's shared across iterations.** Lines 426 and 451:
```rust
let cooling_now = per_step_cooling.powf(global_step as f64);
// ...
global_step += 1;
```
Then at obs-time (line 483) again `per_step_cooling.powf(global_step as f64)`. Since `global_step` increments once at t=0 and once per obs within an iteration, and carries across iterations, cooling accumulates monotonically — this is intended (cooling.fraction.50 semantics). OK. But: the comment at lines 360–368 promises `c = cooling_fraction ^ (2 / (target_iters × n_obs))` and `"After m iterations × n_obs steps each: effective_sd = rw_sd × c^(m * n_obs)"`. Let me check: per iteration, `global_step` increments 1 + n_obs times (once at t=0, once per obs). After m iterations, `global_step = m · (1 + n_obs)`. But the formula computes `per_step_cooling = fraction^(2 / (target × n_obs))` — so after m iterations the effective scale is `fraction^(2 · m · (1 + n_obs) / (target · n_obs))`. For n_obs large that's ≈ `fraction^(2m/target)` — reaches `fraction` at m = target/2, matching pomp's "cooling.fraction.50 at midpoint" promise. Good, the math works out.

But if n_obs is small (say 10) and target=50, the `+1` on the t=0 perturbation matters: `fraction^(2m·11/500) = fraction^(0.044 m)` vs the naive `fraction^(0.04 m)` — 10% more cooling than advertised. For n_obs = 1 it's doubled. Not a bug, an approximation. Worth documenting: "pomp-style cooling; exact fraction reached at midpoint holds for n_obs ≳ 10."

### Minor

**Im1. `prior.rs:102` `PriorDist::LogNormal` maps to `Prior::TransformedNormal`**, assuming the parameter uses a log transform for inference. This is correct *iff* the parameter's `Transform` is `Log`. If someone mis-configures and a log_normal prior is put on a `Transform::None` parameter, `log_density` is computed on the transformed (i.e. identity) scale with no Jacobian — mathematically, this corresponds to a "log-normal prior evaluated as a normal on the natural scale," which is wrong. The CLI should reject this combination at fit-config time. I haven't checked whether it does; flagging for the CLI batch.

**Im2. `types.rs:79–83` `log_sum_exp` early-returns `NEG_INFINITY` when `max.is_infinite()`.** Correct for the all-`-inf` case but also returns `-inf` when `max = +inf` (one log-weight is `+inf`, which shouldn't happen but defensive code should distinguish). Low risk.

**Im3. `multi_stream_obs.rs:199`** in `new`, `stream_specs[0].obs_times.clone()` — only the first stream's obs_times are used, silently ignoring any differences with other streams. Comment doesn't document "all streams must share obs_times" as a precondition. If a user's streams have different schedules (e.g., weekly + monthly), this silently discards the second schedule. Add an assertion comparing all streams' `obs_times`, or support heterogeneous schedules properly.

**Im4. `resampling.rs:44` `while j < n - 1 && cumsum + weights[j] < threshold`**: uses `< threshold` not `<= threshold`. For exact equality (proper weights summing to exactly 1, threshold landing exactly on a boundary), this advances past the particle when it shouldn't. With floating-point weights this is vanishingly unlikely, but systematic resampling literature typically uses the `≤` convention. Noting.

**Im5. `multi_stream_obs.rs:51–52` `resets_after_observation`** correctly identifies that only `FlowSum` projections reset between observations. But I don't see the `log_likelihood` method or its callers in `particle_filter.rs` actually doing per-stream selective reset — line 188 calls `state.reset_flows()` which resets *all* flow accumulators, not just those referenced by flow-sum streams. For a model with both incidence and prevalence streams, this is fine (prevalence doesn't use flows). For a model with multiple incidence streams tracking different flow subsets, this is still fine because all flows get reset together and the subsets are disjoint or overlapping — both work. Flagging as "verify semantics documented" rather than a bug.

**Im6. `if2.rs:388` — the iteration loop does not honor any interrupt.** For a 200-iteration fit with 10k particles, there's no way to Ctrl-C cleanly; the user has to kill -9. Long-running inference benefits from a signal handler. CLI concern, not inference-algorithm concern. Flagging for the CLI batch.

**Im7. `if2.rs:317–327` — `run_if2` calls `run_if2_with_progress(..., None)` via explicit `None` for callback.** Fine pattern, but the `&dyn Fn(usize, f64)` callback only reports iteration number and loglik. Per-parameter diagnostics and clamp_fraction (already computed in `ParamIterDiag`) aren't exposed to the callback. For CLI progress that wants to show "clamp fraction is 0.5, your rw_sd is too large," the data exists but isn't plumbed through. Minor UX.

### Nits

**In1. `prior.rs:21`** constant name `HALF_LN_2PI` shadowed by the `PI` import elsewhere — fine, just noting.

**In2. `if2.rs:405`** — `seed ^ ((iter as u64) << 32)` — XOR with `iter << 32` collides with the `seed` upper bits when `seed > 2^32`. For a 64-bit seed, the upper 32 bits are meaningful; XORing with `iter << 32` overwrites them in low-iter chains. Fine for small iteration counts but would bite at `iter > 2^32` (never in practice).

**In3. `multi_stream_obs.rs:70–71`** — flow-name family match uses `tr.name.starts_with(&format!("{}_", flow_name))`. Same prefix-ambiguity concern as OCaml M15: if two transitions share a prefix (e.g. `infection` and `infection_wild`), asking for incidence of `infection` matches both. For indexed transitions with expanded names like `infection_under5`, this is the intended semantics; for ambiguous base names, it silently conflates. Flag-and-move-on pattern; worth a test.

**In4. `obs_loglik.rs:138–150` `negbin_logpmf`** — if `mu = 0, y = 0`, returns 0. If `mu = 0, y > 0`, returns `-inf`. Correct. But `mu = 0, k = 0, y = 0` also returns 0 because the `mu == 0` branch triggers before the `k == 0` check at line 142. Intentional? A NegBin(0, 0) is ill-defined (Gamma(0, ...) is degenerate). Probably won't arise from the runtime, worth a test.

**In5. `particle_filter.rs:207–241` `weighted_quantiles`** — the quantile lookup does a linear scan on sorted-by-value pairs. For large N, binary search on cumulative weights would be faster. Low priority.

## What's left

The heavy hitters:
1. `pgas.rs` (1718 lines) — the PGAS implementation with conditional SMC + ancestor sampling. This is the densest scientific code in the repo.
2. `pgas_grad.rs` (344) — gradients via PGAS-AD.
3. `nuts.rs` (428) — NUTS sampler, presumably for the PGAS+NUTS combo.
4. `pmmh.rs` (525) — PMMH with correlated pseudo-marginal MCMC.
5. `correlated_pf.rs` (406) — the correlated-PF machinery the chain-binomial step_one hooks into via `binomial_z_values` / `gamma_override`.
6. `diagnostic.rs` (419) — inference-level diagnostics (R-hat, ESS, etc.).
7. CLI drivers: `cli/src/if2.rs`, `cli/src/pfilter.rs`, `cli/src/fit/*.rs`, `cli/src/sampling.rs`.

PGAS next is where the scientific correctness risk is highest. Say the word.


I need to pause and think carefully here. The ancestor sampling in CSMC-AS looks problematic. Let me trace through it once more by looking at how the reference behaves and check the incident docs.I've found a subtle CSMC-AS ancestor sampling concern. Let me mark it and check more structure.This is a really important finding. I want to confirm by reading more of the code path. Let me check the run_pgas main loop and the tempering/swap logic.
