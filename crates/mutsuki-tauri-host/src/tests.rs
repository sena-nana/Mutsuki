use crate::MutsukiTauriHost;
use crate::config::{MutsukiTauriConfig, PathsConfig};
use crate::echo::{ECHO_PROTOCOL_ID, EchoRunner};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, EntryCompletion, ExecutionClass, LifecyclePolicy,
    PermissionGrant, PluginArtifact, PluginManifest, PluginProvides, RunnerDescriptor,
    RunnerPurity, RunnerResult, RuntimeEventKind, TaskOutcome, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_tauri_bridge::{
    ApprovalAttribution, ApprovalDecision, ApprovalResponse, FrontendContext, FrontendTaskRequest,
    MutsukiFrontendEvent, TaskCancelRequest,
};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

#[test]
fn default_host_reports_empty_plugin_and_runner_state() {
    let workspace = TestWorkspace::new("empty-default");
    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .build()
        .expect("host builds without fake runner");

    assert!(host.plugins().is_empty());
    assert!(host.runners().is_empty());
    let status = host.status();
    assert!(status.healthy);
    assert!(status.runtime.healthy);
    assert!(status.host.healthy);
    assert!(status.plugins_health.healthy);
    assert!(status.runners_health.healthy);
    assert!(status.recent_errors.is_empty());
    assert!(status.plugins.is_empty());
    assert!(status.runners.is_empty());
}

#[test]
fn explicit_echo_runner_still_runs_task() {
    let workspace = TestWorkspace::new("explicit-echo");
    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .runner(Box::new(EchoRunner::new()))
        .build()
        .expect("host builds");

    let result = host
        .call(FrontendTaskRequest {
            protocol_id: ECHO_PROTOCOL_ID.into(),
            payload: json!({ "text": "hello" }),
            task_id: Some("task:test:echo".into()),
            trace_id: None,
            correlation_id: None,
            idempotency_key: None,
            input_refs: Vec::new(),
            priority: 0,
            context: Default::default(),
        })
        .expect("echo task completes");

    assert_eq!(result.task_id, "task:test:echo");
    assert!(matches!(
        result.outcome,
        Some(TaskOutcome::Completed { task_id, .. }) if task_id == "task:test:echo"
    ));
    assert!(result.events.iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == "task.enqueue"
            && event.subject_id.as_deref() == Some("task:test:echo")
    }));
    assert!(result.events.iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == "task.completed"
            && event.subject_id.as_deref() == Some("task:test:echo")
    }));
    assert!(
        result
            .events
            .iter()
            .all(|event| event.name != "task.submit")
    );
}

#[test]
fn host_emits_runtime_events_and_trace_spans_for_task() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostObserveTest")
        .runner(Box::new(EchoRunner::new()))
        .build()
        .expect("host builds");
    let mut rx = host.event_hub().subscribe();

    let result = host
        .call(FrontendTaskRequest {
            protocol_id: ECHO_PROTOCOL_ID.into(),
            payload: json!({ "text": "trace" }),
            task_id: Some("task:test:observe".into()),
            trace_id: Some("trace:test:observe".into()),
            correlation_id: None,
            idempotency_key: None,
            input_refs: Vec::new(),
            priority: 0,
            context: Default::default(),
        })
        .expect("echo task completes");

    assert!(result.events.iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == "task.completed"
            && event.subject_id.as_deref() == Some("task:test:observe")
    }));

    let envelopes = collect_events(&mut rx);
    assert!(envelopes.iter().any(|event| {
        matches!(
            &event.payload,
            MutsukiFrontendEvent::Task { task_id, event }
                if task_id == "task:test:observe" && event.name == "task.completed"
        )
    }));
    assert!(envelopes.iter().any(|event| {
        matches!(
            &event.payload,
            MutsukiFrontendEvent::Runtime { event }
                if event.kind == RuntimeEventKind::Trace && event.name == "trace.span"
        )
    }));
    assert!(envelopes.iter().any(|event| {
        matches!(
            &event.payload,
            MutsukiFrontendEvent::Trace { span }
                if span.name == "runner.run_batch" && span.trace_id == "trace:test:observe"
        )
    }));
}

