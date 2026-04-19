//! Ancestor tracing: recover coherent per-particle trajectories from a
//! bootstrap particle filter run.
//!
//! After running the filter, the per-step pre-resample particle states
//! give you filtering marginals `p(x_t | y_{1..t})` at each `t` — but
//! joining particles across `t` by index does NOT give sample paths
//! from the smoothing distribution `p(x_{1:T} | y_{1:T})`, because
//! resampling rearranges the swarm at every step.
//!
//! To recover smoothing draws: pick a particle `j` at the final step
//! with probability proportional to its final log-weight, walk its
//! ancestor chain back to the first step, and collect the state at
//! each point. Doing this N times yields N independent samples from
//! the smoothing distribution (all equally weighted).
//!
//! This module is the primitive both `sim::inference::particle_filter`
//! and (future) `sim::inference::pgas` can share. See
//! `docs/dev/proposals/2026-04-19-pf-latent-trajectories.md`.

use crate::rng::StatefulRng;

/// Per-step ancestry + states recorded during a bootstrap-filter run.
///
/// `states[t][i]` is the **pre-resample** state of particle `i` at
/// observation step `t` (the state on which `log_weights[t][i]` is
/// computed).
///
/// `ancestors[t][i]` is the resampling index used for particle `i`
/// at the END of step `t` — i.e. the parent-in-step-`t` of the
/// particle that propagates into position `i` at step `t+1`. The
/// last entry (`ancestors[T-1]`) is unused for path tracing and
/// MAY be omitted by the recorder.
#[derive(Debug, Clone)]
pub struct AncestorTrace {
    /// Compartment names (for schema clarity at write time).
    pub n_compartments: usize,
    /// `states[t][i][k]` = count in compartment `k` of pre-resample
    /// particle `i` at observation step `t`.
    pub states: Vec<Vec<Vec<f64>>>,
    /// `log_weights[t][i]` = filtering log-weight of pre-resample
    /// particle `i` at observation step `t`.
    pub log_weights: Vec<Vec<f64>>,
    /// `ancestors[t][i]` = parent index in step `t`'s pre-resample
    /// swarm of the particle that propagates into position `i` at
    /// step `t+1`. Length `states.len() - 1` (no ancestors needed
    /// after the final step).
    pub ancestors: Vec<Vec<usize>>,
    /// Observation times, one per step (matches `states.len()`).
    pub obs_times: Vec<f64>,
}

/// One ancestor-traced sample path from the smoothing distribution.
/// `states[t][k]` = count in compartment `k` at observation step
/// `t`. Length `trace.states.len()`.
#[derive(Debug, Clone)]
pub struct SampledPath {
    pub states: Vec<Vec<f64>>,
    pub obs_times: Vec<f64>,
}

