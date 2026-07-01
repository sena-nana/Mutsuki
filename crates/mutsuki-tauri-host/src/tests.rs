use crate::MutsukiTauriHost;
use crate::echo::ECHO_PROTOCOL_ID;
use mutsuki_runtime_contracts::{
    ExecutionClass, RunnerDescriptor, RunnerPurity, RunnerResult, RuntimeEventKind, Task,
    TaskOutcome,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_tauri_bridge::{FrontendTaskRequest, MutsukiFrontendEvent, TaskCancelRequest};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[test]
fn default_host_runs_echo_task() {
    let host = MutsukiTauriHost::builder()
        .app_name("MutsukiTauriHostTest")
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
                if span.name == "runner.step" && span.trace_id == "trace:test:observe"
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

    fn step(&mut self, _ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        self.started_tx.send(()).expect("started signal sends");
        self.release_rx.recv().expect("runner release received");
        Ok(tasks
            .into_iter()
            .map(|task| RunnerResult::completed(task.task_id))
            .collect())
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
