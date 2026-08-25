// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::format_push_string,
    clippy::large_stack_arrays,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mutsuki_agent_adapter_api::ModelProtocolAdapter;
use mutsuki_agent_contracts::{
    AgentError, AgentMessage, AgentModelGenerateRequest, AgentPluginStateKind,
    AgentServiceDescriptor, CodeCompletionCandidate, CodeCompletionCapabilities,
    CodeCompletionFeedback, CodeCompletionFeedbackAction, CodeCompletionHint,
    CodeCompletionLatency, CodeCompletionRequest, CodeCompletionResponse,
    CodeCompletionServiceRequest, CodeCompletionServiceResponse, CodeCompletionStatus,
    CodeCompletionStopReason, DocumentVersion, ModelGenerateRequest, ProviderInstanceDescriptor,
    TextPosition, TextSelection,
};
use mutsuki_agent_plugin_api::{AgentPluginRegistrar, AgentService};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.code-completion";
pub const SERVICE_ID: &str = "mutsuki.agent.service.code-completion";

const CACHE_CAPACITY: usize = 128;

#[derive(Clone, Debug)]
pub struct CodeCompletionConfig {
    pub capabilities: CodeCompletionCapabilities,
    pub model: String,
}

impl Default for CodeCompletionConfig {
    fn default() -> Self {
        Self {
            capabilities: CodeCompletionCapabilities::default(),
            model: "code-completion".into(),
        }
    }
}

#[derive(Clone)]
struct CacheEntry {
    response: CodeCompletionResponse,
}

struct CompletionState {
    latest_generation: u64,
    cancelled: BTreeSet<u64>,
    in_flight: u32,
    cache: BTreeMap<String, CacheEntry>,
    cache_order: VecDeque<String>,
    /// Feedback ledger stores hashes/actions only — never source bodies.
    feedback: Vec<(String, CodeCompletionFeedbackAction, String, String)>,
    /// Last observed document versions for stale rejection at return time.
    document_versions: BTreeMap<(String, String), DocumentVersion>,
}

/// Low-latency prefix/suffix completion. Calls a protocol Model Adapter directly;
/// never opens an Agent session, never executes tools, never touches Agent Server.
#[derive(Clone)]
pub struct CodeCompletionService {
    descriptor: AgentServiceDescriptor,
    adapter: Arc<dyn ModelProtocolAdapter>,
    provider: ProviderInstanceDescriptor,
    config: CodeCompletionConfig,
    next_candidate: Arc<AtomicU64>,
    inner: Arc<Mutex<CompletionState>>,
}

