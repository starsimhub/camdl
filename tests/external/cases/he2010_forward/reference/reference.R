#!/usr/bin/env Rscript
# 01_pomp_reference.R — Run the exact He et al. (2010) measles model in pomp
# and write trajectory output as TSV for comparison with camdl.
#
# This reproduces the model from:
#   https://kingaa.github.io/pomp/vignettes/He2010.html

library(pomp)
library(dplyr)
library(tidyr)

dir.create("out", showWarnings = FALSE)

# ── Load data ────────────────────────────────────────────────────────────────

url <- "https://kingaa.github.io/pomp/vignettes/twentycities.rda"
tmp <- tempfile(fileext = ".rda")
download.file(url, tmp, quiet = TRUE)
load(tmp)  # measles, demog, coord, mles

TOWN <- "London"

# ── Build covariate table ────────────────────────────────────────────────────

demog |>
  filter(town == TOWN) |>
  select(-town) -> dem

# Interpolated covariates: population and birthrate with 4-year delay
with(dem, data.frame(
  year = seq(from = min(year), to = max(year), by = 1/12)
)) -> covar_times

dem |>
  mutate(
    birthrate = births / pop
  ) -> dem

delay <- 4  # school entry age — shift birthrate by 4 years

t_fine <- with(dem, seq(from = min(year), to = max(year), by = 1/12))

covar <- covariate_table(
  t = t_fine,
  pop = predict(smooth.spline(dem$year, dem$pop), x = t_fine)$y,
  # Lagged birthrate: at model-time Y, use birthrate from Y - delay
  birthrate = predict(smooth.spline(dem$year + delay, dem$birthrate), x = t_fine)$y,
  times = "t",
  order = "constant"
)


# ── He et al. MLE parameters ────────────────────────────────────────────────

# London MLE from He et al. (2010), extracted from pomp short-course
# materials (kingaa.github.io/short-course/measles/measles.Rmd, line 484).
theta <- c(
  R0        = 56.8,
  mu        = 0.02,       # yr^-1
  delay     = 4,          # years (school-entry delay, fixed)
  sigma     = 28.9,       # yr^-1
  gamma     = 30.4,       # yr^-1
  rho       = 0.488,
  amplitude = 0.554,
  alpha     = 0.976,
  iota      = 2.9,        # cases/yr
  cohort    = 0.557,
  psi       = 0.116,
  sigmaSE   = 0.0878,
  S_0       = 0.0297,
  E_0       = 5.17e-05,
  I_0       = 5.14e-05,
  R_0       = 0.97
)

paramnames <- names(theta)

message("He et al. MLE parameters for ", TOWN, ":")
print(theta)

# Write parameters to TSV for cross-reference
write.table(
  data.frame(parameter = names(theta), value = as.numeric(theta)),
  "out/pomp_params.tsv", sep = "\t", row.names = FALSE, quote = FALSE
)

# ── Build pomp object ────────────────────────────────────────────────────────

# Process model (C snippets from He et al.)
measles |>
  filter(town == TOWN) |>
  mutate(
    year = as.numeric(format(date, "%Y")) + as.numeric(format(date, "%j")) / 365.25
  ) |>
  select(year, cases) -> dat

