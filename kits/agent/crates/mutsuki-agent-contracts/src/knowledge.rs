use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ResourceRef;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeCollection {
    pub collection_id: String,
    pub tenant_id: String,
    pub workspace_id: String,
    pub version: u64,
    #[serde(default)]
    pub index_state: KnowledgeIndexState,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeIndexState {
    #[default]
    Ready,
    Rebuilding,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub document_id: String,
    pub collection_id: String,
    pub title: String,
    pub content_type: KnowledgeContentType,
    pub content_hash: String,
    pub revision: u64,
    #[serde(default)]
    pub resource: Option<ResourceRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeContentType {
    Text,
    Markdown,
    Html,
}

impl KnowledgeContentType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::Html => "html",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRevision {
    pub document_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub chunk_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub collection_id: String,
    pub revision: u64,
    pub ordinal: u32,
    pub text: String,
    pub content_hash: String,
    #[serde(default)]
    pub embedding: Vec<f32>,
    #[serde(default)]
    pub resource: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestionPlan {
    pub collection_id: String,
    pub tenant_id: String,
    pub workspace_id: String,
    pub document_id: String,
    pub title: String,
    pub content_type: KnowledgeContentType,
    pub content: String,
    #[serde(default)]
    pub rebuild: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestionResult {
    pub document: KnowledgeDocument,
    pub chunks_written: usize,
    pub chunks_skipped: usize,
    pub revision: u64,
    pub deduplicated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetrievalQuery {
    pub query: String,
    pub tenant_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub collection_ids: Vec<String>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub hybrid: bool,
    #[serde(default)]
    pub rerank: bool,
    #[serde(default)]
    pub max_excerpt_chars: Option<usize>,
}

fn default_top_k() -> usize {
    8
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeCitation {
    pub collection_id: String,
    pub document_id: String,
    pub chunk_id: String,
    pub revision: u64,
    pub title: String,
    pub score: f32,
    pub excerpt: String,
    #[serde(default)]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RetrievalResult {
    pub citations: Vec<KnowledgeCitation>,
    #[serde(default)]
    pub provenance: Vec<KnowledgeProvenance>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeProvenance {
    pub collection_id: String,
    pub document_id: String,
    pub chunk_id: String,
    pub revision: u64,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeDeleteRequest {
    pub collection_id: String,
    pub tenant_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub document_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeDeleteResult {
    pub collection_id: String,
    pub documents_removed: usize,
    pub chunks_removed: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKnowledgePolicy {
    #[serde(default)]
    pub collection_allowlist: Vec<String>,
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}
