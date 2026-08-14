use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use mutsuki_runtime_contracts::{RuntimeEvent, ScalarValue, TaskBatch, TaskHandle};

pub type ControlFuture = Pin<Box<dyn Future<Output = ControlResponse> + Send>>;

pub trait ControlHandler: Send + Sync + 'static {
    fn handle(&self, request: ControlRequest) -> ControlFuture;
}

/// Authoritative Service control-plane projections invalidated by a committed owner change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlChangeDomain {
    Tasks,
    Runners,
    EventSources,
    Plugins,
    Logs,
}

/// Lightweight invalidation event. Consumers must read the existing control RPC for the snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlChangeEvent {
    pub revision: u64,
    pub domains: Vec<ControlChangeDomain>,
}

/// Bounded, cancellable subscription to control-plane invalidations.
///
/// A lagged consumer receives the newest available event and must perform a full snapshot read;
/// no business state is carried by this channel.
pub struct ControlChangeSubscription {
    receiver: broadcast::Receiver<ControlChangeEvent>,
}

impl ControlChangeSubscription {
    pub fn new(receiver: broadcast::Receiver<ControlChangeEvent>) -> Self {
        Self { receiver }
    }

    pub async fn changed(&mut self) -> Option<ControlChangeEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlRequest {
    pub token: String,
    pub command: ControlCommand,
}

impl ControlRequest {
    pub fn new(token: impl Into<String>, command: ControlCommand) -> Self {
        Self {
            token: token.into(),
            command,
        }
    }

