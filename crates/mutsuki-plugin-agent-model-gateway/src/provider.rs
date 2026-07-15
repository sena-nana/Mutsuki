use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_protocol::{
    AgentError, AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult,
    AgentModelStopReason, AgentModelStreamRequest, AgentModelStreamResult, AgentResult, AgentRole,
    AgentToolCall, AgentUsage,
};
use mutsuki_agent_sdk::stream_resource_ref;

use crate::PLUGIN_ID;

pub trait ModelProvider: Send + Sync {
    fn provider_id(&self) -> &str;
    fn generate(&self, request: AgentModelGenerateRequest)
    -> AgentResult<AgentModelGenerateResult>;

    fn generate_async(&self, request: AgentModelGenerateRequest) -> ModelProviderFuture {
        let result = self.generate(request);
        Box::pin(async move { result })
    }

    fn execution(&self) -> ModelProviderExecution {
        ModelProviderExecution::InlineDeterministic
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelProviderExecution {
    InlineDeterministic,
    HttpEffect,
}

pub type ModelProviderFuture =
    Pin<Box<dyn Future<Output = AgentResult<AgentModelGenerateResult>> + Send + 'static>>;

#[derive(Clone)]
pub struct ModelGateway {
    default_provider: String,
    providers: Arc<Mutex<BTreeMap<String, Arc<dyn ModelProvider>>>>,
    next_stream: Arc<AtomicU64>,
    streams: Arc<Mutex<BTreeMap<String, String>>>,
}

impl Default for ModelGateway {
    fn default() -> Self {
        Self {
            default_provider: String::new(),
            providers: Arc::new(Mutex::new(BTreeMap::new())),
            next_stream: Arc::new(AtomicU64::new(0)),
            streams: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl ModelGateway {
    pub fn with_default_provider(default_provider: impl Into<String>) -> Self {
        Self {
            default_provider: default_provider.into(),
            ..Self::default()
        }
    }

    pub fn register(&self, provider: Arc<dyn ModelProvider>) {
        self.providers
            .lock()
            .expect("model gateway mutex poisoned")
            .insert(provider.provider_id().to_string(), provider);
    }

    pub fn generate(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        let provider = self.inline_provider(&request)?;
        provider.generate(request)
    }

    pub fn stream(&self, request: AgentModelStreamRequest) -> AgentResult<AgentModelStreamResult> {
        let generated = self.generate(request.request)?;
        let stream_id = self.next_stream.fetch_add(1, Ordering::Relaxed) + 1;
        let slot = format!("stream-{stream_id}");
        self.streams
            .lock()
            .expect("model stream mutex poisoned")
            .insert(slot.clone(), generated.message.content.clone());
        Ok(AgentModelStreamResult {
            stream: stream_resource_ref(PLUGIN_ID, slot),
            stop_reason: generated.stop_reason,
            tool_calls: generated.tool_calls,
            usage: generated.usage,
            cost_microunits: generated.cost_microunits,
        })
    }

    pub async fn generate_async(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        let provider = self.inline_provider(&request)?;
        provider.generate_async(request).await
    }

    pub(crate) fn generate_effect(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        let provider = self.effect_provider(&request)?;
        provider.generate(request)
    }

    pub(crate) async fn generate_effect_async(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        let provider = self.effect_provider(&request)?;
        provider.generate_async(request).await
    }

    pub(crate) fn stream_effect(
        &self,
        request: AgentModelStreamRequest,
    ) -> AgentResult<AgentModelStreamResult> {
        let generated = self.generate_effect(request.request)?;
        Ok(self.store_stream(generated))
    }

    pub(crate) async fn stream_effect_async(
        &self,
        request: AgentModelStreamRequest,
    ) -> AgentResult<AgentModelStreamResult> {
        let generated = self.generate_effect_async(request.request).await?;
        Ok(self.store_stream(generated))
    }

    pub async fn stream_async(
        &self,
        request: AgentModelStreamRequest,
    ) -> AgentResult<AgentModelStreamResult> {
        let generated = self.generate_async(request.request).await?;
        let stream_id = self.next_stream.fetch_add(1, Ordering::Relaxed) + 1;
        let slot = format!("stream-{stream_id}");
        self.streams
            .lock()
            .expect("model stream mutex poisoned")
            .insert(slot.clone(), generated.message.content.clone());
        Ok(AgentModelStreamResult {
            stream: stream_resource_ref(PLUGIN_ID, slot),
            stop_reason: generated.stop_reason,
            tool_calls: generated.tool_calls,
            usage: generated.usage,
            cost_microunits: generated.cost_microunits,
        })
    }

    pub fn read_stream(&self, stream: &mutsuki_agent_protocol::ResourceRef) -> Option<String> {
        self.streams
            .lock()
            .expect("model stream mutex poisoned")
            .get(&stream.resource_id.slot_id)
            .cloned()
    }

    pub fn provider_execution(
        &self,
        request: &AgentModelGenerateRequest,
    ) -> AgentResult<ModelProviderExecution> {
        Ok(self.provider(request)?.execution())
    }

    pub fn health_snapshot(&self) -> ModelGatewayHealth {
        let providers = self.providers.lock().expect("model gateway mutex poisoned");
        ModelGatewayHealth {
            default_provider: self.default_provider.clone(),
            ready: providers.contains_key(&self.default_provider),
            providers: providers.keys().cloned().collect(),
        }
    }

    fn provider(&self, request: &AgentModelGenerateRequest) -> AgentResult<Arc<dyn ModelProvider>> {
        let provider_id = request
            .provider_hint
            .clone()
            .unwrap_or_else(|| self.default_provider.clone());
        self.providers
            .lock()
            .expect("model gateway mutex poisoned")
            .get(&provider_id)
            .cloned()
            .ok_or_else(|| {
                AgentError::provider_unavailable(format!(
                    "model provider `{provider_id}` not registered"
                ))
            })
    }

    fn inline_provider(
        &self,
        request: &AgentModelGenerateRequest,
    ) -> AgentResult<Arc<dyn ModelProvider>> {
        let provider = self.provider(request)?;
        if provider.execution() != ModelProviderExecution::InlineDeterministic {
            return Err(AgentError::new(
                "agent.model.effect_required",
                "effectful model provider must run through the model effect runner",
            ));
        }
        Ok(provider)
    }

    fn effect_provider(
        &self,
        request: &AgentModelGenerateRequest,
    ) -> AgentResult<Arc<dyn ModelProvider>> {
        let provider = self.provider(request)?;
        if provider.execution() != ModelProviderExecution::HttpEffect {
            return Err(AgentError::new(
                "agent.model.effect_provider_required",
                "model effect runner requires an effectful provider",
            ));
        }
        Ok(provider)
    }

    fn store_stream(&self, generated: AgentModelGenerateResult) -> AgentModelStreamResult {
        let stream_id = self.next_stream.fetch_add(1, Ordering::Relaxed) + 1;
        let slot = format!("stream-{stream_id}");
        self.streams
            .lock()
            .expect("model stream mutex poisoned")
            .insert(slot.clone(), generated.message.content.clone());
        AgentModelStreamResult {
            stream: stream_resource_ref(PLUGIN_ID, slot),
            stop_reason: generated.stop_reason,
            tool_calls: generated.tool_calls,
            usage: generated.usage,
            cost_microunits: generated.cost_microunits,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelGatewayHealth {
    pub default_provider: String,
    pub ready: bool,
    pub providers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpModelProviderOptions {
    pub provider_id: String,
    pub endpoint: String,
    pub default_model: String,
    pub timeout_ms: u64,
    pub max_retries: u8,
}

impl HttpModelProviderOptions {
    pub fn validate(&self) -> AgentResult<()> {
        if self.provider_id.trim().is_empty()
            || self.endpoint.trim().is_empty()
            || self.default_model.trim().is_empty()
        {
            return Err(AgentError::invalid_input(
                "provider_id, endpoint and default_model are required",
            ));
        }
        let endpoint = reqwest::Url::parse(&self.endpoint)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let loopback = matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if endpoint.scheme() != "https" && !loopback {
            return Err(AgentError::invalid_input(
                "model endpoint must use https except for loopback tests",
            ));
        }
        if endpoint.username() != "" || endpoint.password().is_some() {
            return Err(AgentError::invalid_input(
                "model endpoint must not contain credentials",
            ));
        }
        if self.timeout_ms == 0 {
            return Err(AgentError::invalid_input("timeout_ms must be positive"));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct HttpModelProvider {
    options: HttpModelProviderOptions,
    api_key: Arc<SecretValue>,
    client: reqwest::Client,
}

impl std::fmt::Debug for HttpModelProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpModelProvider")
            .field("options", &self.options)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl HttpModelProvider {
    pub fn new(options: HttpModelProviderOptions, api_key: impl Into<String>) -> AgentResult<Self> {
        options.validate()?;
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(AgentError::provider_unavailable(
                "model provider secret is missing",
            ));
        }
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| AgentError::provider_unavailable(error.to_string()))?;
        Ok(Self {
            options,
            api_key: Arc::new(SecretValue(api_key)),
            client,
        })
    }

    fn request_blocking(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        let payload = self.payload(&request);
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|error| AgentError::provider_unavailable(error.to_string()))?;
        let attempts = self.options.max_retries.saturating_add(1);
        for attempt in 0..attempts {
            let response = client
                .post(&self.options.endpoint)
                .bearer_auth(self.api_key.expose())
                .timeout(std::time::Duration::from_millis(self.options.timeout_ms))
                .json(&payload)
                .send();
            match response {
                Err(error)
                    if attempt + 1 < attempts && (error.is_timeout() || error.is_connect()) =>
                {
                    continue;
                }
                Err(error) if error.is_timeout() => {
                    return Err(AgentError::new(
                        "agent.model.timeout",
                        "model request timed out",
                    ));
                }
                Err(error) => {
                    return Err(AgentError::provider_unavailable(format!(
                        "model transport failed: {}",
                        transport_error_kind(&error)
                    )));
                }
                Ok(response) if response.status().is_success() => {
                    let body: serde_json::Value = response.json().map_err(|error| {
                        AgentError::new("agent.model.invalid_response", error.to_string())
                    })?;
                    return parse_http_result(body);
                }
                Ok(response)
                    if (response.status().as_u16() == 429
                        || response.status().is_server_error())
                        && attempt + 1 < attempts =>
                {
                    continue;
                }
                Ok(response) => {
                    return Err(AgentError::new(
                        "agent.model.http_status",
                        format!(
                            "model endpoint returned HTTP {}",
                            response.status().as_u16()
                        ),
                    ));
                }
            }
        }
        Err(AgentError::provider_unavailable("model retry exhausted"))
    }

    fn payload(&self, request: &AgentModelGenerateRequest) -> serde_json::Value {
        let model = if request.model.trim().is_empty() {
            self.options.default_model.clone()
        } else {
            request.model.clone()
        };
        serde_json::json!({
            "model": model,
            "messages": request.messages.iter().map(http_message_payload).collect::<Vec<_>>(),
            "stream": false,
            "temperature": request.temperature,
            "max_output_tokens": request.max_output_tokens,
        })
    }

    async fn request(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        let payload = self.payload(&request);
        let attempts = self.options.max_retries.saturating_add(1);
        for attempt in 0..attempts {
            let send = self
                .client
                .post(&self.options.endpoint)
                .bearer_auth(self.api_key.expose())
                .json(&payload)
                .send();
            let response = tokio::time::timeout(
                std::time::Duration::from_millis(self.options.timeout_ms),
                send,
            )
            .await;
            match response {
                Err(_) if attempt + 1 < attempts => continue,
                Err(_) => {
                    return Err(AgentError::new(
                        "agent.model.timeout",
                        "model request timed out",
                    ));
                }
                Ok(Err(_)) if attempt + 1 < attempts => continue,
                Ok(Err(error)) => {
                    return Err(AgentError::provider_unavailable(format!(
                        "model transport failed: {}",
                        transport_error_kind(&error)
                    )));
                }
                Ok(Ok(response)) if response.status().is_success() => {
                    let body: serde_json::Value = response.json().await.map_err(|error| {
                        AgentError::new("agent.model.invalid_response", error.to_string())
                    })?;
                    return parse_http_result(body);
                }
                Ok(Ok(response))
                    if (response.status().as_u16() == 429
                        || response.status().is_server_error())
                        && attempt + 1 < attempts =>
                {
                    continue;
                }
                Ok(Ok(response)) => {
                    return Err(AgentError::new(
                        "agent.model.http_status",
                        format!(
                            "model endpoint returned HTTP {}",
                            response.status().as_u16()
                        ),
                    ));
                }
            }
        }
        Err(AgentError::provider_unavailable("model retry exhausted"))
    }
}

fn http_message_payload(message: &AgentMessage) -> serde_json::Value {
    let role = match message.role {
        AgentRole::System => "system",
        AgentRole::User => "user",
        AgentRole::Assistant => "assistant",
        AgentRole::Tool => "tool",
    };
    let mut payload = serde_json::Map::from_iter([
        ("role".into(), serde_json::Value::String(role.into())),
        (
            "content".into(),
            serde_json::Value::String(message.content.clone()),
        ),
    ]);
    if let Some(name) = &message.name {
        payload.insert("name".into(), serde_json::Value::String(name.clone()));
    }
    if message.role == AgentRole::Assistant
        && let Some(tool_calls) = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("tool_calls"))
            .and_then(|calls| serde_json::from_value::<Vec<AgentToolCall>>(calls.clone()).ok())
    {
        payload.insert(
            "tool_calls".into(),
            serde_json::Value::Array(
                tool_calls
                    .into_iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.call_id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.input.to_string(),
                            }
                        })
                    })
                    .collect(),
            ),
        );
    }
    if message.role == AgentRole::Tool
        && let Some(call_id) = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("call_id"))
            .and_then(serde_json::Value::as_str)
    {
        payload.insert(
            "tool_call_id".into(),
            serde_json::Value::String(call_id.into()),
        );
    }
    serde_json::Value::Object(payload)
}

impl ModelProvider for HttpModelProvider {
    fn provider_id(&self) -> &str {
        &self.options.provider_id
    }

    fn generate(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        self.request_blocking(request)
    }

    fn generate_async(&self, request: AgentModelGenerateRequest) -> ModelProviderFuture {
        let provider = self.clone();
        Box::pin(async move { provider.request(request).await })
    }

    fn execution(&self) -> ModelProviderExecution {
        ModelProviderExecution::HttpEffect
    }
}

struct SecretValue(String);

impl SecretValue {
    fn expose(&self) -> &str {
        &self.0
    }
}

fn transport_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else {
        "request"
    }
}

fn parse_http_result(body: serde_json::Value) -> AgentResult<AgentModelGenerateResult> {
    let content = body
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let tool_calls = body
        .pointer("/choices/0/message/tool_calls")
        .and_then(serde_json::Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|call| {
                    let call_id = call
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            AgentError::new(
                                "agent.model.invalid_response",
                                "tool call is missing id",
                            )
                        })?;
                    let name = call
                        .pointer("/function/name")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            AgentError::new(
                                "agent.model.invalid_response",
                                "tool call is missing function.name",
                            )
                        })?;
                    let arguments = call
                        .pointer("/function/arguments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let input = match arguments {
                        serde_json::Value::String(arguments) => serde_json::from_str(&arguments)
                            .map_err(|error| {
                                AgentError::new(
                                    "agent.model.invalid_response",
                                    format!("tool call arguments are invalid JSON: {error}"),
                                )
                            })?,
                        value => value,
                    };
                    Ok(AgentToolCall {
                        call_id: call_id.into(),
                        name: name.into(),
                        input,
                    })
                })
                .collect::<AgentResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    if content.is_empty() && tool_calls.is_empty() {
        return Err(AgentError::new(
            "agent.model.invalid_response",
            "response contains neither message content nor tool calls",
        ));
    }
    let stop_reason = match body
        .pointer("/choices/0/finish_reason")
        .and_then(serde_json::Value::as_str)
    {
        Some("tool_calls") => AgentModelStopReason::ToolCalls,
        Some("length") => AgentModelStopReason::Length,
        Some("content_filter") => AgentModelStopReason::ContentFilter,
        Some("stop") | None => AgentModelStopReason::Stop,
        Some(_) => AgentModelStopReason::Other,
    };
    let usage = AgentUsage {
        input_tokens: body
            .pointer("/usage/prompt_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        output_tokens: body
            .pointer("/usage/completion_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        total_tokens: body
            .pointer("/usage/total_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
    };
    Ok(AgentModelGenerateResult {
        message: AgentMessage::assistant(content),
        stop_reason,
        tool_calls,
        usage,
        cost_microunits: 0,
        raw: None,
        output_resource: None,
    })
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn production_gateway_has_no_implicit_provider() {
        let gateway = ModelGateway::default();
        let health = gateway.health_snapshot();
        assert!(!health.ready);
        assert!(health.providers.is_empty());
    }

    #[test]
    fn http_result_preserves_typed_tool_call_and_usage() {
        let result = parse_http_result(serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"value\":\"ping\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 4, "completion_tokens": 3, "total_tokens": 7}
        }))
        .unwrap();

        assert_eq!(result.stop_reason, AgentModelStopReason::ToolCalls);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].call_id, "call-1");
        assert_eq!(result.tool_calls[0].name, "echo");
        assert_eq!(
            result.tool_calls[0].input,
            serde_json::json!({"value": "ping"})
        );
        assert_eq!(result.usage.total_tokens, 7);
    }

    #[test]
    fn inline_gateway_rejects_http_effect_provider_without_network_io() {
        let gateway = ModelGateway::with_default_provider("http");
        gateway.register(Arc::new(
            HttpModelProvider::new(
                HttpModelProviderOptions {
                    provider_id: "http".into(),
                    endpoint: "http://127.0.0.1:9/generate".into(),
                    default_model: "test".into(),
                    timeout_ms: 10,
                    max_retries: 0,
                },
                "TEST_SECRET",
            )
            .unwrap(),
        ));

        let error = gateway
            .generate(AgentModelGenerateRequest {
                model: "test".into(),
                messages: vec![AgentMessage::user("must route as effect")],
                temperature: None,
                max_output_tokens: None,
                provider_hint: None,
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            })
            .unwrap_err();

        assert_eq!(error.code, "agent.model.effect_required");
    }

    #[tokio::test]
    async fn http_provider_retries_once_and_redacts_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let server_attempts = attempts.clone();
        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = [0_u8; 4096];
                let read = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test_secret")
                );
                let attempt = server_attempts.fetch_add(1, Ordering::SeqCst);
                let (status, body) = if attempt == 0 {
                    ("500 Internal Server Error", "{}".to_string())
                } else {
                    (
                        "200 OK",
                        serde_json::json!({
                            "choices": [{"message": {"content": "real response"}}],
                            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
                        })
                        .to_string(),
                    )
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let provider = HttpModelProvider::new(
            HttpModelProviderOptions {
                provider_id: "http".into(),
                endpoint: format!("http://{address}/generate"),
                default_model: "test".into(),
                timeout_ms: 1_000,
                max_retries: 1,
            },
            "TEST_SECRET",
        )
        .unwrap();
        assert!(!format!("{provider:?}").contains("TEST_SECRET"));

        let result = provider
            .generate_async(AgentModelGenerateRequest {
                model: "test".into(),
                messages: vec![AgentMessage::user("hello")],
                temperature: None,
                max_output_tokens: None,
                provider_hint: Some("http".into()),
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            })
            .await
            .unwrap();

        assert_eq!(result.message.content, "real response");
        assert_eq!(result.usage.total_tokens, 5);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn http_provider_times_out_without_unbounded_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        let provider = HttpModelProvider::new(
            HttpModelProviderOptions {
                provider_id: "http".into(),
                endpoint: format!("http://{address}/generate"),
                default_model: "test".into(),
                timeout_ms: 20,
                max_retries: 0,
            },
            "TEST_SECRET",
        )
        .unwrap();
        let error = provider
            .generate_async(AgentModelGenerateRequest {
                model: "test".into(),
                messages: vec![AgentMessage::user("hello")],
                temperature: None,
                max_output_tokens: None,
                provider_hint: Some("http".into()),
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "agent.model.timeout");
        server.join().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_provider_future_is_cancellable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            accepted_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let provider = HttpModelProvider::new(
            HttpModelProviderOptions {
                provider_id: "http".into(),
                endpoint: format!("http://{address}/generate"),
                default_model: "test".into(),
                timeout_ms: 5_000,
                max_retries: 2,
            },
            "TEST_SECRET",
        )
        .unwrap();
        let task = tokio::spawn(async move {
            provider
                .generate_async(AgentModelGenerateRequest {
                    model: "test".into(),
                    messages: vec![AgentMessage::user("cancel")],
                    temperature: None,
                    max_output_tokens: None,
                    provider_hint: Some("http".into()),
                    metadata: None,
                    result_protocol_id: None,
                    result_context: None,
                    session_id: None,
                })
                .await
        });
        accepted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn http_provider_maps_non_retryable_status_without_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            )
            .unwrap();
        });
        let provider = HttpModelProvider::new(
            HttpModelProviderOptions {
                provider_id: "http".into(),
                endpoint: format!("http://{address}/generate"),
                default_model: "test".into(),
                timeout_ms: 1_000,
                max_retries: 2,
            },
            "TEST_SECRET",
        )
        .unwrap();
        let error = provider
            .generate_async(AgentModelGenerateRequest {
                model: "test".into(),
                messages: vec![AgentMessage::user("bad")],
                temperature: None,
                max_output_tokens: None,
                provider_hint: Some("http".into()),
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "agent.model.http_status");
        assert!(!error.message.contains("TEST_SECRET"));
        server.join().unwrap();
    }
}
