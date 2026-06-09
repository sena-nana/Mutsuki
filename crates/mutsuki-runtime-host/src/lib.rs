mod host;
mod jsonl;
mod operation;

pub use host::NativeRuntimeHost;
pub use jsonl::JsonlRuntimeBackend;
pub use operation::NativeOperation;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;

    use mutsuki_runtime_contracts::{
        AgentParticipation, AgentSpec, Envelope, OperationDescriptor, OperationHandlerKey,
        OperationSnapshot, OperationStatus, RuntimeError, ScopeRuleSpec, SideEffectPolicy,
        SourceDescriptor, SourceRef, SourceSnapshot, StrategyResult,
    };
    use mutsuki_runtime_core::{AgentRuntime, BackendPayload, OperationBackend, StrategyBackend};
    use serde_json::{Value, json};

    use super::*;

    fn agent() -> AgentSpec {
        AgentSpec {
            agent_id: "native-agent".into(),
            owner: None,
            priority: 0,
            participation: AgentParticipation::PrimaryCandidate,
            accepts: vec![ScopeRuleSpec::BySchema {
                schema_id: "test.input".into(),
            }],
            strategy_id: "native".into(),
            side_effect_policy: SideEffectPolicy::ReadOnly,
        }
    }

    fn envelope() -> Envelope {
        Envelope {
            id: "env-1".into(),
            timestamp: 1.0,
            source: SourceRef {
                source_id: "source:test".into(),
                kind: "test".into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "test.input".into(),
            capabilities_required: Vec::new(),
            payload: Value::Null,
        }
    }

    fn source_snapshot(source_id: &str) -> SourceSnapshot {
        SourceSnapshot {
            descriptor: SourceDescriptor {
                source_id: source_id.into(),
                kind: "test".into(),
                capabilities: Vec::new(),
                description: String::new(),
            },
            plugin_id: "native".into(),
            plugin_generation: 0,
        }
    }

    fn operation_descriptor(op_id: &str) -> OperationDescriptor {
        OperationDescriptor {
            op_id: op_id.into(),
            name: op_id.rsplit('.').next().unwrap_or(op_id).into(),
            description: String::new(),
            plugin_id: op_id.split('.').next().unwrap_or("test").into(),
            func_qualname: String::new(),
            parameters_schema: json!({}),
            return_schema: json!({}),
            perms_rule_id: None,
            requires_capabilities: Vec::new(),
            is_tool: true,
        }
    }

    fn operation_key(op_id: &str) -> OperationHandlerKey {
        let plugin_id = op_id.split('.').next().unwrap_or("test");
        OperationHandlerKey {
            plugin_id: plugin_id.into(),
            plugin_generation: 0,
            op_id: op_id.into(),
            handler_id: format!("{plugin_id}:{op_id}:0"),
        }
    }

    fn operation_snapshot(op_id: &str, status: OperationStatus) -> OperationSnapshot {
        OperationSnapshot {
            descriptor: operation_descriptor(op_id),
            status,
            key: operation_key(op_id),
        }
    }

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

    #[test]
    fn jsonl_backend_decodes_strategy_and_writes_request() {
        let response = json!({
            "id": "req-1",
            "ok": true,
            "result": StrategyResult::wait_input(),
        })
        .to_string()
            + "\n";
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
    fn jsonl_backend_maps_failure_response_to_runtime_failure() {
        let response = json!({
            "id": "req-1",
            "ok": false,
            "error": RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED,
                "python_stdio",
                "python.invoke.test"
            ),
        })
        .to_string()
            + "\n";
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
    fn jsonl_backend_invokes_operation_as_json_payload() {
        let response = json!({
            "id": "req-1",
            "ok": true,
            "result": {"value": "ok"},
        })
        .to_string()
            + "\n";
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
    fn jsonl_backend_lists_operation_snapshots() {
        let snapshot = operation_snapshot("test.echo", OperationStatus::Active);
        let response = json!({
            "id": "req-1",
            "ok": true,
            "result": [snapshot],
        })
        .to_string()
            + "\n";
        let reader = Cursor::new(response.into_bytes());
        let writer = Vec::new();
        let backend = JsonlRuntimeBackend::new(reader, writer);

        let operations = backend.list_operations("agent-a").unwrap();

        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].descriptor.op_id, "test.echo");
    }

    #[test]
    fn jsonl_backend_operation_status_preserves_explicit_not_found() {
        let response = json!({
            "id": "req-1",
            "ok": true,
            "result": "not_found",
        })
        .to_string()
            + "\n";
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
    fn jsonl_backend_operation_status_failure_is_unhealthy() {
        let response = json!({
            "id": "req-1",
            "ok": false,
            "error": RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED,
                "python_stdio",
                "python.operation_status.test"
            ),
        })
        .to_string()
            + "\n";
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
    fn jsonl_backend_operation_status_protocol_error_is_unhealthy() {
        let reader = Cursor::new(b"\n".to_vec());
        let writer = Vec::new();
        let backend = JsonlRuntimeBackend::new(reader, writer);
        let key = operation_key("test.echo");

        assert_eq!(
            backend.operation_status("agent-a", &key),
            OperationStatus::Unhealthy
        );
    }
}
