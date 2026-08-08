//! OpenAI Responses protocol Adapter (`openai.responses`, `/v1/responses`).
//!
//! Maps unified AgentKit model requests onto the Responses API. Hosts inject
//! credentials and endpoints; this module does not read environment variables
//! or ship default secrets.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_adapter_api::{
    CredentialBroker, ModelAdapterFuture, ModelProtocolAdapter, ModelStreamFuture,
};
use mutsuki_agent_contracts::{
    AgentMessage, AgentModelGenerateResult, AgentModelStopReason, AgentRole, AgentToolCall,
    AgentToolResultMetadata, AgentUsage, ModelCapability, ModelGenerateRequest,
    ModelProtocolAdapterDescriptor, ModelStreamEvent, ProtocolError, ProtocolErrorClass,
    ProviderInstanceDescriptor,
};
use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};

pub const ADAPTER_ID: &str = "openai-responses";
pub const PROTOCOL: &str = "openai.responses";
pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.adapter.openai-responses";
pub const RUNNER_ID: &str = "mutsuki.agent.adapter.openai-responses.runner";

#[derive(Clone)]
pub struct OpenAiResponsesAdapter {
    descriptor: ModelProtocolAdapterDescriptor,
    credentials: Arc<dyn CredentialBroker>,
    client: Client,
}

impl OpenAiResponsesAdapter {
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
                context_window: 128_000,
                streaming: true,
                tools: true,
                structured_output: true,
                reasoning: true,
                ..ModelCapability::default()
            },
        }
    }

    pub fn provider_descriptor(
        provider_id: impl Into<String>,
        endpoint: &str,
        credential: mutsuki_agent_contracts::CredentialRef,
        model: &str,
    ) -> ProviderInstanceDescriptor {
        let mut models = BTreeMap::new();
        models.insert(model.to_string(), Self::default_descriptor().capability);
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

    async fn request(
        &self,
        provider: ProviderInstanceDescriptor,
        request: ModelGenerateRequest,
        stream: bool,
    ) -> Result<Value, ProtocolError> {
        let endpoint = responses_endpoint(&provider)?;
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
        let payload = responses_payload(request, stream)?;
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

impl ModelProtocolAdapter for OpenAiResponsesAdapter {
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
            parse_responses_body(body)
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
            parse_responses_sse(body.as_str().unwrap_or_default())
        })
    }
}

fn responses_endpoint(provider: &ProviderInstanceDescriptor) -> Result<Url, ProtocolError> {
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
    if !endpoint.path().ends_with("/responses") {
        let path = format!("{}/responses", endpoint.path().trim_end_matches('/'));
        endpoint.set_path(&path);
    }
    Ok(endpoint)
}

fn responses_payload(value: ModelGenerateRequest, stream: bool) -> Result<Value, ProtocolError> {
    let mut instructions = None;
    let mut input = Vec::new();
    let mut pending_tool_calls: BTreeMap<String, AgentToolCall> = BTreeMap::new();

    for message in &value.request.messages {
        match message.role {
            AgentRole::System => {
                if instructions.is_none() {
                    instructions = Some(message.content.clone());
                } else {
                    input.push(json!({
                        "role": "system",
                        "content": message.content,
                    }));
                }
            }
            AgentRole::User => {
                input.push(json!({
                    "role": "user",
                    "content": message.content,
                }));
            }
            AgentRole::Assistant => {
                let tool_calls = assistant_tool_calls(message)?;
                if !message.content.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": message.content,
                    }));
                }
                for call in tool_calls {
                    validate_tool_call(&call)?;
                    pending_tool_calls.insert(call.call_id.clone(), call.clone());
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.call_id,
                        "name": call.name,
                        "arguments": call.input.to_string(),
                    }));
                }
            }
            AgentRole::Tool => {
                let metadata = tool_result_metadata(message)?;
                if !pending_tool_calls.contains_key(&metadata.call_id) {
                    return Err(invalid_request(format!(
                        "function_call_output `{}` does not reference an earlier function_call",
                        metadata.call_id
                    )));
                }
                pending_tool_calls.remove(&metadata.call_id);
                let output = if metadata.is_error {
                    match &metadata.error {
                        Some(err) => format!("error: {} ({})", err.message, err.code),
                        None => {
                            if message.content.is_empty() {
                                "error".into()
                            } else {
                                message.content.clone()
                            }
                        }
                    }
                } else {
                    message.content.clone()
                };
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": metadata.call_id,
                    "output": output,
                }));
            }
        }
    }

    if let Some(unresolved) = pending_tool_calls.keys().next() {
        return Err(invalid_request(format!(
            "function_call `{unresolved}` is missing its function_call_output"
        )));
    }

    let mut payload = serde_json::Map::from_iter([
        ("model".into(), Value::String(value.request.model)),
        ("input".into(), Value::Array(input)),
        ("stream".into(), Value::Bool(stream)),
    ]);
    if let Some(instructions) = instructions {
        payload.insert("instructions".into(), Value::String(instructions));
    }
    if let Some(max_tokens) = value.request.max_output_tokens {
        payload.insert("max_output_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = value.request.temperature {
        payload.insert("temperature".into(), json!(temperature));
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
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(structured) = value.structured_output {
        payload.insert("text".into(), structured);
    }
    if let Some(reasoning) = value.reasoning {
        payload.insert("reasoning".into(), reasoning);
    }
    Ok(Value::Object(payload))
}

fn assistant_tool_calls(message: &AgentMessage) -> Result<Vec<AgentToolCall>, ProtocolError> {
    let Some(metadata) = message.metadata.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(calls) = metadata.get("tool_calls") else {
        return Ok(Vec::new());
    };
    serde_json::from_value(calls.clone())
        .map_err(|err| invalid_request(format!("assistant tool_calls are malformed: {err}")))
}

fn validate_tool_call(call: &AgentToolCall) -> Result<(), ProtocolError> {
    if call.call_id.trim().is_empty() || call.name.trim().is_empty() {
        return Err(invalid_request(
            "function_call call_id and name must both be non-empty",
        ));
    }
    Ok(())
}

fn tool_result_metadata(message: &AgentMessage) -> Result<AgentToolResultMetadata, ProtocolError> {
    let metadata = message
        .metadata
        .as_ref()
        .ok_or_else(|| invalid_request("function_call_output is missing metadata"))?;
    // Accept full AgentToolResultMetadata or the OpenAI-compatible {call_id} shape.
    if let Ok(parsed) = serde_json::from_value::<AgentToolResultMetadata>(metadata.clone()) {
        if parsed.call_id.trim().is_empty() {
            return Err(invalid_request(
                "function_call_output call_id must be non-empty",
            ));
        }
        return Ok(parsed);
    }
    let call_id = metadata
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_request("function_call_output call_id must be non-empty"))?;
    Ok(AgentToolResultMetadata {
        call_id: call_id.into(),
        output_ref: None,
        is_error: metadata
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        error: None,
    })
}

