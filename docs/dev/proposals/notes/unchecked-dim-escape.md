---
date: 2026-04-22
status: shipped
related: no-dim-check-hardening.md (same "escape hatch visibility" philosophy),
  docs/camdl-language-spec.md §2.2.2 (dim analysis)
---

# Per-expression dimensional escape: `unchecked_dim`

**Shipped 2026-04-22.** See spec §2.2.2 for user-facing docs. This
note retains the design discussion (naming rationale, trade-offs
considered, deferred follow-ups like the I400 audit trail).

## Motivation

Some phenomenological disease models **intentionally break dimensional
homogeneity** to capture inhomogeneous mixing without a mechanistic
contact structure. The canonical case is the He et al. 2010 London
measles model, which uses a fractional mixing exponent:

$$
\lambda(t) = \beta(t) \cdot (I + \iota)^\alpha \cdot S / N,
\quad \alpha \approx 0.976
$$

- `I + ι` has dimension **P** (population count).
- `(I + ι)^α` with non-integer `α` has no well-defined dimension —
  `P^0.976` is not a valid dimensional quantity.
- The He formulation calibrates `β(t)` to absorb the residual
  dimensional irregularity numerically. This is a deliberate
  phenomenological choice, not a bug.

References:

- He, D., Ionides, E. L., & King, A. A. (2010). Plug-and-play inference
  for disease dynamics: measles in large and small populations as a case
  study. *Journal of the Royal Society Interface* 7(43): 271–283.
  doi:10.1098/rsif.2009.0151.
- Bretó, C., He, D., Ionides, E. L., & King, A. A. (2009). Time series
  analysis via mechanistic models. *Annals of Applied Statistics* 3(1):
  319–348. doi:10.1214/08-AOAS201.

camdl's dim-checker correctly flags `(I + ι)^α` as ill-defined.
Users hitting this case currently must reach for `--no-dim-check`,
which disables the checker for the *entire* model — losing protection
on every other rate expression in the file.

The right shape of fix is a **narrow, visible per-expression escape**
that trusts the user's dimensional assertion for exactly the offending
subexpression and continues checking the rest normally.

## Proposed DSL

```camdl
transitions {
  infection : S --> E
    @ beta(t) * unchecked_dim((I + iota)^alpha,
                              dim = population,
                              reason = "He et al. 2010 α-mixing exponent")
             * S / pop
}
```

Semantics:

- The checker trusts that the wrapped expression has the declared
  `dim`. It does **not** verify the assertion — that's the
  programmer's responsibility.
- The surrounding rate expression is still dim-checked as usual.
  With `(I+ι)^α` asserted as P, the checker sees `β * P * S / P =
  β · P/T · P · P / P = β · P/T` — the transition's required
  dimension, so the outer check passes.
- `reason` is required — not decorative. Forces the user to document
  at the call site *why* the assertion is legitimate.

## Why `unchecked_dim`, not `cast_dim` or `as_dim`

Naming shapes reader expectation. `cast_dim` reads like a routine
conversion (cf. `as` in Rust, `(int)` in C). Routine conversions
imply "the language supports this transformation; it's a standard
operation." That's wrong for what we want.

What we want is "the programmer asserts this; the checker will not
verify." The canonical prior art is Rust's `unchecked_*` naming
convention — `unchecked_add`, `get_unchecked`, `from_utf8_unchecked`.
Each one signals: the language's invariants would normally catch a
misuse here, but this call explicitly takes responsibility for
correctness. Same shape as our situation.

Effect on readers:

- Seeing `cast_dim((I+ι)^α, population)` — "I guess the compiler
  needs a hint, fine." Routine, unscrutinised.
- Seeing `unchecked_dim((I+ι)^α, dim = population, reason = …)` —
  "why is this unchecked? Is there a legitimate reason?"

The second is the intended reviewer reaction. `cast_dim` would drift
into routine use; `unchecked_dim` stays a code smell.

Alternative `assume_dim` is slightly shorter and still signals
"programmer assertion," but lacks the specific "checker is disabled
here" connotation that `unchecked_` carries. Use `unchecked_dim`.

## Required dim parameter — domain names, not bracket tuples

The declared dim should use camdl's domain-specific names from the
existing `dim_annotation` vocabulary:

```camdl
unchecked_dim(e, dim = population)        # P
unchecked_dim(e, dim = rate)              # T^-1
unchecked_dim(e, dim = population_rate)   # P·T^-1
unchecked_dim(e, dim = time)              # T
unchecked_dim(e, dim = dimensionless)     # (0, 0)
```

These read correctly and avoid the ambiguity of bracket notation
(where `[1]` could plausibly be read as "population" or
"dimensionless"). Matches the `dim_annotation` vocabulary introduced
in §4.1.1.

