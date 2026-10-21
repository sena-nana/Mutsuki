//! Keeps the hand-written `@mutsuki/tauri-client` types honest.
//!
//! Every DTO the bridge exchanges with the frontend is serialized here from a fully populated
//! Rust value into `packages/tauri-client/contract/fixtures.generated.ts`, each wrapped in
//! `satisfies <Type>`. The frontend typecheck then fails on any field Rust sends that
//! `src/types.ts` does not declare, any field it declares as required that Rust omits, and any
//! type mismatch. This test fails when the checked-in file is stale; regenerate it with
//! `MUTSUKI_BLESS_TS_CONTRACT=1 cargo test -p mutsuki-tauri-bridge --test ts_contract`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use mutsuki_runtime_contracts::{
    ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic,
    RuntimeError, RuntimeEvent, RuntimeEventKind, ScalarValue, SpanStatus, TaskOutcome, TaskStatus,
    TraceSpan,
};
use mutsuki_tauri_bridge::{
    ApprovalDecision, ApprovalRequest, ApprovalResponse, DeliveryProgress, FrontendContext,
    FrontendLogRecord, FrontendTaskRequest, FrontendTaskResult, FrontendTaskRun, HealthComponent,
    HostRecentError, HostStatus, MutsukiFrontendEvent, PluginReloadRequest, PluginReloadResult,
    PluginSummary, PreviewHandle, ResourceBytes, ResourceChunk, ResourceText, RunnerSummary,
    RuntimeHealth, TaskResultRequest,
};
use serde::Serialize;
use serde_json::json;

const BLESS_VAR: &str = "MUTSUKI_BLESS_TS_CONTRACT";

fn context() -> FrontendContext {
    FrontendContext {
        window_label: Some("main".into()),
        webview_id: Some("webview-1".into()),
        session_id: Some("session-1".into()),
        user_action_id: Some("action-1".into()),
    }
}

fn attributes() -> BTreeMap<String, ScalarValue> {
    BTreeMap::from([
        ("text".into(), ScalarValue::String("value".into())),
        ("count".into(), ScalarValue::Int(2)),
        ("ratio".into(), ScalarValue::Float(0.5)),
        ("flag".into(), ScalarValue::Bool(true)),
    ])
}

fn runtime_error() -> RuntimeError {
    let mut error = RuntimeError::new("runtime.host_failed", "fixture", "fixture.route");
    error.lost_capability = Some("fixture.capability".into());
    error.recovery = Some("retry".into());
    error.evidence = attributes();
    error
}

fn runtime_event() -> RuntimeEvent {
    RuntimeEvent {
        sequence: 7,
        kind: RuntimeEventKind::Task,
        name: "task.completed".into(),
        subject_id: Some("task-1".into()),
        attributes: attributes(),
        error: Some(runtime_error()),
    }
}

fn trace_span() -> TraceSpan {
    TraceSpan {
        sequence: 3,
        trace_id: "trace-1".into(),
        span_id: "span-2".into(),
        parent_span_id: Some("span-1".into()),
        name: "runner.run_batch".into(),
        start: 1.0,
        end: Some(2.5),
        attributes: attributes(),
        status: SpanStatus::Ok,
    }
}

