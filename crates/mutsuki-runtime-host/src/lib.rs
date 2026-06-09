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
        OperationSnapshot, OperationStatus, RefDescriptor, ResourceRecord, RuntimeError,
        ScopeRuleSpec, SideEffectPolicy, SourceDescriptor, SourceRef, SourceSnapshot,
        StrategyResult,
    };
    use mutsuki_runtime_core::{
        AgentRuntime, BackendPayload, OperationBackend, ResourceBackend, StrategyBackend,
    };
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

    fn ref_descriptor(ref_id: &str, kind: &str) -> RefDescriptor {
        RefDescriptor {
            ref_id: ref_id.into(),
            kind: kind.into(),
            schema_id_target: kind.into(),
            schema_version_target: "1.0.0".into(),
            attributes: BTreeMap::new(),
            lineage: Vec::new(),
        }
    }

    fn resource_record(ref_id: &str, kind: &str, owner: &str, lease_count: u64) -> ResourceRecord {
        ResourceRecord {
            descriptor: ref_descriptor(ref_id, kind),
            owner: owner.into(),
            lease_count,
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

    #[test]
    fn jsonl_backend_dispatches_resource_register_acquire_and_release() {
        let token = mutsuki_runtime_contracts::LeaseToken {
            token_id: "lease-1".into(),
            ref_id: "ref-1".into(),
            owner: "agent-a".into(),
        };
        let response = [
            json!({"id": "req-1", "ok": true, "result": "ref-1"}).to_string(),
            json!({"id": "req-2", "ok": true, "result": token}).to_string(),
            json!({"id": "req-3", "ok": true, "result": null}).to_string(),
        ]
        .join("\n")
            + "\n";
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
        let written = String::from_utf8(writer).unwrap();
        let requests = written
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(requests[0]["method"], "resource.register");
        assert_eq!(requests[0]["params"]["descriptor"]["ref_id"], "ref-1");
        assert_eq!(requests[0]["params"]["owner"], "resource-host");
        assert_eq!(requests[1]["method"], "resource.acquire");
        assert_eq!(requests[1]["params"]["requester"], "agent-a");
        assert_eq!(requests[2]["method"], "resource.release");
        assert_eq!(requests[2]["params"]["token"]["token_id"], "lease-1");
    }

    #[test]
    fn jsonl_backend_lists_resource_records_with_optional_owner_filter() {
        let all_records = vec![
            resource_record("ref-a", "domain.resource", "owner-a", 0),
            resource_record("ref-b", "domain.resource", "owner-b", 1),
        ];
        let owner_records = vec![resource_record("ref-b", "domain.resource", "owner-b", 1)];
        let response = [
            json!({"id": "req-1", "ok": true, "result": all_records}).to_string(),
            json!({"id": "req-2", "ok": true, "result": owner_records}).to_string(),
        ]
        .join("\n")
            + "\n";
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
        let written = String::from_utf8(writer).unwrap();
        let requests = written
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(requests[0]["method"], "resource.list");
        assert!(requests[0]["params"]["owner"].is_null());
        assert_eq!(requests[1]["method"], "resource.list");
        assert_eq!(requests[1]["params"]["owner"], "owner-b");
    }

    #[test]
    fn jsonl_backend_resource_failure_preserves_backend_error() {
        let response = json!({
            "id": "req-1",
            "ok": false,
            "error": RuntimeError::new(
                "ref.not_found",
                "python_resource_backend",
                "python.resource.acquire.ref-missing"
            ),
        })
        .to_string()
            + "\n";
        let reader = Cursor::new(response.into_bytes());
        let writer = Vec::new();
        let mut backend = JsonlRuntimeBackend::new(reader, writer);

        let err = backend
            .acquire_resource("ref-missing", "agent-a")
            .unwrap_err();

        assert_eq!(err.error().code, "ref.not_found");
        assert_eq!(err.error().source, "python_resource_backend");
    }
}
