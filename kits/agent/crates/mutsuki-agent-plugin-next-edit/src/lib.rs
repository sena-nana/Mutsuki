// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_sign_loss,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::unnecessary_wraps
)]

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mutsuki_agent_adapter_api::ModelProtocolAdapter;
use mutsuki_agent_contracts::{
    AgentError, AgentMessage, AgentModelGenerateRequest, AgentPluginStateKind,
    AgentServiceDescriptor, AgentToolDescriptor, ContextProviderRequest, ContextProviderResult,
    DocumentVersion, EditorDocumentRef, EditorWorkspaceRef, FileChangeDescriptor, FileChangeStatus,
    GitHeadIdentity, ModelGenerateRequest, NextEditCandidate, NextEditFeedback,
    NextEditFeedbackKind, NextEditFeedbackStats, NextEditPlanningPath, NextEditRequest,
    NextEditServiceRequest, NextEditServiceResponse, NextEditStaleConflict, NextEditTarget,
    ProviderInstanceDescriptor, RecentEditEvent, TextPosition, TextSelection, ToolSideEffect,
    WorkspaceEditProposal, WorkspaceTextEdit,
};
use mutsuki_agent_plugin_api::{AgentPluginRegistrar, AgentService, ContextProvider, ToolProvider};
use mutsuki_agent_runtime::AgentResourceStore;
use serde::Deserialize;
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.next-edit";
pub const SERVICE_ID: &str = "mutsuki.agent.service.next-edit";
pub const CONTEXT_PROVIDER_ID: &str = "mutsuki.agent.context.next-edit";

const INLINE_PREVIEW_LIMIT: usize = 2_048;
const DEFAULT_DEBOUNCE_MS: u64 = 80;
const DEFAULT_TTL_MS: u64 = 15_000;

#[derive(Clone, Debug)]
pub struct NextEditServiceConfig {
    pub debounce_ms: u64,
    pub candidate_ttl_ms: u64,
    pub max_recent_edits: usize,
}

impl Default for NextEditServiceConfig {
    fn default() -> Self {
        Self {
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            candidate_ttl_ms: DEFAULT_TTL_MS,
            max_recent_edits: 64,
        }
    }
}

#[derive(Debug)]
struct ServiceState {
    recent_edits: Vec<RecentEditEvent>,
    last_edit_unix_ms: u64,
    active_generation: u64,
    cache: BTreeMap<String, NextEditCandidate>,
    candidates: BTreeMap<String, NextEditCandidate>,
    feedback: NextEditFeedbackStats,
}

/// Shared Next Edit planner. Produces versioned proposals only; never mutates the workspace.
pub struct SharedNextEditService {
    descriptor: AgentServiceDescriptor,
    resources: AgentResourceStore,
    config: NextEditServiceConfig,
    planner: Option<Arc<dyn NextEditPlanner>>,
    next_candidate: AtomicU64,
    state: Mutex<ServiceState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedNextEdit {
    pub document: EditorDocumentRef,
    pub edit: WorkspaceTextEdit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NextEditPlan {
    pub reason: String,
    pub confidence: f64,
    pub edits: Vec<PlannedNextEdit>,
}

/// Replaceable inference boundary for Next Edit. Implementations may call a
/// lightweight protocol model adapter, but must not mutate the workspace.
pub trait NextEditPlanner: Send + Sync {
    fn plan(
        &self,
        request: &NextEditRequest,
        targets: &[NextEditTarget],
    ) -> Result<Option<NextEditPlan>, AgentError>;
}

#[derive(Clone)]
pub struct ProtocolNextEditPlanner {
    adapter: Arc<dyn ModelProtocolAdapter>,
    provider: ProviderInstanceDescriptor,
    model: String,
}

impl ProtocolNextEditPlanner {
    pub fn new(
        adapter: Arc<dyn ModelProtocolAdapter>,
        provider: ProviderInstanceDescriptor,
        model: impl Into<String>,
    ) -> Result<Self, AgentError> {
        let model = model.into();
        if model.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "next edit planner model is required",
            ));
        }
        if provider.adapter_id != adapter.descriptor().adapter_id {
            return Err(AgentError::invalid_input(
                "next edit provider does not belong to the selected adapter",
            ));
        }
        Ok(Self {
            adapter,
            provider,
            model,
        })
    }
}

impl SharedNextEditService {
    pub fn new(resources: AgentResourceStore) -> Self {
        Self::with_config(resources, NextEditServiceConfig::default())
    }

    pub fn with_config(resources: AgentResourceStore, config: NextEditServiceConfig) -> Self {
        Self::with_optional_planner(resources, config, None)
    }

    pub fn with_planner(
        resources: AgentResourceStore,
        config: NextEditServiceConfig,
        planner: Arc<dyn NextEditPlanner>,
    ) -> Self {
        Self::with_optional_planner(resources, config, Some(planner))
    }

    pub fn with_protocol_model(
        resources: AgentResourceStore,
        config: NextEditServiceConfig,
        adapter: Arc<dyn ModelProtocolAdapter>,
        provider: ProviderInstanceDescriptor,
        model: impl Into<String>,
    ) -> Result<Self, AgentError> {
        let planner = ProtocolNextEditPlanner::new(adapter, provider, model)?;
        Ok(Self::with_planner(resources, config, Arc::new(planner)))
    }

