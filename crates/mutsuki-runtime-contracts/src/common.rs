use serde::{Deserialize, Serialize};

pub type RefId = String;
pub type TaskId = String;
pub type RunnerId = String;
pub type PluginId = String;
pub type ExecutorId = String;
pub type BindingId = String;
pub type ProtocolId = String;
pub type TaskLeaseId = String;
pub type ResourceCellId = String;
pub type ResourceLeaseId = String;
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
