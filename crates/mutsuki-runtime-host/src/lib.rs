mod host;
mod operation;

pub use host::NativeRuntimeHost;
pub use operation::NativeOperation;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mutsuki_runtime_contracts::{
        AgentParticipation, AgentSpec, Envelope, OperationDescriptor, ScopeRuleSpec,
        SideEffectPolicy, SourceDescriptor, SourceRef, SourceSnapshot,
    };
    use mutsuki_runtime_core::{AgentRuntime, BackendPayload};
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

    #[test]
    fn native_host_runs_agent_input_and_operation_without_python() {
        let mut runtime = AgentRuntime::new();
        let mut host = NativeRuntimeHost::new();
        host.register_source(SourceSnapshot {
            descriptor: SourceDescriptor {
                source_id: "source:test".into(),
                kind: "test".into(),
                capabilities: Vec::new(),
                description: String::new(),
            },
            plugin_id: "native".into(),
            plugin_generation: 0,
        });
        host.register_operation(NativeOperation::new(
            OperationDescriptor {
                op_id: "native.echo".into(),
                name: "echo".into(),
                description: String::new(),
                plugin_id: "native".into(),
                func_qualname: String::new(),
                parameters_schema: json!({}),
                return_schema: json!({}),
                perms_rule_id: None,
                requires_capabilities: Vec::new(),
                is_tool: true,
            },
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
        host.register_source(SourceSnapshot {
            descriptor: SourceDescriptor {
                source_id: "source:test".into(),
                kind: "test".into(),
                capabilities: Vec::new(),
                description: String::new(),
            },
            plugin_id: "native".into(),
            plugin_generation: 0,
        });
        host.register_operation(NativeOperation::new(
            OperationDescriptor {
                op_id: "native.echo".into(),
                name: "echo".into(),
                description: String::new(),
                plugin_id: "native".into(),
                func_qualname: String::new(),
                parameters_schema: json!({}),
                return_schema: json!({}),
                perms_rule_id: None,
                requires_capabilities: Vec::new(),
                is_tool: true,
            },
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
}
