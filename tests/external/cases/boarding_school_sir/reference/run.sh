#!/usr/bin/env bash
# Invoked by `external-harness regen` (or `run --regen`). Produces the
# pomp ensemble TSV the harness reads to compute the reference summary.

set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out

if [[ "${CAMDL_EXTERNAL_USE_DOCKER:-}" == "1" ]]; then
    echo "docker regen not yet wired for this case; run locally" >&2
    exit 1
fi

command -v Rscript >/dev/null || {
    echo "Rscript not found on PATH. Install R (e.g. 'brew install r') or rerun with CAMDL_EXTERNAL_USE_DOCKER=1." >&2
    exit 1
}

if [[ ! -f "renv.lock" ]]; then
    echo "reference/renv.lock missing — reference dependencies not pinned" >&2
    exit 1
fi

if [[ ! -d "renv/library" ]]; then
    echo "installing R dependencies via renv (first-run bootstrap)…" >&2
    Rscript -e 'if (!requireNamespace("renv", quietly = TRUE)) install.packages("renv", repos = "https://cloud.r-project.org"); renv::restore(prompt = FALSE)'
fi

Rscript reference.R

if [[ ! -s "out/bsflu_ensemble.tsv" ]]; then
    echo "reference.R did not produce out/bsflu_ensemble.tsv" >&2
    exit 1
fi

# No column rename needed — the R script writes `new_infections` directly,
# matching camdl's `flow_infection` column after rename below.
#
# camdl's --output trajectory writes `flow_infection`; this case's
# case.toml summary spec reads from the camdl side. Rename pomp's
# `new_infections` → `flow_infection` to let one [summary.stats] block
# drive both sides.
awk 'BEGIN { FS="\t"; OFS="\t" } NR==1 { for (i=1; i<=NF; i++) if ($i == "new_infections") $i = "flow_infection" } { print }' \
    out/bsflu_ensemble.tsv > out/bsflu_ensemble.renamed.tsv
mv out/bsflu_ensemble.renamed.tsv out/bsflu_ensemble.tsv

echo "reference regen complete: $(wc -l < out/bsflu_ensemble.tsv) rows → out/bsflu_ensemble.tsv"