#[test]
fn host_log_event_redacts_sensitive_fields() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostLogTest")
        .build()
        .expect("host builds");
    let mut rx = host.event_hub().subscribe();

    host.emit_log(
        "info",
        "test.observe",
        "structured log",
        BTreeMap::from([
            ("token".into(), json!("secret-token")),
            (
                "nested".into(),
                json!({
                    "password": "secret-password",
                    "visible": true
                }),
            ),
        ]),
    );

    let envelopes = collect_events(&mut rx);
    let record = envelopes
        .iter()
        .find_map(|event| match &event.payload {
            MutsukiFrontendEvent::Log { record } => Some(record),
            _ => None,
        })
        .expect("log event emitted");

    assert_eq!(record.fields.get("token"), Some(&json!("[redacted]")));
    assert_eq!(
        record
            .fields
            .get("nested")
            .and_then(|value| value.get("password")),
        Some(&json!("[redacted]"))
    );
    assert_eq!(
        record
            .fields
            .get("nested")
            .and_then(|value| value.get("visible")),
        Some(&json!(true))
    );
}

#[test]
fn approval_request_carries_attribution_to_pending_and_event() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostApprovalAttributionTest")
        .build()
        .expect("host builds");
    let mut rx = host.event_hub().subscribe();
    let context = FrontendContext {
        window_label: Some("main".into()),
        webview_id: Some("webview:main".into()),
        session_id: Some("session:test".into()),
        user_action_id: Some("action:delete".into()),
    };

    let request = host.request_approval_with_attribution(
        "fixture.plugin",
        "resource.delete",
        "high",
        json!({ "resource_ref": "ref:test" }),
        ApprovalAttribution {
            trace_id: "trace:approval".into(),
            correlation_id: "corr:approval".into(),
            context: context.clone(),
        },
    );

    assert_eq!(request.trace_id, "trace:approval");
    assert_eq!(request.correlation_id, "corr:approval");
    assert_eq!(request.context, context);

    let pending = host.pending_approvals();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].trace_id, "trace:approval");
    assert_eq!(pending[0].correlation_id, "corr:approval");
    assert_eq!(pending[0].context, context);

    let envelopes = collect_events(&mut rx);
    let event_request = envelopes
        .iter()
        .find_map(|event| match &event.payload {
            MutsukiFrontendEvent::Approval { request } => Some(request),
            _ => None,
        })
        .expect("approval event emitted");
    assert_eq!(event_request.approval_id, request.approval_id);
    assert_eq!(event_request.trace_id, "trace:approval");
    assert_eq!(event_request.correlation_id, "corr:approval");
    assert_eq!(event_request.context, context);
}

#[test]
fn approval_rejects_mismatched_token_or_attribution_without_consuming_pending() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostApprovalResolveTest")
        .build()
        .expect("host builds");
    let request = host.request_approval_with_attribution(
        "fixture.plugin",
        "resource.write",
        "medium",
        json!({ "resource_ref": "ref:test" }),
        ApprovalAttribution {
            trace_id: "trace:approval:resolve".into(),
            correlation_id: "corr:approval:resolve".into(),
            context: FrontendContext {
                session_id: Some("session:resolve".into()),
                user_action_id: Some("action:write".into()),
                ..FrontendContext::default()
            },
        },
    );

    assert!(
        host.resolve_approval(approval_response(&request, "wrong-token"))
            .is_err()
    );
    assert_eq!(host.pending_approvals().len(), 1);

    let mut trace_mismatch = approval_response(&request, &request.token);
    trace_mismatch.trace_id = Some("trace:mismatch".into());
    assert!(host.resolve_approval(trace_mismatch).is_err());
    assert_eq!(host.pending_approvals().len(), 1);

    let mut correlation_mismatch = approval_response(&request, &request.token);
    correlation_mismatch.correlation_id = Some("corr:mismatch".into());
    assert!(host.resolve_approval(correlation_mismatch).is_err());
    assert_eq!(host.pending_approvals().len(), 1);

    let decision = host
        .resolve_approval(approval_response(&request, &request.token))
        .expect("matching approval resolves");
    assert_eq!(decision, ApprovalDecision::Allow);
    assert!(host.pending_approvals().is_empty());
}

