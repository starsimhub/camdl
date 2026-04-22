---
date: 2026-04-21
status: design-plan
related: ../2026-04-21-malaria-model-features.md (#3), ./multi-source-transitions-spike.md
---

# Gate 2 plan: hierarchical log-prior evaluation

Gate 2 is where the math first becomes correctness-critical.
Gate 1 shipped the language surface and IR classification —
`parameter.hierarchical` is a structural field, not consumed by
inference yet. Gate 2 wires it into the log-density evaluator: given
current values of hyperparameters, compute `log p(leaf | hyper)`.

Stakes note: camdl's hierarchical posteriors feed
malaria-policy decisions. A +σ² bias like the
recent `TransformedNormal` Jacobian bug (IC3) would skew every
indexed-parameter credible interval in every multi-site fit. Gate 2
must leave no such class of bug behind.

## Endpoint we're driving toward

The Garki fit vignette — downstream agent — needs PMMH or PGAS to
recover `mu_r1, sigma_r1, r1[age]` within 2σ on synthetic data.
Gate 2 is the math; Gate 3 wires it into the three inference
algorithms. Gate 2 is complete when:

1. **Hierarchical log-density equals scipy** for every supported
   distribution family to ≤ 1e-10 relative error on a battery of
   test points.
2. **Cycles and unknown references are compile errors**, not
   runtime NaNs.
3. **Bounds violations return −∞**, never garbage.
4. **Transform interaction is correct** — the existing `Transform::Log`
   Jacobian semantics compose cleanly with hierarchical priors
   (specifically, the IC3 fix applies).

## Risk catalog

Grouped by class. Every risk has a named test obligation — Gate 2
is not complete without the corresponding test passing.

### Class A — density formula correctness

| # | Risk | Test |
|---|---|---|
| A1 | Parameterisation confusion (e.g. `normal(mu, σ)` vs `normal(mu, σ²)` or log-scale vs natural) | `test_density_matches_scipy_{dist}` per family |
| A2 | Missing normalisation constant — MAP unaffected but log-marginal-likelihoods wrong | scipy oracle includes the constant; same test catches it |
| A3 | Jacobian double-count (IC3 class bug): `TransformedNormal` returns natural-scale density; caller adds `log|dθ/dz|`. Hierarchical version must follow the same contract | `test_transform_jacobian_no_double_count`: same param in z-scale and θ-scale evaluations differ by exactly `log|dθ/dz|` |
| A4 | Sign or bound errors: half-normal at x<0 → −∞, beta at x∉[0,1] → −∞ | `test_out_of_support_returns_neg_inf` per family |

### Class B — hyperparameter lookup at eval time

| # | Risk | Test |
|---|---|---|
| B1 | Stale hyperparameter: leaf density uses old value after a sampler move updated hyper | integration test: after moving hyper by Δ, leaf log-density changes by the analytically-expected amount |
| B2 | Name→index mismatch: `Expr::Param("mu_r1")` looks up wrong slot in the estimated-params vector | unit test: shuffle estimated-param order, assert density unchanged |
| B3 | Expression evaluation in args has bug (e.g. `log(mu_r1)` as a hyperparent-derived arg) | `test_density_with_expression_hyperparent_args`: `mu = log(0.3) + mu_r1` correctness |
| B4 | Uninitialised hyperparent value used before first sample | assert compile-time: leaf initial_value must be derivable OR explicit; runtime: missing value → clear error, not NaN |

### Class C — reference-graph safety

| # | Risk | Test |
|---|---|---|
| C1 | Self-reference: `alpha ~ normal(mu = alpha, ...)` | `test_self_reference_rejected` (compile error) |
| C2 | Cycle: `a ~ f(b); b ~ f(a)` | `test_cycle_rejected` (compile error) |
| C3 | Deep chain: `c ~ f(b); b ~ f(a); a ~ Normal(...)` — evaluation must respect topological order | `test_three_level_hierarchy_evaluates_correctly` (golden-value test) |
| C4 | Leaf references compartment / table / let-binding → undefined semantics | already caught by E100 in Gate 1 — pin with a test, don't let silent-accept regress |

### Class D — interaction with existing machinery

| # | Risk | Test |
|---|---|---|
| D1 | Leaf has `Transform::Log` (typical for rates); hierarchical density on z-scale vs θ-scale | `test_log_transformed_leaf_matches_scipy` on both scales |
| D2 | Leaf has `bounds = [a, b]`; out-of-bounds proposal returns −∞ through the hierarchical path | `test_leaf_bounds_return_neg_inf` |
| D3 | Scenario override `--set mu_r1 = 0.5` applies to hyperparent; leaf density re-evaluates correctly | `test_scenario_override_propagates_to_leaves` |
| D4 | `fit.toml [estimate]` override on a leaf parameter's prior — overrides the hierarchical binding | decide: prohibit (E-code) or document override semantics. Test either way. |

### Class E — numerical stability

| # | Risk | Test |
|---|---|---|
| E1 | `σ_hyper → 0`: leaf density explodes. Sampler normally prevents via `Transform::Log`, but defence-in-depth: clamp or assert | `test_sigma_near_zero_stability` |
| E2 | `(x − μ)² / σ²` catastrophic cancellation when x ≈ μ | scipy oracle test covers this at multiple x-μ separations |
| E3 | `lgamma` overflow for very large `shape` in Gamma/Beta | scipy oracle at `shape = 1e4` |
| E4 | NaN propagation: one bad hyperparent value should poison only that proposal, not the whole chain state | `test_nan_isolated_to_current_proposal` |

## Design decisions to make before code

### D-1: Where does log_prior dispatch to hierarchical vs plain?

Option A: single `log_density(param, natural, transformed, env)`
function, branches internally on `parameter.hierarchical.is_some()`.
Cleanest for callers; all existing call sites just pass the env.

Option B: build a flat `Prior` enum once at fit-config load time
that already has the right args (looked up from env). Requires
re-building the Prior struct on every sampler step because args
depend on current hyperparameter values.

**Decision**: Option A. Inference code already passes a
`&[f64]` of estimated-param values; add one more param to
`log_density` that's a slice-name map (or equivalent) so we can
resolve `Expr::Param(name)` → current value. Minimal refactor to
call sites.

### D-2: Where does the reference-graph walk live?

Option A: compile-time in the expander (catches cycles at .camdl
compile, produces a topo-ordered list in the IR).

Option B: runtime at fit-config load, after .camdl has been
compiled.

**Decision**: Option A. The graph is static — it's defined by the
.camdl source and doesn't change. Catching cycles at compile time
gives better errors. The IR gains an optional field: a
topologically-ordered list of parameter names indicating eval order.

### D-3: How do we handle the bounds-respect contract?

Current plain-prior behaviour: `log_density(θ)` returns finite
density; bounds are enforced separately by the sampler. For
hierarchical, same contract: the hyper's current value might be
out of its bounds, which makes the leaf's mu nonsense, but the
density function faithfully returns whatever it computes. The
sampler is responsible for rejecting.

**Decision**: match existing contract. `log_density` trusts args.
Document.

### D-4: Smart types?

User asked whether phantom types could encode correctness
invariants. Two candidates with real payoff:

**Phantom type on Prior::log_density**: `Scale` phantom (Natural vs
Transformed) prevents mixing. Would have caught IC3 at compile
time. Worth ~100 lines of type plumbing in `inference/prior.rs`.

**Role tag on Parameter**: `Plain | Hyper | Leaf { parents: Vec<Name> }`.
Currently derived from the `hierarchical` field; making it an
explicit enum would let the compiler check that every leaf is
resolved before leaves that reference it. But OCaml and Rust can't
enforce "topo-order is a valid order" through types alone.

**Decision for Gate 2**: add the `Scale` phantom on Rust-side
`Prior::log_density`. Leave the role tag derived. Revisit role tag
for Gate 3 if inference wiring becomes confusing.

## Test plan summary

Gate 2 ships with these tests, in this order:

1. **A1–A4 density-formula battery (7 tests)** — one per distribution
   family. Each asserts: log_density at ≥ 8 points matches
   scipy.stats.{dist}.logpdf to 1e-10 relative error. Points chosen
   to include bulk, tails, near-boundary, and out-of-support.

2. **A3 Jacobian no-double-count (1 test)** — reproduce IC3 scenario
   with a hierarchical leaf: natural-scale and z-scale densities
   differ by exactly `log|dθ/dz|`. Pins the class of bug that hit
   TransformedNormal.

3. **B1–B4 hyperparent plumbing (4 tests)** — lookup semantics.

4. **C1–C4 graph safety (4 tests)** — compile-time rejection of
   self-reference and cycles; topological evaluation of deep chains.

5. **D1–D4 existing-feature interaction (4 tests)** — transform
   composition, bounds, scenario overrides, fit.toml override
   precedence.

6. **E1–E4 numerical stability (4 tests)** — near-zero σ, catastrophic
   cancellation, lgamma overflow, NaN isolation.

7. **Integration test: 2-level Normal-Normal model** —
   synthetic data from known μ_hyper, σ_hyper. Evaluate
   log-posterior at several parameter vectors. Compare against
   analytical posterior for the Normal-Normal conjugate case.
   Recovery within 2σ on 100 synthetic datasets.

Total: ~24 new tests before Gate 2 is called done.

## What gets committed at Gate 2

| File | Change |
|---|---|
| `rust/crates/ir/src/validate.rs` | Add cycle detection + topological ordering on hierarchical priors. New IR field `hierarchical_eval_order: Vec<String>`. |
| `rust/crates/sim/src/inference/prior.rs` | Extend `Prior::log_density` to accept an env (param name → current value). New `HierarchicalPrior` variant. Phantom `Scale` type. |
| `rust/crates/sim/src/inference/hierarchical.rs` | New module: expression evaluator for `Expr::Param` resolution against current values. |
| `rust/crates/sim/tests/hierarchical_log_density.rs` | The 24-test battery. |
| `ocaml/lib/compiler/expander.ml` | Compile-time cycle check; topo-order as IR field. |
| `ocaml/lib/ir/ir.ml` | `hierarchical_eval_order` field on Model. |

## Non-goals for Gate 2

- **Gradient of hierarchical log-density**. Needed for NUTS; deferred
  to Gate 3 when we touch `pgas_grad.rs`. Gate 2 covers PMMH (which
  only needs log-density, not its gradient).
- **Non-centered reparameterisation**. A sampling-efficiency concern,
  not a correctness one. Ships with Gate 3 inference wiring.
- **Multi-level (> 2-level) hierarchies**. Parser already accepts
  them via the same graph machinery, but a 3-level test fixture
  is Gate 2's job; performance profiling of deep hierarchies is
  deferred.

## Rough effort

- Expander cycle check + topo order: 2 hours
- Rust prior.rs extension (env-aware log_density): 3 hours
- Hierarchical module + tests A1–A4: 4 hours
- Tests B1–B4, C1–C4: 3 hours
- Tests D1–D4, E1–E4: 3 hours
- Integration test Normal-Normal conjugate: 3 hours
- Smart-types (Scale phantom): 2 hours
- Spec + changelog: 1 hour

**Total**: ~21 hours. About 3 focused days. Longer than Gate 1 because
every risk class gets a dedicated test before code.

## Sequence for next session

1. Write all 24 test stubs first (TDD red).
2. Expander cycle check (unblocks C1–C4).
3. Rust log-density env-aware extension (unblocks A1–A4, B1–B4).
4. Wire transforms + bounds (D1–D2).
5. Integration test last — proves the whole thing composes.
