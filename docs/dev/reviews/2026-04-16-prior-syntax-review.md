---
status: review
date: 2026-04-16
reviewer: self
scope: prior-syntax PR (commits 9280547 → 53e3079)
severity: mixed — none critical for v0.3 internal use, several must-fix before downstream
---

## Prior-Syntax Implementation Review

Self-review of the `~ prior(...)` feature end-to-end: proposal, OCaml
compiler changes, Rust Prior module, runner resolution, `--draws prior`,
spec updates. The feature works for the happy path — the golden fixture
round-trips and the CLI produces reasonable prior predictive draws. But
the input-validation surface is thin, and several "loose semantics" slip
through compilation into runtime panics or silent wrong answers.

CLAUDE.md is explicit: "If the compiler accepts it, the behavior must be
fully specified and intentional." Several items below violate that.

### Confirmed Bugs (verified with ad-hoc .camdl files)

**1. `mu = log(0.3)` fails — the proposal advertises a feature that
doesn't work.** The proposal says "arithmetic of literals is fine:
`mu = log(0.3)`." Actual behavior: E230 is raised because the parser
emits `EFuncCall("log", ...)` but `is_const_expr` only accepts
`EConst/EUnit/EUnOp/EBinOp`. `eval_const_expr` has `EUnOp(Log, _)` cases
but they're dead code — the parser never produces that form.

Fix options: (a) extend `is_const_expr`/`eval_const_expr` to handle
`EFuncCall` for pure math functions (`log`, `exp`, `sqrt`, `abs`, etc.),
or (b) update the proposal to say only literals + arithmetic work and
remove the `log(0.3)` example. (a) is better — users *will* write
`mu = log(0.3)` to encode median-based priors.

**2. Unknown/extra kwargs silently ignored.** Tested:

```
beta : rate in [0.01, 2.0] ~ log_normal(mu = -1.0, sigma = 0.5, extra = 99)
```

Compiles cleanly. `extra` is dropped. This is exactly the "loose
semantics" pattern CLAUDE.md forbids. E230/E231/E232 exist but no check
for extraneous kwargs. Should emit E233 or reuse E232 with a
per-distribution signature.

**3. Duplicate kwargs silently take the first.** `log_normal(mu = -1, mu = -5, sigma = 0.5)`
emits `mu = -1.0` in the IR without warning. `List.assoc_opt` returns
the first match. A clear bug for users who copy-paste.

**4. Invalid parameter values compile to panics.** Tested:

```
~ uniform(lower = 5.0, upper = 1.0)   # inverted — no error
~ beta(alpha = -1.0, beta = 2.0)       # negative shape — no error
~ gamma(shape = 0.0, rate = 1.0)       # zero shape — no error
~ normal(sigma = -1.0)                 # negative sd — no error (not tested above, but clearly allowed)
~ exponential(rate = 0.0)              # zero rate — no error, runtime div-by-zero
```

All compile. `rand_distr::Gamma::new(...).unwrap()` in `main.rs` will
panic at `--draws prior` time. `Gamma::log_density` with `shape = 0`
gives `NaN` during inference. The user sees a generic panic trace, not
a compiler diagnostic. This is the exact UX failure CLAUDE.md warns
about: "A bad error message is a bug."

Fix: add `validate_prior_args(ps_name, args)` in the expander that
checks per-distribution constraints (shape > 0, sd > 0, lower < upper,
alpha/beta > 0, 0 < prob < 1 for probability-kind params, etc.). Raise
E234 with a specific message for each failure.

