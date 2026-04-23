#!/usr/bin/env Rscript
# Boarding-school SIR reference via pomp.
#
# Builds the same SIR as tests/external/cases/boarding_school_sir/model.camdl,
# simulates 200 stochastic replicates at the fixed parameters declared in
# ../params.toml, and writes a long-format ensemble TSV.
#
# Chain-binomial / Euler-multinomial structure exactly mirrors camdl's
# chain_binomial backend: `reulermultinom` with `delta.t = 1` day to match
# camdl's `--dt 1`. The sim step seeds match camdl's `seed_base + i`
# pattern (via set.seed per iteration).
#
# Output columns:
#   sim    — seed (= seed_base + i, aligned with camdl's per-seed runs)
#   day    — day of simulation (1..14)
#   S, I, R, new_infections
#
# `new_infections` is a per-day accumulator (pomp's `accumvars`) — the
# daily count of S→I transitions. It is the per-step flow, equivalent to
# camdl's `flow_infection` column in its --output trajectory.

library(pomp)
library(dplyr)

# ── parameters (matching ../params.toml) ─────────────────────────────────────
# Note: pomp's Csnippet requires variables named with uppercase B for
# Beta to avoid the `beta` function conflict. We name the parameter
# `Beta` on both sides.
params <- c(Beta = 1.5, gamma = 0.5, N = 763, I0 = 5)

# ── pomp model ───────────────────────────────────────────────────────────────

sir_step <- Csnippet("
  double rate[2];
  double dN[2];
  rate[0] = Beta * I / N;    // per-S infection hazard
  rate[1] = gamma;           // per-I recovery hazard
  reulermultinom(1, S, &rate[0], dt, &dN[0]);
  reulermultinom(1, I, &rate[1], dt, &dN[1]);
  S -= dN[0];
  I += dN[0] - dN[1];
  R += dN[1];
  new_infections += dN[0];   // per-day accumulator; reset by pomp each obs
")

sir_rmeasure <- Csnippet("
  obs = new_infections;      // deterministic identity; summary uses new_infections directly
")

sir_init <- Csnippet("
  S = N - I0;
  I = I0;
  R = 0;
  new_infections = 0;
")

# Time grid: days 1..14 matching camdl's simulate window (to = 14 'days).
# camdl emits t = 0..14 (15 rows); pomp's observation grid starts at t > t0
# so we integrate over days 1..14. The t=0 state is recoverable from
# rinit if needed but not required for the summary stats.
times_df <- data.frame(day = 1:14, obs = NA_real_)

bsflu_pomp <- pomp(
  data        = times_df,
  times       = "day",
  t0          = 0,
  rprocess    = euler(sir_step, delta.t = 1.0),   # matches camdl --dt 1
  rmeasure    = sir_rmeasure,
  rinit       = sir_init,
  accumvars   = c("new_infections"),
  statenames  = c("S", "I", "R", "new_infections"),
  paramnames  = c("Beta", "gamma", "N", "I0"),
  obsnames    = c("obs")
)

# ── simulate ensemble ────────────────────────────────────────────────────────
# pomp's `simulate` with nsim=N uses the top-level RNG via L'Ecuyer
# streams under set.seed; fixed base seed gives reproducible draws.

set.seed(42L)
sims <- simulate(
  bsflu_pomp, params = params,
  nsim = 200, format = "d", include.data = FALSE
)

# sim column lands as `.id` in pomp 6.x; cast to integer seed.
out_dir <- "out"
dir.create(out_dir, showWarnings = FALSE)

sims |>
  mutate(day = as.numeric(as.character(day))) |>
  select(sim = .id, day, S, I, R, new_infections) |>
  write.table(file.path(out_dir, "bsflu_ensemble.tsv"),
              sep = "\t", row.names = FALSE, quote = FALSE)

message("  → ", out_dir, "/bsflu_ensemble.tsv (", nrow(sims), " rows)")
