use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AgentRunRequest;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAffinity {
    None,
    Preferred(String),
    Required(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMigrationPolicy {
    Never,
    CheckpointOnly,
    ReconcileIdempotent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSideEffectClass {
    Pure,
    Idempotent,
    ApprovalBound,
    NonIdempotent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTaskPlacement {
    pub required_capabilities: Vec<String>,
    pub affinity: AgentAffinity,
    pub data_locality: Vec<String>,
    pub latency_class: String,
    pub cost_class: String,
    pub remote_execution_allowed: bool,
    pub migration: AgentMigrationPolicy,
    pub side_effect: AgentSideEffectClass,
    pub required_resource_refs: Vec<String>,
}

impl AgentTaskPlacement {
    pub fn permits_remote_execution(&self) -> bool {
        self.remote_execution_allowed
            && self.required_resource_refs.is_empty()
            && !matches!(
                self.side_effect,
                AgentSideEffectClass::ApprovalBound | AgentSideEffectClass::NonIdempotent
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorLease {
    pub session_id: String,
    pub node_id: String,
    pub epoch: u64,
    pub fencing_token: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRemoteTaskRequest {
    pub global_task_id: String,
    pub task_id: String,
    pub protocol_id: String,
    pub task_schema_id: String,
    pub task_schema_version: String,
    pub payload: Value,
    pub placement: AgentTaskPlacement,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRemoteSubagentRequest {
    pub parent_session_id: String,
    pub child_session_id: String,
    pub run: AgentRunRequest,
    pub placement: AgentTaskPlacement,
}
