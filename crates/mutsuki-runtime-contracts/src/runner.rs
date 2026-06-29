use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ExecutorId, ProtocolId, RefId, ResourceRef, ScalarValue, Task, TaskAwait, TaskId, TaskLeaseId,
    ValueRef,
};
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
pub struct RunnerContext {
    pub registry_generation: u64,
    pub current_step: u64,
    pub executor_id: ExecutorId,
    pub task_lease_id: Option<TaskLeaseId>,
    pub invocation_id: String,
    pub cancel_token: String,
    pub deadline_tick: Option<u64>,
    pub cancel_requested: bool,
}

impl RunnerContext {
    pub fn new(
        registry_generation: u64,
        current_step: u64,
        executor_id: impl Into<ExecutorId>,
        task_lease_id: Option<TaskLeaseId>,
        invocation_id: impl Into<String>,
    ) -> Self {
        let invocation_id = invocation_id.into();
        Self {
            registry_generation,
            current_step,
            executor_id: executor_id.into(),
            task_lease_id,
            cancel_token: invocation_id.clone(),
            invocation_id,
            deadline_tick: None,
            cancel_requested: false,
        }
    }
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
