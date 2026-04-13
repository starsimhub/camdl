//! Multi-stream observation model implementing ObservationModel<ParticleState>.
//!
//! Constructed once from IR observation blocks + data. Stores resolved
//! likelihood expressions and evaluates them with params at call time —
//! no baked-in params. This means IF2, PGAS, and PMMH all correctly
//! respond to observation-level parameter changes (e.g., sigma_se).

use std::sync::Arc;
use crate::compiled_model::CompiledModel;
use crate::rng::StatefulRng;
use crate::resolved_expr::ResolvedLikelihood;
use crate::state::{IntState, RealState};
use super::traits::ObservationModel;
use super::types::ParticleState;
use super::obs_model::{
    resolve_likelihood_from_model, eval_likelihood_resolved,
    sample_obs_resolved, eval_obs_mean_resolved,
};

/// One observation stream: projection indices, resolved likelihood, and data.
struct Stream {
    /// Indices into flow_accumulators for this stream's projection.
    flow_indices: Vec<usize>,
    /// Resolved likelihood expression tree (pre-resolved at construction,
    /// but evaluates with params at call time — no baked-in values).
    resolved: ResolvedLikelihood,
    /// Observed values indexed by observation time index.
    observations: Vec<f64>,
}

/// Multi-stream observation model.
///
/// Stores resolved likelihoods and evaluates with params at call time.
/// This is the fix for the obs-level parameter bug: PGAS and PMMH now
/// correctly re-evaluate obs likelihood when sigma_se etc. change.
pub struct MultiStreamObsModel {
    streams: Vec<Stream>,
    obs_times: Vec<f64>,
    compiled: Arc<CompiledModel>,
    /// Pre-allocated state objects for expression evaluation context.
    /// These are dummies (zeroed) — the obs model only reads params
    /// and the projected value, not compartment counts.
    int_s: IntState,
    real_s: RealState,
}

/// Specification for building one observation stream.
pub struct StreamSpec {
    pub flow_indices: Vec<usize>,
    pub ir_model: ir::observation::ObservationModel,
    pub observations: Vec<f64>,
    pub obs_times: Vec<f64>,
}

impl MultiStreamObsModel {
    /// Create an empty observation model (no streams, no data).
    /// Used when only the transition density is needed (e.g., gradient tests
    /// with no observation data). `log_likelihood_from_flows` returns 0.0.
    pub fn empty(compiled: Arc<CompiledModel>) -> Self {
        let int_s = IntState::new(compiled.int_local_to_global.len());
        let real_s = RealState::new(compiled.real_local_to_global.len());
        MultiStreamObsModel { streams: vec![], obs_times: vec![], compiled, int_s, real_s }
    }

    pub fn new(
        stream_specs: Vec<StreamSpec>,
        compiled: Arc<CompiledModel>,
    ) -> Self {
        assert!(!stream_specs.is_empty(), "at least one observation stream required");
        let obs_times = stream_specs[0].obs_times.clone();

        let streams = stream_specs.into_iter().map(|spec| {
            let resolved = resolve_likelihood_from_model(
                &spec.ir_model.likelihood, &compiled,
            );
            Stream {
                flow_indices: spec.flow_indices,
                resolved,
                observations: spec.observations,
            }
        }).collect();

        let int_s = IntState::new(compiled.int_local_to_global.len());
        let real_s = RealState::new(compiled.real_local_to_global.len());

        MultiStreamObsModel { streams, obs_times, compiled, int_s, real_s }
    }

    /// Project from particle state to observable quantity for one stream.
    fn project(&self, state: &ParticleState, stream_idx: usize) -> f64 {
        self.streams[stream_idx].flow_indices.iter()
            .map(|&i| state.flow_accumulators[i] as f64).sum()
    }

    /// Project from cumulative flow array (used by PGAS which doesn't
    /// have a ParticleState, only raw flow arrays).
    pub fn log_likelihood_from_flows(
        &self, cum_flows: &[u64], obs_idx: usize, params: &[f64],
    ) -> f64 {
        self.streams.iter().map(|s| {
            let projected: f64 = s.flow_indices.iter()
                .map(|&i| cum_flows[i] as f64).sum();
            eval_likelihood_resolved(
                &s.resolved, projected, s.observations[obs_idx],
                params, &self.compiled, &self.int_s, &self.real_s,
            )
        }).sum()
    }
}

impl ObservationModel<ParticleState> for MultiStreamObsModel {
    fn log_likelihood(
        &self, state: &ParticleState, obs_idx: usize, params: &[f64],
    ) -> f64 {
        self.streams.iter().enumerate().map(|(si, s)| {
            let projected = self.project(state, si);
            eval_likelihood_resolved(
                &s.resolved, projected, s.observations[obs_idx],
                params, &self.compiled, &self.int_s, &self.real_s,
            )
        }).sum()
    }

    fn n_observations(&self) -> usize { self.obs_times.len() }
    fn obs_time(&self, obs_idx: usize) -> f64 { self.obs_times[obs_idx] }
    fn n_streams(&self) -> usize { self.streams.len() }

    fn sample(
        &self, state: &ParticleState, _obs_idx: usize,
        params: &[f64], rng: &mut StatefulRng,
    ) -> Vec<f64> {
        self.streams.iter().enumerate().map(|(si, s)| {
            let projected = self.project(state, si);
            sample_obs_resolved(
                &s.resolved, projected, params,
                &self.compiled, &self.int_s, &self.real_s, rng,
            )
        }).collect()
    }

    fn mean(
        &self, state: &ParticleState, _obs_idx: usize, params: &[f64],
    ) -> Vec<f64> {
        self.streams.iter().enumerate().map(|(si, s)| {
            let projected = self.project(state, si);
            eval_obs_mean_resolved(
                &s.resolved, projected, params,
                &self.compiled, &self.int_s, &self.real_s,
            )
        }).collect()
    }
}
