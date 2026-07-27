use std::sync::Arc;

use mutsuki_agent_protocol::{
    AgentMemoryQueryRequest, AgentMemoryWriteRequest, AgentMessage, AgentModelGenerateRequest,
    AgentModelStreamRequest, AgentPromptRenderRequest, AgentPromptTemplate,
    AgentSessionAppendRequest, AgentSessionCreateRequest, AgentToolExecuteRequest,
    AgentToolListRequest,
};
use mutsuki_plugin_agent_loop::{
    AgentLoop, PLUGIN_ID as LOOP_PLUGIN_ID, RUNNER_ID as LOOP_RUNNER_ID, plugin as loop_plugin,
};
use mutsuki_plugin_agent_memory_router::MemoryRouter;
use mutsuki_plugin_agent_model_gateway::ModelGateway;
use mutsuki_plugin_agent_prompt::PromptRegistry;
use mutsuki_plugin_agent_session::SessionStore;
use mutsuki_runtime_sdk::contracts::{RunnerMode, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_runtime_sdk::{RuntimeClient, RuntimeResult};

use crate::{MockModelProvider, echo_tool_descriptor, execute_echo_tool};

struct NoopClient;

impl RuntimeClient for NoopClient {
    fn submit_batch(&self, _batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        Ok(Vec::new())
    }

    fn task_outcome(&self, _handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(None)
    }
}

pub fn run_basic_conformance() {
    session_round_trip();
    memory_round_trip();
    model_stream_round_trip();
    prompt_round_trip();
    tool_round_trip();
    batch_first_runner_descriptor();
}

pub fn session_round_trip() {
    let store = SessionStore::default();
    let session = store
        .create(AgentSessionCreateRequest {
            profile_id: "test.profile".into(),
            title: Some("Conformance".into()),
        })
        .expect("session can be created");
    let session = store
        .append(AgentSessionAppendRequest {
            session_id: session.session_id,
            messages: vec![AgentMessage::user("hello")],
        })
        .expect("session can append messages");
    assert_eq!(session.messages.len(), 1);
    assert_eq!(session.turn_count, 1);
    assert_eq!(session.resource.resource_kind, "mutsuki.agent.session");
    assert_eq!(session.cell.resource_kind, "mutsuki.agent.session");

    let other = store
        .create(AgentSessionCreateRequest {
            profile_id: "test.profile".into(),
            title: Some("Other".into()),
        })
        .expect("second session can be created");
    assert_ne!(session.session_id, other.session_id);
    assert_ne!(session.resource.ref_id, other.resource.ref_id);
    assert_ne!(session.cell.cell_id, other.cell.cell_id);
    assert!(other.messages.is_empty());
    assert_eq!(other.turn_count, 0);
    let other = store
        .append(AgentSessionAppendRequest {
            session_id: other.session_id,
            messages: vec![AgentMessage::user("other")],
        })
        .expect("second session can append independently");
    let first = store
        .get(mutsuki_agent_protocol::AgentSessionGetRequest {
            session_id: session.session_id,
        })
        .expect("first session remains readable");
    assert_eq!(first.messages[0].content, "hello");
    assert_eq!(other.messages[0].content, "other");
    assert_ne!(first.resource.ref_id, other.resource.ref_id);
}

pub fn tool_round_trip() {
    let registry = mutsuki_plugin_agent_tool_router::ToolRegistry::default();
    registry
        .register(echo_tool_descriptor())
        .expect("test tool can register");
    let listed = registry.list(AgentToolListRequest::default());
    assert_eq!(listed.tools.len(), 1);
    assert!(!listed.tools[0].requires_approval);
    let result = execute_echo_tool(AgentToolExecuteRequest {
        call_id: Some("call-1".into()),
        name: "echo".into(),
        input: serde_json::json!({"value": "hello"}),
        session_id: Some("session-a".into()),
    });
    assert_eq!(result.output, Some(serde_json::json!({"value": "hello"})));
    assert!(result.approved);
}

pub fn memory_round_trip() {
    let memory = MemoryRouter::default();
    let written = memory
        .write(AgentMemoryWriteRequest {
            text: "workspace uses Rust native plugins".into(),
            tags: vec!["architecture".into()],
            metadata: None,
        })
        .expect("memory can be written");
    assert!(written.resource.is_some());
    assert!(written.cell.is_some());
    let result = memory
        .query(AgentMemoryQueryRequest {
            query: "Rust plugins".into(),
            limit: 4,
            tags: Vec::new(),
        })
        .expect("memory can be queried");
    assert_eq!(result.records.len(), 1);
}

pub fn model_stream_round_trip() {
    let gateway = ModelGateway::with_default_provider("mock");
    gateway.register(Arc::new(MockModelProvider));
    gateway
        .generate(AgentModelGenerateRequest {
            model: "mock".into(),
            messages: vec![AgentMessage::user("hi")],
            temperature: None,
            max_output_tokens: None,
            provider_hint: None,
            metadata: None,
            result_protocol_id: None,
            result_context: None,
            session_id: None,
        })
        .expect("model generate works");
    let streamed = gateway
        .stream(AgentModelStreamRequest {
            request: AgentModelGenerateRequest {
                model: "mock".into(),
                messages: vec![AgentMessage::user("stream")],
                temperature: None,
                max_output_tokens: None,
                provider_hint: None,
                metadata: None,
                result_protocol_id: None,
                result_context: None,
                session_id: None,
            },
        })
        .expect("model stream opens ResourceRef");
    assert_eq!(streamed.stream.resource_kind, "mutsuki.agent.stream");
    assert_eq!(
        gateway.read_stream(&streamed.stream).as_deref(),
        Some("Echo: stream")
    );
    assert!(
        !serde_json::to_string(&streamed)
            .unwrap()
            .contains("Echo: stream")
    );
}

pub fn prompt_round_trip() {
    let prompts = PromptRegistry::default();
    prompts
        .register(AgentPromptTemplate {
            template_id: "hello".into(),
            body: "Hello {{name}}".into(),
            variables: vec!["name".into()],
        })
        .expect("prompt can be registered");
    let rendered = prompts
        .render(AgentPromptRenderRequest {
            template_id: "hello".into(),
            variables: [("name".to_string(), serde_json::json!("Mutsuki"))].into(),
        })
        .expect("prompt can be rendered");
    assert_eq!(rendered.text, "Hello Mutsuki");
}

pub fn batch_first_runner_descriptor() {
    let client: mutsuki_runtime_sdk::RuntimeClientRef = Arc::new(NoopClient);
    let loaded = loop_plugin(client, AgentLoop::default()).build();
    assert_eq!(loaded.manifest.plugin_id, LOOP_PLUGIN_ID);
    let runner = loaded
        .manifest
        .provides
        .runners
        .iter()
        .find(|runner| runner.runner_id == LOOP_RUNNER_ID)
        .expect("loop runner is registered");
    assert_eq!(runner.batch.mode, RunnerMode::ScalarAdapter);
    assert!(runner.batch.max_batch_entries >= 1);
    assert!(!runner.payload.layouts.is_empty());
    assert!(runner.control.batch_cancel);
}

#[cfg(test)]
mod tests {
    #[test]
    fn basic_conformance_passes() {
        super::run_basic_conformance();
    }
}
