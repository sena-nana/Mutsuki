//! Anthropic Messages protocol Adapter.
//!
//! Implements `ModelProtocolAdapter` for Anthropic Console API keys (`x-api-key`).
//! Hosts inject credentials and provider endpoints; this crate does not read env
//! vars or ship default secrets.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_adapter_api::{
    CredentialBroker, ModelAdapterFuture, ModelProtocolAdapter, ModelStreamFuture,
};
use mutsuki_agent_contracts::{
    AgentMessage, AgentModelGenerateResult, AgentModelStopReason, AgentRole, AgentToolCall,
    AgentToolResultMetadata, AgentUsage, ModelCapability, ModelGenerateRequest,
    ModelProtocolAdapterDescriptor, ProtocolError, ProtocolErrorClass, ProviderInstanceDescriptor,
};
use reqwest::Client;
use serde_json::{Value, json};

pub const ADAPTER_ID: &str = "anthropic-messages";
pub const PROTOCOL: &str = "anthropic.messages";
pub const RUNNER_ID: &str = "mutsuki.agent.adapter.anthropic-messages.runner";
pub const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Clone)]
pub struct AnthropicMessagesAdapter {
    descriptor: ModelProtocolAdapterDescriptor,
    credentials: Arc<dyn CredentialBroker>,
    client: Client,
}

impl AnthropicMessagesAdapter {
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
        let _ = rustls::crypto::ring::default_provider().install_default();
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
            capability: ModelCapability {
                context_window: 200_000,
                streaming: false,
                tools: true,
                structured_output: false,
                ..ModelCapability::default()
            },
        }
    }

    async fn request(
        &self,
        provider: ProviderInstanceDescriptor,
        request: ModelGenerateRequest,
    ) -> Result<Value, ProtocolError> {
        let endpoint = messages_endpoint(&provider)?;
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
        let payload = messages_payload(request)?;
        for attempt in 0..=retries {
            let mut builder = self
                .client
                .post(endpoint.clone())
                .header("x-api-key", credential.expose_secret())
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .timeout(Duration::from_millis(timeout_ms))
                .json(&payload);
            for (name, value) in &provider.headers {
                builder = builder.header(name, value);
            }
            match builder.send().await {
                Ok(response) if response.status().is_success() => {
                    return response.json().await.map_err(|_| {
                        error(
                            "agent.adapter.invalid_response",
                            ProtocolErrorClass::Protocol,
                            "response body is not valid JSON",
                        )
                    });
                }
                Ok(response)
                    if retryable_status(response.status().as_u16()) && attempt < retries =>
                {
                    continue;
                }
                Ok(response) => return Err(status_error(response.status().as_u16())),
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

impl ModelProtocolAdapter for AnthropicMessagesAdapter {
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
            let body = adapter.request(provider, request).await?;
            parse_messages_response(body)
        })
    }

    fn stream(
        &self,
        _provider: ProviderInstanceDescriptor,
        _request: ModelGenerateRequest,
    ) -> ModelStreamFuture {
        Box::pin(async {
            Err(error(
                "agent.adapter.streaming_unsupported",
                ProtocolErrorClass::NonRetryable,
                "anthropic messages adapter is generate-only in this slice",
            ))
        })
    }
}

/// Resolve endpoint from an explicit override, else the public Anthropic default.
/// Hosts/products may resolve env overrides before calling this helper.
pub fn resolve_endpoint(override_endpoint: Option<&str>) -> String {
    override_endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
}

pub fn provider_descriptor(
    provider_id: impl Into<String>,
    endpoint: &str,
    credential: mutsuki_agent_contracts::CredentialRef,
    model: &str,
) -> ProviderInstanceDescriptor {
    let mut models = BTreeMap::new();
    models.insert(
        model.to_string(),
        ModelCapability {
            context_window: 200_000,
            streaming: false,
            tools: true,
            structured_output: false,
            ..ModelCapability::default()
        },
    );
    ProviderInstanceDescriptor {
        provider_id: provider_id.into(),
        adapter_id: ADAPTER_ID.into(),
        endpoint: endpoint.to_string(),
        credential,
        models,
        headers: BTreeMap::new(),
        compatibility: BTreeMap::from([
            ("timeout_ms".into(), json!(30_000)),
            ("max_retries".into(), json!(1)),
        ]),
        remote_execution_allowed: true,
    }
}

