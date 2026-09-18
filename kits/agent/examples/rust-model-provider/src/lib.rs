use mutsuki_agent_contracts::{
    AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult, AgentModelStopReason,
    AgentResult, AgentUsage,
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
            // `AgentModelGenerateResult` has no `Default`, so every field is named
            // here. A real provider maps these from its own response.
            stop_reason: AgentModelStopReason::Stop,
            tool_calls: Vec::new(),
            usage: AgentUsage::default(),
            cost_microunits: 0,
            raw: None,
            output_resource: None,
        })
    }
}
