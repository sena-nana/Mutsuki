use std::any::{Any, TypeId};
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BindingId, DispatchLane, ExecutorId, OrderingRequirement, ProtocolId, RefId, ResourceRef,
    ResourceRequirement, RunnerId, RuntimeError, SurfaceId, TaskId,
};
use crate::{TaskLeaseId, TraceId};

type LocalJsonFn = fn(&(dyn Any + Send + Sync)) -> Result<Value, serde_json::Error>;

/// In-process task payload.
///
/// Hot paths prefer typed local values shared by `Arc`. Wire serialization remains
/// a plain JSON value; local payloads materialize JSON lazily only when required
/// for interoperability or control-plane views.
#[derive(Clone)]
pub struct TaskPayload {
    kind: TaskPayloadKind,
}

#[derive(Clone)]
enum TaskPayloadKind {
    Json(Arc<Value>),
    Local(Arc<LocalPayload>),
}

struct LocalPayload {
    body: Arc<dyn Any + Send + Sync>,
    type_id: TypeId,
    type_name: &'static str,
    to_json: LocalJsonFn,
    json: OnceLock<Arc<Value>>,
}

impl LocalPayload {
    fn ensure_json(&self) -> &Value {
        self.json
            .get_or_init(|| {
                Arc::new((self.to_json)(self.body.as_ref()).unwrap_or_else(|error| {
                    panic!(
                        "local task payload `{}` must serialize for wire form: {error}",
                        self.type_name
                    )
                }))
            })
            .as_ref()
    }

    fn json_arc(&self) -> Arc<Value> {
        Arc::clone(self.json.get_or_init(|| {
            Arc::new((self.to_json)(self.body.as_ref()).unwrap_or_else(|error| {
                panic!(
                    "local task payload `{}` must serialize for wire form: {error}",
                    self.type_name
                )
            }))
        }))
    }
}

impl fmt::Debug for TaskPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TaskPayloadKind::Json(value) => formatter
                .debug_struct("TaskPayload")
                .field("kind", &"json")
                .field("value", value)
                .finish(),
            TaskPayloadKind::Local(local) => formatter
                .debug_struct("TaskPayload")
                .field("kind", &"local")
                .field("type_name", &local.type_name)
                .field("json_cached", &local.json.get().is_some())
                .finish(),
        }
    }
}

impl PartialEq for TaskPayload {
    fn eq(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (TaskPayloadKind::Json(left), TaskPayloadKind::Json(right)) => left == right,
            (TaskPayloadKind::Local(left), TaskPayloadKind::Local(right)) => {
                Arc::ptr_eq(left, right)
                    || Arc::ptr_eq(&left.body, &right.body)
                    || left.ensure_json() == right.ensure_json()
            }
            _ => self.as_value() == other.as_value(),
        }
    }
}

impl Serialize for TaskPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_value().serialize(serializer)
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
            kind: TaskPayloadKind::Json(Arc::new(value)),
        }
    }

    pub fn shared(inner: Arc<Value>) -> Self {
        Self {
            kind: TaskPayloadKind::Json(inner),
        }
    }

    /// Stores an owned local value shared by `Arc` for in-process hot paths.
    pub fn from_local<T>(value: T) -> Self
    where
        T: Any + Send + Sync + Serialize + 'static,
    {
        Self::share_local(Arc::new(value))
    }

    /// Shares an existing local `Arc` without copying the typed body.
    pub fn share_local<T>(value: Arc<T>) -> Self
    where
        T: Any + Send + Sync + Serialize + 'static,
    {
        let local = LocalPayload {
            body: value,
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            to_json: local_to_json::<T>,
            json: OnceLock::new(),
        };
        Self {
            kind: TaskPayloadKind::Local(Arc::new(local)),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self.kind, TaskPayloadKind::Local(_))
    }

    pub fn is_json(&self) -> bool {
        matches!(self.kind, TaskPayloadKind::Json(_))
    }

    pub fn local_type_name(&self) -> Option<&'static str> {
        match &self.kind {
            TaskPayloadKind::Local(local) => Some(local.type_name),
            TaskPayloadKind::Json(_) => None,
        }
    }

    pub fn local_ref<T: Any>(&self) -> Option<&T> {
        match &self.kind {
            TaskPayloadKind::Local(local) => local.body.downcast_ref::<T>(),
            TaskPayloadKind::Json(_) => None,
        }
    }

    pub fn as_local<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        match &self.kind {
            TaskPayloadKind::Local(local) if local.type_id == TypeId::of::<T>() => {
                Arc::downcast(Arc::clone(&local.body)).ok()
            }
            _ => None,
        }
    }

    /// Prefer typed local data; fall back to JSON without cloning the Value tree.
    pub fn decode<T>(&self) -> Result<T, serde_json::Error>
    where
        T: Any + Clone + DeserializeOwned,
    {
        if let Some(local) = self.local_ref::<T>() {
            return Ok(local.clone());
        }
        T::deserialize(self.as_value())
    }

    /// Prefer a shared local `Arc`; JSON payloads are decoded once into a fresh `Arc`.
    pub fn decode_shared<T>(&self) -> Result<Arc<T>, serde_json::Error>
    where
        T: Any + Send + Sync + Clone + DeserializeOwned + 'static,
    {
        if let Some(local) = self.as_local::<T>() {
            return Ok(local);
        }
        Ok(Arc::new(T::deserialize(self.as_value())?))
    }

    pub fn as_value(&self) -> &Value {
        match &self.kind {
            TaskPayloadKind::Json(value) => value,
            TaskPayloadKind::Local(local) => local.ensure_json(),
        }
    }

    pub fn arc(&self) -> Arc<Value> {
        match &self.kind {
            TaskPayloadKind::Json(value) => Arc::clone(value),
            TaskPayloadKind::Local(local) => local.json_arc(),
        }
    }

    pub fn to_value(&self) -> Value {
        self.as_value().clone()
    }

    pub fn into_value(self) -> Value {
        match self.kind {
            TaskPayloadKind::Json(value) => match Arc::try_unwrap(value) {
                Ok(value) => value,
                Err(shared) => (*shared).clone(),
            },
            TaskPayloadKind::Local(local) => match Arc::try_unwrap(local) {
                Ok(local) => match local.json.into_inner() {
                    Some(json) => match Arc::try_unwrap(json) {
                        Ok(value) => value,
                        Err(shared) => (*shared).clone(),
                    },
                    None => (local.to_json)(local.body.as_ref())
                        .expect("local task payload must serialize for wire form"),
                },
                Err(shared) => shared.ensure_json().clone(),
            },
        }
    }

    pub fn strong_count(&self) -> usize {
        match &self.kind {
            TaskPayloadKind::Json(value) => Arc::strong_count(value),
            TaskPayloadKind::Local(local) => Arc::strong_count(local),
        }
    }
}

fn local_to_json<T>(any: &(dyn Any + Send + Sync)) -> Result<Value, serde_json::Error>
where
    T: Any + Serialize + 'static,
{
    let value = any
        .downcast_ref::<T>()
        .expect("local payload type id mismatch");
    serde_json::to_value(value)
}

impl Deref for TaskPayload {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        self.as_value()
    }
}

impl AsRef<Value> for TaskPayload {
    fn as_ref(&self) -> &Value {
        self.as_value()
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
        self.as_value() == other
    }
}

impl PartialEq<TaskPayload> for Value {
    fn eq(&self, other: &TaskPayload) -> bool {
        self == other.as_value()
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
