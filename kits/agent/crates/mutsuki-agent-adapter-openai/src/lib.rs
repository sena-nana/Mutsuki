use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use mutsuki_agent_adapter_api::{
    CredentialBroker, ModelAdapterFuture, ModelProtocolAdapter, ModelStreamFuture,
};
use mutsuki_agent_contracts::{
    AgentContentPart, AgentMessage, AgentModelGenerateResult, AgentModelStopReason, AgentRole,
    AgentToolCall, AgentUsage, ModelGenerateRequest, ModelProtocolAdapterDescriptor,
    ModelStreamEvent, ProtocolError, ProtocolErrorClass, ProviderInstanceDescriptor,
};
use mutsuki_agent_sdk::{AgentModelGenerateProtocol, orchestration_runner};
use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};

use mutsuki_runtime_core::{AsyncBatchHandler, AsyncCompletionFuture, RunnerContext};
use mutsuki_runtime_sdk::contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, InvocationMode, RunnerBatchCapability,
    RunnerConcurrency, RunnerMode, RunnerResult, RunnerSideEffect, WorkBatch,
};

mod media;
mod responses;
pub use media::*;
pub use responses::{
    ADAPTER_ID as RESPONSES_ADAPTER_ID, OpenAiResponsesAdapter, PROTOCOL as RESPONSES_PROTOCOL,
    RUNNER_ID as RESPONSES_RUNNER_ID,
};

pub const ADAPTER_ID: &str = "openai-compatible";
pub const PROTOCOL: &str = "openai.chat-completions";
pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.adapter.openai-compatible";
pub const RUNNER_ID: &str = "mutsuki.agent.adapter.openai-compatible.runner";

#[derive(Clone)]
pub struct OpenAiCompatibleAdapter {
    descriptor: ModelProtocolAdapterDescriptor,
    credentials: Arc<dyn CredentialBroker>,
    client: Client,
}

impl OpenAiCompatibleAdapter {
    pub fn new(
        descriptor: ModelProtocolAdapterDescriptor,
        credentials: Arc<dyn CredentialBroker>,
    ) -> Result<Self, ProtocolError> {
        if descriptor.adapter_id.trim().is_empty() || descriptor.runner_id.trim().is_empty() {
            return Err(error(
                "agent.adapter.invalid_config",
                ProtocolErrorClass::NonRetryable,
                "adapter and runner ids are required",
            ));
        }
        install_crypto_provider();
        let client = Client::builder()
            .build()
            .map_err(|err| transport_error(&err))?;
        Ok(Self {
            descriptor,
            credentials,
            client,
        })
    }

    pub fn default_descriptor() -> ModelProtocolAdapterDescriptor {
        ModelProtocolAdapterDescriptor {
            adapter_id: ADAPTER_ID.into(),
            protocol: PROTOCOL.into(),
            version: "1".into(),
            runner_id: RUNNER_ID.into(),
            capability: mutsuki_agent_contracts::ModelCapability {
                context_window: 128_000,
                streaming: true,
                tools: true,
                structured_output: true,
                ..mutsuki_agent_contracts::ModelCapability::default()
            },
        }
    }

    async fn request(
        &self,
        provider: ProviderInstanceDescriptor,
        request: ModelGenerateRequest,
        stream: bool,
    ) -> Result<Value, ProtocolError> {
        let endpoint = endpoint(&provider)?;
        let credential = self
            .credentials
            .resolve(provider.credential.clone())
            .await?;
        let timeout_ms = provider
            .compatibility
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(30_000);
        let retries = provider
            .compatibility
            .get("max_retries")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .min(3);
        let payload = payload(request, stream);
        for attempt in 0..=retries {
            let mut builder = self
                .client
                .post(endpoint.clone())
                .bearer_auth(credential.expose_secret())
                .timeout(Duration::from_millis(timeout_ms))
                .json(&payload);
            for (name, value) in &provider.headers {
                builder = builder.header(name, value);
            }
            match builder.send().await {
                Ok(response) if response.status().is_success() => {
                    if stream {
                        let text = response.text().await.map_err(|err| transport_error(&err))?;
                        return Ok(Value::String(text));
                    }
                    return response.json().await.map_err(|_| {
                        error(
                            "agent.adapter.invalid_response",
                            ProtocolErrorClass::Protocol,
                            "response body is not valid JSON",
                        )
                    });
                }
                Ok(response) if retryable_status(response.status()) && attempt < retries => {
                    continue;
                }
                Ok(response) => return Err(status_error(response.status())),
                Err(err) if (err.is_timeout() || err.is_connect()) && attempt < retries => {
                    continue;
                }
                Err(err) => return Err(transport_error(&err)),
            }
        }
        Err(error(
            "agent.adapter.retry_exhausted",
            ProtocolErrorClass::Retryable,
            "model request retry budget was exhausted",
        ))
    }
}