    fn with_optional_planner(
        resources: AgentResourceStore,
        config: NextEditServiceConfig,
        planner: Option<Arc<dyn NextEditPlanner>>,
    ) -> Self {
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.next_edit.request@1".into(),
                response_schema: "mutsuki.agent.next_edit.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("workspace".into()),
            },
            resources,
            config,
            planner,
            next_candidate: AtomicU64::new(1),
            state: Mutex::new(ServiceState {
                recent_edits: Vec::new(),
                last_edit_unix_ms: 0,
                active_generation: 1,
                cache: BTreeMap::new(),
                candidates: BTreeMap::new(),
                feedback: NextEditFeedbackStats::default(),
            }),
        }
    }

    pub fn plugin_descriptor(
        generation: u64,
    ) -> Result<mutsuki_agent_contracts::AgentKitPluginDescriptor, AgentError> {
        let mut tool = AgentToolDescriptor::new(
            "next_edit.plan",
            "mutsuki.agent.tool.next_edit.plan@1",
            "Plan the next edit proposal from recent edits, diagnostics and git diff",
        );
        tool.side_effect = ToolSideEffect::WorkspaceRead;
        AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.next_edit.request@1".into(),
                response_schema: "mutsuki.agent.next_edit.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("workspace".into()),
            })
            .context_provider(CONTEXT_PROVIDER_ID)
            .tool(tool)
            .require_service(SERVICE_ID)
            .build()
    }

    pub fn call_typed(
        &self,
        request: NextEditServiceRequest,
    ) -> Result<NextEditServiceResponse, AgentError> {
        match request {
            NextEditServiceRequest::IngestRecentEdit { event } => {
                self.ingest(event)?;
                Ok(NextEditServiceResponse::Ack)
            }
            NextEditServiceRequest::Plan { request } => self.plan(*request),
            NextEditServiceRequest::Validate {
                candidate_id,
                document_versions,
                git_head,
                now_unix_ms,
            } => self.validate(
                &candidate_id,
                &document_versions,
                git_head.as_ref(),
                now_unix_ms,
            ),
            NextEditServiceRequest::Feedback { feedback } => self.record_feedback(feedback),
            NextEditServiceRequest::Cancel { generation } => self.cancel(generation),
            NextEditServiceRequest::Stats => {
                let state = self.state.lock().expect("next-edit state");
                Ok(NextEditServiceResponse::Stats {
                    stats: state.feedback.clone(),
                    active_generation: state.active_generation,
                    cached_candidates: state.cache.len() as u64,
                })
            }
        }
    }

    fn ingest(&self, event: RecentEditEvent) -> Result<(), AgentError> {
        if event.event_id.trim().is_empty() || event.document.uri.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "recent edit event_id and document uri are required",
            ));
        }
        let mut state = self.state.lock().expect("next-edit state");
        state.last_edit_unix_ms = event.timestamp_unix_ms.max(state.last_edit_unix_ms);
        state.active_generation = state
            .active_generation
            .saturating_add(1)
            .max(event.editor_generation);
        // New edits invalidate prior candidates so late results cannot cover newer work.
        state.cache.clear();
        state.candidates.clear();
        state.recent_edits.push(event);
        if state.recent_edits.len() > self.config.max_recent_edits {
            let drop = state.recent_edits.len() - self.config.max_recent_edits;
            state.recent_edits.drain(0..drop);
        }
        Ok(())
    }

    fn cancel(&self, generation: u64) -> Result<NextEditServiceResponse, AgentError> {
        let mut state = self.state.lock().expect("next-edit state");
        state.active_generation = state.active_generation.max(generation).saturating_add(1);
        state.cache.clear();
        state.candidates.clear();
        Ok(NextEditServiceResponse::Cancelled {
            generation: state.active_generation,
        })
    }

    fn plan(&self, request: NextEditRequest) -> Result<NextEditServiceResponse, AgentError> {
        if request.request_id.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "next edit request_id is required",
            ));
        }
        if request.workspace.workspace_id.trim().is_empty() {
            return Err(AgentError::invalid_input("workspace_id is required"));
        }

        let mut state = self.state.lock().expect("next-edit state");
        if request.generation < state.active_generation {
            return Ok(NextEditServiceResponse::Superseded {
                generation: request.generation,
                active_generation: state.active_generation,
            });
        }
        state.active_generation = request.generation;

        if let Some(deadline) = request.deadline_unix_ms
            && request.now_unix_ms > deadline
        {
            return Ok(NextEditServiceResponse::TimedOut {
                request_id: request.request_id,
            });
        }

        if self.config.debounce_ms > 0
            && state.last_edit_unix_ms > 0
            && request.now_unix_ms.saturating_sub(state.last_edit_unix_ms) < self.config.debounce_ms
        {
            // Still inside debounce window: do not interrupt editing.
            return Ok(NextEditServiceResponse::Candidate { candidate: None });
        }

        let cache_key = context_cache_key(&request);
        if let Some(cached) = state.cache.get(&cache_key).cloned()
            && cached.expires_at_unix_ms >= request.now_unix_ms
            && cached.generation == request.generation
        {
            return Ok(NextEditServiceResponse::Candidate {
                candidate: Some(Box::new(cached)),
            });
        }

        let planned = plan_candidate(
            &self.resources,
            self.planner.as_deref(),
            &request,
            &state.recent_edits,
            self.next_candidate.fetch_add(1, Ordering::Relaxed),
            self.config.candidate_ttl_ms,
        )?;

        let Some(candidate) = planned else {
            return Ok(NextEditServiceResponse::Candidate { candidate: None });
        };

        if candidate.confidence < request.min_confidence {
            return Ok(NextEditServiceResponse::Candidate { candidate: None });
        }

        state.cache.insert(cache_key, candidate.clone());
        state
            .candidates
            .insert(candidate.candidate_id.clone(), candidate.clone());
        Ok(NextEditServiceResponse::Candidate {
            candidate: Some(Box::new(candidate)),
        })
    }

    fn validate(
        &self,
        candidate_id: &str,
        document_versions: &[(EditorDocumentRef, DocumentVersion)],
        git_head: Option<&GitHeadIdentity>,
        now_unix_ms: u64,
    ) -> Result<NextEditServiceResponse, AgentError> {
        let state = self.state.lock().expect("next-edit state");
        let candidate = state.candidates.get(candidate_id).cloned().ok_or_else(|| {
            AgentError::not_found(format!("candidate `{candidate_id}` not found"))
        })?;

        if candidate.expires_at_unix_ms < now_unix_ms {
            return Ok(NextEditServiceResponse::Stale {
                conflict: Box::new(NextEditStaleConflict {
                    document: None,
                    expected_version: None,
                    actual_version: None,
                    expected_git_head: candidate.expected_git_head.clone(),
                    actual_git_head: git_head.cloned(),
                    message: format!("candidate `{candidate_id}` expired"),
                }),
            });
        }

        if candidate.generation < state.active_generation {
            return Ok(NextEditServiceResponse::Superseded {
                generation: candidate.generation,
                active_generation: state.active_generation,
            });
        }

        for (expected_doc, expected_version) in &candidate.expected_document_versions {
            let actual = document_versions
                .iter()
                .find(|(doc, _)| doc == expected_doc)
                .map(|(_, version)| *version);
            match actual {
                Some(actual) if actual == *expected_version => {}
                Some(actual) => {
                    return Ok(NextEditServiceResponse::Stale {
                        conflict: Box::new(NextEditStaleConflict {
                            document: Some(expected_doc.clone()),
                            expected_version: Some(*expected_version),
                            actual_version: Some(actual),
                            expected_git_head: candidate.expected_git_head.clone(),
                            actual_git_head: git_head.cloned(),
                            message: format!(
                                "document `{}` expected version {}, observed {}",
                                expected_doc.uri, expected_version.0, actual.0
                            ),
                        }),
                    });
                }
                None => {
                    return Ok(NextEditServiceResponse::Stale {
                        conflict: Box::new(NextEditStaleConflict {
                            document: Some(expected_doc.clone()),
                            expected_version: Some(*expected_version),
                            actual_version: None,
                            expected_git_head: candidate.expected_git_head.clone(),
                            actual_git_head: git_head.cloned(),
                            message: format!(
                                "document `{}` version missing during validation",
                                expected_doc.uri
                            ),
                        }),
                    });
                }
            }
        }

        if let Some(expected) = &candidate.expected_git_head {
            match git_head {
                Some(actual)
                    if actual.commit == expected.commit
                        && actual.generation == expected.generation => {}
                Some(actual) => {
                    return Ok(NextEditServiceResponse::Stale {
                        conflict: Box::new(NextEditStaleConflict {
                            document: None,
                            expected_version: None,
                            actual_version: None,
                            expected_git_head: Some(expected.clone()),
                            actual_git_head: Some(actual.clone()),
                            message: format!(
                                "git head expected {} (gen {}), observed {} (gen {})",
                                expected.commit,
                                expected.generation,
                                actual.commit,
                                actual.generation
                            ),
                        }),
                    });
                }
                None => {
                    return Ok(NextEditServiceResponse::Stale {
                        conflict: Box::new(NextEditStaleConflict {
                            document: None,
                            expected_version: None,
                            actual_version: None,
                            expected_git_head: Some(expected.clone()),
                            actual_git_head: None,
                            message: "git head missing during validation".into(),
                        }),
                    });
                }
            }
        }

        Ok(NextEditServiceResponse::Valid {
            candidate_id: candidate.candidate_id,
        })
    }

    fn record_feedback(
        &self,
        feedback: NextEditFeedback,
    ) -> Result<NextEditServiceResponse, AgentError> {
        if feedback.candidate_id.trim().is_empty() {
            return Err(AgentError::invalid_input("candidate_id is required"));
        }
        let mut state = self.state.lock().expect("next-edit state");
        match feedback.kind {
            NextEditFeedbackKind::Accepted => state.feedback.accepted += 1,
            NextEditFeedbackKind::Rejected => state.feedback.rejected += 1,
            NextEditFeedbackKind::Skipped => state.feedback.skipped += 1,
        }
        Ok(NextEditServiceResponse::FeedbackRecorded {
            stats: state.feedback.clone(),
        })
    }
}

