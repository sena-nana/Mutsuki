use mutsuki_agent_contracts::{
    AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult, AgentModelStopReason,
    AgentResult, AgentUsage,
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
