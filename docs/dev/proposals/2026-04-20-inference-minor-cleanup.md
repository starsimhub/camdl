---
status: implemented
date: 2026-04-20
implemented: 2026-04-20
---

# Inference minor cleanup

Companion to `docs/dev/reviews/2026-04-20-review-rust-design.md`
(RdM4, Rdm1–4, Rdn3). Five independent, low-risk changes that improve
readability, eliminate magic constants, and reduce DRY violations in
the inference layer. Any can be done in isolation.

---

## R1. Decompose `log_transition_density_substep` into named stages

**Problem (RdM4):** `pgas.rs:226–387` is a 162-line function that
computes the complete-data log-transition density for one substep of
the CSMC-AS backward pass. It is the most scientifically sensitive
function in the codebase. Its logical structure has three stages
(overdispersion gammas, exit density, split density) but these stages
are not named sub-functions — they are interlocked via a shared
`gamma_idx` counter (line 265) that must advance in exactly the same
order as `step_one` in `chain_binomial.rs`. The ordering invariant is
documented only in a comment, not in the type system.

**Proposed decomposition** (all within `pgas.rs` or a new private
submodule):

```rust
pub fn log_transition_density_substep(
    model: &CompiledModel,
    counts_before: &[i64],
    flows: &[u64],
    gammas: &[f64],
    params: &[f64],
    t: f64,
    dt: f64,
) -> Result<f64, SimError> {
    // --- setup ---
    let propensities = eval_propensities(model, params, t, dt);
    let mut handled  = vec![false; model.model.transitions.len()];
    let mut log_p    = 0.0;
    let mut gamma_idx = 0;

    for &(src_local, ref group) in &model.source_groups {
        let (total_exits, total_rate, exit_probs) =
            collect_group_probs(model, group, &propensities, counts_before, src_local);

        log_p += overdispersion_log_density(
            model, group, gammas, &mut gamma_idx, total_rate, dt,
        )?;
        log_p += exit_log_density(flows, group, total_exits, total_rate, gammas, gamma_idx)?;
        log_p += split_log_density(flows, group, &exit_probs)?;

        for &tr in group { handled[tr] = true; }
    }

    log_p += remaining_transitions_log_density(model, &handled, flows, &propensities)?;
    Ok(log_p)
}
```

Each extracted function:

```rust
/// Compute total exits, total rate, and per-destination probabilities
/// for one source group. Returns (total_exits, total_rate, exit_probs).
fn collect_group_probs(
    model: &CompiledModel,
    group: &[usize],
    propensities: &[f64],
    counts_before: &[i64],
    src_local: usize,
) -> (u64, f64, Vec<(usize, f64)>) { ... }

/// Log-density of overdispersion gammas for transitions in this group.
/// Advances gamma_idx in the same order as chain_binomial::step_one.
fn overdispersion_log_density(
    model: &CompiledModel,
    group: &[usize],
    gammas: &[f64],
    gamma_idx: &mut usize,
    total_rate: f64,
    dt: f64,
) -> Result<f64, SimError> { ... }

/// Log-density of total exits from source: NegBinomial (overdispersed)
/// or Binomial (standard).
fn exit_log_density(
    flows: &[u64],
    group: &[usize],
    total_exits: u64,
    total_rate: f64,
    gammas: &[f64],
    gamma_idx: usize,
) -> Result<f64, SimError> { ... }

/// Log-density of the multinomial split of exits across destinations.
fn split_log_density(
    flows: &[u64],
    group: &[usize],
    exit_probs: &[(usize, f64)],
) -> Result<f64, SimError> { ... }

/// Log-density of transitions outside source groups (deterministic
/// and zero-rate transitions contribute 0).
fn remaining_transitions_log_density(
    model: &CompiledModel,
    handled: &[bool],
    flows: &[u64],
    propensities: &[f64],
) -> Result<f64, SimError> { ... }
```

