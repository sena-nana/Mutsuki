use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_agent_bundle::{AgentLoop, AgentPluginBundle, AgentRuntimeRunner, ModelGateway};
use mutsuki_agent_protocol::{
    AGENT_RUN_PROTOCOL, AGENT_SESSION_CREATE_PROTOCOL, AGENT_SESSION_GET_PROTOCOL, AgentError,
    AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult, AgentModelStopReason,
    AgentRole, AgentRunBudget, AgentRunRequest, AgentRunResult, AgentRunStatus, AgentSession,
    AgentSessionCreateRequest, AgentSessionGetRequest, AgentToolCall, AgentToolDescriptor,
    AgentUsage,
};
use mutsuki_agent_sdk::{orchestration_runner, runtime_failure};
use mutsuki_bot::assemble_service;
use mutsuki_plugin_agent_model_gateway::{ModelProvider, ModelProviderFuture};
use mutsuki_runtime_contracts::{PluginManifest, RunnerResult, Task, TaskBatch};
use mutsuki_runtime_sdk::{PluginBuilder, ProtocolSpec, SdkProtocol, TaskAwaitRunnerAdapter};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::{ControlMethod, TaskOutcomeView, TaskSubmitBatchParam};
use mutsuki_service_ipc::ControlClient;
use serde_json::{Value, json};
use tempfile::tempdir;

const TEST_PLUGIN_ID: &str = "mutsuki.test.agent.targets";
const TEST_RUNNER_ID: &str = "mutsuki.test.agent.targets.runner";
const ECHO_PROTOCOL: &str = "mutsuki.test.agent/echo@1";
const CALLBACK_PROTOCOL: &str = "mutsuki.test.agent/callback@1";

#[derive(Clone, Debug)]
struct EchoProtocol;

impl SdkProtocol for EchoProtocol {
    const PROTOCOL_ID: &'static str = ECHO_PROTOCOL;
}

impl ProtocolSpec for EchoProtocol {}

#[derive(Clone, Debug)]
struct CallbackProtocol;

impl SdkProtocol for CallbackProtocol {
    const PROTOCOL_ID: &'static str = CALLBACK_PROTOCOL;
}

impl ProtocolSpec for CallbackProtocol {}

#[derive(Default)]
struct ProviderSignals {
    cancel_started: AtomicBool,
    cancel_dropped: AtomicBool,
}

struct CancelDrop(Arc<ProviderSignals>);

impl Drop for CancelDrop {
    fn drop(&mut self) {
        self.0.cancel_dropped.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct ScriptedProvider {
    signals: Arc<ProviderSignals>,
}

impl ScriptedProvider {
    fn result(request: AgentModelGenerateRequest) -> Result<AgentModelGenerateResult, AgentError> {
        let last_user_index = request
            .messages
            .iter()
            .rposition(|message| message.role == AgentRole::User)
            .ok_or_else(|| AgentError::invalid_input("scripted model requires a user message"))?;
        let user = &request.messages[last_user_index].content;
        if user == "model-fail" {
            return Err(AgentError::new(
                "agent.test.model_failed",
                "scripted model failure",
            ));
        }

        let tool_message = request.messages[last_user_index + 1..]
            .iter()
            .find(|message| message.role == AgentRole::Tool);
        let (message, stop_reason, tool_calls) = if user == "stream" {
            (
                AgentMessage::assistant("stream-final"),
                AgentModelStopReason::Stop,
                Vec::new(),
            )
        } else if let Some(tool_message) = tool_message {
            let users = request
                .messages
                .iter()
                .filter(|message| message.role == AgentRole::User)
                .count();
            (
                AgentMessage::assistant(format!(
                    "final users={users} tool={}",
                    tool_message.content
                )),
                AgentModelStopReason::Stop,
                Vec::new(),
            )
        } else {
            let input = if user == "tool-fail" {
                json!({"fail": true})
            } else {
                json!({"value": user})
            };
            (
                AgentMessage::assistant(""),
                AgentModelStopReason::ToolCalls,
                vec![AgentToolCall {
                    call_id: format!("call-{user}"),
                    name: "echo".into(),
                    input,
                }],
            )
        };

        Ok(AgentModelGenerateResult {
            message,
            stop_reason,
            tool_calls,
            usage: AgentUsage {
                input_tokens: 3,
                output_tokens: 2,
                total_tokens: 5,
            },
            cost_microunits: 7,
            raw: None,
            output_resource: None,
        })
    }
}

impl ModelProvider for ScriptedProvider {
    fn provider_id(&self) -> &str {
        "scripted"
    }

    fn generate(
        &self,
        request: AgentModelGenerateRequest,
    ) -> Result<AgentModelGenerateResult, AgentError> {
        Self::result(request)
    }

    fn generate_async(&self, request: AgentModelGenerateRequest) -> ModelProviderFuture {
        let cancel = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == AgentRole::User)
            .is_some_and(|message| message.content == "cancel");
        if !cancel {
            let result = Self::result(request);
            return Box::pin(async move { result });
        }

        let signals = self.signals.clone();
        Box::pin(async move {
            signals.cancel_started.store(true, Ordering::SeqCst);
            let _drop = CancelDrop(signals);
            std::future::pending::<()>().await;
            unreachable!()
        })
    }
}

#[tokio::test]
async fn agentkit_issue3_runs_real_state_machine_through_service_host_and_core() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("agent-e2e.toml");
    std::fs::write(&config_path, service_toml(root.path())).unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();