impl CodeCompletionService {
    pub fn new(
        adapter: Arc<dyn ModelProtocolAdapter>,
        provider: ProviderInstanceDescriptor,
        config: CodeCompletionConfig,
    ) -> Self {
        let mut capabilities = config.capabilities.clone();
        capabilities.adapter_id = Some(adapter.descriptor().adapter_id.clone());
        capabilities.provider_instance_id = Some(provider.provider_id.clone());
        capabilities.model = Some(config.model.clone());
        Self {
            descriptor: AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.code_completion.request@1".into(),
                response_schema: "mutsuki.agent.code_completion.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: None,
            },
            adapter,
            provider,
            config: CodeCompletionConfig {
                capabilities,
                model: config.model,
            },
            next_candidate: Arc::new(AtomicU64::new(0)),
            inner: Arc::new(Mutex::new(CompletionState {
                latest_generation: 0,
                cancelled: BTreeSet::new(),
                in_flight: 0,
                cache: BTreeMap::new(),
                cache_order: VecDeque::new(),
                feedback: Vec::new(),
                document_versions: BTreeMap::new(),
            })),
        }
    }

    pub fn plugin_descriptor(
        generation: u64,
    ) -> Result<mutsuki_agent_contracts::AgentKitPluginDescriptor, AgentError> {
        AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.code_completion.request@1".into(),
                response_schema: "mutsuki.agent.code_completion.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: None,
            })
            .require_service(SERVICE_ID)
            .build()
    }

    pub fn capabilities(&self) -> CodeCompletionCapabilities {
        self.config.capabilities.clone()
    }

    pub fn observe_document_version(
        &self,
        workspace_id: impl Into<String>,
        uri: impl Into<String>,
        version: DocumentVersion,
    ) {
        let mut state = self.inner.lock().expect("completion mutex");
        state
            .document_versions
            .insert((workspace_id.into(), uri.into()), version);
    }

    pub fn cancel(&self, generation: u64) {
        let mut state = self.inner.lock().expect("completion mutex");
        state.cancelled.insert(generation);
        if generation >= state.latest_generation {
            state.latest_generation = generation;
        }
    }

    pub fn latest_generation(&self) -> u64 {
        self.inner
            .lock()
            .expect("completion mutex")
            .latest_generation
    }

    pub fn feedback_len(&self) -> usize {
        self.inner.lock().expect("completion mutex").feedback.len()
    }

    pub fn record_feedback(&self, feedback: CodeCompletionFeedback) -> Result<(), AgentError> {
        if !feedback.prefix_hash.is_empty() && feedback.prefix_hash.len() < 8 {
            return Err(AgentError::invalid_input(
                "prefix_hash must be a content hash, not source text",
            ));
        }
        let mut state = self.inner.lock().expect("completion mutex");
        state.feedback.push((
            feedback.request_id.clone(),
            feedback.action,
            feedback.prefix_hash.clone(),
            feedback.suffix_hash.clone(),
        ));
        Ok(())
    }

    pub fn complete(
        &self,
        request: CodeCompletionRequest,
    ) -> Result<CodeCompletionResponse, AgentError> {
        let started = Instant::now();
        self.validate_request(&request)?;
        let cache_key = cache_key(&request);
        let deadline_ms = request
            .deadline_ms
            .unwrap_or(self.config.capabilities.total_deadline_ms)
            .min(self.config.capabilities.total_deadline_ms);
        let min_confidence = request
            .min_confidence
            .unwrap_or(self.config.capabilities.min_confidence);

        {
            let mut state = self.inner.lock().expect("completion mutex");
            if request.generation < state.latest_generation {
                return Ok(empty_response(
                    &request,
                    CodeCompletionStatus::Superseded,
                    Some(CodeCompletionStopReason::Cancelled),
                    started,
                    false,
                    &self.provider,
                    self.adapter.descriptor().adapter_id.as_str(),
                    &self.config.model,
                ));
            }
            if Self::document_is_stale(&state, &request) {
                return Ok(empty_response(
                    &request,
                    CodeCompletionStatus::StaleDocument,
                    Some(CodeCompletionStopReason::Cancelled),
                    started,
                    false,
                    &self.provider,
                    self.adapter.descriptor().adapter_id.as_str(),
                    &self.config.model,
                ));
            }
            state.latest_generation = request.generation;
            state.cancelled.remove(&request.generation);
            if self.config.capabilities.cache_enabled
                && let Some(entry) = state.cache.get(&cache_key).cloned()
            {
                let mut cached = entry.response;
                cached.request_id = request.request_id.clone();
                cached.generation = request.generation;
                cached.document_version = request.document_version;
                cached.latency = CodeCompletionLatency {
                    total_ms: started.elapsed().as_millis() as u64,
                    adapter_ms: Some(0),
                    cache_hit: true,
                };
                if Self::is_cancelled_or_superseded(&state, request.generation)
                    || Self::document_is_stale(&state, &request)
                {
                    cached.status = if Self::document_is_stale(&state, &request) {
                        CodeCompletionStatus::StaleDocument
                    } else {
                        CodeCompletionStatus::Superseded
                    };
                    cached.candidates.clear();
                    cached.stop_reason = Some(CodeCompletionStopReason::Cancelled);
                    cached.latency.cache_hit = true;
                }
                return Ok(cached);
            }
            if state.in_flight >= self.config.capabilities.max_concurrent {
                return Ok(empty_response(
                    &request,
                    CodeCompletionStatus::Empty,
                    Some(CodeCompletionStopReason::Other),
                    started,
                    false,
                    &self.provider,
                    self.adapter.descriptor().adapter_id.as_str(),
                    &self.config.model,
                ));
            }
            state.in_flight += 1;
        }

        let adapter_started = Instant::now();
        let generate_result = self.invoke_adapter(&request, deadline_ms);
        let adapter_ms = adapter_started.elapsed().as_millis() as u64;

        let mut state = self.inner.lock().expect("completion mutex");
        state.in_flight = state.in_flight.saturating_sub(1);

        if Self::is_cancelled_or_superseded(&state, request.generation) {
            return Ok(empty_response(
                &request,
                CodeCompletionStatus::Superseded,
                Some(CodeCompletionStopReason::Cancelled),
                started,
                false,
                &self.provider,
                self.adapter.descriptor().adapter_id.as_str(),
                &self.config.model,
            ));
        }

        if Self::document_is_stale(&state, &request) {
            return Ok(empty_response(
                &request,
                CodeCompletionStatus::StaleDocument,
                Some(CodeCompletionStopReason::Cancelled),
                started,
                false,
                &self.provider,
                self.adapter.descriptor().adapter_id.as_str(),
                &self.config.model,
            ));
        }

        let response = match generate_result {
            Err(error) if error.code.contains("timeout") || error.message.contains("timeout") => {
                empty_response(
                    &request,
                    CodeCompletionStatus::TimedOut,
                    Some(CodeCompletionStopReason::Timeout),
                    started,
                    false,
                    &self.provider,
                    self.adapter.descriptor().adapter_id.as_str(),
                    &self.config.model,
                )
            }
            Err(error) if error.code.contains("cancel") => empty_response(
                &request,
                CodeCompletionStatus::Cancelled,
                Some(CodeCompletionStopReason::Cancelled),
                started,
                false,
                &self.provider,
                self.adapter.descriptor().adapter_id.as_str(),
                &self.config.model,
            ),
            Err(error) => {
                return Err(error);
            }
            Ok(raw) => {
                let mut candidates =
                    parse_candidates(&request, &raw, min_confidence, &self.next_candidate);
                if candidates.is_empty() {
                    let status = if raw.confidence_below {
                        CodeCompletionStatus::LowConfidence
                    } else {
                        CodeCompletionStatus::Empty
                    };
                    let stop = if raw.confidence_below {
                        CodeCompletionStopReason::LowConfidence
                    } else {
                        CodeCompletionStopReason::Empty
                    };
                    empty_response(
                        &request,
                        status,
                        Some(stop),
                        started,
                        false,
                        &self.provider,
                        self.adapter.descriptor().adapter_id.as_str(),
                        &self.config.model,
                    )
                } else {
                    candidates.truncate(request.max_candidates as usize);
                    CodeCompletionResponse {
                        request_id: request.request_id.clone(),
                        generation: request.generation,
                        document: request.document.clone(),
                        document_version: request.document_version,
                        status: CodeCompletionStatus::Ready,
                        candidates,
                        latency: CodeCompletionLatency {
                            total_ms: started.elapsed().as_millis() as u64,
                            adapter_ms: Some(adapter_ms),
                            cache_hit: false,
                        },
                        stop_reason: Some(raw.stop_reason),
                        provider_instance_id: Some(self.provider.provider_id.clone()),
                        adapter_id: Some(self.adapter.descriptor().adapter_id.clone()),
                        model: Some(self.config.model.clone()),
                    }
                }
            }
        };

        let mut finalized = response;
        finalized.latency.adapter_ms = Some(adapter_ms);
        finalized.latency.total_ms = started.elapsed().as_millis() as u64;

        if matches!(finalized.status, CodeCompletionStatus::Ready)
            && self.config.capabilities.cache_enabled
        {
            if state.cache_order.len() >= CACHE_CAPACITY
                && let Some(old) = state.cache_order.pop_front()
            {
                state.cache.remove(&old);
            }
            state.cache_order.push_back(cache_key.clone());
            state.cache.insert(
                cache_key,
                CacheEntry {
                    response: finalized.clone(),
                },
            );
        }

        Ok(finalized)
    }

    pub fn call(
        &self,
        request: CodeCompletionServiceRequest,
    ) -> Result<CodeCompletionServiceResponse, AgentError> {
        Ok(match request {
            CodeCompletionServiceRequest::Complete { request } => {
                CodeCompletionServiceResponse::Complete {
                    response: self.complete(request)?,
                }
            }
            CodeCompletionServiceRequest::Cancel {
                generation,
                request_id: _,
            } => {
                self.cancel(generation);
                CodeCompletionServiceResponse::Cancelled { generation }
            }
            CodeCompletionServiceRequest::Feedback { feedback } => {
                let request_id = feedback.request_id.clone();
                let action = feedback.action;
                self.record_feedback(feedback)?;
                CodeCompletionServiceResponse::FeedbackAccepted { request_id, action }
            }
            CodeCompletionServiceRequest::Capabilities => {
                CodeCompletionServiceResponse::Capabilities {
                    capabilities: self.capabilities(),
                }
            }
        })
    }

    fn validate_request(&self, request: &CodeCompletionRequest) -> Result<(), AgentError> {
        if request.request_id.trim().is_empty() {
            return Err(AgentError::invalid_input("request_id is required"));
        }
        if request.workspace_id.trim().is_empty() {
            return Err(AgentError::invalid_input("workspace_id is required"));
        }
        if request.prefix.len() as u64 > self.config.capabilities.max_prefix_bytes {
            return Err(AgentError::invalid_input("prefix exceeds max_prefix_bytes"));
        }
        if request.suffix.len() as u64 > self.config.capabilities.max_suffix_bytes {
            return Err(AgentError::invalid_input("suffix exceeds max_suffix_bytes"));
        }
        let hint_bytes: u64 = request
            .nearby_hints
            .iter()
            .map(|hint| hint.summary.len() as u64)
            .sum();
        if hint_bytes > self.config.capabilities.max_hint_bytes {
            return Err(AgentError::invalid_input(
                "nearby_hints exceed max_hint_bytes",
            ));
        }
        Ok(())
    }

    fn is_cancelled_or_superseded(state: &CompletionState, generation: u64) -> bool {
        state.cancelled.contains(&generation) || generation < state.latest_generation
    }

    fn document_is_stale(state: &CompletionState, request: &CodeCompletionRequest) -> bool {
        state
            .document_versions
            .get(&(
                request.document.workspace_id.clone(),
                request.document.uri.clone(),
            ))
            .is_some_and(|current| *current != request.document_version)
    }

    fn invoke_adapter(
        &self,
        request: &CodeCompletionRequest,
        deadline_ms: u64,
    ) -> Result<RawCompletion, AgentError> {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.config.model.clone());
        let prompt = build_completion_prompt(request);
        let generate = ModelGenerateRequest {
            request: AgentModelGenerateRequest {
                model,
                messages: vec![
                    AgentMessage::system(
                        "You are a low-latency code completion engine. Return only the insert text that should appear at the cursor. Never call tools.",
                    ),
                    AgentMessage::user(prompt),
                ],
                temperature: Some(0.0),
                max_output_tokens: Some(128),
                provider_hint: request.provider_instance_id.clone(),
                metadata: Some(json!({
                    "mode": "code_completion",
                    "request_id": request.request_id,
                    "generation": request.generation,
                    "document_version": request.document_version.0,
                    // Hashes only — never embed source into ordinary logs/metadata consumers.
                    "prefix_hash": hash_text(&request.prefix),
                    "suffix_hash": hash_text(&request.suffix),
                })),
                result_protocol_id: Some(
                    mutsuki_agent_contracts::AGENT_CODE_COMPLETION_PROTOCOL.into(),
                ),
                result_context: None,
                // Explicitly no Agent session.
                session_id: None,
            },
            // Explicitly no tools.
            tools: Vec::new(),
            structured_output: None,
            reasoning: None,
        };

        if deadline_ms == 0 {
            return Err(AgentError::new(
                "agent.code_completion.timeout",
                "completion deadline is zero",
            ));
        }
        let future = self.adapter.generate(self.provider.clone(), generate);
        let adapter_started = Instant::now();
        let output = block_on_adapter(future).map_err(|error| {
            AgentError::new(
                error.code,
                format!("protocol adapter failed: {}", error.message),
            )
        });
        if adapter_started.elapsed() > Duration::from_millis(deadline_ms) {
            return Err(AgentError::new(
                "agent.code_completion.timeout",
                "completion exceeded deadline",
            ));
        }
        let result = output?;

        if !result.tool_calls.is_empty() {
            return Err(AgentError::new(
                "agent.code_completion.tool_forbidden",
                "code completion must not emit tool calls",
            ));
        }
        if result.message.content.contains("official agent server") {
            return Err(AgentError::new(
                "agent.code_completion.agent_server_forbidden",
                "code completion must not route through official Agent Server",
            ));
        }

        let confidence = result
            .raw
            .as_ref()
            .and_then(|value| value.get("confidence"))
            .and_then(Value::as_f64)
            .unwrap_or(0.85);
        Ok(RawCompletion {
            insert_text: trim_completion_text(&result.message.content),
            confidence,
            confidence_below: confidence
                < request
                    .min_confidence
                    .unwrap_or(self.config.capabilities.min_confidence),
            stop_reason: match result.stop_reason {
                mutsuki_agent_contracts::AgentModelStopReason::Length => {
                    CodeCompletionStopReason::Length
                }
                mutsuki_agent_contracts::AgentModelStopReason::ContentFilter => {
                    CodeCompletionStopReason::ContentFilter
                }
                _ => CodeCompletionStopReason::Stop,
            },
        })
    }
}