fn context_cache_key(request: &NextEditRequest) -> String {
    let mut versions = request
        .document_versions
        .iter()
        .map(|(doc, version)| format!("{}@{}", doc.uri, version.0))
        .collect::<Vec<_>>();
    versions.sort();
    let diagnostics = request
        .diagnostics
        .iter()
        .map(|hint| {
            format!(
                "{}:{}:{}",
                hint.document.uri, hint.diagnostic.range.start.line, hint.diagnostic.message
            )
        })
        .collect::<Vec<_>>();
    let diffs = request
        .git_diff
        .iter()
        .map(|hint| format!("{}:{}", hint.path, hint.summary))
        .collect::<Vec<_>>();
    format!(
        "{}|{}|{}|{}|{}|{:?}",
        request.workspace.workspace_id,
        request.editor_generation,
        versions.join(","),
        diagnostics.join(","),
        diffs.join(","),
        request.path
    )
}

fn plan_candidate(
    resources: &AgentResourceStore,
    planner: Option<&dyn NextEditPlanner>,
    request: &NextEditRequest,
    recent_edits: &[RecentEditEvent],
    candidate_seq: u64,
    ttl_ms: u64,
) -> Result<Option<NextEditCandidate>, AgentError> {
    let mut signals = Vec::new();

    for hint in &request.diagnostics {
        signals.push(PlanSignal {
            document: hint.document.clone(),
            range: Some(TextSelection {
                start: TextPosition {
                    line: hint.diagnostic.range.start.line,
                    character: hint.diagnostic.range.start.character,
                },
                end: TextPosition {
                    line: hint.diagnostic.range.end.line,
                    character: hint.diagnostic.range.end.character,
                },
            }),
            reason: format!("diagnostic: {}", truncate(&hint.diagnostic.message, 96)),
            confidence: 0.82,
            from_diff: false,
        });
    }

    for edit in recent_edits
        .iter()
        .chain(request.recent_edits.iter())
        .rev()
        .take(8)
    {
        if edit.document.workspace_id != request.workspace.workspace_id {
            continue;
        }
        signals.push(PlanSignal {
            document: edit.document.clone(),
            range: edit.range,
            reason: format!("recent_edit: {}", truncate(&edit.summary, 96)),
            confidence: 0.7,
            from_diff: false,
        });
    }

    for hint in &request.git_diff {
        let related = request
            .related_paths
            .iter()
            .any(|path| path.as_str() == hint.path.as_str() || hint.path.ends_with(path.as_str()));
        signals.push(PlanSignal {
            document: EditorDocumentRef {
                workspace_id: request.workspace.workspace_id.clone(),
                uri: path_to_uri(&hint.path),
            },
            range: None,
            reason: format!("git_diff: {}", truncate(&hint.summary, 96)),
            confidence: if related { 0.74 } else { 0.68 },
            from_diff: true,
        });
    }

    if signals.is_empty() {
        return Ok(None);
    }

    signals.sort_by(|left, right| {
        right
            .confidence
            .partial_cmp(&left.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let primary = &signals[0];
    let mut targets = vec![NextEditTarget {
        document: primary.document.clone(),
        range: primary.range,
        reason: primary.reason.clone(),
        confidence: primary.confidence,
    }];

    let multi_file = request.allow_multi_file
        && signals.iter().any(|signal| {
            signal.from_diff
                && signal.document.uri != primary.document.uri
                && signal.confidence >= request.min_confidence
        });

    if multi_file {
        for signal in signals
            .iter()
            .filter(|signal| signal.document.uri != primary.document.uri && signal.from_diff)
            .take(2)
        {
            targets.push(NextEditTarget {
                document: signal.document.clone(),
                range: signal.range,
                reason: signal.reason.clone(),
                confidence: signal.confidence,
            });
        }
    }

    let path = if multi_file && matches!(request.path, NextEditPlanningPath::ShortAgent) {
        NextEditPlanningPath::ShortAgent
    } else if multi_file {
        // Multi-file stays on lightweight planning unless the caller opted into short agent.
        NextEditPlanningPath::Lightweight
    } else {
        request.path
    };

    let mut confidence = targets
        .iter()
        .map(|target| target.confidence)
        .fold(0.0_f64, f64::max);
    let mut reason = if multi_file {
        format!(
            "next edit across {} files ({})",
            targets.len(),
            truncate(&primary.reason, 64)
        )
    } else {
        primary.reason.clone()
    };

    let planned = planner
        .map(|planner| planner.plan(request, &targets))
        .transpose()?
        .flatten();
    if planner.is_some() && planned.is_none() {
        return Ok(None);
    }
    let mut edits_by_document = BTreeMap::<EditorDocumentRef, Vec<WorkspaceTextEdit>>::new();
    if let Some(planned) = planned {
        if planned.edits.is_empty() {
            return Ok(None);
        }
        confidence = confidence.min(planned.confidence.clamp(0.0, 1.0));
        if !planned.reason.trim().is_empty() {
            reason = planned.reason;
        }
        for planned_edit in planned.edits {
            edits_by_document
                .entry(planned_edit.document)
                .or_default()
                .push(planned_edit.edit);
        }
    }

    let changes = targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            file_change(
                format!("chg-{candidate_seq}-{index}"),
                request.workspace.clone(),
                &target.document,
                expected_version_for(request, &target.document),
                &target.reason,
                edits_by_document
                    .remove(&target.document)
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    if planner.is_some() && changes.iter().all(|change| change.edits.is_empty()) {
        return Ok(None);
    }

    let preview_payload = json!({
        "targets": targets.len(),
        "changes": changes.len(),
        "path": path,
        "reason": reason,
        // Intentionally omit source code bodies from retained preview metadata.
        "uris": targets.iter().map(|target| target.document.uri.clone()).collect::<Vec<_>>(),
    });
    let preview_text = serde_json::to_string_pretty(&preview_payload)
        .map_err(|error| AgentError::invalid_input(error.to_string()))?;
    let preview_ref = if preview_text.len() > INLINE_PREVIEW_LIMIT {
        Some(resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.next_edit.preview",
            "mutsuki.agent.next_edit.preview@1",
            1,
            &preview_payload,
        )?)
    } else {
        None
    };

    let proposal = WorkspaceEditProposal {
        proposal_id: format!("next-edit-proposal-{candidate_seq}"),
        workspace: request.workspace.clone(),
        changes,
        summary: reason.clone(),
        details: preview_ref.clone(),
    };

    Ok(Some(NextEditCandidate {
        candidate_id: format!("next-edit-{candidate_seq}"),
        request_id: request.request_id.clone(),
        generation: request.generation,
        created_at_unix_ms: request.now_unix_ms,
        expires_at_unix_ms: request.now_unix_ms.saturating_add(ttl_ms),
        confidence,
        reason,
        path,
        targets,
        proposal,
        expected_document_versions: request.document_versions.clone(),
        expected_git_head: request.expected_git_head.clone(),
        requires_preview: multi_file,
        preview_ref,
    }))
}

struct PlanSignal {
    document: EditorDocumentRef,
    range: Option<TextSelection>,
    reason: String,
    confidence: f64,
    from_diff: bool,
}

fn file_change(
    change_id: String,
    workspace: EditorWorkspaceRef,
    document: &EditorDocumentRef,
    base_version: DocumentVersion,
    summary: &str,
    edits: Vec<WorkspaceTextEdit>,
) -> FileChangeDescriptor {
    FileChangeDescriptor {
        change_id,
        workspace,
        document: document.clone(),
        base_version,
        status: FileChangeStatus::Proposed,
        summary: truncate(summary, 160),
        edits,
        details: None,
        rejection_reason: None,
    }
}

#[derive(Deserialize)]
struct ModelPlan {
    #[serde(default)]
    reason: String,
    confidence: f64,
    edits: Vec<ModelTextEdit>,
}

#[derive(Deserialize)]
struct ModelTextEdit {
    uri: String,
    start: TextPosition,
    end: TextPosition,
    new_text: String,
}

impl NextEditPlanner for ProtocolNextEditPlanner {
    fn plan(
        &self,
        request: &NextEditRequest,
        targets: &[NextEditTarget],
    ) -> Result<Option<NextEditPlan>, AgentError> {
        let allowed = targets
            .iter()
            .map(|target| target.document.uri.as_str())
            .collect::<Vec<_>>();
        let contexts = request
            .document_contexts
            .iter()
            .filter_map(|context| {
                let text = context.inline_text.as_deref()?;
                if !allowed.contains(&context.document.uri.as_str()) {
                    return None;
                }
                Some(json!({
                    "uri": context.document.uri,
                    "version": context.version.0,
                    "language": context.language_id,
                    "selection": context.selection,
                    "text": truncate_bytes(text, 32 * 1024),
                }))
            })
            .collect::<Vec<_>>();
        if contexts.is_empty() {
            return Ok(None);
        }

        let prompt = serde_json::to_string(&json!({
            "intent": request.intent,
            "targets": targets,
            "documents": contexts,
            "constraints": {
                "allow_multi_file": request.allow_multi_file,
                "maximum_edits": 16,
                "maximum_total_new_text_bytes": 64 * 1024,
            }
        }))
        .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let generate = ModelGenerateRequest {
            request: AgentModelGenerateRequest {
                model: self.model.clone(),
                messages: vec![
                    AgentMessage::system(
                        "Return only strict JSON for the next concrete editor edit. Shape: {\"reason\":string,\"confidence\":number,\"edits\":[{\"uri\":string,\"start\":{\"line\":number,\"character\":number},\"end\":{\"line\":number,\"character\":number},\"new_text\":string}]}. Use only supplied target URIs. Do not call tools.",
                    ),
                    AgentMessage::user(prompt),
                ],
                temperature: Some(0.0),
                max_output_tokens: Some(1_024),
                provider_hint: Some(self.provider.provider_id.clone()),
                metadata: Some(json!({
                    "mode": "next_edit",
                    "request_id": request.request_id,
                    "generation": request.generation,
                })),
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            },
            tools: Vec::new(),
            structured_output: None,
            reasoning: None,
        };

        let started = Instant::now();
        let result = block_on_adapter(self.adapter.generate(self.provider.clone(), generate))
            .map_err(|error| {
                AgentError::new(
                    error.code,
                    format!("next edit protocol adapter failed: {}", error.message),
                )
            })?;
        if let Some(deadline) = request.deadline_unix_ms {
            let remaining = deadline.saturating_sub(request.now_unix_ms);
            if started.elapsed() > Duration::from_millis(remaining) {
                return Ok(None);
            }
        }
        if !result.tool_calls.is_empty() {
            return Err(AgentError::new(
                "agent.next_edit.tool_forbidden",
                "next edit planner must not emit tool calls",
            ));
        }

        let raw = extract_json_object(&result.message.content).ok_or_else(|| {
            AgentError::new(
                "agent.next_edit.invalid_model_output",
                "next edit model did not return a JSON object",
            )
        })?;
        let model_plan: ModelPlan = serde_json::from_str(raw).map_err(|_| {
            AgentError::new(
                "agent.next_edit.invalid_model_output",
                "next edit model returned an invalid edit payload",
            )
        })?;
        if !model_plan.confidence.is_finite()
            || !(0.0..=1.0).contains(&model_plan.confidence)
            || model_plan.edits.is_empty()
            || model_plan.edits.len() > 16
        {
            return Ok(None);
        }

        let mut total_bytes = 0usize;
        let mut edits = Vec::with_capacity(model_plan.edits.len());
        for edit in model_plan.edits {
            let Some(document) = targets
                .iter()
                .find(|target| target.document.uri == edit.uri)
                .map(|target| target.document.clone())
            else {
                return Ok(None);
            };
            if position_after(edit.start, edit.end) {
                return Ok(None);
            }
            total_bytes = total_bytes.saturating_add(edit.new_text.len());
            if total_bytes > 64 * 1024 {
                return Ok(None);
            }
            edits.push(PlannedNextEdit {
                document,
                edit: WorkspaceTextEdit {
                    range: TextSelection {
                        start: edit.start,
                        end: edit.end,
                    },
                    new_text: edit.new_text,
                },
            });
        }
        Ok(Some(NextEditPlan {
            reason: truncate(&model_plan.reason, 160),
            confidence: model_plan.confidence,
            edits,
        }))
    }
}

fn block_on_adapter<T>(future: impl Future<Output = T>) -> T {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return tokio::task::block_in_place(|| handle.block_on(future));
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("next edit runtime")
        .block_on(future)
}

fn extract_json_object(value: &str) -> Option<&str> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    (end >= start).then_some(&value[start..=end])
}

fn position_after(start: TextPosition, end: TextPosition) -> bool {
    (start.line, start.character) > (end.line, end.character)
}

fn truncate_bytes(value: &str, maximum: usize) -> &str {
    if value.len() <= maximum {
        return value;
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn expected_version_for(
    request: &NextEditRequest,
    document: &EditorDocumentRef,
) -> DocumentVersion {
    request
        .document_versions
        .iter()
        .find(|(doc, _)| doc == document)
        .map(|(_, version)| *version)
        .unwrap_or(DocumentVersion(1))
}

fn path_to_uri(path: &str) -> String {
    if path.starts_with("file://") {
        path.into()
    } else {
        format!("file:///workspace/{path}")
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "…"
}

impl AgentService for SharedNextEditService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: NextEditServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = self.call_typed(request)?;
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        let mut state = self.state.lock().expect("next-edit state");
        state.cache.clear();
        state.candidates.clear();
        state.recent_edits.clear();
        Ok(())
    }
}

impl ToolProvider for SharedNextEditService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        let mut tool = AgentToolDescriptor::new(
            "next_edit.plan",
            "mutsuki.agent.tool.next_edit.plan@1",
            "Plan the next edit proposal",
        );
        tool.side_effect = ToolSideEffect::WorkspaceRead;
        vec![tool]
    }
}

impl ContextProvider for SharedNextEditService {
    fn provider_id(&self) -> &str {
        CONTEXT_PROVIDER_ID
    }

    fn collect(
        &self,
        request: ContextProviderRequest,
    ) -> Result<ContextProviderResult, AgentError> {
        let state = self.state.lock().expect("next-edit state");
        let summary = format!(
            "next-edit gen={} recent={} accepted={} rejected={} skipped={}",
            state.active_generation,
            state.recent_edits.len(),
            state.feedback.accepted,
            state.feedback.rejected,
            state.feedback.skipped
        );
        let details = self.resources.put_json(
            SERVICE_ID,
            "mutsuki.agent.next_edit.context",
            "mutsuki.agent.next_edit.context@1",
            1,
            &json!({
                "active_generation": state.active_generation,
                "recent_edits": state.recent_edits.len(),
                "feedback": state.feedback,
            }),
        )?;
        Ok(ContextProviderResult {
            provider_id: request.provider_id,
            summary,
            details: Some(details),
            estimated_tokens: 24,
            estimated_bytes: 96,
            priority: 0,
            required: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::{
        AgentModelGenerateResult, AgentModelStopReason, AgentUsage, CredentialRef, LspDiagnostic,
        LspPosition, LspRange, ModelCapability, ModelProtocolAdapterDescriptor,
        NextEditDiagnosticHint, NextEditDiffHint, NextEditDocumentContext, TextPosition,
        TextSelection,
    };
    use mutsuki_agent_testkit::FakeEditorContextService;

    struct ConcreteEditAdapter {
        descriptor: ModelProtocolAdapterDescriptor,
    }

    impl ConcreteEditAdapter {
        fn new() -> Self {
            Self {
                descriptor: ModelProtocolAdapterDescriptor {
                    adapter_id: "next-edit-test".into(),
                    protocol: "test.next-edit".into(),
                    version: "1".into(),
                    runner_id: "next-edit-test-runner".into(),
                    capability: ModelCapability::default(),
                },
            }
        }
    }

    impl ModelProtocolAdapter for ConcreteEditAdapter {
        fn descriptor(&self) -> &ModelProtocolAdapterDescriptor {
            &self.descriptor
        }

        fn generate(
            &self,
            _provider: ProviderInstanceDescriptor,
            request: ModelGenerateRequest,
        ) -> mutsuki_agent_adapter_api::ModelAdapterFuture {
            Box::pin(async move {
                assert!(request.tools.is_empty());
                assert!(request.request.session_id.is_none());
                Ok(AgentModelGenerateResult {
                    message: AgentMessage::assistant(
                        r#"{"reason":"complete the function","confidence":0.91,"edits":[{"uri":"file:///workspace/main.rs","start":{"line":0,"character":11},"end":{"line":0,"character":11},"new_text":" println!(\"hi\");"}]}"#,
                    ),
                    stop_reason: AgentModelStopReason::Stop,
                    tool_calls: Vec::new(),
                    usage: AgentUsage::default(),
                    cost_microunits: 0,
                    raw: None,
                    output_resource: None,
                })
            })
        }
    }

    fn model_provider() -> ProviderInstanceDescriptor {
        ProviderInstanceDescriptor {
            provider_id: "next-edit-provider".into(),
            adapter_id: "next-edit-test".into(),
            endpoint: "memory://next-edit".into(),
            credential: CredentialRef {
                credential_id: "next-edit-credential".into(),
                revision: 1,
            },
            models: BTreeMap::from([("next-edit-model".into(), ModelCapability::default())]),
            headers: BTreeMap::new(),
            compatibility: BTreeMap::new(),
            remote_execution_allowed: true,
        }
    }

    fn workspace() -> EditorWorkspaceRef {
        EditorWorkspaceRef {
            workspace_id: "ws".into(),
            folders: vec!["/workspace".into()],
            metadata: json!({}),
        }
    }

    fn doc(uri: &str) -> EditorDocumentRef {
        EditorDocumentRef {
            workspace_id: "ws".into(),
            uri: uri.into(),
        }
    }

    fn head(commit: &str, generation: u64) -> GitHeadIdentity {
        GitHeadIdentity {
            commit: commit.into(),
            branch: Some("main".into()),
            upstream: None,
            generation,
        }
    }

    fn base_request() -> NextEditRequest {
        NextEditRequest {
            request_id: "req-1".into(),
            workspace: workspace(),
            generation: 2,
            editor_generation: 2,
            document_versions: vec![(doc("file:///workspace/main.rs"), DocumentVersion(3))],
            document_contexts: Vec::new(),
            recent_edits: Vec::new(),
            diagnostics: Vec::new(),
            related_paths: Vec::new(),
            git_diff: Vec::new(),
            expected_git_head: Some(head("abc", 1)),
            intent: None,
            path: NextEditPlanningPath::Lightweight,
            min_confidence: 0.55,
            allow_multi_file: false,
            deadline_unix_ms: Some(10_000),
            now_unix_ms: 1_000,
            metadata: json!({}),
        }
    }

    #[test]
    fn single_file_next_edit_e2e_with_fake_editor() {
        let editor = FakeEditorContextService::default();
        editor
            .open_document("file:///workspace/main.rs", "rust", "fn main() {}", true)
            .unwrap();
        editor
            .edit_unsaved("file:///workspace/main.rs", "fn main() {\n")
            .unwrap();
        let snapshot = editor.freeze_snapshot(Some("turn-1".into())).unwrap();
        let resources = AgentResourceStore::default();
        let service = SharedNextEditService::with_config(
            resources,
            NextEditServiceConfig {
                debounce_ms: 0,
                ..NextEditServiceConfig::default()
            },
        );
        service
            .call_typed(NextEditServiceRequest::IngestRecentEdit {
                event: RecentEditEvent {
                    event_id: "e1".into(),
                    document: doc("file:///workspace/main.rs"),
                    version: DocumentVersion(2),
                    editor_generation: snapshot.generation,
                    timestamp_unix_ms: 900,
                    kind: mutsuki_agent_contracts::RecentEditKind::Replaced,
                    range: Some(TextSelection {
                        start: TextPosition {
                            line: 0,
                            character: 0,
                        },
                        end: TextPosition {
                            line: 1,
                            character: 0,
                        },
                    }),
                    summary: "opened function body".into(),
                    byte_delta: 4,
                },
            })
            .unwrap();

        let mut request = base_request();
        request.generation = snapshot.generation;
        request.editor_generation = snapshot.generation;
        request.document_versions = vec![(
            doc("file:///workspace/main.rs"),
            snapshot.documents[0].version,
        )];
        request.recent_edits = vec![RecentEditEvent {
            event_id: "e1".into(),
            document: doc("file:///workspace/main.rs"),
            version: snapshot.documents[0].version,
            editor_generation: snapshot.generation,
            timestamp_unix_ms: 900,
            kind: mutsuki_agent_contracts::RecentEditKind::Replaced,
            range: None,
            summary: "opened function body".into(),
            byte_delta: 4,
        }];

        let response = service
            .call_typed(NextEditServiceRequest::Plan {
                request: Box::new(request),
            })
            .unwrap();
        let roundtrip: NextEditServiceResponse =
            serde_json::from_value(serde_json::to_value(&response).unwrap()).unwrap();
        assert_eq!(roundtrip, response);
        let NextEditServiceResponse::Candidate {
            candidate: Some(candidate),
        } = response
        else {
            panic!("expected single-file candidate, got {response:?}");
        };
        assert_eq!(candidate.proposal.changes.len(), 1);
        assert!(!candidate.requires_preview);
        assert_eq!(candidate.path, NextEditPlanningPath::Lightweight);
        assert!(candidate.confidence >= 0.55);
        editor
            .assert_edit_base(
                &candidate.proposal.changes[0].document,
                candidate.proposal.changes[0].base_version,
            )
            .unwrap();
    }

    #[test]
    fn protocol_model_returns_concrete_versioned_workspace_edit() {
        let service = SharedNextEditService::with_protocol_model(
            AgentResourceStore::default(),
            NextEditServiceConfig {
                debounce_ms: 0,
                ..NextEditServiceConfig::default()
            },
            Arc::new(ConcreteEditAdapter::new()),
            model_provider(),
            "next-edit-model",
        )
        .unwrap();
        let mut request = base_request();
        request.recent_edits = vec![RecentEditEvent {
            event_id: "edit-model".into(),
            document: doc("file:///workspace/main.rs"),
            version: DocumentVersion(3),
            editor_generation: 2,
            timestamp_unix_ms: 900,
            kind: mutsuki_agent_contracts::RecentEditKind::Inserted,
            range: Some(TextSelection {
                start: TextPosition {
                    line: 0,
                    character: 11,
                },
                end: TextPosition {
                    line: 0,
                    character: 11,
                },
            }),
            summary: "opened function body".into(),
            byte_delta: 1,
        }];
        request.document_contexts = vec![NextEditDocumentContext {
            document: doc("file:///workspace/main.rs"),
            version: DocumentVersion(3),
            language_id: Some("rust".into()),
            selection: None,
            inline_text: Some("fn main() {}".into()),
            content_ref: None,
        }];

        let response = service
            .call_typed(NextEditServiceRequest::Plan {
                request: Box::new(request),
            })
            .unwrap();
        let NextEditServiceResponse::Candidate {
            candidate: Some(candidate),
        } = response
        else {
            panic!("expected concrete model candidate");
        };
        assert_eq!(candidate.proposal.changes.len(), 1);
        assert_eq!(
            candidate.proposal.changes[0].base_version,
            DocumentVersion(3)
        );
        assert_eq!(candidate.proposal.changes[0].edits.len(), 1);
        assert_eq!(
            candidate.proposal.changes[0].edits[0].new_text,
            " println!(\"hi\");"
        );
    }

    #[test]
    fn multi_file_proposal_requires_preview_and_version_safe_validate() {
        let service = SharedNextEditService::with_config(
            AgentResourceStore::default(),
            NextEditServiceConfig {
                debounce_ms: 0,
                ..NextEditServiceConfig::default()
            },
        );
        let mut request = base_request();
        request.allow_multi_file = true;
        request.document_versions = vec![
            (doc("file:///workspace/main.rs"), DocumentVersion(3)),
            (doc("file:///workspace/lib.rs"), DocumentVersion(5)),
        ];
        request.git_diff = vec![
            NextEditDiffHint {
                path: "main.rs".into(),
                summary: "export new helper".into(),
                details: None,
            },
            NextEditDiffHint {
                path: "lib.rs".into(),
                summary: "wire helper import".into(),
                details: None,
            },
        ];
        request.related_paths = vec!["lib.rs".into()];
        let response = service
            .call_typed(NextEditServiceRequest::Plan {
                request: Box::new(request),
            })
            .unwrap();
        let NextEditServiceResponse::Candidate {
            candidate: Some(candidate),
        } = response
        else {
            panic!("expected multi-file candidate");
        };
        assert!(candidate.requires_preview);
        assert!(candidate.proposal.changes.len() >= 2);

        let valid = service
            .call_typed(NextEditServiceRequest::Validate {
                candidate_id: candidate.candidate_id.clone(),
                document_versions: candidate.expected_document_versions.clone(),
                git_head: candidate.expected_git_head.clone(),
                now_unix_ms: 1_100,
            })
            .unwrap();
        assert!(matches!(valid, NextEditServiceResponse::Valid { .. }));
    }

    #[test]
    fn stale_document_or_git_head_rejects_candidate() {
        let service = SharedNextEditService::with_config(
            AgentResourceStore::default(),
            NextEditServiceConfig {
                debounce_ms: 0,
                ..NextEditServiceConfig::default()
            },
        );
        let mut request = base_request();
        request.diagnostics = vec![NextEditDiagnosticHint {
            document: doc("file:///workspace/main.rs"),
            diagnostic: LspDiagnostic {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        character: 0,
                    },
                    end: LspPosition {
                        line: 0,
                        character: 8,
                    },
                },
                severity: Some(1),
                code: None,
                message: "missing semicolon".into(),
            },
        }];
        let response = service
            .call_typed(NextEditServiceRequest::Plan {
                request: Box::new(request),
            })
            .unwrap();
        let NextEditServiceResponse::Candidate {
            candidate: Some(candidate),
        } = response
        else {
            panic!("expected diagnostic-driven candidate");
        };

        let stale_doc = service
            .call_typed(NextEditServiceRequest::Validate {
                candidate_id: candidate.candidate_id.clone(),
                document_versions: vec![(doc("file:///workspace/main.rs"), DocumentVersion(99))],
                git_head: Some(head("abc", 1)),
                now_unix_ms: 1_100,
            })
            .unwrap();
        assert!(matches!(stale_doc, NextEditServiceResponse::Stale { .. }));

        let stale_git = service
            .call_typed(NextEditServiceRequest::Validate {
                candidate_id: candidate.candidate_id.clone(),
                document_versions: candidate.expected_document_versions.clone(),
                git_head: Some(head("def", 2)),
                now_unix_ms: 1_100,
            })
            .unwrap();
        assert!(matches!(stale_git, NextEditServiceResponse::Stale { .. }));
    }

    #[test]
    fn diagnostics_and_recent_diff_both_influence_candidates() {
        let service = SharedNextEditService::with_config(
            AgentResourceStore::default(),
            NextEditServiceConfig {
                debounce_ms: 0,
                ..NextEditServiceConfig::default()
            },
        );
        let mut diagnostic_request = base_request();
        diagnostic_request.diagnostics = vec![NextEditDiagnosticHint {
            document: doc("file:///workspace/main.rs"),
            diagnostic: LspDiagnostic {
                range: LspRange {
                    start: LspPosition {
                        line: 2,
                        character: 0,
                    },
                    end: LspPosition {
                        line: 2,
                        character: 4,
                    },
                },
                severity: Some(1),
                code: None,
                message: "unused import".into(),
            },
        }];
        let diagnostic = match service
            .call_typed(NextEditServiceRequest::Plan {
                request: Box::new(diagnostic_request),
            })
            .unwrap()
        {
            NextEditServiceResponse::Candidate {
                candidate: Some(candidate),
            } => candidate,
            other => panic!("diagnostic candidate missing: {other:?}"),
        };
        assert!(diagnostic.reason.contains("diagnostic"));

        let mut diff_request = base_request();
        diff_request.request_id = "req-diff".into();
        diff_request.generation = 3;
        diff_request.git_diff = vec![NextEditDiffHint {
            path: "main.rs".into(),
            summary: "rename helper".into(),
            details: None,
        }];
        let diff = match service
            .call_typed(NextEditServiceRequest::Plan {
                request: Box::new(diff_request),
            })
            .unwrap()
        {
            NextEditServiceResponse::Candidate {
                candidate: Some(candidate),
            } => candidate,
            other => panic!("diff candidate missing: {other:?}"),
        };
        assert!(diff.reason.contains("git_diff") || diff.reason.contains("rename"));
    }

    #[test]
    fn feedback_accept_reject_skip_are_counted() {
        let service = SharedNextEditService::new(AgentResourceStore::default());
        for (kind, id) in [
            (NextEditFeedbackKind::Accepted, "a"),
            (NextEditFeedbackKind::Rejected, "b"),
            (NextEditFeedbackKind::Skipped, "c"),
            (NextEditFeedbackKind::Accepted, "d"),
        ] {
            service
                .call_typed(NextEditServiceRequest::Feedback {
                    feedback: NextEditFeedback {
                        candidate_id: id.into(),
                        kind,
                        timestamp_unix_ms: 1,
                        reason_code: None,
                    },
                })
                .unwrap();
        }
        let stats = match service.call_typed(NextEditServiceRequest::Stats).unwrap() {
            NextEditServiceResponse::Stats { stats, .. } => stats,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(stats.accepted, 2);
        assert_eq!(stats.rejected, 1);
        assert_eq!(stats.skipped, 1);
    }

    #[test]
    fn low_confidence_timeout_and_cancel_do_not_interrupt() {
        let service = SharedNextEditService::with_config(
            AgentResourceStore::default(),
            NextEditServiceConfig {
                debounce_ms: 0,
                ..NextEditServiceConfig::default()
            },
        );
        let mut low = base_request();
        low.min_confidence = 0.99;
        low.recent_edits = vec![RecentEditEvent {
            event_id: "e-low".into(),
            document: doc("file:///workspace/main.rs"),
            version: DocumentVersion(3),
            editor_generation: 2,
            timestamp_unix_ms: 900,
            kind: mutsuki_agent_contracts::RecentEditKind::Inserted,
            range: None,
            summary: "weak signal".into(),
            byte_delta: 1,
        }];
        let low_response = service
            .call_typed(NextEditServiceRequest::Plan {
                request: Box::new(low),
            })
            .unwrap();
        assert!(matches!(
            low_response,
            NextEditServiceResponse::Candidate { candidate: None }
        ));

        let mut timed = base_request();
        timed.request_id = "req-timeout".into();
        timed.now_unix_ms = 20_000;
        timed.deadline_unix_ms = Some(10_000);
        timed.diagnostics = vec![NextEditDiagnosticHint {
            document: doc("file:///workspace/main.rs"),
            diagnostic: LspDiagnostic {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        character: 0,
                    },
                    end: LspPosition {
                        line: 0,
                        character: 1,
                    },
                },
                severity: Some(1),
                code: None,
                message: "error".into(),
            },
        }];
        assert!(matches!(
            service
                .call_typed(NextEditServiceRequest::Plan {
                    request: Box::new(timed),
                })
                .unwrap(),
            NextEditServiceResponse::TimedOut { .. }
        ));

        let cancelled = service
            .call_typed(NextEditServiceRequest::Cancel { generation: 9 })
            .unwrap();
        assert!(matches!(
            cancelled,
            NextEditServiceResponse::Cancelled { .. }
        ));

        let mut late = base_request();
        late.generation = 2;
        late.diagnostics = vec![NextEditDiagnosticHint {
            document: doc("file:///workspace/main.rs"),
            diagnostic: LspDiagnostic {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        character: 0,
                    },
                    end: LspPosition {
                        line: 0,
                        character: 1,
                    },
                },
                severity: Some(1),
                code: None,
                message: "late".into(),
            },
        }];
        assert!(matches!(
            service
                .call_typed(NextEditServiceRequest::Plan {
                    request: Box::new(late),
                })
                .unwrap(),
            NextEditServiceResponse::Superseded { .. }
        ));
    }

    #[test]
    fn plugin_descriptor_exposes_service_without_ui_or_agent_server() {
        let descriptor = SharedNextEditService::plugin_descriptor(1).unwrap();
        assert_eq!(descriptor.plugin_id, PLUGIN_ID);
        assert_eq!(descriptor.services[0].service_id, SERVICE_ID);
        assert!(
            descriptor
                .context_providers
                .contains(&CONTEXT_PROVIDER_ID.into())
        );
        assert!(
            !descriptor
                .required_capabilities
                .iter()
                .any(|cap| cap.contains("official") || cap.contains("agent-server"))
        );
    }

    #[test]
    fn performance_smoke_plan_loop() {
        let service = SharedNextEditService::with_config(
            AgentResourceStore::default(),
            NextEditServiceConfig {
                debounce_ms: 0,
                ..NextEditServiceConfig::default()
            },
        );
        let started = std::time::Instant::now();
        for index in 0..200 {
            let mut request = base_request();
            request.request_id = format!("perf-{index}");
            request.generation = (index + 2) as u64;
            request.diagnostics = vec![NextEditDiagnosticHint {
                document: doc("file:///workspace/main.rs"),
                diagnostic: LspDiagnostic {
                    range: LspRange {
                        start: LspPosition {
                            line: (index % 10) as u32,
                            character: 0,
                        },
                        end: LspPosition {
                            line: (index % 10) as u32,
                            character: 2,
                        },
                    },
                    severity: Some(1),
                    code: None,
                    message: format!("diag-{index}"),
                },
            }];
            let _ = service
                .call_typed(NextEditServiceRequest::Plan {
                    request: Box::new(request),
                })
                .unwrap();
        }
        assert!(
            started.elapsed().as_millis() < 750,
            "next-edit plan smoke exceeded budget"
        );
    }
}
