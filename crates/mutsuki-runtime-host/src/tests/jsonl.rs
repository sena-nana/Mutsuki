use std::io::Cursor;

use super::fixtures::*;
use crate::JsonlRuntimeBackend;
use mutsuki_runtime_contracts::{
    OperationStatus, RuntimeError, StrategyResult, StrategyResultStatus,
};
use mutsuki_runtime_core::{
    AgentRuntime, BackendPayload, OperationBackend, ResourceBackend, StrategyBackend,
};
use serde_json::json;

#[test]
fn jsonl_capability_backend_decodes_strategy_and_writes_request() {
    let response = jsonl_response(json!(StrategyResult::wait_input()));
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let mut backend = JsonlRuntimeBackend::new(reader, writer);

    let result = backend.next_step("agent-a").unwrap();
    assert_eq!(
        result.status,
        mutsuki_runtime_contracts::StrategyResultStatus::WaitInput
    );

    let (_reader, writer) = backend.into_inner();
    let request: serde_json::Value = serde_json::from_slice(&writer).unwrap();
    assert_eq!(request["method"], "next_step");
    assert_eq!(request["params"]["agent_id"], "agent-a");
}

#[test]
fn jsonl_capability_backend_maps_failure_response_to_runtime_failure() {
    let response = jsonl_failure_response(RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED,
        "python_stdio",
        "python.invoke.test",
    ));
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let mut backend = JsonlRuntimeBackend::new(reader, writer);

    let err = backend.on_awake("agent-a").unwrap_err();
    assert_eq!(
        err.error().code,
        mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED
    );
}

#[test]
fn jsonl_capability_backend_invokes_operation_as_json_payload() {
    let response = jsonl_response(json!({"value": "ok"}));
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let mut backend = JsonlRuntimeBackend::new(reader, writer);
    let key = operation_key("test.echo");

    let result = backend
        .invoke("agent-a", &key, json!({"value": "input"}))
        .unwrap();

    assert_eq!(result, BackendPayload::Json(json!({"value": "ok"})));
}

#[test]
fn jsonl_capability_backend_lists_operation_snapshots() {
    let snapshot = operation_snapshot("test.echo", OperationStatus::Active);
    let response = jsonl_response(json!([snapshot]));
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let backend = JsonlRuntimeBackend::new(reader, writer);

    let operations = backend.list_operations(&["test".to_string()]).unwrap();

    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].descriptor.op_id, "test.echo");
}

#[test]
fn jsonl_capability_backend_drives_agent_runtime_lifecycle_with_scripted_responses() {
    let response = jsonl_scripted_responses([
        json!({"id": "req-1", "ok": true, "result": null}),
        json!({"id": "req-2", "ok": true, "result": [plugin_snapshot("native")]}),
        json!({"id": "req-3", "ok": true, "result": []}),
        json!({"id": "req-4", "ok": true, "result": [source_snapshot("source:test")]}),
        json!({"id": "req-5", "ok": true, "result": StrategyResult::wait_input()}),
        json!({"id": "req-6", "ok": true, "result": null}),
    ]);
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let mut backend = JsonlRuntimeBackend::new(reader, writer);
    let mut runtime = AgentRuntime::new();

    runtime.register_agent(agent()).unwrap();
    runtime.start_agent("native-agent", &mut backend).unwrap();
    assert_eq!(
        runtime.source_snapshots("native-agent").unwrap()[0]
            .descriptor
            .source_id,
        "source:test"
    );

    assert_eq!(runtime.publish(envelope()).unwrap(), vec!["native-agent"]);
    let result = runtime.tick_once("native-agent", &mut backend).unwrap();
    runtime.stop_agent("native-agent", &mut backend).unwrap();

    assert_eq!(result.status, StrategyResultStatus::WaitInput);
    let events = runtime.events();
    assert!(events.iter().any(|event| event.name == "agent.awake"));
    assert!(events.iter().any(|event| event.name == "runtime.publish"));
    assert!(events.iter().any(|event| event.name == "agent.input"));
    assert!(events.iter().any(|event| event.name == "agent.stop"));
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );

    let (_reader, writer) = backend.into_inner();
    let requests = written_requests(writer);
    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "on_awake",
            "list_plugins",
            "list_operations",
            "list_sources",
            "on_input",
            "on_stop",
        ]
    );
    assert_eq!(
        requests[4]["params"]["envelope"]["source"]["source_id"],
        "source:test"
    );
}

