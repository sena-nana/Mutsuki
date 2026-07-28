use std::sync::Arc;

mod native_coding;

pub use mutsuki_agent_runtime::{KnowledgeService, SkillRegistry};
pub use mutsuki_plugin_agent_context::ContextBuilder;
pub use mutsuki_plugin_agent_loop::AgentLoop;
pub use mutsuki_plugin_agent_memory_router::MemoryRouter;
pub use mutsuki_plugin_agent_model_gateway::{
    HttpModelProvider, HttpModelProviderOptions, ModelGateway,
};
pub use mutsuki_plugin_agent_prompt::PromptRegistry;
pub use mutsuki_plugin_agent_session::SessionStore;
pub use mutsuki_plugin_agent_tool_router::ToolRegistry;
use mutsuki_runtime_contracts::{PluginManifest, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_runtime_core::{AsyncBatchHandler, Runner};
use mutsuki_runtime_sdk::{RuntimeClient, RuntimeClientRef, RuntimeResult};
pub use native_coding::{
    LSP_PLUGIN_ID, NATIVE_CODING_BUNDLE_ID, NativeCodingAgentBundle, NativeCodingBackends,
    UnavailableLspFactory, UnavailableMcpFactory, run_fix_golden_path,
    run_resume_without_duplicate_side_effects, run_review_golden_path, seed_fix_fixture,
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
    use mutsuki_agent_contracts::{
        AGENT_RUN_PROTOCOL, AgentError, AgentMessage, AgentModelGenerateRequest,
        AgentModelGenerateResult, AgentModelStopReason, AgentRole, AgentRunRequest, AgentRunResult,
        AgentRunStatus, AgentToolCall, AgentToolDescriptor, AgentUsage,
    };
    use mutsuki_agent_sdk::orchestration_runner;
    use mutsuki_plugin_agent_model_gateway::{ModelProvider, ModelProviderFuture};
    use mutsuki_runtime_contracts::{
        PluginDeploymentKind, RunnerResult, RuntimeProfile, RuntimeProfileMode, Task, TaskOutcome,
    };
    use mutsuki_runtime_host::{HostRuntimeConfig, RuntimeBootstrapper, TokioAsyncExecutor};
    use mutsuki_runtime_sdk::{
        HostRuntime as _, PluginBuilder, ProtocolSpec, RuntimeClientRef, RuntimeFailure,
        SdkProtocol, TaskAwaitRunnerAdapter, TaskSubmitterRuntimeClient,
    };
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
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

    #[derive(Clone)]
    struct ReferenceProvider {
        calls: Arc<AtomicUsize>,
    }

    impl ModelProvider for ReferenceProvider {
        fn provider_id(&self) -> &str {
            "reference"
        }

        fn generate(
            &self,
            request: AgentModelGenerateRequest,
        ) -> Result<AgentModelGenerateResult, AgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let completed_tool = request
                .messages
                .iter()
                .any(|message| message.role == AgentRole::Tool);
            Ok(AgentModelGenerateResult {
                message: AgentMessage::assistant(if completed_tool {
                    "reference-final"
                } else {
                    ""
                }),
                stop_reason: if completed_tool {
                    AgentModelStopReason::Stop
                } else {
                    AgentModelStopReason::ToolCalls
                },
                tool_calls: if completed_tool {
                    Vec::new()
                } else {
                    vec![AgentToolCall {
                        call_id: "reference-tool".into(),
                        name: "workspace.inspect".into(),
                        input: json!({"path": "."}),
                    }]
                },
                usage: AgentUsage {
                    input_tokens: 2,
                    output_tokens: 1,
                    total_tokens: 3,
                },
                cost_microunits: 1,
                raw: None,
                output_resource: None,
            })
        }

        fn generate_async(&self, request: AgentModelGenerateRequest) -> ModelProviderFuture {
            let result = self.generate(request);
            Box::pin(async move { result })
        }
    }

    #[test]
    fn in_process_core_runs_agent_runtime_through_public_bootstrapper() {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let model = ModelGateway::with_default_provider("reference");
        model.register(Arc::new(ReferenceProvider {
            calls: model_calls.clone(),
        }));
        let bundle = AgentPluginBundle {
            agent_loop: AgentLoop::default().with_default_model("reference-model"),
            model,
            ..Default::default()
        };
        bundle
            .tools
            .register(AgentToolDescriptor::new(
                "workspace.inspect",
                TOOL_PROTOCOL,
                "Reads the public workspace view",
            ))
            .expect("reference tool registers");
        bundle
            .context
            .set_tools(bundle.tools.list(Default::default()).tools);

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
                        let mut result = RunnerResult::completed(task.task_id);
                        result.output = Some(json!({"path": task.payload["path"]}));
                        Ok(result)
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

        let handle = runtime
            .submit_task(Task::new(
                "agent-reference-in-process",
                AGENT_RUN_PROTOCOL,
                serde_json::to_value(AgentRunRequest::new(
                    "reference.profile",
                    vec![AgentMessage::user("inspect")],
                ))
                .expect("run request serializes"),
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
        let result: AgentRunResult = serde_json::from_value(output).expect("typed Agent result");
        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.messages.last().unwrap().content, "reference-final");
        assert_eq!(model_calls.load(Ordering::SeqCst), 2);
        assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    }
}
