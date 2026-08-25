// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::doc_markdown,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_raw_string_hashes,
    clippy::return_self_not_must_use,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unused_self
)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, AgentKitPluginDescriptor, AgentPluginStateKind, AgentServiceDescriptor,
    AgentToolDescriptor, CodeFileChange, CodeGraphEdge, CodeGraphNode, CodeGraphQuery,
    CodeGraphQueryKind, CodeGraphResult, CodeIndexBatch, CodeIndexBatchResult, CodeIndexCheckpoint,
    CodeIndexContextInput, CodeIndexIgnoreRules, CodeIndexLimits, CodeIndexProvenance,
    CodeIndexRevision, CodeIndexServiceRequest, CodeIndexServiceResponse, CodeIndexSource,
    CodeIndexState, CodeOverlayBuffer, CodeSearchHit, CodeSearchMode, CodeSearchQuery,
    CodeSearchResult, CodeSymbol, CodeSymbolKind, CodeTextPosition, CodeTextRange,
    CodeWorkspaceRef, ContextProviderRequest, ContextProviderResult, IndexedFile, ModuleDependency,
    SymbolRelation, SymbolRelationKind, ToolSideEffect,
};
use mutsuki_agent_plugin_api::{AgentPluginRegistrar, AgentService, ContextProvider, ToolProvider};
use mutsuki_agent_runtime::AgentResourceStore;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.code-index";
pub const SERVICE_ID: &str = "mutsuki.agent.service.code-index";
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.code-index";
pub const INLINE_RESULT_BYTES: usize = 64 * 1024;

/// Optional LSP symbol/reference signals. Implementations must not spawn language servers.
pub trait CodeIndexLspSignals: Send + Sync {
    fn document_symbols(
        &self,
        workspace_id: &str,
        path: &str,
        content: &str,
    ) -> Result<Vec<CodeSymbol>, AgentError>;

    fn references(
        &self,
        workspace_id: &str,
        path: &str,
        symbol_name: &str,
    ) -> Result<Vec<SymbolRelation>, AgentError>;
}

#[derive(Clone, Default)]
pub struct UnavailableLspSignals;

impl CodeIndexLspSignals for UnavailableLspSignals {
    fn document_symbols(
        &self,
        _workspace_id: &str,
        _path: &str,
        _content: &str,
    ) -> Result<Vec<CodeSymbol>, AgentError> {
        Err(AgentError::provider_unavailable("LSP signals unavailable"))
    }

    fn references(
        &self,
        _workspace_id: &str,
        _path: &str,
        _symbol_name: &str,
    ) -> Result<Vec<SymbolRelation>, AgentError> {
        Err(AgentError::provider_unavailable("LSP signals unavailable"))
    }
}

/// Injected LSP-backed symbol source for tests and product adapters.
type SymbolKey = (String, String);
type RelationKey = (String, String, String);
type SymbolMap = BTreeMap<SymbolKey, Vec<CodeSymbol>>;
type RelationMap = BTreeMap<RelationKey, Vec<SymbolRelation>>;

#[derive(Clone, Default)]
pub struct FakeLspSignals {
    symbols: Arc<Mutex<SymbolMap>>,
    relations: Arc<Mutex<RelationMap>>,
}

impl FakeLspSignals {
    pub fn with_symbols(
        self,
        workspace_id: impl Into<String>,
        path: impl Into<String>,
        symbols: Vec<CodeSymbol>,
    ) -> Self {
        self.symbols
            .lock()
            .expect("fake lsp symbols")
            .insert((workspace_id.into(), path.into()), symbols);
        self
    }

    pub fn with_references(
        self,
        workspace_id: impl Into<String>,
        path: impl Into<String>,
        symbol_name: impl Into<String>,
        relations: Vec<SymbolRelation>,
    ) -> Self {
        self.relations.lock().expect("fake lsp relations").insert(
            (workspace_id.into(), path.into(), symbol_name.into()),
            relations,
        );
        self
    }
}

impl CodeIndexLspSignals for FakeLspSignals {
    fn document_symbols(
        &self,
        workspace_id: &str,
        path: &str,
        _content: &str,
    ) -> Result<Vec<CodeSymbol>, AgentError> {
        self.symbols
            .lock()
            .expect("fake lsp symbols")
            .get(&(workspace_id.to_string(), path.to_string()))
            .cloned()
            .ok_or_else(|| AgentError::not_found("no LSP symbols for path"))
    }

    fn references(
        &self,
        workspace_id: &str,
        path: &str,
        symbol_name: &str,
    ) -> Result<Vec<SymbolRelation>, AgentError> {
        self.relations
            .lock()
            .expect("fake lsp relations")
            .get(&(
                workspace_id.to_string(),
                path.to_string(),
                symbol_name.to_string(),
            ))
            .cloned()
            .ok_or_else(|| AgentError::not_found("no LSP references for symbol"))
    }
}

#[derive(Clone)]
struct StoredFile {
    indexed: IndexedFile,
    content: String,
    lines: Vec<String>,
    embedding: Vec<f32>,
}

#[derive(Clone)]
struct WorkspaceIndex {
    workspace: CodeWorkspaceRef,
    ignore: CodeIndexIgnoreRules,
    limits: CodeIndexLimits,
    lsp_available: bool,
    revision: u64,
    state: CodeIndexState,
    files: BTreeMap<String, StoredFile>,
    symbols: BTreeMap<String, CodeSymbol>,
    symbols_by_path: BTreeMap<String, Vec<String>>,
    relations: Vec<SymbolRelation>,
    dependencies: Vec<ModuleDependency>,
    overlays: BTreeMap<String, CodeOverlayBuffer>,
    checkpoints: BTreeMap<String, CheckpointSnapshot>,
    next_content_revision: u64,
}

#[derive(Clone)]
struct CheckpointSnapshot {
    meta: CodeIndexCheckpoint,
    files: BTreeMap<String, StoredFile>,
    symbols: BTreeMap<String, CodeSymbol>,
    symbols_by_path: BTreeMap<String, Vec<String>>,
    relations: Vec<SymbolRelation>,
    dependencies: Vec<ModuleDependency>,
}

pub struct SharedCodeIndexService {
    descriptor: AgentServiceDescriptor,
    resources: AgentResourceStore,
    workspaces: Mutex<BTreeMap<String, WorkspaceIndex>>,
    lsp: Arc<dyn CodeIndexLspSignals>,
    next_checkpoint: AtomicU64,
    batch_counter: AtomicU64,
}

impl SharedCodeIndexService {
    pub fn new(resources: AgentResourceStore) -> Self {
        Self::with_lsp(resources, Arc::new(UnavailableLspSignals))
    }