pub(crate) fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

impl ModelProtocolAdapter for OpenAiCompatibleAdapter {
    fn descriptor(&self) -> &ModelProtocolAdapterDescriptor {
        &self.descriptor
    }

    fn generate(
        &self,
        provider: ProviderInstanceDescriptor,
        request: ModelGenerateRequest,
    ) -> ModelAdapterFuture {
        let adapter = self.clone();
        Box::pin(async move {
            let body = adapter.request(provider, request, false).await?;
            parse_response(body)
        })
    }

    fn stream(
        &self,
        provider: ProviderInstanceDescriptor,
        request: ModelGenerateRequest,
    ) -> ModelStreamFuture {
        let adapter = self.clone();
        Box::pin(async move {
            let body = adapter.request(provider, request, true).await?;
            parse_sse(body.as_str().unwrap_or_default())
        })
    }
}

pub struct OpenAiAdapterAsyncHandler {
    descriptor: mutsuki_runtime_sdk::contracts::RunnerDescriptor,
    adapter: Arc<OpenAiCompatibleAdapter>,
    providers: Vec<ProviderInstanceDescriptor>,
}

impl OpenAiAdapterAsyncHandler {
    pub fn new(
        adapter: Arc<OpenAiCompatibleAdapter>,
        providers: Vec<ProviderInstanceDescriptor>,
    ) -> Result<Self, ProtocolError> {
        if providers.is_empty()
            || providers
                .iter()
                .any(|provider| provider.adapter_id != adapter.descriptor.adapter_id)
        {
            return Err(error(
                "agent.adapter.invalid_config",
                ProtocolErrorClass::NonRetryable,
                "runner providers must be non-empty and owned by this adapter",
            ));
        }
        Ok(Self {
            descriptor: orchestration_runner(RUNNER_ID, PLUGIN_ID)
                .accepts::<AgentModelGenerateProtocol>()
                .execution_class(ExecutionClass::Io)
                .invocation_mode(InvocationMode::AsyncReentrant)
                .concurrency(RunnerConcurrency::Reentrant {
                    max_inflight_batches: 32,
                    max_inflight_entries: 64,
                })
                .batch_capability(RunnerBatchCapability {
                    mode: RunnerMode::NativeBatch,
                    preferred_batch_size: 8,
                    max_batch_entries: 64,
                    max_inflight_batches: 32,
                    max_entry_concurrency: 64,
                    side_effect: RunnerSideEffect::External,
                    ..RunnerBatchCapability::default()
                })
                .build(),
            adapter,
            providers,
        })
    }
}

