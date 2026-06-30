use crate::MutsukiTauriHost;
use crate::echo::ECHO_PROTOCOL_ID;
use mutsuki_runtime_contracts::TaskOutcome;
use mutsuki_tauri_bridge::FrontendTaskRequest;
use serde_json::json;

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
