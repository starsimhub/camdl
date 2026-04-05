# Proposal: Correlated Pseudo-Marginal MCMC

**Status:** Proposal
**Date:** 2026-04-05
**Implemented:** _(pending)_
**Motivation:** Vanilla PMMH is impractical on long time series (T>500)
because PF variance scales with T. On He et al. (T=1096), sd(log L̂) =
30-170 at 2000 particles, giving 1-2% acceptance rates. CPM correlates
the PF random numbers between current and proposed evaluations, reducing
the variance of the likelihood RATIO to O(1) regardless of T.

## Design

### Pre-drawn random state

All PF randomness is expressed as a vector of standard normals `u`:

```rust
struct PFRandomState {
    /// Gamma multiplier draws: [n_particles × n_obs_intervals]
    /// Stored as standard normals. Transformed to Gamma(shape, scale)
    /// via inverse CDF at evaluation time.
    gamma_noise: Vec<Vec<f64>>,

    /// Resampling draws: [n_obs_intervals]
    /// Stored as standard normals. Transformed to Uniform(0,1) via
    /// Phi(·) for systematic resampling.
    resample_noise: Vec<f64>,
}
```

The process noise (binomial draws in `reulermultinom`) is NOT stored
and correlated. These are discrete draws that resist smooth correlation.
The Gamma multiplier is the dominant continuous noise source and the
primary target for correlation.

### Crank-Nicolson update

At each MCMC step:

```rust
fn correlate(u: &PFRandomState, rho: f64, rng: &mut StatefulRng) -> PFRandomState {
    // u' = ρu + √(1-ρ²)z, z ~ N(0,1)
    let scale = (1.0 - rho * rho).sqrt();
    PFRandomState {
        gamma_noise: u.gamma_noise.iter().map(|row|
            row.iter().map(|&x| rho * x + scale * rng.normal()).collect()
        ).collect(),
        resample_noise: u.resample_noise.iter()
            .map(|&x| rho * x + scale * rng.normal()).collect(),
    }
}
```

### Modified PF interface

New function alongside `bootstrap_filter`:

```rust
pub fn bootstrap_filter_correlated(
    model: &CompiledModel,
    params: &[f64],
    observations: &[Observation],
    n_particles: usize,
    dt: f64,
    step_fn: &StepFn,
    project_fn: &dyn Fn(&ParticleState) -> f64,
    dmeasure_fn: &DmeasureFn,
    randoms: &PFRandomState,  // pre-drawn, correlated
) -> Result<PFilterResult, SimError>
```

The existing `bootstrap_filter` (seed-based) stays unchanged. All
non-PMMH code paths are unaffected.

### Where randoms are consumed

Per observation interval, the PF does:
1. **Propagate**: call `step_fn` 7× (daily steps). Each step draws
   a Gamma multiplier for the overdispersed infection transition.
   → Use `randoms.gamma_noise[obs_idx][particle_idx]` transformed
   via inverse Gamma CDF.

2. **Resample**: systematic resampling with one uniform.
   → Use `Phi(randoms.resample_noise[obs_idx])` as the base uniform.

The binomial draws in `reulermultinom` (infection count, death count)
use a per-particle RNG seeded deterministically from the stored
gamma noise. This gives partial correlation — same Gamma multiplier
means similar rates, which means similar binomial probabilities,
which means correlated (but not identical) counts.

### Sorted resampling

Before systematic resampling, sort particles by a 1D projection of
their state (e.g., I compartment value). This ensures that correlated
resampling uniforms select similar particles between the current and
proposed PF runs.

```rust
fn sorted_systematic_resample(
    log_weights: &[f64],
    states: &[ParticleState],
    project_fn: &dyn Fn(&ParticleState) -> f64,
    base_uniform: f64,  // from correlated randoms
) -> Vec<usize>
```

### PMMH integration

```rust
// In the PMMH loop:
let mut current_randoms = PFRandomState::draw_fresh(n_particles, n_obs, rng);
let mut current_ll = pf_correlated(current_params, &current_randoms);

for step in 0..n_steps {
    let proposed_params = propose(current_params, rng);
    let proposed_randoms = correlate(&current_randoms, rho, rng);
    let proposed_ll = pf_correlated(proposed_params, &proposed_randoms);

    let log_alpha = proposed_ll - current_ll
                  + log_prior(proposed) - log_prior(current)
                  + log_jacobian(proposed) - log_jacobian(current);

    if rng.uniform().ln() < log_alpha {
        current_params = proposed_params;
        current_randoms = proposed_randoms;
        current_ll = proposed_ll;
    }
}
```

No changes to the MH ratio formula. CPM preserves detailed balance
because the Crank-Nicolson proposal on u is reversible with respect
to N(0,I).

## Memory

For N=2000 particles, T=1096 observations:
- Gamma noise: 2000 × 1096 × 8 bytes = 17.5 MB
- Resampling: 1096 × 8 bytes = 8.8 KB
- Total: ~18 MB per chain. 4 chains = 72 MB. Fine.

## Configuration

```toml
[pmmh]
rho = 0.99          # Crank-Nicolson correlation (default 0.99)
correlated = true   # enable CPM (default true when running pmmh)
```

## Implementation plan

1. Add `PFRandomState` struct and `draw_fresh` / `correlate` methods
2. Add `bootstrap_filter_correlated` that consumes pre-drawn randoms
3. Add `sorted_systematic_resample` to resampling.rs
4. Modify Gamma multiplier in chain_binomial to accept pre-drawn value
5. Wire into PMMH loop (store randoms, Crank-Nicolson, accept/reject)
6. Add `rho` to PMMHSampleConfig
7. Test on pure-death model (analytical posterior)
8. Test on He et al. full model

## What stays unchanged

- `bootstrap_filter` (seed-based) — untouched
- IF2 — untouched (IF2 doesn't need correlated PF)
- All CLI commands except `camdl fit pmmh`
- MH ratio, adaptive proposals, prior handling
- Output format (traces, summary, fit_state)