**5. Silent bounds clamping changes the sampled prior.** In
`sample_from_prior_dist`, the final line is `raw.clamp(lo, hi)`. A user
who writes `beta : rate in [0.01, 2.0] ~ log_normal(mu = 3, sigma = 0.5)`
(prior centered at ≈20) will get every sample clamped to 2.0 — a delta
at the upper bound, not a log-normal. This is not documented and will
mislead anyone doing prior predictive checks ("why is my prior predictive
so degenerate?").

Options:
  (a) Reject draws outside bounds (truncation) via rejection sampling;
      warn if rejection rate > threshold.
  (b) Warn loudly if >X% of samples hit the clamp; proceed with clamped
      values.
  (c) At minimum: log a warning with the clamp fraction.

The correct semantic is (a) — if the parameter has bounds, the prior
should be truncated, not reflected/clamped. Inference code (via
`Transform::Log`/`Logit`) handles bounds by reparameterization to an
unconstrained space, which is different.

**6. `Prior::Normal` and `Prior::TransformedNormal` missing `-0.5·ln(2π)`
normalization.** Other variants (HalfNormal, Beta, Gamma) include the
full normalization constant. Missing the `2π` term doesn't affect MH
acceptance ratios (it cancels), but it poisons any absolute
log-density comparison — model comparison metrics (WAIC, LOOIC) or
cross-prior diagnostics will silently mis-rank. One-line fix:

```rust
Prior::Normal { mean, sd } => {
    let z = (natural - mean) / sd;
    -0.5 * z * z - sd.ln() - 0.5 * (2.0 * std::f64::consts::PI).ln()
}
```

**7. Diagnostics missing source location and parameter name.** Every
error in `resolve_prior_spec` uses `Diagnostics.no_loc` and omits the
parameter name:

```
error[E230]: prior argument 'mu' must be a compile-time constant
  = note: In ~ log_normal(...), the argument 'mu' is not a constant
          expression.
```

If a model has priors on 20 parameters and two of them have bad args,
the user has no way to tell which parameters. CLAUDE.md: "Show where
(source location, transition name, parameter name)." The `prior_spec`
AST node needs a location field threaded through from the parser, and
`resolve_prior_spec` needs the enclosing param name passed in. At
minimum, thread the parameter name.

### Asymmetries and Dead Code

**8. `parse_prior` in `runner.rs` supports only 4 distributions.**
Model IR supports 7 (`uniform`, `normal`, `log_normal`, `half_normal`,
`beta`, `gamma`, `exponential`); `parse_prior` for fit.toml overrides
supports only `flat`, `lognormal`, `normal`, `beta`. So a user cannot
override a model IR `gamma`/`half_normal`/`exponential` prior via
fit.toml — their override string silently fails with a warning, and
inference falls through to the model IR. Asymmetric and surprising.

Fix: extend `parse_prior` to cover all 7 distributions, using the same
naming (`half_normal`, `gamma`, `exponential`).

**9. `--scenario baseline` doesn't feed preset values into the
`--draws prior` default-value check.** `generate_prior_draws_from_ir`
calls `util::load_model` which does NOT apply presets — so a parameter
set by `scenarios.baseline.set` is still seen as `value = None` and the
command errors "no prior and no default." Workflows like "sample from
priors on structural parameters (beta, gamma), hold N0 at its baseline"
are blocked unless the user gives N0 a degenerate prior.

Fix: resolve scenarios (at least the CLI-selected one) before the
missing-prior check.

**10. Dead code: the catch-all `PIndexed` branch in `expand_parameters`.**
The parser only produces `PIndexed { pdims = [dim]; ... }` (single-dim
index). The fallback branch `| PIndexed { pname; pprior; _ } -> ...`
is unreachable in the current grammar. Either delete it or gate it
behind a compile-time error.

**11. Dead code: `eval_const_expr` handling of `EUnOp(Log|Exp|Sqrt|...)`.**
Per issue #1, the parser produces `EFuncCall` for these, not `EUnOp`.
The `EUnOp(Log, _)` match arms are unreachable.

**12. Hardcoded placeholder after E232.** If an unknown distribution is
given, the expander emits `Ir.Uniform { lower = 0.0; upper = 1.0 }` as
a placeholder to keep going. If errors are suppressed (debug builds,
tests with `Result.ok`), this placeholder leaks into the IR. Better to
return `None` or propagate a result-type through.

### Spec / Doc Issues

**13. Misleading spec comment.** Language spec §21.2 example says:

```
N0 : count in [100, 1_000_000]  # no prior — fixed in inference
```

But `N0` has no `value` and no prior — "fixed in inference" requires
`--params` or `fit.toml [fixed]`. The comment implies a free lunch.
Reword: `# no prior — must be supplied via --params or [fixed] in fit.toml`.

**14. Proposal advertises `mu = log(0.3)` as working (per #1).**

**15. Run spec §12 says "For --draws prior, every parameter needs one
[prior]"** — but the implementation also accepts `p.value.is_some()`.
Clarify: "every parameter needs either a prior or a fixed value in
the IR."

### Test Gaps (must-add before downstream use)

**High priority — cover advertised error modes:**

- `E230` non-const arg: `~ log_normal(mu = some_var, sigma = 0.5)` → error
- `E231` missing required kwarg: `~ log_normal(mu = 1.0)` (no sigma) → error
- `E232` already covered, but add: missing distribution name, typo'd name with suggestion
- Extra/unknown kwarg: once fixed, test `log_normal(mu, sigma, extra)` → error
- Duplicate kwarg: once fixed, test `log_normal(mu, mu, sigma)` → error
- Invalid values: `uniform(lo=5, up=1)`, `beta(alpha=-1, ...)`, `gamma(shape=0, ...)`,
  `normal(sigma=-1)`, `exponential(rate=0)` → all errors

**Medium priority — cover untested distributions on OCaml side:**

- `~ uniform(lower, upper)` parse test
- `~ normal(mu, sigma)` parse test
- `~ exponential(rate)` parse test

**Medium priority — integration:**

- End-to-end PGAS run with IR-declared priors (no fit.toml `[estimate]`
  prior): verify the prior is applied in the posterior. Currently only
  `resolve_prior_precedence_chain` tests this as a unit.
- End-to-end: fit.toml prior overrides a model IR prior during actual
  inference — verify posterior differs vs no-override baseline.
- `--draws prior` with `--scenario` once the scenario-interaction bug
  is resolved.

**Low priority but useful:**

- `sample_from_prior_dist` produces draws from the stated distribution
  (e.g., ~3σ spread on Normal, positivity on HalfNormal, unit interval
  on Beta). Quick statistical test with N=10000 samples, loose
  tolerance on mean/variance.
- Arithmetic const arg: `mu = -1.0 * 2.0 + 0.5` should work.
- Once #1 is fixed: `mu = log(0.3)` should work.
- Reproducibility across seeds: same seed → same draws, different seeds
  → different draws.
- `~ fixed(...)` rejected at parse time (per proposal: "not exposed as
  DSL syntax"). Currently there's no explicit test that this fails.

### Priorities for downstream readiness

Must fix before external users touch this:
  1. Extra/duplicate kwarg silent drop (loose semantics)
  2. Invalid parameter values not validated (runtime panic)
  3. Bounds clamping behavior (wrong prior, silent)
  4. Diagnostics missing parameter name
  5. `parse_prior` asymmetry (fit.toml ≠ IR)

Should fix:
  6. Normal log-density normalization constant
  7. Scenario preset interaction with --draws prior check
  8. `log(x)` in prior args — either make it work or remove from proposal
  9. Spec comment "fixed in inference"

Can defer:
 10. Dead code cleanup (non-functional)
 11. Hardcoded placeholder on error (no observed failures)
