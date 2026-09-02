//! End-to-end proof that the approval-gated Bot Flow Agent tools read, apply
//! and report conflicts on the one active flow graph a co-located runtime
//! shares with the web flow editor.

use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_bundle::{AgentLoop, AgentPluginBundle, AgentRuntimeRunner, ModelGateway};
use mutsuki_agent_contracts::{
    AGENT_RUN_PROTOCOL, AGENT_SESSION_CREATE_PROTOCOL, AgentError, AgentMessage,
    AgentModelGenerateRequest, AgentModelGenerateResult, AgentModelStopReason, AgentRole,
    AgentRunRequest, AgentRunResult, AgentRunStatus, AgentSession, AgentSessionCreateRequest,
    AgentToolCall, AgentToolListRequest, AgentUsage, PermissionDecision, PermissionDecisionKind,
};
use mutsuki_agent_service_host_integration::AgentConnectionRegistry;
use mutsuki_bot::assemble_service_with_flow_registry;
use mutsuki_bot_flow::{BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowRegistry, BotNodeCatalog};
use mutsuki_bot_protocol::{BOT_FLOW_TOOL_APPLY, BOT_FLOW_TOOL_READ};
use mutsuki_config_service::{
    ConfigContext, ConfigProviderRegistry, ConfigService, InMemoryConfigRepository, capability,
};
use mutsuki_plugin_agent_model_gateway::{ModelProvider, ModelProviderFuture};
use mutsuki_plugin_bot_flow_agent_tool::{
    flow_tool_descriptors, flow_tool_manifest, flow_tool_runner,
};
use mutsuki_runtime_contracts::{Task, TaskBatch};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::{
    ControlCommand, ControlResponse, ControlResult, IdParam, TaskOutcomeView, TaskSubmitBatchParam,
};
use mutsuki_service_ipc::ControlClient;
use serde_json::json;
use tempfile::tempdir;

/// Scripts one tool call per user prompt, then summarizes the tool result the
/// transcript feeds back: applied revisions, surfaced conflicts, read output.
#[derive(Default)]
struct FlowScriptedProvider;

impl FlowScriptedProvider {
    #[allow(clippy::needless_pass_by_value)]
    fn result(request: AgentModelGenerateRequest) -> Result<AgentModelGenerateResult, AgentError> {
        let last_user_index = request
            .messages
            .iter()
            .rposition(|message| message.role == AgentRole::User)
            .ok_or_else(|| AgentError::invalid_input("scripted model requires a user message"))?;
        let user = request.messages[last_user_index].content.clone();
        let tool_message = request.messages[last_user_index + 1..]
            .iter()
            .find(|message| message.role == AgentRole::Tool)
            .map(|message| message.content.clone());
        let (message, stop_reason, tool_calls) = if let Some(content) = tool_message {
            let summary = if content.contains("revision_conflict") {
                "conflict noted"
            } else {
                "done"
            };
            (
                AgentMessage::assistant(summary),
                AgentModelStopReason::Stop,
                Vec::new(),
            )
        } else {
            let (name, input) = match user.as_str() {
                "apply-flow" => (
                    BOT_FLOW_TOOL_APPLY,
                    json!({"flow": default_flow(), "expected_revision": 0}),
                ),
                "apply-conflict" => (
                    BOT_FLOW_TOOL_APPLY,
                    json!({"flow": default_flow(), "expected_revision": 99}),
                ),
                "read-flow" => (BOT_FLOW_TOOL_READ, json!({})),
                other => {
                    return Err(AgentError::invalid_input(format!(
                        "unexpected scripted prompt `{other}`"
                    )));
                }
            };
            (
                AgentMessage::assistant(""),
                AgentModelStopReason::ToolCalls,
                vec![AgentToolCall {
                    call_id: format!("call-{user}"),
                    name: name.into(),
                    input,
                }],
            )
        };
        Ok(AgentModelGenerateResult {
            message,
            stop_reason,
            tool_calls,
            usage: AgentUsage {
                input_tokens: 1,
                output_tokens: 1,
                total_tokens: 2,
            },
            cost_microunits: 0,
            raw: None,
            output_resource: None,
        })
    }
}

impl ModelProvider for FlowScriptedProvider {
    fn provider_id(&self) -> &'static str {
        "flow-scripted"
    }

    fn generate(
        &self,
        request: AgentModelGenerateRequest,
    ) -> Result<AgentModelGenerateResult, AgentError> {
        Self::result(request)
    }

    fn generate_async(&self, request: AgentModelGenerateRequest) -> ModelProviderFuture {
        Box::pin(async move { Self::result(request) })
    }
}

