use super::fixtures::*;
use crate::*;
use mutsukicore_runtime_contracts::*;
use serde_json::json;

#[test]
fn runtime_routes_and_ticks_agent_input() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();
    assert_eq!(
        runtime.source_snapshots("agent-a").unwrap()[0]
            .descriptor
            .source_id,
        "source:default"
    );

    let matched = runtime.publish(envelope()).unwrap();
    assert_eq!(matched, vec!["agent-a".to_string()]);
    assert_eq!(runtime.inbox_len("agent-a"), Some(1));

    let result = runtime.tick_once("agent-a", &mut backend).unwrap();
    assert_eq!(result.status, StrategyResultStatus::Completed);
    assert_eq!(backend.inputs, 1);
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|s| s.name == "agent.input")
    );
    let input_span = runtime
        .trace_spans()
        .iter()
        .find(|span| span.name == "agent.input")
        .unwrap();
    let strategy_span = runtime
        .trace_spans()
        .iter()
        .find(|span| span.name == "agent.strategy")
        .unwrap();
    assert_eq!(
        strategy_span.parent_span_id.as_deref(),
        Some(input_span.span_id.as_str())
    );
}

#[test]
fn runtime_records_input_backend_failure_as_error_event() {
    let mut runtime = AgentRuntime::new();
    let mut backend = TestBackend {
        fail_input: true,
        ..backend()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();
    runtime.publish(envelope()).unwrap();

    let err = runtime.tick_once("agent-a", &mut backend).unwrap_err();

    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    let events = runtime.events();
    assert!(!events.iter().any(|event| event.name == "agent.input"));
    let event = events
        .iter()
        .find(|event| event.name == "agent.input.error")
        .unwrap();
    assert_eq!(
        event.error.as_ref().unwrap().code,
        ERR_RUNTIME_BACKEND_FAILED
    );
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "agent.strategy" && span.status == SpanStatus::Error)
    );
}

#[test]
fn runtime_records_next_step_backend_failure_as_error_event() {
    let mut runtime = AgentRuntime::new();
    let mut backend = TestBackend {
        fail_next_step: true,
        ..backend()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let err = runtime.tick_once("agent-a", &mut backend).unwrap_err();

    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    let events = runtime.events();
    assert!(!events.iter().any(|event| event.name == "agent.next_step"));
    let event = events
        .iter()
        .find(|event| event.name == "agent.next_step.error")
        .unwrap();
    assert_eq!(
        event.error.as_ref().unwrap().code,
        ERR_RUNTIME_BACKEND_FAILED
    );
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "agent.strategy" && span.status == SpanStatus::Error)
    );
}

#[test]
fn runtime_records_strategy_result_error_as_error_event_without_changing_return() {
    let mut runtime = AgentRuntime::new();
    let mut backend = TestBackend {
        input_result_error: true,
        ..backend()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();
    runtime.publish(envelope()).unwrap();

    let result = runtime.tick_once("agent-a", &mut backend).unwrap();

    assert_eq!(result.status, StrategyResultStatus::Failed);
    assert_eq!(
        result.error.as_ref().unwrap().code,
        ERR_RUNTIME_BACKEND_FAILED
    );
    let events = runtime.events();
    assert!(!events.iter().any(|event| event.name == "agent.input"));
    let event = events
        .iter()
        .find(|event| event.name == "agent.input.error")
        .unwrap();
    assert_eq!(
        event.error.as_ref().unwrap().code,
        ERR_RUNTIME_BACKEND_FAILED
    );
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "agent.strategy" && span.status == SpanStatus::Error)
    );
}

#[test]
fn runtime_records_successful_idle_tick_as_next_step_event() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let result = runtime.tick_once("agent-a", &mut backend).unwrap();

    assert_eq!(result.status, StrategyResultStatus::WaitInput);
    let events = runtime.events();
    assert!(events.iter().any(|event| event.name == "agent.next_step"));
    assert!(
        !events
            .iter()
            .any(|event| event.name == "agent.next_step.error")
    );
}

