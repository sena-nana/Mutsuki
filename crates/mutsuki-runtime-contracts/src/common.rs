use serde::{Deserialize, Serialize};

pub type RefId = String;
pub type TaskId = String;
pub type RunnerId = String;
pub type PluginId = String;
pub type SurfaceId = String;
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
