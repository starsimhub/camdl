#!/usr/bin/env bash
# review-zip.sh — canonical code-review zip generator.
#
# This is the single entry point for producing review zips. See
# scripts/README.md for the design rationale (why four subsystems,
# why the plumbing layer is shared, why git-archive rather than
# copy).
#
# Usage:
#   ./scripts/review-zip.sh <subsystem>    # one subsystem zip
#   ./scripts/review-zip.sh all            # every subsystem
#   ./scripts/review-zip.sh full           # whole repo (no slicing)
#   ./scripts/review-zip.sh list           # subsystems + token estimates
#   ./scripts/review-zip.sh clean          # rm review-zips/*.zip
#
# Subsystems:
#   inference   — fit algorithms (IF2/PGAS/NUTS/PMMH/PF) + fit CLI
#                 + shared plumbing. Anchor for most inference work.
#   engine      — simulation backends (Gillespie/tau-leap/ODE/CB) +
#                 propensity + shared plumbing. Anchor for simulate
#                 + observation work.
#   compiler    — OCaml DSL → IR.
#   docs        — specs, proposals, dev notes.
#
# Output: review-zips/review-<subsystem>-<YYYYMMDD>.zip
# Environment:
#   REVIEW_OUTDIR overrides the output directory (default: review-zips).

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

OUTDIR="${REVIEW_OUTDIR:-review-zips}"
DATE=$(date +%Y%m%d)

# ─── Subsystem file lists ─────────────────────────────────────────────
#
# Each subsystem declares an array of paths (files or dirs) that
# git-archive will pull from HEAD. Keep these lists explicit — no
# wildcards beyond what git-archive naturally supports — so the scope
# of each review is auditable by reading this file.

# Shared CLI plumbing: cache/metadata/path/browse infrastructure used
# by both inference (fit) and engine (simulate) code paths. Duplicated
# into both zips rather than given its own subsystem — reviewers of
# either inference or engine work need this context to trace data
# flow end-to-end (§"Shared plumbing" in README.md).
CLI_PLUMBING=(
    rust/crates/cli/src/main.rs
    rust/crates/cli/src/browse.rs
    rust/crates/cli/src/cas.rs
    rust/crates/cli/src/run_meta.rs
    rust/crates/cli/src/run_paths.rs
    rust/crates/cli/src/hashing.rs
    rust/crates/cli/src/batch.rs
    rust/crates/cli/src/serve.rs
    rust/crates/cli/src/util.rs
    rust/crates/cli/src/version.rs
    rust/crates/cli/tests/
)

INFERENCE=(
    rust/crates/sim/src/inference/
    rust/crates/sim/src/compiled_model.rs
    rust/crates/sim/src/propensity.rs
    rust/crates/sim/src/resolved_expr.rs
    rust/crates/sim/src/rng.rs
    rust/crates/sim/src/error.rs
    rust/crates/sim/src/state.rs
    rust/crates/sim/src/lib.rs
    rust/crates/cli/src/fit/
    rust/crates/cli/src/pfilter.rs
    rust/crates/cli/src/if2.rs
    rust/crates/cli/src/profile.rs
    rust/crates/cli/src/sampling.rs
    rust/crates/ir/src/
    rust/crates/sim/tests/
    "${CLI_PLUMBING[@]}"
    docs/camdl-inference-spec.md
    docs/camdl-run-spec.md
    docs/inference.md
    docs/dev/incidents/
    CLAUDE.md
)

ENGINE=(
    rust/crates/sim/src/
    rust/crates/sim/tests/
    rust/crates/sim/Cargo.toml
    rust/crates/cli/src/eval.rs
    rust/crates/cli/src/data.rs
    rust/crates/ir/src/
    "${CLI_PLUMBING[@]}"
    ocaml/golden/
    docs/runtimes.md
    docs/compartmental-ir-spec.md
    docs/camdl-run-spec.md
    CLAUDE.md
)

COMPILER=(
    ocaml/lib/
    ocaml/bin/
    ocaml/test/
    ocaml/golden/
    rust/crates/ir/src/
    docs/camdl-language-spec.md
    docs/compartmental-ir-spec.md
    CLAUDE.md
)

DOCS=(
    docs/
    CLAUDE.md
    README.md
    ocaml/golden/
)

# ─── Helpers ──────────────────────────────────────────────────────────

estimate_tokens() {
    # Approximation: 1 token ≈ 4 bytes of source text. Good enough for
    # deciding which zips to hand a reviewer in what order.
    git archive HEAD -- "$@" 2>/dev/null \
        | tar -xf - -O 2>/dev/null \
        | wc -c \
        | awk '{printf "%.0fK", $1/4/1000}'
}

make_zip() {
    local name=$1; shift
    local out="$OUTDIR/review-$name-$DATE.zip"
    mkdir -p "$OUTDIR"
    git archive HEAD --prefix="camdl/" -o "$out" -- "$@"
    local tokens
    tokens=$(estimate_tokens "$@")
    local bytes
    bytes=$(ls -l "$out" | awk '{print $5}')
    printf "  %-10s → %s (~%s tokens, %sB)\n" "$name" "$out" "$tokens" "$bytes"
}

# ─── Dispatch ─────────────────────────────────────────────────────────

cmd=${1:-help}
case "$cmd" in
    inference) make_zip inference "${INFERENCE[@]}" ;;
    engine)    make_zip engine    "${ENGINE[@]}"    ;;
    compiler)  make_zip compiler  "${COMPILER[@]}"  ;;
    docs)      make_zip docs      "${DOCS[@]}"      ;;

    all)
        echo "Generating all subsystem zips in $OUTDIR/:"
        make_zip inference "${INFERENCE[@]}"
        make_zip engine    "${ENGINE[@]}"
        make_zip compiler  "${COMPILER[@]}"
        make_zip docs      "${DOCS[@]}"
        ;;

    full)
        # Whole-repo snapshot. Useful when a reviewer needs the entire
        # tree in one blob rather than a scoped subsystem (new
        # contributor onboarding, bisection across subsystems).
        out="$OUTDIR/review-full-$DATE.zip"
        mkdir -p "$OUTDIR"
        git archive HEAD --prefix="camdl/" -o "$out"
        tokens=$(estimate_tokens ":/")
        bytes=$(ls -l "$out" | awk '{print $5}')
        printf "  %-10s → %s (~%s tokens, %sB)\n" "full" "$out" "$tokens" "$bytes"
        ;;

    list)
        echo "Available subsystems:"
        echo
        for sub in inference engine compiler docs; do
            case "$sub" in
                inference) tokens=$(estimate_tokens "${INFERENCE[@]}") ;;
                engine)    tokens=$(estimate_tokens "${ENGINE[@]}")    ;;
                compiler)  tokens=$(estimate_tokens "${COMPILER[@]}")  ;;
                docs)      tokens=$(estimate_tokens "${DOCS[@]}")      ;;
            esac
            printf "  %-10s ~%s tokens\n" "$sub" "$tokens"
        done
        echo
        echo "Plus:"
        echo "  all        generates every subsystem"
        echo "  full       whole-repo snapshot"
        echo "  clean      rm $OUTDIR/*.zip"
        ;;

    clean)
        if [ -d "$OUTDIR" ]; then
            rm -f "$OUTDIR"/*.zip
            echo "cleaned $OUTDIR/*.zip"
        else
            echo "no $OUTDIR/ to clean"
        fi
        ;;

    help|--help|-h)
        sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
        ;;

    *)
        echo "error: unknown subcommand '$cmd'" >&2
        echo "run '$0 help' for usage" >&2
        exit 1
        ;;
esac