fn messages_endpoint(provider: &ProviderInstanceDescriptor) -> Result<String, ProtocolError> {
    let base = provider.endpoint.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(error(
            "agent.adapter.invalid_endpoint",
            ProtocolErrorClass::NonRetryable,
            "provider endpoint is empty",
        ));
    }
    let loopback = base.contains("127.0.0.1") || base.contains("localhost");
    if !(base.starts_with("https://") || (loopback && base.starts_with("http://"))) {
        return Err(error(
            "agent.adapter.invalid_endpoint",
            ProtocolErrorClass::NonRetryable,
            "provider endpoint must use https except for loopback conformance tests",
        ));
    }
    if base.ends_with("/v1/messages") {
        Ok(base.to_string())
    } else if base.ends_with("/v1") {
        Ok(format!("{base}/messages"))
    } else {
        Ok(format!("{base}/v1/messages"))
    }
}

fn messages_payload(value: ModelGenerateRequest) -> Result<Value, ProtocolError> {
    let mut messages = Vec::new();
    let mut tool_uses = BTreeSet::new();
    let mut tool_results = BTreeSet::new();
    let mut index = 0;
    while index < value.request.messages.len() {
        let message = &value.request.messages[index];
        match message.role {
            AgentRole::System => {}
            AgentRole::User => messages.push(json!({
                "role": "user",
                "content": message.content,
            })),
            AgentRole::Assistant => {
                let tool_calls = assistant_tool_calls(message)?;
                if tool_calls.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": message.content,
                    }));
                } else {
                    let mut content = Vec::new();
                    if !message.content.is_empty() {
                        content.push(json!({"type": "text", "text": &message.content}));
                    }
                    for call in tool_calls {
                        validate_tool_call(&call, &mut tool_uses)?;
                        content.push(json!({
                            "type": "tool_use",
                            "id": call.call_id,
                            "name": call.name,
                            "input": call.input,
                        }));
                    }
                    messages.push(json!({
                        "role": "assistant",
                        "content": content,
                    }));
                }
            }
            AgentRole::Tool => {
                let mut content = Vec::new();
                while index < value.request.messages.len()
                    && value.request.messages[index].role == AgentRole::Tool
                {
                    let tool_message = &value.request.messages[index];
                    let metadata = tool_result_metadata(tool_message)?;
                    if !tool_uses.contains(&metadata.call_id) {
                        return Err(invalid_request(format!(
                            "tool_result `{}` does not reference an earlier tool_use",
                            metadata.call_id
                        )));
                    }
                    if !tool_results.insert(metadata.call_id.clone()) {
                        return Err(invalid_request(format!(
                            "duplicate tool_result id `{}`",
                            metadata.call_id
                        )));
                    }
                    if metadata.is_error != metadata.error.is_some() {
                        return Err(invalid_request(format!(
                            "tool_result `{}` has inconsistent error metadata",
                            metadata.call_id
                        )));
                    }
                    let mut block = json!({
                        "type": "tool_result",
                        "tool_use_id": metadata.call_id,
                        "content": tool_message.content,
                    });
                    if metadata.is_error {
                        block["is_error"] = Value::Bool(true);
                    }
                    content.push(block);
                    index += 1;
                }
                messages.push(json!({"role": "user", "content": content}));
                continue;
            }
        }
        index += 1;
    }
    if let Some(unresolved) = tool_uses.difference(&tool_results).next() {
        return Err(invalid_request(format!(
            "tool_use `{unresolved}` is missing its tool_result"
        )));
    }
    let mut payload = json!({
        "model": value.request.model,
        "max_tokens": value.request.max_output_tokens.unwrap_or(1024),
        "messages": messages,
    });
    if let Some(system) = value
        .request
        .messages
        .iter()
        .find(|message| message.role == AgentRole::System)
    {
        payload["system"] = Value::String(system.content.clone());
    }
    if !value.tools.is_empty() {
        payload["tools"] = Value::Array(
            value
                .tools
                .into_iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    })
                })
                .collect(),
        );
    }
    Ok(payload)
}

