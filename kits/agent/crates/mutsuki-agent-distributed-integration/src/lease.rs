use mutsuki_agent_contracts::{AgentRunRequest, AgentTaskPlacement};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Cluster coordinator fencing record. Lives only on the distributed integration
/// surface so default Agent contracts stay node-neutral.
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
