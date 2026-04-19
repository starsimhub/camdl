# camdl Inference Trait Refactor: Design Document

**Status:** Ready to implement (after diagnostic system)\
**Priority:** Do now — unreleased code, no back-compat constraints\
**Estimated effort:** ~500 LOC new traits/impls, ~400 LOC deleted, net −100 LOC\
**Author:** Vince Buffalo + Claude\
**Date:** 2026-04-11

## Problem

The inference algorithms communicate through ad-hoc closures with increasingly
unwieldy signatures. Each algorithm independently reconstructs the same plumbing
from `CompiledModel` + params + observation data.

Evidence of the problem:

1. **`bootstrap_filter` takes 12 arguments**, three of which are `Option`
   because features were bolted on. `project_fn` and `obs_loglik_fn` are dead
   code when `joint_obs_fn` is `Some`.

2. **Closure construction is copy-pasted 6 times.** The pattern "build
   `step_fn`, `project_fn`, `obs_loglik_fn` from `FitRunConfig`" appears in:
   `run_quick_pfilter`, `run_one_chain`, `run_pfilter_at_mle`,
   `run_pfilter_with_obs`, the PGAS CLI, and the PMMH CLI. Each is ~15 lines of
   boilerplate.

3. **No shared vocabulary.** IF2 uses
   `step_fn: &dyn Fn(&mut ParticleState, &[f64], f64, f64, &mut StatefulRng, &mut StepScratch)`.
   The bootstrap PF uses
   `StepFn = dyn Fn(&mut ParticleState, f64, f64, &mut StatefulRng, &mut StepScratch)`
   (note: no `params`). PGAS calls `step_one` directly. Three calling
   conventions for the same operation.

4. **Adding a new algorithm requires understanding the closure plumbing.** If
   you wanted to add SMC² or IBPF, you'd need to reverse-engineer the closure
   types from the existing algorithms rather than implementing a trait.

## Design

### Leaky Abstraction Audit

Before writing traits, here are the leaks in the original draft that have been
fixed in this version:

1. **`reset_accumulators` was on `ObservationModel`.** Resetting flow counters
   is a _state lifecycle_ concern, not an observation concern. The obs model
   shouldn't know the state has flow accumulators. Fixed: moved to a
   `Resettable` bound on the State associated type.

2. **`sample`/`mean` returned first-stream-only.** Multi-stream prediction
   diagnostics need per-stream results. Fixed: `sample`/`mean` return `Vec<f64>`
   (one per stream), and a `n_streams()` method exposes how many streams exist.

3. **`SMCConfig` bundled `seed`.** Seed is a per-chain concern. Fixed: seed is a
   parameter to the algorithm, not part of the config.

4. **IF2 needed a separate `IF2ObservationModel` sub-trait.** This was because
   the PF baked params into the closure but IF2 needed to pass per-particle
   params. Fixed: `ObservationModel::log_likelihood` always takes `params`. This
   isn't a leak — the observation likelihood IS a function of params in general.
   The PF holds params fixed, but that's the PF's concern, not the trait's.

### Core Traits

```
sim/src/inference/traits.rs    (~130 lines)
```