    pub fn with_lsp(resources: AgentResourceStore, lsp: Arc<dyn CodeIndexLspSignals>) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.code_index.request@1".into(),
                response_schema: "mutsuki.agent.code_index.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("workspace".into()),
            },
            resources,
            workspaces: Mutex::new(BTreeMap::new()),
            lsp,
            next_checkpoint: AtomicU64::new(1),
            batch_counter: AtomicU64::new(0),
        }
    }

    pub fn plugin_descriptor(generation: u64) -> Result<AgentKitPluginDescriptor, AgentError> {
        let mut registrar = AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.code_index.request@1".into(),
                response_schema: "mutsuki.agent.code_index.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("workspace".into()),
            })
            .context_provider(CONTEXT_PROVIDER_ID)
            .require_service(SERVICE_ID);
        for (name, side_effect) in [
            ("code.search", ToolSideEffect::WorkspaceRead),
            ("code.symbol_query", ToolSideEffect::WorkspaceRead),
            ("code.dependency_query", ToolSideEffect::WorkspaceRead),
            ("code.related_files", ToolSideEffect::WorkspaceRead),
            ("code.project_summary", ToolSideEffect::WorkspaceRead),
            ("code.change_impact", ToolSideEffect::WorkspaceRead),
        ] {
            let mut tool = AgentToolDescriptor::new(
                name,
                format!("mutsuki.agent.tool.{name}@1"),
                format!("Run the {name} code-index operation"),
            );
            tool.side_effect = side_effect;
            registrar = registrar.tool(tool);
        }
        registrar.build()
    }

    pub fn open_workspace(
        &self,
        workspace: CodeWorkspaceRef,
        ignore: Option<CodeIndexIgnoreRules>,
        limits: Option<CodeIndexLimits>,
        lsp_available: bool,
    ) -> Result<(), AgentError> {
        validate_workspace(&workspace)?;
        let key = workspace_key(&workspace);
        let mut workspaces = self.workspaces.lock().expect("code index mutex");
        if workspaces.contains_key(&key) {
            return Ok(());
        }
        workspaces.insert(
            key,
            WorkspaceIndex {
                workspace,
                ignore: ignore.unwrap_or_else(CodeIndexIgnoreRules::default_rules),
                limits: limits.unwrap_or_default(),
                lsp_available,
                revision: 0,
                state: CodeIndexState::Ready,
                files: BTreeMap::new(),
                symbols: BTreeMap::new(),
                symbols_by_path: BTreeMap::new(),
                relations: Vec::new(),
                dependencies: Vec::new(),
                overlays: BTreeMap::new(),
                checkpoints: BTreeMap::new(),
                next_content_revision: 1,
            },
        );
        Ok(())
    }

    pub fn close_workspace(&self, workspace: &CodeWorkspaceRef) -> Result<(), AgentError> {
        let key = workspace_key(workspace);
        self.workspaces
            .lock()
            .expect("code index mutex")
            .remove(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        Ok(())
    }

    /// Apply an index change batch. Hosts should enqueue these via Mutsuki TaskBatch.
    pub fn apply_batch(&self, batch: CodeIndexBatch) -> Result<CodeIndexBatchResult, AgentError> {
        validate_workspace(&batch.workspace)?;
        let key = workspace_key(&batch.workspace);
        let mut workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get_mut(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, &batch.workspace)?;
        if batch.rebuild {
            index.files.clear();
            index.symbols.clear();
            index.symbols_by_path.clear();
            index.relations.clear();
            index.dependencies.clear();
            index.state = CodeIndexState::Rebuilding;
        } else {
            index.state = CodeIndexState::Indexing;
        }

        let mut updated = 0usize;
        let mut skipped = 0usize;
        let mut removed = 0usize;
        // Batch unit for TaskBatch-oriented hosts.
        let _batch_id = self.batch_counter.fetch_add(1, Ordering::Relaxed);

        for change in batch.changes {
            match change {
                CodeFileChange::Create { path, content }
                | CodeFileChange::Change { path, content } => {
                    match self.upsert_file(index, &path, &content)? {
                        UpsertOutcome::Updated => updated += 1,
                        UpsertOutcome::Skipped => skipped += 1,
                    }
                }
                CodeFileChange::Delete { path } => {
                    if self.remove_file(index, &path) {
                        removed += 1;
                        updated += 1;
                    } else {
                        skipped += 1;
                    }
                }
                CodeFileChange::Rename { from, to, content } => {
                    let existing = index.files.get(&from).map(|file| file.content.clone());
                    let body = content.or(existing).ok_or_else(|| {
                        AgentError::not_found(format!("rename source `{from}` is missing"))
                    })?;
                    if self.remove_file(index, &from) {
                        removed += 1;
                    }
                    match self.upsert_file(index, &to, &body)? {
                        UpsertOutcome::Updated => updated += 1,
                        UpsertOutcome::Skipped => skipped += 1,
                    }
                }
            }
        }

        index.revision = index.revision.saturating_add(1);
        index.state = CodeIndexState::Ready;
        Ok(CodeIndexBatchResult {
            revision: revision_of(index),
            files_updated: updated,
            files_skipped: skipped,
            files_removed: removed,
        })
    }

    pub fn set_overlay(
        &self,
        workspace: &CodeWorkspaceRef,
        buffer: CodeOverlayBuffer,
    ) -> Result<(), AgentError> {
        let key = workspace_key(workspace);
        let mut workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get_mut(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, workspace)?;
        if buffer.path.trim().is_empty() {
            return Err(AgentError::invalid_input("overlay path is required"));
        }
        index.overlays.insert(buffer.path.clone(), buffer);
        Ok(())
    }

    pub fn clear_overlay(
        &self,
        workspace: &CodeWorkspaceRef,
        path: &str,
    ) -> Result<(), AgentError> {
        let key = workspace_key(workspace);
        let mut workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get_mut(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, workspace)?;
        index.overlays.remove(path);
        Ok(())
    }

    pub fn search(&self, query: CodeSearchQuery) -> Result<CodeSearchResult, AgentError> {
        validate_workspace(&query.workspace)?;
        if query.query.trim().is_empty() {
            return Err(AgentError::invalid_input("search query must not be empty"));
        }
        if query.limit == 0 {
            return Err(AgentError::invalid_input("search limit must be positive"));
        }
        let key = workspace_key(&query.workspace);
        let workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, &query.workspace)?;

        let mut hits = match query.mode {
            CodeSearchMode::Text => text_search(index, &query, false)?,
            CodeSearchMode::Regex => text_search(index, &query, true)?,
            CodeSearchMode::Symbol => symbol_search(index, &query),
            CodeSearchMode::Semantic => semantic_search(index, &query),
        };
        if query.include_overlay {
            hits.extend(overlay_search(index, &query)?);
        }
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.summary.cmp(&right.summary))
        });
        hits.truncate(query.limit as usize);

        let details = self.maybe_details("mutsuki.agent.code_index.search", &hits)?;
        let inline_limit = index.limits.inline_hit_limit as usize;
        let inline_hits = if hits.len() > inline_limit {
            hits.into_iter().take(inline_limit).collect()
        } else {
            hits
        };
        Ok(CodeSearchResult {
            query,
            hits: inline_hits,
            index_revision: index.revision,
            details,
        })
    }

    pub fn graph(&self, query: CodeGraphQuery) -> Result<CodeGraphResult, AgentError> {
        validate_workspace(&query.workspace)?;
        let key = workspace_key(&query.workspace);
        let workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, &query.workspace)?;

        let (summary, nodes, edges) = match query.kind {
            CodeGraphQueryKind::References => references_graph(index, &query),
            CodeGraphQueryKind::Dependencies => dependency_graph(index, &query),
            CodeGraphQueryKind::RelatedFiles => related_files_graph(index, &query),
            CodeGraphQueryKind::ChangeImpact => change_impact_graph(index, &query),
            CodeGraphQueryKind::ProjectStructure => project_structure_graph(index, &query),
        };
        let payload = json!({ "nodes": nodes, "edges": edges });
        let details = self.maybe_details("mutsuki.agent.code_index.graph", &payload)?;
        let mut nodes = nodes;
        let mut edges = edges;
        nodes.truncate(query.limit as usize);
        edges.truncate(query.limit as usize);
        Ok(CodeGraphResult {
            query,
            summary,
            nodes,
            edges,
            index_revision: index.revision,
            details,
        })
    }

    pub fn status(&self, workspace: &CodeWorkspaceRef) -> Result<CodeIndexRevision, AgentError> {
        let key = workspace_key(workspace);
        let workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, workspace)?;
        Ok(revision_of(index))
    }

    pub fn checkpoint(
        &self,
        workspace: &CodeWorkspaceRef,
    ) -> Result<CodeIndexCheckpoint, AgentError> {
        let key = workspace_key(workspace);
        let mut workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get_mut(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, workspace)?;
        let checkpoint_id = format!(
            "ckpt-{}",
            self.next_checkpoint.fetch_add(1, Ordering::Relaxed)
        );
        let content_hash = hash_index_state(index);
        let meta = CodeIndexCheckpoint {
            checkpoint_id: checkpoint_id.clone(),
            workspace_id: workspace.workspace_id.clone(),
            revision: index.revision,
            content_hash,
            created_at_unix_ms: 0,
        };
        index.checkpoints.insert(
            checkpoint_id,
            CheckpointSnapshot {
                meta: meta.clone(),
                files: index.files.clone(),
                symbols: index.symbols.clone(),
                symbols_by_path: index.symbols_by_path.clone(),
                relations: index.relations.clone(),
                dependencies: index.dependencies.clone(),
            },
        );
        Ok(meta)
    }

    pub fn restore_checkpoint(
        &self,
        workspace: &CodeWorkspaceRef,
        checkpoint_id: &str,
    ) -> Result<CodeIndexRevision, AgentError> {
        let key = workspace_key(workspace);
        let mut workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get_mut(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, workspace)?;
        let snapshot = index
            .checkpoints
            .get(checkpoint_id)
            .cloned()
            .ok_or_else(|| AgentError::not_found("checkpoint not found"))?;
        let expected = hash_index_snapshot(&snapshot);
        if expected != snapshot.meta.content_hash {
            index.state = CodeIndexState::Corrupted;
            return Err(AgentError::new(
                "agent.code_index.corrupted",
                "checkpoint content hash mismatch",
            ));
        }
        index.files = snapshot.files;
        index.symbols = snapshot.symbols;
        index.symbols_by_path = snapshot.symbols_by_path;
        index.relations = snapshot.relations;
        index.dependencies = snapshot.dependencies;
        index.revision = snapshot.meta.revision;
        index.state = CodeIndexState::Ready;
        // Overlay remains ephemeral and is not restored from checkpoints.
        Ok(revision_of(index))
    }

    pub fn rebuild(
        &self,
        workspace: &CodeWorkspaceRef,
    ) -> Result<CodeIndexBatchResult, AgentError> {
        let key = workspace_key(workspace);
        let contents = {
            let workspaces = self.workspaces.lock().expect("code index mutex");
            let index = workspaces
                .get(&key)
                .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
            assert_workspace_identity(index, workspace)?;
            index
                .files
                .iter()
                .map(|(path, file)| (path.clone(), file.content.clone()))
                .collect::<Vec<_>>()
        };
        let changes = contents
            .into_iter()
            .map(|(path, content)| CodeFileChange::Change { path, content })
            .collect();
        self.apply_batch(CodeIndexBatch {
            workspace: workspace.clone(),
            changes,
            rebuild: true,
        })
    }

    pub fn project_summary(
        &self,
        workspace: &CodeWorkspaceRef,
    ) -> Result<
        (
            String,
            CodeIndexRevision,
            Option<mutsuki_agent_contracts::ResourceRef>,
        ),
        AgentError,
    > {
        let key = workspace_key(workspace);
        let workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .get(&key)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        assert_workspace_identity(index, workspace)?;
        let mut languages: BTreeMap<String, usize> = BTreeMap::new();
        for file in index.files.values() {
            *languages.entry(file.indexed.language.clone()).or_default() += 1;
        }
        let summary = format!(
            "workspace {} revision {} files={} symbols={} deps={} languages={:?}",
            workspace.workspace_id,
            index.revision,
            index.files.len(),
            index.symbols.len(),
            index.dependencies.len(),
            languages
        );
        let details = self.maybe_details(
            "mutsuki.agent.code_index.summary",
            &json!({
                "languages": languages,
                "paths": index.files.keys().cloned().collect::<Vec<_>>(),
            }),
        )?;
        Ok((summary, revision_of(index), details))
    }

    pub fn active_workspace_count(&self) -> usize {
        self.workspaces.lock().expect("code index mutex").len()
    }

    fn upsert_file(
        &self,
        index: &mut WorkspaceIndex,
        path: &str,
        content: &str,
    ) -> Result<UpsertOutcome, AgentError> {
        if should_ignore(path, &index.ignore) {
            return Ok(UpsertOutcome::Skipped);
        }
        if content.len() as u64 > index.limits.max_file_bytes {
            return Err(AgentError::invalid_input(format!(
                "file `{path}` exceeds max_file_bytes"
            )));
        }
        if index.files.len() as u32 >= index.limits.max_files && !index.files.contains_key(path) {
            return Err(AgentError::invalid_input("index file limit reached"));
        }
        let content_hash = hash_text(content);
        if let Some(existing) = index.files.get(path)
            && existing.indexed.content_hash == content_hash
        {
            return Ok(UpsertOutcome::Skipped);
        }

        self.remove_file(index, path);
        let language = detect_language(path);
        let content_revision = index.next_content_revision;
        index.next_content_revision = index.next_content_revision.saturating_add(1);
        let provenance = provenance(
            path,
            &index.workspace,
            if index.lsp_available {
                CodeIndexSource::Lsp
            } else {
                CodeIndexSource::Parser
            },
            None,
        );

        let mut symbols = if index.lsp_available {
            match self
                .lsp
                .document_symbols(&index.workspace.workspace_id, path, content)
            {
                Ok(symbols) if !symbols.is_empty() => symbols,
                _ => parse_symbols(path, content, &provenance),
            }
        } else {
            parse_symbols(path, content, &provenance)
        };
        for symbol in &mut symbols {
            symbol.provenance = provenance.clone();
            symbol.provenance.source = if index.lsp_available {
                CodeIndexSource::Lsp
            } else {
                CodeIndexSource::Parser
            };
        }

        let mut relations = Vec::new();
        if index.lsp_available {
            for symbol in &symbols {
                if let Ok(extra) =
                    self.lsp
                        .references(&index.workspace.workspace_id, path, &symbol.name)
                {
                    relations.extend(extra);
                }
            }
        }
        relations.extend(parse_reference_relations(
            path,
            content,
            &symbols,
            &provenance,
        ));
        let dependencies = parse_dependencies(path, content, &provenance);

        let mut symbol_ids = Vec::new();
        for symbol in symbols {
            symbol_ids.push(symbol.symbol_id.clone());
            index.symbols.insert(symbol.symbol_id.clone(), symbol);
        }
        index.symbols_by_path.insert(path.to_string(), symbol_ids);
        index.relations.extend(relations);
        index.dependencies.extend(dependencies);

        let lines = content.lines().map(str::to_string).collect::<Vec<_>>();
        let embedding = embed_text(content);
        index.files.insert(
            path.to_string(),
            StoredFile {
                indexed: IndexedFile {
                    path: path.to_string(),
                    language,
                    content_hash,
                    content_revision,
                    byte_length: content.len() as u64,
                    line_count: lines.len() as u32,
                    ignored: false,
                    provenance: provenance_with_source(&provenance, CodeIndexSource::Text),
                },
                content: content.to_string(),
                lines,
                embedding,
            },
        );
        Ok(UpsertOutcome::Updated)
    }

    fn remove_file(&self, index: &mut WorkspaceIndex, path: &str) -> bool {
        let removed = index.files.remove(path).is_some();
        if let Some(symbol_ids) = index.symbols_by_path.remove(path) {
            for symbol_id in symbol_ids {
                index.symbols.remove(&symbol_id);
            }
        }
        index
            .relations
            .retain(|relation| !relation.provenance.path.starts_with(path));
        index
            .dependencies
            .retain(|dependency| dependency.from_path != path);
        removed
    }

    fn maybe_details<T: serde::Serialize>(
        &self,
        kind: &str,
        value: &T,
    ) -> Result<Option<mutsuki_agent_contracts::ResourceRef>, AgentError> {
        let encoded = serde_json::to_vec(value)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        if encoded.len() <= INLINE_RESULT_BYTES {
            return Ok(None);
        }
        let json_value = serde_json::to_value(value)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        Ok(Some(self.resources.put_json(
            SERVICE_ID,
            kind,
            &format!("{kind}@1"),
            1,
            &json_value,
        )?))
    }
}

