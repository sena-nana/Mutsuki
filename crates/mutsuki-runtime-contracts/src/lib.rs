mod agent;
mod common;
mod envelope;
mod error;
mod event;
mod operation;
mod resource;
mod scope;
mod strategy;
mod trace;

pub use agent::{AgentParticipation, AgentPhase, AgentSpec, SideEffectPolicy};
pub use common::{AgentId, EnvelopeId, RefId, ScalarValue, SpanId, TraceId};
pub use envelope::{Envelope, SourceRef};
pub use error::{
    ERR_AGENT_NOT_FOUND, ERR_CAPABILITY_EXHAUSTED, ERR_OPERATION_NOT_FOUND,
    ERR_RUNTIME_BACKEND_FAILED, ERR_RUNTIME_BACKEND_GENERATION_MISMATCH, ERR_SCOPE_NO_MATCH,
    ERR_SOURCE_UNREGISTERED, RuntimeError,
};
pub use event::{RuntimeEvent, RuntimeEventKind};
pub use operation::{
    OperationDescriptor, OperationHandlerKey, OperationSnapshot, OperationStatus, SourceDescriptor,
    SourceSnapshot,
};
pub use resource::{LeaseToken, RefDescriptor, ResourceRecord};
pub use scope::ScopeRuleSpec;
pub use strategy::{StrategyResult, StrategyResultStatus};
pub use trace::{SpanStatus, TraceSpan};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::de::DeserializeOwned;
    use serde_json::Value;

    use super::*;

    fn assert_missing_fields_fail<T: DeserializeOwned>(value: serde_json::Value) {
        assert!(serde_json::from_value::<T>(value).is_err());
    }

    #[test]
    fn scope_rule_matches_envelope_by_schema_and_capability() {
        let envelope = Envelope {
            id: "env-1".into(),
            timestamp: 0.0,
            source: SourceRef {
                source_id: "src:default".into(),
                kind: "im".into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "mutsukibot.message".into(),
            capabilities_required: vec!["im.text".into()],
            payload: Value::Null,
        };

        let rule = ScopeRuleSpec::All {
            parts: vec![
                ScopeRuleSpec::BySchema {
                    schema_id: "mutsukibot.message".into(),
                },
                ScopeRuleSpec::ByCapability {
                    capability: "im.text".into(),
                },
            ],
        };

        assert!(rule.matches(&envelope));
    }

    #[test]
    fn pure_contracts_roundtrip_json() {
        let descriptor = OperationDescriptor {
            op_id: "echo.echo".into(),
            name: "echo".into(),
            description: "Echo input".into(),
            plugin_id: "echo".into(),
            func_qualname: "EchoPlugin.echo".into(),
            parameters_schema: serde_json::json!({"type": "object"}),
            return_schema: serde_json::json!({"type": "string"}),
            perms_rule_id: Some("public".into()),
            requires_capabilities: vec!["send_message".into()],
            is_tool: true,
        };
        let json = serde_json::to_string(&descriptor).unwrap();
        let decoded: OperationDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn runtime_event_roundtrip_json() {
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "source_id".into(),
            ScalarValue::String("source:test".into()),
        );
        let event = RuntimeEvent {
            sequence: 1,
            kind: RuntimeEventKind::Routing,
            name: "runtime.publish".into(),
            agent_id: Some("agent-a".into()),
            attributes,
            error: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        let decoded: RuntimeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(ERR_CAPABILITY_EXHAUSTED, "capability.exhausted");
    }

    #[test]
    fn missing_contract_fields_fail_deserialization() {
        assert_missing_fields_fail::<AgentSpec>(serde_json::json!({
            "agent_id": "agent-a"
        }));
        assert_missing_fields_fail::<Envelope>(serde_json::json!({
                "id": "env-1",
                "timestamp": 0.0,
                "source": {
                    "source_id": "source:test",
                    "kind": "test",
                    "metadata": {}
                }
        }));
        assert_missing_fields_fail::<OperationDescriptor>(serde_json::json!({
            "op_id": "test.echo",
            "name": "echo"
        }));
        assert_missing_fields_fail::<StrategyResult>(serde_json::json!({
            "status": "wait_input"
        }));
        assert_missing_fields_fail::<TraceSpan>(serde_json::json!({
            "trace_id": "trace-1",
            "span_id": "span-1"
        }));
        assert_missing_fields_fail::<RuntimeEvent>(serde_json::json!({
            "sequence": 7,
            "kind": "trace",
            "name": "trace.span"
        }));
    }
}
