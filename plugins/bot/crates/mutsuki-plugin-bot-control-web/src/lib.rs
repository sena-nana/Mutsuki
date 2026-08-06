//! Control WebExtension: exposes typed ServiceHost [`ControlCommand`] operations as `control.*` RPC.
//!
//! Read-only methods require the `runtime.read` capability; mutating ops require
//! `runtime.write` (both declared on the extension manifest and checked per RPC params).

use std::sync::Arc;

use mutsuki_runtime_contracts::{CancelPolicy, TaskHandle};
use mutsuki_service_control::{
    ControlCommand, ControlError, ControlErrorBody, ControlErrorCode, ControlHandler,
    ControlRequest, ControlResponse, ControlResult, CoreDrainResponse, EventSourceStatus,
    HealthReport, HostMetrics, IdParam, LogTailParams, LogTailResponse, PluginDeploymentClearParam,
    PluginDeploymentParam, PluginListResponse, PluginReloadResponse, RunnerStatus,
    RuntimeStatisticsView, ServiceStatus, TaskEventPage, TaskEventsAfterParam, TaskSnapshot,
    TaskSubmitBatchParam, TaskSubmitBatchResponse,
};
use mutsuki_web_extension::{ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor};
use mutsuki_web_protocol::{
    EXTENSION_MANIFEST_VERSION, ExtensionManifest, JsonValue, WEB_PROTOCOL_VERSION,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "control";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_RUNTIME_READ: &str = "runtime.read";
pub const CAPABILITY_RUNTIME_WRITE: &str = "runtime.write";

pub type ControlRpcResult<T> = Result<T, ControlRpcError>;

#[derive(Debug)]
pub enum ControlRpcError {
    Remote(ControlErrorBody),
    Protocol(String),
}

impl ControlRpcError {
    pub fn code(&self) -> Option<ControlErrorCode> {
        match self {
            Self::Remote(body) => Some(body.code),
            Self::Protocol(_) => None,
        }
    }
}

impl std::fmt::Display for ControlRpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remote(body) => write!(formatter, "{}: {}", body.code, body.message),
            Self::Protocol(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ControlRpcError {}

impl From<ControlRpcError> for ExtensionError {
    fn from(error: ControlRpcError) -> Self {
        match error {
            ControlRpcError::Remote(body) => Self::Rpc {
                code: body.code.as_str().into(),
                message: body.message,
            },
            ControlRpcError::Protocol(message) => Self::Registration(message),
        }
    }
}

macro_rules! control_result {
    ($result:expr, $variant:ident) => {
        match $result {
            ControlResult::$variant(value) => Ok(value),
            other => Err(unexpected_control_result(stringify!($variant), &other)),
        }
    };
}

macro_rules! unit_control_result {
    ($result:expr, $variant:ident) => {
        match $result {
            ControlResult::$variant => Ok(()),
            other => Err(unexpected_control_result(stringify!($variant), &other)),
        }
    };
}

/// Shared in-process caller used by `control` RPC handlers and aggregating extensions.
#[derive(Clone)]
pub struct ControlRpcCaller {
    control: Arc<dyn ControlHandler>,
    token: String,
}

impl ControlRpcCaller {
    pub fn new(control: Arc<dyn ControlHandler>, token: impl Into<String>) -> Self {
        Self {
            control,
            token: token.into(),
        }
    }

    pub fn invoke(&self, command: ControlCommand) -> ControlRpcResult<ControlResult> {
        let control = self.control.clone();
        let token = self.token.clone();
        let future = async move { control.handle(ControlRequest::new(token, command)).await };
        unwrap_control(run_control_future(future))
    }

    pub fn health(&self) -> ControlRpcResult<HealthReport> {
        control_result!(self.invoke(ControlCommand::HealthCheck)?, HealthCheck)
    }

    pub fn service_status(&self) -> ControlRpcResult<ServiceStatus> {
        control_result!(self.invoke(ControlCommand::ServiceStatus)?, ServiceStatus)
    }

    pub fn plugin_list(&self) -> ControlRpcResult<PluginListResponse> {
        control_result!(self.invoke(ControlCommand::PluginList)?, PluginList)
    }

    pub fn runner_list(&self) -> ControlRpcResult<Vec<RunnerStatus>> {
        control_result!(self.invoke(ControlCommand::RunnerList)?, RunnerList)
    }

    pub fn event_source_list(&self) -> ControlRpcResult<Vec<EventSourceStatus>> {
        control_result!(
            self.invoke(ControlCommand::EventSourceList)?,
            EventSourceList
        )
    }

    pub fn runtime_statistics(&self) -> ControlRpcResult<RuntimeStatisticsView> {
        control_result!(
            self.invoke(ControlCommand::RuntimeStatistics)?,
            RuntimeStatistics
        )
    }

    pub fn host_metrics(&self) -> ControlRpcResult<HostMetrics> {
        control_result!(self.invoke(ControlCommand::HostMetrics)?, HostMetrics)
    }

    pub fn log_tail(&self, params: LogTailParams) -> ControlRpcResult<LogTailResponse> {
        control_result!(self.invoke(ControlCommand::LogTail(params))?, LogTail)
    }

    pub fn task_list(&self) -> ControlRpcResult<Vec<TaskSnapshot>> {
        control_result!(self.invoke(ControlCommand::TaskList)?, TaskList)
    }

    pub fn task_events_after(
        &self,
        params: TaskEventsAfterParam,
    ) -> ControlRpcResult<TaskEventPage> {
        control_result!(
            self.invoke(ControlCommand::TaskEventsAfter(params))?,
            TaskEventsAfter
        )
    }

    pub fn task_submit_batch(
        &self,
        params: TaskSubmitBatchParam,
    ) -> ControlRpcResult<TaskSubmitBatchResponse> {
        control_result!(
            self.invoke(ControlCommand::TaskSubmitBatch(params))?,
            TaskSubmitBatch
        )
    }

    pub fn task_cancel(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(self.invoke(ControlCommand::TaskCancel(params))?, TaskCancel)
    }

    pub fn core_begin_drain(&self) -> ControlRpcResult<CoreDrainResponse> {
        control_result!(self.invoke(ControlCommand::CoreBeginDrain)?, CoreBeginDrain)
    }

    pub fn plugin_reload(&self) -> ControlRpcResult<PluginReloadResponse> {
        control_result!(self.invoke(ControlCommand::PluginReload)?, PluginReload)
    }

    pub fn plugin_deployment_set(
        &self,
        params: PluginDeploymentParam,
    ) -> ControlRpcResult<PluginReloadResponse> {
        control_result!(
            self.invoke(ControlCommand::PluginDeploymentSet(params))?,
            PluginDeploymentSet
        )
    }

    pub fn plugin_deployment_clear(
        &self,
        params: PluginDeploymentClearParam,
    ) -> ControlRpcResult<PluginReloadResponse> {
        control_result!(
            self.invoke(ControlCommand::PluginDeploymentClear(params))?,
            PluginDeploymentClear
        )
    }

    pub fn runner_restart(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::RunnerRestart(params))?,
            RunnerRestart
        )
    }

    pub fn runner_stop(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(self.invoke(ControlCommand::RunnerStop(params))?, RunnerStop)
    }

    pub fn event_source_restart(&self, params: IdParam) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::EventSourceRestart(params))?,
            EventSourceRestart
        )
    }

    pub fn service_shutdown(&self) -> ControlRpcResult<()> {
        unit_control_result!(
            self.invoke(ControlCommand::ServiceShutdown)?,
            ServiceShutdown
        )
    }
}

