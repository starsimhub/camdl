# `camdl fit` audit — what's live, what's stale, what's misshapen

Date: 2026-04-27
Project: camdl
Tags: cli, audit, dead-code, summary, refactor

## Context / question

User flagged a design smell in `camdl fit summary` (works for IF2
stages, ignores PGAS / PMMH). Auditing the question forced a wider
look at the `camdl fit` command surface. This note captures the
state of every live and stale `fit` subcommand as of `3f8921f`,
plus a recommendation on what to clean up.

The motivation: this is unreleased software, and AI agents (which
are increasingly how camdl gets edited) read more context to do
less. Every `#[allow(dead_code)]` module is hundreds of lines of
context an agent has to scroll past to find the live path. Stale
code in unreleased software is pure tax — there are no consumers
to placate, no migrations to stage. Delete aggressively.

---

## 1. Live `camdl fit` subcommands

Six commands dispatched by `main.rs::FitCmd` at `main.rs:274–279`:

| command                            | dispatcher                              | scope                                                    |
|------------------------------------|------------------------------------------|---------------------------------------------------------|
| `camdl fit run <fit.toml>`         | `cmd_fit_run_v2` (`fit/mod.rs:197`)      | runs IF2 / PGAS / PMMH / PFilter stages from v2 fit.toml |
| `camdl fit status <dir>`           | `cmd_fit_status` (`fit/mod.rs:36`)       | walks output tree, lists completed stages               |
| `camdl fit summary <dir>`          | `cmd_fit_summary` (`fit/fit_summary.rs:51`) | per-stage interpretation block (**MLE-only — see §3**) |
| `camdl fit diff <a.toml> <b.toml>` | `cmd_fit_diff` (`fit/mod.rs:1562`)       | side-by-side config diff                                |
| `camdl fit new <a.toml>`           | `cmd_fit_new` (`fit/mod.rs:1679`)        | scaffold derived fit.toml                               |
| `camdl fit where <a.toml>`         | `cmd_fit_where` (`fit/mod.rs:1525`)      | print resolved output dir                               |

`Stage` (`config_v2.rs:396`) has four variants: `IF2`, `PGAS`,
`PMMH`, `PFilter`. `Stage::is_bayesian()` (line 475) classifies
them. So the type system already distinguishes; only `summary`
ignores the distinction.

### 1.1 Stages are independent — important

`StartsFrom::Random` is the default when `starts_from` is omitted
(`config_v2.rs:569–576`). `validate_stage_dag` (line 1019) only
checks that explicit `starts_from = "X"` references resolve to a
prior stage; it never *requires* a prior stage. **A fit.toml with
only `[stages.pgas]` is valid** — runs PGAS from random starts, no
IF2 required.

This matters for §3: any "stage-aware" framing of summary that
hard-codes a list of expected stage names is wrong by construction.
Users name their own stages; the architecture is a DAG, not a
fixed pipeline.

---

## 2. Stale code findings

### A. Duplicate `Cli` / `Command` / `FitCommand` parser tree in `args/mod.rs`

`args/mod.rs:16` defines `pub struct Cli`, `args/mod.rs:28` defines
`pub enum Command`, `args/mod.rs:79` defines `pub enum FitCommand`.
None of these is the live parser. The live binary uses
`main.rs:67::Cli`, `main.rs::Command`, `main.rs:205::FitCmd`.
The `args::*` versions exist *only* so a test helper at
`args/mod.rs:1034` (`try_parse_fit_run`) can parse argv via clap.

The two trees have already drifted: `main.rs::Command` has
`Compare(args::CompareArgs)`; `args::Command` doesn't. Future drift
will keep happening because nothing forces them to match.

**Fix:** delete `args::Cli` / `args::Command` / `args::FitCommand`.
Move the test helper to use `main.rs`'s `Cli` (re-export the
internal types from `main.rs` for test use, or move the
parse-trees into `args/` and have main.rs use those — pick one
home). Estimated impact: ~120 lines deleted from `args/mod.rs`,
trivial test refactor.

### B. Three orphaned legacy stage entry points

`scout::run_scout`, `refine::run_refine`, `validate::run_validate`
are all `#[allow(dead_code)]` in `fit/mod.rs:17–22`. The "v1"
`camdl fit scout / refine / validate` subcommands were removed
2026-04-20 (per the comment at `fit/mod.rs:1265`). The functions
remain as ~250-line entry points each, plus their own
`write_summary` private helpers, plus `pub(crate)` JSON builders
(`build_scout_summary_json`, `build_refine_summary_json`).

Dead code measurement:
- `scout.rs`: 423 lines, of which `run_scout` (~250 lines),
  `write_summary` (~30), `build_scout_summary_json` (~50), and
  `now_iso8601` + helpers (~40) are dead — roughly 80% of the file.
- `refine.rs`: 348 lines, similar ratio.
- `validate.rs`: 769 lines, similar ratio (the file additionally
  has profile-likelihood code that may be used elsewhere — needs
  audit).