```rust
use crate::error::SimError;
use crate::rng::StatefulRng;

/// Trait bound for particle state types that support observation-interval resets.
///
/// After each observation time, the inference algorithm resets accumulators
/// (e.g., flow counters for incidence projection). This is a STATE lifecycle
/// concern — it belongs on the state type, not on the observation model.
pub trait Resettable {
    /// Reset observation-interval accumulators to zero.
    fn reset_accumulators(&mut self);
}

/// A stochastic process model that can simulate forward in time.
///
/// Owns the model structure (compartments, transitions, stoichiometry)
/// and provides methods to initialize state, advance by one timestep,
/// and allocate reusable scratch buffers.
///
/// Generic over State and Scratch to support different backends
/// (chain-binomial, tau-leap, Gillespie) without boxing.
pub trait ProcessModel: Send + Sync {
    /// Particle state type.
    /// - `Clone`: resampling copies particles.
    /// - `Send`: rayon propagates particles across threads.
    /// - `Resettable`: algorithms reset flow accumulators between observations.
    type State: Clone + Send + Resettable;

    /// Pre-allocated scratch buffers, one per particle.
    /// Avoids heap allocation in the inner loop.
    type Scratch: Send;

    /// Number of integer compartments (for sizing).
    fn n_compartments(&self) -> usize;

    /// Number of transitions (for sizing flow accumulators).
    fn n_transitions(&self) -> usize;

    /// Create the initial state from parameters.
    fn initial_state(&self, params: &[f64]) -> Result<Self::State, SimError>;

    /// Advance state by one timestep.
    ///
    /// This is the hot path — called n_particles × n_substeps × n_obs times.
    /// Must not allocate. Uses scratch buffers for temporaries.
    fn step(
        &self,
        state: &mut Self::State,
        params: &[f64],
        t: f64,
        dt: f64,
        rng: &mut StatefulRng,
        scratch: &mut Self::Scratch,
    ) -> Result<(), SimError>;

    /// Allocate a fresh scratch buffer sized for this model.
    fn new_scratch(&self) -> Self::Scratch;
}

/// Observation model: maps latent state to data likelihood.
///
/// Encapsulates projection (state → observable quantity), likelihood
/// evaluation, sampling, and mean computation. Handles multi-stream
/// observations internally.
///
/// The `obs_idx` parameter indexes into the observation time series.
/// `params` is always passed — IF2 uses per-particle params, PF/PMMH
/// ignore it (params are baked in at construction). This is the correct
/// interface: the observation likelihood IS a function of params in general.
pub trait ObservationModel<S>: Send + Sync {
    /// Joint log p(y_{obs_idx} | state, params) across all streams.
    ///
    /// This is the ONLY method required for inference. All algorithms
    /// (PF, IF2, PMMH, PGAS) call this for particle weighting.
    fn log_likelihood(&self, state: &S, obs_idx: usize, params: &[f64]) -> f64;

    /// Number of observation times.
    fn n_observations(&self) -> usize;

    /// Observation time at index `obs_idx`.
    fn obs_time(&self, obs_idx: usize) -> f64;

    /// Number of observation streams.
    fn n_streams(&self) -> usize { 1 }

    /// Sample y ~ p(y | state, params) for prediction diagnostics.
    /// Returns one draw per stream.
    fn sample(
        &self, _state: &S, _obs_idx: usize,
        _params: &[f64], _rng: &mut StatefulRng,
    ) -> Vec<f64> {
        vec![]
    }

    /// E[y | state, params] for prediction diagnostics.
    /// Returns one value per stream.
    fn mean(&self, _state: &S, _obs_idx: usize, _params: &[f64]) -> Vec<f64> {
        vec![]
    }
}

/// Configuration shared by all SMC-based algorithms.
///
/// Note: `seed` is NOT here. Seed is per-chain, passed to the algorithm
/// call, not bundled into config. Config describes the statistical problem.
#[derive(Clone, Debug)]
pub struct SMCConfig {
    pub n_particles: usize,
    pub dt: f64,
}
```

### PGAS: The `DensityProcess` Supertrait

PGAS needs capabilities beyond `ProcessModel::step` — it evaluates transition
densities, simulates reference trajectories, and accesses source group
structure. Rather than making PGAS take a raw `&CompiledModel` (which hides the
requirement in a concrete type), we express it as a trait bound:

```rust
/// Extension of ProcessModel for algorithms that need transition density
/// evaluation (PGAS, CSMC-AS).
///
/// Only chain-binomial processes implement this. If you try to use PGAS
/// with a process model that doesn't implement `DensityProcess`, you get
/// a compile-time error — not a runtime panic or silent wrong answer.
///
/// This is intentionally a small trait surface. PGAS still accesses
/// `CompiledModel` internals through the concrete `ChainBinomialProcess`,
/// but the type system communicates the REQUIREMENT clearly.
pub trait DensityProcess: ProcessModel {
    /// Log transition density for one substep.
    ///
    /// Evaluates log p(flows | state_before, params, gammas, t, dt).
    /// Returns -inf for impossible transitions (flow > source count).
    fn log_transition_density(
        &self,
        state_before: &Self::State,
        flows: &[u64],
        gammas: &[f64],
        params: &[f64],
        t: f64,
        dt: f64,
    ) -> Result<f64, SimError>;

    /// Simulate a forward trajectory recording per-substep detail.
    /// Used to initialize the reference trajectory for PGAS.
    fn simulate_reference(
        &self,
        params: &[f64],
        t_end: f64,
        dt: f64,
        rng: &mut StatefulRng,
    ) -> Result<PGASTrajectory, SimError>;

    /// Access to the underlying compiled model for PGAS internals.
    ///
    /// This is the "escape hatch" — PGAS needs source groups,
    /// stoichiometry, and balance constraints that are deeply
    /// chain-binomial-specific. Rather than exposing 10 accessor
    /// methods that would each have exactly one implementor,
    /// we provide direct access and document that PGAS is
    /// chain-binomial-coupled by design.
    fn compiled_model(&self) -> &CompiledModel;
}
```

