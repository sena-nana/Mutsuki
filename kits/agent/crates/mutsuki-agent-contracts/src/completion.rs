use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CredentialRef, DocumentVersion, EditorDocumentRef, TextPosition, TextSelection};

/// Lightweight nearby context for completion; never a full Agent context pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeCompletionHintKind {
    NearbyFile,
    Import,
    Symbol,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeCompletionHint {
    pub kind: CodeCompletionHintKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeCompletionStopReason {
    #[default]
    Stop,
    Length,
    Timeout,
    Cancelled,
    LowConfidence,
    Empty,
    ContentFilter,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeCompletionStatus {
    Ready,
    Empty,
    Cancelled,
    Superseded,
    TimedOut,
    LowConfidence,
    StaleDocument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeCompletionFeedbackAction {
    Accept,
    Reject,
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeCompletionRequest {
    pub request_id: String,
    /// Monotonic per-editor generation; newer requests supersede older ones.
    pub generation: u64,
    pub workspace_id: String,
    pub document: EditorDocumentRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    pub document_version: DocumentVersion,
    pub cursor: TextPosition,
    pub prefix: String,
    pub suffix: String,
    #[serde(default)]
    pub nearby_hints: Vec<CodeCompletionHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(default = "default_max_candidates")]
    pub max_candidates: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

fn default_max_candidates() -> u32 {
    3
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeCompletionCandidate {
    pub candidate_id: String,
    pub range: TextSelection,
    pub insert_text: String,
    pub confidence: f64,
    #[serde(default)]
    pub stop_reason: CodeCompletionStopReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeCompletionLatency {
    pub total_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_ms: Option<u64>,
    #[serde(default)]
    pub cache_hit: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeCompletionResponse {
    pub request_id: String,
    pub generation: u64,
    pub document: EditorDocumentRef,
    pub document_version: DocumentVersion,
    pub status: CodeCompletionStatus,
    #[serde(default)]
    pub candidates: Vec<CodeCompletionCandidate>,
    pub latency: CodeCompletionLatency,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<CodeCompletionStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl CodeCompletionResponse {
    /// Reject display when the editor document moved past the response version.
    pub fn is_stale(&self, current_version: DocumentVersion) -> bool {
        self.document_version != current_version
            || matches!(self.status, CodeCompletionStatus::StaleDocument)
    }

    /// Reject display when a newer generation superseded this response.
    pub fn is_superseded(&self, latest_generation: u64) -> bool {
        self.generation < latest_generation
            || matches!(self.status, CodeCompletionStatus::Superseded)
    }

    pub fn may_display(&self, current_version: DocumentVersion, latest_generation: u64) -> bool {
        matches!(self.status, CodeCompletionStatus::Ready)
            && !self.is_stale(current_version)
            && !self.is_superseded(latest_generation)
            && !self.candidates.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeCompletionFeedback {
    pub request_id: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    pub action: CodeCompletionFeedbackAction,
    pub document: EditorDocumentRef,
    pub document_version: DocumentVersion,
    /// Opaque language / path class only — never full source body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    #[serde(default)]
    pub prefix_hash: String,
    #[serde(default)]
    pub suffix_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeCompletionCapabilities {
    pub max_prefix_bytes: u64,
    pub max_suffix_bytes: u64,
    pub max_hint_bytes: u64,
    pub max_concurrent: u32,
    pub first_result_deadline_ms: u64,
    pub total_deadline_ms: u64,
    pub cache_enabled: bool,
    pub min_confidence: f64,
    #[serde(default)]
    pub adapter_id: Option<String>,
    #[serde(default)]
    pub provider_instance_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for CodeCompletionCapabilities {
    fn default() -> Self {
        Self {
            max_prefix_bytes: 8 * 1024,
            max_suffix_bytes: 4 * 1024,
            max_hint_bytes: 2 * 1024,
            max_concurrent: 2,
            first_result_deadline_ms: 250,
            total_deadline_ms: 800,
            cache_enabled: true,
            min_confidence: 0.35,
            adapter_id: None,
            provider_instance_id: None,
            model: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CodeCompletionServiceRequest {
    Complete {
        request: CodeCompletionRequest,
    },
    Cancel {
        generation: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
    Feedback {
        feedback: CodeCompletionFeedback,
    },
    Capabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodeCompletionServiceResponse {
    Complete {
        response: CodeCompletionResponse,
    },
    Cancelled {
        generation: u64,
    },
    FeedbackAccepted {
        request_id: String,
        action: CodeCompletionFeedbackAction,
    },
    Capabilities {
        capabilities: CodeCompletionCapabilities,
    },
}
