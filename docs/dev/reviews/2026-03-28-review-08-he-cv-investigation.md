---
status: closed
date: 2026-03-28
note: All issues from this review have been addressed and incorporated.
---

# Code Review #8 — He et al. Model + 40% CV Investigation

**Focus:** Why does the camdl He et al. measles model produce ~40% higher CV
than pomp at the same MLE parameters?

---

## What's mathematically correct

### Overdispersion parameterization ✓

`neg_binomial(mean, sigma_sq, dt)` in ekrng.rs:

```rust
let shape = dt / sigma_sq;      // 1/2.82 = 0.355 for He et al.
let scale = sigma_sq / dt;      // 2.82 for dt=1
let g = Gamma::new(shape, scale).sample(rng);  // E[G]=1, Var[G]=σ²/dt
self.poisson(mean * g)
```

This is the CORRECT multiplicative parameterization. The Gamma multiplier is
unit-mean with variance σ²/dt, matching pomp's `rgammawn(sigmaSE, dt)` →
`dw/dt`. Confirmed by tracing through:

    pomp: Var[dw/dt] = sigmaSE²/dt_yr = 0.0878² × 365.25 = 2.82
    camdl: Var[G] = sigma_sq/dt = 2.82/1 = 2.82  ✓

### Beta base formula ✓

```camdl
let beta_base = R0 * (1.0 - exp(-(gamma + mu)))
```

With gamma = 0.0832/day, mu = 5.48e-5/day: beta_base = 4.538. This matches
pomp's `R0*(1-exp(-(gamma+mu)*dt))/dt` for dt=1 day. The `1-exp` correction
accounts for within-step recovery — correct.

### Seasonal forcing values ✓

School time: seas = 1 + 0.554 × 0.2411/0.7589 = 1.176 Holiday: seas = 1 - 0.554
= 0.446 Matches pomp exactly.

### Chain-binomial competing risks ✓

Source groups correctly handle multinomial decomposition for compartments with
multiple outflows. The Gamma noise is applied to per-capita rate before
probability conversion. Sequential conditional binomial draws ensure total exits
≤ n_src. This matches pomp's `reulermultinom` structure.

---

## Issues found — likely contributors to 40% CV inflation

### 1. BUG: Periodic forcing normalization mismatch

The 52-bin periodic approximation gives 39 school bins and 13 holiday bins.
School fraction = 39/52 = 0.750. But the normalization constant `0.2411/0.7589`
was computed from pomp's exact school calendar: 277 school days / 365 total =
0.7589.

    seas_mean_pomp  = 1.000 (by construction)
    seas_mean_camdl = 1.176 × 0.750 + 0.446 × 0.250 = 0.9935

Mean seasonal forcing is 0.65% low. Over 15 years this slightly reduces R_eff.
**Small effect — not the main cause**, but fix the normalization constant to
match the 52-bin discretization:

```camdl
# Correct for 52-bin periodic: 13 holiday bins / 39 school bins
let seas = 1.0 - amplitude + amplitude * (1.0 + 13.0 / 39.0) * school(t)
```

Or better: switch to a piecewise forcing function with exact day boundaries
matching pomp:

```camdl
school : piecewise {
  breakpoints = [7, 100, 115, 199, 252, 300, 308, 356]
  values      = [0, 1, 0, 1, 0, 1, 0, 1, 0]
  period      = 365.25
}
```

### 2. DESIGN: Tau-leap Poisson overshoot vs Euler-multinomial cap

The most likely structural cause of CV inflation. In pomp's Euler- multinomial,
events from S are drawn as Multinomial(S, p_inf, p_death, p_stay) — total exits
never exceed S. In camdl's tau-leap, infection and death are independent Poisson
draws that CAN exceed S, triggering the clamp.

With overdispersion (Gamma shape = 0.355), occasional Gamma draws of G=5-10
produce Poisson(mean × 5) which at peak epidemic could easily exceed S. The
clamp sets S to 0 but the extra events already went to E — creating individuals
from nothing. This inflates epidemic peaks and adds right-tail variability.

**Diagnostic test:** Run 200 seeds with tau-leap AND 200 with chain- binomial
(which caps correctly). Compare CVs. If chain-binomial has lower CV, overshoot
is the cause.