fn default_flow() -> serde_json::Value {
    json!({"flow_id": "default", "name": "流程", "nodes": [], "edges": []})
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn flow_agent_tools_apply_through_approval_and_report_conflicts() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("flow-tool-e2e.toml");
    std::fs::write(&config_path, service_toml(root.path())).unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();

    let providers = Arc::new(ConfigProviderRegistry::default());
    let config = Arc::new(
        ConfigService::new(providers, Arc::new(InMemoryConfigRepository::default())).unwrap(),
    );
    let flow_registry = Arc::new(BotFlowRegistry::new(BotNodeCatalog::default()));

    let model = ModelGateway::with_default_provider("flow-scripted");
    model.register(Arc::new(FlowScriptedProvider));
    let bundle = AgentPluginBundle {
        agent_loop: AgentLoop::default().with_default_model("flow-scripted-model"),
        model,
        ..Default::default()
    };
    for descriptor in flow_tool_descriptors() {
        bundle.tools.register(descriptor).unwrap();
    }
    bundle
        .context
        .set_tools(bundle.tools.list(AgentToolListRequest::default()).tools);
    bundle
        .context
        .set_system_prompt("Edit the Bot Flow graph through the flow tools.");

    let mut builder = assemble_service_with_flow_registry(
        service.clone(),
        config.clone(),
        AgentConnectionRegistry::new(),
        flow_registry.clone(),
    )
    .unwrap();
    for manifest in bundle.manifests() {
        builder = builder.register_builtin_plugin(manifest);
    }
    builder = builder.register_builtin_plugin(flow_tool_manifest());
    for kind in AgentRuntimeRunner::ALL {
        let bundle = bundle.clone();
        builder = builder
            .register_runtime_client_runner(move |client| bundle.runtime_runner(kind, client));
    }
    let tool_config = config.clone();
    let tool_registry = flow_registry.clone();
    builder = builder.register_runtime_client_runner(move |client| {
        flow_tool_runner(client, tool_config.clone(), tool_registry.clone())
    });
    let model_bundle = bundle.clone();
    builder = builder.register_builtin_async_handler(move || model_bundle.model_async_handler());

    let runtime = builder.start().await.unwrap();
    let client = ControlClient::new((&service).into());

    let session = submit_and_wait::<AgentSession>(
        &client,
        "session-create",
        AGENT_SESSION_CREATE_PROTOCOL,
        &AgentSessionCreateRequest {
            session_id: None,
            profile_id: "test.profile".into(),
            title: Some("Flow tools".into()),
        },
    )
    .await;

    // The apply tool is approval-gated: the first turn must stop before the
    // flow document changes.
    let mut apply = AgentRunRequest::new("test.profile", vec![AgentMessage::user("apply-flow")]);
    apply.session_id = Some(session.session_id.clone());
    let waiting =
        submit_and_wait::<AgentRunResult>(&client, "agent-apply", AGENT_RUN_PROTOCOL, &apply).await;
    assert_eq!(waiting.status, AgentRunStatus::WaitingApproval);
    assert_eq!(waiting.pending_approvals.len(), 1);
    assert_eq!(waiting.pending_approvals[0].tool, BOT_FLOW_TOOL_APPLY);
    assert_eq!(flow_registry.active().revision, 0);

    let pending = waiting.pending_approvals[0].clone();
    let mut resume = AgentRunRequest::new("test.profile", Vec::new());
    resume.session_id = Some(session.session_id.clone());
    resume.turn_id = Some(pending.turn_id.clone());
    resume.permission_decisions = vec![PermissionDecision {
        session_id: pending.session_id,
        turn_id: pending.turn_id,
        action_id: pending.action_id,
        version: pending.version,
        decision: PermissionDecisionKind::Approved,
    }];
    let applied = submit_and_wait::<AgentRunResult>(
        &client,
        "agent-apply-resume",
        AGENT_RUN_PROTOCOL,
        &resume,
    )
    .await;
    assert_eq!(applied.status, AgentRunStatus::Completed);
    assert_eq!(flow_registry.active().revision, 1);
    assert!(applied.messages.iter().any(
        |message| message.role == AgentRole::Tool && message.content.contains("\"revision\":1")
    ));
    let snapshot = config
        .read(
            BOT_FLOW_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    assert_eq!(snapshot.revision.0, 1, "applied document must be persisted");

    // The read tool observes the graph the apply activated.
    let mut read = AgentRunRequest::new("test.profile", vec![AgentMessage::user("read-flow")]);
    read.session_id = Some(session.session_id.clone());
    let read_result =
        submit_and_wait::<AgentRunResult>(&client, "agent-read", AGENT_RUN_PROTOCOL, &read).await;
    assert_eq!(read_result.status, AgentRunStatus::Completed);
    assert!(read_result.messages.iter().any(
        |message| message.role == AgentRole::Tool && message.content.contains("\"revision\":1")
    ));

    // A stale expected revision is a model-visible conflict; the apply is
    // approved like any other edit, and the graph still stays put.
    let mut conflict =
        AgentRunRequest::new("test.profile", vec![AgentMessage::user("apply-conflict")]);
    conflict.session_id = Some(session.session_id.clone());
    let conflict_waiting =
        submit_and_wait::<AgentRunResult>(&client, "agent-conflict", AGENT_RUN_PROTOCOL, &conflict)
            .await;
    assert_eq!(conflict_waiting.status, AgentRunStatus::WaitingApproval);
    let pending = conflict_waiting.pending_approvals[0].clone();
    let mut resume = AgentRunRequest::new("test.profile", Vec::new());
    resume.session_id = Some(session.session_id.clone());
    resume.turn_id = Some(pending.turn_id.clone());
    resume.permission_decisions = vec![PermissionDecision {
        session_id: pending.session_id,
        turn_id: pending.turn_id,
        action_id: pending.action_id,
        version: pending.version,
        decision: PermissionDecisionKind::Approved,
    }];
    let conflict_result = submit_and_wait::<AgentRunResult>(
        &client,
        "agent-conflict-resume",
        AGENT_RUN_PROTOCOL,
        &resume,
    )
    .await;
    assert_eq!(conflict_result.status, AgentRunStatus::Completed);
    assert!(conflict_result.messages.iter().any(|message| {
        message.role == AgentRole::Tool
            && message
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("is_error"))
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            && message.content.contains("bot.flow.revision_conflict")
    }));
    assert_eq!(flow_registry.active().revision, 1);

    runtime.shutdown().await;
}