impl AsyncBatchHandler for OpenAiAdapterAsyncHandler {
    fn descriptor(&self) -> &mutsuki_runtime_sdk::contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, _ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        let adapter = self.adapter.clone();
        let providers = self.providers.clone();
        Box::pin(async move {
            let tasks = match batch.row_payload_tasks() {
                Ok(tasks) => tasks,
                Err(err) => return Ok(CompletionBatch::from_error(&batch, err)),
            };
            let requests = batch.entries.iter().map(|entry| {
                let adapter = adapter.clone();
                let providers = providers.clone();
                let task = tasks
                    .iter()
                    .find(|task| task.task_id == entry.task_id)
                    .expect("validated row payload contains every entry")
                    .clone();
                let entry_id = entry.entry_id.clone();
                async move {
                    let decoded = serde_json::from_value::<
                        mutsuki_agent_contracts::AgentModelGenerateRequest,
                    >(task.payload.clone().into());
                    let completion = match decoded {
                        Ok(request) => {
                            let provider = request
                                .provider_hint
                                .as_ref()
                                .and_then(|hint| {
                                    providers
                                        .iter()
                                        .find(|provider| provider.provider_id == *hint)
                                })
                                .or_else(|| providers.first())
                                .cloned();
                            match provider {
                                Some(provider) => adapter
                                    .generate(
                                        provider,
                                        ModelGenerateRequest {
                                            request,
                                            tools: Vec::new(),
                                            structured_output: None,
                                            reasoning: None,
                                        },
                                    )
                                    .await
                                    .map(|output| {
                                        let mut result =
                                            RunnerResult::completed(task.task_id.clone());
                                        result.output = serde_json::to_value(output).ok();
                                        result
                                    })
                                    .map_err(|err| runtime_error(&task.task_id, err)),
                                None => Err(runtime_error(
                                    &task.task_id,
                                    error(
                                        "agent.adapter.provider_unavailable",
                                        ProtocolErrorClass::NonRetryable,
                                        "no provider is configured for the adapter runner",
                                    ),
                                )),
                            }
                        }
                        Err(_) => Err(runtime_error(
                            &task.task_id,
                            error(
                                "agent.adapter.invalid_request",
                                ProtocolErrorClass::NonRetryable,
                                "model request payload is invalid",
                            ),
                        )),
                    };
                    let (result, error) = match completion {
                        Ok(result) => (Some(result), None),
                        Err(error) => (None, Some(error)),
                    };
                    EntryCompletion {
                        entry_id,
                        task_id: task.task_id,
                        result,
                        error,
                    }
                }
            });
            let results = join_all(requests).await;
            Ok(CompletionBatch::from_results(&batch, results))
        })
    }
}

pub fn async_handler(
    adapter: Arc<OpenAiCompatibleAdapter>,
    providers: Vec<ProviderInstanceDescriptor>,
) -> Result<Arc<dyn AsyncBatchHandler>, ProtocolError> {
    Ok(Arc::new(OpenAiAdapterAsyncHandler::new(
        adapter, providers,
    )?))
}

fn runtime_error(
    task_id: &str,
    value: ProtocolError,
) -> mutsuki_runtime_sdk::contracts::RuntimeError {
    let mut error =
        mutsuki_runtime_sdk::contracts::RuntimeError::new(value.code, PLUGIN_ID, task_id);
    error.evidence.insert(
        "class".into(),
        mutsuki_runtime_sdk::contracts::ScalarValue::String(format!("{:?}", value.class)),
    );
    error.evidence.insert(
        "message".into(),
        mutsuki_runtime_sdk::contracts::ScalarValue::String(value.message),
    );
    error
}

fn endpoint(provider: &ProviderInstanceDescriptor) -> Result<Url, ProtocolError> {
    let mut endpoint = Url::parse(&provider.endpoint).map_err(|_| {
        error(
            "agent.adapter.invalid_endpoint",
            ProtocolErrorClass::NonRetryable,
            "provider endpoint is invalid",
        )
    })?;
    let loopback = matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if endpoint.scheme() != "https" && !loopback {
        return Err(error(
            "agent.adapter.invalid_endpoint",
            ProtocolErrorClass::NonRetryable,
            "provider endpoint must use https except for loopback conformance tests",
        ));
    }
    if endpoint.username() != "" || endpoint.password().is_some() {
        return Err(error(
            "agent.adapter.invalid_endpoint",
            ProtocolErrorClass::NonRetryable,
            "provider endpoint must not contain credentials",
        ));
    }
    if !endpoint.path().ends_with("/chat/completions") {
        let path = format!("{}/chat/completions", endpoint.path().trim_end_matches('/'));
        endpoint.set_path(&path);
    }
    Ok(endpoint)
}

