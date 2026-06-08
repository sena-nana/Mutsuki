mod agent;
mod common;
mod envelope;
mod error;
mod operation;
mod resource;
mod scope;
mod strategy;
mod trace;

pub use agent::{AgentParticipation, AgentPhase, AgentSpec, SideEffectPolicy};
pub use common::{AgentId, EnvelopeId, RefId, ScalarValue, SpanId, TraceId};
pub use envelope::{Envelope, SourceRef};
pub use error::{
    ERR_AGENT_NOT_FOUND, ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED,
    ERR_RUNTIME_BACKEND_GENERATION_MISMATCH, ERR_SCOPE_NO_MATCH, ERR_SOURCE_UNREGISTERED,
    RuntimeError,
};
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

    use serde_json::Value;

    use super::*;

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
}
