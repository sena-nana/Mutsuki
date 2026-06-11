use std::collections::BTreeMap;
use std::env;

use mutsukicore_runtime_contracts::{
    ERR_OPERATION_NOT_FOUND, ERR_RUNTIME_BACKEND_FAILED, Envelope, OperationHandlerKey,
    OperationSnapshot, OperationStatus, PluginDescriptor, PluginSnapshot, PluginStatus,
    RuntimeError, ScalarValue, SourceDescriptor, SourceSnapshot, StrategyResult,
    StrategyResultStatus,
};
use mutsukicore_runtime_core::{
    BackendPayload, OperationBackend, RuntimeFailure, RuntimeResult, StrategyBackend,
};
use reqwest::blocking::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const PLUGIN_ID: &str = "mutsukicore-claude-core";
pub const DEFAULT_SOURCE_ID: &str = "claude:local";
pub const DEFAULT_SOURCE_KIND: &str = "claude.strategy";
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
pub const DEFAULT_MAX_TOKENS: u64 = 4096;
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeConfig {
    pub api_key: Option<String>,
    pub model: String,
    pub api_url: String,
    pub max_tokens: u64,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            api_key: env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty()),
            model: env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
            api_url: env::var("ANTHROPIC_API_URL").unwrap_or_else(|_| ANTHROPIC_API_URL.into()),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeProbeStatus {
    pub available: bool,
    pub model: String,
    pub api_url: String,
    pub failure_kind: Option<String>,
    pub issues: Vec<String>,
}

pub fn build_claude_probe_status(config: &ClaudeConfig) -> ClaudeProbeStatus {
    let api_key_available = config
        .api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());
    ClaudeProbeStatus {
        available: api_key_available,
        model: config.model.clone(),
        api_url: config.api_url.clone(),
        failure_kind: (!api_key_available).then(|| "missingApiKey".into()),
        issues: if api_key_available {
            Vec::new()
        } else {
            vec!["ANTHROPIC_API_KEY was not found".into()]
        },
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeRequest {
    pub model: String,
    pub max_tokens: u64,
    pub messages: Vec<ClaudeMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub content: Vec<ClaudeContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClaudeContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(other)]
    Unknown,
}

pub trait ClaudeClient {
    fn complete(&mut self, request: ClaudeRequest) -> RuntimeResult<ClaudeResponse>;
}

#[derive(Debug)]
pub struct ClaudeHttpClient {
    http: HttpClient,
    config: ClaudeConfig,
}

impl ClaudeHttpClient {
    pub fn new(config: ClaudeConfig) -> Self {
        Self {
            http: HttpClient::new(),
            config,
        }
    }

    pub fn from_env() -> Self {
        Self::new(ClaudeConfig::default())
    }
}

impl ClaudeClient for ClaudeHttpClient {
    fn complete(&mut self, request: ClaudeRequest) -> RuntimeResult<ClaudeResponse> {
        let api_key = self
            .config
            .api_key
            .as_deref()
            .ok_or_else(|| runtime_failure("claude.api.config", "reason", "missing_api_key"))?;
        let response = self
            .http
            .post(&self.config.api_url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&request)
            .send()
            .map_err(|err| runtime_failure("claude.api.request", "exception_repr", err))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            let mut failure = runtime_failure("claude.api.status", "status", status.as_u16());
            failure.0.evidence.insert(
                "body".into(),
                ScalarValue::String(truncate_evidence(&body, 1200)),
            );
            return Err(failure);
        }
        response
            .json::<ClaudeResponse>()
            .map_err(|err| runtime_failure("claude.output.decode", "exception_repr", err))
    }
}

#[derive(Debug)]
pub struct ClaudeStrategyBackend<C> {
    client: C,
    config: ClaudeConfig,
    source_snapshot: SourceSnapshot,
    sessions: BTreeMap<String, ClaudeAgentSession>,
}

#[derive(Clone, Debug, Default)]
struct ClaudeAgentSession {
    history: Vec<ClaudeMessage>,
}

impl<C> ClaudeStrategyBackend<C> {
    pub fn new(client: C, config: ClaudeConfig) -> Self {
        Self {
            client,
            config,
            source_snapshot: claude_source_snapshot(DEFAULT_SOURCE_ID, DEFAULT_SOURCE_KIND),
            sessions: BTreeMap::new(),
        }
    }

