use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EnvelopeId, ScalarValue};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub source_id: String,
    pub kind: String,
    pub metadata: BTreeMap<String, ScalarValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub id: EnvelopeId,
    pub timestamp: f64,
    pub source: SourceRef,
    pub payload_schema_id: String,
    pub capabilities_required: Vec<String>,
    pub payload: Value,
}