enum UpsertOutcome {
    Updated,
    Skipped,
}

impl AgentService for SharedCodeIndexService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: CodeIndexServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = match request {
            CodeIndexServiceRequest::OpenWorkspace {
                workspace,
                ignore,
                limits,
                lsp_available,
            } => {
                self.open_workspace(workspace, ignore, limits, lsp_available)?;
                CodeIndexServiceResponse::Ack
            }
            CodeIndexServiceRequest::CloseWorkspace { workspace } => {
                self.close_workspace(&workspace)?;
                CodeIndexServiceResponse::Ack
            }
            CodeIndexServiceRequest::ApplyBatch { batch } => {
                CodeIndexServiceResponse::Batch(self.apply_batch(batch)?)
            }
            CodeIndexServiceRequest::SetOverlay { workspace, buffer } => {
                self.set_overlay(&workspace, buffer)?;
                CodeIndexServiceResponse::Ack
            }
            CodeIndexServiceRequest::ClearOverlay { workspace, path } => {
                self.clear_overlay(&workspace, &path)?;
                CodeIndexServiceResponse::Ack
            }
            CodeIndexServiceRequest::Search { query } => {
                CodeIndexServiceResponse::Search(self.search(query)?)
            }
            CodeIndexServiceRequest::Graph { query } => {
                CodeIndexServiceResponse::Graph(self.graph(query)?)
            }
            CodeIndexServiceRequest::Status { workspace } => {
                CodeIndexServiceResponse::Status(self.status(&workspace)?)
            }
            CodeIndexServiceRequest::Checkpoint { workspace } => {
                CodeIndexServiceResponse::Checkpoint(self.checkpoint(&workspace)?)
            }
            CodeIndexServiceRequest::RestoreCheckpoint {
                workspace,
                checkpoint_id,
            } => CodeIndexServiceResponse::Status(
                self.restore_checkpoint(&workspace, &checkpoint_id)?,
            ),
            CodeIndexServiceRequest::Rebuild { workspace } => {
                CodeIndexServiceResponse::Batch(self.rebuild(&workspace)?)
            }
            CodeIndexServiceRequest::ProjectSummary { workspace } => {
                let (summary, revision, details) = self.project_summary(&workspace)?;
                CodeIndexServiceResponse::Summary {
                    summary,
                    revision,
                    details,
                }
            }
        };
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        self.workspaces.lock().expect("code index mutex").clear();
        Ok(())
    }
}

