use std::collections::BTreeMap;

use super::fixtures::*;
use crate::*;
use mutsukicore_runtime_contracts::*;
use serde_json::json;

#[test]
fn runtime_emits_deterministic_events_for_lifecycle_routing_and_operation() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    backend
        .operations
        .push(operation_snapshot("test.noop", OperationStatus::Active));
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();
    runtime.publish(envelope()).unwrap();
    runtime.tick_once("agent-a", &mut backend).unwrap();
    runtime
        .invoke_operation("agent-a", "test.noop", json!({}), &mut backend)
        .unwrap();
    runtime.stop_agent("agent-a", &mut backend).unwrap();

    let events = runtime.events();
    assert!(events.iter().any(|event| event.name == "agent.awake"));
    assert!(events.iter().any(|event| event.name == "runtime.publish"));
    assert!(events.iter().any(|event| event.name == "agent.input"));
    assert!(events.iter().any(|event| event.name == "operation.invoke"));
    assert!(events.iter().any(|event| event.name == "agent.stop"));
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    let drained = runtime.drain_events();
    assert_eq!(drained.len(), events.len());
    assert!(
        drained
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert!(runtime.events().is_empty());
}

#[test]
fn runtime_event_sequence_continues_after_drain() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let drained = runtime.drain_events();
    let last_sequence = drained.last().unwrap().sequence;
    runtime.publish(envelope()).unwrap();

    let events = runtime.events();
    assert!(events[0].sequence > last_sequence);
}

#[test]
fn runtime_emits_trace_span_events_with_span_shape() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let events = runtime.events();
    let trace_index = events
        .iter()
        .position(|event| {
            event.kind == RuntimeEventKind::Trace
                && event.name == "trace.span"
                && event.attributes.get("span_name")
                    == Some(&ScalarValue::String("agent.awake".into()))
        })
        .unwrap();
    let awake_index = events
        .iter()
        .position(|event| event.name == "agent.awake")
        .unwrap();
    let event = &events[trace_index];

    assert!(trace_index < awake_index);
    assert_eq!(event.agent_id.as_deref(), Some("agent-a"));
    assert!(event.error.is_none());
    assert_eq!(
        event.attributes.get("trace_id"),
        Some(&ScalarValue::String("trace-agent-a".into()))
    );
    assert!(event.attributes.contains_key("span_id"));
    assert_eq!(
        event.attributes.get("status"),
        Some(&ScalarValue::String("ok".into()))
    );
}

#[test]
fn runtime_trace_events_for_unregistered_source_are_not_agent_scoped() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let mut unknown = envelope();
    unknown.source.source_id = "source:unknown".into();
    runtime.publish(unknown).unwrap_err();

    let events = runtime.events();
    let event = events
        .iter()
        .find(|event| {
            event.kind == RuntimeEventKind::Trace
                && event.name == "trace.span"
                && event.attributes.get("span_name")
                    == Some(&ScalarValue::String("runtime.source_unregistered".into()))
        })
        .unwrap();
    assert_eq!(event.agent_id, None);
    assert_eq!(
        event.attributes.get("trace_id"),
        Some(&ScalarValue::String("trace-runtime".into()))
    );
}

#[test]
fn runtime_trace_events_for_scope_no_match_are_not_agent_scoped() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let mut unmatched = envelope();
    unmatched.payload_schema_id = "other.input".into();
    runtime.publish(unmatched).unwrap_err();

    let events = runtime.events();
    let event = events
        .iter()
        .find(|event| {
            event.kind == RuntimeEventKind::Trace
                && event.name == "trace.span"
                && event.attributes.get("span_name")
                    == Some(&ScalarValue::String("runtime.scope_no_match".into()))
        })
        .unwrap();
    assert_eq!(event.agent_id, None);
    assert_eq!(
        event.attributes.get("trace_id"),
        Some(&ScalarValue::String("trace-runtime".into()))
    );
}

#[test]
fn runtime_emits_structured_routing_error_events() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let mut unknown = envelope();
    unknown.source.source_id = "source:unknown".into();
    let err = runtime.publish(unknown).unwrap_err();
    assert_eq!(err.error().code, ERR_SOURCE_UNREGISTERED);

    let events = runtime.events();
    let event = events
        .iter()
        .find(|event| event.name == "runtime.source_unregistered")
        .unwrap();
    assert_eq!(event.kind, RuntimeEventKind::Routing);
    assert_eq!(event.error.as_ref().unwrap().code, ERR_SOURCE_UNREGISTERED);
}

#[test]
fn trace_closure_helper_reports_expected_issues() {
    let spans = vec![
        TraceSpan {
            trace_id: "trace-a".into(),
            span_id: "span-1".into(),
            parent_span_id: None,
            name: "root".into(),
            start: 1.0,
            end: Some(2.0),
            attributes: BTreeMap::new(),
            status: SpanStatus::Ok,
        },
        TraceSpan {
            trace_id: "trace-a".into(),
            span_id: "span-1".into(),
            parent_span_id: None,
            name: "dupe".into(),
            start: 3.0,
            end: Some(2.0),
            attributes: BTreeMap::new(),
            status: SpanStatus::Ok,
        },
        TraceSpan {
            trace_id: "trace-b".into(),
            span_id: "span-2".into(),
            parent_span_id: Some("span-1".into()),
            name: "wrong-trace".into(),
            start: 1.0,
            end: Some(1.0),
            attributes: BTreeMap::new(),
            status: SpanStatus::Ok,
        },
        TraceSpan {
            trace_id: "trace-a".into(),
            span_id: "span-3".into(),
            parent_span_id: Some("missing".into()),
            name: "missing-parent".into(),
            start: 1.0,
            end: Some(1.0),
            attributes: BTreeMap::new(),
            status: SpanStatus::Ok,
        },
    ];

    let issues = validate_trace_closure(&spans);
    assert!(issues.contains(&TraceClosureIssue::DuplicateSpanId {
        span_id: "span-1".into()
    }));
    assert!(issues.contains(&TraceClosureIssue::InvalidInterval {
        span_id: "span-1".into()
    }));
    assert!(issues.contains(&TraceClosureIssue::ParentTraceMismatch {
        span_id: "span-2".into(),
        parent_span_id: "span-1".into()
    }));
    assert!(issues.contains(&TraceClosureIssue::MissingParent {
        span_id: "span-3".into(),
        parent_span_id: "missing".into()
    }));
}
