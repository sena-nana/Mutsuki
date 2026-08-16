use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use mutsuki_agent_contracts::{
    AgentCommandRequest, AgentCommandResult, AgentError, AgentKitPluginDescriptor,
    AgentServiceDescriptor, AgentToolDescriptor, ContextProviderRequest, ContextProviderResult,
    PermissionDecision, PermissionRequest,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, PluginId, RunnerContext, RunnerDescriptor, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{RunnerDescriptorBuilder, map_work_batch_entries};
use serde_json::Value;

pub trait ToolProvider: Send + Sync {
    fn tools(&self) -> Vec<AgentToolDescriptor>;
}

pub trait ContextProvider: Send + Sync {
    fn provider_id(&self) -> &str;
    fn collect(&self, request: ContextProviderRequest)
    -> Result<ContextProviderResult, AgentError>;
}

pub trait AgentHook: Send + Sync {
    fn hook_id(&self) -> &str;
    fn invoke(&self, event: Value) -> Result<Value, AgentError>;
}

pub trait PermissionPolicy: Send + Sync {
    fn policy_id(&self) -> &str;
    fn decide(&self, request: &PermissionRequest)
    -> Result<Option<PermissionDecision>, AgentError>;
}

pub trait AgentCommand: Send + Sync {
    fn command_id(&self) -> &str;
    fn execute(&self, request: AgentCommandRequest) -> Result<AgentCommandResult, AgentError>;
}

pub trait AgentService: Send + Sync {
    fn descriptor(&self) -> &AgentServiceDescriptor;
    fn call(&self, request: Value) -> Result<Value, AgentError>;
    fn drain(&self) -> Result<(), AgentError>;
    fn dispose(&self) -> Result<(), AgentError>;
}

/// Runs one Agent service through the normal Mutsuki task and generation lifecycle.
///
/// The adapter is the service instance's lifecycle owner: Core/Host drain the owning plugin
/// generation before calling `Runner::dispose`, which then drains and disposes the service. It is
/// intentionally a Runner instead of a second Agent-specific loader or service registry, so
/// builtin and external deployments expose the same batch-first business surface.
pub struct AgentServiceRunner {
    descriptor: RunnerDescriptor,
    service_id: String,
    service: Arc<dyn AgentService>,
    disposed: bool,
}

impl AgentServiceRunner {
    pub fn new<S>(
        plugin_id: impl Into<PluginId>,
        plugin_generation: u64,
        service: Arc<S>,
    ) -> Result<Self, AgentError>
    where
        S: AgentService + 'static,
    {
        if plugin_generation == 0 {
            return Err(AgentError::invalid_input(
                "Agent service requires a non-zero Mutsuki plugin generation",
            ));
        }
        let service_id = service.descriptor().service_id.clone();
        if service_id.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "Agent service requires a non-empty service id",
            ));
        }
        Ok(Self {
            descriptor: RunnerDescriptorBuilder::new(format!("{service_id}.runner"), plugin_id)
                .plugin_generation(plugin_generation)
                .accepted_protocol(service_id.clone())
                .build(),
            service_id,
            service,
            disposed: false,
        })
    }
}

impl Runner for AgentServiceRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let service_id = self.service_id.clone();
        let service = self.service.clone();
        map_work_batch_entries(&batch, move |task| {
            if task.protocol_id != service_id {
                return Err(agent_runtime_error(AgentError::invalid_input(format!(
                    "service `{service_id}` cannot handle protocol `{}`",
                    task.protocol_id
                ))));
            }
            let output = service
                .call(task.payload.clone().into())
                .map_err(agent_runtime_error)?;
            let mut result =
                mutsuki_runtime_contracts::RunnerResult::completed(task.task_id.clone());
            result.output = Some(output);
            Ok(result)
        })
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        if self.disposed {
            return Ok(());
        }
        self.service.drain().map_err(agent_runtime_failure)?;
        self.service.dispose().map_err(agent_runtime_failure)?;
        self.disposed = true;
        Ok(())
    }
}

fn agent_runtime_failure(error: AgentError) -> RuntimeFailure {
    RuntimeFailure::new(agent_runtime_error(error))
}

fn agent_runtime_error(error: AgentError) -> mutsuki_runtime_contracts::RuntimeError {
    mutsuki_runtime_contracts::RuntimeError::new(error.code, "mutsuki.agent.service", error.message)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentPluginGenerationSet {
    generations: BTreeMap<String, u64>,
}

impl AgentPluginGenerationSet {
    pub fn pin<'a>(
        descriptors: impl IntoIterator<Item = &'a AgentKitPluginDescriptor>,
    ) -> Result<Self, AgentError> {
        let mut generations = BTreeMap::new();
        for descriptor in descriptors {
            if descriptor.plugin_id.trim().is_empty()
                || descriptor.generation == 0
                || generations
                    .insert(descriptor.plugin_id.clone(), descriptor.generation)
                    .is_some()
            {
                return Err(AgentError::invalid_input(
                    "turn plugin generations require unique ids and non-zero generations",
                ));
            }
        }
        Ok(Self { generations })
    }

    pub fn generation(&self, plugin_id: &str) -> Option<u64> {
        self.generations.get(plugin_id).copied()
    }

    pub fn validate(&self, descriptor: &AgentKitPluginDescriptor) -> Result<(), AgentError> {
        match self.generation(&descriptor.plugin_id) {
            Some(generation) if generation == descriptor.generation => Ok(()),
            Some(_) => Err(AgentError::new(
                "agent.plugin.generation_changed",
                "plugin generation cannot change within an active turn",
            )),
            None => Err(AgentError::not_found(
                "plugin was not pinned for the active turn",
            )),
        }
    }
}

