use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentPromptFragment, ContextItemRef, ResourceRef, ToolSideEffect};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    StreamableHttp,
    Sse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerManifest {
    pub server_id: String,
    pub source: String,
    pub transport: McpTransportKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Explicit allowlist only. MCP servers never inherit the full host environment.
    #[serde(default)]
    pub env_allowlist: Vec<(String, String)>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerState {
    Connecting,
    Ready,
    Failed,
    Draining,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub server_id: String,
    pub state: McpServerState,
    pub catalog_generation: u64,
    pub tool_count: usize,
    pub resource_count: usize,
    pub prompt_count: usize,
    pub restart_count: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpToolAnnotations {
    #[serde(default)]
    pub read_only_hint: Option<bool>,
    #[serde(default)]
    pub destructive_hint: Option<bool>,
    #[serde(default)]
    pub idempotent_hint: Option<bool>,
    #[serde(default)]
    pub open_world_hint: Option<bool>,
}

impl McpToolAnnotations {
    pub fn side_effect(&self) -> ToolSideEffect {
        if self.destructive_hint == Some(true) || self.open_world_hint == Some(true) {
            ToolSideEffect::ExternalWrite
        } else if self.read_only_hint == Some(true) {
            ToolSideEffect::ExternalRead
        } else {
            ToolSideEffect::ExternalWrite
        }
    }

    pub fn requires_approval(&self) -> bool {
        self.destructive_hint == Some(true)
            || self.open_world_hint == Some(true)
            || self.read_only_hint != Some(true)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub server_id: String,
    pub name: String,
    pub namespaced_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: McpToolAnnotations,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpResourceInfo {
    pub server_id: String,
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpPromptInfo {
    pub server_id: String,
    pub name: String,
    pub namespaced_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpCatalog {
    pub generation: u64,
    #[serde(default)]
    pub tools: Vec<McpToolInfo>,
    #[serde(default)]
    pub resources: Vec<McpResourceInfo>,
    #[serde(default)]
    pub prompts: Vec<McpPromptInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpCallResult {
    pub summary: String,
    #[serde(default)]
    pub inline: Option<Value>,
    #[serde(default)]
    pub details: Option<ResourceRef>,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpResourceReadResult {
    pub uri: String,
    pub summary: String,
    #[serde(default)]
    pub item: Option<ContextItemRef>,
    #[serde(default)]
    pub details: Option<ResourceRef>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpPromptGetResult {
    pub prompt: McpPromptInfo,
    pub fragments: Vec<AgentPromptFragment>,
    #[serde(default)]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum McpServiceRequest {
    Connect {
        manifest: McpServerManifest,
    },
    Disconnect {
        server_id: String,
    },
    Reload {
        server_id: String,
    },
    Status {
        server_id: String,
    },
    ListServers,
    Catalog {
        #[serde(default)]
        server_id: Option<String>,
        #[serde(default)]
        generation: Option<u64>,
    },
    PinTurn {
        generation: u64,
    },
    CallTool {
        namespaced_name: String,
        #[serde(default)]
        arguments: Value,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    ReadResource {
        server_id: String,
        uri: String,
    },
    GetPrompt {
        namespaced_name: String,
        #[serde(default)]
        arguments: Value,
    },
    Cancel {
        server_id: String,
        request_id: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum McpServiceResponse {
    Ack,
    Status(McpServerStatus),
    Servers(Vec<McpServerStatus>),
    Catalog(McpCatalog),
    Call(McpCallResult),
    Resource(McpResourceReadResult),
    Prompt(McpPromptGetResult),
}

pub fn mcp_namespaced_name(server_id: &str, name: &str) -> String {
    format!("{server_id}/{name}")
}
