pub use mutsuki_plugin_agent_model_gateway::{ModelGateway, ModelProvider};

use mutsuki_agent_protocol::{
    AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult, AgentModelStopReason,
    AgentResult, AgentRole, AgentUsage,
};

#[derive(Default)]
pub struct MockModelProvider;

impl ModelProvider for MockModelProvider {
    fn provider_id(&self) -> &str {
        "mock"
    }

    fn generate(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == AgentRole::User)
            .map(|message| message.content.as_str())
            .unwrap_or("");
        let content = if last_user.is_empty() {
            "No user message provided.".to_string()
        } else {
            format!("Echo: {last_user}")
        };
        let input_tokens = request
            .messages
            .iter()
            .map(|message| message.content.len() as u64)
            .sum::<u64>();
        let output_tokens = content.len() as u64;
        Ok(AgentModelGenerateResult {
            message: AgentMessage::assistant(content),
            stop_reason: AgentModelStopReason::Stop,
            tool_calls: Vec::new(),
            usage: AgentUsage {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens.saturating_add(output_tokens),
            },
            cost_microunits: 0,
            raw: None,
            output_resource: None,
        })
    }
}
