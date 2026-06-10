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
    use std::io::{BufReader, BufWriter};
    use std::path::Path;
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    use mutsuki_runtime_contracts::{
        AgentParticipation, AgentPhase, AgentSpec, Envelope, OperationDescriptor,
        OperationHandlerKey, OperationSnapshot, OperationStatus, RefDescriptor, ResourceRecord,
        RuntimeError, RuntimeEventKind, ScalarValue, ScopeRuleSpec, SideEffectPolicy,
        SourceDescriptor, SourceRef, SourceSnapshot, StrategyResult, StrategyResultStatus,
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

    fn codex_agent() -> AgentSpec {
        AgentSpec {
            agent_id: "codex-agent".into(),
            owner: None,
            priority: 0,
            participation: AgentParticipation::PrimaryCandidate,
            accepts: vec![ScopeRuleSpec::BySourceId {
                source_id: "codex:local".into(),
            }],
            strategy_id: "mutsuki-codex-core".into(),
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

    fn codex_envelope() -> Envelope {
        Envelope {
            id: "env-codex-1".into(),
            timestamp: 1.0,
            source: SourceRef {
                source_id: "codex:local".into(),
                kind: "codex.strategy".into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "codex.input".into(),
            capabilities_required: Vec::new(),
            payload: json!({"prompt": "decide"}),
        }
    }

    struct PythonJsonlBackendProcess {
        child: Child,
    }

    impl PythonJsonlBackendProcess {
        fn spawn(
            stub_output: &str,
        ) -> (
            Self,
            JsonlRuntimeBackend<BufReader<ChildStdout>, BufWriter<ChildStdin>>,
        ) {
            let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
            let repo_root = crate_root
                .parent()
                .and_then(|path| path.parent())
                .expect("host crate must live under crates/");
            let script = repo_root
                .join(".agents")
                .join("plugins")
                .join("plugins")
                .join("mutsuki-codex-core")
                .join("scripts")
                .join("mutsuki_codex_strategy_backend.py");
            assert!(
                script.is_file(),
                "missing Python backend script: {script:?}"
            );
            let mut command = if let Ok(python) = std::env::var("PYTHON") {
                Command::new(python)
            } else {
                let mut command = Command::new("uv");
                command
                    .arg("run")
                    .arg("--project")
                    .arg(repo_root.join("python").join("mutsuki-runtime-python"))
                    .arg("python");
                command
            };
            let mut child = command
                .arg(&script)
                .arg("--agent-id")
                .arg("codex-agent")
                .arg("--stub-output")
                .arg(stub_output)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("failed to spawn Python codex strategy backend");
            let stdout = child
                .stdout
                .take()
                .expect("Python backend stdout must be piped");
            let stdin = child
                .stdin
                .take()
                .expect("Python backend stdin must be piped");
            (
                Self { child },
                JsonlRuntimeBackend::new(BufReader::new(stdout), BufWriter::new(stdin)),
            )
        }
    }

    fn tick_codex_python_backend(stub_output: &str) -> (AgentRuntime, StrategyResult) {
        let (_process, mut backend) = PythonJsonlBackendProcess::spawn(stub_output);
        let mut runtime = AgentRuntime::new();

        runtime.register_agent(codex_agent()).unwrap();
        runtime.start_agent("codex-agent", &mut backend).unwrap();
        assert_eq!(runtime.phase("codex-agent"), Some(&AgentPhase::Awake));
        assert_eq!(
            runtime.source_snapshots("codex-agent").unwrap()[0]
                .descriptor
                .source_id,
            "codex:local"
        );
        assert_eq!(
            runtime.publish(codex_envelope()).unwrap(),
            vec!["codex-agent"]
        );

        let result = runtime.tick_once("codex-agent", &mut backend).unwrap();
        runtime.stop_agent("codex-agent", &mut backend).unwrap();
        (runtime, result)
    }

    impl Drop for PythonJsonlBackendProcess {
        fn drop(&mut self) {
            if let Ok(None) = self.child.try_wait() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
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

    #[test]
    fn jsonl_capability_backend_decodes_strategy_and_writes_request() {
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
    fn jsonl_capability_backend_maps_failure_response_to_runtime_failure() {
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
    fn jsonl_capability_backend_invokes_operation_as_json_payload() {
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
    fn jsonl_capability_backend_lists_operation_snapshots() {
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
    fn jsonl_capability_backend_drives_agent_runtime_lifecycle_with_scripted_responses() {
        let response = [
            json!({"id": "req-1", "ok": true, "result": null}).to_string(),
            json!({"id": "req-2", "ok": true, "result": []}).to_string(),
            json!({"id": "req-3", "ok": true, "result": [source_snapshot("source:test")]})
                .to_string(),
            json!({"id": "req-4", "ok": true, "result": StrategyResult::wait_input()}).to_string(),
            json!({"id": "req-5", "ok": true, "result": null}).to_string(),
        ]
        .join("\n")
            + "\n";
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

        assert_eq!(
            result.status,
            mutsuki_runtime_contracts::StrategyResultStatus::WaitInput
        );
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
        let requests = String::from_utf8(writer)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            requests
                .iter()
                .map(|request| request["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "on_awake",
                "list_operations",
                "list_sources",
                "on_input",
                "on_stop",
            ]
        );
        assert_eq!(
            requests[3]["params"]["envelope"]["source"]["source_id"],
            "source:test"
        );
    }

    #[test]
    fn jsonl_runtime_backend_smoke_drives_codex_python_process_roundtrip_and_failure() {
        let (runtime, result) = tick_codex_python_backend(r#"{"status":"wait_input"}"#);

        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        assert!(result.error.is_none());
        let events = runtime.events();
        assert!(events.iter().any(|event| event.name == "agent.awake"));
        assert!(events.iter().any(|event| event.name == "runtime.publish"));
        assert!(events.iter().any(|event| event.name == "agent.input"));
        assert!(events.iter().any(|event| event.name == "agent.stop"));

        let failure_output = json!({
            "status": "failed",
            "error": {
                "code": mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED,
                "source": "mutsuki-codex-core",
                "route": "codex.exec",
                "lost_capability": null,
                "recovery": null,
                "cause": null,
                "evidence": {"exit_code": 7, "stderr": "boom"},
            },
        })
        .to_string();
        let (runtime, result) = tick_codex_python_backend(&failure_output);

        assert_eq!(result.status, StrategyResultStatus::Failed);
        let error = result.error.as_ref().unwrap();
        assert_eq!(
            error.code,
            mutsuki_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED
        );
        assert_eq!(error.source, "mutsuki-codex-core");
        assert_eq!(error.route, "codex.exec");
        assert_eq!(error.evidence.get("exit_code"), Some(&ScalarValue::Int(7)));
        assert_eq!(
            error.evidence.get("stderr"),
            Some(&ScalarValue::String("boom".into()))
        );

        let event = runtime
            .events()
            .into_iter()
            .find(|event| event.name == "agent.input.error")
            .unwrap();
        assert_eq!(event.error.as_ref().unwrap(), error);
    }

    #[test]
    fn jsonl_capability_backend_operation_status_preserves_explicit_not_found() {
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
    fn jsonl_capability_backend_operation_status_failure_is_unhealthy() {
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
    fn jsonl_capability_backend_lists_resource_records_with_optional_owner_filter() {
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
    fn jsonl_capability_backend_resource_failure_preserves_backend_error() {
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
