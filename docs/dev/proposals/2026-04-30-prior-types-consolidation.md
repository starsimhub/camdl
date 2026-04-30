# Prior types consolidation

Date: 2026-04-30
Status: proposed (pre-alpha cleanup)

## Problem

Four representations of "a prior distribution" exist in the workspace,
with two parallel conversion paths to the runtime evaluator. Tracked
in `TYPES-REFERENCE.md` §6.1 and `CLEANUP-prior-types.md`.

| Type | Where | Shape | Variants |
|---|---|---|---|
| `ir::parameter::PriorDist` | `crates/ir/src/parameter.rs` | externally-tagged enum, `{"normal": {"mean": ..., "sd": ...}}` | 8 (Uniform, Normal, LogNormal, HalfNormal, Beta, Gamma, Exponential, Fixed) |
| `cli::fit::config_v2::PriorSpec` | `crates/cli/src/fit/config_v2.rs` | internally-tagged, `{"dist": "normal", "mu": ..., "sigma": ...}` | 7 (no Fixed) |
| `cli::sampling::PriorSpec` | `crates/cli/src/sampling.rs` | stringly-typed `dist: String` + flat opt fields | 4 (Beta, LogNormal, Normal, Uniform) |
| `sim::inference::Prior` | `crates/sim/src/inference/prior.rs` | runtime evaluator (stays) | 9 (incl. Hierarchical) |

Specific smells:

1. **Field-name drift**: `Normal { mean, sd }` (IR) vs `Normal { mu, sigma }` (v2). Same distribution, two names.
2. **Wire-format drift**: external tagging (IR) vs internal tagging (v2).
3. **Stringly-typed sampling::PriorSpec**: panics-as-formatted-error on unknown `dist`; doesn't connect to runtime inference at all.
4. **v2's Uniform discards bounds**: `prior = { dist = "uniform" }` produces `Prior::Flat` regardless of `bounds = [...]`. Surprising.
5. **Two converters into `sim::Prior`**: `prior_spec_to_prior` (v2 path) and `Prior::from_ir` (IR path). Risk of variants drifting between them.

## Decision

Collapse to two types:

- `ir::parameter::PriorDist` — single serialization-form. Used by the OCaml compiler (already emits this), Rust IR parsing (already), and `fit.toml`'s `[estimate.X.prior]` (NEW: replaces `cli::fit::config_v2::PriorSpec`).
- `sim::inference::Prior` — runtime evaluator. Single conversion site `From<ir::PriorDist> for sim::Prior` (the existing `Prior::from_ir`, to be reshaped as a `From` impl).

Delete:
- `cli::fit::config_v2::PriorSpec`
- `cli::sampling::PriorSpec`
- `cli::fit::runner::prior_spec_to_prior`

Wire format: matches OCaml's existing emission. No OCaml-side changes; no golden-file changes.

```toml
# v2 (today, will be removed):
[estimate.beta]
prior = { dist = "log_normal", mu = 0.0, sigma = 1.0 }

# After consolidation:
[estimate.beta]
prior = { log_normal = { mu = 0.0, sigma = 1.0 } }
```

```toml
# Field-name changes (v2 → IR):
prior = { dist = "normal", mu = 0, sigma = 1 }    # OLD
prior = { normal = { mean = 0, sd = 1 } }          # NEW
```

`Uniform` semantics fix: `prior = { uniform = { lower = 0, upper = 10 } }` becomes
`Prior::Uniform { lo: 0, hi: 10 }`. To get the prior `Flat` (uninformative on the
real line), omit the `prior` field entirely. v2's silent
`prior = { dist = "uniform" } → Prior::Flat` was ignoring user-supplied bounds
and is unsupported in the new shape — bounds belong on the prior, not on
`[estimate.X.bounds]` (which is a *constraint* on inference, not the prior).

## Scope

### Rust changes

1. **`crates/cli/src/fit/config_v2.rs`**: delete `PriorSpec` and its 7 variants. Import `ir::parameter::PriorDist`. Update `EstimateSpecV2.prior: Option<PriorDist>`. Update `format_prior()` display function. Update validation tests.

2. **`crates/cli/src/fit/runner.rs`**: delete `prior_spec_to_prior`. The existing `Prior::from_ir(ir::PriorDist)` is the canonical converter. `resolve_prior` reads `EstimateSpecV2.prior: Option<PriorDist>` and either:
   - Forwards to `Prior::from_ir` if the IR has Hierarchical, OR
   - Forwards to `Prior::from_ir` for plain `PriorDist`.
   (Hierarchical priors continue to come from `ir::Parameter.hierarchical`, not `[estimate.X.prior]`.)

