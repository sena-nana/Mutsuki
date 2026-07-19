use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    ArtifactType, BridgeDescriptor, CodecDescriptor, HandlerBinding, HostExtensionDescriptor,
    HostExtensionKind, LifecyclePolicy, PermissionGrant, PluginArtifact, PluginBackendDescriptor,
    PluginDeploymentKind, PluginManifest, PluginProvides, ProtocolDescriptor,
    ResourceTypeDescriptor, RunnerDescriptor, ScalarValue,
};
use mutsuki_runtime_core::{AsyncBatchHandler, Runner, RuntimeResult};

use crate::{
    AsyncResourceProviderGateway, HandlerBindingBuilder, HostService, ProtocolSpec,
    ResourceKindSpec, ResourceProviderGateway,
};

pub struct RuntimeBootstrapperService {
    pub service_id: String,
    pub capability: Option<String>,
    pub service: Arc<dyn std::any::Any + Send + Sync>,
}

pub struct RuntimeBootstrapperResourceProvider {
    pub provider_id: String,
    pub provider: Arc<dyn ResourceProviderGateway>,
}

pub struct RuntimeBootstrapperAsyncResourceProvider {
    pub provider_id: String,
    pub provider: Arc<dyn AsyncResourceProviderGateway>,
}

pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub runners: Vec<Box<dyn Runner>>,
    pub async_handlers: Vec<Arc<dyn AsyncBatchHandler>>,
    pub host_services: Vec<RuntimeBootstrapperService>,
    pub resource_providers: Vec<RuntimeBootstrapperResourceProvider>,
    pub async_resource_providers: Vec<RuntimeBootstrapperAsyncResourceProvider>,
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
    async_handlers: Vec<Arc<dyn AsyncBatchHandler>>,
    host_services: Vec<RuntimeBootstrapperService>,
    resource_providers: Vec<RuntimeBootstrapperResourceProvider>,
    async_resource_providers: Vec<RuntimeBootstrapperAsyncResourceProvider>,
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
            async_handlers: Vec::new(),
            host_services: Vec::new(),
            resource_providers: Vec::new(),
            async_resource_providers: Vec::new(),
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

    pub fn async_handler(mut self, handler: Arc<dyn AsyncBatchHandler>) -> Self {
        self.provides.runners.push(handler.descriptor().clone());
        self.async_handlers.push(handler);
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

    pub fn protocol_handler(
        mut self,
        descriptor: ProtocolDescriptor,
        target_runner_hint: impl Into<String>,
        pool_id: impl Into<String>,
    ) -> Self {
        let protocol_id = descriptor.protocol_id.clone();
        self.provides.protocols.push(descriptor);
        self.provides.handler_bindings.push(
            HandlerBindingBuilder::new(
                format!("binding:{protocol_id}"),
                self.plugin_id.clone(),
                protocol_id.clone(),
                protocol_id,
            )
            .target_runner_hint(target_runner_hint)
            .pool_id(pool_id)
            .build(),
        );
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

    pub fn async_resource_provider_gateway(
        mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn AsyncResourceProviderGateway>,
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
        self.async_resource_providers
            .push(RuntimeBootstrapperAsyncResourceProvider {
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

    pub fn build(mut self) -> LoadedPlugin {
        self.ensure_plugin_backend();
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
            async_handlers: self.async_handlers,
            host_services: self.host_services,
            resource_providers: self.resource_providers,
            async_resource_providers: self.async_resource_providers,
        }
    }

    fn ensure_plugin_backend(&mut self) {
        if self.provides.runners.is_empty() {
            return;
        }
        let deployment = PluginDeploymentKind::default_for_artifact(&self.artifact.artifact_type);
        let deployment_name = match deployment {
            PluginDeploymentKind::Builtin => "builtin",
            PluginDeploymentKind::Abi => "abi",
            PluginDeploymentKind::Wasm => "wasm",
            PluginDeploymentKind::Process => "process",
            PluginDeploymentKind::Python => "python",
        };
        let backend_id = format!("plugin.backend.{}.{deployment_name}", self.plugin_id);
        if !self.provides.host_extensions.iter().any(|extension| {
            extension.extension_id == format!("host.extension.{}.{deployment_name}", self.plugin_id)
        }) {
            self.provides.host_extensions.push(HostExtensionDescriptor {
                extension_id: format!("host.extension.{}.{deployment_name}", self.plugin_id),
                kind: HostExtensionKind::PluginBackend,
                supported_deployments: vec![deployment.clone()],
                reload_policy: "static".into(),
                drain_required: false,
            });
        }
        if !self
            .provides
            .plugin_backends
            .iter()
            .any(|backend| backend.backend_id == backend_id)
        {
            self.provides.plugin_backends.push(PluginBackendDescriptor {
                backend_id,
                deployment_kind: deployment.clone(),
                task_client_protocol: "mutsuki.task.v1".into(),
                resource_client_protocol: "mutsuki.resource-plan.v1".into(),
                codec_id: (deployment == PluginDeploymentKind::Abi)
                    .then(|| crate::abi::ABI_V2_CODEC_ID.into()),
                bridge_id: (deployment == PluginDeploymentKind::Abi)
                    .then(|| crate::abi::ABI_V2_BRIDGE_ID.into()),
            });
        }
        if deployment == PluginDeploymentKind::Abi {
            if !self
                .provides
                .codecs
                .iter()
                .any(|codec| codec.codec_id == crate::abi::ABI_V2_CODEC_ID)
            {
                self.provides.codecs.push(CodecDescriptor {
                    codec_id: crate::abi::ABI_V2_CODEC_ID.into(),
                    media_type: "application/msgpack".into(),
                    version: crate::abi::ABI_V2_TRANSPORT_VERSION.to_string(),
                    connection_scoped: true,
                });
            }
            if !self
                .provides
                .bridges
                .iter()
                .any(|bridge| bridge.bridge_id == crate::abi::ABI_V2_BRIDGE_ID)
            {
                self.provides.bridges.push(BridgeDescriptor {
                    bridge_id: crate::abi::ABI_V2_BRIDGE_ID.into(),
                    deployment_kind: PluginDeploymentKind::Abi,
                    codec_ids: vec![crate::abi::ABI_V2_CODEC_ID.into()],
                    drain_policy: "drain_and_swap".into(),
                });
            }
        }
    }
}

impl Plugin for PluginBuilder {
    fn load(self: Box<Self>) -> RuntimeResult<LoadedPlugin> {
        Ok((*self).build())
    }
}