fn assistant_tool_calls(message: &AgentMessage) -> Result<Vec<AgentToolCall>, ProtocolError> {
    let Some(metadata) = message.metadata.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(calls) = metadata.get("tool_calls") else {
        return Ok(Vec::new());
    };
    serde_json::from_value(calls.clone())
        .map_err(|error| invalid_request(format!("assistant tool_calls are malformed: {error}")))
}

fn validate_tool_call(
    call: &AgentToolCall,
    tool_uses: &mut BTreeSet<String>,
) -> Result<(), ProtocolError> {
    if call.call_id.trim().is_empty() || call.name.trim().is_empty() {
        return Err(invalid_request(
            "tool_use call_id and name must both be non-empty",
        ));
    }
    if !call.input.is_object() {
        return Err(invalid_request(format!(
            "tool_use `{}` input must be an object",
            call.call_id
        )));
    }
    if !tool_uses.insert(call.call_id.clone()) {
        return Err(invalid_request(format!(
            "duplicate tool_use id `{}`",
            call.call_id
        )));
    }
    Ok(())
}

fn tool_result_metadata(message: &AgentMessage) -> Result<AgentToolResultMetadata, ProtocolError> {
    let metadata = message
        .metadata
        .as_ref()
        .ok_or_else(|| invalid_request("tool_result is missing metadata"))?;
    let metadata: AgentToolResultMetadata = serde_json::from_value(metadata.clone())
        .map_err(|error| invalid_request(format!("tool_result metadata is malformed: {error}")))?;
    if metadata.call_id.trim().is_empty() {
        return Err(invalid_request("tool_result call_id must be non-empty"));
    }
    Ok(metadata)
}

fn parse_messages_response(body: Value) -> Result<AgentModelGenerateResult, ProtocolError> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            error(
                "agent.adapter.invalid_response",
                ProtocolErrorClass::Protocol,
                "response is missing content array",
            )
        })?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut tool_call_ids = BTreeSet::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(chunk) = block.get("text").and_then(Value::as_str) {
                    text.push_str(chunk);
                }
            }
            Some("tool_use") => {
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        error(
                            "agent.adapter.invalid_response",
                            ProtocolErrorClass::Protocol,
                            "tool_use block is missing a non-empty id",
                        )
                    })?
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        error(
                            "agent.adapter.invalid_response",
                            ProtocolErrorClass::Protocol,
                            "tool_use block is missing a non-empty name",
                        )
                    })?
                    .to_string();
                if !tool_call_ids.insert(call_id.clone()) {
                    return Err(error(
                        "agent.adapter.invalid_response",
                        ProtocolErrorClass::Protocol,
                        "tool_use response ids must be unique",
                    ));
                }
                let input = block
                    .get("input")
                    .filter(|value| value.is_object())
                    .cloned()
                    .ok_or_else(|| {
                        error(
                            "agent.adapter.invalid_response",
                            ProtocolErrorClass::Protocol,
                            "tool_use block input must be an object",
                        )
                    })?;
                tool_calls.push(AgentToolCall {
                    call_id,
                    name,
                    input,
                });
            }
            _ => {}
        }
    }
    if text.is_empty() && tool_calls.is_empty() {
        return Err(error(
            "agent.adapter.invalid_response",
            ProtocolErrorClass::Protocol,
            "response contains neither text nor tool_use blocks",
        ));
    }
    let stop_reason = match body.get("stop_reason").and_then(Value::as_str) {
        Some("tool_use") => AgentModelStopReason::ToolCalls,
        Some("max_tokens") => AgentModelStopReason::Length,
        Some("end_turn") | None => {
            if tool_calls.is_empty() {
                AgentModelStopReason::Stop
            } else {
                AgentModelStopReason::ToolCalls
            }
        }
        Some(_) => AgentModelStopReason::Other,
    };
    let input_tokens = body
        .pointer("/usage/input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = body
        .pointer("/usage/output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(AgentModelGenerateResult {
        message: AgentMessage::assistant(text),
        stop_reason,
        tool_calls,
        usage: AgentUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens.saturating_add(output_tokens),
        },
        cost_microunits: 0,
        raw: Some(body),
        output_resource: None,
    })
}

