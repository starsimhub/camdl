# polio_es — EVSI of Environmental Surveillance for SIA Targeting

> **Status: not runnable on alpha.** This example depends on `camdl voi`
> (EVSI engine) and the legacy `experiment run / summarize` pipeline,
> both of which are gated for alpha. The model + sweep `.toml` files
> are preserved here so the example can be resurrected when voi ships.
> For sweep-only usage (no EVSI), run the model via
> `camdl batch run examples/polio_es/experiment.toml`.

50-patch SIRV model demonstrating EVSI analysis for poliovirus environmental
surveillance (ES). Answers: **how many ES sites should we fund before deciding
on SIA scope?**

## Model

- 50 independent patches with heterogeneous OPV coverage (35%–89%)
- Shared uncertain `beta` and `gamma`; per-patch
  `beta_eff[p] = beta * (1 - cov[p])`
- Three decisions: `no_sia`, `target_10` (10 lowest-coverage patches), `sia_all`
- SIA fires at day 60, transferring `vacc_frac` of S→V per targeted patch
- V compartment isolates SIA-vaccinated from natural infections so `final_R_p*`
  = cases only

## Pipeline (run from repo root)

```bash
# 1. Compile model
camdlc examples/polio_es/polio_es_50.camdl > examples/polio_es/polio_es_50.ir.json

# 2. Run experiment (2048 parameter points × 3 scenarios × 3 seeds ≈ 18K runs)
camdl-sim experiment run examples/polio_es/experiment.toml --parallel 8

# 3. Summarize trajectories → outputs.tsv
camdl-sim experiment summarize examples/polio_es/experiment.toml

# 4. Add derived columns to outputs.tsv and parameter_points.tsv
#    - p_positive: fraction of patches with beta*(1-cov_p)/gamma > 1 (ES likelihood parameter)
#    - total_cases: sum of final_R_p0..final_R_p49 (utility column for VOI)
#    NOTE: camdl voi run depends on this step. Run it before voi run.
uv run python3 examples/polio_es/postprocess.py

# 5. Compute EVSI
camdl-sim voi run examples/polio_es/voi.toml

# 6. Generate figures
camdl-analysis plot-evsi-curves  --voi-toml examples/polio_es/voi.toml \
    --save examples/polio_es/figures/evsi_curves.png
camdl-analysis plot-study-comparison --voi-toml examples/polio_es/voi.toml \
    --save examples/polio_es/figures/study_comparison.png
```

## Key result

EVSI saturates around **n=10 ES sites** — capturing ~95% of the value of full
information. Beyond 10 sites, the binomial signal is already concentrated enough
to resolve which transmission regime you're in.

## Known limitations (toy model)

- No spatial coupling between patches — see `projects/polio/spatial-models.md`
- ES likelihood uses `p_positive` (R0 > 1), not incidence-based detection
- Shared beta/gamma across all patches; real uncertainty is patch-local
- Costs are case-equivalents estimated from dose counts, not real budget data
