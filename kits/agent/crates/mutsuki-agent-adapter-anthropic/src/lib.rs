//! Anthropic Messages protocol Adapter.
//!
//! Implements `ModelProtocolAdapter` for Anthropic Console API keys (`x-api-key`).
//! Hosts inject credentials and provider endpoints; this crate does not read env
//! vars or ship default secrets.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_adapter_api::{
    CredentialBroker, ModelAdapterFuture, ModelProtocolAdapter, ModelStreamFuture,
};
use mutsuki_agent_contracts::{
    AgentMessage, AgentModelGenerateResult, AgentModelStopReason, AgentRole, AgentToolCall,
    AgentUsage, ModelCapability, ModelGenerateRequest, ModelProtocolAdapterDescriptor,
    ProtocolError, ProtocolErrorClass, ProviderInstanceDescriptor,
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
        let payload = messages_payload(request);
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

fn messages_payload(value: ModelGenerateRequest) -> Value {
    let mut messages = Vec::new();
    for message in &value.request.messages {
        let role = match message.role {
            AgentRole::User => "user",
            AgentRole::Assistant => "assistant",
            AgentRole::System | AgentRole::Tool => continue,
        };
        messages.push(json!({
            "role": role,
            "content": message.content,
        }));
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
    payload
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
                    .unwrap_or("tool_use")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
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
    use std::net::TcpListener;

    struct StaticCredentials;

    impl CredentialBroker for StaticCredentials {
        fn resolve(&self, _credential: CredentialRef) -> CredentialFuture {
            Box::pin(async { CredentialValue::new("sk-ant-test-key") })
        }
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
