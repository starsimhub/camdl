#!/bin/bash
# Generate targeted review zips for external code review.
# Each zip is sized to fit in Claude's context window.
#
# Usage:
#   ./scripts/review-zip.sh inference    # ~135K tokens — inference algorithms + CLI
#   ./scripts/review-zip.sh compiler     # ~80K tokens — OCaml compiler + IR
#   ./scripts/review-zip.sh docs         # ~190K tokens — all docs + DSL examples
#   ./scripts/review-zip.sh engine       # ~120K tokens — simulation backends + propensity
#   ./scripts/review-zip.sh all          # generates all four

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

OUTDIR="${REVIEW_OUTDIR:-review-zips}"
mkdir -p "$OUTDIR"
DATE=$(date +%Y%m%d)

estimate_tokens() {
    git archive HEAD -- "$@" 2>/dev/null | tar -xf - -O 2>/dev/null | wc -c | awk '{printf "%.0fK", $1/4/1000}'
}

zip_inference() {
    local out="$OUTDIR/review-inference-$DATE.zip"
    git archive HEAD --prefix=camdl/ -o "$out" -- \
        rust/crates/sim/src/inference/ \
        rust/crates/sim/src/chain_binomial.rs \
        rust/crates/sim/src/propensity.rs \
        rust/crates/sim/src/resolved_expr.rs \
        rust/crates/sim/src/compiled_model.rs \
        rust/crates/sim/src/rng.rs \
        rust/crates/sim/src/error.rs \
        rust/crates/sim/src/state.rs \
        rust/crates/sim/src/lib.rs \
        rust/crates/cli/src/fit/ \
        rust/crates/ir/src/ \
        rust/crates/sim/tests/ \
        docs/inference.md \
        docs/dev/incidents/ \
        CLAUDE.md
    local tokens=$(estimate_tokens \
        rust/crates/sim/src/inference/ \
        rust/crates/cli/src/fit/ \
        rust/crates/ir/src/ \
        rust/crates/sim/tests/ \
        docs/inference.md CLAUDE.md)
    echo "inference: $out (~${tokens} tokens)"
}

zip_compiler() {
    local out="$OUTDIR/review-compiler-$DATE.zip"
    git archive HEAD --prefix=camdl/ -o "$out" -- \
        ocaml/lib/ \
        ocaml/bin/ \
        ocaml/test/ \
        ocaml/golden/*.camdl \
        ocaml/golden/*.params.toml \
        ocaml/golden/data/ \
        rust/crates/ir/src/ \
        docs/camdl-language-spec.md \
        docs/compartmental-ir-spec.md \
        CLAUDE.md
    local tokens=$(estimate_tokens \
        ocaml/lib/ ocaml/bin/ ocaml/test/ \
        ocaml/golden/*.camdl \
        rust/crates/ir/src/ \
        docs/camdl-language-spec.md CLAUDE.md)
    echo "compiler: $out (~${tokens} tokens)"
}

zip_docs() {
    local out="$OUTDIR/review-docs-$DATE.zip"
    git archive HEAD --prefix=camdl/ -o "$out" -- \
        docs/ \
        CLAUDE.md \
        README.md \
        ocaml/golden/*.camdl \
        ocaml/golden/*.params.toml \
        rust/crates/sim/src/inference/pgas.rs \
        rust/crates/sim/src/inference/nuts.rs \
        rust/crates/sim/src/inference/pmmh.rs \
        rust/crates/sim/src/inference/if2.rs \
        rust/crates/cli/src/fit/runner.rs
    local tokens=$(estimate_tokens \
        docs/ CLAUDE.md README.md \
        ocaml/golden/*.camdl)
    echo "docs: $out (~${tokens} tokens)"
}

zip_engine() {
    local out="$OUTDIR/review-engine-$DATE.zip"
    git archive HEAD --prefix=camdl/ -o "$out" -- \
        rust/crates/sim/src/ \
        rust/crates/ir/src/ \
        rust/crates/sim/tests/ \
        rust/crates/sim/Cargo.toml \
        ocaml/golden/*.camdl \
        docs/runtimes.md \
        docs/compartmental-ir-spec.md \
        CLAUDE.md
    local tokens=$(estimate_tokens \
        rust/crates/sim/src/ \
        rust/crates/ir/src/ \
        rust/crates/sim/tests/ \
        docs/runtimes.md CLAUDE.md)
    echo "engine: $out (~${tokens} tokens)"
}

case "${1:-all}" in
    inference) zip_inference ;;
    compiler)  zip_compiler ;;
    docs)      zip_docs ;;
    engine)    zip_engine ;;
    all)
        zip_inference
        zip_compiler
        zip_docs
        zip_engine
        echo "---"
        ls -lh "$OUTDIR"/review-*-$DATE.zip
        ;;
    *)
        echo "Usage: $0 {inference|compiler|docs|engine|all}"
        exit 1
        ;;
esac
