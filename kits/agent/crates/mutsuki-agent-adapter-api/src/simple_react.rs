//! Simple ReAct: model ↔ tool loop only (no session / approval / memory).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use mutsuki_agent_contracts::{
    AgentMessage, AgentModelGenerateRequest, AgentModelStopReason, AgentRole, AgentToolCall,
    AgentToolDescriptor, AgentToolResultMetadata, AgentUsage, ModelGenerateRequest, ProtocolError,
    ProtocolErrorClass, ProviderInstanceDescriptor,
};
use serde_json::json;

use crate::ModelProtocolAdapter;

pub type ReactToolFuture =
    Pin<Box<dyn Future<Output = Result<AgentMessage, ProtocolError>> + Send + 'static>>;

pub trait ReactToolExecutor: Send + Sync {
    fn execute(&self, call: AgentToolCall) -> ReactToolFuture;
}

pub struct FnToolExecutor<F>(F)
where
    F: Fn(AgentToolCall) -> ReactToolFuture + Send + Sync + 'static;

impl<F> FnToolExecutor<F>
where
    F: Fn(AgentToolCall) -> ReactToolFuture + Send + Sync + 'static,
{
    pub fn new(inner: F) -> Self {
        Self(inner)
    }
}

impl<F> ReactToolExecutor for FnToolExecutor<F>
where
    F: Fn(AgentToolCall) -> ReactToolFuture + Send + Sync + 'static,
{
    fn execute(&self, call: AgentToolCall) -> ReactToolFuture {
        (self.0)(call)
    }
}

/// Tool result with causal `call_id` metadata.
pub fn tool_result_message(
    call: &AgentToolCall,
    content: impl Into<String>,
) -> Result<AgentMessage, ProtocolError> {
    tool_message(call, content.into(), false, None)
}

/// Tool error returned to the model (does not abort the loop).
pub fn tool_error_message(
    call: &AgentToolCall,
    code: impl Into<String>,
    message: impl Into<String>,
) -> Result<AgentMessage, ProtocolError> {
    let err = mutsuki_agent_contracts::AgentError::new(code, message);
    tool_message(call, err.message.clone(), true, Some(err))
}