    let signals = Arc::new(ProviderSignals::default());
    let tool_executions = Arc::new(AtomicUsize::new(0));
    let callbacks = Arc::new(Mutex::new(Vec::new()));
    let model = ModelGateway::with_default_provider("scripted");
    model.register(Arc::new(ScriptedProvider {
        signals: signals.clone(),
    }));
    let bundle = AgentPluginBundle {
        agent_loop: AgentLoop::default().with_default_model("scripted-model"),
        model,
        ..Default::default()
    };
    bundle
        .tools
        .register(AgentToolDescriptor::new(
            "echo",
            ECHO_PROTOCOL,
            "Returns its structured input",
        ))
        .unwrap();

    let mut builder = assemble_service(service.clone()).unwrap();
    for manifest in bundle.manifests() {
        builder = builder.register_builtin_plugin(manifest);
    }
    builder = builder.register_builtin_plugin(test_manifest());
    for kind in AgentRuntimeRunner::ALL {
        let bundle = bundle.clone();
        builder = builder
            .register_runtime_client_runner(move |client| bundle.runtime_runner(kind, client));
    }
    let model_bundle = bundle.clone();
    builder = builder.register_builtin_async_handler(move || model_bundle.model_async_handler());
    let descriptor = test_runner_descriptor();
    builder = builder.register_runtime_client_runner({
        let tool_executions = tool_executions.clone();
        let callbacks = callbacks.clone();
        move |client| {
            let descriptor = descriptor.clone();
            let tool_executions = tool_executions.clone();
            let callbacks = callbacks.clone();
            Box::new(TaskAwaitRunnerAdapter::new(
                descriptor,
                client,
                Box::new(move |_ctx, task| {
                    let tool_executions = tool_executions.clone();
                    let callbacks = callbacks.clone();
                    Box::pin(async move {
                        match task.protocol_id.as_str() {
                            ECHO_PROTOCOL => {
                                tool_executions.fetch_add(1, Ordering::SeqCst);
                                if task.payload["fail"] == true {
                                    return Err(runtime_failure(
                                        TEST_PLUGIN_ID,
                                        &task.task_id,
                                        AgentError::new(
                                            "agent.test.tool_failed",
                                            "scripted tool failure",
                                        ),
                                    ));
                                }
                                let mut result = RunnerResult::completed(task.task_id);
                                result.output = Some(task.payload.into_value());
                                Ok(result)
                            }
                            CALLBACK_PROTOCOL => {
                                callbacks.lock().unwrap().push(task.payload.clone());
                                let mut result = RunnerResult::completed(task.task_id);
                                result.output = Some(json!({"accepted": true}));
                                Ok(result)
                            }
                            _ => unreachable!("descriptor only routes test protocols"),
                        }
                    })
                }),
            ))
        }
    });

