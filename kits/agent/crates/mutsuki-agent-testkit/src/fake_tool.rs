pub use mutsuki_plugin_agent_tool_router::ToolRegistry;

use mutsuki_agent_protocol::{
    AgentToolDescriptor, AgentToolExecuteRequest, AgentToolExecuteResult, ToolSideEffect,
};

pub const TEST_ECHO_TOOL_PROTOCOL: &str = "mutsuki.agent.test/echo@1";

pub fn echo_tool_descriptor() -> AgentToolDescriptor {
    let mut descriptor = AgentToolDescriptor::new(
        "echo",
        TEST_ECHO_TOOL_PROTOCOL,
        "Returns the typed input without side effects",
    );
    descriptor.side_effect = ToolSideEffect::None;
    descriptor
}

pub fn execute_echo_tool(request: AgentToolExecuteRequest) -> AgentToolExecuteResult {
    AgentToolExecuteResult {
        call_id: request.call_id,
        name: request.name,
        output: Some(request.input),
        output_ref: None,
        approved: true,
    }
}
