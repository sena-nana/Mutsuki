use std::collections::BTreeMap;

use super::fixtures::*;
use crate::*;
use mutsukicore_runtime_contracts::*;
use serde_json::json;

#[test]
fn runtime_enables_and_disables_plugins_for_agent_behavior() {
    let mut runtime = AgentRuntime::new();
    let mut backend = TestBackend {
        operations: vec![
            operation_snapshot_for_plugin("plugin-a", "plugin-a.echo", OperationStatus::Active),
            operation_snapshot_for_plugin("plugin-b", "plugin-b.echo", OperationStatus::Active),
        ],
        sources: vec![
            source_snapshot_for_plugin("plugin-a", "source:a"),
            source_snapshot_for_plugin("plugin-b", "source:b"),
        ],
        ..TestBackend::default()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime
        .set_enabled_plugins(vec!["plugin-a".into()], &backend)
        .unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    assert!(
        runtime
            .operation_snapshot("agent-a", "plugin-a.echo")
            .is_some()
    );
    assert!(
        runtime
            .operation_snapshot("agent-a", "plugin-b.echo")
            .is_none()
    );
    assert_eq!(runtime.enabled_plugin_snapshots().len(), 1);
    assert_eq!(
        runtime.enabled_plugin_snapshots()[0].descriptor.plugin_id,
        "plugin-a"
    );
    assert!(
        runtime
            .disabled_plugin_snapshots()
            .iter()
            .any(|plugin| plugin.descriptor.plugin_id == "plugin-b")
    );

    runtime
        .disable_plugins(&["plugin-a".to_string()], &backend)
        .unwrap();
    assert!(
        runtime
            .operation_snapshot("agent-a", "plugin-a.echo")
            .is_none()
    );
    let err = runtime
        .invoke_operation("agent-a", "plugin-a.echo", json!({}), &mut backend)
        .unwrap_err();
    assert_eq!(err.error().code, ERR_OPERATION_NOT_FOUND);

    let mut from_disabled_source = envelope();
    from_disabled_source.source.source_id = "source:a".into();
    let err = runtime.publish(from_disabled_source).unwrap_err();
    assert_eq!(err.error().code, ERR_SOURCE_UNREGISTERED);
}

#[test]
fn plugin_access_refresh_failure_preserves_previous_registry() {
    let mut runtime = AgentRuntime::new();
    let backend = TestBackend {
        operations: vec![operation_snapshot_for_plugin(
            "plugin-a",
            "plugin-a.echo",
            OperationStatus::Active,
        )],
        sources: vec![source_snapshot_for_plugin("plugin-a", "source:a")],
        ..TestBackend::default()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime
        .set_enabled_plugins(vec!["plugin-a".into()], &backend)
        .unwrap();
    assert!(
        runtime
            .operation_snapshot("agent-a", "plugin-a.echo")
            .is_some()
    );

    let failing = TestBackend {
        fail_list_sources: true,
        operations: vec![operation_snapshot_for_plugin(
            "plugin-b",
            "plugin-b.echo",
            OperationStatus::Active,
        )],
        sources: vec![source_snapshot_for_plugin("plugin-b", "source:b")],
        ..TestBackend::default()
    };
    let err = runtime
        .set_enabled_plugins(vec!["plugin-b".into()], &failing)
        .unwrap_err();

    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    assert!(
        runtime
            .operation_snapshot("agent-a", "plugin-a.echo")
            .is_some()
    );
    assert!(
        runtime
            .operation_snapshot("agent-a", "plugin-b.echo")
            .is_none()
    );
    assert_eq!(
        runtime.plugin_access_state().enabled_plugin_ids,
        vec!["plugin-a".to_string()]
    );
    assert!(
        runtime
            .events()
            .iter()
            .any(|event| event.name == "plugin.access.update.error")
    );
}

#[test]
fn enabling_missing_or_disabled_plugin_fails_without_committing_access_state() {
    let mut runtime = AgentRuntime::new();
    let backend = TestBackend {
        operations: vec![operation_snapshot_for_plugin(
            "plugin-a",
            "plugin-a.echo",
            OperationStatus::Active,
        )],
        sources: vec![source_snapshot_for_plugin("plugin-a", "source:a")],
        plugins: vec![PluginSnapshot {
            descriptor: PluginDescriptor {
                plugin_id: "plugin-disabled".into(),
                generation: 0,
                name: "disabled".into(),
                description: String::new(),
                version: String::new(),
                capabilities: Vec::new(),
                metadata: BTreeMap::new(),
            },
            status: PluginStatus::Disabled,
        }],
        ..TestBackend::default()
    };

    runtime
        .set_enabled_plugins(vec!["plugin-a".into()], &backend)
        .unwrap();
    let missing = runtime
        .set_enabled_plugins(vec!["plugin-missing".into()], &backend)
        .unwrap_err();
    assert_eq!(missing.error().code, "plugin.not_found");
    assert_eq!(
        runtime.plugin_access_state().enabled_plugin_ids,
        vec!["plugin-a".to_string()]
    );

    let disabled = runtime
        .set_enabled_plugins(vec!["plugin-disabled".into()], &backend)
        .unwrap_err();
    assert_eq!(disabled.error().code, "plugin.disabled");
    assert_eq!(
        runtime.plugin_access_state().enabled_plugin_ids,
        vec!["plugin-a".to_string()]
    );
}

#[test]
fn failed_operation_refresh_does_not_commit_agent_to_routing() {
    let mut runtime = AgentRuntime::new();
    let mut backend = TestBackend {
        fail_list_operations: true,
        ..backend()
    };
    runtime.register_agent(agent("agent-a", 0)).unwrap();

    let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
    let publish_err = runtime.publish(envelope()).unwrap_err();
    assert_eq!(publish_err.error().code, ERR_SOURCE_UNREGISTERED);
    assert_eq!(runtime.inbox_len("agent-a"), Some(0));
    assert!(runtime.operation_snapshot("agent-a", "test.noop").is_none());
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "agent.awake" && span.status == SpanStatus::Error)
    );
}

#[test]
fn failed_source_refresh_does_not_commit_operation_or_source_registry() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    backend.fail_list_sources = true;
    backend
        .operations
        .push(operation_snapshot("test.noop", OperationStatus::Active));
    runtime.register_agent(agent("agent-a", 0)).unwrap();

    let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
    assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
    assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
    assert!(runtime.operation_snapshot("agent-a", "test.noop").is_none());
    assert!(runtime.source_snapshots("agent-a").unwrap().is_empty());
}