fn resource_ref() -> ResourceRef {
    ResourceRef {
        ref_id: "ref-1".into(),
        resource_id: ResourceId {
            kind_id: "bytes".into(),
            slot_id: "slot-1".into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: "mutsuki.std.resource.memory".into(),
        resource_kind: "bytes".into(),
        schema: "bytes.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::MmapFile {
            path: "resource.bin".into(),
            offset: 0,
            len: 4,
            readonly: true,
        },
        size_hint: Some(4),
        content_hash: Some("hash:4".into()),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn plugin_summary() -> PluginSummary {
    PluginSummary {
        plugin_id: "fixture.plugin".into(),
        version: "1.0.0".into(),
        enabled: false,
        deployment: "process".into(),
        status: "failed".into(),
        error: Some("missing runtime".into()),
    }
}

fn runner_summary() -> RunnerSummary {
    RunnerSummary {
        runner_id: "fixture.runner".into(),
        plugin_id: "fixture.plugin".into(),
        enabled: true,
        deployment: "process".into(),
        status: "running".into(),
        error: Some("stderr".into()),
    }
}

fn health(error: Option<&str>) -> HealthComponent {
    HealthComponent {
        healthy: error.is_none(),
        status: "ok".into(),
        error: error.map(Into::into),
    }
}

fn log_record() -> FrontendLogRecord {
    FrontendLogRecord {
        level: "info".into(),
        target: "mutsuki_tauri_host.runner".into(),
        message: "runner started".into(),
        timestamp_ms: 1_700_000_000_000,
        trace_id: Some("trace-1".into()),
        correlation_id: Some("corr-1".into()),
        fields: BTreeMap::from([("key".into(), json!({ "nested": [1, "two", null] }))]),
    }
}

fn approval_request() -> ApprovalRequest {
    ApprovalRequest {
        approval_id: "approval-1".into(),
        token: "token-1".into(),
        requester: "fixture.plugin".into(),
        operation: "effect.fs.write".into(),
        risk: "high".into(),
        trace_id: "trace-1".into(),
        correlation_id: "corr-1".into(),
        payload: json!({ "path": "out.txt" }),
        context: context(),
    }
}

fn frontend_events() -> Vec<MutsukiFrontendEvent> {
    let leaf = vec![
        MutsukiFrontendEvent::ObservabilityGap {
            stream: "trace".into(),
            lost: 1,
            dropped: 2,
        },
        MutsukiFrontendEvent::Task {
            task_id: "task-1".into(),
            event: runtime_event(),
        },
        MutsukiFrontendEvent::Runtime {
            event: runtime_event(),
        },
        MutsukiFrontendEvent::Trace { span: trace_span() },
        MutsukiFrontendEvent::Log {
            record: log_record(),
        },
        MutsukiFrontendEvent::Resource {
            ref_id: "ref-1".into(),
            operation: "created".into(),
        },
        MutsukiFrontendEvent::Approval {
            request: approval_request(),
        },
        MutsukiFrontendEvent::Plugin {
            plugin: plugin_summary(),
            operation: "reloaded".into(),
        },
        MutsukiFrontendEvent::Runner {
            runner_id: "fixture.runner".into(),
            status: "stderr".into(),
        },
        MutsukiFrontendEvent::AppDelivery {
            progress: DeliveryProgress {
                request_id: "request-1".into(),
                target_app: "fixture.app".into(),
                phase: "delivery_failed".into(),
                error: Some("unreachable".into()),
                error_kind: Some("transport".into()),
            },
        },
    ];
    let mut events = leaf.clone();
    events.push(MutsukiFrontendEvent::Batch { events: leaf });
    events
}

/// `(export name, TypeScript type, serialized Rust value)` for every mirrored DTO.
// One row per mirrored DTO; splitting the table would only scatter it.
#[allow(clippy::too_many_lines)]
fn fixtures() -> Vec<(&'static str, &'static str, serde_json::Value)> {
    fn value<T: Serialize>(value: &T) -> serde_json::Value {
        serde_json::to_value(value).expect("bridge DTO serializes")
    }
    vec![
        (
            "frontendTaskRequest",
            "FrontendTaskRequest",
            value(&FrontendTaskRequest {
                protocol_id: "fixture.echo".into(),
                payload: json!({ "text": "hi" }),
                task_id: Some("task-1".into()),
                trace_id: Some("trace-1".into()),
                correlation_id: Some("corr-1".into()),
                idempotency_key: Some("key-1".into()),
                target_binding_id: Some("binding-1".into()),
                runner_hint: Some("fixture.runner".into()),
                input_refs: vec!["ref-1".into()],
                priority: 3,
                context: context(),
            }),
        ),
        (
            "frontendTaskResult",
            "FrontendTaskResult",
            value(&FrontendTaskResult {
                task_id: "task-1".into(),
                status: Some(TaskStatus::Completed),
                outcome: Some(TaskOutcome::Completed {
                    task_id: "task-1".into(),
                    output: Some(json!({ "text": "hi" })),
                    output_ref: Some("ref-1".into()),
                }),
                events: vec![runtime_event()],
                events_dropped: 1,
                events_truncated: true,
            }),
        ),
        (
            "pendingFrontendTaskResult",
            "FrontendTaskResult",
            value(&FrontendTaskResult {
                task_id: "task-2".into(),
                status: None,
                outcome: None,
                events: Vec::new(),
                events_dropped: 0,
                events_truncated: false,
            }),
        ),
        (
            "frontendTaskRun",
            "FrontendTaskRun",
            value(&FrontendTaskRun {
                task_id: "task-1".into(),
            }),
        ),
        (
            "taskResultRequest",
            "TaskResultRequest",
            value(&TaskResultRequest {
                task_id: "task-1".into(),
            }),
        ),
        (
            "resourceBytes",
            "ResourceBytes",
            value(&ResourceBytes {
                resource: resource_ref(),
                bytes: vec![1, 2, 3],
                media_type: Some("application/octet-stream".into()),
            }),
        ),
        (
            "resourceChunk",
            "ResourceChunk",
            value(&ResourceChunk {
                resource: resource_ref(),
                offset: 0,
                total_bytes: 3,
                bytes: vec![1, 2, 3],
                eof: true,
            }),
        ),
        (
            "resourceText",
            "ResourceText",
            value(&ResourceText {
                ref_id: "ref-1".into(),
                text: "hello".into(),
            }),
        ),
        (
            "previewHandle",
            "PreviewHandle",
            value(&PreviewHandle {
                ref_id: "ref-1".into(),
                token: "preview-token".into(),
                url: "mutsuki://preview/ref-1".into(),
                expires_at_unix_secs: 1_700_000_000,
            }),
        ),
        (
            "approvalRequest",
            "ApprovalRequest",
            value(&approval_request()),
        ),
        (
            "approvalResponse",
            "ApprovalResponse",
            value(&ApprovalResponse {
                approval_id: "approval-1".into(),
                token: "token-1".into(),
                decision: ApprovalDecision::Deny,
                reason: Some("not now".into()),
                trace_id: Some("trace-1".into()),
                correlation_id: Some("corr-1".into()),
                context: Some(context()),
            }),
        ),
        (
            "pluginReloadRequest",
            "PluginReloadRequest",
            value(&PluginReloadRequest {
                enabled_plugin_ids: Some(vec!["fixture.plugin".into()]),
                configs: BTreeMap::from([("fixture.plugin".into(), json!({ "enabled": true }))]),
                drain_timeout_ms: 5_000,
            }),
        ),
        (
            "pluginReloadResult",
            "PluginReloadResult",
            value(&PluginReloadResult {
                plugins: vec![plugin_summary()],
                runners: vec![runner_summary()],
            }),
        ),
        (
            "hostStatus",
            "HostStatus",
            value(&HostStatus {
                app_name: "Fixture".into(),
                profile_id: "default".into(),
                mode: "embedded".into(),
                healthy: false,
                runtime: RuntimeHealth {
                    healthy: true,
                    status: "running".into(),
                    active_tasks: 1,
                    failed_tasks: 0,
                    error: Some("degraded".into()),
                },
                host: health(None),
                plugins_health: health(Some("plugin failed")),
                runners_health: health(None),
                recent_errors: vec![HostRecentError {
                    source: "mutsuki_tauri_host.plugin".into(),
                    message: "plugin failed".into(),
                    timestamp_ms: 1_700_000_000_000,
                    plugin_id: Some("fixture.plugin".into()),
                    runner_id: Some("fixture.runner".into()),
                    task_id: Some("task-1".into()),
                    code: Some("runtime.host_failed".into()),
                    route: Some("fixture.route".into()),
                }],
                plugins: vec![plugin_summary()],
                runners: vec![runner_summary()],
            }),
        ),
        (
            "frontendLogRecord",
            "FrontendLogRecord",
            value(&log_record()),
        ),
        (
            "frontendEvents",
            "MutsukiFrontendEvent[]",
            value(&frontend_events()),
        ),
    ]
}

fn render() -> String {
    let fixtures = fixtures();
    let mut imports = fixtures
        .iter()
        .map(|(_, ty, _)| ty.trim_end_matches("[]"))
        .collect::<Vec<_>>();
    imports.sort_unstable();
    imports.dedup();
    let mut out = String::from(
        "// Generated by `cargo test -p mutsuki-tauri-bridge --test ts_contract`; do not edit.\n\
         // Regenerate with MUTSUKI_BLESS_TS_CONTRACT=1. `pnpm typecheck` proves every value the Rust\n\
         // bridge sends still satisfies the hand-written types in ../src/types.ts.\n\n",
    );
    out.push_str("import type {\n");
    for ty in imports {
        writeln!(out, "  {ty},").expect("write to String");
    }
    out.push_str("} from \"../src/types\";\n");
    for (name, ty, value) in fixtures {
        let literal = serde_json::to_string_pretty(&value).expect("fixture renders");
        writeln!(
            out,
            "
export const {name} = {literal} satisfies {ty};"
        )
        .expect("write to String");
    }
    out
}

#[test]
fn tauri_client_contract_fixtures_are_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/tauri-client/contract/fixtures.generated.ts");
    let expected = render();
    if std::env::var_os(BLESS_VAR).is_some() {
        std::fs::create_dir_all(path.parent().expect("contract dir")).expect("create contract dir");
        std::fs::write(&path, &expected).expect("write contract fixtures");
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "{} is stale; rerun with {BLESS_VAR}=1 and commit the result",
        path.display()
    );
}