#[test]
fn legacy_approval_request_generates_fallback_trace_and_preserves_context() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostApprovalFallbackTest")
        .build()
        .expect("host builds");
    let context = FrontendContext {
        session_id: Some("session:legacy".into()),
        user_action_id: Some("action:legacy".into()),
        ..FrontendContext::default()
    };

    let request = host.request_approval(
        "fixture.plugin",
        "resource.import",
        "low",
        json!({ "resource_ref": "ref:legacy" }),
        context.clone(),
    );

    assert!(request.trace_id.starts_with("approval-trace:"));
    assert!(request.correlation_id.starts_with("approval-correlation:"));
    assert_eq!(request.context, context);
}

#[test]
fn loader_records_malformed_plugin_manifest() {
    let workspace = TestWorkspace::new("malformed-plugin");
    let plugin_dir = workspace.config.paths.plugins_dir.join("bad");
    std::fs::create_dir_all(&plugin_dir).expect("plugin dir created");
    std::fs::write(plugin_dir.join("plugin.toml"), "plugin_id = [")
        .expect("plugin manifest written");

    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .build()
        .expect("host builds with failed plugin state");

    let plugins = host.plugins();
    let status = host.status();
    assert_eq!(plugins.len(), 1);
    assert!(!plugins[0].enabled);
    assert_eq!(plugins[0].status, "failed");
    assert!(
        plugins[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("failed to parse"))
    );
    assert!(!status.healthy);
    assert!(!status.plugins_health.healthy);
    assert!(status.recent_errors.iter().any(|error| {
        error.source == "mutsuki_tauri_host.plugin"
            && error.plugin_id == Some(plugins[0].plugin_id.clone())
    }));
}

#[test]
fn loader_records_missing_runner_spec() {
    let workspace = TestWorkspace::new("missing-runner");
    write_plugin_manifest(
        &workspace.config.paths.plugins_dir.join("plugin"),
        plugin_manifest(
            "fixture.missing_runner",
            "1.0.0",
            ArtifactType::Process,
            vec![runner_descriptor(
                "fixture.missing_runner.runner",
                "fixture.missing",
            )],
        ),
    );

    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .build()
        .expect("host builds with failed runner state");

    let plugins = host.plugins();
    let runners = host.runners();
    let status = host.status();
    assert_eq!(plugins[0].plugin_id, "fixture.missing_runner");
    assert!(!plugins[0].enabled);
    assert!(
        plugins[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("missing runner.toml"))
    );
    assert!(!status.healthy);
    assert!(!status.plugins_health.healthy);
    assert!(!status.runners_health.healthy);
    assert!(status.runners.iter().any(|runner| {
        runner.runner_id == "fixture.missing_runner.runner" && runner.status == "failed"
    }));
    assert_eq!(runners[0].runner_id, "fixture.missing_runner.runner");
    assert!(!runners[0].enabled);
}

