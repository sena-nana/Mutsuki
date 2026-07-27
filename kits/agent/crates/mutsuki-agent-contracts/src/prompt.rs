use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPromptGetRequest {
    pub template_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentPromptTemplate {
    pub template_id: String,
    pub body: String,
    #[serde(default)]
    pub variables: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentPromptRenderRequest {
    pub template_id: String,
    #[serde(default)]
    pub variables: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPromptRenderResult {
    pub template_id: String,
    pub text: String,
}
