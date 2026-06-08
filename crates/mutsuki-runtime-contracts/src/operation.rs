use serde::{Deserialize, Serialize};
use serde_json::Value;

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