#[test]
fn loader_records_unsupported_plugin_artifact() {
    let workspace = TestWorkspace::new("unsupported-artifact");
    write_plugin_manifest(
        &workspace.config.paths.plugins_dir.join("plugin"),
        plugin_manifest(
            "fixture.unsupported",
            "1.0.0",
            ArtifactType::Native,
            Vec::new(),
        ),
    );

    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .build()
        .expect("host builds with failed plugin state");

    let plugins = host.plugins();
    assert_eq!(plugins[0].plugin_id, "fixture.unsupported");
    assert!(!plugins[0].enabled);
    assert!(
        plugins[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unsupported"))
    );
}

#[test]
fn loader_records_duplicate_plugin_and_runner_ids() {
    let workspace = TestWorkspace::new("duplicates");
    write_plugin_manifest(
        &workspace.config.paths.plugins_dir.join("one"),
        plugin_manifest(
            "fixture.duplicate",
            "1.0.0",
            ArtifactType::Process,
            Vec::new(),
        ),
    );
    write_plugin_manifest(
        &workspace.config.paths.plugins_dir.join("two"),
        plugin_manifest(
            "fixture.duplicate",
            "2.0.0",
            ArtifactType::Process,
            Vec::new(),
        ),
    );
    write_runner_spec(
        &workspace.config.paths.runners_dir.join("one"),
        &runner_launch_spec(
            "fixture.duplicate.runner",
            "fixture.duplicate",
            "powershell.exe",
            Vec::new(),
        ),
    );
    write_runner_spec(
        &workspace.config.paths.runners_dir.join("two"),
        &runner_launch_spec(
            "fixture.duplicate.runner",
            "fixture.duplicate",
            "powershell.exe",
            Vec::new(),
        ),
    );

    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .build()
        .expect("host builds with duplicate state");

    assert!(host.plugins().iter().any(|plugin| {
        plugin.plugin_id == "fixture.duplicate"
            && !plugin.enabled
            && plugin
                .error
                .as_deref()
                .is_some_and(|error| error.contains("duplicate"))
    }));
    assert!(host.runners().iter().any(|runner| {
        runner.runner_id == "fixture.duplicate.runner"
            && !runner.enabled
            && runner
                .error
                .as_deref()
                .is_some_and(|error| error.contains("duplicate"))
    }));
}

#[test]
fn external_process_runner_completes_task_and_forwards_stderr() {
    let workspace = TestWorkspace::new("external-runner");
    let script_path = workspace.root.join("jsonl-runner.ps1");
    std::fs::write(&script_path, jsonl_runner_script()).expect("runner script written");
    write_plugin_manifest(
        &workspace.config.paths.plugins_dir.join("plugin"),
        plugin_manifest(
            "fixture.process",
            "2.3.4",
            ArtifactType::Process,
            vec![runner_descriptor(
                "fixture.process.runner",
                "fixture.process.echo",
            )],
        ),
    );
    write_runner_spec(
        &workspace.config.paths.runners_dir.join("runner"),
        &runner_launch_spec(
            "fixture.process.runner",
            "fixture.process",
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script_path.display().to_string(),
            ],
        ),
    );

    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .build()
        .expect("host builds with external runner");
    let mut rx = host.event_hub().subscribe();

    let result = host
        .call(FrontendTaskRequest {
            protocol_id: "fixture.process.echo".into(),
            payload: json!({ "message": "hello" }),
            task_id: Some("task:process".into()),
            trace_id: None,
            correlation_id: None,
            idempotency_key: None,
            input_refs: Vec::new(),
            priority: 0,
            context: Default::default(),
        })
        .expect("external process task completes");

    assert!(matches!(
        result.outcome,
        Some(TaskOutcome::Completed { task_id, .. }) if task_id == "task:process"
    ));
    assert!(host.plugins().iter().any(|plugin| {
        plugin.plugin_id == "fixture.process"
            && plugin.version == "2.3.4"
            && plugin.enabled
            && plugin.deployment == "process"
    }));
    assert!(host.runners().iter().any(|runner| {
        runner.runner_id == "fixture.process.runner"
            && runner.enabled
            && runner.deployment == "process"
    }));

    let envelopes = collect_events_until(&mut rx, Duration::from_secs(1), |events| {
        events.iter().any(|event| {
            matches!(
                &event.payload,
                MutsukiFrontendEvent::Log { record }
                    if record.target == "mutsuki_tauri_host.runner"
                        && record.fields.get("runner_id") == Some(&json!("fixture.process.runner"))
            )
        })
    });
    let runner_log = envelopes
        .iter()
        .find_map(|event| match &event.payload {
            MutsukiFrontendEvent::Log { record }
                if record.target == "mutsuki_tauri_host.runner" =>
            {
                Some(record)
            }
            _ => None,
        })
        .expect("runner stderr log forwarded");
    assert!(!runner_log.message.contains("secret-token"));
    assert!(envelopes.iter().any(|event| {
        matches!(
            &event.payload,
            MutsukiFrontendEvent::Runner { runner_id, status }
                if runner_id == "fixture.process.runner" && status == "stderr"
        )
    }));
}