Then PGAS's signature becomes:

```rust
pub fn run_pgas<P: DensityProcess<State = ParticleState>>(
    process: &P,
    obs_model: &dyn ObservationModel<ParticleState>,
    if2_params: &[EstimatedParam],
    priors: &[Prior],
    base_params: &[f64],
    config: &PGASConfig,
    seed: u64,
    on_sweep: Option<&dyn Fn(usize, &PGASSweep, &PGASTrajectory)>,
    resume_from: Option<ChainResumeState>,
    config_hash: String,
) -> Result<PGASResult, SimError>
```

The key insight: if someone tries `run_pgas(&my_tau_leap_process, ...)`, they
get:

```
error[E0277]: the trait bound `TauLeapProcess: DensityProcess` is not satisfied
  --> src/main.rs:42:15
   |
42 |     run_pgas(&my_tau_leap_process, ...);
   |               ^^^^^^^^^^^^^^^^^^^^ the trait `DensityProcess` is not
   |                                    implemented for `TauLeapProcess`
   |
   = help: the following other types implement trait `DensityProcess`:
             ChainBinomialProcess
   = note: PGAS requires transition density evaluation, which is only
           available for chain-binomial processes
```

This is better than a runtime error, better than a doc comment, and better than
hiding the requirement behind a concrete type parameter. The type system tells
you what's required and what satisfies it.

### `Resettable` Implementation

```rust
impl Resettable for ParticleState {
    fn reset_accumulators(&mut self) {
        for f in &mut self.flow_accumulators { *f = 0; }
    }
}
```

One impl, used by all algorithms. No observation model needs to know about flow
accumulators.

### Concrete Implementations

#### ChainBinomialProcess

```
sim/src/inference/chain_binomial_process.rs    (~100 lines)
```

```rust
use crate::chain_binomial::{step_one, StepScratch};
use crate::compiled_model::CompiledModel;
use crate::inference::traits::{ProcessModel, DensityProcess, Resettable};
use crate::inference::types::ParticleState;
use crate::inference::pgas::{PGASTrajectory, log_transition_density_substep, simulate_reference};
use crate::error::SimError;
use crate::rng::StatefulRng;
use std::sync::Arc;

/// Chain-binomial process model.
///
/// Implements both `ProcessModel` (for PF, IF2, PMMH) and `DensityProcess`
/// (for PGAS). This is the only process backend that supports PGAS.
pub struct ChainBinomialProcess {
    pub compiled: Arc<CompiledModel>,
}

impl ProcessModel for ChainBinomialProcess {
    type State = ParticleState;
    type Scratch = StepScratch;

    fn n_compartments(&self) -> usize {
        self.compiled.int_local_to_global.len()
    }

    fn n_transitions(&self) -> usize {
        self.compiled.model.transitions.len()
    }

    fn initial_state(&self, params: &[f64]) -> Result<ParticleState, SimError> {
        let (init_int, _) = self.compiled.initial_state(params)?;
        let mut state = ParticleState::new(
            self.n_compartments(), self.n_transitions(),
        );
        state.counts.copy_from_slice(&init_int.counts);
        Ok(state)
    }

    fn step(
        &self,
        state: &mut ParticleState,
        params: &[f64],
        t: f64,
        dt: f64,
        rng: &mut StatefulRng,
        scratch: &mut StepScratch,
    ) -> Result<(), SimError> {
        step_one(
            &self.compiled,
            &mut state.counts,
            &mut state.flow_accumulators,
            params, t, dt, rng, scratch,
        )
    }

    fn new_scratch(&self) -> StepScratch {
        StepScratch::new(&self.compiled)
    }
}

impl DensityProcess for ChainBinomialProcess {
    fn log_transition_density(
        &self,
        state_before: &ParticleState,
        flows: &[u64],
        gammas: &[f64],
        params: &[f64],
        t: f64,
        dt: f64,
    ) -> Result<f64, SimError> {
        log_transition_density_substep(
            &self.compiled, &state_before.counts, flows,
            gammas, params, t, dt,
        )
    }

    fn simulate_reference(
        &self,
        params: &[f64],
        t_end: f64,
        dt: f64,
        rng: &mut StatefulRng,
    ) -> Result<PGASTrajectory, SimError> {
        simulate_reference(&self.compiled, params, t_end, dt, rng)
    }

    fn compiled_model(&self) -> &CompiledModel {
        &self.compiled
    }
}
```

