use mutsuki_agent_protocol::*;
use mutsuki_runtime_sdk::{ProtocolSpec, SdkProtocol};

macro_rules! protocol_marker {
    ($name:ident, $id:expr) => {
        #[derive(Clone, Debug)]
        pub struct $name;

        impl SdkProtocol for $name {
            const PROTOCOL_ID: &'static str = $id;
        }

        impl ProtocolSpec for $name {}
    };
}

protocol_marker!(AgentRunProtocol, AGENT_RUN_PROTOCOL);
protocol_marker!(AgentContextBuildProtocol, AGENT_CONTEXT_BUILD_PROTOCOL);
protocol_marker!(AgentToolListProtocol, AGENT_TOOL_LIST_PROTOCOL);
protocol_marker!(AgentToolExecuteProtocol, AGENT_TOOL_EXECUTE_PROTOCOL);
protocol_marker!(AgentSessionCreateProtocol, AGENT_SESSION_CREATE_PROTOCOL);
protocol_marker!(AgentSessionGetProtocol, AGENT_SESSION_GET_PROTOCOL);
protocol_marker!(AgentSessionAppendProtocol, AGENT_SESSION_APPEND_PROTOCOL);
protocol_marker!(
    AgentSessionSnapshotProtocol,
    AGENT_SESSION_SNAPSHOT_PROTOCOL
);
protocol_marker!(AgentMemoryQueryProtocol, AGENT_MEMORY_QUERY_PROTOCOL);
protocol_marker!(AgentMemoryWriteProtocol, AGENT_MEMORY_WRITE_PROTOCOL);
protocol_marker!(AgentMemoryActivateProtocol, AGENT_MEMORY_ACTIVATE_PROTOCOL);
protocol_marker!(AgentModelGenerateProtocol, AGENT_MODEL_GENERATE_PROTOCOL);
protocol_marker!(AgentModelStreamProtocol, AGENT_MODEL_STREAM_PROTOCOL);
protocol_marker!(
    AgentModelHttpEffectProtocol,
    AGENT_MODEL_HTTP_EFFECT_PROTOCOL
);
protocol_marker!(AgentModelPollProtocol, AGENT_MODEL_POLL_PROTOCOL);
protocol_marker!(AgentPromptRenderProtocol, AGENT_PROMPT_RENDER_PROTOCOL);
protocol_marker!(AgentPromptGetProtocol, AGENT_PROMPT_GET_PROTOCOL);
