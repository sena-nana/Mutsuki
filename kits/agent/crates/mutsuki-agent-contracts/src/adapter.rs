use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentModelGenerateRequest, AgentModelGenerateResult, AgentModelStreamResult,
    AgentToolDescriptor,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRef {
    pub credential_id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapability {
    pub context_window: u64,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub structured_output: bool,
    #[serde(default)]
    pub images: bool,
    #[serde(default)]
    pub audio_input: bool,
    #[serde(default)]
    pub transcription: bool,
    #[serde(default)]
    pub speech_synthesis: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub embedding: bool,
    #[serde(default)]
    pub rerank: bool,
    /// Inline prefix/suffix code completion without Agent session/tool loops.
    #[serde(default)]
    pub code_completion: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProtocolAdapterDescriptor {
    pub adapter_id: String,
    pub protocol: String,
    pub version: String,
    pub runner_id: String,
    pub capability: ModelCapability,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderInstanceDescriptor {
    pub provider_id: String,
    pub adapter_id: String,
    pub endpoint: String,
    pub credential: CredentialRef,
    #[serde(default)]
    pub models: BTreeMap<String, ModelCapability>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub compatibility: BTreeMap<String, Value>,
    #[serde(default)]
    pub remote_execution_allowed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelector {
    #[serde(default)]
    pub allowed_adapter_ids: Vec<String>,
    #[serde(default)]
    pub allowed_provider_ids: Vec<String>,
    #[serde(default)]
    pub preferred_model: Option<String>,
    #[serde(default)]
    pub max_cost_microunits: Option<u64>,
    #[serde(default)]
    pub max_latency_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCost {
    pub input_microunits: u64,
    pub output_microunits: u64,
    pub total_microunits: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorClass {
    Retryable,
    NonRetryable,
    Authentication,
    RateLimited,
    Timeout,
    Cancelled,
    Protocol,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub class: ProtocolErrorClass,
    pub message: String,
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStreamEvent {
    MessageDelta {
        sequence: u64,
        text: String,
    },
    ReasoningDelta {
        sequence: u64,
        text: String,
    },
    ToolCallDelta {
        sequence: u64,
        value: Value,
    },
    Usage {
        sequence: u64,
        value: crate::AgentUsage,
    },
    Completed {
        sequence: u64,
        result: Box<AgentModelStreamResult>,
    },
    Failed {
        sequence: u64,
        error: ProtocolError,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelGenerateRequest {
    pub request: AgentModelGenerateRequest,
    #[serde(default)]
    pub tools: Vec<AgentToolDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Value>,
}

pub type ModelGenerateResult = AgentModelGenerateResult;
