use crate::{
    compiled_model::CompiledModel,
    config::SimConfig,
    error::SimError,
    state::Trajectory,
    Capabilities,
};

pub trait Simulate {
    fn run(
        &self,
        model: &CompiledModel,
        params: &[f64],
        seed: u64,
        config: &SimConfig,
    ) -> Result<Trajectory, SimError>;

    /// Features this backend supports.
    fn capabilities(&self) -> Capabilities;

    /// Human-readable backend name for error messages.
    fn name(&self) -> &'static str;
}
