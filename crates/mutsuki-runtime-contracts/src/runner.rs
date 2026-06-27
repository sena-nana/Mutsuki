use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ProtocolId, RefId, ResourceRef, ScalarValue, Task, TaskAwait, TaskId, ValueRef};
use crate::{StateDelta, SurfaceId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionClass {
    Control,
    Orchestration,
    Io,
    Cpu,
    Blocking,
    Script,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerPurity {
    Pure,
    Committer,
    Effectful,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunnerDescriptor {
    pub runner_id: String,
    pub plugin_id: String,
    pub plugin_generation: u64,
    pub accepted_protocol_ids: Vec<ProtocolId>,
    pub purity: RunnerPurity,
    pub execution_class: ExecutionClass,
    pub input_schema: Value,
    pub output_schema: Value,
    pub metadata: BTreeMap<String, ScalarValue>,
    pub contract_surfaces: Vec<SurfaceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerStatus {
    Completed,
    Waiting,
    Blocked,
    Continue,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainEvent {
    pub event_id: String,
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectPrecondition {
    pub ref_id: RefId,
    pub expected_version: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectRequest {
    pub effect_id: String,
    pub kind: String,
    pub payload: Value,
    pub preconditions: Vec<EffectPrecondition>,
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunnerResult {
    pub task_id: TaskId,
    pub deltas: Vec<StateDelta>,
    pub events: Vec<DomainEvent>,
    pub tasks: Vec<Task>,
    pub effects: Vec<EffectRequest>,
    pub values: Vec<ValueRef>,
    pub resources: Vec<ResourceRef>,
    pub task_await: Option<TaskAwait>,
    pub status: RunnerStatus,
}

impl RunnerResult {
    pub fn completed(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            deltas: Vec::new(),
            events: Vec::new(),
            tasks: Vec::new(),
            effects: Vec::new(),
            values: Vec::new(),
            resources: Vec::new(),
            task_await: None,
            status: RunnerStatus::Completed,
        }
    }
}
