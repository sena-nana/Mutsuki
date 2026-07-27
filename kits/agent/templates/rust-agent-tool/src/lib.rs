use mutsuki_agent_protocol::AgentResult;
use mutsuki_agent_sdk::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct ToolInput {
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolOutput {
    pub text: String,
}

pub struct ToolProtocol;

impl SdkProtocol for ToolProtocol {
    const PROTOCOL_ID: &'static str = "my.tool/run@1";
}

#[agent_tool(
    name = "my.tool",
    target = ToolProtocol,
    description = "Template Agent tool",
    side_effect = "none",
    requires_approval = false,
    permissions = []
)]
pub async fn run_tool(_ctx: AgentToolContext, input: ToolInput) -> AgentResult<ToolOutput> {
    Ok(ToolOutput { text: input.text })
}