**Fix:** delete `run_scout`, `run_refine`, `run_validate`, their
private helpers, and the `pub(crate)` builders that nobody calls.
Keep any utility functions that the live v2 path still uses (the
v2 path imports `crate::fit::scout::now_iso8601_pub` for the
timestamp; that one helper survives).

### C. No per-stage summary JSON written on a v2 run

Followed from B: the only callers of `write_summary` /
`build_*_summary_json` are inside `run_scout` / `run_refine` /
`run_validate` themselves. The live `cmd_fit_run_v2` path
(`fit/mod.rs:587` for IF2, similar for PGAS/PMMH) does **not**
invoke them.

But `docs/camdl-inference-spec.md` §6.1, §6.2, §7.2.2 document
`<stage>_summary.json` schema as if the files exist. They don't.

**Fix:** decide which way to make truth match docs. Either:

1. **Wire up summary JSON in v2.** The data is all there in
   `ChainResults`; the v2 path could call the existing builders
   (after extracting them from the dead wrappers). Modest work,
   makes the docs accurate.
2. **Delete the docs and the builders.** Simpler. The new
   `camdl fit summary --format json` command provides the same
   information from `fit_state.toml`, so per-stage JSON files
   are arguably redundant.

I'd recommend (2) — the new `summary --format json` is strictly
better than per-stage JSON files (versioned, stable schema,
includes provenance cross-check). The per-stage JSON was a Phase
7 leftover from the original design.

### D. `fit/status.rs` v1 dead status printer

`status.rs:10` is `#![allow(dead_code)]`. The v1 `run_status`
function (~330 lines) was the prototype for `fit_summary.rs`,
but Phase 1 created `fit_summary.rs` fresh rather than
resurrecting `status.rs`. Now doubly orphaned.

**Fix:** delete. Anything useful from it is already in
`fit_summary.rs`. The Phase-1 minimal fix for GH #18 lives in
`fit/mod.rs::print_stage_status`, not in `status.rs`.

### E. `fit/summary.rs` vs `fit/fit_summary.rs` name collision

`fit/summary.rs` is the *grid-level* summary writer for `camdl
batch run` (writes `summary.tsv` + `coverage.tsv` across grid
cells). `fit/fit_summary.rs` is the new `camdl fit summary`
command. Identical filename root, totally different scope. The
collision bit me already during Phase 1 when I wrote my new code
to `fit/summary.rs` and clobbered the grid writer.

**Fix:** rename. Two reasonable shapes:

- `fit/grid_summary.rs` (the batch writer) + `fit/fit_summary.rs`
  (the command). One-word rename of the older file.
- A `fit/summary/` directory with `grid.rs` + `interpretation.rs`
  modules. More structured.

The first is cheaper.

### F. Scattered `#[allow(dead_code)]` modules in fit/mod.rs

The module declarations at `fit/mod.rs:11–32`:

```
pub mod config;
#[allow(dead_code)]                  // ← only Phase-3 schema additions live here
pub mod config_v2;
pub mod state;
pub mod provenance;
pub mod runner;
#[allow(dead_code)]                  // ← legacy v1
pub mod scout;
#[allow(dead_code)]                  // ← legacy v1
pub mod refine;
#[allow(dead_code)]                  // ← legacy v1
pub mod validate;
pub mod status;                      // ← #![allow(dead_code)] inside
pub mod summary;                     // ← grid-level (live)
pub mod fit_summary;                 // ← interpretation command (live)
...
```

`config_v2` is `#[allow(dead_code)]` because it has many
unreferenced items, but the module *is* live — most of its types
are used. The blanket `dead_code` allow hides which specific items
are dead. Same for the legacy v1 stages. **The smell is
non-targeted dead-code suppression**; an item-level allow forces
us to either delete or use each piece.

**Fix:** drop the module-level `#[allow(dead_code)]`, let the
compiler error on each dead item, then either delete or rationalize
case by case.

### G. Total dead-code estimate

Ballpark:
- `args::Cli` + `args::Command` + `args::FitCommand`: ~120 lines
- `scout.rs` legacy: ~330 lines
- `refine.rs` legacy: ~280 lines
- `validate.rs` legacy: ~600 lines (some kept for profile-likelihood
  code; needs audit)
- `status.rs` v1 printer: ~330 lines

**Total: ~1,500–1,700 lines of dead-but-compiled code in fit/.**
That's ~30% of the fit module. Every PR / commit / review / agent
session pays a context tax on this.

---

## 3. The summary design smell — and the right fix

`fit_summary.rs:38` hard-codes:

```rust
const MLE_STAGES: &[&str] = &["scout", "refine", "validate"];
```

…and iterates only those names. That's wrong on three axes.

### What's wrong

1. **Hard-codes stage names.** Users pick their own — `deep_scout`,
   `fast_refine`, `posterior`. A user with `[stages.deep_scout]`
   gets nothing, even though it's an IF2 stage.
