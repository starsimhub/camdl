#!/usr/bin/env bash
# Stage 2 of 3: every source + test file touched by the unification
# and the cleanup pass.
#
# Derives the file list from `git diff --name-only ddd12de..HEAD` so
# it stays honest about what actually changed — no manual curation
# that could drift from reality. Also bundles a tree listing and the
# cleanup checklist for context (the agent shouldn't need to flip
# back to stage 1 for the checklist).
#
# Output: review-02-code.zip

set -euo pipefail
cd "$(dirname "$0")/.."
REPO=$(pwd)

OUT="$REPO/review-02-code.zip"
STAGING=$(mktemp -d)
trap 'rm -rf "$STAGING"' EXIT

echo "staging code under $STAGING"

# Every file touched since the pre-cleanup baseline. Exclude:
# - cleanup.md (already in stage 1)
# - scripts/ (the zip scripts themselves aren't under review)
# - .claude/ (worktree metadata accidentally committed earlier; the
#   submodule pointers there aren't files we can copy anyway)
# - agent-channel*.md (scratch files not part of the refactor)
TOUCHED=$(git diff --name-only ddd12de..HEAD \
  | grep -v '^cleanup.md$' \
  | grep -v '^scripts/' \
  | grep -v '^\.claude/' \
  | grep -v '^agent-channel' \
  | grep -v '^docs/dev/proposals/2026-04-11' \
  || true)

# Copy each touched file, preserving directory structure. Skip
# directories (submodule pointers) rather than fail.
for f in $TOUCHED; do
  if [ -f "$f" ]; then
    mkdir -p "$STAGING/$(dirname "$f")"
    cp "$f" "$STAGING/$f"
  elif [ -d "$f" ]; then
    # Tracked as a submodule pointer; skip silently.
    :
  else
    # File was deleted — record that fact so the reviewer knows.
    echo "$f" >> "$STAGING/_deleted-files.txt"
  fi
done

# Include the cleanup checklist again (lightweight, context-keeping).
cp cleanup.md "$STAGING/"

# Full per-file diff against the baseline — agent reads code but
# starts from the diff to orient.
git diff ddd12de..HEAD -- \
  $(echo "$TOUCHED" | tr '\n' ' ') \
  > "$STAGING/_full-diff.patch" 2>/dev/null || true

# Listing of what's included, for sanity.
( cd "$STAGING" && find . -type f | sort > _file-list.txt )

cat > "$STAGING/README.md" <<'EOF'
# Review stage 2 of 3: code changes

Every source and test file touched by the output-tree unification
plus the follow-up cleanup pass. Directory structure mirrors the
repo so paths are legible.

Key files to read in roughly this order:

1. **Types + storage layer**
   - `rust/crates/cli/src/run_meta.rs` — the unified `Run` + `RunKind`
     ADT + `CacheStatus`. The design's type foundation.
   - `rust/crates/cli/src/run_paths.rs` — canonical path construction
     (`sim_run_dir`, `fit_run_dir`, `fit_stage_dir`, `output_root`).
   - `rust/crates/cli/src/hashing.rs` — consolidated hash helpers
     including frozen golden-hash regression tests.

2. **Write sites**
   - `rust/crates/cli/src/main.rs` (around `prepare_cas_ctx`, simulate
     `--cas` path) — sim write with hash-aware cache check.
   - `rust/crates/cli/src/batch.rs` — sweep/batch sim writes.
   - `rust/crates/cli/src/fit/mod.rs` — v2 `fit run` orchestrator
     (cell-dir layout, top-level Run::Fit at start + wall-time
     rewrite at end, per-stage Run::FitStage). Also contains the
     v1-migration helpers `prepare_v1_cell`, `build_v1_fit_run`,
     `write_v1_stage_run`.
   - `rust/crates/cli/src/fit/config.rs` — v1 `FitToml::fit_root` /
     `cell_dir` / `fit_content_hash`.
   - `rust/crates/cli/src/fit/config_v2.rs` — v2 `FitConfigV2::fit_dir`.

3. **Browse layer**
   - `rust/crates/cli/src/browse.rs` — `camdl list / show / cat`.
     Two-subtree walk, `--kind` filter, `resolve_any` cross-kind
     short-hash resolver.

4. **Tests**
   - `rust/crates/cli/tests/cas_integration.rs` — end-to-end tests
     exercising the binary.
   - `rust/crates/cli/tests/synthetic_fit_grid.rs` — synthetic-fit
     grid layout tests (updated for `<stem>-<hash[:8]>/` dir names).

5. **Docs in-repo**
   - `docs/camdl-run-spec.md` — §2.2 / §2.3 fit layout.
   - `rust/crates/cli/src/serve.rs` — usage text for the http
     serve command.

Also included:
- `cleanup.md` — the self-audit (same as stage 1).
- `_full-diff.patch` — full `git diff ddd12de..HEAD` across the
  included files. Useful if you want to see the delta rather than
  the final state.
- `_file-list.txt` — every path in this zip.
- `_deleted-files.txt` (if present) — files deleted during the pass.

**What I want you to check:**

- Bugs: anything incorrect in the write sites — wrong hash, wrong
  path, wrong field mapping, silent failures.
- Design smells: fields that don't belong, abstractions that leaked,
  duplication I missed, types that should be stricter.
- Test coverage gaps beyond what `cleanup.md` already acknowledges.
- Any case where the design doc's intent doesn't match what shipped.
- Cross-cutting issues: does the v1 migration actually work when v1
  commands are re-enabled? Does `Run::Fit.wall_time_seconds` actually
  get updated on interrupted fits? Is the `StartsFromRef.stage_hash`
  resolution robust to external `--starts-from` paths?

Don't feel obligated to defend my work — if something's wrong or
suboptimal, say so directly. Findings should be concrete (file +
line) and, where possible, come with a concrete fix.
EOF

( cd "$STAGING" && zip -qr "$OUT" . )
echo "wrote $OUT ($(du -h "$OUT" | cut -f1))"
