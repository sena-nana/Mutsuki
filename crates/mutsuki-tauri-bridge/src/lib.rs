use mutsuki_runtime_contracts::{
    ResourceRef, RuntimeError, RuntimeEvent, ScalarValue, Task, TaskOutcome, TaskStatus, TraceSpan,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone, Debug, Error)]
pub enum BridgeError {
    #[error("{code}: {message}")]
    Frontend { code: String, message: String },
    #[error("event channel has no active receiver")]
    NoEventReceiver,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrontendError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl FrontendError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl From<RuntimeError> for FrontendError {
    fn from(error: RuntimeError) -> Self {
        let error = redact_runtime_error(error);
        Self {
            code: error.code,
            message: error.route,
            details: serde_json::to_value(error.evidence).ok(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontendContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webview_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_action_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrontendTaskRequest {
    pub protocol_id: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub input_refs: Vec<String>,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub context: FrontendContext,
}

impl FrontendTaskRequest {
    pub fn into_task(self) -> Task {
        let task_id = self
            .task_id
            .unwrap_or_else(|| format!("tauri-task:{}", Uuid::new_v4()));
        let mut task = Task::new(task_id, self.protocol_id, self.payload);
        task.trace_id = self.trace_id;
        task.correlation_id = self.correlation_id;
        task.idempotency_key = self.idempotency_key;
        task.input_refs = self.input_refs;
        task.priority = self.priority;
        task
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrontendTaskResult {
    pub task_id: String,
    pub status: Option<TaskStatus>,
    pub outcome: Option<TaskOutcome>,
    pub events: Vec<RuntimeEvent>,
    #[serde(default)]
    pub events_dropped: u64,
    #[serde(default)]
    pub events_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontendTaskRun {
    pub task_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResultRequest {
    pub task_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCancelRequest {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceBytes {
    pub resource: ResourceRef,
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceChunk {
    pub resource: ResourceRef,
    pub offset: u64,
    pub total_bytes: u64,
    pub bytes: Vec<u8>,
    pub eof: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceText {
    pub ref_id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewHandle {
    pub ref_id: String,
    pub token: String,
    pub url: String,
    pub expires_at_unix_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAttribution {
    pub trace_id: String,
    pub correlation_id: String,
    #[serde(default)]
    pub context: FrontendContext,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub token: String,
    pub requester: String,
    pub operation: String,
    pub risk: String,
    pub trace_id: String,
    pub correlation_id: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub context: FrontendContext,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub approval_id: String,
    pub token: String,
    pub decision: ApprovalDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<FrontendContext>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSummary {
    pub plugin_id: String,
    pub version: String,
    pub enabled: bool,
    pub deployment: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerSummary {
    pub runner_id: String,
    pub plugin_id: String,
    pub enabled: bool,
    pub deployment: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthComponent {
    pub healthy: bool,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealth {
    pub healthy: bool,
    pub status: String,
    pub active_tasks: usize,
    pub failed_tasks: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecentError {
    pub source: String,
    pub message: String,
    pub timestamp_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostStatus {
    pub app_name: String,
    pub profile_id: String,
    pub mode: String,
    pub healthy: bool,
    pub runtime: RuntimeHealth,
    pub host: HealthComponent,
    pub plugins_health: HealthComponent,
    pub runners_health: HealthComponent,
    pub recent_errors: Vec<HostRecentError>,
    pub plugins: Vec<PluginSummary>,
    pub runners: Vec<RunnerSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrontendLogRecord {
    pub level: String,
    pub target: String,
    pub message: String,
    pub timestamp_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeliveryProgress {
    pub request_id: String,
    pub target_app: String,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MutsukiFrontendEvent {
    Batch {
        events: Vec<MutsukiFrontendEvent>,
    },
    ObservabilityGap {
        stream: String,
        lost: u64,
        dropped: u64,
    },
    Task {
        task_id: String,
        event: RuntimeEvent,
    },
    Runtime {
        event: RuntimeEvent,
    },
    Trace {
        span: TraceSpan,
    },
    Log {
        record: FrontendLogRecord,
    },
    Resource {
        ref_id: String,
        operation: String,
    },
    Approval {
        request: ApprovalRequest,
    },
    Plugin {
        plugin: PluginSummary,
        operation: String,
    },
    Runner {
        runner_id: String,
        status: String,
    },
    AppDelivery {
        progress: DeliveryProgress,
    },
}

impl MutsukiFrontendEvent {
    pub fn channel(&self) -> &'static str {
        match self {
            Self::Batch { .. } => "mutsuki://event/batch",
            Self::ObservabilityGap { .. } => "mutsuki://observe/gap",
            Self::Task { .. } => "mutsuki://task/event",
            Self::Runtime { .. } => "mutsuki://runtime/event",
            Self::Trace { .. } => "mutsuki://trace/event",
            Self::Log { .. } => "mutsuki://log/event",
            Self::Resource { .. } => "mutsuki://resource/event",
            Self::Approval { .. } => "mutsuki://approval/event",
            Self::Plugin { .. } => "mutsuki://plugin/event",
            Self::Runner { .. } => "mutsuki://runner/event",
            Self::AppDelivery { .. } => "mutsuki://app_delivery/event",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrontendEventEnvelope {
    pub sequence: u64,
    pub channel: String,
    pub payload: MutsukiFrontendEvent,
}

#[derive(Debug)]
pub struct EventHub {
    sequence: AtomicU64,
    tx: broadcast::Sender<FrontendEventEnvelope>,
}

impl EventHub {
    pub fn new(buffer: usize) -> Self {
        let (tx, _) = broadcast::channel(buffer.max(1));
        Self {
            sequence: AtomicU64::new(0),
            tx,
        }
    }

    pub fn emit(
        &self,
        payload: MutsukiFrontendEvent,
    ) -> Result<FrontendEventEnvelope, BridgeError> {
        let envelope = FrontendEventEnvelope {
            sequence: self.sequence.fetch_add(1, Ordering::SeqCst) + 1,
            channel: payload.channel().to_string(),
            payload,
        };
        self.tx
            .send(envelope.clone())
            .map_err(|_| BridgeError::NoEventReceiver)?;
        Ok(envelope)
    }

    pub fn emit_batch(
        &self,
        events: Vec<MutsukiFrontendEvent>,
    ) -> Result<Option<FrontendEventEnvelope>, BridgeError> {
        match events.len() {
            0 => Ok(None),
            1 => self
                .emit(events.into_iter().next().expect("one event"))
                .map(Some),
            _ => self.emit(MutsukiFrontendEvent::Batch { events }).map(Some),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<FrontendEventEnvelope> {
        self.tx.subscribe()
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new(1024)
    }
}

pub fn redact_runtime_event(mut event: RuntimeEvent) -> RuntimeEvent {
    event.attributes = redact_scalar_fields(event.attributes);
    event.error = event.error.map(redact_runtime_error);
    event
}

pub fn redact_log_record(mut record: FrontendLogRecord) -> FrontendLogRecord {
    record.message = redact_message(record.message);
    record.fields = redact_json_fields(record.fields);
    record
}

fn redact_runtime_error(mut error: RuntimeError) -> RuntimeError {
    error.evidence = redact_scalar_fields(error.evidence);
    error.cause = error
        .cause
        .map(|cause| Box::new(redact_runtime_error(*cause)));
    error
}

fn redact_scalar_fields(fields: BTreeMap<String, ScalarValue>) -> BTreeMap<String, ScalarValue> {
    fields
        .into_iter()
        .map(|(key, value)| {
            if is_sensitive_key(&key) {
                (key, ScalarValue::String("[redacted]".into()))
            } else {
                (key, value)
            }
        })
        .collect()
}

fn redact_json_fields(fields: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    fields
        .into_iter()
        .map(|(key, value)| {
            if is_sensitive_key(&key) {
                (key, Value::String("[redacted]".into()))
            } else {
                (key, redact_json_value(value))
            }
        })
        .collect()
}

fn redact_json_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(redact_json_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) {
                        (key, Value::String("[redacted]".into()))
                    } else {
                        (key, redact_json_value(value))
                    }
                })
                .collect(),
        ),
        value => value,
    }
}

fn redact_message(message: String) -> String {
    let lower = message.to_ascii_lowercase();
    let sensitive_fragment = [
        "authorization",
        "bearer ",
        "token=",
        "token:",
        "secret=",
        "secret:",
        "password=",
        "password:",
        "api_key=",
        "api_key:",
        "credential=",
        "credential:",
    ]
    .iter()
    .any(|fragment| lower.contains(fragment));
    if sensitive_fragment {
        "[redacted]".into()
    } else {
        message
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "credential",
        "api_key",
        "apikey",
        "authorization",
    ]
    .iter()
    .any(|fragment| key.contains(fragment))
}

#[cfg(test)]
mod tests {
    use super::{EventHub, MutsukiFrontendEvent};

    #[test]
    fn emit_batch_uses_one_bounded_hub_message_for_multiple_frontend_events() {
        let hub = EventHub::new(4);
        let mut receiver = hub.subscribe();
        let payloads = (0..3)
            .map(|index| MutsukiFrontendEvent::Runner {
                runner_id: format!("runner:{index}"),
                status: "running".into(),
            })
            .collect();

        hub.emit_batch(payloads).expect("batch emits");

        let envelope = receiver.try_recv().expect("one batch envelope");
        assert!(matches!(
            envelope.payload,
            MutsukiFrontendEvent::Batch { events } if events.len() == 3
        ));
        assert!(receiver.try_recv().is_err());
    }
}
