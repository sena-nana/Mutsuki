use mutsuki_agent_protocol::AgentResult;
use mutsuki_agent_sdk::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct EchoInput {
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EchoOutput {
    pub text: String,
}

pub struct EchoProtocol;

impl SdkProtocol for EchoProtocol {
    const PROTOCOL_ID: &'static str = "example.echo/run@1";
}

#[agent_tool(
    name = "example.echo",
    target = EchoProtocol,
    description = "Echo text for Agent examples",
    side_effect = "none",
    requires_approval = false,
    permissions = []
)]
pub async fn echo_tool(_ctx: AgentToolContext, input: EchoInput) -> AgentResult<EchoOutput> {
    Ok(EchoOutput { text: input.text })
}