3. **`crates/cli/src/sampling.rs`**: delete `PriorSpec`. `DesignParam.prior: Option<PriorDist>`. Update VOI importance-weighting code to consume the typed enum.

4. **`crates/cli/src/fit/config_diff.rs`**: update `format_prior` to render `PriorDist` instead of v2's `PriorSpec`.

5. **`crates/cli/src/main.rs`**: `cmd_fit_new`'s prior-rendering path uses `PriorDist`.

6. **Test fixtures**: every fit.toml literal in tests using `prior = { dist = "...", ...}` becomes `prior = { ... = { ... } }`. Search-and-replace by prior family.

7. **`crates/sim/src/inference/prior.rs`**: keep `Prior::from_ir` as-is. Optionally add `impl From<ir::parameter::PriorDist> for sim::inference::Prior` as the canonical converter (idiomatic; current free function works too).

8. **`Hierarchical` handling**: `ir::HierarchicalPrior` stays where it is. `Prior::Hierarchical(HierarchicalPrior)` stays. No change to that path.

### Doc / fixture changes

- camdl-book (separate repo at `/Users/vsb/projects/work/camdl-book`): fit.toml examples in vignettes. Update `prior =` blocks to the new external-tagged form.
- `docs/dev/proposals/`: any prose mentioning `dist = "..."` syntax.
- `TYPES-REFERENCE.md` §6.1: update to reflect collapsed state.
- `CLEANUP-prior-types.md`: delete (work is done).

### What does NOT change

- OCaml `ocaml/lib/ir/ir.ml` and `serde.ml`: zero changes. The wire format these emit IS the canonical form post-consolidation.
- Golden IR JSON files in `ocaml/golden/`: zero changes.
- `sim::inference::Prior` runtime variants: zero changes.
- DSL `.camdl` syntax (`parameter beta { prior = log_normal(mu=0, sigma=1) }`): zero changes.

## Validation

After landing:

1. `cargo build --workspace` clean
2. `cargo test --workspace --no-fail-fast` all green
3. `cargo build -p cli --bin camdl --tests -- -D warnings` clean
4. OCaml round-trip: `make update-golden && make update-expected` produces zero diff (the IR JSON shape is unchanged).
5. A camdl-book vignette renders against a manually-updated fit.toml.

## Acceptance criteria

- Single `pub use ir::parameter::PriorDist` re-exported as `cli::fit::config_v2::PriorSpec` for ergonomic local imports, OR all v2 callers reference `ir::PriorDist` directly. (Pick one; consistency matters more than which.)
- Zero references to `cli::sampling::PriorSpec` outside the module's own delete commit.
- Zero references to `prior_spec_to_prior` outside its delete commit.
- All Rust tests pass.

## Out of scope

- Adding new prior families (e.g., `StudentT`, `Cauchy`).
- Changing `sim::inference::Prior` runtime variants or adding new conversion code beyond the rename / `From` impl.
- The Hierarchical/MultivariatePriorSpec story for simplex-group Dirichlet priors — that's tracked separately (sees `2026-04-28-cas-typed-runs-and-profile-stages.md` for the simplex_groups type design and the eventual Dirichlet add-on).

## Risks

- **camdl-book vignettes will break**: fit.toml prior blocks need rewriting. Estimated 5–15 fit.toml files. The rewrite is mechanical (sed-able). The book repo is separate; this proposal lands the Rust changes; the vignette rewrite is a follow-up.
- **No real risk to data**: this is a schema rename for in-memory and on-disk fit-config representations. No model IR or run.json wire format is affected. No simulation outputs are affected.
- **Test breakage during transition**: a few hundred LOC in test fixtures. Mechanical.

## Implementation sequence

Suggested commit-by-commit order so each step compiles + tests:

1. Add `pub use ir::parameter::PriorDist` re-export to `cli::fit::config_v2`. No behavior change yet.
2. Switch `EstimateSpecV2.prior: Option<PriorSpec>` to `Option<PriorDist>`. Update `prior_spec_to_prior` to take `&PriorDist`. Update validation tests.
3. Delete `cli::fit::config_v2::PriorSpec`. (Test fixtures need updating in this commit.)
4. Replace `prior_spec_to_prior` calls with `Prior::from_ir`. Delete `prior_spec_to_prior`.
5. Replace `cli::sampling::PriorSpec` with `ir::PriorDist` for `DesignParam.prior`.
6. Update `format_prior` displays in `config_diff.rs` and `main.rs`'s `cmd_fit_new`.
7. Final cleanup: delete `CLEANUP-prior-types.md`. Update `TYPES-REFERENCE.md` §6.1.

Each commit must: build clean, pass tests, compile under `-D warnings`.