fn payload(value: ModelGenerateRequest, stream: bool) -> Value {
    let mut payload = serde_json::Map::from_iter([
        ("model".into(), Value::String(value.request.model)),
        (
            "messages".into(),
            Value::Array(value.request.messages.iter().map(message_payload).collect()),
        ),
        ("stream".into(), Value::Bool(stream)),
    ]);
    if let Some(temperature) = value.request.temperature {
        payload.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = value.request.max_output_tokens {
        payload.insert("max_completion_tokens".into(), json!(max_tokens));
    }
    if !value.tools.is_empty() {
        payload.insert(
            "tools".into(),
            Value::Array(
                value
                    .tools
                    .into_iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.input_schema,
                            }
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(response_format) = value.structured_output {
        payload.insert("response_format".into(), response_format);
    }
    if let Some(reasoning) = value.reasoning {
        payload.insert("reasoning_effort".into(), reasoning);
    }
    Value::Object(payload)
}

fn message_payload(message: &AgentMessage) -> Value {
    let role = match message.role {
        AgentRole::System => "system",
        AgentRole::User => "user",
        AgentRole::Assistant => "assistant",
        AgentRole::Tool => "tool",
    };
    let content = if message.parts.is_empty() {
        Value::String(message.content.clone())
    } else {
        let mut parts = Vec::new();
        if !message.content.is_empty() {
            parts.push(json!({"type": "text", "text": message.content}));
        }
        for part in &message.parts {
            match part {
                AgentContentPart::Text { text } => {
                    parts.push(json!({"type": "text", "text": text}));
                }
                AgentContentPart::Image {
                    resource,
                    mime_type,
                    ..
                } => {
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("resource://{}", resource.ref_id),
                            "mime_type": mime_type,
                        }
                    }));
                }
                AgentContentPart::Audio {
                    resource,
                    mime_type,
                    ..
                } => {
                    parts.push(json!({
                        "type": "input_audio",
                        "input_audio": {
                            "resource_ref": resource.ref_id,
                            "format": mime_type,
                        }
                    }));
                }
                AgentContentPart::Document {
                    resource,
                    mime_type,
                    filename,
                    ..
                } => {
                    parts.push(json!({
                        "type": "file",
                        "file": {
                            "resource_ref": resource.ref_id,
                            "mime_type": mime_type,
                            "filename": filename,
                        }
                    }));
                }
                AgentContentPart::RemoteUrl { url, mime_type } => {
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": { "url": url, "mime_type": mime_type }
                    }));
                }
            }
        }
        Value::Array(parts)
    };
    let mut value = json!({"role": role, "content": content});
    if let Some(name) = &message.name {
        value["name"] = Value::String(name.clone());
    }
    if message.role == AgentRole::Assistant
        && let Some(tool_calls) = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("tool_calls"))
            .and_then(|calls| serde_json::from_value::<Vec<AgentToolCall>>(calls.clone()).ok())
    {
        value["tool_calls"] = Value::Array(
            tool_calls
                .into_iter()
                .map(|call| {
                    json!({
                        "id": call.call_id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.input.to_string(),
                        }
                    })
                })
                .collect(),
        );
    }
    if message.role == AgentRole::Tool
        && let Some(call_id) = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("call_id"))
            .and_then(Value::as_str)
    {
        value["tool_call_id"] = Value::String(call_id.into());
    }
    value
}

