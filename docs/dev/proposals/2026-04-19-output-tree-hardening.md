---
status: implemented
date: 2026-04-19
supersedes-partially: 2026-04-19-unified-output-tree.md
shipped:
  - c72d2ae  fix(fit): guard PGAS rename with error-on-collision
  - e3cbed9  fix(run_meta): StartsFromRef.stage_hash → Option<String>
  - facd40c  revert: default output root back to 'results/'
  - 6c605e2  fix(hashing): widen fit_content_hash to full 64-char hex
  - a4eb51e  fix(fit): mle_params.toml.input_hash uses full fit Run.hash
  - 7077df7  fix(hashing): canonicalise TOML before hashing
  - fe9ab65  fix(run_meta): atomic run.json write via .tmp + rename
  - 885d1ca  feat(fit): `camdl fit where FIT.toml [--seed N]`
  - 6a5993e  feat(fit): --starts-from accepts short-hash prefix
---

# Output-Tree Hardening

## Background

The unified output tree shipped in commit range `cafc4d4..a23d0a6`
(proposal: `2026-04-19-unified-output-tree.md`). Downstream review
of that changeset flagged seven concrete issues — all verified
against the code, none speculative. This proposal consolidates the
fixes and records the deferral list.

Two sections: **Ship now** (correctness, cache honesty, provenance
integrity, naming revert) and **Defer** (concurrent writers,
TOML canonicalization — nice-to-have, not load-bearing).

---

## Ship now

### 1. Atomic `run.json` write via `.tmp` + rename

**Problem.** `mle_params.toml` and `fit_state.toml` are written early
in the stage loop (`fit/mod.rs:867, 940`). `run.json` is written
last (`~:1153`). A crash between them leaves a directory that looks
complete to any script reading `mle_params.toml` — the file exists,
parses, has sensible values — but the stage wasn't finished. The
unified tree's whole story is "`run.json` is the authoritative
'this stage finished cleanly' marker"; right now that promise
can't be relied on.

**Fix.** `run_meta::Run::write` writes to `<dir>/run.json.tmp`, then
renames to `<dir>/run.json`. POSIX rename within the same filesystem
is atomic — either the new file is there in full or it isn't there
at all; readers never observe a half-written or truncated JSON. Same
for the top-level fit-root `Run::Fit`.

**Invariant.** After this change: if `run.json` exists, every
sibling artifact is valid (was written before the rename). If it
doesn't, treat the stage as incomplete regardless of what else is
in the directory.

**Affected call sites.** Every `run.write(&dir)` — simulate `--cas`,
batch sweep, fit top-level, fit stages, v1 subcommand wrappers.
All route through `Run::write`, so the fix is in one place.

**Cost.** Trivial. One extra syscall per write; no new dependencies.

### 2. Store full 64-char hash in `Run.hash`, truncate only at path layer

**Problem.** `fit_content_hash` truncates to 4 bytes → 8 hex chars
(`hashing.rs:180`). `Run.hash`'s docstring says "full 64-char hex."
For `FitMeta` it's actually 32 bits. Collision risk at ~65k distinct
fits (birthday bound). Also, the docstring lies.

**Fix.** Two-step:
- `hashing::fit_content_hash` returns full 64-char hex (drop the
  `[..4]` slice before hex-encoding).
- `run_paths::fit_run_dir` remains the only place that truncates,
  via `[..8.min(len)]`, for the human-readable directory name.
  Filesystem presentation and storage key decoupled.

**Side-effect.** Fit directories for existing runs get a new
content hash from the storage layer's perspective. Since the
filesystem dir name still uses the 8-char prefix, existing dirs
keep their *names*; what changes is what's inside `run.json` and
how `Run::check_cache` compares. Existing `run.json` files become
stale-on-read (their stored hash is 8 chars, we now compute 64);
this fires the "stale cache" path, which re-runs. Annoying but
correct, and this is exactly why the staleness detector exists.

**Tests to update.** The `golden_hash_*` regression tests in
`hashing.rs` still pin `scen_hash` + `sim_hash` + `model_hash` —
unaffected. No frozen test pins `fit_content_hash` today (miss);
we add one in this commit.

### 3. `mle_params.toml.input_hash` = full fit `Run.hash`

**Problem.** `mle_params.toml`'s comment header records
`input_hash = model_hash[..8]` (`fit/mod.rs:927`). That scope is
model-only, not the fit. Downstream can't correlate an
`mle_params.toml` back to its originating fit by hash — only
matches when data + params haven't changed between fits on the
same model, which happens rarely.

**Fix.** Replace `model_hash[..8]` with the full `Run.hash` from
the enclosing `Run::Fit`. The `run_fit` struct is already
constructed at the top of `cmd_fit_run_v2` before the stage loop
(`fit/mod.rs:437`); thread `&run_fit.hash` into the `MleMetadata`
struct at the call site.

**After this fix,** given an `mle_params.toml` in isolation, a
user (or script, or downstream agent) can locate its fit root
deterministically: full hash → `output/fits/<stem>-<hash[..8]>/`
with `run.json` containing the full hash for unambiguous match.
This is what a content-addressable store is for.