    pub fn with_source(mut self, source_id: &str, kind: &str) -> Self {
        self.source_snapshot = claude_source_snapshot(source_id, kind);
        self
    }

    pub fn source_snapshot(&self) -> &SourceSnapshot {
        &self.source_snapshot
    }
}

impl ClaudeStrategyBackend<ClaudeHttpClient> {
    pub fn from_env() -> Self {
        let config = ClaudeConfig::default();
        Self::new(ClaudeHttpClient::new(config.clone()), config)
    }
}

impl<C: ClaudeClient> StrategyBackend for ClaudeStrategyBackend<C> {
    fn on_awake(&mut self, agent_id: &str) -> RuntimeResult<()> {
        self.sessions.entry(agent_id.to_string()).or_default();
        Ok(())
    }

    fn on_input(&mut self, agent_id: &str, envelope: &Envelope) -> RuntimeResult<StrategyResult> {
        let input = claude_input_from_envelope(envelope, &self.config)?;
        let session = self.sessions.entry(agent_id.to_string()).or_default();
        session.history.push(ClaudeMessage {
            role: "user".into(),
            content: input.prompt.clone(),
        });
        let request = input.into_request(session);
        let response = self.client.complete(request)?;
        let result = strategy_result_from_response(response);
        if let Some(text) = result
            .decision
            .as_ref()
            .and_then(|decision| decision.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        {
            session.history.push(ClaudeMessage {
                role: "assistant".into(),
                content: text.to_string(),
            });
        }
        Ok(result)
    }

    fn next_step(&mut self, _agent_id: &str) -> RuntimeResult<StrategyResult> {
        Ok(StrategyResult::wait_input())
    }

    fn on_stop(&mut self, agent_id: &str) -> RuntimeResult<()> {
        self.sessions.remove(agent_id);
        Ok(())
    }
}

impl<C> OperationBackend for ClaudeStrategyBackend<C> {
    fn list_plugins(&self) -> RuntimeResult<Vec<PluginSnapshot>> {
        Ok(vec![plugin_snapshot()])
    }

    fn list_operations(
        &self,
        _enabled_plugin_ids: &[String],
    ) -> RuntimeResult<Vec<OperationSnapshot>> {
        Ok(Vec::new())
    }

    fn list_sources(&self, enabled_plugin_ids: &[String]) -> RuntimeResult<Vec<SourceSnapshot>> {
        if enabled_plugin_ids.iter().any(|plugin_id| plugin_id == PLUGIN_ID) {
            Ok(vec![self.source_snapshot.clone()])
        } else {
            Ok(Vec::new())
        }
    }

    fn invoke(
        &mut self,
        _agent_id: &str,
        _key: &OperationHandlerKey,
        _payload: Value,
    ) -> RuntimeResult<BackendPayload> {
        Err(RuntimeFailure::new(RuntimeError::new(
            ERR_OPERATION_NOT_FOUND,
            PLUGIN_ID,
            "claude.operation.invoke",
        )))
    }