pub struct ControlWebExtension {
    caller: ControlRpcCaller,
}

impl ControlWebExtension {
    pub fn new(caller: ControlRpcCaller) -> Self {
        Self { caller }
    }

    pub fn from_handler(control: Arc<dyn ControlHandler>, token: impl Into<String>) -> Self {
        Self::new(ControlRpcCaller::new(control, token))
    }
}

impl WebExtension for ControlWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        manifest()
    }

    fn frontend_assets(&self) -> Option<mutsuki_web_protocol::WebFrontendAssets> {
        None
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let caller = self.caller.clone();
        ctx.register("health", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                encode_web(caller.health()?)
            }
        });
        ctx.register("service_status", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                encode_web(caller.service_status()?)
            }
        });
        ctx.register("plugin_list", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                encode_web(caller.plugin_list()?)
            }
        });
        ctx.register("runner_list", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                encode_web(caller.runner_list()?)
            }
        });
        ctx.register("event_source_list", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                encode_web(caller.event_source_list()?)
            }
        });
        ctx.register("runtime_statistics", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                encode_web(caller.runtime_statistics()?)
            }
        });
        ctx.register("host_metrics", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                encode_web(caller.host_metrics()?)
            }
        });
        ctx.register("log_tail", {
            let caller = caller.clone();
            move |params| {
                require_runtime_read(&params)?;
                let params = decode_control_params::<LogTailParams>(&params)?;
                encode_web(caller.log_tail(params)?)
            }
        });
        ctx.register("task_list", {
            let caller = caller.clone();
            move |params| {
                require_runtime_read(&params)?;
                encode_web(caller.task_list()?)
            }
        });
        ctx.register("task_events_after", {
            let caller = caller.clone();
            move |params| {
                require_runtime_read(&params)?;
                let params = decode_control_params::<TaskEventsAfterParam>(&params)?;
                encode_web(caller.task_events_after(params)?)
            }
        });
        ctx.register("plugin_reload", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                encode_web(caller.plugin_reload()?)
            }
        });
        ctx.register("plugin_deployment_set", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                let params = decode_control_params::<PluginDeploymentParam>(&params)?;
                encode_web(caller.plugin_deployment_set(params)?)
            }
        });
        ctx.register("plugin_deployment_clear", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                let params = decode_control_params::<PluginDeploymentClearParam>(&params)?;
                encode_web(caller.plugin_deployment_clear(params)?)
            }
        });
        ctx.register("runner_restart", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                caller.runner_restart(decode_control_params::<IdParam>(&params)?)?;
                Ok(Value::Null)
            }
        });
        ctx.register("runner_stop", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                caller.runner_stop(decode_control_params::<IdParam>(&params)?)?;
                Ok(Value::Null)
            }
        });
        ctx.register("event_source_restart", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                caller.event_source_restart(decode_control_params::<IdParam>(&params)?)?;
                Ok(Value::Null)
            }
        });
        ctx.register("task_submit_batch", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                let params = decode_control_params::<TaskSubmitBatchParam>(&params)?;
                encode_web(caller.task_submit_batch(params)?)
            }
        });
        ctx.register("task_cancel", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                caller.task_cancel(decode_control_params::<IdParam>(&params)?)?;
                Ok(Value::Null)
            }
        });
        ctx.register("core_begin_drain", {
            let caller = caller.clone();
            move |params| {
                require_runtime_write(&params)?;
                encode_web(caller.core_begin_drain()?)
            }
        });
        ctx.register("service_shutdown", move |params| {
            require_runtime_write(&params)?;
            caller.service_shutdown()?;
            Ok(Value::Null)
        });
        Ok(())
    }

    fn register_events(
        &self,
        _ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

fn require_capability(params: &JsonValue, required: &str) -> Result<(), ExtensionError> {
    let caps = caps_from_params(params);
    if caps.iter().any(|cap| cap == "*" || cap == required) {
        return Ok(());
    }
    Err(ExtensionError::CapabilityDenied(required.into()))
}

fn require_runtime_read(params: &JsonValue) -> Result<(), ExtensionError> {
    require_capability(params, CAPABILITY_RUNTIME_READ)
}

fn require_runtime_write(params: &JsonValue) -> Result<(), ExtensionError> {
    require_capability(params, CAPABILITY_RUNTIME_WRITE)
}

fn control_params(params: &JsonValue) -> Value {
    match params {
        Value::Object(map) => {
            let mut out = map.clone();
            out.remove("capabilities");
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn decode_control_params<T: DeserializeOwned>(params: &JsonValue) -> Result<T, ExtensionError> {
    serde_json::from_value(control_params(params))
        .map_err(|error| ExtensionError::Registration(format!("invalid control request: {error}")))
}

fn encode_web<T: Serialize>(value: T) -> Result<Value, ExtensionError> {
    serde_json::to_value(value)
        .map_err(|error| ExtensionError::Registration(format!("encode control response: {error}")))
}

fn caps_from_params(params: &JsonValue) -> Vec<String> {
    params
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn manifest() -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: String::new(),
        capabilities: vec![
            CAPABILITY_RUNTIME_READ.into(),
            CAPABILITY_RUNTIME_WRITE.into(),
        ],
        permissions: vec![],
        assets: vec![],
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    }
}

fn run_control_future<F>(future: F) -> ControlResponse
where
    F: std::future::Future<Output = ControlResponse> + Send + 'static,
{
    // WebHost RPC handlers are sync; Link control I/O is async.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                // Prefer the host reactor so local Link shares ServiceHost accept loop timing.
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            _ => std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("control bridge runtime")
                    .block_on(future)
            })
            .join()
            .expect("control bridge thread"),
        },
        Err(_) => futures_executor::block_on(future),
    }
}