    let runtime = builder.start().await.unwrap();
    let client = ControlClient::new((&service).into());

    let session = submit_and_wait::<AgentSession>(
        &client,
        "session-create",
        AGENT_SESSION_CREATE_PROTOCOL,
        &AgentSessionCreateRequest {
            profile_id: "test.profile".into(),
            title: Some("Issue 3".into()),
        },
    )
    .await;

    let mut first = AgentRunRequest::new("test.profile", vec![AgentMessage::user("first")]);
    first.session_id = Some(session.session_id.clone());
    first.result_protocol_id = Some(CALLBACK_PROTOCOL.into());
    first.result_context = Some(json!({"source": "issue3-e2e"}));
    let first =
        submit_and_wait::<AgentRunResult>(&client, "agent-first", AGENT_RUN_PROTOCOL, &first).await;
    assert_eq!(first.status, AgentRunStatus::Completed);
    assert_eq!(
        first.messages.last().unwrap().content,
        "final users=1 tool={\"value\":\"first\"}"
    );
    assert_eq!(first.usage.total_tokens, 10);
    assert_eq!(first.cost_microunits, 14);
    wait_until("callback execution", || {
        !callbacks.lock().unwrap().is_empty()
    })
    .await;
    let callback = callbacks.lock().unwrap()[0].clone();
    assert_eq!(callback["context"]["source"], "issue3-e2e");
    assert_eq!(callback["result"]["status"], "completed");

    let mut second = AgentRunRequest::new("test.profile", vec![AgentMessage::user("second")]);
    second.session_id = Some(session.session_id.clone());
    let second =
        submit_and_wait::<AgentRunResult>(&client, "agent-second", AGENT_RUN_PROTOCOL, &second)
            .await;
    assert_eq!(
        second.messages.last().unwrap().content,
        "final users=2 tool={\"value\":\"second\"}"
    );
    let snapshot = submit_and_wait::<AgentSession>(
        &client,
        "session-get",
        AGENT_SESSION_GET_PROTOCOL,
        &AgentSessionGetRequest {
            session_id: session.session_id,
        },
    )
    .await;
    assert_eq!(snapshot.turn_count, 2);
    assert_eq!(snapshot.messages, second.messages);

    let mut streamed = AgentRunRequest::new("test.profile", vec![AgentMessage::user("stream")]);
    streamed.stream = true;
    let streamed =
        submit_and_wait::<AgentRunResult>(&client, "agent-stream", AGENT_RUN_PROTOCOL, &streamed)
            .await;
    let stream = streamed.output_resource.as_ref().unwrap();
    assert_eq!(streamed.status, AgentRunStatus::Completed);
    assert_eq!(streamed.messages.last().unwrap().content, "");
    assert_eq!(
        bundle.model.read_stream(stream).as_deref(),
        Some("stream-final")
    );

    let model_failure = submit_outcome(
        &client,
        "agent-model-fail",
        AGENT_RUN_PROTOCOL,
        &AgentRunRequest::new("test.profile", vec![AgentMessage::user("model-fail")]),
    )
    .await;
    assert_eq!(model_failure.status, "failed");
    assert_eq!(
        model_failure.error_code.as_deref(),
        Some("agent.test.model_failed")
    );

    let tool_failure = submit_outcome(
        &client,
        "agent-tool-fail",
        AGENT_RUN_PROTOCOL,
        &AgentRunRequest::new("test.profile", vec![AgentMessage::user("tool-fail")]),
    )
    .await;
    assert_eq!(tool_failure.status, "failed");
    assert_eq!(
        tool_failure.error_code.as_deref(),
        Some("agent.test.tool_failed")
    );

    let executions_before_budget = tool_executions.load(Ordering::SeqCst);
    let mut budget = AgentRunRequest::new("test.profile", vec![AgentMessage::user("budget")]);
    budget.budget = AgentRunBudget {
        max_total_tokens: Some(5),
        max_cost_microunits: None,
    };
    let budget =
        submit_and_wait::<AgentRunResult>(&client, "agent-budget", AGENT_RUN_PROTOCOL, &budget)
            .await;
    assert_eq!(budget.status, AgentRunStatus::BudgetExceeded);
    assert_eq!(
        tool_executions.load(Ordering::SeqCst),
        executions_before_budget,
        "tool must not run when no model budget remains for the follow-up"
    );

