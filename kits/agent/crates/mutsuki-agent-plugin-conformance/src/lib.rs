use std::sync::atomic::{AtomicBool, Ordering};

use mutsuki_agent_contracts::{
    AgentCommandRequest, AgentCommandResult, AgentError, AgentKitPluginDescriptor,
    AgentPluginStateKind, AgentServiceDescriptor, AgentToolDescriptor, ContextProviderRequest,
    ContextProviderResult, PermissionDecision, PermissionRequest, ToolSideEffect,
};
use mutsuki_agent_plugin_api::{
    AgentCommand, AgentHook, AgentPluginRegistrar, AgentService, AgentServiceRunner,
    ContextProvider, PermissionPolicy, ToolProvider,
};
use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, PluginArtifact, RunnerContext, RunnerDescriptor, ScalarValue,
    WorkBatch,
};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{
    AbiHostClient, PluginBuilder, RunnerDescriptorBuilder, RuntimeResult, map_work_batch_entries,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.conformance";
pub const RUNNER_ID: &str = "mutsuki.agent.conformance.runner";
pub const PROTOCOL_ID: &str = "mutsuki.agent.conformance.echo@1";
pub const SERVICE_ID: &str = "mutsuki.agent.conformance.service@1";

pub struct ConformancePlugin {
    generation: u64,
    drained: AtomicBool,
    disposed: AtomicBool,
}

impl ConformancePlugin {
    pub fn new(generation: u64) -> Result<Self, AgentError> {
        if generation == 0 {
            return Err(AgentError::invalid_input(
                "Mutsuki plugin generation must be non-zero",
            ));
        }
        Ok(Self {
            generation,
            drained: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
        })
    }

    pub fn agent_descriptor(&self) -> Result<AgentKitPluginDescriptor, AgentError> {
        let mut pure = AgentToolDescriptor::new(
            "conformance.echo",
            PROTOCOL_ID,
            "Returns the supplied value without side effects",
        );
        pure.side_effect = ToolSideEffect::None;
        let mut write = AgentToolDescriptor::new(
            "conformance.write",
            PROTOCOL_ID,
            "Conformance-only workspace write plan",
        );
        write.side_effect = ToolSideEffect::WorkspaceWrite;
        write.requires_approval = true;
        write.permissions = vec!["workspace.write".into()];
        AgentPluginRegistrar::new(PLUGIN_ID, self.generation)
            .tool(pure)
            .tool(write)
            .context_provider("conformance.context")
            .hook("conformance.after_turn")
            .policy("conformance.approval")
            .command("conformance.status")
            .service(self.service_descriptor().clone())
            .require_capability("filesystem.read")
            .build()
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }

    fn service_descriptor(&self) -> AgentServiceDescriptor {
        AgentServiceDescriptor {
            service_id: SERVICE_ID.into(),
            version: "1".into(),
            request_schema: "mutsuki.agent.conformance.request@1".into(),
            response_schema: "mutsuki.agent.conformance.response@1".into(),
            state: AgentPluginStateKind::Stateful,
            affinity: Some("session".into()),
        }
    }
}

impl ToolProvider for ConformancePlugin {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        self.agent_descriptor()
            .map(|descriptor| descriptor.tools)
            .unwrap_or_default()
    }
}

impl ContextProvider for ConformancePlugin {
    fn provider_id(&self) -> &str {
        "conformance.context"
    }

    fn collect(
        &self,
        request: ContextProviderRequest,
    ) -> Result<ContextProviderResult, AgentError> {
        Ok(ContextProviderResult {
            provider_id: request.provider_id,
            summary: format!("session={} turn={}", request.session_id, request.turn_id),
            details: None,
            estimated_tokens: 4,
            estimated_bytes: 32,
            priority: 0,
            required: false,
        })
    }
}

impl AgentHook for ConformancePlugin {
    fn hook_id(&self) -> &str {
        "conformance.after_turn"
    }

    fn invoke(&self, event: Value) -> Result<Value, AgentError> {
        Ok(json!({"observed": event}))
    }
}

impl PermissionPolicy for ConformancePlugin {
    fn policy_id(&self) -> &str {
        "conformance.approval"
    }

    fn decide(
        &self,
        _request: &PermissionRequest,
    ) -> Result<Option<PermissionDecision>, AgentError> {
        Ok(None)
    }
}

impl AgentCommand for ConformancePlugin {
    fn command_id(&self) -> &str {
        "conformance.status"
    }

    fn execute(&self, request: AgentCommandRequest) -> Result<AgentCommandResult, AgentError> {
        Ok(AgentCommandResult {
            command_id: request.command_id,
            output: json!({
                "generation": self.generation,
                "drained": self.drained.load(Ordering::Acquire),
                "disposed": self.disposed.load(Ordering::Acquire),
            }),
            output_ref: None,
        })
    }
}