fn retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

fn error(code: &str, class: ProtocolErrorClass, message: &str) -> ProtocolError {
    ProtocolError {
        code: code.into(),
        class,
        message: message.into(),
        retry_after_ms: None,
    }
}

fn invalid_request(message: impl Into<String>) -> ProtocolError {
    let message = message.into();
    error(
        "agent.adapter.invalid_request",
        ProtocolErrorClass::Protocol,
        &message,
    )
}

fn transport_error(err: &reqwest::Error) -> ProtocolError {
    let class = if err.is_timeout() {
        ProtocolErrorClass::Timeout
    } else {
        ProtocolErrorClass::Retryable
    };
    error(
        "agent.adapter.transport",
        class,
        if err.is_timeout() {
            "model request timed out"
        } else {
            "model transport failed"
        },
    )
}

fn status_error(status: u16) -> ProtocolError {
    let class = if status == 429 {
        ProtocolErrorClass::RateLimited
    } else if (500..600).contains(&status) {
        ProtocolErrorClass::Retryable
    } else if status == 401 || status == 403 {
        ProtocolErrorClass::Authentication
    } else {
        ProtocolErrorClass::NonRetryable
    };
    error(
        "agent.adapter.http_status",
        class,
        &format!("anthropic messages endpoint returned HTTP {status}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_adapter_api::{CredentialFuture, CredentialValue};
    use mutsuki_agent_contracts::{AgentToolDescriptor, CredentialRef, ToolSideEffect};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    struct StaticCredentials;

    impl CredentialBroker for StaticCredentials {
        fn resolve(&self, _credential: CredentialRef) -> CredentialFuture {
            Box::pin(async { CredentialValue::new("sk-ant-test-key") })
        }
    }

    fn model_request(messages: Vec<AgentMessage>) -> ModelGenerateRequest {
        ModelGenerateRequest {
            request: mutsuki_agent_contracts::AgentModelGenerateRequest {
                model: DEFAULT_MODEL.into(),
                messages,
                temperature: None,
                max_output_tokens: Some(256),
                provider_hint: None,
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            },
            tools: Vec::new(),
            structured_output: None,
            reasoning: None,
        }
    }

    fn write_tool_descriptor() -> AgentToolDescriptor {
        let mut tool = AgentToolDescriptor::new(
            "computer.fs.write",
            "mutsuki.agent.computer.fs.write@1",
            "write a workspace file",
        );
        tool.side_effect = ToolSideEffect::WorkspaceWrite;
        tool.input_schema = json!({"type": "object"});
        tool
    }

    fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0, "client closed before completing request body");
            bytes.extend_from_slice(&chunk[..count]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
                .expect("request declares content-length");
            let body_start = header_end + 4;
            if bytes.len() >= body_start + content_length {
                return serde_json::from_slice(&bytes[body_start..body_start + content_length])
                    .unwrap();
            }
        }
    }

    fn write_json_response(stream: &mut TcpStream, payload: &str) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
        .unwrap();
    }

    #[test]
    fn resolve_endpoint_prefers_override_over_default() {
        assert_eq!(
            resolve_endpoint(Some("https://example.test/v1")),
            "https://example.test/v1"
        );
        assert_eq!(resolve_endpoint(Some("  ")), DEFAULT_ENDPOINT);
        assert_eq!(resolve_endpoint(None), DEFAULT_ENDPOINT);
    }

    #[test]
    fn tool_loop_messages_preserve_parallel_results_and_errors() {
        let calls = vec![
            AgentToolCall {
                call_id: "toolu_1".into(),
                name: "computer.fs.read".into(),
                input: json!({"path": "README.md"}),
            },
            AgentToolCall {
                call_id: "toolu_2".into(),
                name: "computer.fs.write".into(),
                input: json!({"path": "README.md", "content": "hello"}),
            },
        ];
        let mut assistant = AgentMessage::assistant("checking workspace");
        assistant.metadata = Some(json!({"tool_calls": calls}));
        let read_result = AgentMessage {
            role: AgentRole::Tool,
            content: json!({"kind": "read", "content": "hello"}).to_string(),
            name: Some("computer.fs.read".into()),
            metadata: Some(
                serde_json::to_value(AgentToolResultMetadata {
                    call_id: "toolu_1".into(),
                    output_ref: None,
                    is_error: false,
                    error: None,
                })
                .unwrap(),
            ),
            parts: Vec::new(),
        };
        let write_error = mutsuki_agent_contracts::AgentError::new(
            "computer.fs.denied",
            "workspace write was denied",
        );
        let write_result = AgentMessage {
            role: AgentRole::Tool,
            content: serde_json::to_string(&write_error).unwrap(),
            name: Some("computer.fs.write".into()),
            metadata: Some(
                serde_json::to_value(AgentToolResultMetadata {
                    call_id: "toolu_2".into(),
                    output_ref: None,
                    is_error: true,
                    error: Some(write_error),
                })
                .unwrap(),
            ),
            parts: Vec::new(),
        };
        let payload = messages_payload(ModelGenerateRequest {
            request: mutsuki_agent_contracts::AgentModelGenerateRequest {
                model: DEFAULT_MODEL.into(),
                messages: vec![
                    AgentMessage::user("read and write it"),
                    assistant,
                    read_result,
                    write_result,
                ],
                temperature: None,
                max_output_tokens: Some(256),
                provider_hint: None,
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            },
            tools: Vec::new(),
            structured_output: None,
            reasoning: None,
        })
        .unwrap();
        assert_eq!(payload["messages"][1]["content"][0]["type"], "text");
        assert_eq!(payload["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(payload["messages"][1]["content"][1]["id"], "toolu_1");
        assert_eq!(payload["messages"][1]["content"][2]["id"], "toolu_2");
        assert_eq!(payload["messages"].as_array().unwrap().len(), 3);
        assert_eq!(payload["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(
            payload["messages"][2]["content"][0]["tool_use_id"],
            "toolu_1"
        );
        assert_eq!(
            payload["messages"][2]["content"][1]["tool_use_id"],
            "toolu_2"
        );
        assert_eq!(payload["messages"][2]["content"][1]["is_error"], true);
    }

    #[test]
    fn malformed_tool_causality_is_rejected_deterministically() {
        let mut malformed_calls = AgentMessage::assistant("");
        malformed_calls.metadata = Some(json!({"tool_calls": "not-an-array"}));
        let err = messages_payload(model_request(vec![malformed_calls])).unwrap_err();
        assert_eq!(err.code, "agent.adapter.invalid_request");

        let mut duplicate_calls = AgentMessage::assistant("");
        duplicate_calls.metadata = Some(json!({"tool_calls": [
            {"call_id": "same", "name": "one", "input": {}},
            {"call_id": "same", "name": "two", "input": {}}
        ]}));
        let err = messages_payload(model_request(vec![duplicate_calls])).unwrap_err();
        assert_eq!(err.code, "agent.adapter.invalid_request");

        let orphan = AgentMessage {
            role: AgentRole::Tool,
            content: "orphan".into(),
            name: Some("tool".into()),
            metadata: Some(json!({"call_id": "missing"})),
            parts: Vec::new(),
        };
        let err = messages_payload(model_request(vec![orphan])).unwrap_err();
        assert_eq!(err.code, "agent.adapter.invalid_request");

        let mut missing_result = AgentMessage::assistant("");
        missing_result.metadata = Some(json!({"tool_calls": [
            {"call_id": "toolu_1", "name": "read", "input": {}}
        ]}));
        let err = messages_payload(model_request(vec![missing_result])).unwrap_err();
        assert_eq!(err.code, "agent.adapter.invalid_request");
    }

    #[test]
    fn malformed_tool_use_response_is_rejected() {
        for body in [
            json!({"content": [{"type": "tool_use", "id": "", "name": "read", "input": {}}]}),
            json!({"content": [{"type": "tool_use", "id": "call", "name": "", "input": {}}]}),
            json!({"content": [{"type": "tool_use", "id": "call", "name": "read", "input": []}]}),
            json!({"content": [
                {"type": "tool_use", "id": "call", "name": "read", "input": {}},
                {"type": "tool_use", "id": "call", "name": "write", "input": {}}
            ]}),
        ] {
            let err = parse_messages_response(body).unwrap_err();
            assert_eq!(err.code, "agent.adapter.invalid_response");
        }
    }

    #[test]
    fn loopback_messages_generate_parses_text_and_tool_use() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 16_384];
            let n = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            assert!(request.contains("x-api-key"));
            assert!(request.contains("anthropic-version"));
            assert!(request.contains("\"tools\""));
            let payload = r#"{"content":[{"type":"text","text":"anthropic hello"},{"type":"tool_use","id":"toolu_1","name":"native.coding.fix","input":{"prompt":"fix"}}],"stop_reason":"tool_use","usage":{"input_tokens":4,"output_tokens":6}}"#;
            let body = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            stream.write_all(body.as_bytes()).unwrap();
        });

        let adapter = AnthropicMessagesAdapter::new(
            AnthropicMessagesAdapter::default_descriptor(),
            Arc::new(StaticCredentials),
        )
        .unwrap();
        let provider = provider_descriptor(
            "anthropic-console",
            &format!("http://{address}"),
            CredentialRef {
                credential_id: "cred".into(),
                revision: 1,
            },
            DEFAULT_MODEL,
        );
        let mut tool = AgentToolDescriptor::new(
            "native.coding.fix",
            "mutsuki.agent.tool.native.coding.fix@1",
            "fix",
        );
        tool.side_effect = ToolSideEffect::WorkspaceWrite;
        tool.input_schema = json!({"type":"object"});
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(adapter.generate(
                provider,
                ModelGenerateRequest {
                    request: mutsuki_agent_contracts::AgentModelGenerateRequest {
                        model: DEFAULT_MODEL.into(),
                        messages: vec![AgentMessage::user("hi")],
                        temperature: None,
                        max_output_tokens: Some(256),
                        provider_hint: None,
                        metadata: None,
                        result_protocol_id: None,
                        result_context: None,
                        session_id: None,
                    },
                    tools: vec![tool],
                    structured_output: None,
                    reasoning: None,
                },
            ))
            .unwrap();
        assert!(result.message.content.contains("anthropic hello"));
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "native.coding.fix");
        assert_eq!(result.stop_reason, AgentModelStopReason::ToolCalls);
        assert_eq!(result.usage.total_tokens, 10);
        server.join().unwrap();
    }

    #[test]
    fn loopback_tool_round_trip_sends_causal_error_result_then_final_text() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let first_payload = read_json_request(&mut first);
            assert_eq!(first_payload["messages"][0]["role"], "user");
            write_json_response(
                &mut first,
                r#"{"content":[{"type":"tool_use","id":"toolu_write","name":"computer.fs.write","input":{"path":"README.md","content":"hello"}}],"stop_reason":"tool_use","usage":{"input_tokens":4,"output_tokens":6}}"#,
            );

            let (mut second, _) = listener.accept().unwrap();
            let second_payload = read_json_request(&mut second);
            assert_eq!(
                second_payload["messages"][1]["content"][0]["id"],
                "toolu_write"
            );
            assert_eq!(
                second_payload["messages"][2]["content"][0]["tool_use_id"],
                "toolu_write"
            );
            assert_eq!(
                second_payload["messages"][2]["content"][0]["is_error"],
                true
            );
            write_json_response(
                &mut second,
                r#"{"content":[{"type":"text","text":"write was denied; no file changed"}],"stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":5}}"#,
            );
        });

        let adapter = AnthropicMessagesAdapter::new(
            AnthropicMessagesAdapter::default_descriptor(),
            Arc::new(StaticCredentials),
        )
        .unwrap();
        let provider = provider_descriptor(
            "anthropic-console",
            &format!("http://{address}"),
            CredentialRef {
                credential_id: "cred".into(),
                revision: 1,
            },
            DEFAULT_MODEL,
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut first_request = model_request(vec![AgentMessage::user("write README")]);
        first_request.tools.push(write_tool_descriptor());
        let first = runtime
            .block_on(adapter.generate(provider.clone(), first_request))
            .unwrap();
        assert_eq!(first.tool_calls.len(), 1);
        let mut assistant = first.message;
        assistant.metadata = Some(json!({"tool_calls": first.tool_calls}));
        let error = mutsuki_agent_contracts::AgentError::new(
            "computer.fs.denied",
            "workspace write was denied",
        );
        let tool_result = AgentMessage {
            role: AgentRole::Tool,
            content: serde_json::to_string(&error).unwrap(),
            name: Some("computer.fs.write".into()),
            metadata: Some(
                serde_json::to_value(AgentToolResultMetadata {
                    call_id: "toolu_write".into(),
                    output_ref: None,
                    is_error: true,
                    error: Some(error),
                })
                .unwrap(),
            ),
            parts: Vec::new(),
        };
        let mut second_request = model_request(vec![
            AgentMessage::user("write README"),
            assistant,
            tool_result,
        ]);
        second_request.tools.push(write_tool_descriptor());
        let final_result = runtime
            .block_on(adapter.generate(provider, second_request))
            .unwrap();
        assert_eq!(final_result.stop_reason, AgentModelStopReason::Stop);
        assert_eq!(
            final_result.message.content,
            "write was denied; no file changed"
        );
        server.join().unwrap();
    }

    #[test]
    fn non_retryable_http_status_is_classified() {
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
        let adapter = AnthropicMessagesAdapter::new(
            AnthropicMessagesAdapter::default_descriptor(),
            Arc::new(StaticCredentials),
        )
        .unwrap();
        let mut provider = provider_descriptor(
            "anthropic-console",
            &format!("http://{address}"),
            CredentialRef {
                credential_id: "cred".into(),
                revision: 1,
            },
            DEFAULT_MODEL,
        );
        provider
            .compatibility
            .insert("max_retries".into(), json!(0));
        let err = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(adapter.generate(
                provider,
                ModelGenerateRequest {
                    request: mutsuki_agent_contracts::AgentModelGenerateRequest {
                        model: DEFAULT_MODEL.into(),
                        messages: vec![AgentMessage::user("hi")],
                        temperature: None,
                        max_output_tokens: Some(64),
                        provider_hint: None,
                        metadata: None,
                        result_protocol_id: None,
                        result_context: None,
                        session_id: None,
                    },
                    tools: Vec::new(),
                    structured_output: None,
                    reasoning: None,
                },
            ))
            .unwrap_err();
        assert_eq!(err.class, ProtocolErrorClass::NonRetryable);
        server.join().unwrap();
    }
}