2. **Excludes Bayesian methods.** A fit dir with only `[stages.pgas]`
   produces "(no MLE stages found)" — silent miss.
3. **Pretends pipelines are required.** They're not. Stages are a
   DAG; PGAS-from-random is a perfectly valid one-stage fit.

### Right framing

Walk every subdirectory of `fit_dir` that has `fit_state.toml` or
`run.json`. For each, read `run.json` (or fall back to `fit_state`
metadata) to learn the stage's `method` (`if2` / `pgas` / `pmmh` /
`pfilter`). Render with a method-appropriate stanza.

This is **not** "stage-aware" in the sense of branching on a known
enumeration of stage names. It's "render whichever stages exist,
in declaration order from the fit.toml if available, otherwise
alphabetical." The user's chosen names go through unchanged.

### Per-method stanza shapes

Different methods have different load-bearing diagnostics. Sketch:

**IF2** (current):
- compound scout-convergence gate (Â + decibans-spread)
- per-parameter Â table
- per-chain clean-eval winner ll/se
- provenance cross-check (final ↔ mle ↔ fit_state)

**PGAS** (Bayesian, MCMC):
- per-parameter posterior mean ± SD
- 95% credible interval
- MCMC chain-mixing `rhat` (the original Gelman-Rubin —
  Bayesian context, not the MLE Â)
- effective sample size (per parameter, post-burn-in)
- divergence count (PGAS+NUTS)

**PMMH** (Bayesian, MCMC):
- same as PGAS minus divergences
- acceptance rate (load-bearing for PMMH)

**PFilter**:
- precise loglik ± SD over replicates
- ESS at θ̂ (per observation, mean / min)
- prequential scores if computed

### Implementation sketch

Already on the right track in `fit_summary.rs`: a `StageReport`
struct that the formatters consume. Generalize to a tagged enum:

```rust
pub enum StageReportKind {
    If2(If2Report),       // current StageReport renamed
    Pgas(PgasReport),
    Pmmh(PmmhReport),
    Pfilter(PfilterReport),
}

pub struct StageReport {
    pub name: String,        // user-chosen, e.g. "deep_scout"
    pub method: String,      // "if2" | "pgas" | "pmmh" | "pfilter"
    pub kind: StageReportKind,
    // shared fields: provenance, stage_progression, etc.
}
```

JSON output gets a `kind` discriminator; formatters dispatch on
it. Schema version bumps to 2 if we want strict consumers to
notice the new shape; v1 → v2 migration is just "add a `kind`
field, content stays the same per kind."

### Action

This is its own proposal. Not in this audit note — write
`docs/dev/proposals/2026-04-27-fit-summary-method-aware.md`
once we agree on the audit's recommendations and have a clean
base to build on.

---

## 4. Recommended cleanup order

To avoid landing a "stage-aware summary" on top of stale code,
sequence cleanup *first*:

1. **Delete `args::Cli` / `args::Command` / `args::FitCommand`.**
   Move the test helper to `main.rs`'s tree. (~1 commit.)
2. **Delete `fit/scout::run_scout` + `fit/refine::run_refine` +
   `fit/validate::run_validate` and their `write_summary` / builder
   helpers**, after auditing `validate.rs` for any profile-likelihood
   bits the live path still depends on. Keep `scout::now_iso8601_pub`
   if it's still used. (~1 commit, ~1,200 lines deleted.)
3. **Delete `fit/status.rs`.** (~1 commit, ~330 lines.)
4. **Rename `fit/summary.rs` → `fit/grid_summary.rs`.** Update the
   one caller in `fit/mod.rs:1227`. (~1 commit, mechanical.)
5. **Drop module-level `#[allow(dead_code)]` from `fit/mod.rs`** and
   fix or delete whatever the compiler complains about. (~1 commit.)
6. **Decide on `<stage>_summary.json`:** either wire the builders
   into the v2 path, or delete the docs claiming they exist.
   Recommend the second. (~1 commit.)

Total: ~6 commits, ~1,500 lines net deleted. After that the
`fit/` module is a clean base for the method-aware summary
proposal.

## 5. Process change

I'll propose a CLAUDE.md addition (separate change set) that
makes "delete dead code aggressively" an explicit project policy:

- `#[allow(dead_code)]` is a smell. Used at a definition site, it
  tells a future reader "I know this is dead but didn't have time
  to delete." Used at a module level, it hides which specific
  items are dead. Either prove the item is reachable, or delete it.
- Removed code can come back from `git log -S '<symbol>'`.
  Deletions in unreleased software are cheap.
- A sibling rule already in CLAUDE.md: "Backwards compatibility is
  a non-goal." This is its enforcement mechanism — *deleting* the
  old code, not just renaming.

The motivation is two-sided:
- Smaller surface = faster + clearer agent edits.
- Smaller surface = humans (you) review changes faster.

## Next

Ship the cleanup commits in the order above. Then write the
method-aware summary proposal against the cleaned-up base. Then
implement.
