use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ResourceRef;

/// Product-owned workspace identity for editor context. Distinct from
/// [`crate::AgentWorkspaceRef`] (filesystem/computer-use root).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorWorkspaceRef {
    pub workspace_id: String,
    #[serde(default)]
    pub folders: Vec<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EditorDocumentRef {
    pub workspace_id: String,
    pub uri: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DocumentVersion(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSelection {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnsavedBufferRef {
    pub document: EditorDocumentRef,
    pub version: DocumentVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_text: Option<String>,
    #[serde(default)]
    pub byte_length: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenDocumentSnapshot {
    pub document: EditorDocumentRef,
    pub version: DocumentVersion,
    #[serde(default)]
    pub language_id: Option<String>,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<TextSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffer: Option<UnsavedBufferRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorContextSnapshot {
    pub snapshot_id: String,
    pub workspace: EditorWorkspaceRef,
    pub generation: u64,
    #[serde(default)]
    pub active_document: Option<EditorDocumentRef>,
    #[serde(default)]
    pub documents: Vec<OpenDocumentSnapshot>,
    #[serde(default)]
    pub supports_workspace_edit_preview: bool,
    #[serde(default)]
    pub supports_workspace_edit_apply: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditorContextChangeEvent {
    WorkspaceChanged {
        workspace: EditorWorkspaceRef,
        generation: u64,
    },
    DocumentOpened {
        document: OpenDocumentSnapshot,
        generation: u64,
    },
    DocumentChanged {
        document: EditorDocumentRef,
        version: DocumentVersion,
        generation: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        buffer: Option<UnsavedBufferRef>,
    },
    DocumentSaved {
        document: EditorDocumentRef,
        version: DocumentVersion,
        generation: u64,
    },
    DocumentClosed {
        document: EditorDocumentRef,
        generation: u64,
    },
    ActiveSelectionChanged {
        document: EditorDocumentRef,
        selection: TextSelection,
        generation: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorContextPolicy {
    #[serde(default)]
    pub allowed_workspace_ids: Vec<String>,
    #[serde(default)]
    pub allow_unsaved_buffers: bool,
    #[serde(default)]
    pub allow_selection: bool,
    #[serde(default)]
    pub allow_clipboard: bool,
    #[serde(default)]
    pub max_inline_buffer_bytes: u64,
}

impl EditorContextPolicy {
    pub fn permissive() -> Self {
        Self {
            allowed_workspace_ids: Vec::new(),
            allow_unsaved_buffers: true,
            allow_selection: true,
            allow_clipboard: false,
            max_inline_buffer_bytes: 64 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditorContextServiceRequest {
    GetWorkspace,
    GetActiveDocument,
    ListOpenDocuments {
        #[serde(default)]
        visible_only: bool,
    },
    ReadUnsavedBuffer {
        document: EditorDocumentRef,
        expected_version: DocumentVersion,
    },
    Snapshot {
        #[serde(default)]
        turn_id: Option<String>,
    },
    Capabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditorContextServiceResponse {
    Workspace {
        workspace: EditorWorkspaceRef,
        generation: u64,
    },
    ActiveDocument {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        document: Option<OpenDocumentSnapshot>,
        generation: u64,
    },
    OpenDocuments {
        documents: Vec<OpenDocumentSnapshot>,
        generation: u64,
    },
    UnsavedBuffer {
        buffer: UnsavedBufferRef,
        generation: u64,
    },
    Snapshot {
        snapshot: EditorContextSnapshot,
    },
    Capabilities {
        supports_workspace_edit_preview: bool,
        supports_workspace_edit_apply: bool,
        generation: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEditConflict {
    pub document: EditorDocumentRef,
    pub expected_version: DocumentVersion,
    pub actual_version: DocumentVersion,
    pub message: String,
}

impl WorkspaceEditConflict {
    pub fn stale(
        document: EditorDocumentRef,
        expected_version: DocumentVersion,
        actual_version: DocumentVersion,
    ) -> Self {
        Self {
            document,
            expected_version,
            actual_version,
            message: format!(
                "workspace edit expected version {}, observed {}",
                expected_version.0, actual_version.0
            ),
        }
    }
}
