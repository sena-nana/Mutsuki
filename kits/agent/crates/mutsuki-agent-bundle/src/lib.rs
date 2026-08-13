use std::sync::Arc;

mod native_coding;

pub use mutsuki_agent_runtime::{KnowledgeService, SkillRegistry};
pub use mutsuki_plugin_agent_context::ContextBuilder;
pub use mutsuki_plugin_agent_loop::AgentLoop;
pub use mutsuki_plugin_agent_memory_router::MemoryRouter;
pub use mutsuki_plugin_agent_model_gateway::{
    AdapterBackedModelProvider, HttpModelProvider, HttpModelProviderOptions, ModelGateway,
};
pub use mutsuki_plugin_agent_prompt::PromptRegistry;
pub use mutsuki_plugin_agent_session::SessionStore;
pub use mutsuki_plugin_agent_tool_router::ToolRegistry;
use mutsuki_runtime_contracts::{PluginManifest, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_runtime_core::{AsyncBatchHandler, Runner};
use mutsuki_runtime_sdk::{RuntimeClient, RuntimeClientRef, RuntimeResult};
pub use native_coding::{
    LSP_PLUGIN_ID, NATIVE_CODING_BUNDLE_ID, NATIVE_CODING_TOOL_PLUGIN_ID,
    NATIVE_CODING_TOOL_PROTOCOL, NATIVE_CODING_TOOL_RUNNER_ID, NativeCodingAgentBundle,
    NativeCodingBackends, NativeCodingRunContext, NativeCodingToolContext,
    NativeCodingToolProtocol, UnavailableLspFactory, UnavailableMcpFactory,
    native_coding_tool_plugin, run_fix_golden_path, run_resume_without_duplicate_side_effects,
    run_review_golden_path, seed_fix_fixture,
};

/// Host-neutral collection of Agent services and plugin manifests.
///
/// Product crates own runtime registration, health presentation, provider
/// options, and secret acquisition. AgentKit does not read or manage product
/// configuration.
#[derive(Clone, Default)]
pub struct AgentPluginBundle {
    pub context: ContextBuilder,
    pub agent_loop: AgentLoop,
    pub knowledge: KnowledgeService,
    pub memory: MemoryRouter,
    pub model: ModelGateway,
    pub prompts: PromptRegistry,
    pub sessions: SessionStore,
    pub skills: SkillRegistry,
    pub tools: ToolRegistry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRuntimeRunner {
    Context,
    Knowledge,
    Loop,
    Memory,
    Prompt,
    Session,
    Skill,
    Tool,
}

impl AgentRuntimeRunner {
    pub const ALL: [Self; 8] = [
        Self::Context,
        Self::Knowledge,
        Self::Loop,
        Self::Memory,
        Self::Prompt,
        Self::Session,
        Self::Skill,
        Self::Tool,
    ];
}

impl AgentPluginBundle {
    pub fn manifests(&self) -> Vec<PluginManifest> {
        let client = noop_client();
        vec![
            mutsuki_plugin_agent_context::plugin(client.clone(), self.context.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_knowledge::plugin(client.clone(), self.knowledge.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_loop::plugin(client.clone(), self.agent_loop.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_memory_router::plugin(client.clone(), self.memory.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_model_gateway::plugin(client.clone(), self.model.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_prompt::plugin(client.clone(), self.prompts.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_session::plugin(client.clone(), self.sessions.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_skills::plugin(client.clone(), self.skills.clone())
                .build()
                .manifest,
            mutsuki_plugin_agent_tool_router::plugin(client, self.tools.clone())
                .build()
                .manifest,
        ]
    }

    pub fn runtime_runner(
        &self,
        kind: AgentRuntimeRunner,
        client: RuntimeClientRef,
    ) -> Box<dyn Runner> {
        match kind {
            AgentRuntimeRunner::Context => take_runner(mutsuki_plugin_agent_context::plugin(
                client,
                self.context.clone(),
            )),
            AgentRuntimeRunner::Knowledge => take_runner(mutsuki_plugin_agent_knowledge::plugin(
                client,
                self.knowledge.clone(),
            )),
            AgentRuntimeRunner::Loop => take_runner(mutsuki_plugin_agent_loop::plugin(
                client,
                self.agent_loop.clone(),
            )),
            AgentRuntimeRunner::Memory => take_runner(mutsuki_plugin_agent_memory_router::plugin(
                client,
                self.memory.clone(),
            )),
            AgentRuntimeRunner::Prompt => take_runner(mutsuki_plugin_agent_prompt::plugin(
                client,
                self.prompts.clone(),
            )),
            AgentRuntimeRunner::Session => take_runner(mutsuki_plugin_agent_session::plugin(
                client,
                self.sessions.clone(),
            )),
            AgentRuntimeRunner::Skill => take_runner(mutsuki_plugin_agent_skills::plugin(
                client,
                self.skills.clone(),
            )),
            AgentRuntimeRunner::Tool => take_runner(mutsuki_plugin_agent_tool_router::plugin(
                client,
                self.tools.clone(),
            )),
        }
    }

    pub fn model_async_handler(&self) -> Arc<dyn AsyncBatchHandler> {
        mutsuki_plugin_agent_model_gateway::async_handler(self.model.clone())
    }

    pub fn runner_ids() -> [&'static str; 9] {
        [
            mutsuki_plugin_agent_context::RUNNER_ID,
            mutsuki_plugin_agent_knowledge::RUNNER_ID,
            mutsuki_plugin_agent_loop::RUNNER_ID,
            mutsuki_plugin_agent_memory_router::RUNNER_ID,
            mutsuki_plugin_agent_model_gateway::RUNNER_ID,
            mutsuki_plugin_agent_prompt::RUNNER_ID,
            mutsuki_plugin_agent_session::RUNNER_ID,
            mutsuki_plugin_agent_skills::RUNNER_ID,
            mutsuki_plugin_agent_tool_router::RUNNER_ID,
        ]
    }
}

fn take_runner(builder: mutsuki_runtime_sdk::PluginBuilder) -> Box<dyn Runner> {
    builder
        .build()
        .runners
        .pop()
        .expect("Agent plugin contains one runner")
}

fn noop_client() -> RuntimeClientRef {
    Arc::new(NoopClient)
}

struct NoopClient;

impl RuntimeClient for NoopClient {
    fn submit_batch(&self, _batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        Ok(Vec::new())
    }

    fn task_outcome(&self, _handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_adapter_anthropic::{
        AnthropicMessagesAdapter, provider_descriptor as anthropic_provider_descriptor,
    };
    use mutsuki_agent_adapter_api::{
        CredentialBroker, CredentialFuture, CredentialValue, ModelProtocolAdapter,
    };
    use mutsuki_agent_contracts::{
        AGENT_RUN_PROTOCOL, AgentEvent, AgentMessage, AgentRunRequest, AgentRunResult,
        AgentRunStatus, AgentSessionCreateRequest, AgentSessionGetRequest, AgentToolDescriptor,
        AgentToolExecution, ContextBudget, CredentialRef, InteractionKind, InteractionResolution,
        PermissionDecision, PermissionDecisionKind,
    };
    use mutsuki_agent_runtime::AgentRuntimeProfileBuilder;
    use mutsuki_agent_sdk::orchestration_runner;
    use mutsuki_runtime_contracts::{
        PluginDeploymentKind, RuntimeError, RuntimeProfile, RuntimeProfileMode, ScalarValue, Task,
        TaskOutcome,
    };
    use mutsuki_runtime_host::{HostRuntimeConfig, RuntimeBootstrapper, TokioAsyncExecutor};
    use mutsuki_runtime_sdk::{
        HostRuntime as _, PluginBuilder, ProtocolSpec, RuntimeClientRef, RuntimeFailure,
        SdkProtocol, TaskAwaitRunnerAdapter, TaskSubmitterRuntimeClient,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn standard_bundle_has_unique_batch_first_manifests() {
        let manifests = AgentPluginBundle::default().manifests();
        assert_eq!(manifests.len(), 9);
        let ids = manifests
            .iter()
            .map(|manifest| manifest.plugin_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), manifests.len());
        assert!(manifests.iter().all(|manifest| {
            !manifest.provides.runners.is_empty()
                && manifest
                    .provides
                    .runners
                    .iter()
                    .all(|runner| runner.batch.max_batch_entries > 0)
        }));
    }

    const TOOL_PROTOCOL: &str = "mutsuki.agent.reference/tool@1";
    const TOOL_PLUGIN_ID: &str = "mutsuki.agent.reference.tools";

    #[derive(Clone, Debug)]
    struct ReferenceToolProtocol;

    impl SdkProtocol for ReferenceToolProtocol {
        const PROTOCOL_ID: &'static str = TOOL_PROTOCOL;
    }

    impl ProtocolSpec for ReferenceToolProtocol {}

    #[derive(Default)]
    struct DeferredClient {
        client: OnceLock<RuntimeClientRef>,
    }

    impl DeferredClient {
        fn bind(&self, runtime: &mutsuki_runtime_host::HostRuntime) {
            let submitter = runtime.host_context().task_submitter_ref();
            self.client
                .set(TaskSubmitterRuntimeClient::new(submitter).into_runtime_client())
                .map_err(|_| ())
                .expect("in-process runtime client binds once");
        }

        fn client(&self) -> RuntimeResult<RuntimeClientRef> {
            self.client.get().cloned().ok_or_else(|| {
                RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                    "mutsuki.agent.reference",
                    "runtime_client.not_bound",
                ))
            })
        }
    }

    impl RuntimeClient for DeferredClient {
        fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            self.client()?.submit_batch(batch)
        }

        fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            self.client()?.task_outcome(handle)
        }
    }

    struct StaticCredential;

    impl CredentialBroker for StaticCredential {
        fn resolve(&self, _credential: CredentialRef) -> CredentialFuture {
            Box::pin(async { CredentialValue::new("loopback-secret") })
        }
    }

    fn read_json_request(stream: &mut TcpStream) -> serde_json::Value {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let count = stream.read(&mut chunk).expect("request read succeeds");
            assert!(count > 0, "client closed before request body completed");
            bytes.extend_from_slice(&chunk[..count]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
                .expect("request declares content-length");
            let body_start = header_end + 4;
            if bytes.len() >= body_start + content_length {
                return serde_json::from_slice(&bytes[body_start..body_start + content_length])
                    .expect("request body is JSON");
            }
        }
    }

    fn write_json_response(stream: &mut TcpStream, payload: &str) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
        .expect("response writes");
    }

    #[test]
    fn in_process_core_runs_agent_runtime_through_public_bootstrapper() {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let mut interaction_tool = AgentToolDescriptor::new(
            "ask_user_question",
            AGENT_RUN_PROTOCOL,
            "Ask the user for a clarification",
        );
        interaction_tool.execution = AgentToolExecution::Interaction {
            interaction_kind: InteractionKind::Clarification,
        };
        interaction_tool.input_schema = serde_json::json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "question": { "type": "string" },
                "options": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        });
        let mut reference_tool = AgentToolDescriptor::new(
            "workspace.inspect",
            TOOL_PROTOCOL,
            "Reads the public workspace view",
        );
        reference_tool.requires_approval = true;
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback model server binds");
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server_model_calls = model_calls.clone();
        let model_server = std::thread::spawn(move || {
            let (mut summary, _) = listener.accept().expect("context summary request arrives");
            server_model_calls.fetch_add(1, Ordering::SeqCst);
            let summary_payload = read_json_request(&mut summary);
            let summary_payload_text = summary_payload.to_string();
            assert!(summary_payload_text.contains("Summarize the earlier transcript"));
            assert!(summary_payload_text.contains("legacy-marker"));
            write_json_response(
                &mut summary,
                r#"{"content":[{"type":"text","text":"Earlier work established the legacy-marker fixture and its completed result."}],"stop_reason":"end_turn","usage":{"input_tokens":6,"output_tokens":2}}"#,
            );

            let (mut first, _) = listener.accept().expect("first model request arrives");
            server_model_calls.fetch_add(1, Ordering::SeqCst);
            let first_payload = read_json_request(&mut first);
            assert_eq!(first_payload["messages"][0]["role"], "user");
            let first_payload_text = first_payload.to_string();
            assert!(first_payload_text.contains("Earlier work established"));
            assert!(first_payload_text.contains("inspect"));
            assert!(
                first_payload_text.matches("legacy-marker").count() <= 2,
                "raw historical turns must not be sent to the model"
            );
            let tool_names = first_payload["tools"]
                .as_array()
                .expect("model tools are an array")
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                tool_names,
                BTreeSet::from(["ask_user_question", "workspace.inspect"])
            );
            write_json_response(
                &mut first,
                r#"{"content":[{"type":"tool_use","id":"ask-1","name":"ask_user_question","input":{"question":"Which target?","options":["A","B"]}}],"stop_reason":"tool_use","usage":{"input_tokens":2,"output_tokens":1}}"#,
            );

            let (mut second_summary, _) = listener
                .accept()
                .expect("interaction resume summary request arrives");
            server_model_calls.fetch_add(1, Ordering::SeqCst);
            let second_summary_payload = read_json_request(&mut second_summary);
            assert!(
                second_summary_payload
                    .to_string()
                    .contains("Summarize the earlier transcript"),
                "unexpected interaction resume request: {second_summary_payload}"
            );
            write_json_response(
                &mut second_summary,
                r#"{"content":[{"type":"text","text":"Earlier work established the fixture; the user then selected target A."}],"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":2}}"#,
            );

            let (mut second, _) = listener.accept().expect("second model request arrives");
            server_model_calls.fetch_add(1, Ordering::SeqCst);
            let second_payload = read_json_request(&mut second);
            let second_messages = second_payload["messages"]
                .as_array()
                .expect("second model messages are an array");
            let second_assistant = &second_messages[second_messages.len() - 2];
            let second_tool = &second_messages[second_messages.len() - 1];
            assert_eq!(second_assistant["content"][0]["id"], "ask-1");
            assert_eq!(second_tool["content"][0]["tool_use_id"], "ask-1");
            assert_ne!(second_tool["content"][0]["is_error"], true);
            assert!(
                second_tool["content"][0]["content"]
                    .as_str()
                    .is_some_and(|content| content.contains('A'))
            );
            write_json_response(
                &mut second,
                r#"{"content":[{"type":"tool_use","id":"reference-tool","name":"workspace.inspect","input":{"path":"."}}],"stop_reason":"tool_use","usage":{"input_tokens":4,"output_tokens":1}}"#,
            );

            let (mut third, _) = listener.accept().expect("third model request arrives");
            server_model_calls.fetch_add(1, Ordering::SeqCst);
            let third_payload = read_json_request(&mut third);
            let messages = third_payload["messages"]
                .as_array()
                .expect("model messages are an array");
            let assistant = &messages[messages.len() - 2];
            let tool_result = &messages[messages.len() - 1];
            assert_eq!(assistant["content"][0]["id"], "reference-tool");
            assert_eq!(tool_result["content"][0]["tool_use_id"], "reference-tool");
            assert_eq!(tool_result["content"][0]["is_error"], true);
            assert!(
                tool_result["content"][0]["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("reference.tool_failed"))
            );
            write_json_response(
                &mut third,
                r#"{"content":[{"type":"text","text":"reference-final"}],"stop_reason":"end_turn","usage":{"input_tokens":4,"output_tokens":1}}"#,
            );
        });
        let adapter: Arc<dyn ModelProtocolAdapter> = Arc::new(
            AnthropicMessagesAdapter::new(
                AnthropicMessagesAdapter::default_descriptor(),
                Arc::new(StaticCredential),
            )
            .expect("Anthropic adapter constructs"),
        );
        let provider_id = "anthropic-loopback";
        let model = ModelGateway::with_default_provider(provider_id);
        model.register(Arc::new(
            AdapterBackedModelProvider::new(
                anthropic_provider_descriptor(
                    provider_id,
                    &endpoint,
                    CredentialRef {
                        credential_id: "loopback".into(),
                        revision: 1,
                    },
                    "claude-loopback",
                ),
                adapter,
                vec![interaction_tool.clone(), reference_tool.clone()],
            )
            .expect("adapter-backed provider constructs"),
        ));
        let bundle = AgentPluginBundle {
            agent_loop: AgentLoop::default().with_default_model("claude-loopback"),
            model,
            ..Default::default()
        };
        let profile = AgentRuntimeProfileBuilder::new("reference.profile")
            .context_policy(mutsuki_agent_contracts::AgentContextPolicy {
                provider_ids: Vec::new(),
                budget: ContextBudget {
                    max_tokens: Some(300),
                    max_bytes: None,
                    max_items: None,
                },
                compaction_service: Some("mutsuki.agent.context.compaction@1".into()),
                provider_options: serde_json::json!({}),
            })
            .build()
            .expect("reference profile is valid");
        bundle
            .agent_loop
            .configure_profile(&profile)
            .expect("reference profile configures AgentLoop");
        bundle
            .tools
            .register(interaction_tool)
            .expect("interaction tool registers");
        bundle
            .tools
            .register(reference_tool)
            .expect("reference tool registers");
        bundle
            .context
            .set_tools(bundle.tools.list(Default::default()).tools);
        let session_id = bundle
            .sessions
            .create(AgentSessionCreateRequest {
                session_id: None,
                profile_id: "reference.profile".into(),
                title: Some("approval resume".into()),
            })
            .expect("reference session creates")
            .session_id;

        let deferred = Arc::new(DeferredClient::default());
        let client: RuntimeClientRef = deferred.clone();
        let tool_descriptor =
            orchestration_runner("mutsuki.agent.reference.tools.runner", TOOL_PLUGIN_ID)
                .accepts::<ReferenceToolProtocol>()
                .build();
        let tool_manifest = PluginBuilder::new(TOOL_PLUGIN_ID)
            .protocol::<ReferenceToolProtocol>()
            .runner_descriptor(tool_descriptor.clone())
            .build()
            .manifest;
        let mut manifests = bundle.manifests();
        manifests.push(tool_manifest);
        let mut bootstrapper = RuntimeBootstrapper::new();
        for manifest in &manifests {
            bootstrapper.register_manifest(manifest.clone());
        }
        for kind in AgentRuntimeRunner::ALL {
            bootstrapper.register_builtin_runner(bundle.runtime_runner(kind, client.clone()));
        }
        bootstrapper.register_async_handler(bundle.model_async_handler());
        bootstrapper.register_builtin_runner(Box::new(TaskAwaitRunnerAdapter::new(
            tool_descriptor,
            client,
            Box::new({
                let tool_calls = tool_calls.clone();
                move |_ctx, task| {
                    let tool_calls = tool_calls.clone();
                    Box::pin(async move {
                        tool_calls.fetch_add(1, Ordering::SeqCst);
                        let mut error = RuntimeError::new(
                            "reference.tool_failed",
                            TOOL_PLUGIN_ID,
                            task.task_id,
                        );
                        error.evidence.insert(
                            "message".into(),
                            ScalarValue::String("reference tool failed".into()),
                        );
                        Err(RuntimeFailure::new(error))
                    })
                }
            }),
        )));
        let enabled_plugins = manifests
            .iter()
            .map(|manifest| manifest.plugin_id.clone())
            .collect::<Vec<_>>();
        let profile = RuntimeProfile {
            profile_id: "mutsuki.agent.reference.in-process".into(),
            mode: RuntimeProfileMode::FullDev,
            enabled_plugins: enabled_plugins.clone(),
            bindings: BTreeMap::new(),
            surface_bindings: BTreeMap::new(),
            plugin_deployments: enabled_plugins
                .into_iter()
                .map(|plugin_id| (plugin_id, PluginDeploymentKind::Builtin))
                .collect(),
            observability: Default::default(),
            allow_dynamic_registration: false,
            allow_hot_reload: false,
        };
        let runtime = bootstrapper
            .into_host_runtime_with_config(
                profile,
                HostRuntimeConfig {
                    event_driven: true,
                    async_executor: Some(Arc::new(
                        TokioAsyncExecutor::new(1, 32, 32, 1024 * 1024).expect("async executor"),
                    )),
                    ..HostRuntimeConfig::default()
                },
            )
            .expect("in-process Agent runtime boots");
        deferred.bind(&runtime);

        let mut initial_request = AgentRunRequest::new(
            "reference.profile",
            vec![
                AgentMessage::user(format!("legacy-marker {}", "history ".repeat(600))),
                AgentMessage::assistant(format!("legacy-marker {}", "result ".repeat(600))),
                AgentMessage::user("inspect"),
            ],
        );
        initial_request.session_id = Some(session_id.clone());
        initial_request.turn_id = Some("reference-turn".into());
        initial_request.max_steps = 3;
        initial_request.budget.max_context_tokens = Some(300);
        let handle = runtime
            .submit_task(Task::new(
                "agent-reference-in-process",
                AGENT_RUN_PROTOCOL,
                serde_json::to_value(initial_request).expect("run request serializes"),
            ))
            .expect("Agent run submits");
        let deadline = Instant::now() + Duration::from_secs(2);
        let outcome = loop {
            if let Some(outcome) = runtime
                .task_outcome(&handle)
                .expect("Agent outcome query succeeds")
            {
                break outcome;
            }
            assert!(Instant::now() < deadline, "in-process Agent run timed out");
            std::thread::yield_now();
        };
        let TaskOutcome::Completed {
            output: Some(output),
            ..
        } = outcome
        else {
            panic!("in-process Agent run did not complete: {outcome:?}");
        };
        let waiting: AgentRunResult = serde_json::from_value(output).expect("typed Agent result");
        assert_eq!(waiting.status, AgentRunStatus::WaitingInteraction);
        assert_eq!(waiting.pending_interactions.len(), 1);
        assert!(
            waiting
                .events
                .iter()
                .any(|event| matches!(event.event, AgentEvent::InteractionRequested { .. }))
        );
        let interaction_usage = waiting.usage.total_tokens;
        assert!(interaction_usage > 0);
        assert_eq!(tool_calls.load(Ordering::SeqCst), 0);

        let pending = waiting.pending_interactions[0].clone();
        // Session-backed resume loads the persisted waiting history. Only the
        // interaction resolution is new input; replaying `waiting.messages` would
        // duplicate the assistant tool_use and violate provider causality.
        let mut resume_request = AgentRunRequest::new("reference.profile", Vec::new());
        resume_request.session_id = Some(session_id.clone());
        resume_request.turn_id = Some(pending.turn_id.clone());
        resume_request.max_steps = 100;
        resume_request.budget.max_context_tokens = Some(300);
        resume_request.interaction_resolutions = vec![InteractionResolution {
            session_id: pending.session_id,
            turn_id: pending.turn_id,
            version: pending.version,
            interaction_id: pending.interaction_id,
            accepted: true,
            response: serde_json::json!({ "answer": "A" }),
        }];
        let handle = runtime
            .submit_task(Task::new(
                "agent-reference-interaction-resume",
                AGENT_RUN_PROTOCOL,
                serde_json::to_value(resume_request).expect("interaction resume serializes"),
            ))
            .expect("Agent interaction resume submits");
        let deadline = Instant::now() + Duration::from_secs(2);
        let outcome = loop {
            if let Some(outcome) = runtime
                .task_outcome(&handle)
                .expect("Agent interaction resume outcome query succeeds")
            {
                break outcome;
            }
            assert!(
                Instant::now() < deadline,
                "in-process Agent interaction resume timed out"
            );
            std::thread::yield_now();
        };
        let TaskOutcome::Completed {
            output: Some(output),
            ..
        } = outcome
        else {
            panic!("in-process Agent interaction resume did not complete: {outcome:?}");
        };
        let waiting: AgentRunResult = serde_json::from_value(output).expect("typed Agent result");
        assert_eq!(waiting.status, AgentRunStatus::WaitingApproval);
        assert_eq!(waiting.pending_approvals.len(), 1);
        assert!(
            waiting
                .events
                .iter()
                .any(|event| matches!(event.event, AgentEvent::InteractionResolved { .. }))
        );
        let approval_usage = waiting.usage.total_tokens;
        assert!(approval_usage > interaction_usage);
        assert!(waiting.steps.iter().any(|step| step.step_index == 1));
        assert_eq!(tool_calls.load(Ordering::SeqCst), 0);
        let persisted = bundle
            .sessions
            .get(AgentSessionGetRequest {
                session_id: session_id.clone(),
            })
            .expect("interaction resume persists session state");
        assert!(
            persisted
                .messages
                .iter()
                .any(|message| message.content.starts_with("legacy-marker history")),
            "context compaction must not replace the durable transcript"
        );
        assert_eq!(
            persisted
                .messages
                .iter()
                .filter(|message| {
                    message.metadata.as_ref().is_some_and(|metadata| {
                        metadata.get("interaction_resume_receipt").is_some()
                    })
                })
                .count(),
            1
        );
        assert_eq!(
            persisted
                .events
                .iter()
                .filter(|event| matches!(event.event, AgentEvent::InteractionResolved { .. }))
                .count(),
            1
        );

        let pending = waiting.pending_approvals[0].clone();
        let mut resume_request = AgentRunRequest::new("reference.profile", Vec::new());
        resume_request.session_id = Some(session_id);
        resume_request.turn_id = Some(pending.turn_id.clone());
        resume_request.max_steps = 100;
        resume_request.budget.max_context_tokens = Some(300);
        resume_request.permission_decisions = vec![PermissionDecision {
            session_id: pending.session_id,
            turn_id: pending.turn_id,
            action_id: pending.action_id,
            version: pending.version,
            decision: PermissionDecisionKind::Approved,
        }];
        let handle = runtime
            .submit_task(Task::new(
                "agent-reference-approval-resume",
                AGENT_RUN_PROTOCOL,
                serde_json::to_value(resume_request).expect("resume request serializes"),
            ))
            .expect("Agent resume submits");
        let deadline = Instant::now() + Duration::from_secs(2);
        let outcome = loop {
            if let Some(outcome) = runtime
                .task_outcome(&handle)
                .expect("Agent resume outcome query succeeds")
            {
                break outcome;
            }
            assert!(
                Instant::now() < deadline,
                "in-process Agent resume timed out"
            );
            std::thread::yield_now();
        };
        let TaskOutcome::Completed {
            output: Some(output),
            ..
        } = outcome
        else {
            panic!("in-process Agent resume did not complete: {outcome:?}");
        };
        let result: AgentRunResult = serde_json::from_value(output).expect("typed Agent result");
        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.messages.last().unwrap().content, "reference-final");
        assert!(result.usage.total_tokens > approval_usage);
        assert!(result.steps.iter().any(|step| step.step_index == 2));
        assert_eq!(model_calls.load(Ordering::SeqCst), 5);
        assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
        model_server.join().expect("model server assertions pass");
    }
}
