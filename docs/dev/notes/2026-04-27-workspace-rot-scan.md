# Workspace dead-code scan — pre-cleanup inventory

Date: 2026-04-27
Project: camdl
Tags: cleanup, dead-code, audit

## Question

After the 2026-04-27 fit-command audit (`2026-04-27-fit-command-audit.md`)
identified ~1,500 lines of dead code in `fit/`, the user asked for a
full-workspace scan and aggressive cleanup before alpha. This note
captures the scan; cleanup happens in subsequent commits.

Mode: aggressive. If a symbol can't be reached from a live entry point,
it goes. Recovery via `git log -S '<symbol>'` if anything was removed in
error. Per CLAUDE.md "Delete dead code on sight."

## Method

Five passes:

1. Every `#![allow(dead_code)]` (module-level) — 2 hits.
2. Every `#[allow(dead_code)]` (item-level) — 54 hits across 16 files.
3. Every commented-out `mod` declaration in `main.rs` / `lib.rs`.
4. Every orphan `.rs` file (no `mod` declaration anywhere).
5. Symbol-grep for each item under an allow to verify whether it has
   live callers.

Checked workspace: `rust/crates/cli/src/`, `rust/crates/sim/src/`,
`rust/crates/ir/src/`, `rust/crates/io/src/`,
`rust/crates/observe/src/`, `rust/crates/external-harness/src/`,
`rust/crates/wasm/src/`.

## Findings

### Pass 1: module-level `#![allow(dead_code)]`

| file                                | reason given             | verdict           |
|-------------------------------------|--------------------------|-------------------|
| `cli/src/args/types.rs`             | none stated              | item-level allows where needed; remove the file-level blanket |
| `cli/src/fit/status.rs`             | "v1 status printer kept" | **delete file**   |

### Pass 2: item-level `#[allow(dead_code)]`

Grouped by category.

#### 2a. fit/ legacy v1 helpers (delete in Task 3 — already audited)

`cli/src/fit/mod.rs` lines 1265, 1307, 1363, 1441, 1741, 1794, 1821,
1835, 1851 — all v1 helpers (`prepare_v1_cell`, `build_v1_fit_run`,
`write_v1_stage_run`, `read_v1_stage_best`,
`parse_optional_starts_from`, etc.). Commit `246a1aa` (2026-04-20)
explicitly noted "actual deletion is a follow-up cleanup." This is
that follow-up.

`cli/src/fit/mod.rs` lines 12, 17, 19, 21 — module-level allows on
`config_v2`, `scout`, `refine`, `validate`. Items inside these modules
follow.

`cli/src/fit/scout.rs`, `cli/src/fit/refine.rs`, `cli/src/fit/validate.rs`
— their `run_*` entry points are dead. Some helpers
(e.g. `now_iso8601_pub`) are still called from `cmd_fit_run_v2` and
must survive.

#### 2b. Stale "consumed by step N" markers (verify, then remove allow)

The Unit A pipeline shipped in full (`d22e0d8`). These markers
predicted future use that has now happened — the items *are* live;
the `#[allow(dead_code)]` is stale.

| location                                 | item                       | actually live? |
|------------------------------------------|----------------------------|----------------|
| `fit/loglik_eval.rs:162,181,198`          | CandidateScore, ChainWinner, LoglikEvalOutcome | yes — written + read everywhere |
| `fit/runner.rs:87,578,954`               | ChainResults marker, helpers | yes |
| `fit/synthetic.rs:32`                    | sim_seed / content_hash    | grep needed   |
| `cli/src/evidence.rs:123,158`            | "used by Unit A compound gate" | Unit A shipped — yes |

Action: **remove the allow** on each; let the compiler confirm the
items are live. If the compiler complains, delete.

#### 2c. main.rs comprehensive allows (5 lines + color helpers)

`cli/src/main.rs` lines 4, 6, 8, 12, 16, 25, 40, 42, 44, 46.

- Line 4 (`run_meta`): "wire sites in commit 4/6 of the unified-output-tree
  rollout." That rollout shipped. Verify.
- Line 6 (`run_paths`): same.
- Line 8 (`cas`): "wired up by follow-up commits (obs caching)." Verify.
- Line 12 (`batch`): used as live entry point. Verify why allow is needed.
- Line 16 (`pfilter`): "used internally by fit runner for data loading."
  Verify.
- Line 25 (`if2`): top-level `mod if2;` allow. Live entry point —
  Command::If2(a) at 280. Why still under allow?
- Lines 40, 42, 44, 46 (term color helpers): green / yellow / red / cyan.
  Need to check actual usage. If `bold` and `dim` are used but the
  others aren't, delete the unused ones.

#### 2d. util.rs

`cli/src/util.rs` lines:
- 27, 43: "progressively replacing the inline pattern; not all call
  sites migrated." This kind of comment is a smell — finish the
  migration or revert.
- 53, 60: "used by voi (gated — not in alpha)." Voi is commented out
  (see Pass 3); these are dead with it.

#### 2e. Other CLI items

- `cli/src/progress.rs:72, 129`: verify usage.
- `cli/src/browse.rs:793`: verify.
- `cli/src/fit/config.rs:242, 297`: verify.
- `cli/src/fit/trace_writer.rs:93`: verify.

#### 2f. sim/ inference items