pub struct AgentPluginRegistrar {
    descriptor: AgentKitPluginDescriptor,
}

impl AgentPluginRegistrar {
    pub fn new(plugin_id: impl Into<String>, generation: u64) -> Self {
        Self {
            descriptor: AgentKitPluginDescriptor {
                plugin_id: plugin_id.into(),
                generation,
                tools: Vec::new(),
                context_providers: Vec::new(),
                hooks: Vec::new(),
                policies: Vec::new(),
                commands: Vec::new(),
                services: Vec::new(),
                required_capabilities: Vec::new(),
                required_services: Vec::new(),
            },
        }
    }

    pub fn tool(mut self, descriptor: AgentToolDescriptor) -> Self {
        self.descriptor.tools.push(descriptor);
        self
    }

    pub fn context_provider(mut self, provider_id: impl Into<String>) -> Self {
        self.descriptor.context_providers.push(provider_id.into());
        self
    }

    pub fn hook(mut self, hook_id: impl Into<String>) -> Self {
        self.descriptor.hooks.push(hook_id.into());
        self
    }

    pub fn policy(mut self, policy_id: impl Into<String>) -> Self {
        self.descriptor.policies.push(policy_id.into());
        self
    }

    pub fn command(mut self, command_id: impl Into<String>) -> Self {
        self.descriptor.commands.push(command_id.into());
        self
    }

    pub fn service(mut self, descriptor: AgentServiceDescriptor) -> Self {
        self.descriptor.services.push(descriptor);
        self
    }

    pub fn require_capability(mut self, capability: impl Into<String>) -> Self {
        self.descriptor
            .required_capabilities
            .push(capability.into());
        self
    }

    pub fn require_service(mut self, service: impl Into<String>) -> Self {
        self.descriptor.required_services.push(service.into());
        self
    }

    pub fn build(self) -> Result<AgentKitPluginDescriptor, AgentError> {
        if self.descriptor.plugin_id.trim().is_empty() || self.descriptor.generation == 0 {
            return Err(AgentError::invalid_input(
                "plugin id and non-zero Mutsuki generation are required",
            ));
        }
        let mut surface_ids = BTreeSet::new();
        for id in self
            .descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .chain(self.descriptor.context_providers.iter().map(String::as_str))
            .chain(self.descriptor.hooks.iter().map(String::as_str))
            .chain(self.descriptor.policies.iter().map(String::as_str))
            .chain(self.descriptor.commands.iter().map(String::as_str))
            .chain(
                self.descriptor
                    .services
                    .iter()
                    .map(|service| service.service_id.as_str()),
            )
        {
            if id.trim().is_empty() || !surface_ids.insert(id) {
                return Err(AgentError::invalid_input(format!(
                    "duplicate or empty Agent plugin surface `{id}`"
                )));
            }
        }
        Ok(self.descriptor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::{AgentPluginStateKind, ToolSideEffect};

    #[test]
    fn registrar_covers_all_surfaces_and_preserves_mutsuki_generation() {
        let mut tool = AgentToolDescriptor::new("read", "workspace.read@1", "read workspace");
        tool.side_effect = ToolSideEffect::WorkspaceRead;
        let descriptor = AgentPluginRegistrar::new("mutsuki.plugin.agent.conformance", 7)
            .tool(tool)
            .context_provider("workspace")
            .hook("after-turn")
            .policy("approval")
            .command("status")
            .service(AgentServiceDescriptor {
                service_id: "test-service".into(),
                version: "1".into(),
                request_schema: "test.request@1".into(),
                response_schema: "test.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("session".into()),
            })
            .build()
            .unwrap();
        assert_eq!(descriptor.generation, 7);
        assert_eq!(descriptor.tools.len(), 1);
        assert_eq!(descriptor.services.len(), 1);
    }

    #[test]
    fn generation_set_rejects_hot_reload_within_a_turn() {
        let original = AgentPluginRegistrar::new("plugin", 7).build().unwrap();
        let reloaded = AgentPluginRegistrar::new("plugin", 8).build().unwrap();
        let pinned = AgentPluginGenerationSet::pin([&original]).unwrap();
        pinned.validate(&original).unwrap();
        assert_eq!(
            pinned.validate(&reloaded).unwrap_err().code,
            "agent.plugin.generation_changed"
        );
    }
}
