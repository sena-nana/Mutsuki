use mutsuki_agent_protocol::{
    AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult, AgentResult, AgentUsage,
};
use mutsuki_plugin_agent_model_gateway::ModelProvider;

pub struct StaticModelProvider;

impl ModelProvider for StaticModelProvider {
    fn provider_id(&self) -> &str {
        "static"
    }

    fn generate(&self, request: AgentModelGenerateRequest) -> AgentResult<AgentModelGenerateResult> {
        Ok(AgentModelGenerateResult {
            message: AgentMessage::assistant(format!("static model: {}", request.model)),
            usage: AgentUsage::default(),
            raw: None,
            output_resource: None,
        })
    }
}