async fn submit(
    client: &ControlClient,
    task_id: &str,
    protocol_id: &str,
    payload: &impl serde::Serialize,
) {
    let request = TaskSubmitBatchParam {
        batch: TaskBatch::one(
            format!("batch-{task_id}"),
            Task::new(task_id, protocol_id, serde_json::to_value(payload).unwrap()),
        ),
    };
    let response = client
        .request(ControlCommand::TaskSubmitBatch(request))
        .await
        .unwrap();
    assert!(matches!(
        response,
        ControlResponse::Ok(ControlResult::TaskSubmitBatch(_))
    ));
}

async fn wait_outcome(client: &ControlClient, task_id: &str) -> TaskOutcomeView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let response = client
            .request(ControlCommand::TaskOutcome(IdParam { id: task_id.into() }))
            .await
            .unwrap();
        let ControlResponse::Ok(ControlResult::TaskOutcome(outcome)) = response else {
            panic!("outcome failed: {response:?}");
        };
        if outcome.status != "pending" {
            return outcome;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "task {task_id} timed out"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn submit_and_wait<T: serde::de::DeserializeOwned>(
    client: &ControlClient,
    task_id: &str,
    protocol_id: &str,
    payload: &impl serde::Serialize,
) -> T {
    submit(client, task_id, protocol_id, payload).await;
    let outcome = wait_outcome(client, task_id).await;
    assert_eq!(outcome.status, "completed", "task outcome: {outcome:?}");
    serde_json::from_value(outcome.output.expect("completed task has typed output")).unwrap()
}

fn service_toml(root: &std::path::Path) -> String {
    format!(
        r#"[service]
profile = "flow-tool-e2e"
instance_id = "flow-tool-e2e"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = true
name = "flow-tool-e2e"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"

[[plugins.configured]]
id = "mutsuki.plugin.agent.context"

[[plugins.configured]]
id = "mutsuki.plugin.agent.loop"

[[plugins.configured]]
id = "mutsuki.plugin.agent.context.collect.uninjected"

[[plugins.configured]]
id = "mutsuki.plugin.agent.credential"

[[plugins.configured]]
id = "mutsuki.plugin.agent.knowledge"

[[plugins.configured]]
id = "mutsuki.plugin.agent.memory_router"

[[plugins.configured]]
id = "mutsuki.plugin.agent.model_gateway"

[[plugins.configured]]
id = "mutsuki.plugin.agent.prompt"

[[plugins.configured]]
id = "mutsuki.plugin.agent.session"

[[plugins.configured]]
id = "mutsuki.plugin.agent.skills"

[[plugins.configured]]
id = "mutsuki.plugin.agent.tool_router"

[[plugins.configured]]
id = "mutsuki.bot.router.flow"

[[plugins.configured]]
id = "mutsuki.plugin.bot.flow.tool"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/")
    )
}
