# Prior types — unify across CLI / IR / sim / sampling

> **Status:** flagged for follow-up cleanup (post v1-fit refactor).
> Do not fix in the same session as the v1 deletion — keep that diff
> focused.

## The problem

Four prior representations exist in the codebase. They overlap heavily
in semantic meaning but diverge in field names, type shape, and
variant coverage.

| Type | Path | Used by | Variants |
|---|---|---|---|
| `ir::PriorDist` | `rust/crates/ir/src/parameter.rs:22` | `.camdl` files (in-model declarations: `beta : rate ~ log_normal(2.0, 0.3)`) | Uniform, Normal, LogNormal, HalfNormal, Beta, Gamma, Exponential, Fixed |
| `sim::inference::Prior` | `rust/crates/sim/src/inference/prior.rs:41` | runtime — what gets passed to PGAS/PMMH/IF2 log-density evaluators | Flat, Uniform{lo,hi}, Normal, TransformedNormal, HalfNormal, Beta, Gamma, Exponential, Hierarchical |
| `config_v2::PriorSpec` | `rust/crates/cli/src/fit/config_v2.rs:327` | v2 `fit.toml [estimate.X.prior]` (override declared model priors) | LogNormal, Normal, Beta, Uniform, HalfNormal *(+ Gamma + Exponential after the v1 cleanup)* |
| `sampling::PriorSpec` | `rust/crates/cli/src/sampling.rs:27` | VOI / design tool (`{ dist = "beta", alpha=4, beta=6 }` in design TOMLs) | stringly-typed: `dist: String` + Option<f64> fields |

## Specific smells

1. **Field name drift.** `ir::LogNormalPrior { mu, sigma }`,
   `sim::Prior::TransformedNormal { mean, sd }`, and
   `config_v2::PriorSpec::LogNormal { mu, sigma }` all describe the
   same distribution with three different field-name conventions.
   `sim::Prior::TransformedNormal` is especially confusing — it's
   "log-normal when the transform is Log" but the field names
   (`mean`/`sd`) suggest natural-scale parameters.

2. **Wrapper-struct vs inline fields.** `ir::PriorDist` wraps each
   variant in a separate struct (`LogNormalPrior`, `GammaPrior`, etc.);
   `config_v2::PriorSpec` uses inline-field variants. Same data,
   different ergonomics.

3. **`sampling::PriorSpec` is stringly-typed.** Could share
   `config_v2::PriorSpec` (both are TOML-decoded prior specs). Today
   it's a parallel implementation with worse typing.

4. **`ir::PriorDist::Fixed(f64)` is conceptually distinct.** That's
   not a prior; it's a parameter pin (degenerate prior). Probably
   belongs as a separate concept (`Parameter.fixed: Option<f64>`)
   rather than a `PriorDist` variant.

5. **Hierarchical priors are `sim`-only.** `sim::Prior::Hierarchical`
   exists at runtime, but `ir::PriorDist` doesn't have a Hierarchical
   variant — instead `Parameter.hierarchical` is a separate field
   (`HierarchicalPrior` struct). Asymmetric.

## Proposed unification (sketch — to be refined when actually fixing)

Two shared types across the workspace:

- **`PriorSpec` (in a new shared crate, e.g. `priors`)** — the
  user-facing TOML form. Replaces `ir::PriorDist`,
  `config_v2::PriorSpec`, and `sampling::PriorSpec`. Single field-name
  convention (`mu`/`sigma` matches stats convention; `mean`/`sd` is
  also fine but pick one).
- **`Prior` (stays in `sim::inference`)** — the runtime evaluator
  type. Adds methods `from_spec(spec: &PriorSpec, transform: Transform)
  -> Prior` for the conversion. Keeps `Hierarchical` since it carries
  `Expr` arguments that depend on the IR.

Conversions:

- `.camdl` parser: `PriorDist token tree → PriorSpec`. (Currently:
  parser → `ir::PriorDist` → at inference time, `Prior::from_ir(&pd)`.
  New: parser → `PriorSpec` directly; the IR carries `PriorSpec`.)
- `fit.toml [estimate.X.prior]`: deserializes to `PriorSpec` directly.
- `sampling::DesignParam.prior`: deserializes to `PriorSpec`. Drop
  the bespoke `sampling::PriorSpec`.
- Inference call site: `Prior::from_spec(spec, transform)` once,
  cached. PGAS/PMMH/IF2 take `&[Prior]`.

`Parameter.fixed` separates from `prior` — a fixed value is not a
prior. Eliminates the `PriorDist::Fixed(f64)` variant.

## Why not now

- The v1 fit-config cleanup (in flight) is already a multi-hour
  refactor across `runner.rs / pgas.rs / pmmh.rs / fit/mod.rs`. Adding
  a workspace-wide prior-type unification on top would 4× the scope
  and conflate two unrelated changes in one diff.
- The v1 cleanup makes the prior conversion problem *more visible*
  (since v2 PriorSpec becomes the single CLI-side prior input). After
  it lands, the unification target is clearer.

## When

After v1 fit-config deletion is merged. Probably a single afternoon's
focused work — touches priors only.

## Tracking

- Source: `TYPES-REFERENCE.md` opportunity (B): "two unrelated
  PriorSpec types share a name." This file expands on that with the
  full four-way duplication picture.
- See also: `TYPES-REFERENCE.md` for the broader CLI types audit.