    let cancel_id = "agent-cancel";
    submit(
        &client,
        cancel_id,
        AGENT_RUN_PROTOCOL,
        &AgentRunRequest::new("test.profile", vec![AgentMessage::user("cancel")]),
    )
    .await;
    wait_until("cancel provider start", || {
        signals.cancel_started.load(Ordering::SeqCst)
    })
    .await;
    let cancelled = client
        .request(ControlMethod::TaskCancel, json!({"id": cancel_id}))
        .await
        .unwrap();
    assert!(cancelled.ok, "cancel failed: {:?}", cancelled.error);
    let cancelled = wait_outcome(&client, cancel_id).await;
    assert_eq!(cancelled.status, "cancelled");
    wait_until("cancel provider drop", || {
        signals.cancel_dropped.load(Ordering::SeqCst)
    })
    .await;

    runtime.shutdown().await;
}

fn test_runner_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    orchestration_runner(TEST_RUNNER_ID, TEST_PLUGIN_ID)
        .accepts::<EchoProtocol>()
        .accepts::<CallbackProtocol>()
        .build()
}

fn test_manifest() -> PluginManifest {
    PluginBuilder::new(TEST_PLUGIN_ID)
        .protocol::<EchoProtocol>()
        .protocol::<CallbackProtocol>()
        .runner_descriptor(test_runner_descriptor())
        .build()
        .manifest
}

async fn submit_and_wait<T: serde::de::DeserializeOwned>(
    client: &ControlClient,
    task_id: &str,
    protocol_id: &str,
    payload: &impl serde::Serialize,
) -> T {
    let outcome = submit_outcome(client, task_id, protocol_id, payload).await;
    assert_eq!(outcome.status, "completed", "task outcome: {outcome:?}");
    serde_json::from_value(outcome.output.expect("completed task has typed output")).unwrap()
}

async fn submit_outcome(
    client: &ControlClient,
    task_id: &str,
    protocol_id: &str,
    payload: &impl serde::Serialize,
) -> TaskOutcomeView {
    submit(client, task_id, protocol_id, payload).await;
    wait_outcome(client, task_id).await
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
        .request(
            ControlMethod::TaskSubmitBatch,
            serde_json::to_value(request).unwrap(),
        )
        .await
        .unwrap();
    assert!(response.ok, "submit failed: {:?}", response.error);
}

async fn wait_outcome(client: &ControlClient, task_id: &str) -> TaskOutcomeView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = client
            .request(ControlMethod::TaskOutcome, json!({"id": task_id}))
            .await
            .unwrap();
        assert!(response.ok, "outcome failed: {:?}", response.error);
        let outcome: TaskOutcomeView = serde_json::from_value(response.result.unwrap()).unwrap();
        if outcome.status != "pending" {
            return outcome;
        }
        if tokio::time::Instant::now() >= deadline {
            let tasks = client
                .request(ControlMethod::TaskList, Value::Null)
                .await
                .unwrap();
            panic!("task {task_id} timed out: {:?}", tasks.result);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_until(label: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(tokio::time::Instant::now() < deadline, "{label} timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn service_toml(root: &std::path::Path) -> String {
    format!(
        r#"[service]
profile = "agent-issue3-e2e"
instance_id = "agent-issue3-e2e"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = true
name = "agent-issue3-e2e"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"

[[plugins.configured]]
id = "mutsuki.plugin.agent.context"

[[plugins.configured]]
id = "mutsuki.plugin.agent.loop"

[[plugins.configured]]
id = "mutsuki.plugin.agent.memory_router"

[[plugins.configured]]
id = "mutsuki.plugin.agent.model_gateway"

[[plugins.configured]]
id = "mutsuki.plugin.agent.prompt"

[[plugins.configured]]
id = "mutsuki.plugin.agent.session"

[[plugins.configured]]
id = "mutsuki.plugin.agent.tool_router"

[[plugins.configured]]
id = "mutsuki.test.agent.targets"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/")
    )
}
