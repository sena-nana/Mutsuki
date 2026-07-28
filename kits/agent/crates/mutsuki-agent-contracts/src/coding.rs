use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ArtifactRef, DocumentVersion, EditorDocumentRef, EditorWorkspaceRef, ResourceRef, TextSelection,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_id: String,
    pub title: String,
    pub status: PlanStepStatus,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanState {
    pub plan_id: String,
    pub revision: u64,
    #[serde(default)]
    pub source: Option<String>,
    pub steps: Vec<PlanStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoItemStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub item_id: String,
    pub title: String,
    pub status: TodoItemStatus,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub relation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoState {
    pub todo_id: String,
    pub revision: u64,
    pub items: Vec<TodoItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
    Proposed,
    Applied,
    Rejected,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileChangeDescriptor {
    pub change_id: String,
    pub workspace: EditorWorkspaceRef,
    pub document: EditorDocumentRef,
    pub base_version: DocumentVersion,
    pub status: FileChangeStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEditProposal {
    pub proposal_id: String,
    pub workspace: EditorWorkspaceRef,
    pub changes: Vec<FileChangeDescriptor>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingCommandRef {
    pub command_id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingDiagnostic {
    pub document: EditorDocumentRef,
    pub severity: CodingDiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<TextSelection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestOrBuildResult {
    pub result_id: String,
    pub kind: String,
    pub success: bool,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Approval,
    Clarification,
    PlanConfirm,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InteractionRequest {
    pub interaction_id: String,
    pub kind: InteractionKind,
    pub prompt: String,
    #[serde(default)]
    pub options: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InteractionResolution {
    pub interaction_id: String,
    pub accepted: bool,
    #[serde(default)]
    pub response: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageSnapshot {
    pub input_tokens: u64,
    pub reserved_tokens: u64,
    pub limit_tokens: u64,
    #[serde(default)]
    pub breakdown: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubAgentStatusUpdate {
    pub child_run_id: String,
    pub status: SubAgentRunStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
}

/// Shared metadata for coding-oriented wire events. Optional fields keep
/// existing generic Agent events forward-compatible.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentEventMeta {
    #[serde(default)]
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub timestamp_unix_ms: u64,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    #[serde(default)]
    pub visibility: String,
}

impl AgentEventMeta {
    pub fn new(event_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            event_id: event_id.into(),
            summary: summary.into(),
            visibility: "default".into(),
            ..Default::default()
        }
    }

    pub fn with_turn(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }
}

/// Re-export for coding event consumers that need artifact descriptors.
pub type CodingArtifactRef = ArtifactRef;