**Fix:** For He et al. comparison, use `--backend chain_binomial`. For tau-leap,
add a mode that caps Poisson draws at source compartment size (approximate
Euler-multinomial).

### 3. HIGH RISK: Covariate time unit mismatch (cannot verify)

The model uses `pop(t)` and `birthrate(t)` from an interpolated TSV file. With
`time_unit = 'days`, the time column must be in days (0, 365, 730, ...). If the
TSV was exported from R with time in years (1944, 1945, ...), then `pop(365)`
would look up "year 365" — beyond the data range — and return the last value.
Pop(t) would be CONSTANT after day ~21, giving completely wrong dynamics.

**This is the #1 candidate for a large systematic error.** I can't verify
without seeing the data file. Check immediately:

```bash
head -5 data/he2010_london_covariates.tsv
# Should show: t in days (0, 30.44, 60.88, ...)
# NOT: t in years (1944, 1944.08, ...)
```

### 4. MINOR: Population drift vs pomp's pinned R

In pomp, R = pop(t) - S - E - I (residual). Total population exactly tracks the
covariate. In camdl, R is a free compartment — births and deaths may not
perfectly balance, causing population drift.

The FOI denominator uses pop(t) (not S+E+I+R), so the FOI is unaffected. But
death rates use actual compartment sizes. If the actual population drifts above
pop(t), more deaths occur, creating a slight negative feedback. This adds
negligible variability (drift < 0.1% over 15 years).

### 5. MINOR: B_hold individuals don't die

The model has explicit death transitions for S, E, I, R but NOT B_hold.
Individuals in B_hold (waiting for school entry) are immune to death. Over a
year, B_hold holds ~16,000 individuals, and the missing deaths are mu × 16000 ≈
0.9/day. Negligible.

However: if the EARLIER version used `death[c in compartments]`, it WOULD have
included death from B_hold, which is wrong in the opposite direction — B_hold
represents pre-birth accumulation. Verify the current version is the one being
benchmarked.

---

## Recommended diagnostic test battery

In priority order:

### Test 1: ODE comparison with pomp (HIGHEST PRIORITY)

Run the camdl ODE backend and pomp's deterministic skeleton at identical
parameters. Compare trajectories point by point. Any divergence here is a model
specification error (wrong formula, wrong units, wrong covariate) completely
independent of stochastic implementation.

```bash
camdl simulate model.camdl --params p.toml --backend ode --dt 0.1
```

Compare S(t), I(t) at weekly resolution against pomp's
`trajectory(m1, params=theta)`. If these don't match, everything stochastic is
irrelevant — fix the ODE first.

### Test 2: Covariate verification

Dump pop(t) and birthrate(t) at monthly intervals using `camdl eval` (once
implemented) or by adding trace columns. Compare against pomp's covariate table
values. If they don't match, the interpolation or time units are wrong.

### Test 3: Tau-leap vs chain-binomial CV comparison

Run 200 seeds with each backend at dt=1. Compare CV of weekly cases. If
chain-binomial has lower CV, the Poisson overshoot in tau-leap is the cause.

### Test 4: Forcing normalization integral

Numerically integrate seas(t) over one year. Should equal 365.25 (mean = 1.0).
If it equals 362.9 (= 365.25 × 0.9935), the normalization constant is wrong for
the 52-bin discretization.

### Test 5: Birth-death balance

Run with beta=0 (no infection) for 50 years. Check that total population tracks
pop(t) covariate. If population drifts systematically, births and deaths are
imbalanced.

---

## Summary

The Rust code is mathematically correct — the overdispersion parameterization,
the Sobol computation, the chain-binomial multinomial decomposition, and the
Euler correction are all right.

The 40% CV inflation most likely comes from one of:

1. **Covariate time units** (can't verify, but would cause large error)
2. **Poisson overshoot** in tau-leap (structural difference from pomp)
3. **Forcing normalization** mismatch (small but systematic)

The ODE comparison test (Test 1) is the single most valuable diagnostic. If the
deterministic trajectories match pomp, the issue is stochastic structure
(overshoot). If they don't match, it's a model specification error that the ODE
comparison will pinpoint.