- `sim/src/inference/types.rs:61`: verify.
- `sim/src/inference/nuts.rs:127`: verify (NUTS is live; what's dead?).
- `sim/src/inference/pgas_grad.rs:246`: "Disabled alongside gamma
  density in complete_data_loglik." Branch of code disabled but kept;
  if disabled, delete — git restores it if the gamma density gets
  re-enabled.
- `sim/src/inference/correlated_pf.rs:103`: verify.
- `sim/src/inference/diagnostic.rs:353`: verify.

#### 2g. external-harness items

5 items in `subprocess.rs`, `hashing.rs`, `main.rs`, `runner.rs`.
All small; verify each.

### Pass 3: commented-out modules

`cli/src/main.rs:14`: `// mod serve; // not mature enough for alpha`
→ corresponds to `cli/src/serve.rs` (87 lines, orphan file, no `mod`
declaration that compiles). **Delete the file and the comment.**

`cli/src/main.rs:24`: `// mod voi; // not mature enough for alpha`
→ corresponds to `cli/src/voi.rs` (669 lines, orphan file). **Delete
the file, the comment, and the `voi`-only helpers in `util.rs`.**

These are two whole files that aren't compiled, plus their helpers.
Pure context tax. If voi/serve come back, recover from git.

### Pass 4: orphan .rs files (no mod declaration)

Identified above:
- `cli/src/serve.rs` — 87 lines.
- `cli/src/voi.rs` — 669 lines.

Total: ~756 lines of source the compiler doesn't see.

### Pass 5: known dead from the fit-command audit

Already documented in `2026-04-27-fit-command-audit.md`:
- `args::Cli` + `args::Command` + `args::FitCommand` (~120 lines).
- `fit::scout::run_scout` (~250) + helpers.
- `fit::refine::run_refine` (~250) + helpers.
- `fit::validate::run_validate` (~250) + helpers (audit profile-likelihood
  code separately).
- `fit/status.rs` (~330).

## Triage

### Bucket 1: delete-now (no design ambiguity)

| target                                                           | est. lines |
|------------------------------------------------------------------|------------|
| `args::Cli`/`Command`/`FitCommand` (Task 2)                      | ~120       |
| `fit::scout/refine/validate::run_*` + helpers (Task 3)           | ~1,200     |
| v1 helpers `prepare_v1_cell` etc. in `fit/mod.rs` (Task 3)       | ~300       |
| `fit/status.rs` (Task 4)                                         | ~330       |
| Module-level `#[allow(dead_code)]` blanket attrs (Task 6)        | mechanical |
| `cli/src/serve.rs` (Task 8a)                                     | ~87        |
| `cli/src/voi.rs` (Task 8a)                                       | ~669       |
| voi helpers in `util.rs:51,53,58,60` (Task 8a)                   | ~30        |
| `// mod serve;` and `// mod voi;` comments in main.rs (Task 8a)  | mechanical |
| **Total bucket 1**                                               | **~2,750** |

### Bucket 2: verify-then-delete

The "consumed by step N" markers (§2b) — items are live; just remove
the allows. The various "verify" items in §2c–§2g — read the code,
decide. Each is small.

### Bucket 3: keep with named reason

Some items legitimately have an external use case that compiles only
under a feature flag or behind a downstream caller. These keep
`#[allow(dead_code)]` but with a *named, specific* reason in the
comment.

Examples likely to land here:
- `sim/inference/pgas_grad.rs:246` — gamma-density branch is disabled
  but referenced by a commented-out call site that may return when
  gamma priors are re-enabled. **Decision: delete — git restores when
  needed.**
- WASM-specific helpers in `cli/` that are dead in the native build —
  none found, but watch for them.

### Bucket 4: design-ambiguity items (file as issues, not delete)

None identified in this scan. Everything in §1 either reaches a live
caller or is unambiguously dead.

## What I'm NOT scanning

- **`agent-channel.md` (9,790 lines).** The downstream-coordination
  log is large but not "dead code" — it's a project artefact like a
  lab notebook. Out of scope for this cleanup; if we want to truncate
  / archive it, that's a separate decision.
- **OCaml side.** The user asked for a workspace scan; OCaml is a
  separate codebase with different idioms. Worth a parallel pass
  later, not in this session.
- **Test fixtures and goldens.** Some `.tsv` / `.ir.json` may be
  orphaned, but they cost ~kilobytes, and removing the wrong fixture
  is a different failure mode than removing the wrong source. Defer.

## Cleanup execution order

Per the task list:

1. Task 1 — this scan (in progress, completes when this note lands)
2. Task 2 — delete duplicate `args::Cli` parser tree
3. Task 3 — delete v1 stage entry points + their helpers
4. Task 4 — delete `fit/status.rs`
5. Task 5 — rename `fit/summary.rs` → `fit/grid_summary.rs`
6. Task 6 — drop module-level `#[allow(dead_code)]` from `fit/mod.rs`
7. Task 7 — reconcile `<stage>_summary.json` docs vs reality
8. Task 8 — workspace cleanup (orphan files, main.rs allows, util voi
   helpers, sim/external-harness items)

Each commit independently passes:
- `cargo test --workspace`
- `cargo test --test external_validation`
- `make test` (36 batch integration cases via pre-push)

After this cleanup:
- `~2,750+ lines deleted` from bucket 1
- `~10–15 stale allows removed` from bucket 2
- Zero `#![allow(dead_code)]` files in the source tree
- Module-level `#[allow(dead_code)]` only at item sites with
  explicitly-named reasons (per CLAUDE.md)