m1 <- dat |>
  pomp(
    times = "year",
    t0 = with(dat, min(year) - 1/52),
    covar = covar,
    accumvars = c("C", "W"),
    rprocess = euler(
      step.fun = Csnippet("
        double beta, br, seas, foi, dw, births;
        double rate[6], trans[6];

        // cohort effect
        if (fabs(t - floor(t) - 251.0/365.0) < 0.5*dt)
          br = cohort*birthrate/dt + (1-cohort)*birthrate;
        else
          br = (1-cohort)*birthrate;

        // term-time seasonality
        double t_day = (t - floor(t)) * 365.25;
        if ((t_day>=7 && t_day<=100) ||
            (t_day>=115 && t_day<=199) ||
            (t_day>=252 && t_day<=300) ||
            (t_day>=308 && t_day<=356))
          seas = 1.0 + amplitude * 0.2411/0.7589;
        else
          seas = 1.0 - amplitude;

        // transmission rate
        beta = R0 * seas * (1.0 - exp(-(gamma+mu)*dt)) / dt;

        // force of infection
        foi = beta * pow(I + iota, alpha) / pop;

        // white noise (extrademographic stochasticity)
        dw = rgammawn(sigmaSE, dt);

        rate[0] = foi * dw/dt;   // infection
        rate[1] = mu;            // natural S death
        rate[2] = sigma;         // E to I
        rate[3] = mu;            // natural E death
        rate[4] = gamma;         // I to R
        rate[5] = mu;            // natural I death

        // transitions
        reulermultinom(2, nearbyint(S), &rate[0], dt, &trans[0]);
        reulermultinom(2, nearbyint(E), &rate[2], dt, &trans[2]);
        reulermultinom(2, nearbyint(I), &rate[4], dt, &trans[4]);

        S += nearbyint(pop*br*dt) - trans[0] - trans[1];
        E += trans[0] - trans[2] - trans[3];
        I += trans[2] - trans[4] - trans[5];
        R = nearbyint(pop) - S - E - I;
        W += (dw - dt)/sigmaSE;
        C += trans[4];
      "),
      delta.t = 1/365.25
    ),
    skeleton = vectorfield(
      Csnippet("
        double beta, br, seas, foi;

        // cohort effect
        if (fabs(t - floor(t) - 251.0/365.0) < 0.5*0.01)
          br = cohort*birthrate/0.01 + (1-cohort)*birthrate;
        else
          br = (1-cohort)*birthrate;

        // term-time seasonality
        double t_day = (t - floor(t)) * 365.25;
        if ((t_day>=7 && t_day<=100) ||
            (t_day>=115 && t_day<=199) ||
            (t_day>=252 && t_day<=300) ||
            (t_day>=308 && t_day<=356))
          seas = 1.0 + amplitude * 0.2411/0.7589;
        else
          seas = 1.0 - amplitude;

        beta = R0 * seas * (gamma+mu);
        foi = beta * pow(I + iota, alpha) / pop;

        DS = pop*br - foi*S - mu*S;
        DE = foi*S - (sigma+mu)*E;
        DI = sigma*E - (gamma+mu)*I;
        DR = gamma*I - mu*R;
        DC = gamma*I;
        DW = 0;
      ")
    ),
    rinit = Csnippet("
      double m = pop / (S_0 + E_0 + I_0 + R_0);
      S = nearbyint(m * S_0);
      E = nearbyint(m * E_0);
      I = nearbyint(m * I_0);
      R = nearbyint(m * R_0);
      W = 0;
      C = 0;
    "),
    dmeasure = Csnippet("
      double m = rho * C;
      double v = m * (1.0 - rho + psi*psi*m);
      double tol = 1.0e-18;
      if (cases > 0.0) {
        lik = pnorm(cases+0.5, m, sqrt(v)+tol, 1, 0) -
              pnorm(cases-0.5, m, sqrt(v)+tol, 1, 0) + tol;
      } else {
        lik = pnorm(0.5, m, sqrt(v)+tol, 1, 0) + tol;
      }
      if (give_log) lik = log(lik);
    "),
    rmeasure = Csnippet("
      double m = rho * C;
      double v = m * (1.0 - rho + psi*psi*m);
      double tol = 1.0e-18;
      cases = rnorm(m, sqrt(v) + tol);
      if (cases > 0.0) {
        cases = nearbyint(cases);
      } else {
        cases = 0.0;
      }
    "),
    statenames = c("S", "E", "I", "R", "C", "W"),
    paramnames = paramnames
  )

# ── Deterministic skeleton ───────────────────────────────────────────────────

message("Running deterministic skeleton...")
traj <- trajectory(m1, params = theta, format = "d")

t0 <- min(dat$year)

message("  trajectory columns: ", paste(names(traj), collapse = ", "))

# pomp 6.x trajectory(format="d") has columns: S,E,I,R,C,W,year,.id
traj |>
  select(year, S, E, I, R, C) |>
  mutate(
    year = as.numeric(as.character(year)),
    t_days = (year - t0) * 365.25
  ) -> traj_out

write.table(traj_out, "out/pomp_skeleton.tsv", sep = "\t",
            row.names = FALSE, quote = FALSE)
message("  → out/pomp_skeleton.tsv (", nrow(traj_out), " rows)")

# ── Stochastic simulations ───────────────────────────────────────────────────

message("Running stochastic simulation (seed 1)...")
set.seed(1L)
sim1 <- simulate(m1, params = theta, nsim = 1, format = "d")

message("  simulate columns: ", paste(names(sim1), collapse = ", "))

# pomp 6.x simulate(format="d") has: .id, year, S, E, I, R, C, W, cases
sim1 |>
  select(year, S, E, I, R, C, cases) |>
  mutate(
    year = as.numeric(as.character(year)),
    t_days = (year - t0) * 365.25
  ) -> sim1_out

write.table(sim1_out, "out/pomp_stochastic_seed1.tsv", sep = "\t",
            row.names = FALSE, quote = FALSE)
message("  → out/pomp_stochastic_seed1.tsv (", nrow(sim1_out), " rows)")

# ── Ensemble (200 sims) ─────────────────────────────────────────────────────

message("Running 200 stochastic simulations...")
set.seed(42L)
sims <- simulate(m1, params = theta, nsim = 200, format = "d",
                 include.data = FALSE)

sims |>
  mutate(
    year_num = as.numeric(as.character(year)),
    t_days = (year_num - t0) * 365.25
  ) |>
  select(sim = .id, year = year_num, t_days, S, E, I, R, C, cases) -> sims_out

write.table(sims_out, "out/pomp_ensemble.tsv", sep = "\t",
            row.names = FALSE, quote = FALSE)
message("  → out/pomp_ensemble.tsv (", nrow(sims_out), " rows)")

message("Done.")
