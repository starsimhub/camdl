#!/usr/bin/env bash
# Stage 3 of 3: book chapters that depend on the fit output layout.
#
# Scoped to the three fitting chapters + scenarios.qmd (the ones
# touched by L5 in cleanup.md). Separate from the code zip so the
# reviewing agent can opt out — book review is orthogonal to the
# Rust-side review.
#
# Output: review-03-book.zip

set -euo pipefail

BOOK=/Users/vsb/projects/work/camdl-book
if [ ! -d "$BOOK" ]; then
  echo "error: camdl-book not found at $BOOK" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
REPO=$(pwd)
OUT="$REPO/review-03-book.zip"
STAGING=$(mktemp -d)
trap 'rm -rf "$STAGING"' EXIT

echo "staging book under $STAGING"

mkdir -p "$STAGING/guide"
for f in fitting.qmd fitting_backup_with_rho.qmd fitting_diagnostic.qmd scenarios.qmd; do
  cp "$BOOK/guide/$f" "$STAGING/guide/$f"
done

# Diff against the book's pre-cleanup state so the reviewer can see
# exactly what the L5 migration changed (vs re-reading whole files).
( cd "$BOOK" && git log --oneline -5 > "$STAGING/_book-git-log.txt" )
( cd "$BOOK" && git show HEAD -- guide/fitting.qmd guide/fitting_backup_with_rho.qmd \
    guide/fitting_diagnostic.qmd guide/scenarios.qmd ) \
  > "$STAGING/_book-diff.patch"

cat > "$STAGING/README.md" <<'EOF'
# Review stage 3 of 3: book chapters

Four Quarto files from `camdl-book` whose Python snippets depend on
the fit output layout. Each previously hard-coded paths like
`results/fits/fit_sir/refine/mle_params.toml` and now resolves the
content-addressable `output/fits/<stem>-<hash[:8]>/real/fit_<seed>/<stage>/`
tree via a `find_fit_root(stem)` / `find_fit_stage(stem, stage)`
helper that globs on the stem prefix and picks the most-recent match.

Files:
- `guide/fitting.qmd`                 — 10 path sites rewritten.
- `guide/fitting_backup_with_rho.qmd` — 8 sites rewritten.
- `guide/fitting_diagnostic.qmd`      — 8 sites rewritten.
- `guide/scenarios.qmd`               — 2 `output/runs/` → `output/sims/`.

Also:
- `_book-diff.patch` — full diff of the commit that landed these
  edits.
- `_book-git-log.txt` — last 5 commits in the book repo for context.

**What I want you to check:**

- Does `find_fit_root(stem)` have the right semantics? Specifically,
  "most-recent glob match" is correct when users re-run a fit with
  the same toml (creates a new hash → new dir), but ambiguous when
  two unrelated fits share a stem. Is there a case that breaks?
- Any remaining hard-coded `results/fits/…` I missed.
- Snippets where the rewritten path is correct but the *surrounding
  prose* still describes the old layout.
- The `find_fit_stage(stem, stage, seed=None)` default (glob on
  `fit_*` when seed is None) — does this ever pick the wrong seed
  cell silently? The book's existing fits all have a single seed
  today, but SBC chapters may have multiple.

Book `.html` derivatives are stale until the user runs quarto render;
that's expected. Don't treat it as a code issue.
EOF

( cd "$STAGING" && zip -qr "$OUT" . )
echo "wrote $OUT ($(du -h "$OUT" | cut -f1))"
