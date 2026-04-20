# Simplex Parameter Groups

**Status (2026-04-19):** proposal / design doc only. The
cross-language IR plumbing (`parameter_groups` field,
`simplex_member` param_kind) was removed in M17/m24 of the
2026-04-19 compiler review because no parser rule produced it and
no runtime code consumed it end-to-end. The IF2 SimplexGroup
runtime struct still exists and is instantiated with vec![]
everywhere. When this feature is implemented, the IR plumbing will
need to be re-added atomically — do not refer to this doc as a
live schema description.

## What

Parameters that sum to 1 (initial compartment fractions) should
be declared as a group in the DSL. The inference engine uses
barycentric (log-ratio + softmax) transform jointly, guaranteeing
the sum constraint is maintained during IF2 perturbation.

## DSL syntax

```camdl
parameters {
  R0    : positive in [1, 100]
  sigma : rate     in [0.01, 0.5]

  (s0, e0, i0, r0) : simplex {
    s0 in [0.01, 0.10]
    e0 in [1e-6, 0.01]
    i0 in [1e-6, 0.01]
    r0 in [0.90, 0.99]
  }
}
```

Semantics: s0 + e0 + i0 + r0 = 1. All members explicit, including
the residual. Softmax guarantees the sum constraint by construction.
Per-member bounds are soft (documentation + random init range, not
enforced post-softmax). Matches pomp's `barycentric` exactly.

## OCaml changes

### AST (types.ml or equivalent)

New parameter declaration variant:

```ocaml
| PSimplex of {
    members: (string * param_bounds option) list;
  }
```

### Parser

Parse `(name, name, ...) : simplex { name in [lo, hi], ... }`.
The parenthesized tuple of names comes first, then `: simplex`,
then a braced block with per-member bounds.

**Check:** Does the existing parser have a lookahead issue with
`(` at the start of a parameter declaration? Currently params
start with an identifier. The `(` is new. May need a keyword
prefix like `simplex (s0, e0, i0) { ... }` if the tuple-first
syntax creates ambiguity. Pick whichever parses cleanly.

### Expander

Emit each simplex member as a normal parameter (with bounds,
`param_kind = "simplex_member"`). Additionally emit a
`ParameterGroup` record.

```ocaml
(* For each member: emit as normal parameter *)
{ name = "s0"; bounds = Some (0.01, 0.10);
  param_kind = Some "simplex_member"; ... }

(* Plus one group record *)
{ kind = "simplex"; members = ["s0"; "e0"; "i0"] }
```

### IR serialization

Add `parameter_groups` field to the model JSON:

```json
{
  "parameters": [ ... ],
  "parameter_groups": [
    { "kind": "simplex", "members": ["s0", "e0", "i0"] }
  ]
}
```

## Rust IR changes

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct ParameterGroup {
    pub kind: String,
    pub members: Vec<String>,
}

// On ir::Model:
#[serde(default)]
pub parameter_groups: Vec<ParameterGroup>,
```

`#[serde(default)]` means old IR files without the field parse
fine (empty vec).

## Rust inference changes

### IF2 perturbation loop (if2.rs)

Before the per-parameter perturbation, handle simplex groups:

```rust
for group in &simplex_groups {
    // Forward: fractions → log-ratios
    let fracs: Vec<f64> = group.indices.iter()
        .map(|&idx| particle_params[i][idx].max(1e-300))
        .collect();
    let sum: f64 = fracs.iter().sum();
    let log_ratios: Vec<f64> = fracs.iter()
        .map(|&f| (f / sum).max(1e-300).ln())
        .collect();

    // Perturb in log-ratio space
    let perturbed: Vec<f64> = log_ratios.iter()
        .zip(&group.rw_sds)
        .map(|(&z, &sd)| z + rng.normal() * sd * cooling_now)
        .collect();

    // Inverse: softmax (numerically stable)
    let max_z = perturbed.iter().cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let exp_z: Vec<f64> = perturbed.iter()
        .map(|&z| (z - max_z).exp()).collect();
    let exp_sum: f64 = exp_z.iter().sum();
    for (j, &idx) in group.indices.iter().enumerate() {
        particle_params[i][idx] = exp_z[j] / exp_sum;
    }
}

// Then perturb non-group params as usual
for spec in if2_params {
    if !in_simplex_group[spec.index] { ... }
}
```

### build_if2_params

Simplex members get `transform = None` (the group handles it).
Their `rw_sd` is still per-parameter — each member can have a
different perturbation scale in log-ratio space.

### Resolving group indices

At setup time, resolve group member names to parameter indices:

```rust
struct SimplexGroup {
    indices: Vec<usize>,   // into the params array
    rw_sds: Vec<f64>,      // per-member, on log-ratio scale
}

let simplex_groups: Vec<SimplexGroup> = model.parameter_groups.iter()
    .filter(|g| g.kind == "simplex")
    .map(|g| {
        let indices: Vec<usize> = g.members.iter()
            .map(|name| compiled.param_index[name.as_str()])
            .collect();
        let rw_sds: Vec<f64> = indices.iter()
            .map(|&idx| /* from ParamSpec or auto */)
            .collect();
        SimplexGroup { indices, rw_sds }
    })
    .collect();
```

## What to check

1. **Sum constraint after softmax:** After every perturbation,
   assert `group.indices.iter().map(|&i| particle_params[p][i]).sum() ≈ 1.0`.
   This should hold by construction (softmax output sums to 1)
   but verify with a test.

2. **Softmax stability:** The max-subtraction trick prevents
   overflow. Verify with extreme inputs: all log-ratios at -300,
   or one at +300 and rest at -300.

3. **IVP interaction:** Simplex members are almost always IVP
   params (initial fractions). Verify they're perturbed at t=0
   only, like other IVP params.

4. **Init block consistency: RESOLVED.** All members are explicit,
   including the residual (r0). Softmax over (s0, e0, i0, r0)
   guarantees sum = 1 by construction. No implicit residual, no
   subtraction in init block, no negative R. Matches pomp exactly.
   Per-member bounds are soft (documentation + random init range,
   not enforced post-softmax — clamping would break sum-to-1).

5. **Indexed simplex (future):** `(S0[p], E0[p], I0[p]) : simplex`
   for spatial models. Not needed now but the IR/Rust design
   should not preclude it. The `ParameterGroup.members` field
   could hold patterns like `["S0_*", "E0_*", "I0_*"]` later.
   For now, just scalar names.

6. **Parser ambiguity:** Test that the parser handles both
   regular params and simplex groups in the same `parameters {}`
   block without ambiguity.

7. **Golden files:** Regenerate. The new `parameter_groups` field
   appears in all IR JSON files (as empty array for models without
   simplex groups).

## Tests

- **OCaml:** Parse simplex block, verify IR has parameter_groups
  with correct members and per-member bounds.
- **Rust unit:** Softmax round-trip: fracs → log-ratio → perturb
  → softmax → verify sum = 1, verify each frac in bounds.
- **Rust integration:** Small 3-compartment SIR with simplex
  init fracs. Run 10 IF2 iterations. Verify: no panics, all
  particles have fracs summing to 1, loglik is finite.

## Priority note

For He et al., independent logit on s0/e0/i0 works because the
fractions are tiny (~3%, 0.005%, 0.005%) and the sum constraint
is never violated in practice. The simplex transform matters for
models where initial fractions are larger or where many
compartments share a budget (polio patches, age-structured models).
Implement and test now, but the vignette can proceed with the
current independent logit — the simplex is a correctness
improvement, not a blocker.