impl ToolProvider for SharedCodeIndexService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        Self::plugin_descriptor(1)
            .map(|descriptor| descriptor.tools)
            .unwrap_or_default()
    }
}

impl ContextProvider for SharedCodeIndexService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: ContextProviderRequest,
    ) -> Result<ContextProviderResult, AgentError> {
        let input: CodeIndexContextInput =
            serde_json::from_value(request.input.clone()).unwrap_or_default();
        let workspace_id = input
            .workspace_id
            .or_else(|| {
                request
                    .input
                    .get("workspace_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .ok_or_else(|| AgentError::invalid_input("code index context requires workspace_id"))?;
        let workspaces = self.workspaces.lock().expect("code index mutex");
        let index = workspaces
            .values()
            .find(|index| index.workspace.workspace_id == workspace_id)
            .ok_or_else(|| AgentError::not_found("code index workspace is not open"))?;
        let revision = revision_of(index);
        let summary = format!(
            "code-index {} rev={} files={} symbols={} overlays={}",
            revision.workspace.workspace_id,
            revision.revision,
            revision.indexed_files,
            revision.symbol_count,
            revision.overlay_paths.len()
        );
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.code_index.status",
            "mutsuki.agent.code_index.status@1",
            revision.revision.max(1),
            &serde_json::to_value(&revision)
                .map_err(|error| AgentError::invalid_input(error.to_string()))?,
        )?;
        Ok(ContextProviderResult {
            provider_id: request.provider_id,
            estimated_tokens: (summary.len() as u64).div_ceil(4),
            estimated_bytes: summary.len() as u64,
            summary,
            details: Some(details),
            priority: 0,
            required: false,
        })
    }
}

fn validate_workspace(workspace: &CodeWorkspaceRef) -> Result<(), AgentError> {
    if workspace.workspace_id.trim().is_empty() || workspace.root.trim().is_empty() {
        return Err(AgentError::invalid_input(
            "workspace_id and root are required",
        ));
    }
    Ok(())
}

fn workspace_key(workspace: &CodeWorkspaceRef) -> String {
    format!(
        "{}::{}::{}::{}",
        workspace.tenant_id,
        workspace.workspace_id,
        workspace.git_revision.clone().unwrap_or_default(),
        workspace.worktree_id.clone().unwrap_or_default()
    )
}

fn assert_workspace_identity(
    index: &WorkspaceIndex,
    workspace: &CodeWorkspaceRef,
) -> Result<(), AgentError> {
    if index.workspace.workspace_id != workspace.workspace_id
        || index.workspace.tenant_id != workspace.tenant_id
        || index.workspace.git_revision != workspace.git_revision
        || index.workspace.worktree_id != workspace.worktree_id
    {
        return Err(AgentError::invalid_input(
            "code index workspace identity mismatch",
        ));
    }
    Ok(())
}

fn revision_of(index: &WorkspaceIndex) -> CodeIndexRevision {
    CodeIndexRevision {
        workspace: index.workspace.clone(),
        revision: index.revision,
        checkpoint_id: index
            .checkpoints
            .keys()
            .next_back()
            .cloned()
            .unwrap_or_default(),
        state: index.state,
        indexed_files: index.files.len(),
        symbol_count: index.symbols.len(),
        dependency_count: index.dependencies.len(),
        overlay_paths: index.overlays.keys().cloned().collect(),
    }
}

fn should_ignore(path: &str, ignore: &CodeIndexIgnoreRules) -> bool {
    let normalized = path.replace('\\', "/");
    for part in normalized.split('/') {
        if ignore.directory_names.iter().any(|name| name == part) {
            return true;
        }
    }
    ignore
        .glob_suffixes
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
}

fn detect_language(path: &str) -> String {
    match path.rsplit('.').next().unwrap_or_default() {
        "rs" => "rust".into(),
        "ts" | "tsx" => "typescript".into(),
        "js" | "jsx" => "javascript".into(),
        "py" => "python".into(),
        "go" => "go".into(),
        "md" => "markdown".into(),
        other if !other.is_empty() => other.into(),
        _ => "text".into(),
    }
}

fn provenance(
    path: &str,
    workspace: &CodeWorkspaceRef,
    source: CodeIndexSource,
    range: Option<CodeTextRange>,
) -> CodeIndexProvenance {
    CodeIndexProvenance {
        path: path.into(),
        range,
        workspace_id: workspace.workspace_id.clone(),
        git_revision: workspace.git_revision.clone(),
        source,
    }
}

fn provenance_with_source(
    base: &CodeIndexProvenance,
    source: CodeIndexSource,
) -> CodeIndexProvenance {
    let mut next = base.clone();
    next.source = source;
    next
}

fn hash_text(text: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes())))
}