The `gamma_idx` ordering invariant becomes visible in the function
signature: `overdispersion_log_density` takes `&mut gamma_idx` and
must advance it before `exit_log_density` reads it. This is still not
fully type-checked, but it concentrates the ordering concern in one
function rather than spreading it across 160 lines.

**Scope:** Mechanical extraction within `pgas.rs`. No change to the
mathematical content or the external signature of
`log_transition_density_substep`. The existing `pgas_resume.rs` and
`pgas_tempering.rs` integration tests provide regression coverage.

---

## R2. Define `LOG_PROB_FLOOR` as a named constant

**Problem (Rdm2):** The literal `1e-300` appears at eight or more
sites across four files as a floor for log-probability arguments to
avoid `−∞` log-weights. The value is not explained at any site.

Confirmed occurrences:
- `if2.rs:68,71,125,200` — barycentric transform, log-transform clamp,
  perturbation SD floor.
- `pgas.rs:505` — Gamma log-density floor.
- `obs_model.rs:144,230,231,280` — Bernoulli, BetaBinomial log-density.
- `correlated_pf.rs:132` — binomial quantile underflow guard.

**Proposed addition to `inference/types.rs`:**

```rust
/// Minimum argument for ln() in log-weight computations.
///
/// Chosen so that ln(LOG_PROB_FLOOR) ≈ −690, well above the
/// underflow threshold for any realistic particle count: even at
/// N=10_000 particles, a weight of 1e-300 contributes less than
/// −690 to log_sum_exp, which rounds to −∞ for the particle but
/// does not corrupt the normaliser.
///
/// Do NOT reduce below f64::MIN_POSITIVE (5e-324), which would
/// produce −∞ and defeat the purpose.
pub const LOG_PROB_FLOOR: f64 = 1e-300;
```

Replace all eight occurrences. Add a test:

```rust
#[test]
fn log_prob_floor_is_finite() {
    assert!(LOG_PROB_FLOOR.ln().is_finite());
    assert!(LOG_PROB_FLOOR.ln() < -600.0);
    assert!(LOG_PROB_FLOOR > f64::MIN_POSITIVE);
}
```

**Scope:** Search-and-replace of `1e-300` in the four files listed
above after verifying each is the same semantic use (log-probability
floor). The `correlated_pf.rs:132` use is a slightly different context
(early-exit from a loop) — verify separately before replacing.

---

## R3. Extract `init_particle_rngs` helper

**Problem (Rdm3):** Per-particle RNG initialisation appears in three
files with a subtle variant in IF2:

`particle_filter.rs:87–89`:
```rust
let rngs: Vec<StatefulRng> = (0..n)
    .map(|i| StatefulRng::new_stream(seed, i as u64))
    .collect();
```

`if2.rs:417–420` (with per-iteration stream offset):
```rust
let rngs: Vec<StatefulRng> = (0..n)
    .map(|i| StatefulRng::new_stream(seed, stream_base | (i as u64)))
    .collect();
```

`pgas.rs:657–659`:
```rust
let rngs: Vec<StatefulRng> = (0..n_particles)
    .map(|i| StatefulRng::new_stream(seed, i as u64))
    .collect();
```

A future algorithm author copying from PF or PGAS would silently omit
the IF2 stream-offset pattern without knowing it exists. The variation
is intentional and correct but opaque.

**Proposed addition to `inference/types.rs`:**

```rust
/// Allocate `n` per-particle RNG streams from `seed`.
///
/// `stream_offset` separates particles from different callers or
/// iterations: PF and PGAS pass `0` (particles differentiated by
/// index alone); IF2 passes `(iter as u64) << 32` so each
/// iteration's particle streams are disjoint from all others.
pub fn init_particle_rngs(
    seed: u64,
    n: usize,
    stream_offset: u64,
) -> Vec<StatefulRng> {
    (0..n)
        .map(|i| StatefulRng::new_stream(seed, stream_offset | (i as u64)))
        .collect()
}
```