#[test]
fn health_reports_external_runner_runtime_failure() {
    let workspace = TestWorkspace::new("external-runner-failure");
    let script_path = workspace.root.join("jsonl-runner-fails.ps1");
    std::fs::write(&script_path, failing_jsonl_runner_script()).expect("runner script written");
    write_plugin_manifest(
        &workspace.config.paths.plugins_dir.join("plugin"),
        plugin_manifest(
            "fixture.failing_process",
            "1.0.0",
            ArtifactType::Process,
            vec![runner_descriptor(
                "fixture.failing_process.runner",
                "fixture.failing_process.echo",
            )],
        ),
    );
    write_runner_spec(
        &workspace.config.paths.runners_dir.join("runner"),
        &runner_launch_spec(
            "fixture.failing_process.runner",
            "fixture.failing_process",
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script_path.display().to_string(),
            ],
        ),
    );

    let host = MutsukiTauriHost::builder()
        .config(workspace.config())
        .build()
        .expect("host builds with external runner");

    let result = host
        .call(FrontendTaskRequest {
            protocol_id: "fixture.failing_process.echo".into(),
            payload: json!({ "message": "hello" }),
            task_id: Some("task:failing-process".into()),
            trace_id: None,
            correlation_id: None,
            idempotency_key: None,
            input_refs: Vec::new(),
            priority: 0,
            context: Default::default(),
        })
        .expect("runner failure is reported as a task outcome");

    assert!(matches!(
        result.outcome,
        Some(TaskOutcome::Failed { task_id, .. }) if task_id == "task:failing-process"
    ));
    let status = host.status();
    assert!(!status.healthy);
    assert!(status.runtime.healthy);
    assert_eq!(status.runtime.failed_tasks, 1);
    assert!(!status.plugins_health.healthy);
    assert!(!status.runners_health.healthy);
    assert!(status.plugins.iter().any(|plugin| {
        plugin.plugin_id == "fixture.failing_process" && plugin.status == "degraded"
    }));
    assert!(status.runners.iter().any(|runner| {
        runner.runner_id == "fixture.failing_process.runner" && runner.status == "failed"
    }));
    assert!(status.recent_errors.iter().any(|error| {
        error.runner_id.as_deref() == Some("fixture.failing_process.runner")
            && error.code.as_deref() == Some("fixture.runner_failed")
    }));
}

#[test]
fn streaming_task_can_be_cancelled_while_runner_is_still_running() {
    let descriptor = RunnerDescriptor {
        runner_id: "stream.blocking.runner".into(),
        plugin_id: "stream.blocking.plugin".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec!["stream.blocking".into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Blocking,
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec!["task_protocol:stream.blocking".into()],
    };
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostStreamTest")
        .runner(Box::new(BlockingRunner {
            descriptor,
            started_tx,
            release_rx,
            cancelled: cancelled.clone(),
        }))
        .build()
        .expect("host builds");
    let mut rx = host.event_hub().subscribe();

    let run = host
        .start_task(FrontendTaskRequest {
            protocol_id: "stream.blocking".into(),
            payload: json!({}),
            task_id: Some("stream-cancel".into()),
            trace_id: None,
            correlation_id: None,
            idempotency_key: None,
            input_refs: Vec::new(),
            priority: 0,
            context: Default::default(),
        })
        .expect("task starts");

    assert_eq!(run.task_id, "stream-cancel");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("runner starts");

    let cancelled_task = host
        .cancel_task(TaskCancelRequest {
            task_id: "stream-cancel".into(),
            reason: Some("test".into()),
        })
        .expect("task cancels while runner is running");
    assert_eq!(cancelled_task, "stream-cancel");

    let result = host
        .task_result(mutsuki_tauri_bridge::TaskResultRequest {
            task_id: "stream-cancel".into(),
        })
        .expect("cancelled result resolves");
    assert!(matches!(
        result.outcome,
        Some(TaskOutcome::Cancelled { task_id, .. }) if task_id == "stream-cancel"
    ));
    assert!(result.events.iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == "task.cancelled"
            && event.subject_id.as_deref() == Some("stream-cancel")
    }));
    assert!(
        result
            .events
            .iter()
            .all(|event| event.name != "task.submit")
    );
    assert!(
        cancelled
            .lock()
            .expect("cancelled mutex poisoned")
            .is_empty()
    );

    release_tx.send(()).expect("runner releases");
    let envelopes = collect_events(&mut rx);
    assert!(envelopes.iter().any(|event| {
        matches!(
            &event.payload,
            MutsukiFrontendEvent::Task { task_id, event }
                if task_id == "stream-cancel" && event.name == "task.cancelled"
        )
    }));
}

struct BlockingRunner {
    descriptor: RunnerDescriptor,
    started_tx: mpsc::Sender<()>,
    release_rx: mpsc::Receiver<()>,
    cancelled: Arc<Mutex<Vec<String>>>,
}

impl Runner for BlockingRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.started_tx.send(()).expect("started signal sends");
        self.release_rx.recv().expect("runner release received");
        let tasks = match batch.row_payload_tasks() {
            Ok(tasks) => tasks,
            Err(error) => return Ok(CompletionBatch::from_error(&batch, error)),
        };
        let results = batch
            .entries
            .iter()
            .map(|entry| {
                let result = tasks
                    .iter()
                    .find(|task| task.task_id == entry.task_id)
                    .map(|task| RunnerResult::completed(task.task_id.clone()));
                EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result,
                    error: None,
                }
            })
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.cancelled
            .lock()
            .expect("cancelled mutex poisoned")
            .push(invocation_id.to_string());
        Ok(())
    }
}

