use serde::{Deserialize, Serialize};
use crate::expr::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OdeEquation {
    pub compartment: String,
    pub derivative:  Expr,
}