#[test]
fn runtime_records_stop_backend_failure_as_error_event() {
    let mut runtime = AgentRuntime::new();
    let mut backend = TestBackend {
        fail_stop: true,
        ..backend()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let err = runtime.stop_agent("agent-a", &mut backend).unwrap_err();

    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Sleep));
    assert_eq!(backend.stopped, 0);
    let events = runtime.events();
    assert!(events.iter().any(|event| event.name == "agent.sleep"));
    assert!(!events.iter().any(|event| event.name == "agent.stop"));
    let event = events
        .iter()
        .find(|event| event.name == "agent.stop.error")
        .unwrap();
    assert_eq!(event.kind, RuntimeEventKind::Lifecycle);
    assert_eq!(
        event.error.as_ref().unwrap().code,
        ERR_RUNTIME_BACKEND_FAILED
    );
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "agent.stop" && span.status == SpanStatus::Error)
    );
}

#[test]
fn runtime_rejects_unregistered_source_before_routing() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let mut unknown = envelope();
    unknown.source.source_id = "source:unknown".into();

    let err = runtime.publish(unknown).unwrap_err();
    assert_eq!(err.error().code, ERR_SOURCE_UNREGISTERED);
    assert_eq!(runtime.inbox_len("agent-a"), Some(0));
}

#[test]
fn runtime_returns_scope_no_match_for_registered_source_without_accepting_agent() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let mut unmatched = envelope();
    unmatched.payload_schema_id = "other.input".into();

    let err = runtime.publish(unmatched).unwrap_err();
    assert_eq!(err.error().code, ERR_SCOPE_NO_MATCH);
    assert_eq!(runtime.inbox_len("agent-a"), Some(0));
}

#[test]
fn runtime_invokes_operation_through_backend_key() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    backend
        .operations
        .push(operation_snapshot("test.noop", OperationStatus::Active));
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let result = runtime
        .invoke_operation("agent-a", "test.noop", json!({"ok": true}), &mut backend)
        .unwrap();
    assert_eq!(result, BackendPayload::Json(json!({"ok": true})));
    assert_eq!(backend.invocations, 1);
}

#[test]
fn runtime_reports_agent_snapshots() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();
    runtime.publish(envelope()).unwrap();

    let snapshots = runtime.agent_snapshots();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].spec.agent_id, "agent-a");
    assert_eq!(snapshots[0].phase, AgentPhase::Awake);
    assert_eq!(snapshots[0].inbox_len, 1);
}

#[test]
fn runtime_returns_structured_error_for_missing_operation() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let err = runtime
        .invoke_operation("agent-a", "test.missing", json!({}), &mut backend)
        .unwrap_err();
    assert_eq!(err.error().code, ERR_OPERATION_NOT_FOUND);
    assert_eq!(backend.invocations, 0);
}

#[test]
fn runtime_records_inactive_operation_as_error_event_without_invoking_backend() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    backend
        .operations
        .push(operation_snapshot("test.noop", OperationStatus::Unhealthy));
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let err = runtime
        .invoke_operation("agent-a", "test.noop", json!({}), &mut backend)
        .unwrap_err();

    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    assert_eq!(
        err.error().evidence.get("operation_status"),
        Some(&ScalarValue::String("Unhealthy".into()))
    );
    assert_eq!(backend.invocations, 0);
    let event = runtime
        .events()
        .into_iter()
        .find(|event| event.name == "operation.invoke.error")
        .unwrap();
    assert_eq!(event.kind, RuntimeEventKind::Operation);
    assert_eq!(
        event.attributes.get("operation_status"),
        Some(&ScalarValue::String("Unhealthy".into()))
    );
    assert_eq!(
        event.error.as_ref().unwrap().code,
        ERR_RUNTIME_BACKEND_FAILED
    );
}

#[test]
fn runtime_native_backend_smoke_without_external_plugin_host() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("native-agent", 0)).unwrap();
    runtime.start_agent("native-agent", &mut backend).unwrap();
    let result = runtime.tick_once("native-agent", &mut backend).unwrap();
    assert_eq!(result.status, StrategyResultStatus::WaitInput);
    runtime.stop_agent("native-agent", &mut backend).unwrap();
    assert_eq!(runtime.phase("native-agent"), Some(&AgentPhase::Stop));
    assert_eq!(backend.awake, 1);
    assert_eq!(backend.stopped, 1);
}

#[test]
fn failed_awake_does_not_commit_agent_to_routing() {
    let mut runtime = AgentRuntime::new();
    let mut backend = TestBackend {
        fail_awake: true,
        ..backend()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();

    let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
    let publish_err = runtime.publish(envelope()).unwrap_err();
    assert_eq!(publish_err.error().code, ERR_SOURCE_UNREGISTERED);
    assert_eq!(runtime.inbox_len("agent-a"), Some(0));
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "agent.awake" && span.status == SpanStatus::Error)
    );
}
