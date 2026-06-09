use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationDescriptor {
    pub op_id: String,
    pub name: String,
    pub description: String,
    pub plugin_id: String,
    pub func_qualname: String,
    pub parameters_schema: Value,
    pub return_schema: Value,
    pub perms_rule_id: Option<String>,
    pub requires_capabilities: Vec<String>,
    pub is_tool: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceDescriptor {
    pub source_id: String,
    pub kind: String,
    pub capabilities: Vec<String>,
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
