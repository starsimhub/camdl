use crate::{
    compiled_model::CompiledModel,
    config::SimConfig,
    error::SimError,
    state::Trajectory,
};

pub trait Simulate {
    fn run(
        &self,
        model: &CompiledModel,
        params: &[f64],
        seed: u64,
        config: &SimConfig,
    ) -> Result<Trajectory, SimError>;
}
