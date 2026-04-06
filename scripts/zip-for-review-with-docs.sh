#!/usr/bin/env bash
# zip-for-review-with-docs.sh — package source + docs for comprehensive review
#
# Usage:
#   scripts/zip-for-review-with-docs.sh
#
# Output: review-full-YYYYMMDD-HHMMSS.zip in the repo root.
#
# Same as zip-for-review.sh but explicitly includes:
#   - All docs/ (specs, proposals, reviews, blog)
#   - CLAUDE.md, README.md
#   - agent-channel.md (if present)
#
# Use this when the reviewer needs architectural context, not just code.

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
OUTFILE="$REPO_ROOT/review-full-${TIMESTAMP}.zip"

EXCLUDE_PATTERNS=(
    '*.tsbuildinfo'
    '*.tsv'
    '*.conflicts'
    'tree-sitter/*'
    '.claude/*'
    'output/*'
    'bin/*'
    'web/.vite/*'
    'review-*.zip'
    'rust/target/*'
    'ocaml/_build/*'
)

mapfile -t files < <(
    {
        git ls-files
        git ls-files --others --exclude-standard
    } | sort -u
)

filtered=()
for f in "${files[@]}"; do
    [[ -f "$f" ]] || continue
    skip=0
    for pat in "${EXCLUDE_PATTERNS[@]}"; do
        # shellcheck disable=SC2254
        case "$f" in $pat) skip=1; break;; esac
    done
    [[ $skip -eq 1 ]] && continue
    filtered+=("$f")
done

if [[ ${#filtered[@]} -eq 0 ]]; then
    echo "No files found after exclusions."
    exit 0
fi

# Count by category
n_code=$(printf '%s\n' "${filtered[@]}" | grep -cE '\.(rs|ml|mli|mll|mly)$' || true)
n_docs=$(printf '%s\n' "${filtered[@]}" | grep -c '^docs/' || true)
n_total=${#filtered[@]}

echo "Packaging for review:"
echo "  Source files: $n_code"
echo "  Documentation: $n_docs"
echo "  Total files: $n_total"

zip -q "$OUTFILE" "${filtered[@]}"
echo
echo "Created: $OUTFILE ($(du -h "$OUTFILE" | cut -f1))"