impl AgentService for CodeCompletionService {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        &self.descriptor
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        let request: CodeCompletionServiceRequest = serde_json::from_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let response = CodeCompletionService::call(self, request)?;
        serde_json::to_value(response).map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    fn drain(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        let mut state = self.inner.lock().expect("completion mutex");
        state.cache.clear();
        state.cache_order.clear();
        state.cancelled.clear();
        state.in_flight = 0;
        Ok(())
    }
}

struct RawCompletion {
    insert_text: String,
    confidence: f64,
    confidence_below: bool,
    stop_reason: CodeCompletionStopReason,
}

fn parse_candidates(
    request: &CodeCompletionRequest,
    raw: &RawCompletion,
    min_confidence: f64,
    next_candidate: &AtomicU64,
) -> Vec<CodeCompletionCandidate> {
    if raw.insert_text.is_empty() || raw.confidence < min_confidence {
        return Vec::new();
    }
    let id = next_candidate.fetch_add(1, Ordering::Relaxed) + 1;
    vec![CodeCompletionCandidate {
        candidate_id: format!("cand-{id}"),
        range: TextSelection {
            start: request.cursor,
            end: request.cursor,
        },
        insert_text: raw.insert_text.clone(),
        confidence: raw.confidence,
        stop_reason: raw.stop_reason,
    }]
}

