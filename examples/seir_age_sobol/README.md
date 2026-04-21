# Outbreak Triage: Which Uncertainty to Resolve First?

> **Status: not runnable on alpha.** This example depends on `camdl voi`
> (EVSI) and the legacy `experiment run / analyze` pipeline, both of
> which are gated for alpha. The model + sweep `.toml` files are
> preserved for resurrection post-voi. Sobol sample *generation* still
> works via `camdl batch run examples/seir_age_sobol/experiment.toml`;
> Sobol *index computation* (the analyze step) is out-of-tree.

A novel respiratory pathogen is detected in a town of ~10,000 residents. Two age
groups are at risk: children (school-age) and adults (working-age). Before
recommending targeted interventions, the team wants to know:

> **Given uncertainty in transmission (beta), latent period (sigma), and
> recovery rate (gamma), which parameter most drives peak infections by age? And
> which study — seroprevalence (narrows beta) or household shedding (narrows
> gamma) — would most reduce model output uncertainty?**

## Model

`seir_age` — a 2-age-group SEIR model with contact matrix [[12,4],[4,8]].
Parameters: beta (transmission coefficient), sigma (rate of progression from
exposed to infectious), gamma (recovery rate).

## Design

Three belief-state designs are compared:

| Design         | Scenario                      | Parameters                                                       |
| -------------- | ----------------------------- | ---------------------------------------------------------------- |
| `current`      | No studies done               | beta∈[0.05,0.5], sigma∈[0.1,1.0], gamma∈[0.05,0.5] (log-uniform) |
| `narrow_beta`  | Seroprevalence study done     | beta narrowed to [0.2,0.4]                                       |
| `narrow_gamma` | Household shedding study done | gamma narrowed to [0.1,0.3]                                      |

All ranges are log-uniform. Saltelli sampling (n=256) produces 2,048 parameter
points per design × 3 seeds = 6,144 runs per design, 18,432 runs total.

## Running

From the repository root:

```bash
# Build the simulator
make build-rust

# Run all simulations (requires ~2 min with --parallel 8)
camdl-sim experiment run examples/seir_age_sobol/experiment.toml --parallel 8

# Compute Sobol indices (fast — pure Rust)
camdl-sim experiment analyze examples/seir_age_sobol/experiment.toml

# Generate figures (requires: pip install -e python/)
camdl-analysis plot-sensitivity examples/seir_age_sobol/experiment.toml \
    --output peak_I_child \
    --save examples/seir_age_sobol/figures/sensitivity_child.png

camdl-analysis plot-sensitivity examples/seir_age_sobol/experiment.toml \
    --output peak_I_adult \
    --save examples/seir_age_sobol/figures/sensitivity_adult.png

camdl-analysis plot-voi examples/seir_age_sobol/experiment.toml \
    --output peak_I_child \
    --save examples/seir_age_sobol/figures/voi_child.png

camdl-analysis plot-scatter examples/seir_age_sobol/experiment.toml \
    --design current \
    --save examples/seir_age_sobol/figures/scatter_current.png

camdl-analysis plot-convergence examples/seir_age_sobol/experiment.toml \
    --design current \
    --save examples/seir_age_sobol/figures/convergence_current.png
```

## Expected Findings

Under `current` (wide priors), **beta dominates** peak_I_child: the scale of the
epidemic is almost entirely driven by transmission intensity. sigma and gamma
contribute, but at lower first-order indices.

After the seroprevalence study (`narrow_beta`), beta's S1 collapses because its
range is now tight. sigma and gamma become relatively more important — the
remaining uncertainty is in epidemic timing and duration, not scale.

After the household shedding study (`narrow_gamma`), beta and sigma still
dominate. The VOI waterfall shows that the seroprevalence study removes more
output variance than the household study — making it the higher-priority
investigation to commission first.

## Output Directory Structure

```
output/
  designs/
    current/
      parameter_points.tsv     ← committed (2048 rows)
    narrow_beta/
      parameter_points.tsv     ← committed
    narrow_gamma/
      parameter_points.tsv     ← committed
  analysis/sensitivity/
    current/
      sobol_indices.tsv        ← committed
      convergence.tsv          ← committed
      assumptions.txt          ← committed
      outputs.tsv              ← committed
    narrow_beta/               ← committed
    narrow_gamma/              ← committed
  runs/                        ← NOT committed (too large; ~18k traj.tsv files)
```

## Interpreting the Figures

- **sensitivity_*.png**: S1 (dark bar) = direct effect of that parameter. ST
  (light bar) = total effect including interactions. ST ≫ S1 indicates strong
  interactions with other parameters.

- **voi_*.png**: Horizontal bars show how much each parameter's total-order
  index decreases after a study. Larger bar = that parameter became less
  uncertain = the study was more informative.

- **scatter_current.png**: Lower-triangle scatter plots reveal the shape of the
  parameter-output relationship (monotone vs. non-monotone). Upper-triangle
  Spearman r confirms that beta vs. peak_I_child is strongly monotone.

- **convergence_current.png**: S1/ST estimates at n/8, n/4, n/2, n. Flat lines
  at n=256 confirm the indices have converged; instability at small n shows why
  n≥128 is needed for log-uniform sampling.
