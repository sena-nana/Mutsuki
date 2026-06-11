mod agent_runtime;
mod backend;
mod election;
mod error;
mod event;
mod id;
mod resource_gate;
mod trace;

pub use agent_runtime::{AgentRuntime, AgentState};
pub use backend::{
    BackendPayload, OperationBackend, ResourceBackend, RuntimeBackend, StrategyBackend,
};
pub use election::{ElectionCandidate, ElectionPolicy, PriorityElectionPolicy};
pub use error::{RuntimeFailure, RuntimeResult, scope_no_match_error};
pub use id::{IdSource, SequentialIdSource};
pub use resource_gate::{ResourceGate, ResourceQuotaPolicy};
pub use trace::{TraceBook, TraceClosureIssue, validate_trace_closure};

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::BTreeMap};

    use mutsukicore_runtime_contracts::{
        AgentId, AgentParticipation, AgentPhase, AgentSpec, ERR_CAPABILITY_EXHAUSTED,
        ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED, ERR_SCOPE_NO_MATCH,
        ERR_SOURCE_UNREGISTERED, Envelope, LeaseToken, OperationDescriptor, OperationHandlerKey,
        OperationSnapshot, OperationStatus, PluginDescriptor, PluginSnapshot, PluginStatus,
        RefDescriptor, RuntimeError, RuntimeEventKind, ScalarValue, ScopeRuleSpec,
        SourceDescriptor, SourceRef, SourceSnapshot, SpanStatus, StrategyResult,
        StrategyResultStatus, TraceSpan,
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
        plugins: Vec<PluginSnapshot>,
        fail_list_operations: bool,
        fail_list_sources: bool,
        fail_awake: bool,
        fail_input: bool,
        fail_next_step: bool,
        input_result_error: bool,
        fail_stop: bool,
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
            if self.fail_input {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.on_input",
                )));
            }
            self.inputs += 1;
            let error = self.input_result_error.then(|| {
                RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.on_input.result",
                )
            });
            let status = if error.is_some() {
                StrategyResultStatus::Failed
            } else {
                StrategyResultStatus::Completed
            };
            Ok(StrategyResult {
                status,
                decision: None,
                emitted: Vec::new(),
                error,
            })
        }

        fn next_step(&mut self, _agent_id: &str) -> RuntimeResult<StrategyResult> {
            if self.fail_next_step {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.next_step",
                )));
            }
            Ok(StrategyResult::wait_input())
        }

        fn on_stop(&mut self, _agent_id: &str) -> RuntimeResult<()> {
            if self.fail_stop {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.on_stop",
                )));
            }
            self.stopped += 1;
            Ok(())
        }
    }

    impl OperationBackend for NativeBackend {
        fn list_plugins(&self) -> RuntimeResult<Vec<PluginSnapshot>> {
            let mut plugins: BTreeMap<String, u64> = BTreeMap::new();
            for source in &self.sources {
                plugins.insert(source.plugin_id.clone(), source.plugin_generation);
            }
            for operation in &self.operations {
                plugins.insert(
                    operation.key.plugin_id.clone(),
                    operation.key.plugin_generation,
                );
            }
            let mut snapshots: Vec<PluginSnapshot> = plugins
                .into_iter()
                .map(|(plugin_id, generation)| PluginSnapshot {
                    descriptor: PluginDescriptor {
                        plugin_id: plugin_id.clone(),
                        generation,
                        name: plugin_id,
                        description: String::new(),
                        version: String::new(),
                        capabilities: Vec::new(),
                        metadata: BTreeMap::new(),
                    },
                    status: PluginStatus::Enabled,
                })
                .collect();
            snapshots.extend(self.plugins.iter().cloned());
            Ok(snapshots)
        }

        fn list_operations(
            &self,
            enabled_plugin_ids: &[String],
        ) -> RuntimeResult<Vec<OperationSnapshot>> {
            if self.fail_list_operations {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.list_operations",
                )));
            }
            Ok(self
                .operations
                .iter()
                .filter(|operation| enabled_plugin_ids.contains(&operation.key.plugin_id))
                .cloned()
                .collect())
        }

        fn list_sources(
            &self,
            enabled_plugin_ids: &[String],
        ) -> RuntimeResult<Vec<SourceSnapshot>> {
            if self.fail_list_sources {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_BACKEND_FAILED,
                    "native",
                    "native.list_sources",
                )));
            }
            Ok(self
                .sources
                .iter()
                .filter(|source| enabled_plugin_ids.contains(&source.plugin_id))
                .cloned()
                .collect())
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
            side_effect_policy: mutsukicore_runtime_contracts::SideEffectPolicy::ReadOnly,
        }
    }

    fn backend() -> NativeBackend {
        NativeBackend {
            sources: vec![source_snapshot("source:default")],
            ..NativeBackend::default()
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
            plugin_id: "test".into(),
            func_qualname: String::new(),
            parameters_schema: json!({}),
            return_schema: json!({}),
            perms_rule_id: None,
            requires_capabilities: Vec::new(),
            is_tool: true,
        }
    }

    fn operation_key(op_id: &str) -> OperationHandlerKey {
        OperationHandlerKey {
            plugin_id: "test".into(),
            plugin_generation: 0,
            op_id: op_id.into(),
            handler_id: format!("test:{op_id}:0"),
        }
    }

    fn operation_snapshot(op_id: &str, status: OperationStatus) -> OperationSnapshot {
        OperationSnapshot {
            descriptor: operation_descriptor(op_id),
            status,
            key: operation_key(op_id),
        }
    }

    fn operation_snapshot_for_plugin(
        plugin_id: &str,
        op_id: &str,
        status: OperationStatus,
    ) -> OperationSnapshot {
        let mut descriptor = operation_descriptor(op_id);
        descriptor.plugin_id = plugin_id.into();
        OperationSnapshot {
            descriptor,
            status,
            key: OperationHandlerKey {
                plugin_id: plugin_id.into(),
                plugin_generation: 0,
                op_id: op_id.into(),
                handler_id: format!("{plugin_id}:{op_id}:0"),
            },
        }
    }

    fn source_snapshot_for_plugin(plugin_id: &str, source_id: &str) -> SourceSnapshot {
        let mut snapshot = source_snapshot(source_id);
        snapshot.plugin_id = plugin_id.into();
        snapshot
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
        let mut backend = NativeBackend {
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
        let mut backend = NativeBackend {
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
        let mut backend = NativeBackend {
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
        let mut backend = NativeBackend {
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
    fn runtime_enables_and_disables_plugins_for_agent_behavior() {
        let mut runtime = AgentRuntime::new();
        let mut backend = NativeBackend {
            operations: vec![
                operation_snapshot_for_plugin("plugin-a", "plugin-a.echo", OperationStatus::Active),
                operation_snapshot_for_plugin("plugin-b", "plugin-b.echo", OperationStatus::Active),
            ],
            sources: vec![
                source_snapshot_for_plugin("plugin-a", "source:a"),
                source_snapshot_for_plugin("plugin-b", "source:b"),
            ],
            ..NativeBackend::default()
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
        let backend = NativeBackend {
            operations: vec![operation_snapshot_for_plugin(
                "plugin-a",
                "plugin-a.echo",
                OperationStatus::Active,
            )],
            sources: vec![source_snapshot_for_plugin("plugin-a", "source:a")],
            ..NativeBackend::default()
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

        let failing = NativeBackend {
            fail_list_sources: true,
            operations: vec![operation_snapshot_for_plugin(
                "plugin-b",
                "plugin-b.echo",
                OperationStatus::Active,
            )],
            sources: vec![source_snapshot_for_plugin("plugin-b", "source:b")],
            ..NativeBackend::default()
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
        let backend = NativeBackend {
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
            ..NativeBackend::default()
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
    fn resource_gate_tracks_descriptor_leases_without_handles() {
        let mut gate = ResourceGate::new();
        let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");

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
        gate.register(ref_descriptor("ref-a", "domain.resource"), "owner-a");
        gate.register(ref_descriptor("ref-b", "domain.resource"), "owner-b");

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

    #[test]
    fn resource_gate_rejects_forged_lease_token_without_releasing() {
        let mut gate = ResourceGate::new();
        let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");

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

    #[test]
    fn standalone_resource_gate_does_not_collect_event_drafts() {
        let mut gate = ResourceGate::new();
        let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "resource-host");
        let lease = gate.acquire(&ref_id, "agent-a").unwrap();
        gate.release(&lease).unwrap();

        assert!(gate.event_drafts().is_empty());
    }

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
    fn runtime_assigns_global_event_sequence_to_pending_resource_events() {
        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 0)).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();
        let ref_id = runtime
            .resources_mut()
            .register(ref_descriptor("ref-1", "domain.resource"), "resource-host");
        let lease = runtime.resources_mut().acquire(&ref_id, "agent-a").unwrap();

        let snapshot = runtime.events();
        assert!(
            snapshot
                .iter()
                .any(|event| event.name == "resource.register")
        );
        assert!(
            snapshot
                .iter()
                .any(|event| event.name == "resource.acquire")
        );
        assert!(
            snapshot
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );

        let drained = runtime.drain_events();
        assert_eq!(drained, snapshot);
        assert!(runtime.events().is_empty());

        runtime.resources_mut().release(&lease).unwrap();
        runtime.publish(envelope()).unwrap();
        let events = runtime.events();
        let release_index = events
            .iter()
            .position(|event| event.name == "resource.release")
            .unwrap();
        let publish_index = events
            .iter()
            .position(|event| event.name == "runtime.publish")
            .unwrap();
        assert!(release_index < publish_index);
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
    }

    #[test]
    fn runtime_emits_structured_resource_error_events() {
        let mut runtime = AgentRuntime::new();

        let acquire_err = runtime
            .resources_mut()
            .acquire("ref-missing", "agent-a")
            .unwrap_err();
        assert_eq!(acquire_err.error().code, "ref.not_found");
        let events = runtime.events();
        let event = events
            .iter()
            .find(|event| event.name == "resource.acquire.error")
            .unwrap();
        assert_eq!(event.kind, RuntimeEventKind::Resource);
        assert_eq!(event.error.as_ref().unwrap().code, "ref.not_found");
        assert_eq!(
            event.attributes.get("ref_id"),
            Some(&ScalarValue::String("ref-missing".into()))
        );

        runtime.drain_events();
        let stale = LeaseToken {
            token_id: "lease-missing".into(),
            ref_id: "ref-missing".into(),
            owner: "agent-a".into(),
        };
        let release_err = runtime.resources_mut().release(&stale).unwrap_err();
        assert_eq!(release_err.error().code, "ref.not_found");
        let events = runtime.events();
        let event = events
            .iter()
            .find(|event| event.name == "resource.release.error")
            .unwrap();
        assert_eq!(event.kind, RuntimeEventKind::Resource);
        assert_eq!(event.error.as_ref().unwrap().code, "ref.not_found");
        assert_eq!(
            event.attributes.get("token_id"),
            Some(&ScalarValue::String("lease-missing".into()))
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
    fn resource_gate_enforces_ref_quota_without_incrementing_leases() {
        let mut policy = ResourceQuotaPolicy::default();
        policy.max_leases_by_ref.insert("ref-1".into(), 1);
        let mut gate = ResourceGate::with_quota_policy(policy);
        let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");

        let lease = gate.acquire(&ref_id, "agent-a").unwrap();
        let err = gate.acquire(&ref_id, "agent-b").unwrap_err();
        assert_eq!(err.error().code, ERR_CAPABILITY_EXHAUSTED);
        assert_eq!(
            err.error().evidence.get("dimension"),
            Some(&ScalarValue::String("ref_id".into()))
        );
        assert_eq!(gate.list_records()[0].lease_count, 1);

        gate.release(&lease).unwrap();
    }

    #[test]
    fn resource_gate_enforces_kind_quota_and_ref_quota_takes_precedence() {
        let mut policy = ResourceQuotaPolicy::default();
        policy
            .max_leases_by_kind
            .insert("domain.resource".into(), 1);
        let mut gate = ResourceGate::with_quota_policy(policy.clone());
        gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");
        gate.register(ref_descriptor("ref-2", "domain.resource"), "backend");
        gate.acquire("ref-1", "agent-a").unwrap();
        let kind_err = gate.acquire("ref-2", "agent-b").unwrap_err();
        assert_eq!(
            kind_err.error().evidence.get("dimension"),
            Some(&ScalarValue::String("kind".into()))
        );
        assert_eq!(
            kind_err.error().evidence.get("current"),
            Some(&ScalarValue::Int(1))
        );
        let records = gate.list_records();
        assert_eq!(
            records.iter().map(|record| record.lease_count).sum::<u64>(),
            1
        );

        policy.max_leases_by_ref.insert("ref-1".into(), 1);
        let mut gate = ResourceGate::with_quota_policy(policy);
        gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");
        gate.acquire("ref-1", "agent-a").unwrap();
        let ref_err = gate.acquire("ref-1", "agent-b").unwrap_err();
        assert_eq!(
            ref_err.error().evidence.get("dimension"),
            Some(&ScalarValue::String("ref_id".into()))
        );
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

    #[test]
    fn custom_election_policy_only_sees_prefiltered_candidates() {
        struct PreferB<'a> {
            seen: &'a RefCell<Vec<ElectionCandidate>>,
        }

        impl ElectionPolicy for PreferB<'_> {
            fn select(&self, candidates: &[ElectionCandidate]) -> Option<AgentId> {
                self.seen.borrow_mut().extend_from_slice(candidates);
                candidates
                    .iter()
                    .find(|candidate| candidate.agent_id == "agent-b")
                    .map(|candidate| candidate.agent_id.clone())
                    .or_else(|| Some("sleeping-agent".into()))
            }
        }

        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        runtime.register_agent(agent("agent-a", 10)).unwrap();
        runtime.register_agent(agent("agent-b", 1)).unwrap();

        let mut observer = agent("observer-agent", 99);
        observer.participation = AgentParticipation::Observer;
        runtime.register_agent(observer).unwrap();

        let mut helper = agent("helper-agent", 98);
        helper.participation = AgentParticipation::ExplicitHelper;
        runtime.register_agent(helper).unwrap();

        let mut empty_accepts = agent("empty-accepts-agent", 97);
        empty_accepts.accepts.clear();
        runtime.register_agent(empty_accepts).unwrap();

        let mut no_match = agent("no-match-agent", 96);
        no_match.accepts = vec![ScopeRuleSpec::BySchemaPrefix {
            prefix: "other.".into(),
        }];
        runtime.register_agent(no_match).unwrap();

        runtime.register_agent(agent("sleeping-agent", 95)).unwrap();
        runtime.register_agent(agent("stopped-agent", 94)).unwrap();

        for agent_id in [
            "agent-a",
            "agent-b",
            "observer-agent",
            "helper-agent",
            "empty-accepts-agent",
            "no-match-agent",
            "stopped-agent",
        ] {
            runtime.start_agent(agent_id, &mut backend).unwrap();
        }
        runtime.stop_agent("stopped-agent", &mut backend).unwrap();

        let seen = RefCell::new(Vec::new());
        let policy = PreferB { seen: &seen };

        assert_eq!(
            runtime.select_accepting(&envelope()),
            Some("agent-a".into())
        );
        assert_eq!(
            runtime.select_accepting_with_policy(&envelope(), &policy),
            Some("agent-b".into())
        );

        let mut seen_snapshot = seen.borrow().clone();
        seen_snapshot.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        assert_eq!(
            seen_snapshot,
            vec![
                ElectionCandidate {
                    agent_id: "agent-a".into(),
                    priority: 10
                },
                ElectionCandidate {
                    agent_id: "agent-b".into(),
                    priority: 1
                }
            ]
        );

        runtime.stop_agent("agent-b", &mut backend).unwrap();
        assert_eq!(
            runtime.select_accepting_with_policy(&envelope(), &policy),
            None
        );
    }

    #[test]
    fn election_policy_is_not_called_when_prefiltered_candidates_are_empty() {
        struct ShouldNotRun;

        impl ElectionPolicy for ShouldNotRun {
            fn select(&self, _candidates: &[ElectionCandidate]) -> Option<AgentId> {
                panic!("policy must not run without prefiltered candidates");
            }
        }

        let mut runtime = AgentRuntime::new();
        let mut backend = backend();
        let mut agent = agent("agent-a", 0);
        agent.accepts.clear();
        runtime.register_agent(agent).unwrap();
        runtime.start_agent("agent-a", &mut backend).unwrap();

        assert_eq!(
            runtime.select_accepting_with_policy(&envelope(), &ShouldNotRun),
            None
        );
    }
}
