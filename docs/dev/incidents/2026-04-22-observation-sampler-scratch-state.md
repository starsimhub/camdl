# Incident: Observation sampler evaluated likelihood args against zero scratch state

**Severity:** Critical (silent wrong answer class)
**Discovered:** 2026-04-22 by the book / vignette agent, filed as GitHub
issues [#5](https://github.com/vsbuffalo/camdl/issues/5) and
[#6](https://github.com/vsbuffalo/camdl/issues/6).
**Found in:** `rust/crates/sim/src/inference/multi_stream_obs.rs`
**Status:** Three strikes before the bug was fully closed.
- First fix (`c87b275`, 05:53 UTC) — patched `MultiStreamObsModel`
  scoring-helper path. Issue closed.
- Book agent reopened 11 minutes later: CLI `--obs` path still broken.
- Second fix (`2030a2c`) — patched `compile_obs_sample_pf`. Issue
  closed.
- Book agent reopened again: `camdl pfilter` still broken (log-ll off
  by ~100×).
- Third fix (`d28664a`) — patched `ObservationModel::log_likelihood`
  trait impl on `MultiStreamObsModel`. **Also deleted the
  zero-scratch helper entirely** so the bug class cannot recur at a
  future fifth path. Issue closed for real this time.

Hardening proposal (phantom types on `IntState`) captured below — **held,
not implemented**. The helper-deletion in the third fix is a cheaper
structural fix that achieves similar protection: if the API doesn't
offer a "get a zero-filled IntState" call, no one can accidentally use
one. See the "Meta-incident" section for the full accounting.

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

## The fix(es)

**Two independent code paths, each with its own instance of the same
class of bug. The first fix patched one path and closed the issue
prematurely. Book agent rebuilt, re-ran, reported back: still broken.**

### Path A — `MultiStreamObsModel` (fixed in `c87b275`)

Used by the inference stack (PGAS, pfilter, etc.). `sample()`,
`mean()`, and `log_likelihood_from_flows_and_counts()` constructed a
scratch `IntState` via `with_scratch_int(n, f)` (zeros) instead of
`with_scratch_int_from_counts(counts, f)` (real state). The latter
helper already existed on line 51; three call sites used the wrong
sibling. Four-line swap.

### Path B — `compile_obs_sample_pf` (fixed in `2030a2c`)

Used by `camdl simulate --obs` **and this was the actual path the
book agent's reproducer exercised.** Constructed a bare
`IntState::new(n)` at closure-build time and captured it by value;
compartment state never entered the sampler regardless of what the
caller did at each obs time. Fix: extend the closure signature to take
`counts: &[i64]`; callers pass `snap_at(traj, obs_t).int_state.counts`
for each obs time.

Both paths exhibit the same bug class — **the code path's internal
`IntState` was never populated with real compartment data before the
likelihood expression was evaluated against it.** Different
implementations, same shape of mistake. See "Meta-incident" below for
why this recurrence is itself load-bearing.

Companion fixture bug (GH #5): the Ross-Macdonald golden was missing a
mosquito-recruitment transition, so `M → 0` and the epidemic collapsed.
One-line addition of `birth_v : --> S_v @ mu_v * M0`.

## Meta-meta-incident: even the proactive audit missed the third path

Between the second fix and the third, I did a "proactive audit" of
all `IntState` construction sites in `sim/` and `cli/`. The audit
found five latent instances of the same bug class and flagged the
fixes. I committed the fixes (`obs_model.rs` dead functions,
overdispersion-state-independence invariant) in the commit message
for the audit round.

The audit **still missed the site that was actively broken** — the
`ObservationModel::log_likelihood` trait impl on
`MultiStreamObsModel`. The book agent's reopen comment came in
concurrent with my audit commit.

Why the audit missed it: I grepped `IntState::new\|IntState::from_vec\|
IntState {` across the codebase. The missed site uses
`with_scratch_int(...)` — a thread-local-pool helper that doesn't
construct an `IntState` at its callsite at all; it borrows a reused
scratch from `SCRATCH_INT: RefCell<IntState>`. My grep pattern was
shaped by my mental model of the bug ("someone makes a zero
`IntState`"), not by the bug's actual manifestation in the codebase
("someone evaluates a likelihood expression against a state they
forgot to populate").

**Audit-technique lesson**: audit by the *symptom semantics*, not by
the syntactic form. The right grep here would have been something
like:

```sh
# Every place a likelihood expression is evaluated — find all of
# them, regardless of how they got their IntState.
grep -rn 'eval_likelihood_resolved\|sample_obs_resolved\|eval_obs_mean_resolved' sim/
```

That grep would have found every eval site and forced a per-site
inspection of "where did this IntState come from?" Three of those
sites would have revealed the scratch-pool helper, and the trait
impl would have been among them.

## Why deleting the helper didn't regress perf

Relevant context that was missing when the third-strike deletion
landed: `with_scratch_int` was originally added as a **performance
optimization** in commit `e77ab11` (perf(obs): thread-local scratch
IntState eliminates hot-loop alloc, IM2). The concern was that PF/IF2
hot paths allocated `IntState::new(n_int)` per-particle,
per-observation, per-stream — hundreds of thousands of heap allocs
per full filter pass.

The optimization was a `thread_local! SCRATCH_INT: RefCell<IntState>`
plus **two** helpers that both reuse that buffer:

- `with_scratch_int(n, f)` — zero-fills the buffer
- `with_scratch_int_from_counts(counts, f)` — `copy_from_slice`s
  the caller's counts into the buffer

Deleting only the zero-fill variant **preserves the optimization
entirely**. Both helpers share the `SCRATCH_INT` thread-local; the
only functional difference is the final populate step (`for v in ...
{ *v = 0; }` vs `copy_from_slice(counts)`). The retained variant is
if anything marginally *faster* on the populate step (SIMD memcpy
beats per-entry zero). All five hot-path sites in
`multi_stream_obs.rs` now use the populating helper and reuse the
same thread-local buffer with zero per-call allocation.

The per-call allocation concern does apply to the smaller set of
fixes in `obs_model.rs` (`compile_obs_sample_pf`,
`compile_obs_mean_pf`, the two dead `compile_obs_loglik_*`). Those
closures don't use the thread-local pattern — they were outside the
IM2 perf-fix scope — and my third-strike fix swapped their captured
zero `IntState` for a per-call `IntState::from_vec(counts.to_vec())`.
That's one Vec alloc per call. Quantified:

- `camdl simulate --obs` for a weekly-obs 1-year model: 53 calls × 40
  bytes ≈ 2 KB per simulate run. Negligible next to simulation work.
- Dead functions `compile_obs_loglik_if2` / `_pf`: zero callers in
  production. Documented with a note: if future callers wire these
  into a hot inference path, use the thread-local pattern from
  `multi_stream_obs.rs` (zero-alloc per call).

Net: no perf regression on any hot path. A small, non-hot Vec alloc
per obs emission that's ≈0.001% of simulate runtime.

## Third-strike structural fix: delete the footgun

The third fix did something the first two didn't: **deleted the
zero-scratch helper entirely.** The helper
`with_scratch_int(n, f)` in `multi_stream_obs.rs` existed precisely
for "construct a zero IntState for expression evaluation" — the exact
anti-pattern at the centre of all four bug sites. Every caller had
real `counts` in hand but was using the zero helper anyway. The
sibling `with_scratch_int_from_counts(counts, f)` has always existed
alongside it.

Removing the zero variant leaves only the populating one. The API
no longer offers the mistake. Future code paths that evaluate a
likelihood expression can only get a populated state, because that's
the only way to get a scratch. This is a smaller, cheaper version of
the phantom-type proposal — same protection, no refactor.

The general lesson: **if a helper's purpose is 'do X incorrectly,'
consider whether any caller actually wants that, and if not, delete
it.** The zero-scratch existed because at one point some code path
legitimately needed a zero state (the `log_likelihood_from_flows`
deprecated helper still does, by passing a zero slice explicitly).
But the thread-local pool variant was only ever used by the broken
sites. Deleting it cost one line of code; it eliminated a class of
bugs that had struck four times in 12 hours.

## Meta-incident: the first fix closed the issue but didn't fix the bug

The GH #6 issue was closed with `c87b275` at 05:53 UTC on 2026-04-22.
The book agent rebuilt and tested against the fix commit, reported
the behaviour unchanged at 06:04 UTC. The issue was reopened. The
actual fix landed at `2030a2c`.

This is itself an incident within the incident, and worth looking at
straight on.

### What actually happened

1. The book agent's bug report (GH #6) named the symptom:
   `diagnostic_test + prevalence()` produces wrong obs values.
2. I searched the codebase for observation-sampling paths that
   evaluated likelihood args and found
   `MultiStreamObsModel::sample()`. Its scratch-`IntState`
   construction was a plausible culprit — with zero compartments,
   `PopSum([S,I,R])` would collapse to 0, NaN the p-value, garbage
   the sampler output. That story fit the observed numbers.
3. I patched that code path, verified the scipy-match math still
   checked out, ran the workspace tests (which passed because no
   existing test exercised the state-dependent-arg path on
   `MultiStreamObsModel` either), ran the `test-integration` script
   (36/36 passed — but no integration test covers the state-
   dependent-obs path), and closed the issue.
4. At no point did I **run `camdl simulate --obs` with the book
   agent's reproducer and check the output numbers myself.**
5. At no point did I **trace the call graph from `camdl simulate
   --obs` down to the sampler to verify that path A was the path
   being exercised.**

The story I told was consistent with the symptom, consistent with the
fix I applied, and consistent with the passing test suite. It was
not consistent with the user-facing feature, and I never checked
that last piece.

### The deeper design smell

Path A and Path B are two independent implementations of
"evaluate a likelihood argument expression against the compartment
state at some time." Neither shares code with the other. Each
constructs its own `IntState`, makes its own choice about population,
and has its own shape of bug.

In principle, there should be exactly one primitive:

```rust
fn evaluate_likelihood_at_state(
    likelihood: &ResolvedLikelihood,
    projected: f64,
    state: CompartmentSnapshot,   // irreducible: name-to-value at a point in time
    params: &[f64],
) -> f64
```

Any path that wants to sample or score an observation constructs a
`CompartmentSnapshot` from whatever it has (a trajectory snapshot, a
particle's current state, a PGAS state-slice) and calls this one
function. There is nowhere to "forget to populate" the state,
because the snapshot is the input, not a field on a captured struct.

Instead, the current architecture has:
- `MultiStreamObsModel` with its own `SCRATCH_INT` thread-local,
  its own `with_scratch_int` / `with_scratch_int_from_counts` pair.
- `compile_obs_sample_pf` with its own `IntState::new()` captured
  by the returned closure.
- `compile_obs_mean_pf` with yet another copy of the same pattern.
- Several more in the pfilter / PGAS inference code that weren't
  touched here but very likely have the same structural issue.

The `IntState<Actual>` / `IntState<Scratch>` phantom-type proposal
from earlier in this document would make the scratch-vs-real
distinction a type-level contract. It would not, on its own, collapse
the independent implementations — but it would force each of them to
consciously pick which state flavour it wants, making the
"forgot to populate" mistake a compile error instead of a latent bug.

### What I should have done differently

Three disciplines that would have caught this in round one:

1. **Always run the exact reproducer the bug reporter gave.** The book
   agent's report ended with `tail -5 /tmp/out.tsv  # expect ~148, see
   3-14`. Running that command against the fix binary would have taken
   30 seconds and revealed the numbers hadn't changed. I relied on
   pattern-matching the symptom to a plausible call site instead.
2. **Trace the actual call graph from the user-facing command to the
   code.** "The user runs `camdl simulate --obs`. That calls
   `run_simulate` in `main.rs`. Inside, the obs-emission block calls
   `compile_obs_sample_pf`. That's the function I need to audit." The
   fact that I fixed a sibling function without first verifying the
   call path was traversing my fix is the central mistake.
3. **Before closing, reconstruct the symptom from first principles
   against the fix.** "Given `counts` properly wired in, I expect
   p_observed = 0.85·0.865 + 0.05·0.135 = 0.742, so N=200 survey
   draws should cluster near 148. Does the fix produce that?" That
   check is independent of whatever story I was telling about the
   cause.

These three are just variations on the same discipline: **independently
verify the user-facing symptom is resolved** before claiming the fix
is done. Passing tests are necessary but not sufficient. Passing tests
against a broken-in-the-same-way implementation are worse than
useless — they encode the bug as expected behaviour.

### Relationship to the held hardening proposal

The phantom-type proposal for `IntState` would have made path A
impossible-to-mistake (the `&IntState<Scratch>` call-site would
have required either populating or deliberately choosing scratch
semantics). It would have made path B obvious at the API boundary
(the closure holding an `IntState<Scratch>` would have to explicitly
label itself as using uninitialised state, or take state at call time).

But the deeper design fix is the unified `evaluate_likelihood_at_state`
primitive described above. That's a larger refactor — probably a week
of work — and it's the right next step if a third path of the same
class shows up. For now, both paths are patched and the bug class is
documented. Two strikes. A third signals structural change.

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