fn collect_events(
    rx: &mut tokio::sync::broadcast::Receiver<mutsuki_tauri_bridge::FrontendEventEnvelope>,
) -> Vec<mutsuki_tauri_bridge::FrontendEventEnvelope> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

fn approval_response(
    request: &mutsuki_tauri_bridge::ApprovalRequest,
    token: &str,
) -> ApprovalResponse {
    ApprovalResponse {
        approval_id: request.approval_id.clone(),
        token: token.into(),
        decision: ApprovalDecision::Allow,
        reason: None,
        trace_id: Some(request.trace_id.clone()),
        correlation_id: Some(request.correlation_id.clone()),
        context: Some(request.context.clone()),
    }
}

fn collect_events_until(
    rx: &mut tokio::sync::broadcast::Receiver<mutsuki_tauri_bridge::FrontendEventEnvelope>,
    timeout: Duration,
    done: impl Fn(&[mutsuki_tauri_bridge::FrontendEventEnvelope]) -> bool,
) -> Vec<mutsuki_tauri_bridge::FrontendEventEnvelope> {
    let deadline = std::time::Instant::now() + timeout;
    let mut events = Vec::new();
    while std::time::Instant::now() < deadline {
        events.extend(collect_events(rx));
        if done(&events) {
            return events;
        }
        thread::sleep(Duration::from_millis(20));
    }
    events.extend(collect_events(rx));
    events
}

struct TestWorkspace {
    root: PathBuf,
    config: MutsukiTauriConfig,
}

impl TestWorkspace {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("mutsuki-tauri-host-{name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test workspace root created");
        let paths = PathsConfig {
            app_data_dir: root.clone(),
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            logs_dir: root.join("logs"),
            plugins_dir: root.join("plugins"),
            resources_dir: root.join("resources"),
            runners_dir: root.join("runners"),
        };
        let mut config = MutsukiTauriConfig::for_app(format!("MutsukiTauriHostTest-{name}"));
        config.paths = paths;
        Self { root, config }
    }

    fn config(&self) -> MutsukiTauriConfig {
        self.config.clone()
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write_plugin_manifest(dir: &Path, manifest: PluginManifest) {
    std::fs::create_dir_all(dir).expect("plugin dir created");
    let text = toml::to_string(&manifest).expect("plugin manifest serializes");
    std::fs::write(dir.join("plugin.toml"), text).expect("plugin manifest written");
}

fn plugin_manifest(
    plugin_id: &str,
    version: &str,
    artifact_type: ArtifactType,
    runners: Vec<RunnerDescriptor>,
) -> PluginManifest {
    PluginManifest {
        plugin_id: plugin_id.into(),
        version: version.into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type,
            path: "process".into(),
            sha256: "sha256:test".into(),
        },
        provides: PluginProvides {
            runners,
            ..PluginProvides::default()
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: Vec::new(),
            resources: Vec::new(),
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "drain_and_swap".into(),
            unload_timeout_ms: 5000,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: false,
        },
        metadata: BTreeMap::new(),
    }
}

fn runner_descriptor(runner_id: &str, protocol_id: &str) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: runner_id.into(),
        plugin_id: runner_id
            .rsplit_once('.')
            .map(|(plugin, _)| plugin)
            .unwrap_or(runner_id)
            .into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![protocol_id.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Io,
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("task_protocol:{protocol_id}")],
    }
}

#[derive(Serialize)]
struct RunnerSpecFixture {
    runner_id: String,
    plugin_id: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    env_inherit: Vec<String>,
}

fn runner_launch_spec(
    runner_id: &str,
    plugin_id: &str,
    command: &str,
    args: Vec<String>,
) -> RunnerSpecFixture {
    RunnerSpecFixture {
        runner_id: runner_id.into(),
        plugin_id: plugin_id.into(),
        command: command.into(),
        args,
        env: BTreeMap::new(),
        env_inherit: vec![
            "PATH".into(),
            "SystemRoot".into(),
            "WINDIR".into(),
            "ComSpec".into(),
            "PATHEXT".into(),
        ],
    }
}

