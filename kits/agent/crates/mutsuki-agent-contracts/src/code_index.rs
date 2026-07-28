use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ResourceRef;

/// Workspace identity for Code Index. Must stay consistent with LSP workspace ids.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CodeWorkspaceRef {
    pub workspace_id: String,
    pub root: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeIndexSource {
    Parser,
    Lsp,
    Text,
    Overlay,
    Semantic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeIndexProvenance {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<CodeTextRange>,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_revision: Option<String>,
    pub source: CodeIndexSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeTextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeTextRange {
    pub start: CodeTextPosition,
    pub end: CodeTextPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexedFile {
    pub path: String,
    pub language: String,
    pub content_hash: String,
    pub content_revision: u64,
    pub byte_length: u64,
    #[serde(default)]
    pub line_count: u32,
    #[serde(default)]
    pub ignored: bool,
    pub provenance: CodeIndexProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSymbolKind {
    Module,
    Function,
    Method,
    Struct,
    Class,
    Enum,
    Trait,
    Interface,
    Type,
    Const,
    Variable,
    Field,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeSymbol {
    pub symbol_id: String,
    pub name: String,
    pub kind: CodeSymbolKind,
    pub path: String,
    pub range: CodeTextRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    pub provenance: CodeIndexProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolRelationKind {
    References,
    Defines,
    Implements,
    Extends,
    Calls,
    Contains,
    Imports,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolRelation {
    pub from_symbol_id: String,
    pub to_symbol_id: String,
    pub kind: SymbolRelationKind,
    pub provenance: CodeIndexProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleDependency {
    pub from_path: String,
    pub to_module: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_path: Option<String>,
    pub provenance: CodeIndexProvenance,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeIndexState {
    #[default]
    Ready,
    Indexing,
    Rebuilding,
    Corrupted,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeIndexRevision {
    pub workspace: CodeWorkspaceRef,
    pub revision: u64,
    pub checkpoint_id: String,
    pub state: CodeIndexState,
    pub indexed_files: usize,
    pub symbol_count: usize,
    pub dependency_count: usize,
    #[serde(default)]
    pub overlay_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSearchMode {
    Text,
    Regex,
    Symbol,
    Semantic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeSearchQuery {
    pub workspace: CodeWorkspaceRef,
    pub query: String,
    pub mode: CodeSearchMode,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    #[serde(default)]
    pub include_overlay: bool,
}

fn default_search_limit() -> u32 {
    32
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeSearchHit {
    pub path: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<CodeTextRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub provenance: CodeIndexProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeSearchResult {
    pub query: CodeSearchQuery,
    pub hits: Vec<CodeSearchHit>,
    pub index_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeGraphQueryKind {
    References,
    Dependencies,
    RelatedFiles,
    ChangeImpact,
    ProjectStructure,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeGraphQuery {
    pub workspace: CodeWorkspaceRef,
    pub kind: CodeGraphQueryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_symbol_id: Option<String>,
    #[serde(default = "default_graph_depth")]
    pub depth: u32,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
}

fn default_graph_depth() -> u32 {
    2
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeGraphNode {
    pub node_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeGraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeGraphResult {
    pub query: CodeGraphQuery,
    pub summary: String,
    pub nodes: Vec<CodeGraphNode>,
    pub edges: Vec<CodeGraphEdge>,
    pub index_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CodeFileChange {
    Create {
        path: String,
        content: String,
    },
    Change {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Rename {
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeIndexBatch {
    pub workspace: CodeWorkspaceRef,
    pub changes: Vec<CodeFileChange>,
    #[serde(default)]
    pub rebuild: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeIndexBatchResult {
    pub revision: CodeIndexRevision,
    pub files_updated: usize,
    pub files_skipped: usize,
    pub files_removed: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeOverlayBuffer {
    pub path: String,
    pub document_version: u64,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeIndexCheckpoint {
    pub checkpoint_id: String,
    pub workspace_id: String,
    pub revision: u64,
    pub content_hash: String,
    #[serde(default)]
    pub created_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeIndexLimits {
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_max_files")]
    pub max_files: u32,
    #[serde(default = "default_inline_hit_limit")]
    pub inline_hit_limit: u32,
}

fn default_max_file_bytes() -> u64 {
    1_048_576
}

fn default_max_files() -> u32 {
    50_000
}

fn default_inline_hit_limit() -> u32 {
    64
}

impl Default for CodeIndexLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: default_max_file_bytes(),
            max_files: default_max_files(),
            inline_hit_limit: default_inline_hit_limit(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeIndexIgnoreRules {
    #[serde(default)]
    pub directory_names: Vec<String>,
    #[serde(default)]
    pub glob_suffixes: Vec<String>,
}

impl CodeIndexIgnoreRules {
    pub fn default_rules() -> Self {
        Self {
            directory_names: vec![
                ".git".into(),
                "node_modules".into(),
                "target".into(),
                "dist".into(),
                "build".into(),
                ".next".into(),
                "vendor".into(),
            ],
            glob_suffixes: vec![
                ".png".into(),
                ".jpg".into(),
                ".jpeg".into(),
                ".gif".into(),
                ".webp".into(),
                ".ico".into(),
                ".bin".into(),
                ".exe".into(),
                ".dll".into(),
                ".so".into(),
                ".dylib".into(),
                ".wasm".into(),
                ".lock".into(),
            ],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CodeIndexServiceRequest {
    OpenWorkspace {
        workspace: CodeWorkspaceRef,
        #[serde(default)]
        ignore: Option<CodeIndexIgnoreRules>,
        #[serde(default)]
        limits: Option<CodeIndexLimits>,
        #[serde(default)]
        lsp_available: bool,
    },
    CloseWorkspace {
        workspace: CodeWorkspaceRef,
    },
    ApplyBatch {
        batch: CodeIndexBatch,
    },
    SetOverlay {
        workspace: CodeWorkspaceRef,
        buffer: CodeOverlayBuffer,
    },
    ClearOverlay {
        workspace: CodeWorkspaceRef,
        path: String,
    },
    Search {
        query: CodeSearchQuery,
    },
    Graph {
        query: CodeGraphQuery,
    },
    Status {
        workspace: CodeWorkspaceRef,
    },
    Checkpoint {
        workspace: CodeWorkspaceRef,
    },
    RestoreCheckpoint {
        workspace: CodeWorkspaceRef,
        checkpoint_id: String,
    },
    Rebuild {
        workspace: CodeWorkspaceRef,
    },
    ProjectSummary {
        workspace: CodeWorkspaceRef,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodeIndexServiceResponse {
    Ack,
    Batch(CodeIndexBatchResult),
    Search(CodeSearchResult),
    Graph(CodeGraphResult),
    Status(CodeIndexRevision),
    Checkpoint(CodeIndexCheckpoint),
    Summary {
        summary: String,
        revision: CodeIndexRevision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<ResourceRef>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodeIndexContextInput {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub extra: Value,
}
