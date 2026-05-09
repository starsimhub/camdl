//! Chain-binomial process model implementing ProcessModel + DensityProcess.
//!
//! This is the only process backend that supports PGAS (via DensityProcess).
//! PF, IF2, and PMMH work through ProcessModel alone.

use std::sync::Arc;
use crate::chain_binomial::{step_one, StepScratch};
use crate::compiled_model::CompiledModel;
use crate::error::SimError;
use crate::rng::StatefulRng;
use super::traits::{ProcessModel, DensityProcess};
use super::types::ParticleState;

/// Chain-binomial process model.
///
/// Wraps a `CompiledModel` and delegates to `step_one` for simulation.
/// Implements `ProcessModel` (for PF, IF2, PMMH) and `DensityProcess`
/// (for PGAS). The only process backend that supports PGAS.
pub struct ChainBinomialProcess {
    pub compiled: Arc<CompiledModel>,
    /// Runtime view of intervention/event fire steps, resolved once
    /// at construction using the integrator's `dt`. gh#53 — the
    /// CompiledModel stores dt-invariant `fire_times`; the per-run
    /// `fire_steps` view depends on the runtime dt and must be
    /// resolved with that value, not the compile-time
    /// `model.simulation.dt`.
    pub(crate) fire_steps: Vec<std::collections::BTreeSet<i64>>,
}

impl ChainBinomialProcess {
    /// Construct a process for a model with integrator step `dt`.
    /// `dt` is required because `fire_steps` (the runtime view of
    /// the model's intervention schedule) must be resolved with it
    /// (see gh#53). Reusing the same process across runs at
    /// different dts is unsupported — build a fresh process per
    /// dt; the gh#52 Richardson ladder already does this via
    /// `run_quick_pfilter_with_dt`'s per-rung config rebuild.
    pub fn new(compiled: Arc<CompiledModel>, dt: f64) -> Self {
        let fire_steps = compiled.resolve_fire_steps(dt);
        ChainBinomialProcess { compiled, fire_steps }
    }
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
            &self.fire_steps,
        )
    }

    fn new_scratch(&self) -> StepScratch {
        StepScratch::new(&self.compiled)
    }
}

impl DensityProcess for ChainBinomialProcess {
    fn log_transition_density(
        &self,
        counts_before: &[i64],
        flows: &[u64],
        gammas: &[f64],
        params: &[f64],
        t: f64,
        dt: f64,
    ) -> Result<f64, SimError> {
        super::pgas::log_transition_density_substep(
            &self.compiled, counts_before, flows, gammas, params, t, dt,
        )
    }

    fn compiled_model(&self) -> &CompiledModel {
        &self.compiled
    }
}