#### MultiStreamObsModel

```
sim/src/inference/multi_stream_obs.rs    (~120 lines)
```

```rust
use crate::inference::traits::ObservationModel;
use crate::inference::types::ParticleState;
use crate::inference::obs_model;
use crate::compiled_model::CompiledModel;
use crate::rng::StatefulRng;
use std::sync::Arc;
use ir::observation::ObservationModel as IrObsModel;

/// One observation stream.
struct Stream {
    flow_indices: Vec<usize>,
    loglik_fn: Box<dyn Fn(f64, f64, &[f64]) -> f64 + Send + Sync>,
    sample_fn: Box<dyn Fn(f64, &mut StatefulRng) -> f64 + Send + Sync>,
    mean_fn: Box<dyn Fn(f64) -> f64 + Send + Sync>,
    observations: Vec<f64>,
}

/// Multi-stream observation model.
///
/// Constructed once from IR observation blocks + data.
/// The `log_likelihood` closure receives `params` so IF2 can pass
/// per-particle params. For PF/PMMH, the params are the same
/// every call — the closure may or may not use them depending
/// on whether obs model parameters are estimated.
pub struct MultiStreamObsModel {
    streams: Vec<Stream>,
    obs_times: Vec<f64>,
}

/// Specification for building one observation stream.
pub struct StreamSpec {
    pub flow_indices: Vec<usize>,
    pub ir_model: IrObsModel,
    pub observations: Vec<f64>,
    pub obs_times: Vec<f64>,
}

impl MultiStreamObsModel {
    pub fn new(
        stream_specs: Vec<StreamSpec>,
        compiled: Arc<CompiledModel>,
    ) -> Self {
        let obs_times = stream_specs[0].obs_times.clone();
        let streams = stream_specs.into_iter().map(|spec| {
            // Use IF2-style closure that accepts params
            let loglik_fn = obs_model::compile_obs_loglik_if2(
                &spec.ir_model, compiled.clone(),
            );
            let sample_fn = obs_model::compile_obs_sample_pf(
                &spec.ir_model, compiled.clone(), &[], // params bound later per-call
            );
            let mean_fn = obs_model::compile_obs_mean_pf(
                &spec.ir_model, compiled.clone(), &[],
            );
            Stream {
                flow_indices: spec.flow_indices,
                loglik_fn,
                sample_fn,
                mean_fn,
                observations: spec.observations,
            }
        }).collect();
        MultiStreamObsModel { streams, obs_times }
    }

    fn project(&self, state: &ParticleState, stream_idx: usize) -> f64 {
        self.streams[stream_idx].flow_indices.iter()
            .map(|&i| state.flow_accumulators[i] as f64).sum()
    }
}

impl ObservationModel<ParticleState> for MultiStreamObsModel {
    fn log_likelihood(
        &self, state: &ParticleState, obs_idx: usize, params: &[f64],
    ) -> f64 {
        self.streams.iter().enumerate().map(|(si, s)| {
            let projected = self.project(state, si);
            (s.loglik_fn)(projected, s.observations[obs_idx], params)
        }).sum()
    }

    fn n_observations(&self) -> usize { self.obs_times.len() }
    fn obs_time(&self, obs_idx: usize) -> f64 { self.obs_times[obs_idx] }
    fn n_streams(&self) -> usize { self.streams.len() }

    fn sample(
        &self, state: &ParticleState, obs_idx: usize,
        _params: &[f64], rng: &mut StatefulRng,
    ) -> Vec<f64> {
        self.streams.iter().enumerate().map(|(si, s)| {
            let projected = self.project(state, si);
            (s.sample_fn)(projected, rng)
        }).collect()
    }

    fn mean(
        &self, state: &ParticleState, obs_idx: usize, _params: &[f64],
    ) -> Vec<f64> {
        self.streams.iter().enumerate().map(|(si, s)| {
            let projected = self.project(state, si);
            (s.mean_fn)(projected)
        }).collect()
    }
}
```