Update call sites:
- `particle_filter.rs`: `init_particle_rngs(seed, n, 0)`
- `pgas.rs`: `init_particle_rngs(seed, n_particles, 0)`
- `if2.rs`: `init_particle_rngs(seed, n, stream_base)`

**Scope:** Three call sites + one new function. No semantic change.

---

## R4. Fix `resample_rng` seeding to match the `new_stream` pattern

**Problem (Rdn3):** After the IM1 fix to per-particle seeding, the
resample RNG in both PF and PGAS still uses the old XOR-constant pattern:

`particle_filter.rs:123`:
```rust
let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));
```

`pgas.rs:717`:
```rust
let mut resample_rng = StatefulRng::new(seed.wrapping_add(0xdeadbeef));
```

The constant `0xdeadbeef` is unexplained. The `new_stream` API exists
precisely to avoid ad-hoc seed mixing; the convention should be consistent
throughout the inference layer.

**Proposed change:** Reserve a high stream index for the resample RNG —
high enough to never collide with particle indices (which start from 0
and go up to at most `n_particles`, typically ≤ 10_000):

```rust
// In inference/types.rs:
/// Reserved stream index for the resampling RNG.
/// High enough to never collide with particle stream indices
/// (particles use [0, n_particles)).
pub const RESAMPLE_RNG_STREAM: u64 = 1u64 << 48;
```

```rust
// particle_filter.rs and pgas.rs:
let mut resample_rng = StatefulRng::new_stream(seed, RESAMPLE_RNG_STREAM);
```

**Scope:** Two call sites. The `0xdeadbeef` magic constant disappears.
Self-documenting: `RESAMPLE_RNG_STREAM` makes clear this is a
reservation, not a random choice.

**Test:** After the change, reproduce existing RNG-determinism tests
(`gillespie_determinism.rs`, `particle_filter.rs` in sim/tests) to
confirm the resample stream change does not break bit-for-bit
reproducibility. The test output will change (different resampling RNG
sequence), so the expected values in determinism tests need to be
updated; that is expected and correct.

---

## R5. Remove `n_obs` and `steps_per_obs` from `PMMHConfig`

**Problem (Rdm4):** `pmmh.rs:49–52`:
```rust
/// Number of observations (for sizing PFRandomState).
pub n_obs: usize,
/// Substeps per observation interval (= obs_spacing / dt).
pub steps_per_obs: usize,
```

These are computed by the CLI and passed into `PMMHConfig`, but they
are internal sizing details derivable from the observation model and
`dt`. PGAS does not carry equivalent fields — it derives them at
algorithm entry from `obs_model.n_observations()` and the observation
times. The asymmetry is unintentional and adds an unnecessary coupling
between the CLI and PMMH internals.

**Proposed change:** Remove the two fields from `PMMHConfig`. Compute
them in `pmmh::run_pmmh` at the same point PGAS computes them:

```rust
// In run_pmmh, near the top:
let n_obs = obs_model.n_observations();
let steps_per_obs = obs_model.steps_per_obs(config.dt)?;
```

where `steps_per_obs` is a helper already needed elsewhere:

```rust
// In obs_model or multi_stream_obs:
pub fn steps_per_obs(obs_model: &MultiStreamObsModel, dt: f64) -> usize {
    // round((t_next_obs - t_prev_obs) / dt) for the first pair;
    // validate uniformity if strict_obs_spacing is required.
    ...
}
```

Update the CLI launch site in `cli/src/fit/pmmh.rs` to drop the two
fields from the config builder.

**Scope:** `pmmh.rs` (remove fields, compute inline), CLI `fit/pmmh.rs`
(remove two builder lines). Structural change only; PMMH behavior is
unchanged.

---

## Ordering

R2 is the most mechanical (constant rename) and can be merged first.
R3 and R4 are independent; R3 should be done before any new algorithm
is added. R1 (decomposition) is the largest change and benefits from
the codebase being otherwise tidy first. R5 is independent.

Suggested order: R2 → R4 → R3 → R5 → R1.