#[test]
fn jsonl_capability_backend_operation_status_preserves_explicit_not_found() {
    let response = jsonl_response(json!("not_found"));
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let backend = JsonlRuntimeBackend::new(reader, writer);
    let key = operation_key("test.echo");

    assert_eq!(
        backend.operation_status("agent-a", &key),
        OperationStatus::NotFound
    );
}

#[test]
fn jsonl_capability_backend_operation_status_failure_is_unhealthy() {
    let response = jsonl_failure_response(RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED,
        "python_stdio",
        "python.operation_status.test",
    ));
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let backend = JsonlRuntimeBackend::new(reader, writer);
    let key = operation_key("test.echo");

    assert_eq!(
        backend.operation_status("agent-a", &key),
        OperationStatus::Unhealthy
    );
}

#[test]
fn jsonl_capability_backend_operation_status_protocol_error_is_unhealthy() {
    let reader = Cursor::new(b"\n".to_vec());
    let writer = Vec::new();
    let backend = JsonlRuntimeBackend::new(reader, writer);
    let key = operation_key("test.echo");

    assert_eq!(
        backend.operation_status("agent-a", &key),
        OperationStatus::Unhealthy
    );
}

#[test]
fn jsonl_capability_backend_dispatches_resource_register_acquire_and_release() {
    let token = mutsuki_runtime_contracts::LeaseToken {
        token_id: "lease-1".into(),
        ref_id: "ref-1".into(),
        owner: "agent-a".into(),
    };
    let response = jsonl_scripted_responses([
        json!({"id": "req-1", "ok": true, "result": "ref-1"}),
        json!({"id": "req-2", "ok": true, "result": token}),
        json!({"id": "req-3", "ok": true, "result": null}),
    ]);
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let mut backend = JsonlRuntimeBackend::new(reader, writer);

    let registered = backend
        .register_resource(ref_descriptor("ref-1", "domain.resource"), "resource-host")
        .unwrap();
    let acquired = backend.acquire_resource("ref-1", "agent-a").unwrap();
    backend.release_resource(&acquired).unwrap();

    assert_eq!(registered, "ref-1");
    assert_eq!(acquired.token_id, "lease-1");
    let (_reader, writer) = backend.into_inner();
    let requests = written_requests(writer);
    assert_eq!(requests[0]["method"], "resource.register");
    assert_eq!(requests[0]["params"]["descriptor"]["ref_id"], "ref-1");
    assert_eq!(requests[0]["params"]["owner"], "resource-host");
    assert_eq!(requests[1]["method"], "resource.acquire");
    assert_eq!(requests[1]["params"]["requester"], "agent-a");
    assert_eq!(requests[2]["method"], "resource.release");
    assert_eq!(requests[2]["params"]["token"]["token_id"], "lease-1");
}

#[test]
fn jsonl_capability_backend_lists_resource_records_with_optional_owner_filter() {
    let all_records = vec![
        resource_record("ref-a", "domain.resource", "owner-a", 0),
        resource_record("ref-b", "domain.resource", "owner-b", 1),
    ];
    let owner_records = vec![resource_record("ref-b", "domain.resource", "owner-b", 1)];
    let response = jsonl_scripted_responses([
        json!({"id": "req-1", "ok": true, "result": all_records}),
        json!({"id": "req-2", "ok": true, "result": owner_records}),
    ]);
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let backend = JsonlRuntimeBackend::new(reader, writer);

    let all = backend.try_list_records(None).unwrap();
    let owner_b = backend.try_list_records(Some("owner-b")).unwrap();

    assert_eq!(all.len(), 2);
    assert_eq!(
        owner_b,
        vec![resource_record("ref-b", "domain.resource", "owner-b", 1)]
    );
    let (_reader, writer) = backend.into_inner();
    let requests = written_requests(writer);
    assert_eq!(requests[0]["method"], "resource.list");
    assert!(requests[0]["params"]["owner"].is_null());
    assert_eq!(requests[1]["method"], "resource.list");
    assert_eq!(requests[1]["params"]["owner"], "owner-b");
}

#[test]
fn jsonl_capability_backend_resource_failure_preserves_backend_error() {
    let response = jsonl_failure_response(RuntimeError::new(
        "ref.not_found",
        "python_resource_backend",
        "python.resource.acquire.ref-missing",
    ));
    let reader = Cursor::new(response.into_bytes());
    let writer = Vec::new();
    let mut backend = JsonlRuntimeBackend::new(reader, writer);

    let err = backend
        .acquire_resource("ref-missing", "agent-a")
        .unwrap_err();

    assert_eq!(err.error().code, "ref.not_found");
    assert_eq!(err.error().source, "python_resource_backend");
}