fn tool_message(
    call: &AgentToolCall,
    content: String,
    is_error: bool,
    error: Option<mutsuki_agent_contracts::AgentError>,
) -> Result<AgentMessage, ProtocolError> {
    if call.call_id.trim().is_empty() {
        return Err(err(
            "agent.react.invalid_tool_call",
            "tool call_id must be non-empty",
        ));
    }
    let metadata = AgentToolResultMetadata {
        call_id: call.call_id.clone(),
        output_ref: None,
        is_error,
        error,
    };
    Ok(AgentMessage {
        role: AgentRole::Tool,
        content,
        name: Some(call.name.clone()),
        metadata: Some(serde_json::to_value(metadata).map_err(|_| {
            err(
                "agent.react.invalid_tool_result",
                "failed to encode tool result metadata",
            )
        })?),
        parts: Vec::new(),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimpleReactRequest {
    pub model: String,
    pub messages: Vec<AgentMessage>,
    pub max_steps: u32,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u64>,
}

impl SimpleReactRequest {
    pub fn new(model: impl Into<String>, messages: Vec<AgentMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_steps: 8,
            temperature: None,
            max_output_tokens: None,
        }
    }

    pub fn with_max_steps(mut self, max_steps: u32) -> Self {
        self.max_steps = max_steps;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimpleReactStatus {
    Completed,
    BudgetExceeded,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimpleReactResult {
    pub messages: Vec<AgentMessage>,
    pub final_text: String,
    pub model_steps: u32,
    pub usage: AgentUsage,
    pub stop_reason: AgentModelStopReason,
    pub status: SimpleReactStatus,
}

/// Minimal ReAct over any `ModelProtocolAdapter`.
pub struct SimpleReact {
    adapter: Arc<dyn ModelProtocolAdapter>,
    provider: ProviderInstanceDescriptor,
    tools: Vec<AgentToolDescriptor>,
    executor: Arc<dyn ReactToolExecutor>,
}

impl SimpleReact {
    pub fn new(
        adapter: Arc<dyn ModelProtocolAdapter>,
        provider: ProviderInstanceDescriptor,
        tools: Vec<AgentToolDescriptor>,
        executor: Arc<dyn ReactToolExecutor>,
    ) -> Result<Self, ProtocolError> {
        if provider.adapter_id != adapter.descriptor().adapter_id {
            return Err(err(
                "agent.react.adapter_mismatch",
                format!(
                    "provider `{}` selects adapter `{}` but received `{}`",
                    provider.provider_id,
                    provider.adapter_id,
                    adapter.descriptor().adapter_id
                ),
            ));
        }
        Ok(Self {
            adapter,
            provider,
            tools,
            executor,
        })
    }

    pub async fn run(
        &self,
        request: SimpleReactRequest,
    ) -> Result<SimpleReactResult, ProtocolError> {
        if request.model.trim().is_empty() {
            return Err(err("agent.react.invalid_request", "model is required"));
        }
        if request.max_steps == 0 {
            return Ok(SimpleReactResult {
                messages: request.messages,
                final_text: String::new(),
                model_steps: 0,
                usage: AgentUsage::default(),
                stop_reason: AgentModelStopReason::Other,
                status: SimpleReactStatus::BudgetExceeded,
            });
        }

        let mut messages = request.messages;
        let mut usage = AgentUsage::default();
        let mut model_steps = 0;
        let mut last_stop = AgentModelStopReason::Stop;

        for _ in 0..request.max_steps {
            model_steps += 1;
            let generated = self
                .adapter
                .generate(
                    self.provider.clone(),
                    ModelGenerateRequest {
                        request: AgentModelGenerateRequest {
                            model: request.model.clone(),
                            messages: messages.clone(),
                            temperature: request.temperature,
                            max_output_tokens: request.max_output_tokens,
                            provider_hint: Some(self.provider.provider_id.clone()),
                            metadata: None,
                            result_protocol_id: None,
                            result_context: None,
                            session_id: None,
                        },
                        tools: self.tools.clone(),
                        structured_output: None,
                        reasoning: None,
                    },
                )
                .await?;
            usage.add(&generated.usage);
            last_stop = generated.stop_reason.clone();

            let tool_calls = generated.tool_calls;
            let mut assistant = generated.message;
            if tool_calls.is_empty() {
                if generated.stop_reason == AgentModelStopReason::ToolCalls {
                    return Err(err(
                        "agent.react.invalid_model_result",
                        "model declared tool_calls without returning a tool call",
                    ));
                }
                let final_text = assistant.content.clone();
                messages.push(assistant);
                return Ok(SimpleReactResult {
                    messages,
                    final_text,
                    model_steps,
                    usage,
                    stop_reason: last_stop,
                    status: SimpleReactStatus::Completed,
                });
            }

            assistant.metadata = Some(json!({"tool_calls": &tool_calls}));
            messages.push(assistant);
            for call in tool_calls {
                let tool_message = self.executor.execute(call).await?;
                if tool_message.role != AgentRole::Tool
                    || tool_message
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("call_id"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .is_empty()
                {
                    return Err(err(
                        "agent.react.invalid_tool_result",
                        "tool executor must return AgentRole::Tool with call_id",
                    ));
                }
                messages.push(tool_message);
            }
        }

        Ok(SimpleReactResult {
            messages,
            final_text: String::new(),
            model_steps,
            usage,
            stop_reason: last_stop,
            status: SimpleReactStatus::BudgetExceeded,
        })
    }
}

fn err(code: impl Into<String>, message: impl Into<String>) -> ProtocolError {
    ProtocolError {
        code: code.into(),
        class: ProtocolErrorClass::NonRetryable,
        message: message.into(),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelAdapterFuture, ModelStreamFuture};
    use mutsuki_agent_contracts::{
        AgentModelGenerateResult, CredentialRef, ModelCapability, ModelProtocolAdapterDescriptor,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScriptedAdapter {
        descriptor: ModelProtocolAdapterDescriptor,
        calls: AtomicUsize,
    }

    impl ModelProtocolAdapter for ScriptedAdapter {
        fn descriptor(&self) -> &ModelProtocolAdapterDescriptor {
            &self.descriptor
        }

        fn generate(
            &self,
            _provider: ProviderInstanceDescriptor,
            request: ModelGenerateRequest,
        ) -> ModelAdapterFuture {
            let step = self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if step == 0 {
                    Ok(AgentModelGenerateResult {
                        message: AgentMessage::assistant(""),
                        stop_reason: AgentModelStopReason::ToolCalls,
                        tool_calls: vec![AgentToolCall {
                            call_id: "call-1".into(),
                            name: "echo".into(),
                            input: json!({"text": "ping"}),
                        }],
                        usage: AgentUsage {
                            input_tokens: 2,
                            output_tokens: 1,
                            total_tokens: 3,
                        },
                        cost_microunits: 0,
                        raw: None,
                        output_resource: None,
                    })
                } else {
                    assert!(
                        request
                            .request
                            .messages
                            .iter()
                            .any(|m| m.role == AgentRole::Tool)
                    );
                    Ok(AgentModelGenerateResult {
                        message: AgentMessage::assistant("pong"),
                        stop_reason: AgentModelStopReason::Stop,
                        tool_calls: Vec::new(),
                        usage: AgentUsage {
                            input_tokens: 4,
                            output_tokens: 1,
                            total_tokens: 5,
                        },
                        cost_microunits: 0,
                        raw: None,
                        output_resource: None,
                    })
                }
            })
        }

        fn stream(
            &self,
            _provider: ProviderInstanceDescriptor,
            _request: ModelGenerateRequest,
        ) -> ModelStreamFuture {
            Box::pin(async {
                Err(ProtocolError {
                    code: "unused".into(),
                    class: ProtocolErrorClass::NonRetryable,
                    message: "unused".into(),
                    retry_after_ms: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn simple_react_runs_one_tool_round_then_completes() {
        let adapter: Arc<dyn ModelProtocolAdapter> = Arc::new(ScriptedAdapter {
            descriptor: ModelProtocolAdapterDescriptor {
                adapter_id: "test".into(),
                protocol: "test".into(),
                version: "1".into(),
                runner_id: "test.runner".into(),
                capability: ModelCapability {
                    tools: true,
                    ..ModelCapability::default()
                },
            },
            calls: AtomicUsize::new(0),
        });
        let provider = ProviderInstanceDescriptor {
            provider_id: "p1".into(),
            adapter_id: "test".into(),
            endpoint: "https://example.invalid".into(),
            credential: CredentialRef {
                credential_id: "c1".into(),
                revision: 1,
            },
            models: Default::default(),
            headers: Default::default(),
            compatibility: Default::default(),
            remote_execution_allowed: true,
        };
        let executor = Arc::new(FnToolExecutor::new(|call| {
            Box::pin(async move { tool_result_message(&call, "pong-tool") })
        }));
        let mut tool = AgentToolDescriptor::new("echo", "test.echo@1", "echo");
        tool.input_schema = json!({"type": "object"});
        let result = SimpleReact::new(adapter, provider, vec![tool], executor)
            .unwrap()
            .run(SimpleReactRequest::new("model", vec![AgentMessage::user("hi")]).with_max_steps(4))
            .await
            .unwrap();
        assert_eq!(result.status, SimpleReactStatus::Completed);
        assert_eq!(result.final_text, "pong");
        assert_eq!(result.model_steps, 2);
        assert_eq!(result.usage.total_tokens, 8);
    }
}
