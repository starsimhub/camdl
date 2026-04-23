# SIR final-size: closed-form reference

## Model

Homogeneously-mixed SIR with demographically closed population `N` and
constant rates `beta`, `gamma`:

```
dS/dt = -beta * S * I / N
dI/dt =  beta * S * I / N - gamma * I
dR/dt =  gamma * I
```

`R0 = beta / gamma = 3.0` for this case.

## Final-size equation

Kermack & McKendrick (1927); reviewed in Diekmann, Heesterbeek &
Britton (2013, *Mathematical Tools for Understanding Infectious
Disease Dynamics*, §7.2). Let `s_∞ = S(∞)/N` and `r_∞ = R(∞)/N`. With
initial conditions `S(0) ≈ N`, `I(0) ≪ N`, in the deterministic
mean-field limit:

```
s_∞ = exp(-R0 · (1 - s_∞))
```

Equivalently with `r_∞ = 1 - s_∞`:

```
1 - r_∞ = exp(-R0 · r_∞)
```

## Numerical solution at R0 = 3

Fixed-point iteration converges to

```
r_∞ ≈ 0.9405
s_∞ ≈ 0.0595
```

Source: hand iteration, verified against Mathematica
`FindRoot[1 - r == Exp[-3 r], {r, 0.9}]` → `r = 0.940478`.

## Stochastic SIR — what the harness actually compares

The camdl simulator runs a *stochastic* chain-binomial SIR, not the
deterministic ODE. For large-enough N and well-seeded initial I, the
ensemble mean of the final attack rate (cumulative I→R recoveries / N0)
converges to `r_∞` above. The variance shrinks as O(1/N). At `N0 = 10,000`
and `I0 = 10`:

- Extinction probability (epidemic fails to take off) ≈ `(1/R0)^I0 =
  (1/3)^10 ≈ 1.7e-5` — negligible for a 200-seed ensemble.
- Stochastic spread around the mean attack rate: approximated by a
  CLT argument, SD / mean ≈ 1 / sqrt(R0 · (R0-1) · N · r_∞) ≈ 0.004,
  so final-size SD ≈ `N · 0.004 ≈ 40` individuals.

## Reference summary statistics

For the harness fixture (`fixtures/summary.tsv`), with `N0 = 10,000`
and the assumptions above:

- `final_R`  (total cumulative recoveries across the full simulation;
  summed over the per-day `recoveries` observation column at the
  `recovery` transition) — mean ≈ 9,405, SD ≈ 40. The initial 10
  infected contribute their own recoveries; they're included.

A peak-prevalence analytical benchmark would also be useful but is
harder to compute tightly; leaving it for a follow-up case.

## How to regenerate the fixture

This case has no external tool. If the derivation above changes (a
correction, a new R0, a richer parameterisation), the `reference_sha`
in `fixtures/MANIFEST.toml` will no longer match the hash of this
file, and the harness will mark the fixture stale.

To regenerate: manually recompute the expected summary statistics
from the updated derivation, edit `fixtures/summary.tsv` with the new
values, recompute the `case_sha` / `fixture_sha` / `reference_sha`,
and update `MANIFEST.toml`. A `external-harness regen` command that
does this automatically for analytical cases is a follow-up.
