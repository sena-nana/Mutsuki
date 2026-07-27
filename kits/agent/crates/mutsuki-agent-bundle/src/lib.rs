use std::sync::Arc;

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

/// Host-neutral collection of Agent services and plugin manifests.
///
/// Product crates own runtime registration, health presentation, provider
/// options, and secret acquisition. AgentKit does not read or manage product
/// configuration.
#[derive(Clone, Default)]
pub struct AgentPluginBundle {
    pub context: ContextBuilder,
    pub agent_loop: AgentLoop,
    pub memory: MemoryRouter,
    pub model: ModelGateway,
    pub prompts: PromptRegistry,
    pub sessions: SessionStore,
    pub tools: ToolRegistry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRuntimeRunner {
    Context,
    Loop,
    Memory,
    Prompt,
    Session,
    Tool,
}

impl AgentRuntimeRunner {
    pub const ALL: [Self; 6] = [
        Self::Context,
        Self::Loop,
        Self::Memory,
        Self::Prompt,
        Self::Session,
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
            AgentRuntimeRunner::Tool => take_runner(mutsuki_plugin_agent_tool_router::plugin(
                client,
                self.tools.clone(),
            )),
        }
    }

    pub fn model_async_handler(&self) -> Arc<dyn AsyncBatchHandler> {
        mutsuki_plugin_agent_model_gateway::async_handler(self.model.clone())
    }

    pub fn runner_ids() -> [&'static str; 7] {
        [
            mutsuki_plugin_agent_context::RUNNER_ID,
            mutsuki_plugin_agent_loop::RUNNER_ID,
            mutsuki_plugin_agent_memory_router::RUNNER_ID,
            mutsuki_plugin_agent_model_gateway::RUNNER_ID,
            mutsuki_plugin_agent_prompt::RUNNER_ID,
            mutsuki_plugin_agent_session::RUNNER_ID,
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
    use std::collections::BTreeSet;

    #[test]
    fn standard_bundle_has_unique_batch_first_manifests() {
        let manifests = AgentPluginBundle::default().manifests();
        assert_eq!(manifests.len(), 7);
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
}
