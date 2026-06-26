use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BindingId, ExecutorId, ProtocolId, RefId, ResourceRef, RunnerId, RuntimeError, SurfaceId,
    TaskId,
};
use crate::{TaskLeaseId, TraceId};

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
    pub protocol_id: ProtocolId,
    pub priority: i64,
    pub ready_at_step: Option<u64>,
    pub payload: Value,
    pub input_refs: Vec<RefId>,
    pub output_ref: Option<RefId>,
    pub continuation_ref: Option<RefId>,
    pub target_binding_id: Option<BindingId>,
    pub lease_id: Option<TaskLeaseId>,
    pub trace_id: Option<TraceId>,
    pub expected_versions: Vec<VersionExpectation>,
    pub correlation_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub runner_hint: Option<String>,
    pub registry_generation: u64,
    pub required_surfaces: Vec<SurfaceId>,
    pub created_sequence: u64,
}

impl Task {
    pub fn new(task_id: impl Into<String>, protocol_id: impl Into<String>, payload: Value) -> Self {
        let protocol_id = protocol_id.into();
        Self {
            task_id: task_id.into(),
            protocol_id,
            priority: 0,
            ready_at_step: None,
            payload,
            input_refs: Vec::new(),
            output_ref: None,
            continuation_ref: None,
            target_binding_id: None,
            lease_id: None,
            trace_id: None,
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
pub struct TaskLease {
    pub lease_id: TaskLeaseId,
    pub task_id: TaskId,
    pub runner_id: RunnerId,
    pub executor_id: ExecutorId,
    pub registry_generation: u64,
    pub acquired_at_step: u64,
    pub expires_at_step: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelPolicy {
    Cascade,
    Detach,
    Shield,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskHandle {
    pub task_id: TaskId,
    pub protocol_id: ProtocolId,
    pub target_binding_id: Option<BindingId>,
    pub cancel_policy: CancelPolicy,
    pub trace_id: Option<TraceId>,
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskOutcome {
    Completed {
        task_id: TaskId,
        output_ref: Option<RefId>,
    },
    Failed {
        task_id: TaskId,
        error: RuntimeError,
    },
    Cancelled {
        task_id: TaskId,
        reason: Option<String>,
    },
    Expired {
        task_id: TaskId,
        reason: Option<String>,
    },
    DeadLetter {
        task_id: TaskId,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WakeCondition {
    Timer { ready_at_step: u64 },
    RetryAfter { ready_at_step: u64 },
    ResourceEvent { ref_id: RefId },
    ExternalSignal { signal_id: String },
    ManualWake,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskStepContinuation {
    pub continuation: ResourceRef,
    pub wake: Option<WakeCondition>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskAwait {
    pub parent_task_id: TaskId,
    pub child: TaskHandle,
    pub continuation: TaskStepContinuation,
    pub cancel_policy: CancelPolicy,
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
