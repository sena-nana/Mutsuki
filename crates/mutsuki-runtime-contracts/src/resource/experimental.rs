//! Experimental protocol descriptors for higher-level workflows.
//! These types are intentionally separated as an optional boundary; they are
//! kept out of the crate-root export surface by default and remain wire-compatible
//! for explicit `resource::experimental` imports.

use serde::{Deserialize, Serialize};

use super::core::{CommandPlan, WritePlan};

/// Experimental provider/workflow descriptor. CoreRuntime does not interpret
/// or execute transaction semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionPlan {
    pub plan_id: String,
    pub operations: Vec<WritePlan>,
    pub strict: bool,
}

/// Experimental provider/workflow descriptor. CoreRuntime does not interpret
/// or execute batch semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandBatch {
    pub batch_id: String,
    pub commands: Vec<CommandPlan>,
    pub rollback_guarantee: bool,
}

/// Experimental provider/workflow descriptor. CoreRuntime does not interpret
/// or execute saga semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SagaPlan {
    pub saga_id: String,
    pub steps: Vec<CommandPlan>,
    pub compensations: Vec<CommandPlan>,
}
