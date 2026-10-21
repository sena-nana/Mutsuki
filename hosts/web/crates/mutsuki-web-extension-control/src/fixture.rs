#[allow(clippy::wildcard_imports)]
use super::*;
use mutsuki_runtime_contracts::{CancelPolicy, TaskHandle};
use mutsuki_service_control::ControlError;
use serde_json::json;

/// Fixture ControlHandler for demos/tests.
#[derive(Clone)]
pub struct FixtureControlHandler {
    pub fail_statistics: bool,
    pub mutations: Arc<std::sync::Mutex<Vec<String>>>,
    pub uptime_ms: Arc<std::sync::atomic::AtomicU64>,
    pub service_status_calls: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for FixtureControlHandler {
    fn default() -> Self {
        Self {
            fail_statistics: false,
            mutations: Arc::new(std::sync::Mutex::new(Vec::new())),
            uptime_ms: Arc::new(std::sync::atomic::AtomicU64::new(12_345)),
            service_status_calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
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
        let uptime_ms = self.uptime_ms.clone();
        let service_status_calls = self.service_status_calls.clone();
        let fixture = self.clone();
        Box::pin(async move {
            if !ok {
                return ControlResponse::err(ControlError::Unauthorized);
            }
            match command {
                ControlCommand::ServiceStatus => {
                    service_status_calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    ControlResponse::ok(ControlResult::ServiceStatus(ServiceStatus {
                        instance_id: "demo".into(),
                        profile: "dev".into(),
                        uptime_ms: u128::from(uptime_ms.load(std::sync::atomic::Ordering::Acquire)),
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
