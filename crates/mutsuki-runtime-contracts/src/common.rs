use serde::{Deserialize, Serialize};

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
