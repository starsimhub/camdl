#!/usr/bin/env bash
# Stage 1 of 3: the brief.
#
# What the reviewing agent reads first: the proposal, the cleanup
# checklist, the git log, and a commit-by-commit diff summary. Small
# (~100 KB) so the agent can ingest the scope before touching code.
#
# Output: review-01-brief.zip

set -euo pipefail
cd "$(dirname "$0")/.."
REPO=$(pwd)

OUT="$REPO/review-01-brief.zip"
STAGING=$(mktemp -d)
trap 'rm -rf "$STAGING"' EXIT

echo "staging brief under $STAGING"

# Design doc + self-audit checklist.
mkdir -p "$STAGING/docs/dev/proposals"
cp docs/dev/proposals/2026-04-19-unified-output-tree.md "$STAGING/docs/dev/proposals/"
cp cleanup.md "$STAGING/"

# Git log: commits since the last pre-cleanup commit (ddd12de was the
# final commit of the initial unification; everything after is cleanup).
git log --oneline ddd12de..HEAD > "$STAGING/git-log.txt"

# Diff stats per commit — lets the agent see blast radius at a glance.
git log ddd12de..HEAD --stat --format="=== %h %s ===%n" > "$STAGING/git-diffstat.txt"

# Full README of what's in here and how to read it.
cat > "$STAGING/README.md" <<'EOF'
# Review stage 1 of 3: the brief

This zip contains the scope and self-audit of a refactor that unified
the camdl sim + fit output trees into a single content-addressable
`output/` tree discriminated by a `Run`/`RunKind` ADT.

Files:
- `docs/dev/proposals/2026-04-19-unified-output-tree.md` — the
  design doc that drove the work.
- `cleanup.md` — self-audit of what shipped against the proposal,
  plus the follow-up cleanup items worked through. Ticks (`[x]`),
  intentional partials (`[~]`), and zero open items.
- `git-log.txt` — one-line commit summary from the unification point
  through the cleanup pass.
- `git-diffstat.txt` — per-commit file-level blast radius.

**What you're reviewing:** whether the design in the proposal was
faithfully implemented, whether the cleanup items actually fixed the
issues they claim, and whether there are bugs / design smells / test
gaps I missed.

**Next stages** (separate zips):
- Stage 2: `review-02-code.zip` — every source + test file touched
  since the unification. ~1-2 MB.
- Stage 3: `review-03-book.zip` — book chapters that depend on the
  output-tree shape (fitting walkthroughs). Optional, only if you
  want to verify downstream doc consistency.

Please read this stage fully before asking for stage 2.
EOF

( cd "$STAGING" && zip -qr "$OUT" . )
echo "wrote $OUT ($(du -h "$OUT" | cut -f1))"
