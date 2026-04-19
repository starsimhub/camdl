
---
## 2026-04-18 — v2 `camdl fit run` skips random chain initialization

**Severity:** Critical. Every fit via `camdl fit run fit.toml` since the v2
dispatch landed has been running its scout stage with all chains starting
from the same initial point, not uniform-over-bounds as `run_scout` (v1)
does.

**Evidence.** On any fit (SIR, SIBR, Erlang) the new `chain_starts.tsv`
file shows every chain at the same values:

```
# scout/chain_starts.tsv for SIR fit (32 chains, seed=42)
chain   beta    gamma   k
1       4.9476  0.4784  97.9298
2       4.9476  0.4784  97.9298
...
32      4.9476  0.4784  97.9298
```

Different top-level seeds give different values (RNG advances with seed),
but within a run all chains are identical.

**Root cause.** `cmd_fit_run_v2` at
`rust/crates/cli/src/fit/mod.rs:636` dispatches IF2 stages through
`runner::run_chains_with_diagnostics(&run_config, &collector)` which
internally calls
`run_chains_with_per_chain_params(config, None, collector)`. The `None`
means every chain gets the same `config.estimated_params.initial`.

The random-chain initialization in `scout.rs:72-93` (builds a
`Vec<Vec<EstimatedParam>>` with `random_from_bounds` across n_chains) is
never reached from the v2 path. It was never ported.

**Impact.**
- Scout is not actually exploring the likelihood landscape — it's
  running N correlated chains from one starting point.
- Rhat across chains is meaningless: between-chain variance is 0 by
  construction at init; any spread at final iter is just IF2's own
  noise, not a genuine independence-of-starts signal.
- The "63 random + 1 seeded" eprintln message in `run_scout` does not
  fire in the v2 path, so nothing tells the user what's happening.

**Fix.** Port the per-chain init builder from `scout.rs:72-93` into the
v2 `Stage::IF2` dispatch in `mod.rs`. Gate on whether the stage has a
`starts_from` dependency (only the first/scout-role stage should
randomize; refine should use the v2 `chain_starts_override` from scout's
summary).

**Test to add.** `test_v2_scout_chains_uniform_over_bounds`:
  1. Run scout with 32 chains and no starts_from.
  2. Read chain_starts.tsv.
  3. Assert: for each estimated param, the 32 values span > 50% of the
     bounds range (i.e., max - min > 0.5 * (upper - lower)).

**Downstream state:** All fits in camdl-book's boarding-school chapter
(SIR, Erlang-2 SIR, SIBR, SEIR) have been affected. Rhat diagnostics in
those fits are not trustable. Will rerun once this is fixed.

---
## 2026-04-18 — IF2 fails on stratified SEIBCR-style models

**Severity:** High. Cannot fit Avilov-style SEIBCR models in camdl; the
reference comparison for the 1978 boarding-school chapter is blocked.

**Repro.** `boarding_school_avilov.camdl` (in scratch dir) defines a 4-way
stratified model: E (Erlang-3), I (Erlang-2), B (Erlang-2), C (Erlang-2).
The IF2 particle filter crashes mid-fit with NegativePropensity at t=0,
on specific chains that vary by seed. Simulation at identical parameters
succeeds. Removing the E-bypass branch does not fix it.

**Observed crashes**
```
chain N error: NegativePropensity { transition: "infect_E",
   value: -0.020855..., t: 0.0 }
chain N error: NegativePropensity { transition: "infection",
   value: -14913749.51..., t: 0.0 }  (yes, minus-14M, no bypass here)
```

Values range from tiny (1e-2) to absurd (1e7). Same seed crashes the
same chain; different seeds crash different chains.

**Concurrent symptom:** scout `best_loglik` values of −440 to −5333 on a
28-observation dataset (14×2 streams) when Poisson likelihood at
Avilov's known-good parameters should be ~ -100 range. Scout cannot
find the basin even though it exists.

**Hypothesis (not verified).** Particle-filter state or propensity
evaluation has a bug when:
(a) a compartment is stratified and used in force-of-infection across
    its substage sum (I_tot = I[i1] + I[i2]), or
(b) multiple transitions compete for the same source compartment with
    logit-transformed fractional parameters, or
(c) some interaction between 4-way stratification and IF2 perturbation.

**Requested:** diagnose the NegativePropensity root cause. The reference
model (Avilov 2024, Royal Soc Interface) achieves R²_B = 99.88% on this
data with published code in Stan. camdl should be able to fit a
comparable model.
