use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BatchId, ExecutorId, OrderingRequirement, PayloadLayout, ProtocolId, RefId, ResourceRef,
    ScalarValue, Task, TaskAwait, TaskId, TaskLeaseId, TickId, ValueRef,
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
    pub batch: RunnerBatchCapability,
    pub payload: RunnerPayloadCapability,
    pub resources: RunnerResourceCapability,
    pub ordering: RunnerOrderingCapability,
    pub control: RunnerControlCapability,
    pub metadata: BTreeMap<String, ScalarValue>,
    pub contract_surfaces: Vec<SurfaceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerMode {
    ScalarAdapter,
    NativeBatch,
    Batch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerBatchCapability {
    pub mode: RunnerMode,
    pub preferred_batch_size: usize,
    pub max_batch_entries: usize,
    pub max_inflight_batches: usize,
    pub partial_failure: bool,
    pub preserve_order: bool,
}

impl Default for RunnerBatchCapability {
    fn default() -> Self {
        Self {
            mode: RunnerMode::ScalarAdapter,
            preferred_batch_size: 1,
            max_batch_entries: 1,
            max_inflight_batches: 1,
            partial_failure: true,
            preserve_order: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerPayloadCapability {
    pub layouts: Vec<PayloadLayout>,
    pub preferred_layout: PayloadLayout,
    pub zero_copy: bool,
}

impl Default for RunnerPayloadCapability {
    fn default() -> Self {
        Self {
            layouts: vec![PayloadLayout::Row],
            preferred_layout: PayloadLayout::Row,
            zero_copy: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerResourceCapability {
    pub batch_read: bool,
    pub batch_write: bool,
    pub requires_resource_plan: bool,
    pub supports_shared_memory: bool,
}

impl Default for RunnerResourceCapability {
    fn default() -> Self {
        Self {
            batch_read: true,
            batch_write: true,
            requires_resource_plan: true,
            supports_shared_memory: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerOrderingCapability {
    pub default: OrderingRequirement,
    pub supports_sequence: bool,
    pub supports_same_resource_order: bool,
}

impl Default for RunnerOrderingCapability {
    fn default() -> Self {
        Self {
            default: OrderingRequirement::None,
            supports_sequence: true,
            supports_same_resource_order: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutGranularity {
    Batch,
    Entry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerControlCapability {
    pub entry_cancel: bool,
    pub batch_cancel: bool,
    pub timeout_granularity: TimeoutGranularity,
}

impl Default for RunnerControlCapability {
    fn default() -> Self {
        Self {
            entry_cancel: true,
            batch_cancel: true,
            timeout_granularity: TimeoutGranularity::Entry,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerContext {
    pub registry_generation: u64,
    pub current_step: u64,
    pub tick_id: TickId,
    pub batch_id: BatchId,
    pub executor_id: ExecutorId,
    pub task_lease_ids: Vec<TaskLeaseId>,
    pub entry_count: usize,
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
        task_lease_ids: impl IntoTaskLeaseIds,
        invocation_id: impl Into<String>,
    ) -> Self {
        let invocation_id = invocation_id.into();
        let task_lease_ids = task_lease_ids.into_task_lease_ids();
        let entry_count = task_lease_ids.len();
        Self {
            registry_generation,
            current_step,
            tick_id: format!("tick-{current_step}"),
            batch_id: invocation_id.clone(),
            executor_id: executor_id.into(),
            task_lease_ids,
            entry_count,
            cancel_token: invocation_id.clone(),
            invocation_id,
            deadline_tick: None,
            cancel_requested: false,
        }
    }

    pub fn with_batch(mut self, batch_id: impl Into<BatchId>, entry_count: usize) -> Self {
        self.batch_id = batch_id.into();
        self.entry_count = entry_count;
        self
    }
}

pub trait IntoTaskLeaseIds {
    fn into_task_lease_ids(self) -> Vec<TaskLeaseId>;
}

impl IntoTaskLeaseIds for Vec<TaskLeaseId> {
    fn into_task_lease_ids(self) -> Vec<TaskLeaseId> {
        self
    }
}

impl IntoTaskLeaseIds for Option<TaskLeaseId> {
    fn into_task_lease_ids(self) -> Vec<TaskLeaseId> {
        self.into_iter().collect()
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