fn parse_response(body: Value) -> Result<AgentModelGenerateResult, ProtocolError> {
    let message = body.pointer("/choices/0/message").ok_or_else(|| {
        error(
            "agent.adapter.invalid_response",
            ProtocolErrorClass::Protocol,
            "response is missing choices[0].message",
        )
    })?;
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool_calls = parse_tool_calls(message.get("tool_calls"))?;
    if content.is_empty() && tool_calls.is_empty() {
        return Err(error(
            "agent.adapter.invalid_response",
            ProtocolErrorClass::Protocol,
            "response contains neither content nor tool calls",
        ));
    }
    let stop_reason = match body
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
    {
        Some("tool_calls") => AgentModelStopReason::ToolCalls,
        Some("length") => AgentModelStopReason::Length,
        Some("content_filter") => AgentModelStopReason::ContentFilter,
        Some("stop") | None => AgentModelStopReason::Stop,
        Some(_) => AgentModelStopReason::Other,
    };
    Ok(AgentModelGenerateResult {
        message: AgentMessage::assistant(content),
        stop_reason,
        tool_calls,
        usage: AgentUsage {
            input_tokens: body
                .pointer("/usage/prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            output_tokens: body
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            total_tokens: body
                .pointer("/usage/total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        },
        cost_microunits: 0,
        raw: Some(body),
        output_resource: None,
    })
}

fn parse_tool_calls(value: Option<&Value>) -> Result<Vec<AgentToolCall>, ProtocolError> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|call| {
            let call_id = call.get("id").and_then(Value::as_str).ok_or_else(|| {
                error(
                    "agent.adapter.invalid_response",
                    ProtocolErrorClass::Protocol,
                    "tool call is missing id",
                )
            })?;
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    error(
                        "agent.adapter.invalid_response",
                        ProtocolErrorClass::Protocol,
                        "tool call is missing function name",
                    )
                })?;
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let input = serde_json::from_str(arguments).map_err(|_| {
                error(
                    "agent.adapter.invalid_response",
                    ProtocolErrorClass::Protocol,
                    "tool call arguments are invalid JSON",
                )
            })?;
            Ok(AgentToolCall {
                call_id: call_id.into(),
                name: name.into(),
                input,
            })
        })
        .collect()
}

fn parse_sse(body: &str) -> Result<Vec<ModelStreamEvent>, ProtocolError> {
    let mut events = Vec::new();
    for (data, sequence) in body
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .zip(1_u64..)
    {
        if data == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(data).map_err(|_| {
            error(
                "agent.adapter.invalid_stream",
                ProtocolErrorClass::Protocol,
                "stream event is not valid JSON",
            )
        })?;
        if let Some(text) = value
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            events.push(ModelStreamEvent::MessageDelta {
                sequence,
                text: text.into(),
            });
        }
        if let Some(reasoning) = value
            .pointer("/choices/0/delta/reasoning")
            .and_then(Value::as_str)
        {
            events.push(ModelStreamEvent::ReasoningDelta {
                sequence,
                text: reasoning.into(),
            });
        }
    }
    Ok(events)
}

pub(crate) fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

pub(crate) fn status_error(status: StatusCode) -> ProtocolError {
    let class = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProtocolErrorClass::Authentication,
        StatusCode::TOO_MANY_REQUESTS => ProtocolErrorClass::RateLimited,
        status if status.is_server_error() => ProtocolErrorClass::Retryable,
        _ => ProtocolErrorClass::NonRetryable,
    };
    error(
        "agent.adapter.http_status",
        class,
        format!("model endpoint returned HTTP {}", status.as_u16()),
    )
}

pub(crate) fn transport_error(value: &reqwest::Error) -> ProtocolError {
    let class = if value.is_timeout() {
        ProtocolErrorClass::Timeout
    } else {
        ProtocolErrorClass::Retryable
    };
    error(
        "agent.adapter.transport",
        class,
        if value.is_timeout() {
            "model request timed out"
        } else {
            "model transport failed"
        },
    )
}

