#!/usr/bin/env Rscript
# pomp pfilter log-likelihood at the He et al. (2010) London MLE.
#
# Runs the same model as ../he2010_forward/reference/reference.R but
# instead of simulating, evaluates `pfilter()` at fixed parameters and
# writes the log-lik per replicate. The harness compares camdl's
# particle-filter log-lik (via `camdl pfilter --replicates N`) against
# this ensemble.
#
# Output: out/pomp_pfilter_loglik.tsv with columns:
#   sim      — replicate index (1..N_REPS)
#   loglik   — pomp's pfilter log-lik estimate
#
# The harness summariser reads this as a long-format TSV with seed_col
# = "sim" and aggregates the per-replicate loglik via sum/mean/etc.
# Only one row per sim, so aggregate = mean gives back the scalar
# per-replicate log-lik.

library(pomp)
library(dplyr)

# ── Load data (He et al. London weekly cases) ────────────────────────────────

url <- "https://kingaa.github.io/pomp/vignettes/twentycities.rda"
tmp <- tempfile(fileext = ".rda")
download.file(url, tmp, quiet = TRUE)
load(tmp)

TOWN <- "London"
demog |> filter(town == TOWN) |> select(-town) -> dem
dem |> mutate(birthrate = births / pop) -> dem
delay <- 4
t_fine <- with(dem, seq(from = min(year), to = max(year), by = 1/12))

covar <- covariate_table(
  t        = t_fine,
  pop      = predict(smooth.spline(dem$year, dem$pop), x = t_fine)$y,
  birthrate= predict(smooth.spline(dem$year + delay, dem$birthrate), x = t_fine)$y,
  times    = "t",
  order    = "constant"
)

theta <- c(
  R0 = 56.8, mu = 0.02, delay = 4, sigma = 28.9, gamma = 30.4,
  rho = 0.488, amplitude = 0.554, alpha = 0.976, iota = 2.9,
  cohort = 0.557, psi = 0.116, sigmaSE = 0.0878,
  S_0 = 0.0297, E_0 = 5.17e-05, I_0 = 5.14e-05, R_0 = 0.97
)
paramnames <- names(theta)

measles |>
  filter(town == TOWN) |>
  mutate(year = as.numeric(format(date, "%Y")) + as.numeric(format(date, "%j")) / 365.25) |>
  select(year, cases) -> dat

m1 <- dat |>
  pomp(
    times = "year", t0 = with(dat, min(year) - 1/52),
    covar = covar, accumvars = c("C", "W"),
    rprocess = euler(
      step.fun = Csnippet("
        double beta, br, seas, foi, dw;
        double rate[6], trans[6];
        if (fabs(t - floor(t) - 251.0/365.0) < 0.5*dt)
          br = cohort*birthrate/dt + (1-cohort)*birthrate;
        else br = (1-cohort)*birthrate;
        double t_day = (t - floor(t)) * 365.25;
        if ((t_day>=7 && t_day<=100) || (t_day>=115 && t_day<=199) ||
            (t_day>=252 && t_day<=300) || (t_day>=308 && t_day<=356))
          seas = 1.0 + amplitude * 0.2411/0.7589;
        else seas = 1.0 - amplitude;
        beta = R0 * seas * (1.0 - exp(-(gamma+mu)*dt)) / dt;
        foi = beta * pow(I + iota, alpha) / pop;
        dw = rgammawn(sigmaSE, dt);
        rate[0] = foi * dw/dt; rate[1] = mu;
        rate[2] = sigma;       rate[3] = mu;
        rate[4] = gamma;       rate[5] = mu;
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
    rinit = Csnippet("
      double m = pop / (S_0 + E_0 + I_0 + R_0);
      S = nearbyint(m * S_0);
      E = nearbyint(m * E_0);
      I = nearbyint(m * I_0);
      R = nearbyint(m * R_0);
      W = 0; C = 0;
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
    statenames = c("S", "E", "I", "R", "C", "W"),
    paramnames = paramnames
  )

# ── pfilter replicates ───────────────────────────────────────────────────────

N_PARTICLES <- 2000
N_REPS      <- 20
SEED_BASE   <- 42L

message("Running ", N_REPS, " pfilter replicates × ", N_PARTICLES, " particles…")

set.seed(SEED_BASE)
logliks <- numeric(N_REPS)
for (i in seq_len(N_REPS)) {
  pf <- pfilter(m1, params = theta, Np = N_PARTICLES)
  logliks[i] <- logLik(pf)
  if (i %% 5 == 0) message("  rep ", i, "/", N_REPS, ": ll = ", round(logliks[i], 2))
}

# ── Write output ─────────────────────────────────────────────────────────────

out_dir <- "out"
dir.create(out_dir, showWarnings = FALSE)

data.frame(sim = seq_len(N_REPS), loglik = logliks) |>
  write.table(file.path(out_dir, "pomp_pfilter_loglik.tsv"),
              sep = "\t", row.names = FALSE, quote = FALSE)

message("  → out/pomp_pfilter_loglik.tsv (",
        N_REPS, " replicates; mean ll = ", round(mean(logliks), 2),
        ", sd = ", round(sd(logliks), 2), ")")
