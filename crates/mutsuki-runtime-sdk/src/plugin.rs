use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    ArtifactType, HandlerBinding, LifecyclePolicy, PermissionGrant, PluginArtifact, PluginManifest,
    PluginProvides, ProtocolDescriptor, ResourceTypeDescriptor, RunnerDescriptor, ScalarValue,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};

use crate::{HostService, ProtocolSpec, ResourceKindSpec, ResourceProviderGateway};

pub struct RuntimeBootstrapperService {
    pub service_id: String,
    pub capability: Option<String>,
    pub service: Arc<dyn std::any::Any + Send + Sync>,
}

pub struct RuntimeBootstrapperResourceProvider {
    pub provider_id: String,
    pub provider: Arc<dyn ResourceProviderGateway>,
}

pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub runners: Vec<Box<dyn Runner>>,
    pub host_services: Vec<RuntimeBootstrapperService>,
    pub resource_providers: Vec<RuntimeBootstrapperResourceProvider>,
}

pub trait Plugin: Send {
    fn load(self: Box<Self>) -> RuntimeResult<LoadedPlugin>;
}

pub trait PluginLoader: Send {
    fn load_plugins(&mut self) -> RuntimeResult<Vec<LoadedPlugin>>;
}

#[derive(Default)]
pub struct BuiltinPluginLoader {
    plugins: Vec<Box<dyn Plugin>>,
}

impl BuiltinPluginLoader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_plugin(mut self, plugin: Box<dyn Plugin>) -> Self {
        self.plugins.push(plugin);
        self
    }

    pub fn push(&mut self, plugin: Box<dyn Plugin>) {
        self.plugins.push(plugin);
    }
}

impl PluginLoader for BuiltinPluginLoader {
    fn load_plugins(&mut self) -> RuntimeResult<Vec<LoadedPlugin>> {
        self.plugins.drain(..).map(|plugin| plugin.load()).collect()
    }
}

pub struct PluginBuilder {
    plugin_id: String,
    version: String,
    api_version: String,
    artifact: PluginArtifact,
    provides: PluginProvides,
    requires: Vec<String>,
    permissions: PermissionGrant,
    lifecycle: LifecyclePolicy,
    metadata: BTreeMap<String, ScalarValue>,
    runners: Vec<Box<dyn Runner>>,
    host_services: Vec<RuntimeBootstrapperService>,
    resource_providers: Vec<RuntimeBootstrapperResourceProvider>,
}

impl PluginBuilder {
    pub fn new(plugin_id: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            version: "0.1.0".into(),
            api_version: "mutsuki-plugin-v1".into(),
            artifact: PluginArtifact {
                artifact_type: ArtifactType::Native,
                path: "native".into(),
                sha256: "sha256:native".into(),
            },
            provides: PluginProvides::default(),
            requires: Vec::new(),
            permissions: PermissionGrant {
                effects: Vec::new(),
                resources: Vec::new(),
            },
            lifecycle: LifecyclePolicy {
                reload_policy: "drain_and_swap".into(),
                unload_timeout_ms: 5000,
                supports_cancel: true,
                supports_dispose: true,
                supports_snapshot: false,
            },
            metadata: BTreeMap::new(),
            runners: Vec::new(),
            host_services: Vec::new(),
            resource_providers: Vec::new(),
        }
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn api_version(mut self, api_version: impl Into<String>) -> Self {
        self.api_version = api_version.into();
        self
    }

    pub fn artifact(mut self, artifact: PluginArtifact) -> Self {
        self.artifact = artifact;
        self
    }

    pub fn requires(mut self, capability: impl Into<String>) -> Self {
        self.requires.push(capability.into());
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: ScalarValue) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    pub fn permissions(mut self, permissions: PermissionGrant) -> Self {
        self.permissions = permissions;
        self
    }

    pub fn lifecycle(mut self, lifecycle: LifecyclePolicy) -> Self {
        self.lifecycle = lifecycle;
        self
    }

    pub fn runner(mut self, runner: Box<dyn Runner>) -> Self {
        self.provides.runners.push(runner.descriptor().clone());
        self.runners.push(runner);
        self
    }

    pub fn runner_descriptor(mut self, descriptor: RunnerDescriptor) -> Self {
        self.provides.runners.push(descriptor);
        self
    }

    pub fn protocol<P>(mut self) -> Self
    where
        P: ProtocolSpec,
    {
        self.provides.protocols.push(P::descriptor());
        self
    }

    pub fn protocol_descriptor(mut self, descriptor: ProtocolDescriptor) -> Self {
        self.provides.protocols.push(descriptor);
        self
    }

    pub fn handler_binding(mut self, binding: HandlerBinding) -> Self {
        self.provides.handler_bindings.push(binding);
        self
    }

    pub fn resource_type<R>(mut self) -> Self
    where
        R: ResourceKindSpec,
    {
        self.provides.resource_types.push(R::descriptor());
        self
    }

    pub fn resource_type_descriptor(mut self, descriptor: ResourceTypeDescriptor) -> Self {
        self.provides.resource_types.push(descriptor);
        self
    }

    pub fn resource_provider(mut self, provider_id: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        if !self
            .provides
            .resource_providers
            .iter()
            .any(|known| known == &provider_id)
        {
            self.provides.resource_providers.push(provider_id);
        }
        self
    }

    pub fn resource_provider_gateway(
        mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn ResourceProviderGateway>,
    ) -> Self {
        let provider_id = provider_id.into();
        if !self
            .provides
            .resource_providers
            .iter()
            .any(|known| known == &provider_id)
        {
            self.provides.resource_providers.push(provider_id.clone());
        }
        self.resource_providers
            .push(RuntimeBootstrapperResourceProvider {
                provider_id,
                provider,
            });
        self
    }

    pub fn provides(mut self, provides: PluginProvides) -> Self {
        self.provides = provides;
        self
    }

    pub fn host_service<T>(
        mut self,
        service_id: impl Into<String>,
        service: Arc<T>,
        capability: Option<String>,
    ) -> Self
    where
        T: HostService,
    {
        self.host_services.push(RuntimeBootstrapperService {
            service_id: service_id.into(),
            capability,
            service,
        });
        self
    }

    pub fn build(self) -> LoadedPlugin {
        LoadedPlugin {
            manifest: PluginManifest {
                plugin_id: self.plugin_id,
                version: self.version,
                api_version: self.api_version,
                artifact: self.artifact,
                provides: self.provides,
                requires: self.requires,
                permissions: self.permissions,
                lifecycle: self.lifecycle,
                metadata: self.metadata,
            },
            runners: self.runners,
            host_services: self.host_services,
            resource_providers: self.resource_providers,
        }
    }
}

impl Plugin for PluginBuilder {
    fn load(self: Box<Self>) -> RuntimeResult<LoadedPlugin> {
        Ok((*self).build())
    }
}
