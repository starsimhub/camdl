#!/usr/bin/env bash
# zip-for-review.sh — package all source files for code review
#
# Usage:
#   scripts/zip-for-review.sh
#
# Output: review-YYYYMMDD-HHMMSS.zip in the repo root.
#
# Includes:
#   - All tracked files + untracked non-ignored files
# Excludes:
#   - Build artifacts, diagnostics, lock files, tsbuildinfo, tree-sitter/

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
OUTFILE="$REPO_ROOT/review-${TIMESTAMP}.zip"

# Noise patterns to exclude (glob-matched against each file path)
EXCLUDE_PATTERNS=(
    '*.tsbuildinfo'
    '*.tsv'
    '*.conflicts'
    'plan.md'
    'polio-plan.md'
    'tree-sitter/*'
    '.claude/*'
    'output/*'
    'bin/*'
    'web/.vite/*'
    'review-*.zip'
)

# Collect all tracked files + untracked non-ignored files
mapfile -t files < <(
    {
        git ls-files
        git ls-files --others --exclude-standard
    } | sort -u
)

# Filter to existing files, applying excludes
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
    echo "No changed files found vs $BASE (after exclusions)."
    exit 0
fi

echo "Files to include (${#filtered[@]}):"
for f in "${filtered[@]}"; do
    echo "  $f"
done

zip -q "$OUTFILE" "${filtered[@]}"
echo
echo "Created: $OUTFILE"