fn parse_responses_body(body: Value) -> Result<AgentModelGenerateResult, ProtocolError> {
    let output = body
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            error(
                "agent.adapter.invalid_response",
                ProtocolErrorClass::Protocol,
                "response is missing output array",
            )
        })?;

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for item in output {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        match item_type {
            "message" => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
                        if matches!(part_type, "output_text" | "text")
                            && let Some(part_text) = part.get("text").and_then(Value::as_str)
                        {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(part_text);
                        }
                    }
                } else if let Some(content) = item.get("content").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(content);
                }
            }
            "function_call" => {
                tool_calls.push(parse_function_call(item)?);
            }
            _ => {}
        }
    }

    if text.is_empty() && tool_calls.is_empty() {
        return Err(error(
            "agent.adapter.invalid_response",
            ProtocolErrorClass::Protocol,
            "response contains neither output text nor function calls",
        ));
    }

    let stop_reason = if !tool_calls.is_empty() {
        AgentModelStopReason::ToolCalls
    } else {
        match body.get("status").and_then(Value::as_str) {
            Some("incomplete") => AgentModelStopReason::Length,
            Some("failed") => AgentModelStopReason::Other,
            _ => AgentModelStopReason::Stop,
        }
    };

    Ok(AgentModelGenerateResult {
        message: AgentMessage::assistant(text),
        stop_reason,
        tool_calls,
        usage: AgentUsage {
            input_tokens: body
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            output_tokens: body
                .pointer("/usage/output_tokens")
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

fn parse_function_call(item: &Value) -> Result<AgentToolCall, ProtocolError> {
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            error(
                "agent.adapter.invalid_response",
                ProtocolErrorClass::Protocol,
                "function_call is missing call_id",
            )
        })?;
    let name = item.get("name").and_then(Value::as_str).ok_or_else(|| {
        error(
            "agent.adapter.invalid_response",
            ProtocolErrorClass::Protocol,
            "function_call is missing name",
        )
    })?;
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let input = serde_json::from_str(arguments).map_err(|_| {
        error(
            "agent.adapter.invalid_response",
            ProtocolErrorClass::Protocol,
            "function_call arguments are invalid JSON",
        )
    })?;
    Ok(AgentToolCall {
        call_id: call_id.into(),
        name: name.into(),
        input,
    })
}

fn parse_responses_sse(body: &str) -> Result<Vec<ModelStreamEvent>, ProtocolError> {
    let mut events = Vec::new();
    let mut sequence = 0_u64;
    for data in body
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
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
        sequence += 1;
        if let Some(text) = value
            .pointer("/delta")
            .and_then(Value::as_str)
            .or_else(|| value.get("text").and_then(Value::as_str))
            .or_else(|| {
                value
                    .pointer("/response/output_text")
                    .and_then(Value::as_str)
            })
        {
            events.push(ModelStreamEvent::MessageDelta {
                sequence,
                text: text.into(),
            });
            continue;
        }
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(
            event_type,
            "response.output_text.delta" | "response.content_part.delta"
        ) && let Some(text) = value.get("delta").and_then(Value::as_str)
        {
            events.push(ModelStreamEvent::MessageDelta {
                sequence,
                text: text.into(),
            });
        } else if event_type == "response.reasoning_summary_text.delta"
            && let Some(text) = value.get("delta").and_then(Value::as_str)
        {
            events.push(ModelStreamEvent::ReasoningDelta {
                sequence,
                text: text.into(),
            });
        }
    }
    Ok(events)
}

fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn status_error(status: StatusCode) -> ProtocolError {
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

fn transport_error(value: &reqwest::Error) -> ProtocolError {
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

fn invalid_request(message: impl Into<String>) -> ProtocolError {
    error(
        "agent.adapter.invalid_request",
        ProtocolErrorClass::NonRetryable,
        message,
    )
}

fn error(
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    use mutsuki_agent_adapter_api::{CredentialBroker, CredentialFuture, CredentialValue};
    use mutsuki_agent_contracts::{
        AgentToolDescriptor, CredentialRef, ModelGenerateRequest, ToolSideEffect,
    };

    use super::*;

    struct TestCredentials;

    impl CredentialBroker for TestCredentials {
        fn resolve(&self, _credential: CredentialRef) -> CredentialFuture {
            Box::pin(async { CredentialValue::new("TEST_SECRET") })
        }
    }

    fn request_with_tools() -> ModelGenerateRequest {
        let mut tool = AgentToolDescriptor::new("echo", "test.echo@1", "echo value");
        tool.side_effect = ToolSideEffect::None;
        tool.input_schema = json!({"type": "object", "properties": {"text": {"type": "string"}}});
        ModelGenerateRequest {
            request: mutsuki_agent_contracts::AgentModelGenerateRequest {
                model: "gpt-4.1".into(),
                messages: vec![
                    AgentMessage::system("be brief"),
                    AgentMessage::user("hello"),
                ],
                temperature: None,
                max_output_tokens: Some(128),
                provider_hint: None,
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            },
            tools: vec![tool],
            structured_output: None,
            reasoning: None,
        }
    }

    #[test]
    fn payload_maps_system_tools_and_function_call_loop() {
        let mut req = request_with_tools();
        let call = AgentToolCall {
            call_id: "call_1".into(),
            name: "echo".into(),
            input: json!({"text": "hi"}),
        };
        let mut assistant = AgentMessage::assistant("");
        assistant.metadata = Some(json!({"tool_calls": [call.clone()]}));
        req.request.messages.push(assistant);
        req.request.messages.push(AgentMessage {
            role: AgentRole::Tool,
            content: "hi".into(),
            name: Some("echo".into()),
            metadata: Some(json!({"call_id": "call_1"})),
            parts: Vec::new(),
        });

        let payload = responses_payload(req, false).unwrap();
        assert_eq!(payload["instructions"], "be brief");
        assert_eq!(payload["tools"][0]["type"], "function");
        assert_eq!(payload["tools"][0]["name"], "echo");
        let input = payload["input"].as_array().unwrap();
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call" && item["call_id"] == "call_1")
        );
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_1")
        );
    }

    #[test]
    fn payload_rejects_orphan_function_call_output() {
        let mut req = request_with_tools();
        req.request.messages.push(AgentMessage {
            role: AgentRole::Tool,
            content: "oops".into(),
            name: Some("echo".into()),
            metadata: Some(json!({"call_id": "missing"})),
            parts: Vec::new(),
        });
        let err = responses_payload(req, false).unwrap_err();
        assert_eq!(err.code, "agent.adapter.invalid_request");
    }

    #[test]
    fn parse_maps_message_and_function_call_output() {
        let body = json!({
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "need tool"}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_abc",
                    "name": "echo",
                    "arguments": "{\"text\":\"x\"}"
                }
            ],
            "usage": {"input_tokens": 10, "output_tokens": 4, "total_tokens": 14}
        });
        let result = parse_responses_body(body).unwrap();
        assert_eq!(result.message.content, "need tool");
        assert_eq!(result.stop_reason, AgentModelStopReason::ToolCalls);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].call_id, "call_abc");
        assert_eq!(result.tool_calls[0].name, "echo");
        assert_eq!(result.usage.total_tokens, 14);
    }

    #[tokio::test]
    async fn generate_hits_responses_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 32_768];
            let read = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.contains("POST /v1/responses"));
            assert!(request.to_ascii_lowercase().contains("authorization: bearer test_secret"));
            assert!(request.contains("\"instructions\":\"be brief\""));
            let payload = r#"{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#;
            let body = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            stream.write_all(body.as_bytes()).unwrap();
        });

        let adapter = OpenAiResponsesAdapter::new(
            OpenAiResponsesAdapter::default_descriptor(),
            Arc::new(TestCredentials),
        )
        .unwrap();
        let result = adapter
            .generate(
                OpenAiResponsesAdapter::provider_descriptor(
                    "openai-responses-local",
                    &format!("http://{address}/v1"),
                    CredentialRef {
                        credential_id: "test".into(),
                        revision: 1,
                    },
                    "gpt-4.1",
                ),
                request_with_tools(),
            )
            .await
            .unwrap();
        assert_eq!(result.message.content, "ok");
        server.join().unwrap();
    }
}
