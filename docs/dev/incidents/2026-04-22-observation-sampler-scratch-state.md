# Incident: Observation sampler evaluated likelihood args against zero scratch state

**Severity:** Critical (silent wrong answer class)
**Discovered:** 2026-04-22 by the book / vignette agent, filed as GitHub
issues [#5](https://github.com/vsbuffalo/camdl/issues/5) and
[#6](https://github.com/vsbuffalo/camdl/issues/6).
**Found in:** `rust/crates/sim/src/inference/multi_stream_obs.rs`
**Status:** Fixed in commit `c87b275`. Hardening proposal (phantom types
on `IntState`) captured below — **held, not implemented**.

---

## Summary

`MultiStreamObsModel` has two paths that evaluate observation-model
expressions — the sampling path (`sample()`) used by forward simulation
with `--obs`, and the scoring path
(`log_likelihood_from_flows_and_counts()`) used by inference. Both paths
evaluated the likelihood's argument expressions (the `p` in binomial,
the `mean` in normal/neg-binomial) against a **scratch `IntState`
filled with zeros** instead of the actual compartment counts.

Whenever a likelihood's args referenced compartment state — for
instance, `p = projected / N` where `N = PopSum([S, I, R])` —
the PopSum evaluated to `0`, `projected / 0` became `NaN`,
`NaN.clamp(0, 1)` stayed `NaN`, and `rng.binomial(n, NaN)` returned
garbage. Log-likelihood scoring was similarly poisoned because
`log p(y | p=NaN)` propagates NaN through the accumulator.

The bug affected every `Binomial`, `Bernoulli`, and `Normal` likelihood
whose arguments reference state. `NegBinomial(mean=rho*projected, r=k)`
and `Poisson(rate=rho*projected)` with `projected`-only args were safe
— which is why the He et al. 2010 replication never tripped it.

## Concrete reproducer

From the book agent's Ross-Macdonald fit (GH #6). True state at
equilibrium: `I_h ≈ 865`, `S_h ≈ 135`, `H = S_h + I_h = 1000`.
Observation model:

```camdl
slide_positivity : {
  projected = prevalence(I_h)
  every     = 1 'weeks
  likelihood = diagnostic_test(
    base = binomial(n = N_tested, p = projected / H),
    sens = rho_sens, spec = rho_spec
  )
}
```

with `N_tested = 200`, `rho_sens = 0.85`, `rho_spec = 0.95`.

Expected survey: `p = 0.85 · 0.865 + 0.05 · 0.135 = 0.742`, `N · p ≈
148` positives per weekly survey.
Observed pre-fix: **3–17 positives** across all obs times, including
equilibrium.
Cause: `PopSum([S_h, I_h])` evaluated against the zero scratch → 0 →
divide-by-zero → NaN → garbage sampler output.

## Why this slipped

Five overlapping blind spots:

1. **No existing observation test exercised a state-dependent
   likelihood argument.** The in-tree tests covered
   `poisson_logpmf(y, projected)` (no state refs) and `binomial(n =
   param, p = projected)` shapes (only references `Projected`,
   which *was* plumbed). The `Pop`/`PopSum`/stateful-let paths went
   untested on the sampling and scoring sides.

2. **The He 2010 replication didn't hit it.** Its likelihood is
   `neg_binomial(mean = rho * projected, r = k)` — `mean` depends only
   on the projected value. The most load-bearing production fit in
   the repo was structurally insulated from the bug.

3. **The Gate-2 hierarchical-prior tests validated density logic in
   isolation** (scipy oracle, IC3 Jacobian regression) but did not
   plumb through the full observation-sampling pipeline where this
   bug lived.

4. **`diagnostic_test` was the first feature that *structurally
   requires* a state-dependent `p`.** You can't express sens/spec
   correction on a bare count; you need `projected / N`. So the
   moment the sugar shipped and was actually used, the latent bug
   was forced out.

5. **`NaN` propagation hid the failure mode.** `p.clamp(0, 1)` on NaN
   stays NaN; `rng.binomial(n, NaN)` produces implementation-defined
   low-number garbage rather than panicking. There is no assertion
   saying "the p-value passed to binomial must be finite" at the
   sampler boundary.

## The fix

`multi_stream_obs.rs` already had the right helper —
`with_scratch_int_from_counts(counts: &[i64], f)` at line 51 — which
populates a scratch `IntState` from real `counts`. Three call sites in
the file used the wrong sibling (`with_scratch_int(n, f)` which fills
with zeros). Swap to the correct helper:

```diff
-  with_scratch_int(self.n_int, |int_s| { eval_likelihood_resolved(…, int_s, …) })
+  with_scratch_int_from_counts(counts, |int_s| { eval_likelihood_resolved(…, int_s, …) })
```

Plus the same swap in `sample()` and `mean()`, which had their own
identical mistakes using `state.counts`. Four-line change across three
sites. See `c87b275`.

Companion fixture bug (GH #5): the Ross-Macdonald golden was missing a
mosquito-recruitment transition, so `M → 0` and the epidemic collapsed.
One-line addition of `birth_v : --> S_v @ mu_v * M0`.

## Hardening proposal: phantom types on `IntState` (HELD)

The root pattern is **"context was assumed populated but wasn't"** —
a variant of the uninitialised-read class. Rust's type system
can prevent it entirely with a phantom tag:

```rust
pub struct IntState<Kind = Actual> {
    pub counts: Vec<i64>,
    _kind: PhantomData<Kind>,
}

pub struct Actual;   // backed by a real state, populated
pub struct Scratch;  // reused scratch — may be uninitialised
```

Functions that need the real state take `&IntState<Actual>`; scratch
helpers return `IntState<Scratch>`. A call that passes a scratch where
actual is expected fails at compile time. The IC3 `Scale` phantom
shipped for `Prior::log_density` has the same shape and same payoff:
make a silent-wrong contract into a compile error.

**Why held:**

- Non-trivial refactor — `IntState` is used throughout the sim crate,
  ~50 call sites.
- No other currently-known incidents of the same class. `Scratch` for
  obs sampling was the canonical instance, and the fix above closed
  it with a four-line change.
- The cheaper mitigation — a hardening test pass that exercises every
  observation-model code path with at least one state-referencing
  argument — catches 90% of regressions at 5% of the cost.

**When to revisit:** if a second incident in the "scratch state slipped
through" class surfaces, re-prioritise the phantom refactor. Until
then, the test-based mitigation is where the incremental effort lands.

## Cheaper mitigation (to land soon, not this session)

Add to `rust/crates/sim/tests/` a small file
`obs_state_dependent_args.rs` with one test per likelihood family that
has state-referencing args:

- `binomial(n = param, p = projected / PopSum(...))`
- `binomial(n = PopSum(...), p = projected / param)`
- `bernoulli(p = f(projected, PopSum(...)))`
- `normal(mean = f(projected, Pop(...)))`

Each test builds a known state, calls `sample()` + `log_likelihood(...)`,
asserts the result matches an analytical oracle to 1e-10. Seed pinning
+ many draws averaged for the stochastic `sample()` side.

This mitigation is the same shape as the Gate-2 scipy-oracle
hierarchical tests and takes ~3 hours of focused work.

## Closing note

The fix itself was mechanical. The lesson is structural: **any code
path that takes an `IntState` parameter is a potential instance of
this class of bug.** The fix swapped one helper for another; the
hardening (tests today, types tomorrow) prevents the next lookalike.