impl AgentService for ConformancePlugin {
    fn descriptor(&self) -> &AgentServiceDescriptor {
        // The descriptor is stable and allocation-free at the contract level;
        // callers should use `agent_descriptor` when they need an owned copy.
        static DESCRIPTOR: std::sync::OnceLock<AgentServiceDescriptor> = std::sync::OnceLock::new();
        DESCRIPTOR.get_or_init(|| self.service_descriptor())
    }

    fn call(&self, request: Value) -> Result<Value, AgentError> {
        if self.disposed.load(Ordering::Acquire) {
            return Err(AgentError::new(
                "agent.plugin.disposed",
                "service was disposed",
            ));
        }
        Ok(json!({"generation": self.generation, "echo": request}))
    }

    fn drain(&self) -> Result<(), AgentError> {
        self.drained.store(true, Ordering::Release);
        Ok(())
    }

    fn dispose(&self) -> Result<(), AgentError> {
        if !self.drained.load(Ordering::Acquire) {
            return Err(AgentError::new(
                "agent.plugin.not_drained",
                "stateful service must drain before dispose",
            ));
        }
        self.disposed.store(true, Ordering::Release);
        Ok(())
    }
}

struct ConformanceRunner {
    descriptor: RunnerDescriptor,
    generation: u64,
}

impl ConformanceRunner {
    fn new(generation: u64) -> Self {
        Self {
            descriptor: RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
                .accepted_protocol(PROTOCOL_ID)
                .build(),
            generation,
        }
    }
}

impl Runner for ConformanceRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let mut result = mutsuki_runtime_contracts::RunnerResult::completed(&task.task_id);
            result.output = Some(json!({
                "generation": self.generation,
                "echo": task.payload.clone(),
            }));
            Ok(result)
        })
    }
}

pub fn plugin_builder(
    generation: u64,
    artifact_type: ArtifactType,
) -> Result<PluginBuilder, AgentError> {
    let plugin = std::sync::Arc::new(ConformancePlugin::new(generation)?);
    let descriptor = plugin.agent_descriptor()?;
    let descriptor_json = serde_json::to_string(&descriptor)
        .map_err(|error| AgentError::invalid_input(error.to_string()))?;
    let artifact = match artifact_type {
        ArtifactType::Abi => PluginArtifact {
            artifact_type,
            path: dynamic_library_file_name().into(),
            sha256: "sha256:agent-conformance".into(),
            companion_artifacts: Vec::new(),
        },
        _ => PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "native".into(),
            sha256: "sha256:native".into(),
            companion_artifacts: Vec::new(),
        },
    };
    Ok(PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ConformanceRunner::new(generation)))
        .runner(Box::new(AgentServiceRunner::new(
            PLUGIN_ID, generation, plugin,
        )?))
        .artifact(artifact)
        .metadata("agentkit.descriptor", ScalarValue::String(descriptor_json)))
}

fn create_abi_plugin(
    _host: AbiHostClient,
    config: Value,
) -> RuntimeResult<mutsuki_runtime_sdk::LoadedPlugin> {
    let generation = config
        .get("generation")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    plugin_builder(generation, ArtifactType::Abi)
        .map(PluginBuilder::build)
        .map_err(|error| {
            mutsuki_runtime_core::RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
                "agent.plugin.invalid_config",
                PLUGIN_ID,
                error.to_string(),
            ))
        })
}

pub const fn dynamic_library_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "mutsuki_agent_plugin_conformance.dll"
    } else if cfg!(target_os = "macos") {
        "libmutsuki_agent_plugin_conformance.dylib"
    } else {
        "libmutsuki_agent_plugin_conformance.so"
    }
}

mutsuki_runtime_sdk::export_mutsuki_plugin_abi_v2!(create_abi_plugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_plugin_covers_every_agent_surface_and_stateful_lifecycle() {
        let plugin = ConformancePlugin::new(7).unwrap();
        let descriptor = plugin.agent_descriptor().unwrap();
        assert_eq!(descriptor.tools.len(), 2);
        assert!(descriptor.tools.iter().any(|tool| tool.requires_approval));
        assert_eq!(descriptor.context_providers.len(), 1);
        assert_eq!(descriptor.hooks.len(), 1);
        assert_eq!(descriptor.policies.len(), 1);
        assert_eq!(descriptor.commands.len(), 1);
        assert_eq!(descriptor.services.len(), 1);
        assert_eq!(
            plugin.dispose().unwrap_err().code,
            "agent.plugin.not_drained"
        );
        plugin.drain().unwrap();
        plugin.dispose().unwrap();
        assert!(plugin.is_disposed());
        assert_eq!(
            plugin.call(json!({})).unwrap_err().code,
            "agent.plugin.disposed"
        );
    }
}
