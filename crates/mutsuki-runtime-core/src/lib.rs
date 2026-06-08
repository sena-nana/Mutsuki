mod agent_runtime;
mod backend;
mod error;
mod id;
mod resource_gate;
mod trace;

pub use agent_runtime::{AgentRuntime, AgentState};
pub use backend::{
    BackendPayload, OperationBackend, ResourceBackend, RuntimeBackend, StrategyBackend,
};
pub use error::{RuntimeFailure, RuntimeResult, scope_no_match_error};
pub use id::{IdSource, SequentialIdSource};
pub use resource_gate::ResourceGate;
pub use trace::TraceBook;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mutsuki_runtime_contracts::{
        AgentParticipation, AgentPhase, AgentSpec, ERR_OPERATION_NOT_FOUND,
        ERR_RUNTIME_BACKEND_FAILED, ERR_SCOPE_NO_MATCH, ERR_SOURCE_UNREGISTERED, Envelope,
        LeaseToken, OperationDescriptor, OperationHandlerKey, OperationSnapshot, OperationStatus,
        RefDescriptor, RuntimeError, ScalarValue, ScopeRuleSpec, SourceDescriptor, SourceRef,
        SourceSnapshot, SpanStatus, StrategyResult, StrategyResultStatus,
    };
    use serde_json::{Value, json};

    use crate::*;

    #[derive(Default)]
    struct NativeBackend {
        awake: usize,
        stopped: usize,
        inputs: usize,
        invocations: usize,
        operations: Vec<OperationSnapshot>,
        sources: Vec<SourceSnapshot>,
        fail_list_operations: bool,
        fail_list_sources: bool,
        fail_awake: bool,
    }

    impl StrategyBackend for NativeBackend {
        fn on_awake(&mut self, _agent_id: &str) -> RuntimeResult<()> {
            if self.fail_awake {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.on_awake",
                )));
            }
            self.awake += 1;
            Ok(())
        }

        fn on_input(
            &mut self,
            _agent_id: &str,
            _envelope: &Envelope,
        ) -> RuntimeResult<StrategyResult> {
            self.inputs += 1;
            Ok(StrategyResult {
                status: StrategyResultStatus::Completed,
                decision: None,
                emitted: Vec::new(),
                error: None,
            })
        }

        fn next_step(&mut self, _agent_id: &str) -> RuntimeResult<StrategyResult> {
            Ok(StrategyResult::wait_input())
        }

        fn on_stop(&mut self, _agent_id: &str) -> RuntimeResult<()> {
            self.stopped += 1;
            Ok(())
        }
    }

    impl OperationBackend for NativeBackend {
        fn list_operations(&self, _agent_id: &str) -> RuntimeResult<Vec<OperationSnapshot>> {
            if self.fail_list_operations {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.list_operations",
                )));
            }
            Ok(self.operations.clone())
        }

        fn list_sources(&self, _agent_id: &str) -> RuntimeResult<Vec<SourceSnapshot>> {
            if self.fail_list_sources {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.list_sources",
                )));
            }
            Ok(self.sources.clone())
        }

        fn invoke(
            &mut self,
            _agent_id: &str,
            _key: &OperationHandlerKey,
            payload: Value,
        ) -> RuntimeResult<BackendPayload> {
            self.invocations += 1;
            Ok(BackendPayload::Json(payload))
        }

        fn operation_status(&self, _agent_id: &str, _key: &OperationHandlerKey) -> OperationStatus {
            OperationStatus::Active
        }
    }

    fn envelope() -> Envelope {
        Envelope {
            id: "env-1".into(),
            timestamp: 0.0,
            source: SourceRef {
                source_id: "source:default".into(),
                kind: "test".into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "test.input".into(),
            capabilities_required: vec!["test.cap".into()],
            payload: Value::Null,
        }
    }

    fn agent(agent_id: &str, priority: i64) -> AgentSpec {
        AgentSpec {
            agent_id: agent_id.into(),
            owner: None,
            priority,
            participation: AgentParticipation::PrimaryCandidate,
            accepts: vec![ScopeRuleSpec::BySchemaPrefix {
                prefix: "test.".into(),
            }],
            strategy_id: "native".into(),
            side_effect_policy: mutsuki_runtime_contracts::SideEffectPolicy::ReadOnly,
        }
    }

    fn backend() -> NativeBackend {
        NativeBackend {
            sources: vec![SourceSnapshot {
                descriptor: SourceDescriptor {
                    source_id: "source:default".into(),
                    kind: "test".into(),
                    capabilities: Vec::new(),
                    description: String::new(),
                },
                plugin_id: "native".into(),
                plugin_generation: 0,
            }],
            ..NativeBackend::default()
        }
    }

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
    fn runtime_selects_primary_candidate_by_priority_then_id() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-b", 1)).unwrap();
        runtime.register_agent(agent("agent-a", 1)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();
        runtime.start_agent("agent-b", &mut backend).unwrap();

        assert_eq!(
            runtime.select_accepting(&envelope()),
            Some("agent-a".into())
        );
    }

    #[test]
    fn runtime_select_accepting_ignores_unregistered_source() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let mut unknown = envelope();
        unknown.source.source_id = "source:unknown".into();

        assert_eq!(runtime.select_accepting(&unknown), None);
    }

    #[test]
    fn runtime_invokes_operation_through_backend_key() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        backend.operations.push(OperationSnapshot {
            descriptor: OperationDescriptor {
                op_id: "test.noop".into(),
                name: "noop".into(),
                description: String::new(),
                plugin_id: "test".into(),
                func_qualname: String::new(),
                parameters_schema: json!({}),
                return_schema: json!({}),
                perms_rule_id: None,
                requires_capabilities: Vec::new(),
                is_tool: true,
            },
            status: OperationStatus::Active,
            key: OperationHandlerKey {
                plugin_id: "test".into(),
                plugin_generation: 0,
                op_id: "test.noop".into(),
                handler_id: "test:test.noop:0".into(),
            },
        });
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        let result = runtime
            .invoke_operation("agent-a", "test.noop", json!({"ok": true}), &mut backend)
            .unwrap();
        assert_eq!(result, BackendPayload::Json(json!({"ok": true})));
        assert_eq!(backend.invocations, 1);
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
    fn resource_gate_tracks_descriptor_leases_without_handles() {
        let mut gate = ResourceGate::new();
        let ref_id = gate.register(
            RefDescriptor {
                ref_id: "ref-1".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "backend",
        );

        let lease = gate.acquire(&ref_id, "agent-a").unwrap();
        assert_eq!(lease.ref_id, "ref-1");
        assert_eq!(lease.token_id, "lease-00000000000000000000000001");
        assert_eq!(gate.list_records()[0].lease_count, 1);

        gate.release(&lease).unwrap();
        assert_eq!(gate.list_records()[0].lease_count, 0);
    }

    #[test]
    fn resource_backend_filters_records_by_owner() {
        let mut gate = ResourceGate::new();
        gate.register(
            RefDescriptor {
                ref_id: "ref-a".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "owner-a",
        );
        gate.register(
            RefDescriptor {
                ref_id: "ref-b".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "owner-b",
        );

        let owner_a = <ResourceGate as ResourceBackend>::list_records(&gate, Some("owner-a"));
        assert_eq!(owner_a.len(), 1);
        assert_eq!(owner_a[0].descriptor.ref_id, "ref-a");
    }

    #[test]
    fn failed_awake_does_not_commit_agent_to_routing() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend {
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

    #[test]
    fn failed_operation_refresh_does_not_commit_agent_to_routing() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend {
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
        backend.operations.push(OperationSnapshot {
            descriptor: OperationDescriptor {
                op_id: "test.noop".into(),
                name: "noop".into(),
                description: String::new(),
                plugin_id: "test".into(),
                func_qualname: String::new(),
                parameters_schema: json!({}),
                return_schema: json!({}),
                perms_rule_id: None,
                requires_capabilities: Vec::new(),
                is_tool: true,
            },
            status: OperationStatus::Active,
            key: OperationHandlerKey {
                plugin_id: "test".into(),
                plugin_generation: 0,
                op_id: "test.noop".into(),
                handler_id: "test:test.noop:0".into(),
            },
        });
        runtime.register_agent(agent("agent-a", 0)).unwrap();

        let err = runtime.start_agent("agent-a", &mut backend).unwrap_err();
        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(runtime.phase("agent-a"), Some(&AgentPhase::Spawn));
        assert!(runtime.operation_snapshot("agent-a", "test.noop").is_none());
        assert!(runtime.source_snapshots("agent-a").is_none());
    }

    #[test]
    fn resource_gate_rejects_forged_lease_token_without_releasing() {
        let mut gate = ResourceGate::new();
        let ref_id = gate.register(
            RefDescriptor {
                ref_id: "ref-1".into(),
                kind: "domain.resource".into(),
                schema_id_target: "domain.resource".into(),
                schema_version_target: "1.0.0".into(),
                attributes: BTreeMap::new(),
                lineage: Vec::new(),
            },
            "backend",
        );

        let lease = gate.acquire(&ref_id, "agent-a").unwrap();
        let forged = LeaseToken {
            token_id: lease.token_id.clone(),
            ref_id: "ref-other".into(),
            owner: "agent-b".into(),
        };

        let err = gate.release(&forged).unwrap_err();
        assert_eq!(err.error().code, "ref.not_found");
        assert_eq!(
            err.error().evidence.get("reason"),
            Some(&ScalarValue::String("lease_token_mismatch".into()))
        );
        assert_eq!(gate.list_records()[0].lease_count, 1);

        gate.release(&lease).unwrap();
        assert_eq!(gate.list_records()[0].lease_count, 0);
    }
}
