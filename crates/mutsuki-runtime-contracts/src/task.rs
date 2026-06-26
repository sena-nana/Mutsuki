use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{RefId, SurfaceId, TaskId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Ready,
    Running,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Cancelled,
    Expired,
    DeadLetter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionExpectation {
    pub ref_id: RefId,
    pub expected_version: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub task_id: TaskId,
    pub kind: String,
    pub priority: i64,
    pub ready_at_step: Option<u64>,
    pub payload: Value,
    pub input_refs: Vec<RefId>,
    pub expected_versions: Vec<VersionExpectation>,
    pub correlation_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub runner_hint: Option<String>,
    pub registry_generation: u64,
    pub required_surfaces: Vec<SurfaceId>,
    pub created_sequence: u64,
}

impl Task {
    pub fn new(task_id: impl Into<String>, kind: impl Into<String>, payload: Value) -> Self {
        Self {
            task_id: task_id.into(),
            kind: kind.into(),
            priority: 0,
            ready_at_step: None,
            payload,
            input_refs: Vec::new(),
            expected_versions: Vec::new(),
            correlation_id: None,
            idempotency_key: None,
            runner_hint: None,
            registry_generation: 0,
            required_surfaces: Vec::new(),
            created_sequence: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    Retry,
    Merge,
    Discard,
    Fail,
    EmitConflictTask,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateRef {
    pub ref_id: RefId,
    pub schema: String,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateDelta {
    pub target_ref: RefId,
    pub expected_version: u64,
    pub patch: Value,
    pub conflict_policy: ConflictPolicy,
}