### Refactored Algorithm Signatures

#### Bootstrap Particle Filter

Before (12 args):

```rust
pub fn bootstrap_filter(
    model: &CompiledModel, params: &[f64], observations: &[Observation],
    n_particles: usize, dt: f64, step_fn: &StepFn,
    project_fn: &dyn Fn(&ParticleState) -> f64, obs_loglik_fn: &ObsLoglikFn,
    obs_sample_fn: Option<&ObsSampleFn>, obs_mean_fn: Option<&ObsMeanFn>,
    seed: u64, joint_obs_fn: Option<&JointObsFn>,
) -> Result<PFilterResult, SimError>
```

After (5 args):

```rust
pub fn bootstrap_filter<P: ProcessModel<State = ParticleState>>(
    process: &P,
    obs_model: &dyn ObservationModel<ParticleState>,
    params: &[f64],
    config: &SMCConfig,
    seed: u64,
) -> Result<PFilterResult, SimError>
```

#### IF2

Before (10 args + callback):

```rust
pub fn run_if2(
    model: &CompiledModel, base_params: &[f64], if2_params: &[EstimatedParam],
    observations: &[Observation], config: &IF2Config,
    step_fn: &(dyn Fn(...) + Send + Sync),
    project_fn: &dyn Fn(&ParticleState) -> f64,
    obs_loglik_fn: &dyn Fn(f64, f64, &[f64]) -> f64, seed: u64,
) -> Result<IF2Result, SimError>
```

After (6 args):

```rust
pub fn run_if2<P: ProcessModel<State = ParticleState>>(
    process: &P,
    obs_model: &dyn ObservationModel<ParticleState>,
    if2_params: &[EstimatedParam],
    base_params: &[f64],
    config: &IF2Config,
    seed: u64,
) -> Result<IF2Result, SimError>
```

No separate `IF2ObservationModel` trait needed —
`ObservationModel::log_likelihood` already takes `params`, which is what IF2
needs. The leak was in the old design where PF's obs model didn't take params,
forcing a separate closure type.

#### PMMH

```rust
pub fn run_pmmh<P: ProcessModel<State = ParticleState>>(
    process: &P,
    obs_model: &dyn ObservationModel<ParticleState>,
    if2_params: &[EstimatedParam],
    priors: &[Prior],
    base_params: &[f64],
    config: &PMMHConfig,
    seed: u64,
    diagnostics: &DiagnosticCollector,
) -> Result<PMMHResult, SimError>
```

#### PGAS

```rust
pub fn run_pgas<P: DensityProcess<State = ParticleState>>(
    process: &P,
    obs_model: &dyn ObservationModel<ParticleState>,
    if2_params: &[EstimatedParam],
    priors: &[Prior],
    base_params: &[f64],
    config: &PGASConfig,
    seed: u64,
    on_sweep: Option<&dyn Fn(usize, &PGASSweep, &PGASTrajectory)>,
    resume_from: Option<ChainResumeState>,
    config_hash: String,
) -> Result<PGASResult, SimError>
```

The `DensityProcess` bound makes it a compile error to pass a non-chain-binomial
process. The error message from rustc explains exactly what's missing.

### CLI Plumbing: Before and After

Before (6 duplicated sites):

```rust
// In run_quick_pfilter:
let step_fn = |state: &mut ParticleState, t: f64, step_dt: f64, rng: &mut StatefulRng, scratch: &mut StepScratch| {
    step_one(compiled, &mut state.counts, &mut state.flow_accumulators, params, t, step_dt, rng, scratch)
};
let flow_indices = &config.flow_indices;
let project_fn = |state: &ParticleState| -> f64 {
    flow_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
};
let obs_loglik_fn = compile_obs_loglik_pf(&config.obs_model_ir, config.compiled.clone(), params);
// ... 5 more of these in other functions
```

After (constructed once in `FitRunConfig::build`):

```rust
// In FitRunConfig:
pub process: ChainBinomialProcess,
pub obs_model: MultiStreamObsModel,

// Every call site:
bootstrap_filter(&config.process, &config.obs_model, &params, &smc_config, seed)?;
run_if2(&config.process, &config.obs_model, &if2_params, &base_params, &if2_config, seed)?;
```

### Migration Plan (No Back-Compat)

Since the code is unreleased, we do a clean swap in two steps:

#### Step 1: Add traits + impls + tests (~300 LOC)

Add `traits.rs`, `chain_binomial_process.rs`, `multi_stream_obs.rs`. Write tests
that verify the trait impls match the current behavior:

```rust
#[test]
fn chain_binomial_process_step_matches_step_one() { ... }

#[test]
fn multi_stream_obs_matches_joint_obs_weight() { ... }

#[test]
fn density_process_matches_log_transition_density_substep() { ... }
```

#### Step 2: Rewrite algorithm signatures + all call sites

Rewrite `bootstrap_filter`, `run_if2`, `run_pmmh`, `run_pgas` to use the
trait-based API. Rewrite all 6 CLI call sites. Delete old closure types,
`ObsStreamSpec`, `joint_obs_weight`, `joint_obs_weight_particle`.

Run full test suite after each file migration.

No `_v2` functions, no coexistence period, no rename dance.

### What This Enables

1. **Adding new algorithms is easy.** Implement IBPF or Liu-West filter by
   taking `&dyn ProcessModel + &dyn ObservationModel`. No closure plumbing.

2. **Adding new backends is easy.** Implement `ProcessModel` for tau-leap. PF,
   IF2, and PMMH work automatically. PGAS gives a compile error (correct — it
   needs `DensityProcess`).

3. **Testing is cleaner.** `MockProcessModel` with deterministic trajectories.
   Test PF logic without a real compiled model.

4. **The CLI layer shrinks.** `FitRunConfig` stores the process + obs model. All
   plumbing code disappears from 6 call sites.

### Estimated LOC Impact

| Component                                                             | Added   | Removed | Net      |
| --------------------------------------------------------------------- | ------- | ------- | -------- |
| `traits.rs`                                                           | 130     | 0       | +130     |
| `chain_binomial_process.rs`                                           | 100     | 0       | +100     |
| `multi_stream_obs.rs`                                                 | 120     | 0       | +120     |
| `Resettable` impl on ParticleState                                    | 5       | 0       | +5       |
| `bootstrap_filter` rewrite                                            | 40      | 80      | −40      |
| `run_if2` rewrite                                                     | 20      | 60      | −40      |
| Closure type aliases deleted                                          | 0       | 40      | −40      |
| `ObsStreamSpec` + `joint_obs_weight` + `joint_obs_weight_particle`    | 0       | 60      | −60      |
| CLI plumbing cleanup (6 sites × ~15 lines)                            | 6       | 90      | −84      |
| Tests                                                                 | 150     | 0       | +150     |
| Old `ChainBinomialProcess` (existing trait impl in chain_binomial.rs) | 0       | 20      | −20      |
| **Total**                                                             | **571** | **350** | **+221** |

Net positive because of tests. Production code is net −130.

### Open Questions

1. **`ObservationModel` as a generic trait vs associated type.** The current
   design uses `ObservationModel<S>` (generic parameter) rather than
   `ObservationModel { type State; }` (associated type). Generic parameter means
   one type could implement `ObservationModel<ParticleState>` AND
   `ObservationModel<ContinuousState>`. This is theoretically cleaner but
   practically unnecessary — an obs model is always paired with one process
   model. **Decision:** Use generic parameter. It makes the trait bounds on
   algorithm signatures cleaner (`&dyn ObservationModel<ParticleState>` vs
   `&dyn ObservationModel<State=ParticleState>`). Marginal preference.

2. **Should `DensityProcess::compiled_model()` exist?** It's an escape hatch
   that exposes the concrete `CompiledModel` through a trait. This is honest
   (PGAS really does need it) but somewhat defeats the abstraction. The
   alternative is 10+ accessor methods (`source_groups()`,
   `transition_stoich()`, etc.) that each have exactly one implementor.
   **Decision:** Keep the escape hatch. PGAS is coupled to chain-binomial by
   design. The `DensityProcess` bound already communicates this. Adding accessor
   methods would be abstraction theater.

3. **Should `run_pgas` take `&P` or `&ChainBinomialProcess` directly?** Taking
   `&P: DensityProcess` is more principled (the type system enforces the bound).
   Taking `&ChainBinomialProcess` is more honest (there will never be another
   implementor). **Decision:** Take `&P: DensityProcess`. It costs nothing, the
   error messages are better, and it leaves the door open for a hypothetical
   tau-leap-with-density backend without any API change.