**Pairs with #2.** Free once #2 lands — the full hash to embed is
just `run_fit.hash` (now 64 chars instead of 8).

### 4. PGAS/PMMH rename: error on collision, not silent clobber

**Problem.** `fit/mod.rs:980` does
`std::fs::remove_dir_all(&stage_dir)` before renaming `pgas/` to
the target stage name. Concurrent fits against one `fit_dir`
race here; even serial iterations of the same fit mid-edit can
silently wipe a previous stage's results.

**Fix (now).** Replace the silent `remove_dir_all` with:
- If `stage_dir` exists and `--force` was passed: remove, warn,
  proceed (current behavior but loud).
- If `stage_dir` exists and `--force` wasn't passed: error, exit 1,
  tell the user either to pass `--force` or to delete manually.
- If `stage_dir` doesn't exist: proceed.

**Deferred (later — see "Defer" §).** Real concurrent-writer
support via lockfile + partial-write recovery.

### 5. `StartsFromRef.stage_hash`: `Option<String>` instead of empty string

**Problem.** When the upstream stage's `run.json` can't be read
(`fit/mod.rs:1106-1116`), `stage_hash` falls back to empty string.
The provenance chain then looks structurally intact but silently
loses its backward reference. B1 in cleanup.md made this less bad
(we now *try* to read the upstream run.json first), but the
error-path fallback is still a silent corruption.

**Fix.** Change `StartsFromRef.stage_hash: String` to
`Option<String>`. On read error: `stage_hash: None` with an explicit
`eprintln!` warning naming the upstream directory that couldn't be
read and what that means for provenance. Absent ≠ empty; absent is
the honest signal.

**Schema migration.** Existing `run.json` files have
`"stage_hash": ""`. Serde `#[serde(default)]` on the `Option` field
+ a migration note: empty-string on read deserializes to
`Some("")` by default; we'd need a custom deserializer that maps
empty-string → `None` to treat old records consistently with new.
Worth it; one small function.

### 6. TOML canonicalization in `fit_content_hash`

**Problem.** Current: `fit_content_hash` hashes raw fit.toml bytes
+ `VERSION_SHORT`. A comment or whitespace edit busts the cache;
so does a `camdl` version bump that didn't change inference
behavior. Users are surprised by unexplained recomputation.

**Fix.** Parse fit.toml → serialize back in canonical form (sorted
keys, stripped comments, normalized whitespace) → hash the canonical
bytes. Uses `toml` crate, no new dependency (already used
everywhere for fit-config loading).

**Trade.** We lose "edit a comment, get a new dir" as a property.
Net positive — comments in fit.toml are for humans, not provenance
inputs. The `VERSION_SHORT` component stays (the proposal keeps it
because code-path changes that alter fit semantics without changing
the TOML need some way to bust the cache; discussed in the original
unification proposal, unchanged).

**Testing.** Add a test: two fit.tomls that differ only in whitespace
and comments must produce the same `fit_content_hash`. Conversely,
a real config change must still produce a different hash.

### 7. Revert default `output/` → `results/`

