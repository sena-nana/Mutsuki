use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    DocumentVersion, EditorDocumentRef, EditorWorkspaceRef, FileChangeDescriptor, GitHeadIdentity,
    LspDiagnostic, ResourceRef, TextSelection, WorkspaceEditProposal,
};

/// Recent editor mutation observed by the product-side editor context.
///
/// Summaries must stay free of sensitive code bodies so ordinary logs remain safe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentEditEvent {
    pub event_id: String,
    pub document: EditorDocumentRef,
    pub version: DocumentVersion,
    pub editor_generation: u64,
    pub timestamp_unix_ms: u64,
    pub kind: RecentEditKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TextSelection>,
    pub summary: String,
    #[serde(default)]
    pub byte_delta: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecentEditKind {
    Inserted,
    Deleted,
    Replaced,
    Saved,
}

/// Planning path. Lightweight never starts a full Agent loop; ShortAgent is an
/// explicit short run used only when the request asks for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextEditPlanningPath {
    #[default]
    Lightweight,
    ShortAgent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextEditDiagnosticHint {
    pub document: EditorDocumentRef,
    pub diagnostic: LspDiagnostic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextEditDiffHint {
    pub path: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextEditRequest {
    pub request_id: String,
    pub workspace: EditorWorkspaceRef,
    /// Monotonic request generation used for supersession / debounce.
    pub generation: u64,
    pub editor_generation: u64,
    #[serde(default)]
    pub document_versions: Vec<(EditorDocumentRef, DocumentVersion)>,
    #[serde(default)]
    pub recent_edits: Vec<RecentEditEvent>,
    #[serde(default)]
    pub diagnostics: Vec<NextEditDiagnosticHint>,
    #[serde(default)]
    pub related_paths: Vec<String>,
    #[serde(default)]
    pub git_diff: Vec<NextEditDiffHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_git_head: Option<GitHeadIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    #[serde(default)]
    pub path: NextEditPlanningPath,
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    #[serde(default)]
    pub allow_multi_file: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_unix_ms: Option<u64>,
    #[serde(default)]
    pub now_unix_ms: u64,
    #[serde(default)]
    pub metadata: Value,
}

fn default_min_confidence() -> f64 {
    0.55
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextEditTarget {
    pub document: EditorDocumentRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TextSelection>,
    pub reason: String,
    pub confidence: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextEditCandidate {
    pub candidate_id: String,
    pub request_id: String,
    pub generation: u64,
    pub created_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub confidence: f64,
    pub reason: String,
    pub path: NextEditPlanningPath,
    pub targets: Vec<NextEditTarget>,
    pub proposal: WorkspaceEditProposal,
    pub expected_document_versions: Vec<(EditorDocumentRef, DocumentVersion)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_git_head: Option<GitHeadIdentity>,
    /// Multi-file proposals always require explicit product preview/approval.
    pub requires_preview: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_ref: Option<ResourceRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextEditFeedbackKind {
    Accepted,
    Rejected,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextEditFeedback {
    pub candidate_id: String,
    pub kind: NextEditFeedbackKind,
    pub timestamp_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextEditFeedbackStats {
    pub accepted: u64,
    pub rejected: u64,
    pub skipped: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextEditStaleConflict {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<EditorDocumentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<DocumentVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_version: Option<DocumentVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_git_head: Option<GitHeadIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_git_head: Option<GitHeadIdentity>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NextEditServiceRequest {
    IngestRecentEdit {
        event: RecentEditEvent,
    },
    Plan {
        request: Box<NextEditRequest>,
    },
    Validate {
        candidate_id: String,
        document_versions: Vec<(EditorDocumentRef, DocumentVersion)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git_head: Option<GitHeadIdentity>,
        #[serde(default)]
        now_unix_ms: u64,
    },
    Feedback {
        feedback: NextEditFeedback,
    },
    Cancel {
        generation: u64,
    },
    Stats,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NextEditServiceResponse {
    Ack,
    /// Empty candidate means low confidence / no interruption.
    Candidate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        candidate: Option<Box<NextEditCandidate>>,
    },
    Valid {
        candidate_id: String,
    },
    Stale {
        conflict: Box<NextEditStaleConflict>,
    },
    Superseded {
        generation: u64,
        active_generation: u64,
    },
    TimedOut {
        request_id: String,
    },
    Cancelled {
        generation: u64,
    },
    FeedbackRecorded {
        stats: NextEditFeedbackStats,
    },
    Stats {
        stats: NextEditFeedbackStats,
        active_generation: u64,
        cached_candidates: u64,
    },
}

/// Convenience constructor for single-file workspace edit proposals.
pub fn single_file_workspace_edit(
    proposal_id: impl Into<String>,
    workspace: EditorWorkspaceRef,
    change: FileChangeDescriptor,
    summary: impl Into<String>,
    details: Option<ResourceRef>,
) -> WorkspaceEditProposal {
    WorkspaceEditProposal {
        proposal_id: proposal_id.into(),
        workspace,
        changes: vec![change],
        summary: summary.into(),
        details,
    }
}