For the canonical He case, the target is **`population`** (P), not
dimensionless. Reasoning:

- The full rate is `β(t) · (I+ι)^α · S / N`.
- `β(t)` has units T^-1.
- `S/N` is dimensionless.
- For the full expression to have units P·T^-1 (transition rate),
  `(I+ι)^α` must absorb the P-exponent — i.e. the assertion is that
  this phenomenological expression **behaves as if it were a count**.

An assertion of `dimensionless` would leave the full rate at T^-1
(per-capita rate), not P·T^-1 (transition rate), and downstream
dim-checks would fail. Any future documentation of this feature
must use the correct target dim in its example — getting this wrong
would propagate to user code.

## Required reason string

Following the `--no-dim-check` hardening principle ("never ship a
silent escape"), the `reason` kwarg is required. Cheap to implement,
forces articulation, persists in the IR for audit.

The IR gains an `IrExpr` variant `UncheckedDim { inner, dim, reason }`.
It behaves at runtime exactly like `inner` — no runtime cost. The
escape exists only at the dim-check stage.

## Compiler audit trail

At the end of compilation, emit an informational diagnostic listing
every `unchecked_dim` site in the model:

```
info[I400]: 1 unchecked_dim assertion in this model
  he2010_london.camdl:42
    expression: (I + iota)^alpha
    asserted dim: population (P)
    reason: "He et al. 2010 α-mixing exponent"
```

Not a warning — the feature is legitimate when used. But an
inspectable list lets reviewers audit whether each use is justified.
Surface this in `camdl inspect --strict` (future) as the audit tool
for model quality review.

In CAS provenance (`run.json`), include the count and a structured
list of unchecked sites so experiment logs show which runs contained
dimensional assertions and on what justification.

## Interaction with `--no-dim-check`

Orthogonal. `--no-dim-check` is whole-model; `unchecked_dim` is
per-expression. Recommended usage:

| Case | Use |
|---|---|
| Known dim-checker bug | `--no-dim-check` (with reason, per hardening proposal) |
| One phenomenological subexpression in an otherwise clean model | `unchecked_dim` |
| Porting a model with many irregularities | `--no-dim-check` initially, replace with `unchecked_dim` sites as you verify each one |

The explicit kill condition: once `unchecked_dim` covers the common
escape cases, `--no-dim-check` should be deprecated down to "compiler
bug workaround only" and eventually removed.

## Effort estimate

~1 day:

- Parser: new `unchecked_dim(expr, dim = NAME, reason = STR)` form
  as a pseudo-function-call in expression position (treat as a new
  primitive, not EFuncCall). Reject missing `reason`.
- AST + IR: new `EUncheckedDim` / `IR::UncheckedDim` variant.
- Expander: resolve inner expression, record dim + reason.
- Dimcheck: `UncheckedDim { inner, dim, ... }` returns `Known
  (dim_of_name dim)` without recursing into `inner` for unification
  purposes. Still typechecks `inner` against its own expectations,
  but the assertion is load-bearing at the boundary.
- Runtime: `UncheckedDim` acts as identity — `eval_resolved` unwraps
  the inner and evaluates. No runtime cost.
- Diagnostics: I400 info at end of compile; site list in run.json.
- Spec §2.2.1 addition + the He et al. canonical example.
- Tests: wrapping fractional exponent compiles; missing `reason`
  errors; audit-trail list exactly matches source sites.

## Spec integration

Proposed placement in `docs/camdl-language-spec.md` §2.2.1 (dim
analysis), as a new subsection after the existing `exp()`/`log()`
rules:

> **Non-integer exponents** (`x^y` with `y` not an integer constant)
> require `x` to be dimensionless. Phenomenological models that
> intentionally break this — e.g. the He et al. (2010) α-mixing term
> `(I + ι)^α` — should use `unchecked_dim` (§2.2.2) to document the
> assertion at the call site.

Then §2.2.2 introduces `unchecked_dim` with the He example, the
naming rationale in one line ("per Rust's `unchecked_*` convention"),
and the required `reason` semantics.

## Why deferred

Not blocking anything currently. GH #8 closed; He 2010 vignette can
use `--no-dim-check` in the interim with a prose note citing the
phenomenological-mixing rationale. Landing `unchecked_dim` cleanly
is a single day of focused work; slot into the next CLI/DSL polish
window.

Revisit when:

- A user other than the book agent hits the same pattern (signal
  that it's not a one-off).
- `camdl inspect --strict` lands and the audit-trail hook has a home.
- An external model with 3+ phenomenological subexpressions wants
  to port; at that point the one-day cost pays back immediately.