**Problem.** The unified-tree proposal (mine) flipped the default
from `results/` to `output/` on aesthetic grounds ("output is
CLI-generic"). Downstream's counter is stronger:
- Prior convention: book + vignettes baked `results/` into path
  literals and scripts, already paid the migration cost once.
- Research-domain fit: "results" pairs with "data"; "output"
  connotes build-artifact (and "rm -rf the output dir" is the
  intuition that follows from that naming, which is wrong for
  artifacts users might want to archive).

**Fix.** `run_paths::DEFAULT_OUTPUT_ROOT` constant: `"output"` →
`"results"`. Update:
- `FitConfigV2::fit_dir` default.
- v1 `FitToml::fit.output_dir` default.
- Help text in `main.rs` fit section.
- Docstrings in `run_paths.rs`, `cleanup.md`.
- Integration tests (`synthetic_fit_grid.rs`, `cas_integration.rs`)
  where the tempdir path assembles `output/`.
- Book chapters — already clean (they set explicit `output_dir`),
  but the `find_fit_root` helper's default needs matching.

### 8. `camdl fit where fit.toml [--seed N]`

**New command.** Resolves a fit.toml to its fit directory (and
optionally its cell directory under `real/fit_<seed>/`) without
running anything. Prints the path on stdout.

```
$ camdl fit where fits/01.toml
results/fits/01-deadbeef/

$ camdl fit where fits/01.toml --seed 42
results/fits/01-deadbeef/real/fit_42/
```

Usage: Python/R snippets in book scripts can shell out to this
instead of globbing on the stem prefix. Solves the "I know the
fit.toml, I want the dir" lookup directly; glob-based lookup in
`find_fit_root` becomes a fallback for cases where only the name
is known and not the config file.

Implementation: ~30 lines. `FitConfigV2::load` + `fit_dir()` + `println`.
Dispatch under `fit where` in `main.rs` alongside `fit run`.

### 9. `--starts-from <hash>` short-hash resolution

**Current.** `--starts-from` takes a directory path. Downstream
orchestration scripts have to know or compute the path.

**Fix.** Extend the parser: if the arg contains `/` → treat as
path (today's behavior). Else → resolve as a short-hash prefix via
the already-shipped `resolve_any`. Because `resolve_any` handles
both sim and fit hashes, a stage-hash prefix (for stages under a
fit) needs a small extension — walk `output/fits/*/*/fit_*/*/run.json`
and match on `FitStage.stage_hash` (or `Run.hash`, equivalent after
fix #2).

**Cost.** ~50 lines, mostly in `browse.rs`'s resolver + a parser
arm in fit/mod.rs's `parse_starts_from`.

---

## Defer

### D1. Concurrent-writer support

**What it is.** Real safety for multiple processes writing to the
same `fit_dir`: lockfile discipline, partial-write recovery, stage-
level write coordination. Needed for batch/coverage workflows that
parallelize across cells but share a fit root.

**Why defer.** Out of scope for "make the feature we just shipped
solid." The minimum fix for #4 (error instead of silent clobber)
removes the immediate footgun; concurrent-safe writing is a design
in its own right, belongs in a separate proposal when a concrete
workflow asks for it.

**Mitigation meanwhile.** Document the single-writer-per-fit-dir
contract in `docs/camdl-run-spec.md`. Downstream batch jobs should
either sequence per fit_dir or partition by fit_hash.

### D2. Automatic migration for existing `run.json` files

**What it is.** When reading a pre-fix `run.json` (8-char
`Run.hash` for fits, empty-string `stage_hash`), automatically
upgrade in place to the new schema.

**Why defer.** Treating existing files as stale and re-running is
the cheaper path — we have no pre-release users who care, the cache
is the only "migration" layer needed, and staleness detection
(`Run::check_cache → Stale`) already handles this gracefully
(warn + re-run). If someone later has a persistent cache they can't
afford to invalidate, revisit.

### D3. `camdl fit diff <hash> <hash>` + `camdl fit lineage`

Navigation conveniences on top of the hash graph — see what
changed between two fits, walk the `starts_from` chain. Natural
but additive; wait for a scripting workflow that actually needs
them.

### D4. Provenance `status` field

Longer-term: a `status: "running" | "completed" | "failed"` field
in `Run` so partial runs announce themselves. Less urgent once #1
(atomic rename) ships — existence of `run.json` becomes the status
signal. Revisit if we find cases where a completed-but-invalid run
is a class we need to enumerate.

---

## Implementation plan

Rough order of operations, optimizing for standalone commits that
each lock down one invariant:

1. **#4** — guard PGAS rename with error-on-collision. 10 lines,
   test in isolation. First because it's trivial and unblocks the
   minimum concurrent-safety story.
2. **#5** — `StartsFromRef.stage_hash` → `Option<String>`. Small,
   schema-cascade. Touches serde + v1/v2 write sites + tests.
3. **#7** — naming revert `output/` → `results/`. Mechanical;
   touches doc + default constants + tests. Flush this before
   deeper refactors to minimize rebase churn in other commits.
4. **#1** — hash widening. Then cascades.
5. **#2** — `input_hash` in `mle_params.toml` uses full fit hash.
   Lands after #1, free.
6. **#6** — TOML canonicalization in `fit_content_hash`. After #1
   so only one change to the hash test's golden.
7. **#3** — atomic `run.json` write via `.tmp` + rename. One-place
   change in `Run::write`; all callers inherit.
8. **#8** — `camdl fit where`. Standalone new command.
9. **#9** — `--starts-from <hash>`. Reuses `resolve_any`; new
   parser arm.

Each commit stands alone + compiles + tests pass. At the end, one
doc commit updates `docs/camdl-run-spec.md` with the single-writer
contract (D1 mitigation) and cross-refs this proposal's shipping
commits in its status header (following the 2026-04-19 PF-traj
proposal precedent).

## Verification

- Cargo workspace tests all green after each commit.
- New frozen-hash test pinning `fit_content_hash` to 64 chars.
- New atomicity test: write `run.json` with a large enough payload
  that a hypothetical half-write would be observable, verify no
  `run.json.tmp` left behind on clean completion, verify that a
  simulated crash-before-rename leaves the directory without
  `run.json`.
- New test for TOML canonicalization: two semantically-identical
  fit.tomls produce the same `fit_content_hash`.
- Regression test for the guarded rename: pre-existing stage dir
  + no `--force` → process exits 1 with a named-path error.
- `camdl fit where` integration test.
- `--starts-from <hash>` resolves a known stage hash.

## Test plan for review-request.md cross-cutting findings

The original review listed things it looked for and didn't find.
Each fix closes one of those; no additional adversarial testing
beyond "the finding is gone" is needed. The test set above is
comprehensive.

## Acknowledgments

The review finding these issues (`agent-channel.md:8571`) is a
direct example of why we run external code review after shipping
big refactors. The two-hour pass that produced the findings
covered four real bugs (critical + three majors) that the internal
cleanup checklist missed — because the internal checklist was
scoped to the author's mental model of what could go wrong, and
the external reviewer checks the cases outside that model.
