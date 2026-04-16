---
status: proposal
date: 2026-04-16
---

# Prior Distribution Syntax for Parameter Declarations

## Motivation

camdl exists to support decisions about people's lives — vaccination campaigns,
outbreak response, resource allocation. Getting uncertainty wrong means making
confident-looking recommendations on shaky foundations. Prior predictive checks
are the first line of defense: "do my stated beliefs produce data that looks
plausible before I've seen any real data?" If the answer is no, the model or
the priors need revision.

Currently, priors can only be declared in fit.toml — a runtime configuration
file separate from the model. This means:

- `camdl simulate --draws prior` requires a fit.toml even for a basic prior
  predictive check
- Priors are disconnected from the parameters they describe
- The model file alone doesn't encode what the modeler believes about its
  parameters
- Reproducibility suffers: to understand a model's assumptions, you need both
  the .camdl file and the fit.toml that was used with it

The run spec §12 currently says "priors are analysis choices that vary between
fits — they belong in fit.toml." This proposal changes that position: **priors
are beliefs about parameters — they belong with the parameter declaration.**
fit.toml can override model priors for sensitivity analysis, preserving the
existing workflow while making the default case simpler and more honest.

This aligns camdl with Stan, PyMC, and Turing.jl, all of which encode priors
in the model definition.

## Syntax

```
parameters {
    beta  : rate in [0.01, 2.0] ~ log_normal(mu = -1.0, sigma = 0.5)
    gamma : rate in [0.05, 1.0] ~ half_normal(sigma = 0.5)
    rho   : probability in [0.001, 1.0] ~ beta(alpha = 2.0, beta = 5.0)
    N0    : count in [100, 1_000_000]     # no prior — must be fixed in inference
    I0    : count                          # no bounds, no prior — structural constant
}
```

The `~` reads as "distributed as" — the standard notation across Bayesian
modeling languages. It is always optional; models without priors are valid.

### Supported distributions

| Distribution    | Syntax                              | Parameters (named)     |
|-----------------|-------------------------------------|------------------------|
| `log_normal`    | `~ log_normal(mu = M, sigma = S)`   | mu, sigma on log scale |
| `normal`        | `~ normal(mu = M, sigma = S)`       | mean, sd               |
| `half_normal`   | `~ half_normal(sigma = S)`          | sd of underlying normal|
| `beta`          | `~ beta(alpha = A, beta = B)`       | shape parameters       |
| `gamma`         | `~ gamma(shape = K, rate = R)`      | shape, rate (NOT scale)|
| `exponential`   | `~ exponential(rate = R)`           | rate = 1/mean          |
| `uniform`       | `~ uniform(lower = L, upper = U)`   | bounds                 |

All arguments are keyword (named), never positional. All must be compile-time
constants (arithmetic of literals is fine: `mu = log(0.3)`).

### Parameterization conventions

These are load-bearing choices. Documenting them precisely avoids the bugs
that come from mu/sigma vs median/geometric_sd confusion:

**`log_normal(mu, sigma)`**: mu and sigma are on the **log scale**.
`log(X) ~ Normal(mu, sigma)`. The median of X is `exp(mu)`. Example:

```
beta : rate ~ log_normal(mu = -1.0, sigma = 0.5)
# log(beta) ~ Normal(-1.0, 0.5)
# median(beta) = exp(-1.0) ≈ 0.37
# 95% CI: [exp(-1.0 - 1.0), exp(-1.0 + 1.0)] ≈ [0.14, 1.0]
```

**`half_normal(sigma)`**: sigma is the standard deviation of the **underlying**
(unfolded) normal. The half-normal has `E[X] = sigma * sqrt(2/π)` and
`Var[X] = sigma² * (1 - 2/π)`.

**`gamma(shape, rate)`**: rate parameterization (rate = 1/scale). This matches
Stan and avoids the R/numpy disagreement. `E[X] = shape/rate`.

### What `~` is NOT

- Not a default value. `beta : rate ~ log_normal(...)` does not set beta's
  value. Values come from `--params` or inference.
- Not required. Parameters without `~` are valid — they just can't be used
  with `--draws prior` unless a prior is supplied externally.