fn write_runner_spec(dir: &Path, spec: &RunnerSpecFixture) {
    std::fs::create_dir_all(dir).expect("runner dir created");
    let text = toml::to_string(spec).expect("runner spec serializes");
    std::fs::write(dir.join("runner.toml"), text).expect("runner spec written");
}

fn jsonl_runner_script() -> &'static str {
    r#"
$ErrorActionPreference = 'Stop'
while (($line = [Console]::In.ReadLine()) -ne $null) {
  if ($line.Trim().Length -eq 0) { continue }
  $request = $line | ConvertFrom-Json
  if ($request.method -eq 'runner.run_batch') {
    [Console]::Error.WriteLine('runner stderr token=secret-token')
    $batch = $request.params.batch
    $completions = @()
    foreach ($entry in @($batch.entries)) {
      $completions += @{
        entry_id = $entry.entry_id
        task_id = $entry.task_id
        result = @{
          task_id = $entry.task_id
          deltas = @()
          events = @()
          tasks = @()
          effects = @()
          values = @()
          resources = @()
          task_await = $null
          status = 'completed'
        }
        error = $null
      }
    }
    $result = @{
      batch_id = $batch.batch_id
      tick_id = $batch.tick_id
      results = @($completions)
      metadata = @()
    }
    @{ id = $request.id; ok = $true; result = $result } | ConvertTo-Json -Depth 20 -Compress
  } elseif ($request.method -eq 'runner.cancel') {
    @{ id = $request.id; ok = $true; result = $null } | ConvertTo-Json -Depth 20 -Compress
  } elseif ($request.method -eq 'runner.dispose') {
    @{ id = $request.id; ok = $true; result = $null } | ConvertTo-Json -Depth 20 -Compress
    break
  } else {
    @{ id = $request.id; ok = $false; error = @{ code = 'test.unsupported'; source = 'test'; route = $request.method; lost_capability = $null; recovery = $null; cause = $null; evidence = @{} } } | ConvertTo-Json -Depth 20 -Compress
  }
}
"#
}

fn failing_jsonl_runner_script() -> &'static str {
    r#"
$ErrorActionPreference = 'Stop'
while (($line = [Console]::In.ReadLine()) -ne $null) {
  if ($line.Trim().Length -eq 0) { continue }
  $request = $line | ConvertFrom-Json
  if ($request.method -eq 'runner.run_batch') {
    @{
      id = $request.id
      ok = $false
      error = @{
        code = 'fixture.runner_failed'
        source = 'fixture.runner'
        route = 'runner.run_batch'
        lost_capability = $null
        recovery = $null
        cause = $null
        evidence = @{
          plugin_id = 'fixture.failing_process'
          runner_id = 'fixture.failing_process.runner'
        }
      }
    } | ConvertTo-Json -Depth 20 -Compress
  } elseif ($request.method -eq 'runner.dispose') {
    @{ id = $request.id; ok = $true; result = $null } | ConvertTo-Json -Depth 20 -Compress
    break
  } else {
    @{ id = $request.id; ok = $true; result = $null } | ConvertTo-Json -Depth 20 -Compress
  }
}
"#
}

#[tokio::test]
async fn resource_store_round_trips_written_bytes() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostTest")
        .build()
        .expect("host builds");
    let resource = host
        .resource_store()
        .create_blob("text/plain", b"before".to_vec(), Some("text/plain".into()))
        .await
        .expect("resource created");

    host.write_resource_bytes(&resource.ref_id, b"after".to_vec())
        .await
        .expect("resource updated");
    let text = host
        .read_resource_text(&resource.ref_id)
        .await
        .expect("resource text readable");

    assert_eq!(text.text, "after");
}

#[tokio::test]
async fn host_resource_provider_id_matches_registered_gateway() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostProviderIdTest")
        .build()
        .expect("host builds");
    let resource = host
        .resource_store()
        .create_blob(
            "text/plain",
            b"provider-id".to_vec(),
            Some("text/plain".into()),
        )
        .await
        .expect("resource created");
    assert_eq!(resource.provider_id, mutsuki_tauri_resource::PROVIDER_ID);

    let preview = host
        .create_preview_handle(&resource.ref_id)
        .expect("preview handle created");
    assert!(preview.url.starts_with("mutsuki-resource://"));
    assert_eq!(preview.ref_id, resource.ref_id);
}