pub(crate) fn error(
    code: impl Into<String>,
    class: ProtocolErrorClass,
    message: impl Into<String>,
) -> ProtocolError {
    ProtocolError {
        code: code.into(),
        class,
        message: message.into(),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Instant;

    use mutsuki_agent_adapter_api::{CredentialFuture, CredentialValue};
    use mutsuki_agent_contracts::{
        AGENT_MODEL_GENERATE_PROTOCOL, AgentContentPart, AgentToolDescriptor, CredentialRef,
        ModelCapability, ToolSideEffect,
    };
    use mutsuki_runtime_sdk::contracts::{
        BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, Task, WorkResourcePlan,
    };

    use super::*;

    struct TestCredentials;

    impl CredentialBroker for TestCredentials {
        fn resolve(&self, _credential: CredentialRef) -> CredentialFuture {
            Box::pin(async { CredentialValue::new("TEST_SECRET") })
        }
    }

    struct SelectiveCredentials;

    impl CredentialBroker for SelectiveCredentials {
        fn resolve(&self, credential: CredentialRef) -> CredentialFuture {
            Box::pin(async move {
                if credential.credential_id == "unavailable" {
                    Err(error(
                        "agent.credential.unavailable",
                        ProtocolErrorClass::NonRetryable,
                        "provider credential is unavailable",
                    ))
                } else {
                    CredentialValue::new("TEST_SECRET")
                }
            })
        }
    }

    fn descriptor() -> ModelProtocolAdapterDescriptor {
        ModelProtocolAdapterDescriptor {
            adapter_id: "openai-compatible".into(),
            protocol: "openai.chat-completions".into(),
            version: "1".into(),
            runner_id: "agent.adapter.openai-compatible".into(),
            capability: ModelCapability {
                context_window: 128_000,
                streaming: true,
                tools: true,
                structured_output: true,
                ..ModelCapability::default()
            },
        }
    }

    fn provider(endpoint: String) -> ProviderInstanceDescriptor {
        ProviderInstanceDescriptor {
            provider_id: "provider-a".into(),
            adapter_id: "openai-compatible".into(),
            endpoint,
            credential: CredentialRef {
                credential_id: "test".into(),
                revision: 1,
            },
            models: BTreeMap::from([("model".into(), descriptor().capability)]),
            headers: BTreeMap::new(),
            compatibility: BTreeMap::from([
                ("timeout_ms".into(), json!(1_000)),
                ("max_retries".into(), json!(0)),
            ]),
            remote_execution_allowed: true,
        }
    }

    #[test]
    fn tool_loop_messages_preserve_assistant_calls_and_tool_results() {
        let call = AgentToolCall {
            call_id: "call-1".into(),
            name: "computer.fs.read".into(),
            input: json!({"path": "README.md"}),
        };
        let mut assistant = AgentMessage::assistant("");
        assistant.metadata = Some(json!({"tool_calls": [call]}));
        let assistant_payload = message_payload(&assistant);
        assert_eq!(assistant_payload["tool_calls"][0]["id"], "call-1");
        assert_eq!(
            assistant_payload["tool_calls"][0]["function"]["name"],
            "computer.fs.read"
        );

        let tool = AgentMessage {
            role: AgentRole::Tool,
            content: json!({"kind": "read", "content": "hello"}).to_string(),
            name: Some("computer.fs.read".into()),
            metadata: Some(json!({"call_id": "call-1"})),
            parts: Vec::new(),
        };
        let tool_payload = message_payload(&tool);
        assert_eq!(tool_payload["role"], "tool");
        assert_eq!(tool_payload["tool_call_id"], "call-1");
    }

    fn request() -> ModelGenerateRequest {
        let mut tool = AgentToolDescriptor::new("echo", "test.echo@1", "echo value");
        tool.side_effect = ToolSideEffect::None;
        tool.input_schema = json!({"type": "object"});
        ModelGenerateRequest {
            request: mutsuki_agent_contracts::AgentModelGenerateRequest {
                model: "model".into(),
                messages: vec![AgentMessage::user("hello")],
                temperature: None,
                max_output_tokens: Some(128),
                provider_hint: None,
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            },
            tools: vec![tool],
            structured_output: Some(json!({
                "type": "json_schema",
                "json_schema": {"name": "answer", "schema": {"type": "object"}}
            })),
            reasoning: None,
        }
    }

    #[tokio::test]
    async fn image_resource_part_is_encoded_for_protocol_generate() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 16_384];
            let read = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.contains("img-1"));
            assert!(request.contains("image_url"));
            assert!(request.contains("resource://"));
            let payload = r#"{"choices":[{"message":{"role":"assistant","content":"saw image"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#;
            let body = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            stream.write_all(body.as_bytes()).unwrap();
        });
        let adapter =
            OpenAiCompatibleAdapter::new(descriptor(), Arc::new(TestCredentials)).unwrap();
        let image = AgentMessage::user("describe").with_parts(vec![AgentContentPart::Image {
            resource: mutsuki_agent_sdk::stream_resource_ref("test", "img-1"),
            mime_type: "image/png".into(),
            width: Some(32),
            height: Some(32),
            size_bytes: Some(128),
            provenance: None,
        }]);
        let mut generate = request();
        generate.request.messages = vec![image];
        let result = adapter
            .generate(
                provider(format!("http://{address}/v1/chat/completions")),
                generate,
            )
            .await
            .unwrap();
        assert_eq!(result.message.content, "saw image");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn protocol_adapter_sends_tools_and_structured_output_and_parses_tool_call() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 16_384];
            let read = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.contains("authorization: Bearer TEST_SECRET"));
            assert!(request.contains("\"tools\""));
            assert!(request.contains("\"response_format\""));
            let body = json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "content": null,
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": "echo", "arguments": "{\"value\":\"ping\"}"}
                        }]
                    }
                }],
                "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let adapter =
            OpenAiCompatibleAdapter::new(descriptor(), Arc::new(TestCredentials)).unwrap();
        let result = adapter
            .generate(provider(format!("http://{address}/v1")), request())
            .await
            .unwrap();
        assert_eq!(result.stop_reason, AgentModelStopReason::ToolCalls);
        assert_eq!(result.tool_calls[0].name, "echo");
        assert_eq!(result.usage.total_tokens, 5);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn retryable_status_retries_but_non_retryable_status_does_not() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = [0_u8; 16_384];
                let _bytes_read = stream.read(&mut bytes).unwrap();
                if attempt == 0 {
                    write!(
                        stream,
                        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                } else {
                    let body = json!({
                        "choices": [{
                            "finish_reason": "stop",
                            "message": {"content": "retried"}
                        }],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                    })
                    .to_string();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
            }
        });
        let adapter =
            OpenAiCompatibleAdapter::new(descriptor(), Arc::new(TestCredentials)).unwrap();
        let mut retrying_provider = provider(format!("http://{address}/v1"));
        retrying_provider
            .compatibility
            .insert("max_retries".into(), json!(1));
        let result = adapter
            .generate(retrying_provider, request())
            .await
            .unwrap();
        assert_eq!(result.message.content, "retried");
        server.join().unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 16_384];
            let _bytes_read = stream.read(&mut bytes).unwrap();
            write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let error = adapter
            .generate(provider(format!("http://{address}/v1")), request())
            .await
            .unwrap_err();
        assert_eq!(error.class, ProtocolErrorClass::NonRetryable);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn timeout_is_classified_and_respects_provider_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 16_384];
            let _bytes_read = stream.read(&mut bytes).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        let adapter =
            OpenAiCompatibleAdapter::new(descriptor(), Arc::new(TestCredentials)).unwrap();
        let mut timeout_provider = provider(format!("http://{address}/v1"));
        timeout_provider
            .compatibility
            .insert("timeout_ms".into(), json!(20));
        let started = Instant::now();
        let error = adapter
            .generate(timeout_provider, request())
            .await
            .unwrap_err();
        assert_eq!(error.class, ProtocolErrorClass::Timeout);
        assert!(started.elapsed() < Duration::from_millis(90));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn unavailable_provider_credential_does_not_break_another_session() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 16_384];
            let _bytes_read = stream.read(&mut bytes).unwrap();
            let body = json!({
                "choices": [{
                    "finish_reason": "stop",
                    "message": {"content": "healthy session completed"}
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });

        let adapter = Arc::new(
            OpenAiCompatibleAdapter::new(descriptor(), Arc::new(SelectiveCredentials)).unwrap(),
        );
        let mut unavailable_provider = provider(format!("http://{address}/v1"));
        unavailable_provider.provider_id = "provider-unavailable".into();
        unavailable_provider.credential.credential_id = "unavailable".into();
        let mut healthy_provider = provider(format!("http://{address}/v1"));
        healthy_provider.provider_id = "provider-healthy".into();
        healthy_provider.credential.credential_id = "healthy".into();
        let handler =
            OpenAiAdapterAsyncHandler::new(adapter, vec![unavailable_provider, healthy_provider])
                .unwrap();

        let mut unavailable_request = request().request;
        unavailable_request.provider_hint = Some("provider-unavailable".into());
        unavailable_request.session_id = Some("session-unavailable".into());
        let mut healthy_request = request().request;
        healthy_request.provider_hint = Some("provider-healthy".into());
        healthy_request.session_id = Some("session-healthy".into());
        let tasks = vec![
            Task::new(
                "task-unavailable",
                AGENT_MODEL_GENERATE_PROTOCOL,
                serde_json::to_value(unavailable_request).unwrap(),
            ),
            Task::new(
                "task-healthy",
                AGENT_MODEL_GENERATE_PROTOCOL,
                serde_json::to_value(healthy_request).unwrap(),
            ),
        ];
        let batch = WorkBatch {
            batch_id: "batch:credential-isolation".into(),
            tick_id: "tick:credential-isolation".into(),
            batch_key: RUNNER_ID.into(),
            entries: tasks
                .iter()
                .enumerate()
                .map(|(index, task)| BatchEntry {
                    entry_id: format!("entry-{index}"),
                    task_id: task.task_id.clone(),
                    trace_id: None,
                    parent_id: None,
                    payload_index: index,
                    resource_requirement_indices: Vec::new(),
                    cancel_index: None,
                    deadline_tick: None,
                    priority: 0,
                    lane: DispatchLane::Normal,
                    ordering: OrderingRequirement::PreserveSubmitOrder,
                })
                .collect(),
            payload: BatchPayload::from_tasks(&tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        };
        let completion = handler
            .run_batch(
                RunnerContext::new(
                    1,
                    1,
                    "executor:credential-isolation",
                    Vec::<String>::new(),
                    "batch:credential-isolation",
                )
                .with_batch("batch:credential-isolation", 2),
                batch,
            )
            .await
            .unwrap();

        assert_eq!(
            completion.results[0].error.as_ref().unwrap().code,
            "agent.credential.unavailable"
        );
        assert!(completion.results[0].result.is_none());
        assert!(completion.results[1].error.is_none());
        assert!(completion.results[1].result.is_some());
        server.join().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_model_future_cancels_the_inflight_transport() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (closed_tx, closed_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 16_384];
            let _bytes_read = stream.read(&mut bytes).unwrap();
            accepted_tx.send(()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let closed = matches!(stream.read(&mut bytes), Ok(0) | Err(_));
            closed_tx.send(closed).unwrap();
        });
        let adapter = Arc::new(
            OpenAiCompatibleAdapter::new(descriptor(), Arc::new(TestCredentials)).unwrap(),
        );
        let provider = provider(format!("http://{address}/v1"));
        let request = request();
        let handle = tokio::spawn(async move { adapter.generate(provider, request).await });
        accepted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        handle.abort();
        let _ = handle.await;
        assert!(
            closed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "aborting the Host-driven adapter future must release its HTTP request"
        );
        server.join().unwrap();
    }

    #[test]
    fn streaming_events_are_normalized_with_monotonic_sequence() {
        let events = parse_sse(
            "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"reasoning\":\"b\"}}]}\n\n\
             data: [DONE]\n\n",
        )
        .unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            ModelStreamEvent::MessageDelta { sequence: 1, text } if text == "a"
        ));
        assert!(matches!(
            &events[1],
            ModelStreamEvent::ReasoningDelta { sequence: 2, text } if text == "b"
        ));
    }
}
