---
status: implemented
date: 2026-04-20
implemented: 2026-04-20
---

# Gradient parameter index resolution

Companion to `docs/dev/reviews/2026-04-20-review-rust-design.md`
(RdM2). Replace name-keyed linear lookup in the hot gradient evaluation
loop with index-keyed array access resolved once at model construction.

---

## Problem

`compiled_model.rs:346`:
```rust
pub rate_grads: Vec<Vec<(String, ResolvedExpr)>>,
```

One outer `Vec` per transition; each inner `Vec` is a list of
`(param_name, gradient_expression)` pairs emitted by the OCaml
autodiff pass. The `String` key mirrors the OCaml representation:
the compiler operates on names, so names are what it emits.

At evaluation time, `pgas_grad.rs:99–105`:
```rust
let mut d_rate = vec![0.0; d];
for (name, resolved_grad) in &model.resolved.rate_grads[tr_idx] {
    if let Some(i) = param_names.iter().position(|pn| pn == name) {
        d_rate[i] = eval_resolved(resolved_grad, &ctx) / n_src as f64;
    }
}
```

And again at `pgas_grad.rs:205–210`. This runs once per gradient
term, per transition, per substep, across all particles and sweeps.
`param_names.iter().position(...)` is O(n_params) per call. For a
model with 8 estimated params and 20 transitions, that is up to 160
string comparisons per substep. At 100 substeps × 500 particles ×
2000 PGAS sweeps, the hot path executes ≈160 million string-comparison
scans before amortisation.

A second issue: the `if let Some(i)` silently drops gradient terms
for parameters not in `param_names`. This is correct when a parameter
is in the model but not being estimated in this run. But it is also
silent when the OCaml compiler emits a gradient for a parameter name
that is misspelled or renamed — the gradient is silently zero, NUTS
sees a flat surface for that parameter, and inference degrades without
any error. The current code has no way to distinguish "intentionally
not estimated" from "name mismatch."

---

## Proposed change

### Step 1 — resolve at `CompiledModel` construction time

Add a method to `CompiledModel::new` (or call it from the CLI
model-load path, just before PGAS launches):

```rust
/// Resolve rate_grads String keys to param_index offsets given
/// the set of names actually being estimated.
///
/// Returns a resolved form and warns if any gradient name is
/// present in the IR but absent from `estimated_names` (likely
/// a name mismatch between the compiler and the fit config).
pub fn resolve_rate_grad_indices(
    rate_grads: &[Vec<(String, ResolvedExpr)>],
    estimated_names: &[String],
    all_param_names: &[String],
) -> (Vec<Vec<(usize, ResolvedExpr)>>, Vec<String>) {
    let mut resolved = Vec::with_capacity(rate_grads.len());
    let mut unmatched = Vec::new();

    for transition_grads in rate_grads {
        let mut tr_resolved = Vec::with_capacity(transition_grads.len());
        for (name, expr) in transition_grads {
            if let Some(i) = estimated_names.iter().position(|n| n == name) {
                tr_resolved.push((i, expr.clone()));
            } else if all_param_names.contains(name) {
                // In the model but not estimated — intentional drop.
                // No warning; this is the expected case for fixed params.
            } else {
                // Not in the model at all — almost certainly a name mismatch.
                unmatched.push(name.clone());
            }
        }
        resolved.push(tr_resolved);
    }
    (resolved, unmatched)
}
```

Call site in CLI's PGAS launch path (e.g., `cli/src/fit/pgas.rs`):

```rust
let (rate_grads_indexed, unmatched) = CompiledModel::resolve_rate_grad_indices(
    &model.resolved.rate_grads,
    &estimated_names,
    &model.model.parameters.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
);
if !unmatched.is_empty() {
    return Err(format!(
        "rate_grad names not found in model parameters: {}.\n\
         This is likely a mismatch between the compiler's autodiff output\n\
         and the model IR. Recompile with `make build-ocaml`.",
        unmatched.join(", ")
    ));
}
```

### Step 2 — store alongside the original

Add a field to `CompiledModel` (or to the resolved model struct):

