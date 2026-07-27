use std::collections::{BTreeMap, BTreeSet};

pub use mutsuki_agent_contracts::{
    AgentCommandRequest, AgentCommandResult, AgentError, AgentKitPluginDescriptor,
    AgentServiceDescriptor, AgentToolDescriptor, ContextProviderRequest, ContextProviderResult,
    PermissionDecision, PermissionRequest,
};
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
