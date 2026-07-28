use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{PermissionRequest, ResourceRef, ToolSideEffect};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AgentWorkspaceRef {
    pub workspace_id: String,
    pub root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMount {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRequirement {
    #[serde(default)]
    pub require_isolated: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub allow_workspace_write: bool,
    #[serde(default)]
    pub mounts: Vec<WorkspaceMount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionLimits {
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: u32,
}

fn default_timeout_ms() -> u64 {
    30_000
}

fn default_max_output_bytes() -> u64 {
    256 * 1024
}

fn default_max_concurrency() -> u32 {
    4
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout_ms(),
            max_output_bytes: default_max_output_bytes(),
            max_concurrency: default_max_concurrency(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub artifact_id: String,
    pub media_type: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<ResourceRef>,
    /// Logical artifact class (file, report, screenshot, build, link, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseRisk {
    ReadOnly,
    ReversibleWrite,
    DestructiveWrite,
    ProcessExec,
    Network,
    OutsideWorkspace,
    SecretAccess,
}

impl ComputerUseRisk {
    pub fn requires_approval(self) -> bool {
        !matches!(self, Self::ReadOnly | Self::ReversibleWrite)
    }

    pub fn side_effect(self) -> ToolSideEffect {
        match self {
            Self::ReadOnly => ToolSideEffect::WorkspaceRead,
            Self::ReversibleWrite | Self::DestructiveWrite | Self::OutsideWorkspace => {
                ToolSideEffect::WorkspaceWrite
            }
            Self::ProcessExec | Self::Network | Self::SecretAccess => ToolSideEffect::ExternalWrite,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePathRequest {
    pub workspace: AgentWorkspaceRef,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FsEntry {
    pub path: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FsReadResult {
    pub path: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<ResourceRef>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FsPatchRequest {
    pub workspace: AgentWorkspaceRef,
    pub path: String,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: String,
    pub line: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessExecRequest {
    pub workspace: AgentWorkspaceRef,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default)]
    pub limits: ExecutionLimits,
    #[serde(default)]
    pub allow_network: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessExecResult {
    pub exit_code: i32,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_ref: Option<ResourceRef>,
    pub truncated: bool,
    pub cancelled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserNavigateRequest {
    pub url: String,
    #[serde(default)]
    pub limits: ExecutionLimits,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserSnapshotResult {
    pub url: String,
    pub title: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<ResourceRef>,
    pub artifact: Option<ArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComputerUseActionPlan {
    pub action_id: String,
    pub operation: String,
    pub risk: ComputerUseRisk,
    pub summary: String,
    pub approval: PermissionRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ComputerUseServiceRequest {
    List {
        request: WorkspacePathRequest,
    },
    Read {
        request: WorkspacePathRequest,
        #[serde(default = "default_max_output_bytes")]
        max_bytes: u64,
    },
    Stat {
        request: WorkspacePathRequest,
    },
    Write {
        workspace: AgentWorkspaceRef,
        path: String,
        content: String,
        #[serde(default)]
        create: bool,
        #[serde(default)]
        overwrite: bool,
        session_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_version: Option<u64>,
        #[serde(default)]
        approved: bool,
    },
    Delete {
        request: WorkspacePathRequest,
        session_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_version: Option<u64>,
        #[serde(default)]
        approved: bool,
    },
    Move {
        workspace: AgentWorkspaceRef,
        from: String,
        to: String,
        session_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_version: Option<u64>,
        #[serde(default)]
        approved: bool,
    },
    Patch {
        request: FsPatchRequest,
        session_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_version: Option<u64>,
        #[serde(default)]
        approved: bool,
    },
    Glob {
        workspace: AgentWorkspaceRef,
        pattern: String,
    },
    Grep {
        workspace: AgentWorkspaceRef,
        pattern: String,
        #[serde(default)]
        path: Option<String>,
    },
    Exec {
        request: ProcessExecRequest,
        session_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_version: Option<u64>,
        #[serde(default)]
        approved: bool,
    },
    BrowserSnapshot {
        request: BrowserNavigateRequest,
        session_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_version: Option<u64>,
        #[serde(default)]
        approved: bool,
    },
    Plan {
        operation: String,
        risk: ComputerUseRisk,
        summary: String,
        session_id: String,
        turn_id: String,
        version: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preview: Option<Value>,
    },
    Cancel {
        handle_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComputerUseServiceResponse {
    Entries { entries: Vec<FsEntry> },
    Read(FsReadResult),
    Stat(FsEntry),
    Written { path: String },
    Deleted { path: String },
    Moved { from: String, to: String },
    Patched { path: String },
    Paths { paths: Vec<String> },
    Grep { matches: Vec<GrepMatch> },
    Exec(ProcessExecResult),
    Browser(BrowserSnapshotResult),
    Plan(ComputerUseActionPlan),
    Ack,
}
