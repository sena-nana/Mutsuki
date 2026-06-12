use super::fixtures::*;
use crate::{NativeOperation, NativeRuntimeHost};
use mutsuki_runtime_contracts::{RuntimeEventKind, ScalarValue};
use mutsuki_runtime_core::{AgentRuntime, BackendPayload};
use serde_json::json;

#[test]
fn native_host_runs_agent_input_and_operation_without_python() {
    let mut runtime = AgentRuntime::new();
    let mut host = NativeRuntimeHost::new();
    host.register_source(source_snapshot("source:test"));
    host.register_operation(NativeOperation::new(
        operation_descriptor("native.echo"),
        |payload| Ok(BackendPayload::Json(payload)),
    ));

    host.start_agent(&mut runtime, agent()).unwrap();
    assert_eq!(host.awake_count(), 1);

    assert_eq!(runtime.publish(envelope()).unwrap(), vec!["native-agent"]);
    runtime.tick_once("native-agent", &mut host).unwrap();
    assert_eq!(host.received_inputs().len(), 1);

    let result = runtime
        .invoke_operation(
            "native-agent",
            "native.echo",
            json!({"value": "ok"}),
            &mut host,
        )
        .unwrap();
    assert_eq!(result, BackendPayload::Json(json!({"value": "ok"})));

    runtime.stop_agent("native-agent", &mut host).unwrap();
    assert_eq!(host.stop_count(), 1);
}

#[test]
fn native_host_exposes_core_runtime_events_after_driving_agent() {
    let mut runtime = AgentRuntime::new();
    let mut host = NativeRuntimeHost::new();
    host.register_source(source_snapshot("source:test"));

    host.start_agent(&mut runtime, agent()).unwrap();
    runtime.publish(envelope()).unwrap();

    let events = runtime.events();
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    let publish = events
        .iter()
        .find(|event| event.name == "runtime.publish")
        .unwrap();
    assert_eq!(publish.kind, RuntimeEventKind::Routing);
    assert!(publish.error.is_none());
    assert_eq!(
        publish.attributes.get("source_id"),
        Some(&ScalarValue::String("source:test".into()))
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == RuntimeEventKind::Trace && event.name == "trace.span")
    );
}

#[test]
fn native_host_rejects_stale_operation_generation() {
    let mut runtime = AgentRuntime::new();
    let mut host = NativeRuntimeHost::new();
    host.register_source(source_snapshot("source:test"));
    host.register_operation(NativeOperation::new(
        operation_descriptor("native.echo"),
        |payload| Ok(BackendPayload::Json(payload)),
    ));

    host.start_agent(&mut runtime, agent()).unwrap();
    host.set_operation_generation("native.echo", 1);

    let err = runtime
        .invoke_operation("native-agent", "native.echo", json!({}), &mut host)
        .unwrap_err();
    assert_eq!(
        err.error().code,
        mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_GENERATION_MISMATCH
    );
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "operation.invoke.error")
    );
}
