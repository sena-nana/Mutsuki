use std::ops::Deref;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BindingId, DispatchLane, ExecutorId, OrderingRequirement, ProtocolId, RefId, ResourceRef,
    ResourceRequirement, RunnerId, RuntimeError, SurfaceId, TaskId,
};
use crate::{TaskLeaseId, TraceId};

/// In-process shared JSON payload.
///
/// Wire serialization remains a plain JSON value. Cloning shares the underlying
/// `Arc` so builtin facade derivation can rebind protocol metadata without a
/// deep copy of large request bodies.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskPayload {
    inner: Arc<Value>,
}

impl Serialize for TaskPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.inner.as_ref().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TaskPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self::new)
    }
}

impl TaskPayload {
    pub fn new(value: Value) -> Self {
        Self {
            inner: Arc::new(value),
        }
    }

    pub fn shared(inner: Arc<Value>) -> Self {
        Self { inner }
    }

    pub fn as_value(&self) -> &Value {
        &self.inner
    }

    pub fn arc(&self) -> Arc<Value> {
        Arc::clone(&self.inner)
    }

    pub fn to_value(&self) -> Value {
        (*self.inner).clone()
    }

    pub fn into_value(self) -> Value {
        match Arc::try_unwrap(self.inner) {
            Ok(value) => value,
            Err(shared) => (*shared).clone(),
        }
    }

    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }
}

impl Deref for TaskPayload {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl AsRef<Value> for TaskPayload {
    fn as_ref(&self) -> &Value {
        &self.inner
    }
}

impl From<Value> for TaskPayload {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

impl From<Arc<Value>> for TaskPayload {
    fn from(inner: Arc<Value>) -> Self {
        Self::shared(inner)
    }
}

impl From<TaskPayload> for Value {
    fn from(payload: TaskPayload) -> Self {
        payload.into_value()
    }
}

impl PartialEq<Value> for TaskPayload {
    fn eq(&self, other: &Value) -> bool {
        self.inner.as_ref() == other
    }
}

impl PartialEq<TaskPayload> for Value {
    fn eq(&self, other: &TaskPayload) -> bool {
        self == other.inner.as_ref()
    }
}

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
    pub payload: TaskPayload,
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
    pub dispatch_lane: DispatchLane,
    pub ordering: OrderingRequirement,
    pub resource_requirements: Vec<ResourceRequirement>,
    pub created_sequence: u64,
}

impl Task {
    pub fn new(
        task_id: impl Into<String>,
        protocol_id: impl Into<String>,
        payload: impl Into<TaskPayload>,
    ) -> Self {
        let protocol_id = protocol_id.into();
        Self {
            task_id: task_id.into(),
            protocol_id,
            priority: 0,
            ready_at_step: None,
            payload: payload.into(),
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
            dispatch_lane: DispatchLane::Normal,
            ordering: OrderingRequirement::None,
            resource_requirements: Vec::new(),
            created_sequence: 0,
        }
    }

    /// Rebinds protocol metadata while sharing the source payload `Arc`.
    pub fn derive_with_protocol(
        &self,
        task_id: impl Into<String>,
        protocol_id: impl Into<String>,
    ) -> Self {
        let mut derived = Self::new(task_id, protocol_id, self.payload.clone());
        derived.priority = self.priority;
        derived.trace_id = self.trace_id.clone();
        derived.correlation_id = self
            .correlation_id
            .clone()
            .or_else(|| Some(self.task_id.clone()));
        derived.idempotency_key = self.idempotency_key.clone();
        derived.input_refs = self.input_refs.clone();
        derived.expected_versions = self.expected_versions.clone();
        derived.required_surfaces = self.required_surfaces.clone();
        derived.dispatch_lane = self.dispatch_lane.clone();
        derived.ordering = self.ordering.clone();
        derived.resource_requirements = self.resource_requirements.clone();
        derived.registry_generation = self.registry_generation;
        derived
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLease {
    pub lease_id: TaskLeaseId,
    pub task_id: TaskId,
    #[serde(default)]
    pub attempt_generation: u64,
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
        #[serde(default)]
        output: Option<Value>,
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