fn empty_response(
    request: &CodeCompletionRequest,
    status: CodeCompletionStatus,
    stop_reason: Option<CodeCompletionStopReason>,
    started: Instant,
    cache_hit: bool,
    provider: &ProviderInstanceDescriptor,
    adapter_id: &str,
    model: &str,
) -> CodeCompletionResponse {
    CodeCompletionResponse {
        request_id: request.request_id.clone(),
        generation: request.generation,
        document: request.document.clone(),
        document_version: request.document_version,
        status,
        candidates: Vec::new(),
        latency: CodeCompletionLatency {
            total_ms: started.elapsed().as_millis() as u64,
            adapter_ms: None,
            cache_hit,
        },
        stop_reason,
        provider_instance_id: Some(provider.provider_id.clone()),
        adapter_id: Some(adapter_id.into()),
        model: Some(model.into()),
    }
}

fn build_completion_prompt(request: &CodeCompletionRequest) -> String {
    let mut body = String::new();
    body.push_str("Complete the code at <CURSOR>.\n");
    if let Some(language) = &request.language_id {
        body.push_str(&format!("Language: {language}\n"));
    }
    body.push_str("PREFIX:\n");
    body.push_str(&request.prefix);
    body.push_str("\n<CURSOR>\nSUFFIX:\n");
    body.push_str(&request.suffix);
    if !request.nearby_hints.is_empty() {
        body.push_str("\nHINTS:\n");
        for hint in &request.nearby_hints {
            body.push_str(&format_hint(hint));
            body.push('\n');
        }
    }
    body
}