The `fixed` pseudo-distribution from the IR is NOT exposed as DSL syntax. A
fixed parameter is one without a prior, supplied via `--params` or set in
fit.toml's `[fixed]` section. The syntax for "this parameter has a known
value" is already `--params baseline.toml`.

## Prior precedence chain

When the system needs a prior (for PGAS/PMMH inference or `--draws prior`):

```
fit.toml [estimate] prior override    (highest — sensitivity analysis)
  ↓ if absent
model IR parameter.prior              (from ~ syntax in .camdl)
  ↓ if absent
Flat (improper uniform)               (default for inference)
  ↓ but for --draws prior:
Error                                 (must have a prior to sample from)
```

fit.toml overrides preserve the existing workflow: "I want to test what
happens with a wider prior on beta" without editing the model file.

## Implementation

### OCaml compiler

**1. AST** (`ast.ml`): Add `prior_spec` type and `pprior` field to `param_decl`.

```ocaml
type prior_spec = {
  ps_name: string;                    (* "log_normal", "beta", etc. *)
  ps_args: (string * expr) list;      (* keyword args *)
}
```

**2. Lexer** (`lexer.mll`): Add `| '~' { TILDE }` token.

**3. Parser** (`parser.mly`): Add `prior_clause_opt` rule with `TILDE`, keyword
argument parsing. Append to all 4 `param_decl` alternatives.

**4. Expander** (`expander.ml`): Add `resolve_prior_spec` helper mapping AST
`prior_spec` → `Ir.prior_dist`. Prior kwargs must be compile-time constants —
use `eval_const_expr` (not `resolve_float_expr`, which silently returns 0.0
for non-constant expressions). Error via `Diagnostics.error` with code `E230`
if a kwarg is not a constant. Update `expand_parameters` to call it, replacing
`Ir.prior = None` with `Ir.prior = Option.map (resolve_prior_spec ctx) pprior`.

### Rust runtime

**5. Prior module** (`rust/crates/sim/src/inference/prior.rs`, new file):
Extract `Prior` enum from `pmmh.rs` into new shared module `inference/prior.rs`.
Add `HalfNormal`, `Gamma`, `Exponential` variants. Add
`Prior::from_ir(pd: &ir::PriorDist) -> Self` conversion. Add `log_density`
implementations for new variants. Update `pgas.rs` and `pmmh.rs` to import
from the shared module.

**6. Fit runners** (`pgas.rs`, `pmmh.rs`): Change prior resolution to:
IR prior → fit.toml override → Flat. Extract shared `resolve_prior` to
`runner.rs`.

**7. `--draws prior`** (`main.rs`): When `--fit` is not provided, read priors
from the compiled model's IR. Error if any parameter lacks both a prior and
a default value, with actionable message.

### Documentation

**8. Run spec §12**: Rewrite to reflect model-embedded priors as primary,
fit.toml as override.

**9. Language spec**: Add `~` syntax to parameter declaration section.
Remove the `priors.toml (v0.2+)` section.

### Tests

- Parse + expand: scalar with prior, indexed with prior, no prior
- All 7 distribution types
- Error: unknown distribution name
- Error: missing required keyword argument
- Error: non-constant argument
- Golden model with priors → verify IR JSON round-trip
- Rust: `--draws prior` from IR (no --fit)
- Rust: fit.toml prior overrides IR prior
- Rust: `--draws prior` error when params lack priors

## Follow-up work (not in this PR)

- **Bounds consistency warning**: W-code if prior places >X% mass outside
  declared bounds. E.g., `normal(0.5, 2.0)` on a `probability in [0, 1]`
  parameter. Useful but not blocking.

- **`--priors FILE`**: A standalone priors TOML file as a third source between
  IR and fit.toml, for cases where the user wants priors without modifying the
  model. Precedence: fit.toml → `--priors` → model IR → error.

- **Indexed heterogeneous priors**: `R0[p in patch] ~ log_normal(mu = f(p))`
  where the prior varies per index value. Current design applies the same
  prior to all expanded instances.

- **`camdl sample-prior` command**: A dedicated top-level command for drawing
  from the prior without running dynamics. Conceptually cleaner than
  `--draws prior` on the simulate command. Syntax:
  `camdl sample-prior model.camdl -n 1000 -o prior_draws.tsv`
