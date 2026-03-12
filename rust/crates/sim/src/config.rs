/// Config for the Gillespie (exact SSA) backend.
/// No `dt` — time steps are drawn from exponential distributions.
#[derive(Debug, Clone)]
pub struct GillespieConfig {
    pub t_start: f64,
    pub t_end: f64,
    /// How often to record output. If None, record at every event.
    pub output_dt: Option<f64>,
}

/// Config for the tau-leaping backend.
#[derive(Debug, Clone)]
pub struct TauLeapConfig {
    pub t_start: f64,
    pub t_end: f64,
    pub dt: f64,
}

/// Config for the discrete-time chain-binomial backend.
#[derive(Debug, Clone)]
pub struct ChainBinomialConfig {
    pub t_start: f64,
    pub t_end: f64,
    pub dt: f64,
}

/// Dispatch enum — lets cross-backend test loops iterate over all three backends.
/// Use the type system: `dt` only exists in the variants that need it.
#[derive(Debug, Clone)]
pub enum SimConfig {
    Gillespie(GillespieConfig),
    TauLeap(TauLeapConfig),
    ChainBinomial(ChainBinomialConfig),
}

impl SimConfig {
    pub fn t_start(&self) -> f64 {
        match self {
            SimConfig::Gillespie(c) => c.t_start,
            SimConfig::TauLeap(c) => c.t_start,
            SimConfig::ChainBinomial(c) => c.t_start,
        }
    }

    pub fn t_end(&self) -> f64 {
        match self {
            SimConfig::Gillespie(c) => c.t_end,
            SimConfig::TauLeap(c) => c.t_end,
            SimConfig::ChainBinomial(c) => c.t_end,
        }
    }

    pub fn variant_name(&self) -> &'static str {
        match self {
            SimConfig::Gillespie(_) => "Gillespie",
            SimConfig::TauLeap(_) => "TauLeap",
            SimConfig::ChainBinomial(_) => "ChainBinomial",
        }
    }
}