```rust
/// rate_grads with param positions pre-resolved to indices
/// into the estimated-parameter vector. Populated at inference
/// launch time; `None` until then (pure simulation doesn't need it).
pub rate_grads_indexed: Option<Vec<Vec<(usize, ResolvedExpr)>>>,
```

`Option<...>` keeps the field absent for pure simulation runs that
never call PGAS. Alternatively, resolve to model-param indices (not
estimated-param indices) at `CompiledModel::new` time:

```rust
/// rate_grads with param positions resolved to model-parameter indices.
/// Populated at CompiledModel::new using param_index_map.
/// Keys are model param indices; the PGAS driver re-indexes to
/// estimated-param positions in O(n_estimated) once at launch.
pub rate_grads_model_indexed: Vec<Vec<(usize, ResolvedExpr)>>,
```

This second form is cheaper to construct (no estimated set needed)
and reduces the launch-time work to a single O(n_params × n_estimated)
pass rather than an O(n_transitions × n_grads × n_params) loop.

### Step 3 — update the hot loop

`pgas_grad.rs:99–105` becomes:

```rust
let mut d_rate = vec![0.0; d];
for &(param_idx, ref resolved_grad) in &rate_grads_indexed[tr_idx] {
    d_rate[param_idx] = eval_resolved(resolved_grad, &ctx) / n_src as f64;
}
```

No string comparison, no `Option` unwrap, no branch. The `for` loop
body is now two operations: `eval_resolved` (the expensive one,
unchanged) and an array store.

Same update at `pgas_grad.rs:205–210`.

---

## Correctness invariant

The resolved index `param_idx` is an index into the `estimated_names`
slice passed at resolution time. The PGAS gradient caller must pass the
same `estimated_names` at resolution time and at evaluation time. This
is already the case: both are derived from the same `FitConfigV2`
parameter list in a single launch. If the estimated set ever changes
mid-run (it doesn't today), re-resolution would be needed.

---

## Test plan

**Unit test — resolution is correct:**
```rust
#[test]
fn gradient_indices_resolve_to_correct_positions() {
    let grads: Vec<Vec<(String, ResolvedExpr)>> = vec![
        vec![("beta".into(), ResolvedExpr::Const(1.0)),
             ("gamma".into(), ResolvedExpr::Const(2.0))],
    ];
    let estimated = vec!["gamma".into(), "beta".into()]; // reversed order
    let (indexed, unmatched) = resolve_rate_grad_indices(
        &grads, &estimated, &estimated
    );
    assert!(unmatched.is_empty());
    // gamma is index 0 in estimated, beta is index 1
    assert_eq!(indexed[0][0].0, 1); // beta
    assert_eq!(indexed[0][1].0, 0); // gamma
}
```

**Unit test — unmatched name is caught:**
```rust
#[test]
fn gradient_name_mismatch_is_reported() {
    let grads = vec![vec![("typo_param".into(), ResolvedExpr::Const(0.0))]];
    let estimated = vec!["beta".into()];
    let all_params = vec!["beta".into()];
    let (_, unmatched) = resolve_rate_grad_indices(&grads, &estimated, &all_params);
    assert_eq!(unmatched, vec!["typo_param"]);
}
```

**Regression test — gradient values unchanged:**
The existing `gradient_check.rs` test file in `sim/tests/` runs finite-
difference gradient verification. After this change, the gradient values
computed by `pgas_grad` must be identical to before (only the data
structure changed). Run `cargo test --test gradient_check` before and
after to confirm.

---

## Scope

- `compiled_model.rs`: add `rate_grads_model_indexed` field + populate
  in `CompiledModel::new`.
- `pgas_grad.rs`: update two loop bodies (~6 lines each).
- CLI `fit/pgas.rs`: add resolution call at launch; propagate unmatched
  error.
- `sim/tests/gradient_check.rs`: confirm no numeric change (regression).

The original `rate_grads: Vec<Vec<(String, ResolvedExpr)>>` can be
retained for debugging and for the OCaml-compatibility path; the indexed
form is additive, not a replacement.
