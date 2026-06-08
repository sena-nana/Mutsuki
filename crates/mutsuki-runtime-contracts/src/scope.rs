use serde::{Deserialize, Serialize};

use crate::{Envelope, ScalarValue};

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