    fn operation_status(&self, _agent_id: &str, _key: &OperationHandlerKey) -> OperationStatus {
        OperationStatus::NotFound
    }
}

fn plugin_snapshot() -> PluginSnapshot {
    PluginSnapshot {
        descriptor: PluginDescriptor {
            plugin_id: PLUGIN_ID.into(),
            generation: 0,
            name: "MutsukiCore Claude Core".into(),
            description: "Claude StrategyBackend for MutsukiCore runtime".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec!["strategy".into()],
            metadata: BTreeMap::new(),
        },
        status: PluginStatus::Enabled,
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ClaudeInput {
    prompt: String,
    model: String,
    max_tokens: u64,
    system: Option<String>,
    temperature: Option<f64>,
    cwd: Option<String>,
}

impl ClaudeInput {
    fn into_request(self, session: &ClaudeAgentSession) -> ClaudeRequest {
        let mut metadata = Map::new();
        if let Some(cwd) = self.cwd {
            metadata.insert("cwd".into(), json!(cwd));
        }
        ClaudeRequest {
            model: self.model,
            max_tokens: self.max_tokens,
            messages: session.history.clone(),
            system: self.system,
            temperature: self.temperature,
            metadata: (!metadata.is_empty()).then(|| Value::Object(metadata)),
        }
    }
}

pub fn claude_source_snapshot(source_id: &str, kind: &str) -> SourceSnapshot {
    SourceSnapshot {
        descriptor: SourceDescriptor {
            source_id: source_id.into(),
            kind: kind.into(),
            capabilities: vec!["strategy".into()],
            description: "Local Claude strategy backend source".into(),
        },
        plugin_id: PLUGIN_ID.into(),
        plugin_generation: 0,
    }
}

pub fn strategy_result_from_response(response: ClaudeResponse) -> StrategyResult {
    let tool_uses = response
        .content
        .iter()
        .filter_map(|block| match block {
            ClaudeContentBlock::ToolUse { id, name, input } => Some(json!({
                "id": id,
                "name": name,
                "input": input,
            })),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !tool_uses.is_empty() {
        return StrategyResult {
            status: StrategyResultStatus::WaitInput,
            decision: Some(json!({
                "kind": "claude_interaction_request",
                "interaction": "tool_use",
                "toolUses": tool_uses,
                "stopReason": response.stop_reason,
                "model": response.model,
                "usage": response.usage,
            })),
            emitted: Vec::new(),
            error: None,
        };
    }

    let text = response
        .content
        .iter()
        .filter_map(|block| match block {
            ClaudeContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        return StrategyResult {
            status: StrategyResultStatus::Failed,
            decision: None,
            emitted: Vec::new(),
            error: Some(runtime_error_with_evidence(
                "claude.output.empty",
                "reason",
                "empty_text",
            )),
        };
    }
    StrategyResult {
        status: StrategyResultStatus::WaitInput,
        decision: Some(json!({
            "kind": "claude_message",
            "text": text,
            "stopReason": response.stop_reason,
            "model": response.model,
            "usage": response.usage,
        })),
        emitted: Vec::new(),
        error: None,
    }
}

fn claude_input_from_envelope(
    envelope: &Envelope,
    config: &ClaudeConfig,
) -> RuntimeResult<ClaudeInput> {
    let prompt = envelope
        .payload
        .get("prompt")
        .or_else(|| envelope.payload.get("text"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| runtime_failure("claude.input", "reason", "missing prompt"))?;
    let settings = envelope.payload.get("settings").and_then(Value::as_object);
    let model = settings
        .and_then(|item| item.get("model"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.model)
        .to_string();
    let max_tokens = settings
        .and_then(|item| item.get("maxTokens").or_else(|| item.get("max_tokens")))
        .and_then(Value::as_u64)
        .unwrap_or(config.max_tokens);
    let system = settings
        .and_then(|item| item.get("systemPrompt").or_else(|| item.get("system")))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let temperature = settings
        .and_then(|item| item.get("temperature"))
        .and_then(Value::as_f64);
    let cwd = envelope
        .payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    Ok(ClaudeInput {
        prompt,
        model,
        max_tokens,
        system,
        temperature,
        cwd,
    })
}

fn runtime_failure(
    route: &str,
    evidence_key: &str,
    evidence_value: impl ToString,
) -> RuntimeFailure {
    RuntimeFailure::new(runtime_error_with_evidence(
        route,
        evidence_key,
        evidence_value,
    ))
}

fn runtime_error_with_evidence(
    route: &str,
    evidence_key: &str,
    evidence_value: impl ToString,
) -> RuntimeError {
    let mut err = RuntimeError::new(ERR_RUNTIME_BACKEND_FAILED, PLUGIN_ID, route);
    err.evidence.insert(
        evidence_key.into(),
        ScalarValue::String(evidence_value.to_string()),
    );
    err
}

fn truncate_evidence(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use mutsukicore_runtime_contracts::{
        AgentParticipation, AgentPhase, AgentSpec, ScopeRuleSpec, SideEffectPolicy, SourceRef,
    };
    use mutsukicore_runtime_core::AgentRuntime;
    use serde_json::json;

    use super::*;

    #[derive(Debug)]
    struct StubClaudeClient {
        result: StubClaudeResult,
        requests: Vec<ClaudeRequest>,
    }

    #[derive(Clone, Debug)]
    enum StubClaudeResult {
        Ok(ClaudeResponse),
        Failed(String),
    }

    impl StubClaudeClient {
        fn ok(response: ClaudeResponse) -> Self {
            Self {
                result: StubClaudeResult::Ok(response),
                requests: Vec::new(),
            }
        }

        fn failed(route: &str) -> Self {
            Self {
                result: StubClaudeResult::Failed(route.into()),
                requests: Vec::new(),
            }
        }
    }

    impl ClaudeClient for StubClaudeClient {
        fn complete(&mut self, request: ClaudeRequest) -> RuntimeResult<ClaudeResponse> {
            self.requests.push(request);
            match &self.result {
                StubClaudeResult::Ok(response) => Ok(response.clone()),
                StubClaudeResult::Failed(route) => Err(runtime_failure(route, "reason", "stub")),
            }
        }
    }

    fn config() -> ClaudeConfig {
        ClaudeConfig {
            api_key: Some("test-key".into()),
            model: "claude-test".into(),
            api_url: "https://example.invalid/messages".into(),
            max_tokens: 128,
        }
    }

    fn text_response(text: &str) -> ClaudeResponse {
        ClaudeResponse {
            id: Some("msg-1".into()),
            content: vec![ClaudeContentBlock::Text { text: text.into() }],
            stop_reason: Some("end_turn".into()),
            model: Some("claude-test".into()),
            usage: Some(json!({"input_tokens": 1, "output_tokens": 2})),
        }
    }

    fn tool_response() -> ClaudeResponse {
        ClaudeResponse {
            id: Some("msg-tool".into()),
            content: vec![ClaudeContentBlock::ToolUse {
                id: "toolu-1".into(),
                name: "Bash".into(),
                input: json!({"command": "pwd"}),
            }],
            stop_reason: Some("tool_use".into()),
            model: Some("claude-test".into()),
            usage: None,
        }
    }

    fn envelope(payload: Value) -> Envelope {
        Envelope {
            id: "env-1".into(),
            timestamp: 1.0,
            source: SourceRef {
                source_id: DEFAULT_SOURCE_ID.into(),
                kind: DEFAULT_SOURCE_KIND.into(),
                metadata: BTreeMap::new(),
            },
            payload_schema_id: "claude.input".into(),
            capabilities_required: Vec::new(),
            payload,
        }
    }

    fn agent() -> AgentSpec {
        AgentSpec {
            agent_id: "claude-agent".into(),
            owner: None,
            priority: 0,
            participation: AgentParticipation::PrimaryCandidate,
            accepts: vec![ScopeRuleSpec::BySourceId {
                source_id: DEFAULT_SOURCE_ID.into(),
            }],
            strategy_id: PLUGIN_ID.into(),
            side_effect_policy: SideEffectPolicy::ReadOnly,
        }
    }

    #[test]
    fn source_snapshot_registers_default_claude_source() {
        let snapshot = claude_source_snapshot(DEFAULT_SOURCE_ID, DEFAULT_SOURCE_KIND);

        assert_eq!(snapshot.descriptor.source_id, "claude:local");
        assert_eq!(snapshot.descriptor.kind, "claude.strategy");
        assert_eq!(snapshot.plugin_id, PLUGIN_ID);
    }

    #[test]
    fn operation_backend_lists_plugin_and_filters_source_by_enabled_plugins() {
        let backend = ClaudeStrategyBackend::new(StubClaudeClient::ok(text_response("ok")), config());

        assert_eq!(
            backend.list_plugins().unwrap()[0].descriptor.plugin_id,
            PLUGIN_ID
        );
        assert!(
            backend
                .list_sources(&["other-plugin".to_string()])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            backend.list_sources(&[PLUGIN_ID.to_string()]).unwrap()[0].plugin_id,
            PLUGIN_ID
        );
    }

    #[test]
    fn probe_reports_missing_api_key_without_network() {
        let mut cfg = config();
        cfg.api_key = None;

        let status = build_claude_probe_status(&cfg);

        assert!(!status.available);
        assert_eq!(status.failure_kind.as_deref(), Some("missingApiKey"));
    }

    #[test]
    fn missing_prompt_fails_loud() {
        let mut backend =
            ClaudeStrategyBackend::new(StubClaudeClient::ok(text_response("ok")), config());

        let err = backend
            .on_input("agent-a", &envelope(json!({"settings": {}})))
            .unwrap_err();

        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(err.error().route, "claude.input");
    }

    #[test]
    fn text_response_maps_to_wait_input_message_decision() {
        let mut backend =
            ClaudeStrategyBackend::new(StubClaudeClient::ok(text_response("hello")), config());

        let result = backend
            .on_input(
                "agent-a",
                &envelope(json!({
                    "text": "decide",
                    "cwd": "C:\\repo",
                    "settings": {
                        "model": "claude-custom",
                        "systemPrompt": "system",
                        "maxTokens": 64,
                        "temperature": 0.2
                    }
                })),
            )
            .unwrap();

        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        let decision = result.decision.unwrap();
        assert_eq!(decision["kind"], "claude_message");
        assert_eq!(decision["text"], "hello");
        let request = &backend.client.requests[0];
        assert_eq!(request.model, "claude-custom");
        assert_eq!(request.max_tokens, 64);
        assert_eq!(request.system.as_deref(), Some("system"));
        assert_eq!(request.metadata.as_ref().unwrap()["cwd"], "C:\\repo");
    }

    #[test]
    fn tool_response_maps_to_interaction_request_decision() {
        let mut backend =
            ClaudeStrategyBackend::new(StubClaudeClient::ok(tool_response()), config());

        let result = backend
            .on_input("agent-a", &envelope(json!({"prompt": "use tool"})))
            .unwrap();

        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        let decision = result.decision.unwrap();
        assert_eq!(decision["kind"], "claude_interaction_request");
        assert_eq!(decision["interaction"], "tool_use");
        assert_eq!(decision["toolUses"][0]["name"], "Bash");
    }

    #[test]
    fn client_error_surfaces_as_runtime_backend_failed() {
        let mut backend =
            ClaudeStrategyBackend::new(StubClaudeClient::failed("claude.api.status"), config());

        let err = backend
            .on_input("agent-a", &envelope(json!({"prompt": "fail"})))
            .unwrap_err();

        assert_eq!(err.error().code, ERR_RUNTIME_BACKEND_FAILED);
        assert_eq!(err.error().route, "claude.api.status");
    }

    #[test]
    fn empty_response_returns_failed_strategy_result() {
        let mut backend =
            ClaudeStrategyBackend::new(StubClaudeClient::ok(text_response("")), config());

        let result = backend
            .on_input("agent-a", &envelope(json!({"prompt": "empty"})))
            .unwrap();

        assert_eq!(result.status, StrategyResultStatus::Failed);
        assert_eq!(result.error.unwrap().route, "claude.output.empty");
    }

    #[test]
    fn smoke_runtime_routes_claude_agent() {
        let mut backend =
            ClaudeStrategyBackend::new(StubClaudeClient::ok(text_response("ok")), config());
        let mut runtime = AgentRuntime::new();

        runtime.register_agent(agent()).unwrap();
        runtime.start_agent("claude-agent", &mut backend).unwrap();
        assert_eq!(runtime.phase("claude-agent"), Some(&AgentPhase::Awake));
        assert_eq!(
            runtime
                .publish(envelope(json!({"prompt": "hello"})))
                .unwrap(),
            vec!["claude-agent"]
        );

        let result = runtime.tick_once("claude-agent", &mut backend).unwrap();

        assert_eq!(result.status, StrategyResultStatus::WaitInput);
        assert_eq!(result.decision.unwrap()["kind"], "claude_message");
        assert!(
            runtime
                .events()
                .iter()
                .any(|event| event.name == "agent.input")
        );
    }

    #[test]
    fn runtime_records_failed_strategy_result_event_error() {
        let mut backend =
            ClaudeStrategyBackend::new(StubClaudeClient::ok(text_response("")), config());
        let mut runtime = AgentRuntime::new();

        runtime.register_agent(agent()).unwrap();
        runtime.start_agent("claude-agent", &mut backend).unwrap();
        runtime
            .publish(envelope(json!({"prompt": "hello"})))
            .unwrap();
        let result = runtime.tick_once("claude-agent", &mut backend).unwrap();

        assert_eq!(result.status, StrategyResultStatus::Failed);
        let event = runtime
            .events()
            .into_iter()
            .find(|event| event.name == "agent.input.error")
            .unwrap();
        assert_eq!(event.error.unwrap().route, "claude.output.empty");
    }
}
