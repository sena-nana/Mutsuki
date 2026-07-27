use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{PermissionRequest, ResourceRef};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LspWorkspaceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspDocumentId {
    pub workspace: LspWorkspaceId,
    pub uri: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LspDiagnostic {
    pub range: LspRange,
    #[serde(default)]
    pub severity: Option<u32>,
    #[serde(default)]
    pub code: Option<Value>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LspDocumentSnapshot {
    pub document: LspDocumentId,
    pub language_id: String,
    pub version: i64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LspQueryResult {
    pub summary: String,
    #[serde(default)]
    pub inline: Option<Value>,
    #[serde(default)]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspServerDescriptor {
    pub server_id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub workspace_uri: String,
    #[serde(default)]
    pub initialization_options: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspWorkspaceState {
    Starting,
    Ready,
    Restarting,
    Failed,
    Draining,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspWorkspaceStatus {
    pub workspace: LspWorkspaceId,
    pub server_id: String,
    pub state: LspWorkspaceState,
    pub open_documents: usize,
    pub restart_count: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LspWorkspaceEditPlan {
    pub action_id: String,
    pub operation: String,
    pub edit: LspQueryResult,
    pub approval: PermissionRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum LspServiceRequest {
    OpenWorkspace {
        workspace: LspWorkspaceId,
        descriptor: LspServerDescriptor,
    },
    RestartWorkspace {
        workspace: LspWorkspaceId,
    },
    CloseWorkspace {
        workspace: LspWorkspaceId,
    },
    WorkspaceStatus {
        workspace: LspWorkspaceId,
    },
    OpenDocument {
        document: LspDocumentSnapshot,
    },
    ChangeDocument {
        document: LspDocumentId,
        version: i64,
        text: String,
    },
    SaveDocument {
        document: LspDocumentId,
        #[serde(default)]
        text: Option<String>,
    },
    CloseDocument {
        document: LspDocumentId,
    },
    Definition {
        document: LspDocumentId,
        position: LspPosition,
    },
    Declaration {
        document: LspDocumentId,
        position: LspPosition,
    },
    Implementation {
        document: LspDocumentId,
        position: LspPosition,
    },
    Hover {
        document: LspDocumentId,
        position: LspPosition,
    },
    References {
        document: LspDocumentId,
        position: LspPosition,
    },
    RenamePlan {
        document: LspDocumentId,
        position: LspPosition,
        new_name: String,
        session_id: String,
        turn_id: String,
    },
    DocumentSymbols {
        document: LspDocumentId,
    },
    WorkspaceSymbols {
        workspace: LspWorkspaceId,
        query: String,
    },
    Diagnostics {
        document: LspDocumentId,
    },
    CodeActionsPlan {
        document: LspDocumentId,
        range: LspRange,
        #[serde(default)]
        context: Value,
        session_id: String,
        turn_id: String,
    },
    Formatting {
        document: LspDocumentId,
        #[serde(default)]
        options: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum LspServiceResponse {
    Ack,
    Query(LspQueryResult),
    Diagnostics(Vec<LspDiagnostic>),
    WorkspaceStatus(LspWorkspaceStatus),
    EditPlan(LspWorkspaceEditPlan),
}
