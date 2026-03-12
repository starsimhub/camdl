# Runtime Design Notes

This document records design decisions for the `camdl` Rust simulation runtime
(`rust/crates/sim/`). Future agents and contributors should read this before
modifying the core simulation loop or EKRNG.

## 1. EKRNG Two-Type Design

`EkRng` and `StatefulRng` are **separate types**, not one type with an optional
key. This is intentional:

- `EkRng::poisson_keyed(event_key, counter, lambda)` — each call is fully
  determined by `(seed, event_key, counter)`. Stateless. Order-independent. The
  fundamental EKRNG property: drawing event A then B gives exactly the same
  per-event values as drawing B then A.

- `StatefulRng` — conventional streaming PRNG for transitions with no
  `event_key` (backward compatibility fallback). Its draws are order-dependent.

**Why separate types?** The compiler prevents callers from accidentally using
`StatefulRng` for an event that has an `event_key`, or `EkRng` with global
state. Type-system enforcement of the EKRNG contract.

**Implementation:** `EkRng` hashes `(seed, event_key, counter)` with `ahash` to
get a `u64`, then expands it to a 256-bit ChaCha8 seed. This gives a completely
independent PRNG stream per `(key, counter)` triple.

**Why ahash?** Non-cryptographic, fast, good avalanche. We do not need
cryptographic security — we need low collision probability and good mixing. The
`u64 → [u8; 32]` expansion uses three multiplications with different constants
to fill the seed uniformly.

## 2. CompiledModel Pre-Computation Strategy

`CompiledModel::new` builds all index maps once at model load time:

- `comp_index: HashMap<String, usize>` — compartment name → global index
- `param_index: HashMap<String, usize>` — parameter name → slice index
- `time_func_index`, `table_index` — same pattern
- `transition_stoich: Vec<Vec<(usize, i64)>>` — stoichiometry pre-resolved to
  local integer compartment indices (real compartments excluded)
- `global_to_int`, `global_to_real` — O(1) dispatch from global → local index

**Hot-loop invariant:** `eval_expr` and `eval_propensities` never call
`HashMap::get` on a string. All string→index lookups happened at load time. The
steady-state path is pure arithmetic on `Vec<f64>` and `Vec<i64>`.

## 3. Propensity Evaluator

`eval_expr` is a recursive tree walk over `ir::Expr`. Key decisions:

- **Div(a, 0) = 0.0** — not an error. The spec's `Cond` pattern uses
  `Div(Pop("I"), PopSum(...))` guarded by `Cond(Pop("I"), rate, 0)`. When `I=0`,
  the `Cond` short-circuits before `Div` is reached. But if `Div` is ever called
  with denominator 0 (e.g., total population = 0), returning 0 is safer than
  NaN/Inf and matches the intended semantics.

- **Cond semantics:** `pred > 0 → then; pred ≤ 0 → else`. Zero is **falsy**.
  This is the exact spec wording. Matches the use case:
  `Cond(Pop("I"), rate, 0)` returns 0 when `I=0` because `Pop("I") = 0 ≤ 0`.

- **Allocation-free:** No `Vec` allocation in `eval_expr` (except for `PopSum`
  iteration which is a loop, not an allocation). `eval_propensities` takes
  `out: &mut Vec<f64>` and clears+fills in place.

## 4. Gillespie SSA Implementation

Standard Gillespie algorithm with:

1. Evaluate all propensities → total rate Λ
2. Draw waiting time τ ~ Exp(Λ) via stateful RNG (clock draw)
3. Select transition proportional to propensity (also stateful)
4. Advance real state via RK4 over τ (PDMP approximation — see §5)
5. Apply stoichiometry
6. Clamp negative compartments, log warning

**v0.1 limitation:** The stateful RNG is used for the global clock and
transition selection. EKRNG is used for tau-leaping and chain-binomial (keyed by
transition name + step counter). Full per-event EKRNG for Gillespie requires
using the keyed exponential draw for each transition individually (exact SSA via
next-reaction method), deferred to v0.2.

**Intervention handling:** When the drawn event time overshoots an intervention,
the simulator discards the draw, advances to the intervention time, applies the
state modification, and restarts — never "resumes" with remaining time. The
exponential distribution is memoryless; the restart is statistically correct.

## 5. PDMP Approximation for Real Compartments

Real compartments (e.g., the environmental bacteria concentration `W` in the
cholera model) follow ODEs between stochastic jumps. The exact solution is a
Piecewise-Deterministic Markov Process (PDMP).

**v0.1 approximation:** Treat propensities as locally constant within each
Gillespie step. Advance real state via RK4 over the inter-event time, then
re-evaluate propensities from scratch at the new (integer + real) state.

This approximation is valid when real state changes slowly relative to the mean
inter-event time. For the cholera `W` compartment at typical parameter values
(decay rate δ ≈ 0.2 per day, mean inter-event time ≪ 1 day), the error is
negligible.

**TODO(v0.2):** Replace with exact PDMP thinning (Rejection/First-Passage
algorithm). See features-roadmap.md for design notes.

## 6. Non-Negativity Clamping

After every state update (Gillespie event, tau-leap step, chain-binomial step),
integer and real state is clamped to ≥ 0. A `log::warn!` is emitted if clamping
occurs — this indicates the step is too large (tau-leaping) or a propensity bug
(Gillespie).

`debug_assert!` checks non-negativity in debug builds (zero cost in release).

## 7. Test Locations

Integration tests live in `rust/crates/sim/tests/`:

```
cargo test -p sim --test expr_eval
cargo test -p sim --test ekrng_determinism
cargo test -p sim --test gillespie_invariants
cargo test -p sim --test golden_simulate
cargo test -p sim --test statistical_distribution -- --ignored  # nightly only
```

Statistical tests are `#[ignore]`d — they require thousands of seeds and compare
distributions, not point values. Run in nightly CI only.
