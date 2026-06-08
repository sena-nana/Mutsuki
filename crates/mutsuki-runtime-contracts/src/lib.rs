use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type AgentId = String;
pub type EnvelopeId = String;
pub type RefId = String;
pub type SpanId = String;
pub type TraceId = String;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Spawn,
    Awake,
    Sleep,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentParticipation {
    PrimaryCandidate,
    Observer,
    ExplicitHelper,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectPolicy {
    ReadOnly,
    AllowExternal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub agent_id: AgentId,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub priority: i64,
    #[serde(default = "default_participation")]
    pub participation: AgentParticipation,
    #[serde(default)]
    pub accepts: Vec<ScopeRuleSpec>,
    #[serde(default)]
    pub strategy_id: String,
    #[serde(default = "default_side_effect_policy")]
    pub side_effect_policy: SideEffectPolicy,
}

fn default_participation() -> AgentParticipation {
    AgentParticipation::PrimaryCandidate
}

fn default_side_effect_policy() -> SideEffectPolicy {
    SideEffectPolicy::ReadOnly
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub source_id: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, ScalarValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub id: EnvelopeId,
    pub timestamp: f64,
    pub source: SourceRef,
    #[serde(default)]
    pub payload_schema_id: String,
    #[serde(default)]
    pub capabilities_required: Vec<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScopeRuleSpec {
    Always,
    Never,
    All { parts: Vec<ScopeRuleSpec> },
    Any { parts: Vec<ScopeRuleSpec> },
    BySchema { schema_id: String },
    BySchemaPrefix { prefix: String },
    BySourceId { source_id: String },
    BySourceKind { kind: String },
    ByCapability { capability: String },
    BySourceField { field: String, value: ScalarValue },
}

impl ScopeRuleSpec {
    pub fn matches(&self, envelope: &Envelope) -> bool {
        match self {
            ScopeRuleSpec::Always => true,
            ScopeRuleSpec::Never => false,
            ScopeRuleSpec::All { parts } => parts.iter().all(|part| part.matches(envelope)),
            ScopeRuleSpec::Any { parts } => parts.iter().any(|part| part.matches(envelope)),
            ScopeRuleSpec::BySchema { schema_id } => envelope.payload_schema_id == *schema_id,
            ScopeRuleSpec::BySchemaPrefix { prefix } => {
                envelope.payload_schema_id.starts_with(prefix)
            }
            ScopeRuleSpec::BySourceId { source_id } => envelope.source.source_id == *source_id,
            ScopeRuleSpec::BySourceKind { kind } => envelope.source.kind == *kind,
            ScopeRuleSpec::ByCapability { capability } => envelope
                .capabilities_required
                .iter()
                .any(|candidate| candidate == capability),
            ScopeRuleSpec::BySourceField { field, value } => {
                envelope.source.metadata.get(field) == Some(value)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationDescriptor {
    pub op_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub plugin_id: String,
    #[serde(default)]
    pub func_qualname: String,
    #[serde(default)]
    pub parameters_schema: Value,
    #[serde(default)]
    pub return_schema: Value,
    #[serde(default)]
    pub perms_rule_id: Option<String>,
    #[serde(default)]
    pub requires_capabilities: Vec<String>,
    #[serde(default = "default_is_tool")]
    pub is_tool: bool,
}

fn default_is_tool() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceDescriptor {
    pub source_id: String,
    pub kind: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Active,
    Unhealthy,
    Unregistering,
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationHandlerKey {
    pub plugin_id: String,
    pub plugin_generation: u64,
    pub op_id: String,
    pub handler_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationSnapshot {
    pub descriptor: OperationDescriptor,
    pub status: OperationStatus,
    pub key: OperationHandlerKey,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub descriptor: SourceDescriptor,
    pub plugin_id: String,
    pub plugin_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyResultStatus {
    Continue,
    WaitInput,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyResult {
    pub status: StrategyResultStatus,
    #[serde(default)]
    pub decision: Option<Value>,
    #[serde(default)]
    pub emitted: Vec<Envelope>,
    #[serde(default)]
    pub error: Option<RuntimeError>,
}

impl StrategyResult {
    pub fn wait_input() -> Self {
        Self {
            status: StrategyResultStatus::WaitInput,
            decision: None,
            emitted: Vec::new(),
            error: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub source: String,
    pub route: String,
    #[serde(default)]
    pub lost_capability: Option<String>,
    #[serde(default)]
    pub recovery: Option<String>,
    #[serde(default)]
    pub cause: Option<Box<RuntimeError>>,
    #[serde(default)]
    pub evidence: BTreeMap<String, ScalarValue>,
}

impl RuntimeError {
    pub fn new(
        code: impl Into<String>,
        source: impl Into<String>,
        route: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            source: source.into(),
            route: route.into(),
            lost_capability: None,
            recovery: None,
            cause: None,
            evidence: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceSpan {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    #[serde(default)]
    pub parent_span_id: Option<SpanId>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub attributes: BTreeMap<String, ScalarValue>,
    #[serde(default = "default_span_status")]
    pub status: SpanStatus,
}

fn default_span_status() -> SpanStatus {
    SpanStatus::Ok
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefDescriptor {
    pub ref_id: RefId,
    pub kind: String,
    pub schema_id_target: String,
    pub schema_version_target: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, ScalarValue>,
    #[serde(default)]
    pub lineage: Vec<RefId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeaseToken {
    pub token_id: String,
    pub ref_id: RefId,
    pub owner: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceRecord {
    pub descriptor: RefDescriptor,
    pub owner: String,
    #[serde(default)]
    pub lease_count: u64,
}

pub const ERR_AGENT_NOT_FOUND: &str = "agent.not_found";
pub const ERR_OPERATION_NOT_FOUND: &str = "operation.not_found";
pub const ERR_RUNTIME_BACKEND_FAILED: &str = "runtime.backend_failed";
pub const ERR_RUNTIME_BACKEND_GENERATION_MISMATCH: &str = "runtime.backend_generation_mismatch";
pub const ERR_SCOPE_NO_MATCH: &str = "scope.no_match";

#[cfg(test)]
mod tests {
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
