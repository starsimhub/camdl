#!/usr/bin/env bash
# Invoked by `external-harness regen` (or `run --regen`). Produces the
# pomp ensemble TSV the harness reads to compute the reference summary.
#
# Runs inside the case directory; the R script writes its outputs to
# `reference/out/` which is named in case.toml as the ensemble_tsv path.

set -euo pipefail

# Ensure we're anchored relative to this script's location, not the
# caller's cwd.
cd "$(dirname "$0")"
mkdir -p out

# If running in docker mode, delegate to the Dockerfile path.
if [[ "${CAMDL_EXTERNAL_USE_DOCKER:-}" == "1" ]]; then
    echo "docker regen not yet wired for this case; run locally" >&2
    exit 1
fi

# Require R; renv snapshot will restore the pinned packages on first run.
command -v Rscript >/dev/null || {
    echo "Rscript not found on PATH. Install R (e.g. 'brew install r') or rerun with CAMDL_EXTERNAL_USE_DOCKER=1." >&2
    exit 1
}

# renv.lock is committed; renv::restore() is idempotent.
if [[ ! -f "renv.lock" ]]; then
    echo "reference/renv.lock missing — reference dependencies not pinned" >&2
    exit 1
fi

# First-run bootstrap: ensure renv library is present.
if [[ ! -d "renv/library" ]]; then
    echo "installing R dependencies via renv (first-run bootstrap)…" >&2
    Rscript -e 'if (!requireNamespace("renv", quietly = TRUE)) install.packages("renv", repos = "https://cloud.r-project.org"); renv::restore(prompt = FALSE)'
fi

# Run the reference.
Rscript reference.R

# Sanity: the harness expects out/pomp_ensemble.tsv to exist.
if [[ ! -s "out/pomp_ensemble.tsv" ]]; then
    echo "reference.R did not produce out/pomp_ensemble.tsv" >&2
    exit 1
fi

# Rename pomp's `cases` column to `weekly_cases` so the single
# [summary] stats block in case.toml applies to both camdl and
# reference output without a column-alias layer. `sim` is kept
# as-is; case.toml declares seed_col = "sim".
awk 'BEGIN { FS="\t"; OFS="\t" } NR==1 { for (i=1; i<=NF; i++) if ($i == "cases") $i = "weekly_cases" } { print }' \
    out/pomp_ensemble.tsv > out/pomp_ensemble.renamed.tsv
mv out/pomp_ensemble.renamed.tsv out/pomp_ensemble.tsv

echo "reference regen complete: $(wc -l < out/pomp_ensemble.tsv) rows → out/pomp_ensemble.tsv"