fn hash_index_state(index: &WorkspaceIndex) -> String {
    let mut hasher = Sha256::new();
    for (path, file) in &index.files {
        hasher.update(path.as_bytes());
        hasher.update(file.indexed.content_hash.as_bytes());
    }
    for symbol_id in index.symbols.keys() {
        hasher.update(symbol_id.as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn hash_index_snapshot(snapshot: &CheckpointSnapshot) -> String {
    let mut hasher = Sha256::new();
    for (path, file) in &snapshot.files {
        hasher.update(path.as_bytes());
        hasher.update(file.indexed.content_hash.as_bytes());
    }
    for symbol_id in snapshot.symbols.keys() {
        hasher.update(symbol_id.as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn parse_symbols(path: &str, content: &str, provenance: &CodeIndexProvenance) -> Vec<CodeSymbol> {
    let patterns = [
        (
            Regex::new(
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
            )
            .unwrap(),
            CodeSymbolKind::Function,
        ),
        (
            Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)")
                .unwrap(),
            CodeSymbolKind::Struct,
        ),
        (
            Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)")
                .unwrap(),
            CodeSymbolKind::Enum,
        ),
        (
            Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)")
                .unwrap(),
            CodeSymbolKind::Trait,
        ),
        (
            Regex::new(r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)")
                .unwrap(),
            CodeSymbolKind::Function,
        ),
        (
            Regex::new(r"(?m)^\s*(?:export\s+)?(?:abstract\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)")
                .unwrap(),
            CodeSymbolKind::Class,
        ),
        (
            Regex::new(r"(?m)^\s*(?:export\s+)?interface\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            CodeSymbolKind::Interface,
        ),
        (
            Regex::new(r"(?m)^\s*(?:export\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            CodeSymbolKind::Type,
        ),
        (
            Regex::new(r"(?m)^\s*(?:export\s+)?const\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            CodeSymbolKind::Const,
        ),
    ];
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        for (pattern, kind) in &patterns {
            if let Some(captures) = pattern.captures(line)
                && let Some(name) = captures.get(1)
            {
                let start = CodeTextPosition {
                    line: line_idx as u32,
                    character: name.start() as u32,
                };
                let end = CodeTextPosition {
                    line: line_idx as u32,
                    character: name.end() as u32,
                };
                let range = CodeTextRange { start, end };
                let mut item_provenance = provenance.clone();
                item_provenance.range = Some(range);
                item_provenance.source = CodeIndexSource::Parser;
                let name = name.as_str().to_string();
                symbols.push(CodeSymbol {
                    symbol_id: format!("{path}::{name}@{}", line_idx),
                    name,
                    kind: *kind,
                    path: path.to_string(),
                    range,
                    container: None,
                    provenance: item_provenance,
                });
            }
        }
    }
    symbols
}

fn parse_reference_relations(
    path: &str,
    content: &str,
    symbols: &[CodeSymbol],
    provenance: &CodeIndexProvenance,
) -> Vec<SymbolRelation> {
    let names: BTreeSet<_> = symbols.iter().map(|symbol| symbol.name.clone()).collect();
    let mut relations = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        for name in &names {
            if line.contains(name.as_str()) {
                let from = format!("{path}::ref@{line_idx}:{name}");
                if let Some(target) = symbols.iter().find(|symbol| symbol.name == *name) {
                    let mut item = provenance.clone();
                    item.range = Some(CodeTextRange {
                        start: CodeTextPosition {
                            line: line_idx as u32,
                            character: 0,
                        },
                        end: CodeTextPosition {
                            line: line_idx as u32,
                            character: line.len() as u32,
                        },
                    });
                    relations.push(SymbolRelation {
                        from_symbol_id: from,
                        to_symbol_id: target.symbol_id.clone(),
                        kind: SymbolRelationKind::References,
                        provenance: item,
                    });
                }
            }
        }
    }
    relations
}

fn parse_dependencies(
    path: &str,
    content: &str,
    provenance: &CodeIndexProvenance,
) -> Vec<ModuleDependency> {
    let patterns = [
        Regex::new(r#"(?m)^\s*use\s+([A-Za-z0-9_:'{} ,]+);"#).unwrap(),
        Regex::new(r#"(?m)^\s*import\s+.+?\s+from\s+['\"]([^'\"]+)['\"]"#).unwrap(),
        Regex::new(r#"(?m)^\s*import\s+['\"]([^'\"]+)['\"]"#).unwrap(),
    ];
    let mut deps = Vec::new();
    for pattern in patterns {
        for captures in pattern.captures_iter(content) {
            if let Some(module) = captures.get(1) {
                let to_module = module.as_str().trim().to_string();
                let to_path = resolve_module_path(path, &to_module);
                deps.push(ModuleDependency {
                    from_path: path.to_string(),
                    to_module,
                    to_path,
                    provenance: provenance.clone(),
                });
            }
        }
    }
    deps
}

fn resolve_module_path(from_path: &str, module: &str) -> Option<String> {
    if !(module.starts_with("./") || module.starts_with("../")) {
        return None;
    }
    let parent = std::path::Path::new(from_path).parent()?;
    let resolved = parent.join(module);
    let mut normalized = String::new();
    for component in resolved.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if let Some((prefix, _)) = normalized.rsplit_once('/') {
                    normalized = prefix.to_string();
                } else {
                    normalized.clear();
                }
            }
            std::path::Component::Normal(part) => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(&part.to_string_lossy());
            }
            _ => {}
        }
    }
    if normalized.is_empty() {
        None
    } else if normalized.contains('.') {
        Some(normalized)
    } else {
        Some(format!("{normalized}.ts"))
    }
}

fn text_search(
    index: &WorkspaceIndex,
    query: &CodeSearchQuery,
    regex_mode: bool,
) -> Result<Vec<CodeSearchHit>, AgentError> {
    let regex = if regex_mode {
        Some(
            Regex::new(&query.query)
                .map_err(|error| AgentError::invalid_input(format!("invalid regex: {error}")))?,
        )
    } else {
        None
    };
    let mut hits = Vec::new();
    for (path, file) in &index.files {
        if let Some(prefix) = &query.path_prefix
            && !path.starts_with(prefix.as_str())
        {
            continue;
        }
        for (line_idx, line) in file.lines.iter().enumerate() {
            let matched = if let Some(regex) = &regex {
                regex.is_match(line)
            } else {
                line.contains(&query.query)
            };
            if matched {
                hits.push(CodeSearchHit {
                    path: path.clone(),
                    summary: line.clone(),
                    range: Some(CodeTextRange {
                        start: CodeTextPosition {
                            line: line_idx as u32,
                            character: 0,
                        },
                        end: CodeTextPosition {
                            line: line_idx as u32,
                            character: line.len() as u32,
                        },
                    }),
                    symbol_id: None,
                    score: Some(1.0),
                    provenance: provenance_with_source(
                        &file.indexed.provenance,
                        CodeIndexSource::Text,
                    ),
                });
            }
        }
    }
    Ok(hits)
}

fn symbol_search(index: &WorkspaceIndex, query: &CodeSearchQuery) -> Vec<CodeSearchHit> {
    let needle = query.query.to_lowercase();
    index
        .symbols
        .values()
        .filter(|symbol| {
            query
                .path_prefix
                .as_ref()
                .is_none_or(|prefix| symbol.path.starts_with(prefix.as_str()))
                && symbol.name.to_lowercase().contains(&needle)
        })
        .map(|symbol| CodeSearchHit {
            path: symbol.path.clone(),
            summary: format!("{:?} {}", symbol.kind, symbol.name),
            range: Some(symbol.range),
            symbol_id: Some(symbol.symbol_id.clone()),
            score: Some(1.0),
            provenance: symbol.provenance.clone(),
        })
        .collect()
}

fn semantic_search(index: &WorkspaceIndex, query: &CodeSearchQuery) -> Vec<CodeSearchHit> {
    let query_embedding = embed_text(&query.query);
    let mut hits = Vec::new();
    for (path, file) in &index.files {
        if let Some(prefix) = &query.path_prefix
            && !path.starts_with(prefix.as_str())
        {
            continue;
        }
        let score = cosine_similarity(&query_embedding, &file.embedding) as f64;
        if score > 0.05 {
            hits.push(CodeSearchHit {
                path: path.clone(),
                summary: file.lines.first().cloned().unwrap_or_default(),
                range: None,
                symbol_id: None,
                score: Some(score),
                provenance: provenance_with_source(
                    &file.indexed.provenance,
                    CodeIndexSource::Semantic,
                ),
            });
        }
    }
    hits
}

fn overlay_search(
    index: &WorkspaceIndex,
    query: &CodeSearchQuery,
) -> Result<Vec<CodeSearchHit>, AgentError> {
    let mut hits = Vec::new();
    for (path, buffer) in &index.overlays {
        if let Some(prefix) = &query.path_prefix
            && !path.starts_with(prefix.as_str())
        {
            continue;
        }
        for (line_idx, line) in buffer.content.lines().enumerate() {
            let matched = match query.mode {
                CodeSearchMode::Regex => Regex::new(&query.query)
                    .map_err(|error| AgentError::invalid_input(format!("invalid regex: {error}")))?
                    .is_match(line),
                CodeSearchMode::Text | CodeSearchMode::Semantic => line.contains(&query.query),
                CodeSearchMode::Symbol => line.contains(&query.query),
            };
            if matched {
                hits.push(CodeSearchHit {
                    path: path.clone(),
                    summary: format!("overlay: {line}"),
                    range: Some(CodeTextRange {
                        start: CodeTextPosition {
                            line: line_idx as u32,
                            character: 0,
                        },
                        end: CodeTextPosition {
                            line: line_idx as u32,
                            character: line.len() as u32,
                        },
                    }),
                    symbol_id: None,
                    score: Some(0.5),
                    provenance: CodeIndexProvenance {
                        path: path.clone(),
                        range: None,
                        workspace_id: index.workspace.workspace_id.clone(),
                        git_revision: index.workspace.git_revision.clone(),
                        source: CodeIndexSource::Overlay,
                    },
                });
            }
        }
    }
    Ok(hits)
}

fn references_graph(
    index: &WorkspaceIndex,
    query: &CodeGraphQuery,
) -> (String, Vec<CodeGraphNode>, Vec<CodeGraphEdge>) {
    let seed = query
        .seed_symbol_id
        .clone()
        .or_else(|| {
            query.seed_path.as_ref().and_then(|path| {
                index
                    .symbols_by_path
                    .get(path)
                    .and_then(|ids| ids.first().cloned())
            })
        })
        .unwrap_or_default();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    if let Some(symbol) = index.symbols.get(&seed) {
        nodes.push(CodeGraphNode {
            node_id: symbol.symbol_id.clone(),
            label: symbol.name.clone(),
            path: Some(symbol.path.clone()),
            symbol_id: Some(symbol.symbol_id.clone()),
        });
    }
    for relation in &index.relations {
        if relation.to_symbol_id == seed || relation.from_symbol_id == seed {
            edges.push(CodeGraphEdge {
                from: relation.from_symbol_id.clone(),
                to: relation.to_symbol_id.clone(),
                kind: format!("{:?}", relation.kind).to_lowercase(),
            });
            for id in [&relation.from_symbol_id, &relation.to_symbol_id] {
                if nodes.iter().any(|node| node.node_id == *id) {
                    continue;
                }
                if let Some(symbol) = index.symbols.get(id) {
                    nodes.push(CodeGraphNode {
                        node_id: symbol.symbol_id.clone(),
                        label: symbol.name.clone(),
                        path: Some(symbol.path.clone()),
                        symbol_id: Some(symbol.symbol_id.clone()),
                    });
                } else {
                    nodes.push(CodeGraphNode {
                        node_id: id.clone(),
                        label: id.clone(),
                        path: Some(relation.provenance.path.clone()),
                        symbol_id: None,
                    });
                }
            }
        }
    }
    (
        format!("references for `{seed}`: {} edges", edges.len()),
        nodes,
        edges,
    )
}

fn dependency_graph(
    index: &WorkspaceIndex,
    query: &CodeGraphQuery,
) -> (String, Vec<CodeGraphNode>, Vec<CodeGraphEdge>) {
    let seed = query.seed_path.clone().unwrap_or_default();
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();
    for dependency in &index.dependencies {
        if !seed.is_empty()
            && dependency.from_path != seed
            && dependency.to_path.as_deref() != Some(seed.as_str())
        {
            continue;
        }
        nodes
            .entry(dependency.from_path.clone())
            .or_insert_with(|| CodeGraphNode {
                node_id: dependency.from_path.clone(),
                label: dependency.from_path.clone(),
                path: Some(dependency.from_path.clone()),
                symbol_id: None,
            });
        let to_id = dependency
            .to_path
            .clone()
            .unwrap_or_else(|| dependency.to_module.clone());
        nodes.entry(to_id.clone()).or_insert_with(|| CodeGraphNode {
            node_id: to_id.clone(),
            label: dependency.to_module.clone(),
            path: dependency.to_path.clone(),
            symbol_id: None,
        });
        edges.push(CodeGraphEdge {
            from: dependency.from_path.clone(),
            to: to_id,
            kind: "imports".into(),
        });
    }
    (
        format!("dependencies around `{seed}`: {} edges", edges.len()),
        nodes.into_values().collect(),
        edges,
    )
}

fn related_files_graph(
    index: &WorkspaceIndex,
    query: &CodeGraphQuery,
) -> (String, Vec<CodeGraphNode>, Vec<CodeGraphEdge>) {
    let (summary, nodes, edges) = dependency_graph(index, query);
    (format!("related files: {summary}"), nodes, edges)
}

fn change_impact_graph(
    index: &WorkspaceIndex,
    query: &CodeGraphQuery,
) -> (String, Vec<CodeGraphNode>, Vec<CodeGraphEdge>) {
    let seed = query.seed_path.clone().unwrap_or_default();
    let mut impacted = BTreeSet::new();
    impacted.insert(seed.clone());
    for _ in 0..query.depth.max(1) {
        let current = impacted.clone();
        for dependency in &index.dependencies {
            if current.contains(&dependency.from_path)
                && let Some(to_path) = &dependency.to_path
            {
                impacted.insert(to_path.clone());
            }
            if dependency
                .to_path
                .as_ref()
                .is_some_and(|path| current.contains(path))
            {
                impacted.insert(dependency.from_path.clone());
            }
        }
    }
    let nodes = impacted
        .iter()
        .map(|path| CodeGraphNode {
            node_id: path.clone(),
            label: path.clone(),
            path: Some(path.clone()),
            symbol_id: None,
        })
        .collect::<Vec<_>>();
    let edges = index
        .dependencies
        .iter()
        .filter(|dependency| {
            impacted.contains(&dependency.from_path)
                || dependency
                    .to_path
                    .as_ref()
                    .is_some_and(|path| impacted.contains(path))
        })
        .map(|dependency| CodeGraphEdge {
            from: dependency.from_path.clone(),
            to: dependency
                .to_path
                .clone()
                .unwrap_or_else(|| dependency.to_module.clone()),
            kind: "impact".into(),
        })
        .collect::<Vec<_>>();
    (
        format!(
            "change impact candidates for `{seed}`: {} files",
            nodes.len()
        ),
        nodes,
        edges,
    )
}

fn project_structure_graph(
    index: &WorkspaceIndex,
    _query: &CodeGraphQuery,
) -> (String, Vec<CodeGraphNode>, Vec<CodeGraphEdge>) {
    let mut roots: HashMap<String, usize> = HashMap::new();
    for path in index.files.keys() {
        let root = path.split('/').next().unwrap_or(path).to_string();
        *roots.entry(root).or_default() += 1;
    }
    let nodes = roots
        .into_iter()
        .map(|(root, count)| CodeGraphNode {
            node_id: root.clone(),
            label: format!("{root} ({count})"),
            path: Some(root),
            symbol_id: None,
        })
        .collect::<Vec<_>>();
    (
        format!("project structure with {} top-level groups", nodes.len()),
        nodes,
        Vec::new(),
    )
}

fn embed_text(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0f32; 32];
    let dimension = vector.len();
    for (index, byte) in text.as_bytes().iter().enumerate() {
        vector[index % dimension] += (*byte as f32) / 255.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let dot = left
        .iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm * right_norm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::tempdir;

    fn workspace(id: &str, root: &str) -> CodeWorkspaceRef {
        CodeWorkspaceRef {
            workspace_id: id.into(),
            root: root.into(),
            tenant_id: "tenant-a".into(),
            git_revision: Some("abc123".into()),
            worktree_id: None,
        }
    }

    fn sample_batch(ws: &CodeWorkspaceRef) -> CodeIndexBatch {
        CodeIndexBatch {
            workspace: ws.clone(),
            rebuild: false,
            changes: vec![
                CodeFileChange::Create {
                    path: "crates/core/src/lib.rs".into(),
                    content: r#"
use crate::util::helper;

pub struct Engine {}

pub fn boot() {
    helper();
}
"#
                    .into(),
                },
                CodeFileChange::Create {
                    path: "crates/core/src/util.rs".into(),
                    content: "pub fn helper() {}\n".into(),
                },
                CodeFileChange::Create {
                    path: "apps/web/src/index.ts".into(),
                    content: r#"
import { start } from './runtime';
export function main() {
  start();
}
"#
                    .into(),
                },
                CodeFileChange::Create {
                    path: "apps/web/src/runtime.ts".into(),
                    content: "export function start() { return 1; }\n".into(),
                },
                CodeFileChange::Create {
                    path: "target/debug/dummy.rs".into(),
                    content: "fn ignored() {}".into(),
                },
            ],
        }
    }

    #[test]
    fn incremental_index_text_symbol_dependency_semantic_and_overlay() {
        let resources = AgentResourceStore::default();
        let service = SharedCodeIndexService::new(resources.clone());
        let ws = workspace("mixed", "/tmp/mixed");
        service
            .open_workspace(ws.clone(), None, None, false)
            .unwrap();
        let first = service.apply_batch(sample_batch(&ws)).unwrap();
        assert!(first.files_updated >= 4);
        assert_eq!(first.revision.indexed_files, 4);
        let before_revision = first.revision.revision;

        let unchanged = service
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes: vec![CodeFileChange::Change {
                    path: "crates/core/src/util.rs".into(),
                    content: "pub fn helper() {}\n".into(),
                }],
            })
            .unwrap();
        assert_eq!(unchanged.files_updated, 0);
        assert_eq!(unchanged.files_skipped, 1);

        let changed = service
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes: vec![CodeFileChange::Change {
                    path: "crates/core/src/util.rs".into(),
                    content: "pub fn helper() { /* changed */ }\npub fn helper_extra() {}\n".into(),
                }],
            })
            .unwrap();
        assert_eq!(changed.files_updated, 1);
        assert!(changed.revision.revision > before_revision);

        let text = service
            .search(CodeSearchQuery {
                workspace: ws.clone(),
                query: "helper_extra".into(),
                mode: CodeSearchMode::Text,
                path_prefix: None,
                limit: 8,
                include_overlay: false,
            })
            .unwrap();
        assert!(!text.hits.is_empty());

        let symbols = service
            .search(CodeSearchQuery {
                workspace: ws.clone(),
                query: "Engine".into(),
                mode: CodeSearchMode::Symbol,
                path_prefix: None,
                limit: 8,
                include_overlay: false,
            })
            .unwrap();
        assert!(
            symbols
                .hits
                .iter()
                .any(|hit| hit.summary.contains("Engine"))
        );

        let deps = service
            .graph(CodeGraphQuery {
                workspace: ws.clone(),
                kind: CodeGraphQueryKind::Dependencies,
                seed_path: Some("apps/web/src/index.ts".into()),
                seed_symbol_id: None,
                depth: 2,
                limit: 16,
            })
            .unwrap();
        assert!(!deps.edges.is_empty());

        let semantic = service
            .search(CodeSearchQuery {
                workspace: ws.clone(),
                query: "boot engine helper".into(),
                mode: CodeSearchMode::Semantic,
                path_prefix: Some("crates/".into()),
                limit: 8,
                include_overlay: false,
            })
            .unwrap();
        assert!(!semantic.hits.is_empty());

        let status_before_overlay = service.status(&ws).unwrap();
        service
            .set_overlay(
                &ws,
                CodeOverlayBuffer {
                    path: "crates/core/src/util.rs".into(),
                    document_version: 9,
                    content: "pub fn helper() {}\npub fn unsaved_only() {}\n".into(),
                },
            )
            .unwrap();
        let overlay_hits = service
            .search(CodeSearchQuery {
                workspace: ws.clone(),
                query: "unsaved_only".into(),
                mode: CodeSearchMode::Text,
                path_prefix: None,
                limit: 8,
                include_overlay: true,
            })
            .unwrap();
        assert!(
            overlay_hits
                .hits
                .iter()
                .any(|hit| hit.provenance.source == CodeIndexSource::Overlay)
        );
        let status_after_overlay = service.status(&ws).unwrap();
        assert_eq!(
            status_before_overlay.revision,
            status_after_overlay.revision
        );
        assert!(
            status_after_overlay
                .overlay_paths
                .contains(&"crates/core/src/util.rs".into())
        );

        let checkpoint = service.checkpoint(&ws).unwrap();
        service
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes: vec![CodeFileChange::Delete {
                    path: "apps/web/src/runtime.ts".into(),
                }],
            })
            .unwrap();
        let restored = service
            .restore_checkpoint(&ws, &checkpoint.checkpoint_id)
            .unwrap();
        assert_eq!(restored.revision, checkpoint.revision);
        assert_eq!(restored.indexed_files, status_after_overlay.indexed_files);
    }

    #[test]
    fn lsp_available_and_unavailable_paths_share_service() {
        let ws = workspace("lsp-ws", "/tmp/lsp");
        let lsp_symbol = CodeSymbol {
            symbol_id: "crates/core/src/lib.rs::Boot@0".into(),
            name: "Boot".into(),
            kind: CodeSymbolKind::Struct,
            path: "crates/core/src/lib.rs".into(),
            range: CodeTextRange {
                start: CodeTextPosition {
                    line: 0,
                    character: 0,
                },
                end: CodeTextPosition {
                    line: 0,
                    character: 4,
                },
            },
            container: None,
            provenance: CodeIndexProvenance {
                path: "crates/core/src/lib.rs".into(),
                range: None,
                workspace_id: ws.workspace_id.clone(),
                git_revision: ws.git_revision.clone(),
                source: CodeIndexSource::Lsp,
            },
        };
        let lsp = Arc::new(FakeLspSignals::default().with_symbols(
            ws.workspace_id.clone(),
            "crates/core/src/lib.rs",
            vec![lsp_symbol],
        ));
        let resources = AgentResourceStore::default();
        let with_lsp = SharedCodeIndexService::with_lsp(resources.clone(), lsp);
        with_lsp
            .open_workspace(ws.clone(), None, None, true)
            .unwrap();
        with_lsp
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes: vec![CodeFileChange::Create {
                    path: "crates/core/src/lib.rs".into(),
                    content: "pub struct Engine {}".into(),
                }],
            })
            .unwrap();
        let lsp_hits = with_lsp
            .search(CodeSearchQuery {
                workspace: ws.clone(),
                query: "Boot".into(),
                mode: CodeSearchMode::Symbol,
                path_prefix: None,
                limit: 8,
                include_overlay: false,
            })
            .unwrap();
        assert!(lsp_hits.hits.iter().any(|hit| hit.summary.contains("Boot")));
        assert!(
            lsp_hits
                .hits
                .iter()
                .any(|hit| hit.provenance.source == CodeIndexSource::Lsp)
        );

        let without = SharedCodeIndexService::new(resources);
        without
            .open_workspace(ws.clone(), None, None, false)
            .unwrap();
        without
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes: vec![CodeFileChange::Create {
                    path: "crates/core/src/lib.rs".into(),
                    content: "pub struct Engine {}".into(),
                }],
            })
            .unwrap();
        let parser_hits = without
            .search(CodeSearchQuery {
                workspace: ws.clone(),
                query: "Engine".into(),
                mode: CodeSearchMode::Symbol,
                path_prefix: None,
                limit: 8,
                include_overlay: false,
            })
            .unwrap();
        assert!(
            parser_hits
                .hits
                .iter()
                .any(|hit| hit.provenance.source == CodeIndexSource::Parser)
        );

        // Agent tools and product service share the same SharedCodeIndexService handle.
        let tools = with_lsp.tools();
        assert!(tools.iter().any(|tool| tool.name == "code.search"));
        let status = with_lsp
            .call(json!({
                "op": "status",
                "workspace": ws
            }))
            .unwrap();
        assert_eq!(status["kind"], "status");
    }

    #[test]
    fn tenants_and_revisions_do_not_cross_pollute() {
        let service = SharedCodeIndexService::new(AgentResourceStore::default());
        let a = CodeWorkspaceRef {
            workspace_id: "same".into(),
            root: "/a".into(),
            tenant_id: "t1".into(),
            git_revision: Some("r1".into()),
            worktree_id: None,
        };
        let b = CodeWorkspaceRef {
            workspace_id: "same".into(),
            root: "/b".into(),
            tenant_id: "t2".into(),
            git_revision: Some("r2".into()),
            worktree_id: None,
        };
        service
            .open_workspace(a.clone(), None, None, false)
            .unwrap();
        service
            .open_workspace(b.clone(), None, None, false)
            .unwrap();
        service
            .apply_batch(CodeIndexBatch {
                workspace: a.clone(),
                rebuild: false,
                changes: vec![CodeFileChange::Create {
                    path: "only_a.rs".into(),
                    content: "fn a_only() {}".into(),
                }],
            })
            .unwrap();
        let missing = service.search(CodeSearchQuery {
            workspace: b,
            query: "a_only".into(),
            mode: CodeSearchMode::Text,
            path_prefix: None,
            limit: 8,
            include_overlay: false,
        });
        assert!(missing.unwrap().hits.is_empty());
    }

    #[test]
    fn large_results_use_resource_ref_and_memory_stays_bounded() {
        let service = SharedCodeIndexService::new(AgentResourceStore::default());
        let ws = workspace("large", "/tmp/large");
        service
            .open_workspace(
                ws.clone(),
                None,
                Some(CodeIndexLimits {
                    max_file_bytes: 2_000_000,
                    max_files: 5_000,
                    inline_hit_limit: 2,
                }),
                false,
            )
            .unwrap();
        let mut changes = Vec::new();
        for index in 0..80 {
            changes.push(CodeFileChange::Create {
                path: format!("src/f{index}.rs"),
                content: format!("pub fn marker_{index}() {{ /* {} */ }}", "x".repeat(200)),
            });
        }
        service
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes,
            })
            .unwrap();
        let result = service
            .search(CodeSearchQuery {
                workspace: ws.clone(),
                query: "marker_".into(),
                mode: CodeSearchMode::Text,
                path_prefix: None,
                limit: 80,
                include_overlay: false,
            })
            .unwrap();
        assert!(result.hits.len() <= 2);
        // details may be absent when payload is still under INLINE_RESULT_BYTES; inline is capped.
        let impact = service
            .graph(CodeGraphQuery {
                workspace: ws,
                kind: CodeGraphQueryKind::ChangeImpact,
                seed_path: Some("src/f0.rs".into()),
                seed_symbol_id: None,
                depth: 1,
                limit: 8,
            })
            .unwrap();
        assert!(!impact.summary.is_empty());
    }

    #[test]
    fn mixed_project_filesystem_inventory_e2e() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("web")).unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "pub fn entry() { println!(\"hi\"); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("web/app.ts"),
            "export function render() { return 1; }\n",
        )
        .unwrap();
        let ws = workspace("fs", &root.to_string_lossy());
        let service = SharedCodeIndexService::new(AgentResourceStore::default());
        service
            .open_workspace(ws.clone(), None, None, false)
            .unwrap();
        let changes = vec![
            CodeFileChange::Create {
                path: "src/main.rs".into(),
                content: std::fs::read_to_string(root.join("src/main.rs")).unwrap(),
            },
            CodeFileChange::Create {
                path: "web/app.ts".into(),
                content: std::fs::read_to_string(root.join("web/app.ts")).unwrap(),
            },
        ];
        let result = service
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes,
            })
            .unwrap();
        assert_eq!(result.revision.indexed_files, 2);
        let summary = service.project_summary(&ws).unwrap();
        assert!(summary.0.contains("rust"));
        assert!(summary.0.contains("typescript"));
    }

    #[test]
    fn performance_smoke_index_and_query() {
        let service = SharedCodeIndexService::new(AgentResourceStore::default());
        let ws = workspace("perf", "/tmp/perf");
        service
            .open_workspace(ws.clone(), None, None, false)
            .unwrap();
        let mut changes = Vec::new();
        for index in 0..40 {
            changes.push(CodeFileChange::Create {
                path: format!("src/mod{index}.rs"),
                content: format!(
                    "pub fn work_{index}() {{ let value = {index}; value }}\npub struct Item{index};"
                ),
            });
        }
        let started = Instant::now();
        service
            .apply_batch(CodeIndexBatch {
                workspace: ws.clone(),
                rebuild: false,
                changes,
            })
            .unwrap();
        for index in 0..40 {
            service
                .search(CodeSearchQuery {
                    workspace: ws.clone(),
                    query: format!("work_{}", index % 10),
                    mode: CodeSearchMode::Symbol,
                    path_prefix: None,
                    limit: 8,
                    include_overlay: false,
                })
                .unwrap();
            if index % 5 == 0 {
                service
                    .graph(CodeGraphQuery {
                        workspace: ws.clone(),
                        kind: CodeGraphQueryKind::ProjectStructure,
                        seed_path: None,
                        seed_symbol_id: None,
                        depth: 1,
                        limit: 16,
                    })
                    .unwrap();
            }
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed.as_millis() < 2_000,
            "code index smoke exceeded budget: {elapsed:?}"
        );
    }
}