fn unwrap_control(response: ControlResponse) -> ControlRpcResult<ControlResult> {
    match response {
        ControlResponse::Ok(result) => Ok(result),
        ControlResponse::Error(body) => Err(ControlRpcError::Remote(body)),
    }
}

fn unexpected_control_result(expected: &str, actual: &ControlResult) -> ControlRpcError {
    ControlRpcError::Protocol(format!(
        "control response type mismatch: expected {expected}, received {:?}",
        actual.method()
    ))
}

/// Fixture ControlHandler for demos/tests.
#[derive(Clone)]
pub struct FixtureControlHandler {
    pub fail_statistics: bool,
    pub mutations: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Default for FixtureControlHandler {
    fn default() -> Self {
        Self {
            fail_statistics: false,
            mutations: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

impl FixtureControlHandler {
    fn record_mutation(&self, name: &str) {
        if let Ok(mut items) = self.mutations.lock() {
            items.push(name.into());
        }
    }
}

fn fixture_reload_response() -> PluginReloadResponse {
    PluginReloadResponse {
        previous_generation: 1,
        registry_generation: 2,
        plugin_count: 2,
        changes: Vec::new(),
        runner_errors: Vec::new(),
        event_sources: "unchanged".into(),
    }
}

impl ControlHandler for FixtureControlHandler {
    fn handle(&self, request: ControlRequest) -> mutsuki_service_control::ControlFuture {
        let ok = request.token == "local-dev" || request.token == "fixture";
        let command = request.command;
        let fail_statistics = self.fail_statistics;
        let fixture = self.clone();
        Box::pin(async move {
            if !ok {
                return ControlResponse::err(ControlError::Unauthorized);
            }
            match command {
                ControlCommand::ServiceStatus => {
                    ControlResponse::ok(ControlResult::ServiceStatus(ServiceStatus {
                        instance_id: "demo".into(),
                        profile: "dev".into(),
                        uptime_ms: 12_345,
                        ipc_endpoint: "local://demo".into(),
                        core_running: true,
                        plugin_count: 2,
                        runner_count: 1,
                    }))
                }
                ControlCommand::HealthCheck => {
                    ControlResponse::ok(ControlResult::HealthCheck(HealthReport {
                        service: "ok".into(),
                        core: "ok".into(),
                        plugins: "ok".into(),
                        runners: "ok".into(),
                        event_sources: "ok".into(),
                        event_source_details: vec![mutsuki_service_control::EventSourceStatus {
                            source_id: "demo.source".into(),
                            plugin_id: "demo.plugin".into(),
                            instance_id: "demo".into(),
                            state: "running".into(),
                            health: "healthy".into(),
                            last_error: None,
                            reconnects: 0,
                            last_event_unix_ms: Some(1_700_000_000_000),
                            started_at_unix_ms: Some(1_699_999_940_000),
                        }],
                        recent_errors: Vec::new(),
                        components: [(
                            "mutsuki.bot.qqbot.gateway:demo".into(),
                            json!({
                                "status": "ok",
                                "started_at_unix_ms": 1_699_999_880_000u64,
                                "connected_since_unix_ms": 1_699_999_910_000u64,
                            }),
                        )]
                        .into_iter()
                        .collect(),
                    }))
                }
                ControlCommand::PluginList => {
                    ControlResponse::ok(ControlResult::PluginList(PluginListResponse {
                        plugins: vec![mutsuki_service_control::PluginStatus {
                            plugin_id: "demo.plugin".into(),
                            configured: true,
                            active_deployment: Some("builtin".into()),
                            preferred_deployment: Some("builtin".into()),
                            candidates: vec![
                                mutsuki_service_control::PluginCandidateStatus {
                                    deployment: "builtin".into(),
                                    version: "0.1.0".into(),
                                    api_version: "1".into(),
                                    sha256: "abc123".into(),
                                    path: "/plugins/demo".into(),
                                    available: true,
                                    runner_link: None,
                                },
                                mutsuki_service_control::PluginCandidateStatus {
                                    deployment: "abi".into(),
                                    version: "0.1.0".into(),
                                    api_version: "1".into(),
                                    sha256: "def456".into(),
                                    path: "/plugins/demo-abi".into(),
                                    available: false,
                                    runner_link: Some("standalone".into()),
                                },
                            ],
                        }],
                        diagnostics: vec![mutsuki_service_control::PluginInventoryDiagnostic {
                            manifest_path: "/plugins/broken/manifest.json".into(),
                            plugin_id: Some("broken.plugin".into()),
                            deployment: Some("wasm".into()),
                            detail: "invalid manifest version".into(),
                        }],
                    }))
                }
                ControlCommand::RunnerList => {
                    ControlResponse::ok(ControlResult::RunnerList(vec![RunnerStatus {
                        runner_id: "demo.runner".into(),
                        plugin_id: "demo.plugin".into(),
                        state: "running".into(),
                        pid: Some(4242),
                        restarts: 0,
                        last_error: None,
                    }]))
                }
                ControlCommand::EventSourceList => {
                    ControlResponse::ok(ControlResult::EventSourceList(vec![
                        mutsuki_service_control::EventSourceStatus {
                            source_id: "demo.source".into(),
                            plugin_id: "demo.plugin".into(),
                            instance_id: "demo".into(),
                            state: "running".into(),
                            health: "healthy".into(),
                            last_error: None,
                            reconnects: 0,
                            last_event_unix_ms: Some(1_700_000_000_000),
                            started_at_unix_ms: Some(1_699_999_940_000),
                        },
                    ]))
                }
                ControlCommand::RuntimeStatistics => {
                    if fail_statistics {
                        ControlResponse::err(ControlError::CoreUnavailable)
                    } else {
                        ControlResponse::ok(ControlResult::RuntimeStatistics(
                            RuntimeStatisticsView {
                                tasks: mutsuki_service_control::TaskPoolStatisticsView {
                                    ready: 1,
                                    running: 2,
                                    submitted_total: 14,
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                        ))
                    }
                }
                ControlCommand::HostMetrics => {
                    ControlResponse::ok(ControlResult::HostMetrics(HostMetrics {
                        pid: 4242,
                        uptime_ms: 90_000,
                        rss_bytes: Some(64 * 1024 * 1024),
                        cpu_time_ms: Some(1_200),
                        core: None,
                        execution_domains: Vec::new(),
                    }))
                }
                ControlCommand::LogTail(_) => {
                    ControlResponse::ok(ControlResult::LogTail(LogTailResponse {
                        cursor: 2,
                        entries: vec![
                            mutsuki_service_control::LogTailEntry {
                                offset: 1,
                                line: "demo log line".into(),
                            },
                            mutsuki_service_control::LogTailEntry {
                                offset: 2,
                                line: "fixture tail".into(),
                            },
                        ],
                    }))
                }
                ControlCommand::TaskList => {
                    ControlResponse::ok(ControlResult::TaskList(vec![TaskSnapshot {
                        task_id: "demo.task".into(),
                        protocol_id: "demo.protocol".into(),
                        status: "ready".into(),
                        priority: 0,
                        ready_at_step: Some(1),
                        created_sequence: 1,
                        registry_generation: 1,
                        target_binding_id: None,
                        runner_hint: Some("demo.runner".into()),
                        claimed_by: None,
                        owner_runner: None,
                        lease_id: None,
                        trace_id: None,
                        correlation_id: None,
                        input_refs: Vec::new(),
                        output_ref: None,
                        continuation_ref: None,
                        required_surfaces: Vec::new(),
                        failure: None,
                    }]))
                }
                ControlCommand::PluginReload => {
                    fixture.record_mutation("plugin_reload");
                    ControlResponse::ok(ControlResult::PluginReload(fixture_reload_response()))
                }
                ControlCommand::PluginDeploymentSet(_) => {
                    fixture.record_mutation("plugin_deployment_set");
                    ControlResponse::ok(ControlResult::PluginDeploymentSet(
                        fixture_reload_response(),
                    ))
                }
                ControlCommand::PluginDeploymentClear(_) => {
                    fixture.record_mutation("plugin_deployment_clear");
                    ControlResponse::ok(ControlResult::PluginDeploymentClear(
                        fixture_reload_response(),
                    ))
                }
                ControlCommand::RunnerRestart(_) => {
                    fixture.record_mutation("runner_restart");
                    ControlResponse::ok(ControlResult::RunnerRestart)
                }
                ControlCommand::RunnerStop(_) => {
                    fixture.record_mutation("runner_stop");
                    ControlResponse::ok(ControlResult::RunnerStop)
                }
                ControlCommand::EventSourceRestart(_) => {
                    fixture.record_mutation("event_source_restart");
                    ControlResponse::ok(ControlResult::EventSourceRestart)
                }
                ControlCommand::TaskSubmitBatch(_) => {
                    fixture.record_mutation("task_submit_batch");
                    ControlResponse::ok(ControlResult::TaskSubmitBatch(TaskSubmitBatchResponse {
                        handles: vec![TaskHandle {
                            task_id: "submitted.demo".into(),
                            protocol_id: "demo.protocol".into(),
                            target_binding_id: None,
                            cancel_policy: CancelPolicy::Cascade,
                            trace_id: None,
                            correlation_id: None,
                        }],
                    }))
                }
                ControlCommand::TaskCancel(_) => {
                    fixture.record_mutation("task_cancel");
                    ControlResponse::ok(ControlResult::TaskCancel)
                }
                ControlCommand::TaskEventsAfter(param) => {
                    if param.limit == 0 {
                        return ControlResponse::err(ControlError::BadRequest(
                            "limit must be greater than zero".into(),
                        ));
                    }
                    ControlResponse::ok(ControlResult::TaskEventsAfter(TaskEventPage {
                        next_sequence: param.sequence + 1,
                        earliest_available_sequence: Some(1),
                        latest_sequence: 1,
                        lost: 0,
                        dropped: 0,
                        has_more: false,
                        events: vec![],
                    }))
                }
                ControlCommand::CoreBeginDrain => {
                    fixture.record_mutation("core_begin_drain");
                    ControlResponse::ok(ControlResult::CoreBeginDrain(CoreDrainResponse {
                        state: "draining".into(),
                    }))
                }
                ControlCommand::ServiceShutdown => {
                    fixture.record_mutation("service_shutdown");
                    ControlResponse::ok(ControlResult::ServiceShutdown)
                }
                other => {
                    ControlResponse::err(ControlError::Unsupported(format!("{:?}", other.method())))
                }
            }
        })
    }
}
