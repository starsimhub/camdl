/// Integer compartment state — one `i64` per integer compartment, in model order.
#[derive(Debug, Clone, PartialEq)]
pub struct IntState {
    pub counts: Vec<i64>,
}

impl IntState {
    pub fn new(n: usize) -> Self {
        IntState { counts: vec![0; n] }
    }

    pub fn from_vec(counts: Vec<i64>) -> Self {
        IntState { counts }
    }

    /// Clamp all components to ≥ 0 in-place.
    /// Returns the number of components that were clamped (0 = no violation).
    pub fn clamp_nonneg(&mut self) -> usize {
        let mut clamped = 0;
        for v in &mut self.counts {
            if *v < 0 {
                *v = 0;
                clamped += 1;
            }
        }
        clamped
    }

    pub fn total(&self) -> i64 {
        self.counts.iter().sum()
    }
}

/// Real compartment state — one `f64` per real compartment, in model order.
#[derive(Debug, Clone, PartialEq)]
pub struct RealState {
    pub values: Vec<f64>,
}

impl RealState {
    pub fn new(n: usize) -> Self {
        RealState { values: vec![0.0; n] }
    }

    pub fn from_vec(values: Vec<f64>) -> Self {
        RealState { values }
    }

    /// Clamp all components to ≥ 0 in-place.
    /// Returns the number of components that were clamped.
    pub fn clamp_nonneg(&mut self) -> usize {
        let mut clamped = 0;
        for v in &mut self.values {
            if *v < 0.0 {
                *v = 0.0;
                clamped += 1;
            }
        }
        clamped
    }
}

/// Cumulative flow counters — one `u64` per transition, reset at each output boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowVec {
    pub counts: Vec<u64>,
}

impl FlowVec {
    pub fn new(n: usize) -> Self {
        FlowVec { counts: vec![0; n] }
    }

    pub fn from_vec(counts: Vec<u64>) -> Self {
        FlowVec { counts }
    }

    pub fn reset(&mut self) {
        for v in &mut self.counts {
            *v = 0;
        }
    }

    pub fn add(&mut self, transition_idx: usize, n: u64) {
        self.counts[transition_idx] += n;
    }
}

/// A single recorded state snapshot at time `t`.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub t: f64,
    pub int_state: IntState,
    pub real_state: RealState,
    /// Cumulative flows since the previous snapshot (or since t_start for the first).
    pub flows: FlowVec,
}

/// The full time series produced by a simulation run.
#[derive(Debug, Clone)]
pub struct Trajectory {
    pub snapshots: Vec<Snapshot>,
    /// Per-transition firing diagnostics (populated by Gillespie; empty for tau-leap/chain-binomial).
    pub transition_diagnostics: Vec<crate::transition_diagnostics::TransitionDiagnostics>,
}

impl Trajectory {
    pub fn new() -> Self {
        Trajectory {
            snapshots: Vec::new(),
            transition_diagnostics: Vec::new(),
        }
    }

    pub fn push(&mut self, snap: Snapshot) {
        self.snapshots.push(snap);
    }
}

impl Default for Trajectory {
    fn default() -> Self { Self::new() }
}
