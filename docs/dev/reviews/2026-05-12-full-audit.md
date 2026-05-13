---
status: open
date: 2026-05-12
scope: full codebase — Critical, High, Medium severity
reviewer: internal (post-gh#53 cohort-fire-step landing, HEAD = 793c6968)
triggered-by: full code review per docs/dev/code-review.md
---

# Full audit — 2026-05-12

Full pass over `rust/crates/sim/src/inference/`, `rust/crates/sim/src/`,
`rust/crates/ir/`, `rust/crates/cli/`, `ocaml/lib/`, and the IR boundary,
following the `docs/dev/code-review.md` brief. Outputs from camdl inform
polio (cVDPV2) eradication strategy; treat correctness as the dominant axis.

Verified-against: HEAD `793c6968` (gh#53 audit: 2 new ladder-dimension
regression tests).

**Counts: 8 Critical, 14 High, 21 Medium.**

## Summary

The shape of the codebase is healthy at the seams checked closely. `sim::time`
is genuinely the single time-conversion entry point — recent gh#53 work landed
cleanly. There is no parallel `step_csmc`; PGAS calls `step_one` from
`chain_binomial.rs` and `log_transition_density_substep` is a deliberate
density shadow with an explicit parity contract. `log_sum_exp` has one
canonical implementation that all consumers route through.
`eval_likelihood_resolved` is the single observation-likelihood dispatcher.
Dimension checking covers per-day/per-week/probability distinctions at parse
time. The `2026-04-30-correctness.md` Critical findings appear resolved.

Trouble is concentrated in three failure modes:

1. **Silent fallbacks where errors should fire.** Div-by-zero returns `0`,
   negative compartment counts get clamped, missing priors silently become
   improper uniform, `balance {}` is silently dropped on three backends.
2. **Value/gradient/objective inconsistencies in the inference math.** PGAS
   gradient omits obs-likelihood derivatives while including obs-likelihood
   value; IF2 returns the MLE from the iteration that maximized a quantity
   the same file documents as "NOT useful for model assessment."
3. **The OCaml↔Rust IR contract has no enforced single source of truth.**
   `ir/schema.json` and `ir/VERSION` are inert. Both sides are hand-maintained
   mirrors. Several Highs (H6, H7, H14) and Mediums (M12, M13) follow from this.

The "what's clean" list at the bottom records seams I verified and chose not
to flag, so a future reviewer doesn't re-derive.

---

## Findings

### Critical

---

**C1. PGAS gradient drops obs-likelihood and gamma-density derivatives.**

`rust/crates/sim/src/inference/pgas_grad.rs:298-317`. `complete_data_loglik_grad`
adds `obs_model.log_likelihood_from_flows_and_counts(...)` to the **value**
(line 314) but never adds its gradient. Same for the per-substep
gamma-multiplier density. Inline comments dismiss this with "currently zero
because σ² is typically a constant (not estimated)" and "Observation density
(gradient is zero when obs params are fixed)."

This is a value/gradient mismatch. When the estimated-parameter set includes
any obs-likelihood parameter (`rho`, `psi`, dispersion `k`) or any parameter
driving an overdispersion σ² expression, NUTS sees zero gradient for that
coordinate while the log-posterior responds. Symptom: divergences spike,
step size collapses, the chain barely moves on that coordinate, and its
marginal posterior regresses to its initial value — silently, because there
is no preflight check. `inference/if2.rs:185` confirms obs likelihoods take
`params` ("`obs_loglik_fn` — observation log-likelihood (takes projected,
observed, params)"); obs params are a real estimation target.

**Fix.** Either (i) emit a hard error at `run_pgas` entry when any
`EstimatedParam.index` appears in an obs-likelihood expression or in an
overdispersion σ² expression and is not covered by `rate_grads_for_run`; or
(ii) thread the obs-likelihood / gamma derivatives through
`complete_data_loglik_grad` using the existing compiler autodiff. Until
(ii) lands, (i) is mandatory.

---

**C2. IF2 returns MLE from the iteration that maximized the perturbed
log-likelihood.**

`rust/crates/sim/src/inference/if2.rs:525-535`. `best_iter` is selected by
`it.if2_perturbed_loglik.total_cmp(...)` (lines 527-528); `mle:
best_iter.param_means.clone()` is returned at line 532. The same file
documents this field at lines 125-129:

> "IF2 perturbed-model log-likelihood (internal diagnostic only). Peaks
> early due to perturbation smoothing, then declines as cooling progresses.
> NOT useful for model assessment or convergence — use `loglik` instead."

The clean-PF `loglik` field (lines 121-124) is populated post-hoc by the
caller and is `NaN` when not evaluated, so it can't be used as the
selection key without a fallback.

Under typical cooling, perturbed-loglik peaks at an early-to-middle
iteration; thereafter it declines even though the chain converges on the
true MLE. So `run_if2` returns parameters from an under-converged
iteration. Anything that consumes `result.mle` directly (starting point
for PGAS/PMMH; the typhoid-diagnostic gate referenced in CLAUDE.md;
headline point estimates in reports) is biased.

**Fix.** Select `best_iter` on `it.loglik` (clean PF) when finite; fall
back to the last iteration otherwise. Assert at least one iteration had
a finite `loglik` for any "real" run (not just smoke-testing).

---

**C3. `balance {}` is silently dropped by tau_leap, gillespie, and ode
backends.**

Applied in `rust/crates/sim/src/chain_binomial.rs:407,432` and
`inference/pgas.rs:736,1279`. **No reference** in `tau_leap.rs`,
`gillespie.rs`, or `ode.rs` (verified by grep).

A model with `balance { R = pop(t) - S - E - I }` is honored on
chain-binomial and PGAS but produces a silently different trajectory on
the other three backends, where the balance compartment drifts according
to whatever transitions modified it.

Worst-case scenario: a user fits with chain-binomial and forward-simulates
with `tau_leap` (faster); simulated trajectories no longer conserve
population, and the resulting scenario projection is wrong. No warning,
no diagnostic.

**Fix.** Option (a): apply balance at the end of each substep in
tau_leap.rs, gillespie.rs (post-event), and ode.rs, mirroring the
chain_binomial path. Option (b): if balance is intentionally
chain-binomial-only, declare a `BALANCE` capability in `Capabilities`
(`sim/src/lib.rs`); `CompiledModel::required_capabilities()` sets it
whenever `model.balance.is_some()`; only chain_binomial's
`Simulate::capabilities()` declares support; dispatch produces the
standard hard error. (a) is correct if balance is a model invariant;
(b) is correct if it's a chain-binomial construct.

---

**C4. Estimated parameters with no explicit prior silently get improper
uniform.**

`rust/crates/cli/src/fit/runner.rs:1932-1957` (`resolve_prior` falls back
to `Prior::Flat`); `rust/crates/sim/src/inference/prior.rs:91`
(`Prior::Flat.log_density = 0`); `prior.rs:181` (silently maps
`PriorDist::Fixed` → `Prior::Flat` when in a "prior slot").

A fit.toml that estimates a parameter without an IR or fit.toml prior
runs with no warning, returning a "posterior" that is actually a
likelihood profile rescaled. The `Fixed → Flat` coercion is worse: the
user thought they had pinned the value.

The code-review brief calls this out explicitly: "If the user omits a
prior and the system silently uses an 'uninformative' default, that
default appears in the posterior. Make it explicit. No prior, no run."
For polio decision support, a credible interval reported as Bayesian but
actually a likelihood profile is the worst-case communication failure.

**Fix.** `resolve_prior` returns `Result`. Hard error (e.g.,
`E5xx prior_required`) listing offending parameter names. Drop the
`PriorDist::Fixed → Prior::Flat` conversion at `prior.rs:181` — if a
parameter is in a "prior slot" but typed `Fixed`, that's a config error,
not a coercion target.

---

**C5. Compartment counts below zero silently clamped to 0 in chain-binomial.**

`rust/crates/sim/src/chain_binomial.rs:407-416`. Any overshoot in the
binomial split, or an `Action::Add` with a negative resolved value
(`intervention.rs:248-262`, which only `log::warn!`s), silently zeros the
offending compartment. Population is no longer conserved and the
trajectory continues.

Inference proceeds against trajectories that violate the conservation
invariant the model assumes. Log-likelihood remains finite but is
computed against the wrong dynamics. Parameter posteriors shift
systematically toward parameter values that produce overshoot — exactly
the regime the clamp hides.

**Fix.** Replace the silent clamp with `SimError::NegativeCount {
compartment, value, t }`. Add
`tests/chain_binomial_invariants.rs::overshoot_errors_not_clamps`:
construct a model with two competing transitions whose
`1 - exp(-r_i dt)` sum exceeds 1; assert hard error.

---

**C6. Division by zero, NaN/Inf in `Pow`, and negative `Sqrt` silently
return `0` in rate evaluation.**

`rust/crates/sim/src/propensity.rs:86-101` (Div, Pow, Mod);
`propensity.rs:117-124` (`Sqrt` of negative → 0; NaN absorbed → 0);
`resolved_expr.rs:261,444`.

`beta * I / N` evaluates to `0` when `N = 0`, not an error.
`Sqrt(-1)` returns 0. The `EvalStats` global counters
(`sim/src/eval_stats.rs:16-67`) record these — but nothing reads the
counters (see H5). `log::debug!` is the only user-visible signal.

A spatial model with a small patch that empties at runtime continues
with zero force-of-infection from that patch; the user reads "outbreak
ended in patch X" when in fact the structural rate expression divides
by an empty divisor. This is the single most insidious class of bug in
the inference math because it can pass every unit test and still corrupt
every scenario projection.

**Fix.** Return `SimError::NumericalCollapse { expr, t }` from `eval_expr`.
Add `tests/numerical_collapse.rs::div_by_zero_errors_not_silences`. If a
best-effort mode is wanted, gate it behind explicit
`--allow-degenerate-rates` rather than make it the default.

---

**C7. PGAS post-burn-in `n_divergent` and `n_max_treedepth` not surfaced
into result.**

Declared at `rust/crates/sim/src/inference/pgas.rs:1391-1392`; accumulated
only when `rung == 0` at `:1526-1532`; logged at burn-in boundary
`:1717-1739`. `PGASResult` constructor at `:1807-1812` does not include
these fields. The `DiagnosticKind::DivergentTransitions` and
`::MaxTreeDepthHits` variants exist in `inference/diagnostic.rs:72-81`
with full render/hint plumbing but are constructed nowhere (see H4).

A PGAS run with 500 burn-in + 5000 sampling sweeps producing 200
post-burn-in divergences reports zero divergences in any persisted output.
The user signs off on the posterior.

**Fix.** Add `pub n_divergent: usize, pub n_max_treedepth: usize` (plus
the per-rung swap-acceptance counts from M18) to `PGASResult`; populate
at line 1807; have the fit CLI emit them into `diagnostics.json` and
wire them through `DiagnosticKind::DivergentTransitions`/`MaxTreeDepthHits`
so the gating module fires.

---

**C8. `ir/schema.json` and `ir/VERSION` are inert — no single source of
truth for the IR contract.**

`/Users/vsb/projects/work/camdl/ir/schema.json`,
`/Users/vsb/projects/work/camdl/ir/VERSION`; `rust/crates/ir/src/lib.rs:14-22`,
`ocaml/lib/ir/serde.ml:982-998`. Verified by grep: `schema.json` is
referenced nowhere in source; `ir/VERSION` is read by nothing (the only
`VERSION` reference is `external-harness/src/runner.rs:11`, which is
`CARGO_PKG_VERSION`, unrelated).

CLAUDE.md and the project brief say "the IR schema is the contract
between OCaml and Rust." In reality both sides are hand-maintained
mirrors. `from_str`/`from_reader` perform no schema-version handshake.
`model.version` is the user's model semver, not the IR schema version.
Schema drift therefore manifests as a `serde::Error` at golden-test time
(best case) or as a wrong-but-parseable simulation (worst case — see C3
+ H6 + H7 + H14 for concrete examples).

This is the structural root cause behind several other findings. Every
IR change has to be done by hand in three places; the system has no way
to detect when one of those three falls behind.

**Fix.** Pick one source. Option (a): generate both OCaml and Rust IR
types from `schema.json` via `quicktype`/`schemafy` + a small OCaml
codegen step. Option (b): designate `ocaml/lib/ir/ir.ml` as authoritative;
delete `schema.json`; add a CI step that diffs Rust struct fields
against OCaml types. Either way: wrap the IR in
`IrEnvelope { ir_version: String, model: Model }`; Rust includes
`pub const IR_VERSION: &str = include_str!("../../../../ir/VERSION");`
and rejects mismatched envelopes with a hard, named error.

---

### High

---

**H1. NUTS outer-tree combine uses `n_prime/(n_valid + n_prime)` with
slice-indicator counts.**

`rust/crates/sim/src/inference/nuts.rs:244-250` (outer combine); slice
indicator at `:323`.

The leaf at line 323 uses slice indicators (`if log_slice <= -h_new
{ 1 } else { 0 }`). The outer combine at line 245 uses
`n_prime / (n_valid + n_prime)`. Hoffman & Gelman 2014 Algorithm 6
(referenced by the comment at `:348`) uses `min(n_prime / n_valid, 1)`
at the outer level; multinomial-NUTS (Stan) uses weights, not slice
indicators, in a ratio form. The code is a hybrid.

The current form is closer to Algorithm 3 (original slice-NUTS endpoint
= uniform sampling from slice-valid set) than Algorithm 6 (biased toward
newer subtrees). It may still target the correct stationary distribution
— but it is non-standard and the choice is undocumented. Marking High
not Critical for that reason; full derivation against H&G needed.

**Fix.** Either (a) change line 245 to
`let accept_prob = (n_prime as f64 / n_valid as f64).min(1.0);` to match
H&G Alg 6 exactly, or (b) keep the current form and add a comment +
citation explaining it is Alg-3-style uniform endpoint sampling, and
prove that the combination with the Alg 6 inner sampling at line 350
preserves the target.

---

**H2. Discretized-normal observation likelihood uses A&S 7.1.26 erf
(1.5e-7 absolute error) in the tails.**

`rust/crates/sim/src/inference/obs_loglik.rs:166-179` (`normal_cdf`),
`:213-226` (`discretized_normal_logpmf_tol`).

`discretized_normal_logpmf_tol` computes `Φ(z_hi) − Φ(z_lo)` for the
observation interval. With Abramowitz & Stegun 7.1.26 (max abs error
≈ 1.5e-7), if both Φ values are within 1e-7 of 0 or 1, the difference
is dominated by approximation noise.

For polio (rare-event endemicity, AFP surveillance with high zero-rates),
tail observations are exactly where inference is hardest. Particle
weights at those steps are determined by 1e-7-scale noise rather than
the model's predicted incidence. The `DEFAULT_TOL = 1e-18` log-PMF
floor hides this from sanity checks while leaving gradients and relative
particle ordering arbitrary.

**Fix.** Use `libm::erfc` directly. Compute `Φ(z) = 0.5 * erfc(-z / √2)`.
For the interval, use an erfc-based formula that avoids subtracting two
near-1 values (split by sign of `z_lo + z_hi`).

---

**H3. Catastrophic cancellation `1.0 - p_total` in PGAS when rate·dt is
large.**

`rust/crates/sim/src/inference/pgas.rs:301-302, :319`; gradient at
`pgas_grad.rs:165, :171`.

`p_total = (1.0 - (-total_rate * dt).exp()).clamp(1e-15, 1-1e-15)`.
For `total_rate * dt ≫ 1`, direct subtraction of `1 - exp(-large)`
collapses to floating-point noise; `binom_logpmf` tail accuracy and the
gradient `(n-k) / (1-p)` blow up. The IM17 comment at
`pgas_grad.rs:157-164` calls these gradient values "tolerated behavior"
and admits divergences.

Tolerated divergences are not benign — they pin NUTS step-size adaptation
low, slow mixing, and effectively bias the posterior toward parameter
values that keep `p_total` in the interior (favoring smaller rates).
For typhoid/polio models with fast clearance rates and `dt = 1` day,
this regime is reached on observed peaks.

**Fix.** Carry `(p, q = 1 - p)` as a pair throughout. Compute
`q_total = (-total_rate * dt).exp()` directly; pass both to a
`binom_logpmf` overload that accepts `(k, n, p, q)`. The gradient form
reads `k/p − (n-k)/q` without any subtraction.

---

**H4. ~14 of 22 `DiagnosticKind` variants never constructed.**

`rust/crates/sim/src/inference/diagnostic.rs:36-166`. Grep verified:

- Constructed: `RhatHigh`, `MultimodalLikelihood`, `ConvergenceIncomplete`,
  `InitialLoglikInfinite`, `AcceptanceRateUnhealthy`, `AutoRwSd`,
  `CompressedLogitPosition`, `CoolingExhausted`.
- Never constructed: `ChainDiverged`, `LowESS`, `LowESSAtMLE`,
  `MaxTreeDepthHits`, `DivergentTransitions`,
  `DegenerateAncestorSampling`, `LowTrajectoryRenewal`,
  `GammaDensityDisabled`, `ParamNearBound`, `ProfileCIUnbounded`,
  `FlatProfile`, `AutoRwSdNoConsensus`, `ObsModelMismatch`,
  `ZeroRateNonzeroFlow`, `LowSwapRate`, `ResumeConfigMismatch`,
  `ResumeParamMissing`.

The diagnostic infrastructure exists with full render + hint + severity
classification + gating-rule integration. The detection step that
actually constructs each variant is missing — even though the underlying
quantities are computed (ESS in `particle_filter.rs:285`, trajectory
renewal in `pgas.rs:1724`, swap rates in `pgas.rs:1745-1750`).

Combined with C7, the standard sanity checks (low ESS, divergences,
near-bound params, low swap rate on tempered chains) are not telling the
user what they think they are.

**Fix.** For each unwired variant, add the threshold check at the site
where the underlying quantity is computed. Specifically: `LowESS`
whenever `ess_trace.last() < n_particles * 0.10`;
`DivergentTransitions`/`MaxTreeDepthHits` from the now-surfaced
`PGASResult` fields (C7); `LowSwapRate` from the swap-acceptance counts
(M18); `ParamNearBound` after each MCMC chain finalizes. The work is in
choosing thresholds, not in code volume.

---

**H5. `EvalStats` degenerate-evaluation counters incremented everywhere,
read nowhere.**

`rust/crates/sim/src/eval_stats.rs:16-67` (defines `DIV_BY_ZERO`,
`POW_NAN_INF`, `UNOP_NAN`, `NEG_BINOMIAL_POIS`, `BINOMIAL_FALLBACK`
atomic counters + `snapshot/diff_since` API); incremented at
`rng.rs:58, :68, :115`, `resolved_expr.rs:262, :269, :297`.

The header comment says "a cheap summary the caller can check at sim
end." That caller does not exist in `rust/crates/cli/src/`. Directly
compounds C6 — the user has no way to know the rate-eval path hit a
degenerate regime, even with the counters infrastructure in place.

**Fix.** In `cmd_simulate`, `cmd_pfilter`, `cmd_if2`, `cmd_fit_run_v2`:
snapshot `EvalStats::snapshot()` at start, diff at end, emit
`eval_stats.json` to the run dir whenever `total() > 0`. ~10 lines of
code per CLI entry point.

---

**H6. `param_kind` is `Option<String>` on both sides despite finite
domain.**

`rust/crates/ir/src/parameter.rs:127-130`, `ocaml/lib/ir/ir.ml:270`,
`ocaml/lib/compiler/expander.ml:1628` (`param_kind_to_string` adapter);
`table.cell_kind: Option<String>` inherits the same defect (gh#32).

Valid kinds are exactly {rate, probability, positive, count, real}.
The OCaml DSL has a proper sum (`ast.ml:70 param_type = PRate |
PProbability | PPositive | PCount | PReal`); the compiler downgrades to
`string` for IR; the Rust side keeps `Option<String>` and pattern-matches
string literals at every dim-check and transform-default site. No
`UnknownParamKind` error in `validate.rs`.

Typos route silently to default behavior. Exhaustiveness on the consumer
side is unenforceable.

**Fix.** `enum ParamKind { Rate, Probability, Positive, Count, Real }`
with `#[serde(rename_all = "snake_case")]` in `parameter.rs`; mirror in
`ir.ml`; drop the `param_kind_to_string` adapter. One refactor closes
off many call sites.

---

**H7. Schema drift: `Interpolated.method` is a typed enum in Rust, raw
string in OCaml.**

`rust/crates/ir/src/time_func.rs:19-31` (`InterpMethod { Linear,
Constant, Spline }`); `ocaml/lib/ir/ir.ml:85` (`method_: string`);
`ocaml/lib/ir/serde.ml:326,354`.

Rust rejects any value not in {linear, constant, spline}. OCaml accepts
and round-trips arbitrary strings. A typo `"cubic"` from the DSL passes
OCaml validation and crashes Rust at deserialize time, far from the
source line — the exact bug class C8 should catch and currently doesn't.

**Fix.** Mirror `interp_method = Linear | Constant | Spline` in
`ir.ml`/`serde.ml`. Validate at the OCaml deserialize boundary. Same
pattern probably lurks for `time_semantics` and `output.format`; sweep
in one pass.

---

**H8. CSMC ancestor-sampling categorical uses post-resample predecessor
states.**

`rust/crates/sim/src/inference/pgas.rs:826-859` (snapshot `prev_counts`
after resample at lines 828-830; reference slot corrected at line 859);
line 891-902 (ancestor weight evaluation).

`prev_counts[j]` is set from `counts[j]` for all j after the resampling
shuffle. The reference slot is corrected back to `ref_rec.counts_before`.
Ancestor-sampling weights at line 891-902 then pair "transition density
from slot j's post-resample state to ref flows" with "particle j's slot
identity." Ancestor sampling is supposed to be a categorical over the
pre-step particle ensemble; the code categoricalizes over a
post-resample-relabeled one.

Often invisible at the marginal-posterior level (the permuted multiset
has the same support), but on observation-tight steps with heterogeneous
pre-step states (spatial models with very different patch prevalences),
the wrong index can be selected. Parameter estimates drift toward
whatever values are over-represented in commonly-resampled slots. The
trajectory-renewal diagnostic looks fine because the trajectory does
renew — the latent-path marginal is wrong.

**Fix.** Cache `prev_counts_for_ancestor = counts.clone()` immediately
on entering each substep (before the resampling block at 802-825). Use
it (with the reference-slot correction) at line 891-902. The existing
`prev_counts` save can stay for the rest of the loop.

---

**H9. Two diverged `systematic_resample` implementations.**

`rust/crates/sim/src/inference/resampling.rs:18-40`
(`systematic_resample`) vs `correlated_pf.rs:421-442`
(`sorted_systematic_resample`).

`sorted_systematic_resample` is a copy of `systematic_resample` with
the same cumsum loop and `normalize_log_weights` call; the only
difference is the uniform source (RNG vs caller-provided `base_uniform`).
Both have the same off-by-one risk on the last weight.

The vanilla PF and PMMH's CPM filter no longer share their statistical
core. A fix to one (boundary handling, weight-floor policy) won't
propagate.

**Fix.** One canonical
`systematic_resample_with_u(log_weights, u) -> Vec<usize>`.
`systematic_resample(log_weights, rng)` becomes a one-liner wrapping it.
CPM passes `base_uniform`. Delete `sorted_systematic_resample`.

---

**H10. PGAS ancestor-sampling categorical bypasses canonical
`normalize_log_weights`.**

`rust/crates/sim/src/inference/pgas.rs:1017-1034`
(`sample_categorical_log`); called from `pgas.rs:909` and `:960`.

Re-implements max-subtract softmax + categorical inverse-CDF rather than
delegating to `normalize_log_weights` + a shared sampler. The function's
degenerate-case contract differs from `normalize_log_weights` (`None`
vs uniform fallback). At the final-trajectory pick (line 960), the
`.unwrap_or(j_ref)` fallback differs from what systematic-resample
would do for the same swarm.

This is the ancestor-sampling weight transform in CSMC — the heart of
PGAS. Drift between this and the PF resampling biases the posterior
silently.

**Fix.** Add `categorical_log(log_weights, &mut rng) -> Option<usize>`
to `inference/resampling.rs`. Replace both call sites. Document the
degenerate-policy contract in one place.

---

**H11. Inference indices have no newtypes — `ParticleIdx`,
`CompartmentIdx`, `ObsIdx`, `StratumIdx` all bare `usize`.**

`rust/crates/sim/src/state.rs:3,36,66`; `inference/types.rs:50,233-241`;
`inference/pgas.rs:113, :1054` and many other call sites.

`state.counts: Vec<i64>`, `state.flow_accumulators: Vec<u64>`,
`EstimatedParam.index: usize`, `IVPMapping.compartment_idx: usize`,
plus implicit `obs_idx`/`particle_idx`/`stratum_idx` arguments swirl
around together in `pgas.rs`. Nothing prevents `pgas.rs:1054` from
indexing `initial_counts[stratum_idx]` instead of `[compartment_idx]`.

CLAUDE.md flags this code as high-risk. A swap typo between two `usize`
arguments is invisible to `cargo check` and can produce wrong posteriors
silently. The compile-time cost of `pub struct ParticleIdx(usize);`
is zero.

**Fix.** Introduce `ParticleIdx`, `CompartmentIdx`, `ObsIdx`,
`TransitionIdx`, `StratumIdx` in `sim/inference/types.rs`. Roll out
incrementally — even the first newtype shuts off a class of error.
Pair with `LogWeight(f64)`, `LogDensity(f64)`, `Probability(f64)`,
`Rate(f64)` newtypes in priority order for hot inference paths.

---

**H12. `--record-prequential` and `--record-ancestry` silently no-op
outside PFilter stage.**

`rust/crates/cli/src/args/mod.rs:465-472` (declared with only
`requires = "stage"`); consumed only in `rust/crates/cli/src/fit/mod.rs:1192-1248`
(the `Stage::PFilter { .. }` match arm).

clap accepts the flag for any stage name; only the PFilter arm reads it.
`camdl fit run config.toml --stage scout --record-prequential` silently
drops the flag.

**Fix.** Runtime check in `cmd_fit_run_v2` after the stage type is known:
if `a.record_prequential || a.record_ancestry` and the stage is not
PFilter, exit with `error: --record-prequential requires --stage
<pfilter-stage>` listing valid PFilter stages from the current config.

---

**H13. `--parallel` / `CAMDL_PARALLEL` ignored by `camdl pfilter`.**

Declared on `InferenceCore` at `cli/src/args/mod.rs:78-79`, embedded into
`PfilterArgs` at `:773`; consumed only in `if2.rs:100,282,372` and
`profile.rs:333,849-851`. Grep confirms `pfilter.rs` has no `parallel`
or `rayon` reference.

`camdl pfilter --parallel 16` runs single-threaded. Replicate loops at
`N=10000` particles × 100 replicates take 16× longer than the user
expects, no warning.

**Fix.** In `pfilter.rs::cmd_pfilter`, build a rayon pool from
`a.inference.parallel` (matching the `if2.rs:369-374` idiom) before the
replicate loop at line 209; use `into_par_iter()`.

---

**H14. Validation logic duplicated OCaml↔Rust with subtly different
surfaces.**

`ocaml/lib/ir/validate.ml` (130 LOC) vs `rust/crates/ir/src/validate.rs`
(337 LOC); dimensional analysis exists only in `ocaml/lib/ir/dimcheck.ml`
(940 LOC); Rust dim-check lives in `compiled_model.rs`.

Rust checks `UnknownTransitionInObservation` with obs context; OCaml
uses `UnknownTransition` (worse diagnostic). Rust checks
`PriorAndHierarchicalBothSet`; OCaml does not. Dimensional analysis is
OCaml-only — a hand-edited IR JSON that bypasses the DSL escapes those
checks entirely.

The Rust runtime is what executes user models; the OCaml frontend is
the only line of defense for dim-check. If users ever pipe IR around
without going through OCaml, half the validation disappears silently.

**Fix.** Define one validation contract. Move pure structural checks
(refs, dup names, prior/hierarchical exclusivity) to one side; have
Rust re-run them post-deserialize. Document explicitly which invariants
are OCaml-only and emit a marker in the IR envelope (e.g.,
`validated_by: "ocaml-compiler-v0.3"`) that Rust requires to skip
re-validation.

---

### Medium

Numerical / statistical (8):

- **M1.** `normalize_log_weights` falls back to uniform when all weights
  are `-inf`; PF/IF2/PGAS proceed as if information was uniform rather
  than as if no particle is consistent. `inference/types.rs:345-358`.
  Fix: check `swarm.ess() > 0` in `bootstrap_filter`
  (`particle_filter.rs:280-283`) and `csmc_as` (`pgas.rs:928-939`); return
  `-inf` increment + `n_collapsed` flag.
- **M2.** IF2 `cooling_target_iters` uses `n_obs` instead of
  `(1 + n_obs)`; cooling fires twice as fast on sparse-observation
  models. `inference/if2.rs:252-261`.
- **M3.** `transformed_sd` delta-method singular at log lower bound —
  small-rate IF2 particles get giant perturbations that effectively
  re-initialize them. `inference/types.rs:141-151`. Fix: cap or use
  `rw_sd` directly in transformed space.
- **M4.** PMMH `acceptance_rate` divides by total `n_steps`, not
  post-burn-in count. `inference/pmmh.rs:500-505`.
- **M5.** Correlated PF sort key (`flow_accumulators.iter().sum()`)
  collapses dissimilar particles; CPM acceptance variance blows up.
  `inference/correlated_pf.rs:362-372`. Fix: lexicographic on full
  compartment vector, or Hilbert-curve sort per Choppala 2016.
- **M6.** `binom_logpmf` near `p ≈ 1` floor (paired with H3) — gradient
  ~1e15. `inference/obs_loglik.rs:234-240`. Fix subsumed by H3.
- **M7.** `ESS` inlines softmax instead of calling canonical helper.
  `inference/types.rs:300-307`.
- **M8.** Cholesky-times-z inlined in PMMH instead of using
  `nuts.rs::matvec_lower`. `inference/pmmh.rs:217-223` vs
  `inference/nuts.rs:105-113`. Fix: move `matvec_lower` into
  `linalg.rs`; share.

Type / FFI (5):

- **M9.** `Expr` derives `PartialEq` over `f64` → `Const(NaN) !=
  Const(NaN)`, `Const(0.0) == Const(-0.0)`; entire IR tree inherits it.
  `rust/crates/ir/src/expr.rs:5,155-170`. Fix: hand-write bitwise
  equality on `value.to_bits()`.
- **M10.** `Trajectory::default()` produces empty trajectory — degenerate
  state any consumer would crash on.
  `rust/crates/sim/src/state.rs:120`. Fix: delete the impl; force
  `Trajectory::with_initial(...)`.
- **M11.** No `.mli` files anywhere in `ocaml/lib/`; all module internals
  public. Refactoring cascades silently.
- **M12.** `ModelStructure` IR field computed by OCaml, deserialized by
  Rust, never read. `ocaml/lib/compiler/expander.ml:3369,3456,3475` vs
  `rust/crates/ir/src/model.rs:104-153`. Fix: delete in one atomic commit
  per CLAUDE.md.
- **M13.** `SimulationConfig.rng_seed` and `time_semantics` deserialized
  but never read. `rust/crates/ir/src/model.rs:68-75`. Either delete or
  actually consume.

Footguns / wiring (5):

- **M14.** Missing compartments in `init {}` default to 0 silently — typo
  `I0 = 10` instead of `I = 10` produces a flat-line trajectory.
  `ocaml/lib/compiler/expander.ml:2144-2189`,
  `rust/crates/sim/src/compiled_model.rs:764-782`. Fix: `validate.ml`
  enumerates missing compartments → diagnostic E411.
- **M15.** Default `--seed 1` indistinguishable from user-supplied
  `--seed 1`. `rust/crates/cli/src/args/mod.rs:74, :145-147`. Fix:
  `Option<u64>`; if absent, draw + log + persist.
- **M16.** `--dt 1.0` default applied silently across backends with no
  unit consistency check against `time_unit`.
  `rust/crates/cli/src/main.rs:374-378`.
- **M17.** `Cond` predicate `pred > 0.0` with no float-equality safety
  on `Time`; `cond(t == 100.0, …)` flickers across dts/seeds.
  `rust/crates/sim/src/propensity.rs:132-138`. Fix: dim-check rule
  against `Eq/Neq` on `Time`.
- **M18.** PGAS swap-acceptance rates only logged to stderr, never reach
  `PGASResult`. `inference/pgas.rs:1393, :1741-1762`. Fix subsumed by C7.

Tests (3):

- **M19.** No OCaml-side `autodiff.ml` finite-difference test. `ocaml/test/`
  has no `test_autodiff.ml`; coverage is end-to-end via
  `rust/crates/sim/tests/gradient_check.rs:20`, which exercises only
  `sir_basic` (linear-in-params rates). A sign-flip on `Pow`/`Sqrt`/
  `Cond`/`Log` derivatives would not be caught. Fix: add `test_autodiff.ml`
  per-operator FD comparison.
- **M20.** No PGAS recovery test. `rust/crates/sim/tests/if2.rs:163` and
  `tests/pmmh.rs:163` have recovery tests; PGAS does not
  (`pgas_tempering.rs` only asserts no-panic + determinism). CLAUDE.md
  names PGAS as "the production Bayesian method." Fix:
  `rust/crates/sim/tests/pgas_recovery.rs::test_pgas_sir_recovers_beta`.
- **M21.** Conservation tests don't cover the silent-clamp path of C5 —
  existing `chain_binomial_invariants.rs:49` runs `sir_basic` with
  default params where overshoot doesn't occur.

---

## What's clean (verified, no finding)

Recorded so the next reviewer doesn't re-derive:

- **Time-step centralization** (gh#53): `sim::time` is the single
  conversion entry point. Grep for `(t / dt).round()`, `(t1 - t0) / dt`
  outside `time.rs` returns nothing.
- **No parallel `step_csmc`**: PGAS calls `step_one` from
  `chain_binomial.rs`. `log_transition_density_substep` is a deliberate
  density shadow (one draws, the other scores) — heavily commented about
  its parity contract at `pgas.rs:331-339`.
- **`log_sum_exp` canonical** at `inference/types.rs:316-321`; consumers
  in `prequential.rs`, `particle_filter.rs`, `if2.rs`, `correlated_pf.rs`,
  `resampling.rs` route through it.
- **Observation likelihood**: `eval_likelihood_resolved` in
  `obs_model.rs:103-178` is the single dispatcher all backends route
  through.
- **Distribution log-pmfs**: `obs_loglik.rs` is the single home; PGAS
  and `pgas_grad` import from it.
- **`apply_interventions_at`** guards NaN `t` (`intervention.rs:59`);
  table OOB defaults to Error (`expander.ml:2100`);
  `InitialConditions::FromDistribution` is a hard error rather than
  silent zero (`compiled_model.rs:798-808`).
- **`dimcheck.ml`** covers per-day/per-week/probability distinctions at
  parse time (`dimcheck.ml:218-223`).

---

## Suggested triage order

If you can only fix five things this week, the leverage ranking:

1. **C8** — single source of truth for IR; unblocks C3 detection, H6,
   H7, M12, M13 and prevents whole classes of future drift.
2. **C1** — PGAS gradient gate (preflight check is one day; full
   derivative threading is a week).
3. **C7 + H4** — surface PGAS divergent counts and wire the unused
   `DiagnosticKind` variants. Same root cause, paired fix.
4. **C2** — IF2 MLE selection (~3 lines; gate on `it.loglik`).
5. **C6 + H5** — silent rate-eval degeneracy + unread `EvalStats`.
   Same root cause; either fail-fast or surface the counters.

C3 (balance), C4 (implicit prior), C5 (negative-count clamp) are short
fixes if you want to bundle them.

The numerical-precision items (H2, H3, H8) need careful work and a
paired regression test each; budget half a day per finding.

The DRY findings (H9, H10) are cleanup that pays off the next time
anyone touches resampling. The newtype work (H11) is the
highest-leverage change for keeping the inference math safer over the
project's lifetime but is necessarily incremental.
