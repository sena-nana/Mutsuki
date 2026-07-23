//! Control WebExtension: exposes ServiceHost [`ControlMethod`] as `control.*` Web RPC.
//!
//! Read-only methods require the `runtime.read` capability; mutating ops require
//! `runtime.write` (both declared on the extension manifest and checked per RPC params).

use std::sync::Arc;

use mutsuki_service_control::{
    ControlError, ControlHandler, ControlMethod, ControlRequest, ControlResponse,
    CoreDrainResponse, EventSourceStatus, HealthReport, LogTailResponse,
    PluginListResponse, PluginReloadResponse, RunnerStatus, RuntimeStatisticsView, ServiceStatus,
    TaskEventPage, TaskEventsAfterParam, TaskSnapshot, TaskSubmitBatchResponse,
};
use mutsuki_web_extension::{ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor};
use mutsuki_web_protocol::{
    EXTENSION_MANIFEST_VERSION, ExtensionManifest, JsonValue, WEB_PROTOCOL_VERSION,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "control";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_RUNTIME_READ: &str = "runtime.read";
pub const CAPABILITY_RUNTIME_WRITE: &str = "runtime.write";

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

    pub fn invoke(&self, method: ControlMethod, params: Value) -> Result<Value, ExtensionError> {
        let control = self.control.clone();
        let token = self.token.clone();
        unwrap_control(futures_executor::block_on(async move {
            control
                .handle(ControlRequest {
                    token,
                    method,
                    params,
                })
                .await
        }))
    }

    pub fn health(&self) -> Result<HealthReport, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::HealthCheck, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn service_status(&self) -> Result<ServiceStatus, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::ServiceStatus, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn plugin_list(&self) -> Result<PluginListResponse, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::PluginList, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn runner_list(&self) -> Result<Vec<RunnerStatus>, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::RunnerList, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn event_source_list(&self) -> Result<Vec<EventSourceStatus>, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::EventSourceList, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn runtime_statistics(&self) -> Result<RuntimeStatisticsView, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::RuntimeStatistics, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn log_tail(&self, params: Value) -> Result<LogTailResponse, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::LogTail, params)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn task_list(&self) -> Result<Vec<TaskSnapshot>, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::TaskList, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn task_events_after(&self, params: Value) -> Result<TaskEventPage, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::TaskEventsAfter, params)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn task_submit_batch(
        &self,
        params: Value,
    ) -> Result<TaskSubmitBatchResponse, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::TaskSubmitBatch, params)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn task_cancel(&self, params: Value) -> Result<Value, ExtensionError> {
        self.invoke(ControlMethod::TaskCancel, params)
    }

    pub fn core_begin_drain(&self) -> Result<CoreDrainResponse, ExtensionError> {
        serde_json::from_value(self.invoke(ControlMethod::CoreBeginDrain, Value::Null)?)
            .map_err(|e| ExtensionError::Registration(e.to_string()))
    }

    pub fn service_shutdown(&self) -> Result<Value, ExtensionError> {
        self.invoke(ControlMethod::ServiceShutdown, Value::Null)
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
                Ok(serde_json::to_value(caller.health()?).unwrap_or_default())
            }
        });
        ctx.register("service_status", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                Ok(serde_json::to_value(caller.service_status()?).unwrap_or_default())
            }
        });
        ctx.register("plugin_list", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                Ok(serde_json::to_value(caller.plugin_list()?).unwrap_or_default())
            }
        });
        ctx.register("runner_list", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                Ok(serde_json::to_value(caller.runner_list()?).unwrap_or_default())
            }
        });
        ctx.register("event_source_list", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                Ok(serde_json::to_value(caller.event_source_list()?).unwrap_or_default())
            }
        });
        ctx.register("runtime_statistics", {
            let caller = caller.clone();
            move |_params| {
                require_runtime_read(&_params)?;
                Ok(serde_json::to_value(caller.runtime_statistics()?).unwrap_or_default())
            }
        });
        ctx.register("log_tail", {
            let caller = caller.clone();
            move |params| {
                require_runtime_read(&params)?;
                Ok(
                    serde_json::to_value(caller.log_tail(control_params(&params))?)
                        .unwrap_or_default(),
                )
            }
        });
        ctx.register("task_list", {
            let caller = caller.clone();
            move |params| {
                require_runtime_read(&params)?;
                Ok(serde_json::to_value(caller.task_list()?).unwrap_or_default())
            }
        });
        ctx.register("task_events_after", {
            let caller = caller.clone();
            move |params| {
                require_runtime_read(&params)?;
                Ok(
                    serde_json::to_value(caller.task_events_after(control_params(&params))?)
                        .unwrap_or_default(),
                )
            }
        });
        register_write(
            ctx,
            caller.clone(),
            "plugin_reload",
            ControlMethod::PluginReload,
        );
        register_write(
            ctx,
            caller.clone(),
            "plugin_deployment_set",
            ControlMethod::PluginDeploymentSet,
        );
        register_write(
            ctx,
            caller.clone(),
            "plugin_deployment_clear",
            ControlMethod::PluginDeploymentClear,
        );
        register_write(
            ctx,
            caller.clone(),
            "runner_restart",
            ControlMethod::RunnerRestart,
        );
        register_write(
            ctx,
            caller.clone(),
            "runner_stop",
            ControlMethod::RunnerStop,
        );
        register_write(
            ctx,
            caller.clone(),
            "event_source_restart",
            ControlMethod::EventSourceRestart,
        );
        register_write(
            ctx,
            caller.clone(),
            "task_submit_batch",
            ControlMethod::TaskSubmitBatch,
        );
        register_write(
            ctx,
            caller.clone(),
            "task_cancel",
            ControlMethod::TaskCancel,
        );
        register_write(
            ctx,
            caller.clone(),
            "core_begin_drain",
            ControlMethod::CoreBeginDrain,
        );
        register_write(
            ctx,
            caller,
            "service_shutdown",
            ControlMethod::ServiceShutdown,
        );
        Ok(())
    }

    fn register_events(
        &self,
        _ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

fn register_write(
    ctx: &mut RpcRegistry,
    caller: ControlRpcCaller,
    method_name: &'static str,
    method: ControlMethod,
) {
    ctx.register(method_name, move |params| {
        require_runtime_write(&params)?;
        Ok(caller.invoke(method, control_params(&params))?)
    });
}

fn require_capability(params: &JsonValue, required: &str) -> Result<(), ExtensionError> {
    let caps = caps_from_params(params);
    if caps.iter().any(|cap| cap == "*" || cap == required) {
        return Ok(());
    }
    Err(ExtensionError::Registration(format!(
        "capability denied: {required}"
    )))
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

fn unwrap_control(response: ControlResponse) -> Result<Value, ExtensionError> {
    if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        let body = response
            .error
            .unwrap_or(mutsuki_service_control::ControlErrorBody {
                code: "failed".into(),
                message: "control request failed".into(),
            });
        Err(ExtensionError::Registration(format!(
            "{}: {}",
            body.code, body.message
        )))
    }
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

impl ControlHandler for FixtureControlHandler {
    fn handle(&self, request: ControlRequest) -> mutsuki_service_control::ControlFuture {
        let ok = request.token == "local-dev" || request.token == "fixture";
        let method = request.method;
        let fail_statistics = self.fail_statistics;
        let fixture = self.clone();
        Box::pin(async move {
            if !ok {
                return ControlResponse::err(ControlError::Unauthorized);
            }
            match method {
                ControlMethod::ServiceStatus => ControlResponse::ok(ServiceStatus {
                    instance_id: "demo".into(),
                    profile: "dev".into(),
                    uptime_ms: 12_345,
                    ipc_endpoint: "local://demo".into(),
                    core_running: true,
                    plugin_count: 2,
                    runner_count: 1,
                }),
                ControlMethod::HealthCheck => ControlResponse::ok(HealthReport {
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
                }),
                ControlMethod::PluginList => ControlResponse::ok(PluginListResponse {
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
                }),
                ControlMethod::RunnerList => ControlResponse::ok(vec![RunnerStatus {
                    runner_id: "demo.runner".into(),
                    plugin_id: "demo.plugin".into(),
                    state: "running".into(),
                    pid: Some(4242),
                    restarts: 0,
                    last_error: None,
                }]),
                ControlMethod::EventSourceList => {
                    ControlResponse::ok(vec![mutsuki_service_control::EventSourceStatus {
                        source_id: "demo.source".into(),
                        plugin_id: "demo.plugin".into(),
                        instance_id: "demo".into(),
                        state: "running".into(),
                        health: "healthy".into(),
                        last_error: None,
                        reconnects: 0,
                        last_event_unix_ms: Some(1_700_000_000_000),
                        started_at_unix_ms: Some(1_699_999_940_000),
                    }])
                }
                ControlMethod::RuntimeStatistics => {
                    if fail_statistics {
                        ControlResponse::err(ControlError::Failed("core is not running".into()))
                    } else {
                        ControlResponse::ok(RuntimeStatisticsView {
                            tasks: mutsuki_service_control::TaskPoolStatisticsView {
                                ready: 1,
                                running: 2,
                                submitted_total: 14,
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                    }
                }
                ControlMethod::LogTail => ControlResponse::ok(LogTailResponse {
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
                }),
                ControlMethod::TaskList => ControlResponse::ok(vec![TaskSnapshot {
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
                }]),
                ControlMethod::PluginReload => {
                    fixture.record_mutation("plugin_reload");
                    ControlResponse::ok(PluginReloadResponse {
                        previous_generation: 1,
                        registry_generation: 2,
                        plugin_count: 2,
                        changes: Vec::new(),
                        runner_errors: Vec::new(),
                        event_sources: "unchanged".into(),
                    })
                }
                ControlMethod::PluginDeploymentSet => {
                    fixture.record_mutation("plugin_deployment_set");
                    ControlResponse::ok(Value::Null)
                }
                ControlMethod::PluginDeploymentClear => {
                    fixture.record_mutation("plugin_deployment_clear");
                    ControlResponse::ok(Value::Null)
                }
                ControlMethod::RunnerRestart => {
                    fixture.record_mutation("runner_restart");
                    ControlResponse::ok(Value::Null)
                }
                ControlMethod::RunnerStop => {
                    fixture.record_mutation("runner_stop");
                    ControlResponse::ok(Value::Null)
                }
                ControlMethod::EventSourceRestart => {
                    fixture.record_mutation("event_source_restart");
                    ControlResponse::ok(Value::Null)
                }
                ControlMethod::TaskSubmitBatch => {
                    fixture.record_mutation("task_submit_batch");
                    ControlResponse::ok(json!({
                        "handles": [{
                            "task_id": "submitted.demo",
                            "protocol_id": "demo.protocol",
                            "target_binding_id": null,
                            "cancel_policy": "best_effort",
                            "trace_id": null,
                            "correlation_id": null,
                        }]
                    }))
                }
                ControlMethod::TaskCancel => {
                    fixture.record_mutation("task_cancel");
                    ControlResponse::ok(json!({
                        "task_id": request.params.get("id").and_then(|v| v.as_str()).unwrap_or("demo.task"),
                        "status": "cancelled",
                    }))
                }
                ControlMethod::TaskEventsAfter => {
                    let param = match serde_json::from_value::<TaskEventsAfterParam>(request.params)
                    {
                        Ok(param) => param,
                        Err(error) => {
                            return ControlResponse::err(ControlError::BadRequest(
                                error.to_string(),
                            ));
                        }
                    };
                    if param.limit == 0 {
                        return ControlResponse::err(ControlError::BadRequest(
                            "limit must be greater than zero".into(),
                        ));
                    }
                    ControlResponse::ok(TaskEventPage {
                        next_sequence: param.sequence + 1,
                        earliest_available_sequence: Some(1),
                        latest_sequence: 1,
                        lost: 0,
                        dropped: 0,
                        has_more: false,
                        events: vec![],
                    })
                }
                ControlMethod::CoreBeginDrain => {
                    fixture.record_mutation("core_begin_drain");
                    ControlResponse::ok(CoreDrainResponse {
                        state: "draining".into(),
                    })
                }
                ControlMethod::ServiceShutdown => {
                    fixture.record_mutation("service_shutdown");
                    ControlResponse::empty_ok()
                }
                other => ControlResponse::err(ControlError::Unsupported(format!("{other:?}"))),
            }
        })
    }
}
