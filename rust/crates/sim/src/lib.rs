pub mod config;
pub mod error;
pub mod state;
pub mod compiled_model;
pub mod propensity;
pub mod output;
pub mod ekrng;
pub mod ode_integrator;
pub mod gillespie;
pub mod tau_leap;
pub mod chain_binomial;
pub mod ode;
pub mod intervention;
pub mod simulate;
pub mod transition_diagnostics;

pub use config::{GillespieConfig, TauLeapConfig, ChainBinomialConfig, OdeConfig, SimConfig};
pub use error::SimError;
pub use state::{IntState, RealState, FlowVec, Snapshot, Trajectory};
pub use compiled_model::CompiledModel;
pub use simulate::Simulate;
pub use gillespie::GillespieSim;
pub use tau_leap::TauLeapSim;
pub use chain_binomial::ChainBinomialSim;
pub use ode::OdeSim;
pub use transition_diagnostics::{TransitionDiagnostics, write_tsv as write_diagnostics_tsv, warn_zero_firings};

// ── Backend capability constraints ────────────────────────────────────────

bitflags::bitflags! {
    /// Model features that constrain which backends can run a model.
    /// The `CompiledModel` declares what it requires; each backend declares
    /// what it provides.  Mismatch → hard error at dispatch time.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Capabilities: u32 {
        /// Transitions with `overdispersion` (NegBinomial draws).
        /// Supported by tau-leap and chain-binomial, not Gillespie or ODE.
        const OVERDISPERSION    = 1 << 0;
        /// Real-valued compartments with explicit ODE equations (PDMP).
        const REAL_COMPARTMENTS = 1 << 1;
    }
}