fn format_hint(hint: &CodeCompletionHint) -> String {
    let mut parts = vec![format!("{:?}", hint.kind)];
    if let Some(path) = &hint.path {
        parts.push(format!("path={path}"));
    }
    if let Some(symbol) = &hint.symbol {
        parts.push(format!("symbol={symbol}"));
    }
    if !hint.summary.is_empty() {
        parts.push(hint.summary.clone());
    }
    parts.join(" ")
}

fn cache_key(request: &CodeCompletionRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.language_id.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(request.prefix.as_bytes());
    hasher.update(b"\0");
    hasher.update(request.suffix.as_bytes());
    hasher.update(b"\0");
    for hint in &request.nearby_hints {
        hasher.update(format_hint(hint).as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize())
}

pub fn hash_text(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn trim_completion_text(text: &str) -> String {
    text.trim_end_matches(['\r', '\n']).to_string()
}

fn block_on_adapter<T>(future: impl Future<Output = T>) -> T {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return tokio::task::block_in_place(|| handle.block_on(future));
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("code completion runtime")
        .block_on(future)
}

/// Deterministic protocol adapter for completion conformance and E2E tests.
#[derive(Clone)]
pub struct DeterministicCompletionAdapter {
    descriptor: mutsuki_agent_contracts::ModelProtocolAdapterDescriptor,
    delay_ms: u64,
    confidence: f64,
    force_empty: bool,
}

impl DeterministicCompletionAdapter {
    pub fn new() -> Self {
        Self {
            descriptor: mutsuki_agent_contracts::ModelProtocolAdapterDescriptor {
                adapter_id: "deterministic-completion".into(),
                protocol: "mutsuki.completion.deterministic".into(),
                version: "1".into(),
                runner_id: "agent.adapter.deterministic-completion".into(),
                capability: mutsuki_agent_contracts::ModelCapability {
                    context_window: 8_192,
                    code_completion: true,
                    ..mutsuki_agent_contracts::ModelCapability::default()
                },
            },
            delay_ms: 0,
            confidence: 0.92,
            force_empty: false,
        }
    }

    pub fn with_delay_ms(mut self, delay_ms: u64) -> Self {
        self.delay_ms = delay_ms;
        self
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn force_empty(mut self) -> Self {
        self.force_empty = true;
        self
    }
}

impl Default for DeterministicCompletionAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelProtocolAdapter for DeterministicCompletionAdapter {
    fn descriptor(&self) -> &mutsuki_agent_contracts::ModelProtocolAdapterDescriptor {
        &self.descriptor
    }

    fn generate(
        &self,
        _provider: ProviderInstanceDescriptor,
        request: ModelGenerateRequest,
    ) -> mutsuki_agent_adapter_api::ModelAdapterFuture {
        let delay_ms = self.delay_ms;
        let confidence = self.confidence;
        let force_empty = self.force_empty;
        Box::pin(async move {
            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            if request.request.session_id.is_some() {
                return Err(mutsuki_agent_contracts::ProtocolError {
                    code: "agent.completion.session_forbidden".into(),
                    class: mutsuki_agent_contracts::ProtocolErrorClass::Protocol,
                    message: "deterministic completion rejects session_id".into(),
                    retry_after_ms: None,
                });
            }
            if !request.tools.is_empty() {
                return Err(mutsuki_agent_contracts::ProtocolError {
                    code: "agent.completion.tool_forbidden".into(),
                    class: mutsuki_agent_contracts::ProtocolErrorClass::Protocol,
                    message: "deterministic completion rejects tools".into(),
                    retry_after_ms: None,
                });
            }
            let user = request
                .request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == mutsuki_agent_contracts::AgentRole::User)
                .map(|message| message.content.as_str())
                .unwrap_or("");
            let insert = if force_empty {
                String::new()
            } else {
                deterministic_insert(user)
            };
            Ok(mutsuki_agent_contracts::AgentModelGenerateResult {
                message: AgentMessage::assistant(insert),
                stop_reason: mutsuki_agent_contracts::AgentModelStopReason::Stop,
                tool_calls: Vec::new(),
                usage: mutsuki_agent_contracts::AgentUsage {
                    input_tokens: user.len() as u64,
                    output_tokens: 8,
                    total_tokens: user.len() as u64 + 8,
                },
                cost_microunits: 0,
                raw: Some(json!({ "confidence": confidence })),
                output_resource: None,
            })
        })
    }
}

fn deterministic_insert(prompt: &str) -> String {
    let prefix = prompt
        .split("PREFIX:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n<CURSOR>").next())
        .unwrap_or("");
    if prefix.trim_end().ends_with("fn main() {") {
        " println!(\"hi\"); }".into()
    } else if prefix.contains("let x =") && !prefix.trim_end().ends_with(';') {
        " 42;".into()
    } else if let Some(ident) = prefix.lines().last().and_then(|line| {
        line.trim()
            .strip_prefix("// complete:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }) {
        format!(" {ident}")
    } else {
        " /* done */".into()
    }
}

pub fn test_provider(endpoint: impl Into<String>) -> ProviderInstanceDescriptor {
    ProviderInstanceDescriptor {
        provider_id: "completion-provider".into(),
        adapter_id: "deterministic-completion".into(),
        endpoint: endpoint.into(),
        credential: mutsuki_agent_contracts::CredentialRef {
            credential_id: "completion-credential".into(),
            revision: 1,
        },
        models: BTreeMap::from([(
            "code-completion".into(),
            mutsuki_agent_contracts::ModelCapability {
                code_completion: true,
                context_window: 8_192,
                ..mutsuki_agent_contracts::ModelCapability::default()
            },
        )]),
        headers: BTreeMap::new(),
        compatibility: BTreeMap::from([("timeout_ms".into(), json!(1_000))]),
        remote_execution_allowed: true,
    }
}

/// Build a completion request from a fake-editor style prefix/suffix snapshot.
pub fn request_from_snapshot(
    request_id: impl Into<String>,
    generation: u64,
    workspace_id: impl Into<String>,
    document: mutsuki_agent_contracts::EditorDocumentRef,
    language_id: Option<String>,
    document_version: DocumentVersion,
    cursor: TextPosition,
    surrounding_text: (impl Into<String>, impl Into<String>),
) -> CodeCompletionRequest {
    let (prefix, suffix) = surrounding_text;
    CodeCompletionRequest {
        request_id: request_id.into(),
        generation,
        workspace_id: workspace_id.into(),
        document,
        language_id,
        document_version,
        cursor,
        prefix: prefix.into(),
        suffix: suffix.into(),
        nearby_hints: Vec::new(),
        deadline_ms: None,
        max_candidates: 3,
        min_confidence: None,
        provider_instance_id: None,
        model: None,
        credential: None,
        metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_adapter_api::{CredentialBroker, CredentialFuture, CredentialValue};
    use mutsuki_agent_adapter_openai::OpenAiCompatibleAdapter;
    use mutsuki_agent_contracts::{
        EditorDocumentRef, ModelCapability, ModelProtocolAdapterDescriptor,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;

    struct TestCredentials;

    impl CredentialBroker for TestCredentials {
        fn resolve(&self, _credential: mutsuki_agent_contracts::CredentialRef) -> CredentialFuture {
            Box::pin(async { CredentialValue::new("TEST_SECRET") })
        }
    }

    fn service_with(adapter: DeterministicCompletionAdapter) -> CodeCompletionService {
        let mut config = CodeCompletionConfig::default();
        config.capabilities.total_deadline_ms = 200;
        config.capabilities.first_result_deadline_ms = 100;
        config.capabilities.min_confidence = 0.35;
        CodeCompletionService::new(
            Arc::new(adapter),
            test_provider("memory://deterministic"),
            config,
        )
    }

    fn sample_request(generation: u64, version: u64, prefix: &str) -> CodeCompletionRequest {
        request_from_snapshot(
            format!("req-{generation}"),
            generation,
            "ws",
            EditorDocumentRef {
                workspace_id: "ws".into(),
                uri: "file:///workspace/main.rs".into(),
            },
            Some("rust".into()),
            DocumentVersion(version),
            TextPosition {
                line: 0,
                character: prefix.len() as u32,
            },
            (prefix, "\n"),
        )
    }

    #[test]
    fn deterministic_adapter_completes_prefix_suffix() {
        let service = service_with(DeterministicCompletionAdapter::new());
        let request = sample_request(1, 1, "fn main() {");
        let response = service.complete(request).unwrap();
        assert_eq!(response.status, CodeCompletionStatus::Ready);
        assert_eq!(response.candidates[0].insert_text, " println!(\"hi\"); }");
        assert!(response.may_display(DocumentVersion(1), 1));
        assert!(response.provider_instance_id.is_some());
        assert_eq!(
            response.adapter_id.as_deref(),
            Some("deterministic-completion")
        );
    }

    #[test]
    fn continuous_input_cancels_stale_generation() {
        let service = service_with(DeterministicCompletionAdapter::new().with_delay_ms(30));
        let slow = service.clone();
        let handle = thread::spawn(move || slow.complete(sample_request(1, 1, "fn main() {")));
        // Newer keystroke supersedes generation 1.
        thread::sleep(Duration::from_millis(5));
        let newer = service
            .complete(sample_request(2, 1, "fn main() { //"))
            .unwrap();
        assert!(matches!(
            newer.status,
            CodeCompletionStatus::Ready | CodeCompletionStatus::Empty
        ));
        service.cancel(1);
        let late = handle.join().unwrap().unwrap();
        assert!(
            matches!(
                late.status,
                CodeCompletionStatus::Superseded | CodeCompletionStatus::Cancelled
            ) || !late.may_display(DocumentVersion(1), service.latest_generation())
        );
        assert!(!late.may_display(DocumentVersion(1), service.latest_generation()));
    }

    #[test]
    fn document_version_change_rejects_old_candidates() {
        let service = service_with(DeterministicCompletionAdapter::new());
        let response = service
            .complete(sample_request(1, 1, "fn main() {"))
            .unwrap();
        assert!(response.may_display(DocumentVersion(1), 1));
        assert!(response.is_stale(DocumentVersion(2)));
        assert!(!response.may_display(DocumentVersion(2), 1));

        service.observe_document_version("ws", "file:///workspace/main.rs", DocumentVersion(99));
        let late = service
            .complete(sample_request(4, 1, "fn main() {"))
            .unwrap();
        assert_eq!(late.status, CodeCompletionStatus::StaleDocument);
    }

    #[test]
    fn cache_hit_timeout_and_low_confidence_paths() {
        let service = service_with(DeterministicCompletionAdapter::new());
        let first = service
            .complete(sample_request(1, 1, "fn main() {"))
            .unwrap();
        assert!(!first.latency.cache_hit);
        let second = service
            .complete(sample_request(2, 1, "fn main() {"))
            .unwrap();
        assert!(second.latency.cache_hit);
        assert_eq!(second.status, CodeCompletionStatus::Ready);

        let timeout_service =
            service_with(DeterministicCompletionAdapter::new().with_delay_ms(250));
        let timed = timeout_service
            .complete({
                let mut request = sample_request(1, 1, "let x =");
                request.deadline_ms = Some(20);
                request
            })
            .unwrap();
        assert_eq!(timed.status, CodeCompletionStatus::TimedOut);

        let low = service_with(DeterministicCompletionAdapter::new().with_confidence(0.1));
        let muted = low.complete(sample_request(1, 1, "fn main() {")).unwrap();
        assert_eq!(muted.status, CodeCompletionStatus::LowConfidence);
        assert!(muted.candidates.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn openai_compatible_provider_completes_inline_without_session_or_tools() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 32_768];
            let read = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.contains("/v1/chat/completions"));
            assert!(!request.contains("\"tools\""));
            assert!(!request.contains("\"session_id\""));
            assert!(request.contains("code_completion") || request.contains("PREFIX"));
            let payload = r#"{"choices":[{"message":{"role":"assistant","content":" 1;"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#;
            let body = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            stream.write_all(body.as_bytes()).unwrap();
        });

        let adapter = OpenAiCompatibleAdapter::new(
            ModelProtocolAdapterDescriptor {
                adapter_id: "openai-compatible".into(),
                protocol: "openai.chat-completions".into(),
                version: "1".into(),
                runner_id: "agent.adapter.openai-compatible".into(),
                capability: ModelCapability {
                    context_window: 128_000,
                    code_completion: true,
                    streaming: true,
                    ..ModelCapability::default()
                },
            },
            Arc::new(TestCredentials),
        )
        .unwrap();
        let mut provider = test_provider(format!("http://{address}/v1/chat/completions"));
        provider.adapter_id = "openai-compatible".into();
        let mut config = CodeCompletionConfig::default();
        config.capabilities.total_deadline_ms = 2_000;
        config.model = "model".into();
        let service = CodeCompletionService::new(Arc::new(adapter), provider, config);
        let response = service
            .complete(sample_request(1, 1, "let answer ="))
            .unwrap();
        assert_eq!(response.status, CodeCompletionStatus::Ready);
        assert_eq!(response.candidates[0].insert_text, " 1;");
        assert_eq!(response.adapter_id.as_deref(), Some("openai-compatible"));
        server.join().unwrap();
    }

    #[test]
    fn feedback_stores_hashes_not_source_bodies() {
        let service = service_with(DeterministicCompletionAdapter::new());
        let request = sample_request(1, 1, "fn main() {");
        let response = service.complete(request.clone()).unwrap();
        service
            .record_feedback(CodeCompletionFeedback {
                request_id: response.request_id.clone(),
                generation: response.generation,
                candidate_id: response.candidates.first().map(|c| c.candidate_id.clone()),
                action: CodeCompletionFeedbackAction::Accept,
                document: request.document.clone(),
                document_version: request.document_version,
                language_id: request.language_id.clone(),
                prefix_hash: hash_text(&request.prefix),
                suffix_hash: hash_text(&request.suffix),
            })
            .unwrap();
        assert_eq!(service.feedback_len(), 1);
        let ledger = service.inner.lock().unwrap();
        assert!(!ledger.feedback[0].2.contains("fn main"));
        assert!(ledger.feedback[0].2.starts_with("sha256:"));
    }

    #[test]
    fn service_contract_round_trip_without_agent_session() {
        let service = service_with(DeterministicCompletionAdapter::new());
        let response = service
            .call(CodeCompletionServiceRequest::Complete {
                request: sample_request(1, 1, "fn main() {"),
            })
            .unwrap();
        match response {
            CodeCompletionServiceResponse::Complete { response } => {
                assert_eq!(response.status, CodeCompletionStatus::Ready);
            }
            other => panic!("unexpected {other:?}"),
        }
        let caps = service
            .call(CodeCompletionServiceRequest::Capabilities)
            .unwrap();
        assert!(matches!(
            caps,
            CodeCompletionServiceResponse::Capabilities { .. }
        ));
    }

    #[test]
    fn performance_smoke_prefix_hash_cache_loop() {
        let service = service_with(DeterministicCompletionAdapter::new());
        let started = Instant::now();
        for index in 0..200 {
            let prefix = if index % 7 == 0 {
                format!("fn main() {{ // {index}")
            } else {
                "fn main() {".into()
            };
            let _ = service
                .complete(sample_request(index as u64 + 1, 1, &prefix))
                .unwrap();
        }
        assert!(
            started.elapsed().as_millis() < 750,
            "completion cache smoke exceeded budget"
        );
    }
}