    pub const fn method(&self) -> ControlMethod {
        self.command.method()
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ControlMethod {
    ServiceStatus = 0x0001,
    ServiceShutdown = 0x0002,
    CoreStatus = 0x0003,
    PluginList = 0x0004,
    PluginReload = 0x0005,
    PluginDeploymentSet = 0x0006,
    PluginDeploymentClear = 0x0007,
    RunnerList = 0x0008,
    RunnerRestart = 0x0009,
    RunnerStop = 0x000A,
    EventSourceList = 0x000B,
    EventSourceRestart = 0x000C,
    CoreBeginDrain = 0x000D,
    TaskSubmitBatch = 0x000E,
    TaskList = 0x000F,
    TaskCancel = 0x0010,
    TaskOutcome = 0x0011,
    TaskEventsAfter = 0x0012,
    HealthCheck = 0x0013,
    LogTail = 0x0014,
    TaskOutcomesBatch = 0x0015,
    TaskWait = 0x0016,
    RuntimeStatistics = 0x0017,
    /// Host process metrics (pid / RSS / CPU) for the console dashboard.
    HostMetrics = 0x0018,
}

impl ControlMethod {
    pub const fn opcode(self) -> u16 {
        self as u16
    }

    pub fn from_opcode(opcode: u16) -> Option<Self> {
        Some(match opcode {
            0x0001 => Self::ServiceStatus,
            0x0002 => Self::ServiceShutdown,
            0x0003 => Self::CoreStatus,
            0x0004 => Self::PluginList,
            0x0005 => Self::PluginReload,
            0x0006 => Self::PluginDeploymentSet,
            0x0007 => Self::PluginDeploymentClear,
            0x0008 => Self::RunnerList,
            0x0009 => Self::RunnerRestart,
            0x000A => Self::RunnerStop,
            0x000B => Self::EventSourceList,
            0x000C => Self::EventSourceRestart,
            0x000D => Self::CoreBeginDrain,
            0x000E => Self::TaskSubmitBatch,
            0x000F => Self::TaskList,
            0x0010 => Self::TaskCancel,
            0x0011 => Self::TaskOutcome,
            0x0012 => Self::TaskEventsAfter,
            0x0013 => Self::HealthCheck,
            0x0014 => Self::LogTail,
            0x0015 => Self::TaskOutcomesBatch,
            0x0016 => Self::TaskWait,
            0x0017 => Self::RuntimeStatistics,
            0x0018 => Self::HostMetrics,
            _ => return None,
        })
    }

    /// Mutating ops stay ordered on a connection under multiplex.
    pub const fn is_mutating(self) -> bool {
        matches!(
            self,
            Self::ServiceShutdown
                | Self::PluginReload
                | Self::PluginDeploymentSet
                | Self::PluginDeploymentClear
                | Self::RunnerRestart
                | Self::RunnerStop
                | Self::EventSourceRestart
                | Self::CoreBeginDrain
                | Self::TaskSubmitBatch
                | Self::TaskCancel
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", content = "request", rename_all = "snake_case")]
pub enum ControlCommand {
    ServiceStatus,
    ServiceShutdown,
    CoreStatus,
    PluginList,
    PluginReload,
    PluginDeploymentSet(PluginDeploymentParam),
    PluginDeploymentClear(PluginDeploymentClearParam),
    RunnerList,
    RunnerRestart(IdParam),
    RunnerStop(IdParam),
    EventSourceList,
    EventSourceRestart(IdParam),
    CoreBeginDrain,
    TaskSubmitBatch(TaskSubmitBatchParam),
    TaskList,
    TaskCancel(IdParam),
    TaskOutcome(IdParam),
    TaskEventsAfter(TaskEventsAfterParam),
    HealthCheck,
    LogTail(LogTailParams),
    TaskOutcomesBatch(TaskOutcomesBatchParam),
    TaskWait(TaskWaitParam),
    RuntimeStatistics,
    HostMetrics,
}

impl ControlCommand {
    pub const fn method(&self) -> ControlMethod {
        match self {
            Self::ServiceStatus => ControlMethod::ServiceStatus,
            Self::ServiceShutdown => ControlMethod::ServiceShutdown,
            Self::CoreStatus => ControlMethod::CoreStatus,
            Self::PluginList => ControlMethod::PluginList,
            Self::PluginReload => ControlMethod::PluginReload,
            Self::PluginDeploymentSet(_) => ControlMethod::PluginDeploymentSet,
            Self::PluginDeploymentClear(_) => ControlMethod::PluginDeploymentClear,
            Self::RunnerList => ControlMethod::RunnerList,
            Self::RunnerRestart(_) => ControlMethod::RunnerRestart,
            Self::RunnerStop(_) => ControlMethod::RunnerStop,
            Self::EventSourceList => ControlMethod::EventSourceList,
            Self::EventSourceRestart(_) => ControlMethod::EventSourceRestart,
            Self::CoreBeginDrain => ControlMethod::CoreBeginDrain,
            Self::TaskSubmitBatch(_) => ControlMethod::TaskSubmitBatch,
            Self::TaskList => ControlMethod::TaskList,
            Self::TaskCancel(_) => ControlMethod::TaskCancel,
            Self::TaskOutcome(_) => ControlMethod::TaskOutcome,
            Self::TaskEventsAfter(_) => ControlMethod::TaskEventsAfter,
            Self::HealthCheck => ControlMethod::HealthCheck,
            Self::LogTail(_) => ControlMethod::LogTail,
            Self::TaskOutcomesBatch(_) => ControlMethod::TaskOutcomesBatch,
            Self::TaskWait(_) => ControlMethod::TaskWait,
            Self::RuntimeStatistics => ControlMethod::RuntimeStatistics,
            Self::HostMetrics => ControlMethod::HostMetrics,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", content = "response", rename_all = "snake_case")]
pub enum ControlResult {
    ServiceStatus(ServiceStatus),
    ServiceShutdown,
    CoreStatus(CoreStatus),
    PluginList(PluginListResponse),
    PluginReload(PluginReloadResponse),
    PluginDeploymentSet(PluginReloadResponse),
    PluginDeploymentClear(PluginReloadResponse),
    RunnerList(Vec<RunnerStatus>),
    RunnerRestart,
    RunnerStop,
    EventSourceList(Vec<EventSourceStatus>),
    EventSourceRestart,
    CoreBeginDrain(CoreDrainResponse),
    TaskSubmitBatch(TaskSubmitBatchResponse),
    TaskList(Vec<TaskSnapshot>),
    TaskCancel,
    TaskOutcome(TaskOutcomeView),
    TaskEventsAfter(TaskEventPage),
    HealthCheck(HealthReport),
    LogTail(LogTailResponse),
    TaskOutcomesBatch(TaskOutcomesBatchResponse),
    TaskWait(TaskWaitResponse),
    RuntimeStatistics(RuntimeStatisticsView),
    HostMetrics(HostMetrics),
}

impl ControlResult {
    pub const fn method(&self) -> ControlMethod {
        match self {
            Self::ServiceStatus(_) => ControlMethod::ServiceStatus,
            Self::ServiceShutdown => ControlMethod::ServiceShutdown,
            Self::CoreStatus(_) => ControlMethod::CoreStatus,
            Self::PluginList(_) => ControlMethod::PluginList,
            Self::PluginReload(_) => ControlMethod::PluginReload,
            Self::PluginDeploymentSet(_) => ControlMethod::PluginDeploymentSet,
            Self::PluginDeploymentClear(_) => ControlMethod::PluginDeploymentClear,
            Self::RunnerList(_) => ControlMethod::RunnerList,
            Self::RunnerRestart => ControlMethod::RunnerRestart,
            Self::RunnerStop => ControlMethod::RunnerStop,
            Self::EventSourceList(_) => ControlMethod::EventSourceList,
            Self::EventSourceRestart => ControlMethod::EventSourceRestart,
            Self::CoreBeginDrain(_) => ControlMethod::CoreBeginDrain,
            Self::TaskSubmitBatch(_) => ControlMethod::TaskSubmitBatch,
            Self::TaskList(_) => ControlMethod::TaskList,
            Self::TaskCancel => ControlMethod::TaskCancel,
            Self::TaskOutcome(_) => ControlMethod::TaskOutcome,
            Self::TaskEventsAfter(_) => ControlMethod::TaskEventsAfter,
            Self::HealthCheck(_) => ControlMethod::HealthCheck,
            Self::LogTail(_) => ControlMethod::LogTail,
            Self::TaskOutcomesBatch(_) => ControlMethod::TaskOutcomesBatch,
            Self::TaskWait(_) => ControlMethod::TaskWait,
            Self::RuntimeStatistics(_) => ControlMethod::RuntimeStatistics,
            Self::HostMetrics(_) => ControlMethod::HostMetrics,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "payload", rename_all = "snake_case")]
pub enum ControlResponse {
    Ok(ControlResult),
    Error(ControlErrorBody),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    Unauthorized,
    Unsupported,
    BadRequest,
    CoreUnavailable,
    Failed,
    DeliveryUnknown,
}

impl ControlErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Unsupported => "unsupported",
            Self::BadRequest => "bad_request",
            Self::CoreUnavailable => "core_unavailable",
            Self::Failed => "failed",
            Self::DeliveryUnknown => "delivery_unknown",
        }
    }
}

impl std::fmt::Display for ControlErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlErrorBody {
    pub code: ControlErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, ScalarValue>,
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("unauthorized control request")]
    Unauthorized,
    #[error("unsupported control method: {0}")]
    Unsupported(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("core is not running")]
    CoreUnavailable,
    #[error("operation failed: {0}")]
    Failed(String),
}

impl ControlResponse {
    pub fn ok(result: ControlResult) -> Self {
        Self::Ok(result)
    }

    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }

    pub fn into_result(self) -> Result<ControlResult, ControlErrorBody> {
        match self {
            Self::Ok(result) => Ok(result),
            Self::Error(error) => Err(error),
        }
    }

    pub fn result(&self) -> Option<&ControlResult> {
        match self {
            Self::Ok(result) => Some(result),
            Self::Error(_) => None,
        }
    }

    pub fn error(&self) -> Option<&ControlErrorBody> {
        match self {
            Self::Ok(_) => None,
            Self::Error(error) => Some(error),
        }
    }

    pub fn err(error: ControlError) -> Self {
        let (code, message) = match error {
            ControlError::Unauthorized => (ControlErrorCode::Unauthorized, error.to_string()),
            ControlError::Unsupported(method) => (
                ControlErrorCode::Unsupported,
                format!("{method} is not supported by the current runtime API"),
            ),
            ControlError::BadRequest(message) => (ControlErrorCode::BadRequest, message),
            ControlError::CoreUnavailable => (
                ControlErrorCode::CoreUnavailable,
                "core is not running".into(),
            ),
            ControlError::Failed(message) => (ControlErrorCode::Failed, message),
        };
        Self::Error(ControlErrorBody {
            code,
            message,
            evidence: BTreeMap::new(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub instance_id: String,
    pub profile: String,
    pub uptime_ms: u128,
    pub ipc_endpoint: String,
    pub core_running: bool,
    pub plugin_count: usize,
    pub runner_count: usize,
}

/// Host process metrics for the overview dashboard (read-only).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostMetrics {
    pub pid: u32,
    pub uptime_ms: u128,
    /// Resident set size in bytes when the platform can report it.
    ///
    /// On Windows this is the current process Working Set (`WorkingSetSize`),
    /// not Commit Size or Private Bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    /// Cumulative process CPU time in milliseconds when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_time_ms: Option<u64>,
    /// Core runtime scheduling and mailbox metrics. Absent when Core is stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core: Option<CoreRuntimeMetrics>,
    /// Physical execution-domain capacity and lane pressure.
    #[serde(default)]
    pub execution_domains: Vec<ExecutionDomainMetrics>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoreRuntimeMetrics {
    pub control_mailbox_depth: usize,
    pub data_mailbox_depth: usize,
    pub control_oldest_message_age_ns: u64,
    pub data_oldest_message_age_ns: u64,
    pub submit_to_dispatch_samples: u64,
    pub submit_to_dispatch_total_ns: u64,
    pub submit_to_dispatch_max_ns: u64,
    pub cancel_propagation_samples: u64,
    pub cancel_propagation_total_ns: u64,
    pub cancel_propagation_max_ns: u64,
    pub completion_route_samples: u64,
    pub completion_route_total_ns: u64,
    pub completion_route_max_ns: u64,
    pub scheduler_passes: u64,
    pub scheduler_total_ns: u64,
    pub scheduler_max_ns: u64,
    pub lane_starvation_events: u64,
    pub reserved_capacity_uses: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionDomainMetrics {
    pub domain_id: String,
    pub execution_classes: Vec<String>,
    pub configured_threads: usize,
    pub active_threads: usize,
    pub queued_entries: usize,
    pub running_entries: usize,
    pub inflight_bytes: usize,
    pub max_inflight_bytes: usize,
    pub degraded: bool,
    pub lanes: Vec<ExecutionLaneMetrics>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionLaneMetrics {
    pub lane: String,
    pub queued_entries: usize,
    pub running_entries: usize,
    pub inflight_bytes: usize,
    pub queue_entry_limit: usize,
    pub max_inflight_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoreStatus {
    pub running: bool,
    pub profile_id: Option<String>,
    pub registry_generation: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginListResponse {
    pub plugins: Vec<PluginStatus>,
    pub diagnostics: Vec<PluginInventoryDiagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginStatus {
    pub plugin_id: String,
    pub configured: bool,
    pub active_deployment: Option<String>,
    pub preferred_deployment: Option<String>,
    pub candidates: Vec<PluginCandidateStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginCandidateStatus {
    pub deployment: String,
    pub version: String,
    pub api_version: String,
    pub sha256: String,
    pub path: String,
    pub available: bool,
    pub runner_link: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginInventoryDiagnostic {
    pub manifest_path: String,
    pub plugin_id: Option<String>,
    pub deployment: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDeploymentParam {
    pub plugin_id: String,
    pub deployment: mutsuki_runtime_contracts::PluginDeploymentKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDeploymentClearParam {
    pub plugin_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginReloadResponse {
    pub previous_generation: u64,
    pub registry_generation: u64,
    pub plugin_count: usize,
    pub changes: Vec<PluginReloadChange>,
    pub runner_errors: Vec<String>,
    /// Event sources are product-scoped and remain running across plugin generation reloads.
    pub event_sources: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginReloadChange {
    pub surface_id: String,
    pub compatibility: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunnerStatus {
    pub runner_id: String,
    pub plugin_id: String,
    pub state: String,
    pub pid: Option<u32>,
    pub restarts: u32,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventSourceStatus {
    pub source_id: String,
    pub plugin_id: String,
    pub instance_id: String,
    pub state: String,
    pub health: String,
    pub last_error: Option<String>,
    pub reconnects: u32,
    pub last_event_unix_ms: Option<u128>,
    #[serde(default)]
    pub started_at_unix_ms: Option<u128>,
}

/// Control-plane mirror of Core `RuntimeStatistics` (Core types are not serde).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeStatisticsView {
    pub tasks: TaskPoolStatisticsView,
    pub retained_events: usize,
    pub dropped_events: u64,
    pub retained_traces: usize,
    pub dropped_traces: u64,
    pub scheduler_decisions: u64,
}

/// Control-plane mirror of Core `TaskPoolStatistics`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskPoolStatisticsView {
    pub ready: usize,
    pub running: usize,
    pub waiting: usize,
    pub blocked: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub expired: usize,
    pub dead_letter: usize,
    pub submitted_total: u64,
    pub attempts_started: u64,
    pub cumulative_queue_steps: u64,
    pub cumulative_execution_steps: u64,
    pub stale_results_rejected: u64,
    pub terminal_records_evicted: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskFailureSummary {
    pub code: String,
    pub source: String,
    pub route: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskOutcomeView {
    pub task_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub protocol_id: String,
    pub status: String,
    pub priority: i64,
    pub ready_at_step: Option<u64>,
    pub created_sequence: u64,
    pub registry_generation: u64,
    pub target_binding_id: Option<String>,
    pub runner_hint: Option<String>,
    pub claimed_by: Option<String>,
    pub owner_runner: Option<String>,
    pub lease_id: Option<String>,
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
    pub input_refs: Vec<String>,
    pub output_ref: Option<String>,
    pub continuation_ref: Option<String>,
    pub required_surfaces: Vec<String>,
    pub failure: Option<TaskFailureSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthReport {
    pub service: String,
    pub core: String,
    pub plugins: String,
    pub runners: String,
    pub event_sources: String,
    pub event_source_details: Vec<EventSourceStatus>,
    pub recent_errors: Vec<String>,
    #[serde(default)]
    pub components: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdParam {
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOutcomesBatchParam {
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskOutcomesBatchResponse {
    pub outcomes: Vec<TaskOutcomeView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWaitParam {
    pub ids: Vec<String>,
    #[serde(default = "default_task_wait_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_task_wait_timeout_ms() -> u64 {
    5_000
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskWaitResponse {
    pub outcomes: Vec<TaskOutcomeView>,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSubmitBatchParam {
    pub batch: TaskBatch,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskSubmitBatchResponse {
    pub handles: Vec<TaskHandle>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskEventsAfterParam {
    pub sequence: u64,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskEventPage {
    pub next_sequence: u64,
    pub earliest_available_sequence: Option<u64>,
    pub latest_sequence: u64,
    pub lost: u64,
    pub dropped: u64,
    pub has_more: bool,
    pub events: Vec<RuntimeEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoreDrainResponse {
    pub state: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogTailParams {
    pub cursor: Option<u64>,
    pub lines: Option<usize>,
    #[serde(default)]
    pub filters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogTailEntry {
    pub offset: u64,
    pub line: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogTailResponse {
    pub cursor: u64,
    pub entries: Vec<LogTailEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_metrics_rss_remains_optional_across_serialization() {
        let without_rss = HostMetrics {
            pid: 42,
            uptime_ms: 7,
            rss_bytes: None,
            cpu_time_ms: None,
            core: None,
            execution_domains: Vec::new(),
        };
        let encoded = serde_json::to_value(&without_rss).expect("serialize host metrics");
        assert!(
            !encoded
                .as_object()
                .expect("host metrics object")
                .contains_key("rss_bytes")
        );
        let decoded: HostMetrics =
            serde_json::from_value(encoded).expect("deserialize host metrics without RSS");
        assert_eq!(decoded.rss_bytes, None);

        let with_rss = HostMetrics {
            rss_bytes: Some(123_456),
            ..without_rss
        };
        let encoded = serde_json::to_value(&with_rss).expect("serialize host metrics with RSS");
        assert_eq!(encoded["rss_bytes"], 123_456);
        let decoded: HostMetrics =
            serde_json::from_value(encoded).expect("deserialize host metrics with RSS");
        assert_eq!(decoded.rss_bytes, Some(123_456));
    }
}
