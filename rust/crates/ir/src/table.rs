use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OobPolicy {
    Clamp,
    Wrap,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub name:          String,
    pub values:        Vec<f64>,
    pub out_of_bounds: OobPolicy,
}
