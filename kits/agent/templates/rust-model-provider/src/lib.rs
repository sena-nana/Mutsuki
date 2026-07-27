use mutsuki_agent_protocol::{
    AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult, AgentResult, AgentUsage,
};
use mutsuki_plugin_agent_model_gateway::ModelProvider;

pub struct MyModelProvider;

impl ModelProvider for MyModelProvider {
    fn provider_id(&self) -> &str {
        "my-provider"
    }

    fn generate(&self, request: AgentModelGenerateRequest) -> AgentResult<AgentModelGenerateResult> {
        Ok(AgentModelGenerateResult {
            message: AgentMessage::assistant(format!("handled by {}", request.model)),
            usage: AgentUsage::default(),
            raw: None,
            output_resource: None,
        })
    }
}