/// Sample `n` trajectory paths from the smoothing distribution
/// `p(x_{1:T} | y_{1:T}, θ)` via ancestor tracing on a bootstrap-
/// filter trace.
///
/// Algorithm: for each output path, sample a final-step particle
/// with probability ∝ `exp(log_weights[T-1])`, then walk backwards
/// collecting each step's pre-resample state. Each returned path is
/// an independent, equally-weighted draw from the smoothing
/// distribution.
pub fn sample_paths(
    trace: &AncestorTrace,
    n_paths: usize,
    seed: u64,
) -> Vec<SampledPath> {
    let n_obs = trace.states.len();
    if n_obs == 0 || n_paths == 0 {
        return Vec::new();
    }

    let mut rng = StatefulRng::new(seed.wrapping_add(0xa5ce57ea));
    let final_log_w = &trace.log_weights[n_obs - 1];

    // Normalise final-step weights to a CDF we can inverse-sample
    // from. Uses the usual log-sum-exp trick for numerical safety.
    let max_lw = final_log_w.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let total: f64 = if max_lw.is_infinite() {
        // Pathological: all -inf. Fall back to uniform over particles.
        final_log_w.len() as f64
    } else {
        final_log_w.iter().map(|&lw| (lw - max_lw).exp()).sum()
    };

    let mut paths = Vec::with_capacity(n_paths);
    for _ in 0..n_paths {
        // Pick the final particle.
        let j_final = if max_lw.is_infinite() {
            (rng.uniform() * final_log_w.len() as f64) as usize
        } else {
            let target = rng.uniform() * total;
            let mut acc = 0.0;
            let mut chosen = final_log_w.len() - 1;
            for (i, &lw) in final_log_w.iter().enumerate() {
                acc += (lw - max_lw).exp();
                if acc >= target { chosen = i; break; }
            }
            chosen
        };

        // Walk back. states[t] is pre-resample; ancestors[t] points
        // into states[t]'s indices and IS the parent of position i
        // at step t+1.
        let mut states = vec![Vec::new(); n_obs];
        let mut j = j_final;
        states[n_obs - 1] = trace.states[n_obs - 1][j].clone();
        for t in (0..n_obs - 1).rev() {
            j = trace.ancestors[t][j];
            states[t] = trace.states[t][j].clone();
        }
        paths.push(SampledPath {
            states,
            obs_times: trace.obs_times.clone(),
        });
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trace(
        n_obs: usize,
        n_particles: usize,
        n_comps: usize,
    ) -> AncestorTrace {
        // Synthetic trace: particle i at step t has count[0] = t*100 + i
        // so we can recognise a path by its value sequence.
        let states: Vec<Vec<Vec<f64>>> = (0..n_obs)
            .map(|t| (0..n_particles)
                .map(|i| {
                    let mut s = vec![0.0; n_comps];
                    s[0] = (t * 100 + i) as f64;
                    s
                })
                .collect())
            .collect();
        let log_weights = vec![vec![0.0; n_particles]; n_obs];
        // All particles at step t+1 descend from particle 0 at step t
        // — a "single surviving lineage" ancestry. Every final-step
        // particle walks back to particle 0 at every prior step.
        let ancestors: Vec<Vec<usize>> = (0..n_obs - 1)
            .map(|_| vec![0; n_particles])
            .collect();
        let obs_times: Vec<f64> = (0..n_obs).map(|t| t as f64).collect();
        AncestorTrace {
            n_compartments: n_comps,
            states,
            log_weights,
            ancestors,
            obs_times,
        }
    }

    #[test]
    fn single_lineage_all_paths_go_through_particle_0() {
        // Every ancestor at every step is 0, so every sampled path
        // traces through particle 0 at every t < final. Only the final
        // step is sensitive to the final-step sampling.
        let trace = make_trace(5, 10, 2);
        let paths = sample_paths(&trace, 4, 42);
        assert_eq!(paths.len(), 4);
        for p in &paths {
            assert_eq!(p.states.len(), 5, "path spans all obs steps");
            // Steps 0..4 should all have count[0] = t*100 + 0 since
            // every backward walk lands on particle 0.
            for t in 0..4 {
                assert_eq!(p.states[t][0], (t * 100) as f64,
                    "step {} of path walked off the expected lineage", t);
            }
        }
    }

    #[test]
    fn identity_ancestry_preserves_lineages() {
        // ancestors[t][i] = i — no resampling shuffle. Each final
        // particle's backward walk stays on its own lineage; so
        // path for final=j has count[0] = t*100 + j at every t.
        let n_obs = 4;
        let n_particles = 6;
        let mut trace = make_trace(n_obs, n_particles, 1);
        for t in 0..n_obs - 1 {
            trace.ancestors[t] = (0..n_particles).collect();
        }
        // Force final selection to pick a specific particle by making
        // all but one weight -inf.
        let target_final = 3;
        for i in 0..n_particles {
            trace.log_weights[n_obs - 1][i] =
                if i == target_final { 0.0 } else { f64::NEG_INFINITY };
        }
        let paths = sample_paths(&trace, 5, 99);
        for p in &paths {
            for t in 0..n_obs {
                assert_eq!(p.states[t][0], (t * 100 + target_final) as f64,
                    "identity ancestry should preserve lineage {}", target_final);
            }
        }
    }

    #[test]
    fn empty_trace_returns_empty() {
        let trace = AncestorTrace {
            n_compartments: 2,
            states: vec![],
            log_weights: vec![],
            ancestors: vec![],
            obs_times: vec![],
        };
        assert!(sample_paths(&trace, 10, 0).is_empty());
    }

    #[test]
    fn zero_paths_requested_returns_empty() {
        let trace = make_trace(3, 4, 1);
        assert!(sample_paths(&trace, 0, 0).is_empty());
    }

    #[test]
    fn all_neg_inf_weights_falls_back_to_uniform() {
        // Shouldn't panic even in the degenerate case.
        let mut trace = make_trace(3, 5, 1);
        for w in &mut trace.log_weights[2] { *w = f64::NEG_INFINITY; }
        let paths = sample_paths(&trace, 10, 7);
        assert_eq!(paths.len(), 10);
    }
}
