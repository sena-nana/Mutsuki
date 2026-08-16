use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use mutsuki_runtime_contracts::{
    CompletionBatch, ContractSurface, ContractSurfaceKind, PluginDeploymentKind, PluginId,
    PluginManifest, RequirementBinding, RequirementKind, RunnerDescriptor, RuntimeLoadPlan,
    RuntimeProfile, WorkBatch,
};
use mutsuki_runtime_core::{
    AsyncBatchHandler, AsyncCompletionFuture, CoreKernelRunner, CoreRuntime, Runner, RunnerContext,
    RunnerIsolation, RunnerManagementHandle, RuntimeResult,
};
use mutsuki_runtime_sdk::{
    AsyncResourceProviderGateway, HostEffect, HostEffectKind, HostServiceRegistry, LoadedPlugin,
    PluginLoader, ResourceProviderGateway, RuntimeBootstrapperEffect, RuntimeBootstrapperService,
};

use crate::capabilities::HostCapabilityRegistry;
use crate::error::{
    capability_provider_missing, capability_pruned, deployment_mismatch,
    resource_provider_duplicate, resource_provider_missing, runner_for_disabled_plugin,
    runner_missing_for_deployment,
};
use crate::host::{HostRuntime, HostRuntimeConfig};
use crate::resolver::{core_manifest, resolve_load_plan};
use crate::scheduler::{DefaultScheduler, RunnerLimits, SchedulerPolicy};
use crate::scope::{PluginLifetime, PluginScopeManager, ScopeId, ServiceDependency};

#[derive(Clone)]
pub struct PluginScopeSet {
    manager: PluginScopeManager,
    root_scope: ScopeId,
    plugin_scopes: BTreeMap<PluginId, ScopeId>,
    activation_order: Vec<ScopeId>,
}

impl PluginScopeSet {
    #[must_use]
    pub fn manager(&self) -> &PluginScopeManager {
        &self.manager
    }

    #[must_use]
    pub fn root_scope(&self) -> &ScopeId {
        &self.root_scope
    }

    #[must_use]
    pub fn plugin_scope(&self, plugin_id: impl AsRef<str>) -> Option<&ScopeId> {
        self.plugin_scopes.get(plugin_id.as_ref())
    }

    pub(crate) fn has_live_plugins(&self) -> bool {
        self.plugin_scopes
            .values()
            .any(|scope_id| self.manager.snapshot(scope_id).is_some())
    }

    pub(crate) fn commit_activation(&self) -> RuntimeResult<()> {
        self.manager.commit_activation(&self.root_scope)?;
        for scope_id in &self.activation_order {
            self.manager.commit_activation(scope_id)?;
        }
        Ok(())
    }

    pub(crate) fn mark_activation_failed(&self, detail: &str) {
        let _ = self
            .manager
            .mark_scope_failed_dirty(&self.root_scope, detail.to_string());
        for scope_id in &self.activation_order {
            let _ = self
                .manager
                .mark_scope_failed_dirty(scope_id, detail.to_string());
        }
    }

    pub(crate) fn has_dirty_scope(&self) -> bool {
        self.plugin_scopes.values().any(|scope_id| {
            self.manager
                .snapshot(scope_id)
                .is_some_and(|scope| scope.state == crate::ScopeState::FailedDirty)
        })
    }

    pub(crate) fn validate_reload(
        &self,
        affected_plugins: Option<&BTreeSet<PluginId>>,
    ) -> RuntimeResult<()> {
        for (plugin_id, scope_id) in &self.plugin_scopes {
            if affected_plugins.is_some_and(|affected| !affected.contains(plugin_id)) {
                continue;
            }
            let Some(scope) = self.manager.snapshot(scope_id) else {
                continue;
            };
            if scope.state == crate::ScopeState::FailedDirty {
                continue;
            }
            if scope.lifetime != PluginLifetime::DrainRequired {
                return Err(crate::error::host_failure(
                    "host.scope.reload_requires_restart",
                    plugin_id.as_str(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn validate_reload_domain(
        &self,
        affected_plugins: &BTreeSet<PluginId>,
    ) -> RuntimeResult<()> {
        let snapshots = self.manager.snapshots();
        let providers = snapshots
            .iter()
            .filter(|scope| self.plugin_scopes.contains_key(scope.plugin_id.as_str()))
            .flat_map(|scope| {
                scope
                    .provided_services
                    .iter()
                    .map(move |service_id| (service_id.as_str(), scope.plugin_id.as_str()))
            })
            .collect::<BTreeMap<_, _>>();

        for consumer in snapshots
            .iter()
            .filter(|scope| self.plugin_scopes.contains_key(scope.plugin_id.as_str()))
        {
            for dependency in &consumer.dependencies {
                let Some(provider) = providers.get(dependency.service_id.as_str()) else {
                    continue;
                };
                if affected_plugins.contains(consumer.plugin_id.as_str())
                    != affected_plugins.contains(*provider)
                {
                    return Err(crate::error::host_failure(
                        "host.scope.reload_domain_incomplete",
                        format!(
                            "{}:{}:{}",
                            consumer.plugin_id, dependency.service_id, provider
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct RuntimeBootstrapper {
    manifests: Vec<PluginManifest>,
    runners: Vec<RegisteredRunner>,
    async_handlers: Vec<RegisteredAsyncHandler>,
    host_services: Vec<RegisteredHostService>,
    host_effects: Vec<RegisteredHostEffect>,
    shared_services: Option<Arc<HostServiceRegistry>>,
    resource_providers: Vec<RegisteredResourceProvider>,
    async_resource_providers: Vec<RegisteredAsyncResourceProvider>,
}

pub struct PreparedRuntimeReload {
    pub(crate) plan: RuntimeLoadPlan,
    pub(crate) runners: Vec<Box<dyn Runner>>,
    pub(crate) async_handlers: Vec<Arc<dyn AsyncBatchHandler>>,
    pub(crate) capabilities: HostCapabilityRegistry,
    pub(crate) services: Arc<HostServiceRegistry>,
    pub(crate) profile_id: String,
    pub(crate) registry_generation: u64,
    pub(crate) runner_limits: Option<BTreeMap<mutsuki_runtime_contracts::RunnerId, RunnerLimits>>,
    pub(crate) affected_plugins: Option<BTreeSet<PluginId>>,
    pub(crate) scopes: Option<PluginScopeSet>,
}

pub struct PreparedHostRuntime {
    prepared: PreparedRuntime,
    config: HostRuntimeConfig,
}

impl PreparedHostRuntime {
    #[must_use]
    pub fn load_plan(&self) -> &RuntimeLoadPlan {
        &self.prepared.plan
    }

    pub fn start(self) -> RuntimeResult<HostRuntime> {
        let booted = boot_prepared_runtime(self.prepared)?;
        let config = configure_resource_provider(
            self.config,
            &booted.active_resource_providers,
            booted.resource_providers,
            booted.async_resource_providers,
        )?;
        HostRuntime::start(
            booted.core,
            config,
            booted.capabilities,
            booted.services,
            booted.profile_id,
            booted.registry_generation,
            booted.scopes,
        )
    }
}

impl PreparedRuntimeReload {
    #[must_use]
    pub fn load_plan(&self) -> &RuntimeLoadPlan {
        &self.plan
    }

    pub(crate) fn append_core_kernel(&mut self) {
        append_core_kernel(&mut self.plan, &mut self.runners);
    }
}

struct RegisteredRunner {
    deployment_kind: PluginDeploymentKind,
    runner: Box<dyn Runner>,
}

struct RegisteredAsyncHandler {
    deployment_kind: PluginDeploymentKind,
    handler: Arc<dyn AsyncBatchHandler>,
}

#[derive(Clone)]
struct RegisteredHostService {
    owner_plugin_id: PluginId,
    service: RuntimeBootstrapperService,
}

struct RegisteredHostEffect {
    owner_plugin_id: PluginId,
    effect: RuntimeBootstrapperEffect,
}

struct RegisteredResourceProvider {
    provider_id: String,
    provider: Arc<dyn ResourceProviderGateway>,
}

impl RuntimeBootstrapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_manifest(&mut self, manifest: PluginManifest) {
        self.manifests.push(manifest);
    }

    pub fn use_shared_services(&mut self, services: Arc<HostServiceRegistry>) -> RuntimeResult<()> {
        if self.shared_services.is_some() || !self.host_services.is_empty() {
            return Err(crate::error::host_failure(
                "host.services.duplicate_registry",
                "runtime bootstrapper already owns host services",
            ));
        }
        self.shared_services = Some(services);
        Ok(())
    }

    pub fn register_loaded_plugin(&mut self, plugin: LoadedPlugin) {
        let LoadedPlugin {
            manifest,
            runners,
            async_handlers,
            host_services,
            host_effects,
            resource_providers,
            async_resource_providers,
        } = plugin;
        let owner_plugin_id = manifest.plugin_id.clone();
        let deployment_kind =
            PluginDeploymentKind::default_for_artifact(&manifest.artifact.artifact_type);
        self.register_manifest(manifest);
        for runner in runners {
            self.register_external_runner(deployment_kind.clone(), runner);
        }
        for handler in async_handlers {
            self.register_external_async_handler(deployment_kind.clone(), handler);
        }
        self.host_services.extend(
            host_services
                .into_iter()
                .map(|service| RegisteredHostService {
                    owner_plugin_id: owner_plugin_id.clone(),
                    service,
                }),
        );
        self.host_effects
            .extend(host_effects.into_iter().map(|effect| RegisteredHostEffect {
                owner_plugin_id: owner_plugin_id.clone(),
                effect,
            }));
        for resource_provider in resource_providers {
            self.resource_providers.push(RegisteredResourceProvider {
                provider_id: resource_provider.provider_id,
                provider: resource_provider.provider,
            });
        }
        for resource_provider in async_resource_providers {
            self.async_resource_providers
                .push(RegisteredAsyncResourceProvider {
                    provider_id: resource_provider.provider_id,
                    provider: resource_provider.provider,
                });
        }
    }

    pub fn load_plugins(&mut self, loader: &mut dyn PluginLoader) -> RuntimeResult<()> {
        for plugin in loader.load_plugins()? {
            self.register_loaded_plugin(plugin);
        }
        Ok(())
    }

    pub fn register_runner(&mut self, runner: Box<dyn Runner>) {
        self.register_builtin_runner(runner);
    }

    pub fn register_builtin_runner(&mut self, runner: Box<dyn Runner>) {
        self.register_external_runner(PluginDeploymentKind::Builtin, runner);
    }

    pub fn register_abi_runner(&mut self, runner: Box<dyn Runner>) {
        self.register_external_runner(PluginDeploymentKind::Abi, runner);
    }

    pub fn register_async_handler(&mut self, handler: Arc<dyn AsyncBatchHandler>) {
        self.register_external_async_handler(PluginDeploymentKind::Builtin, handler);
    }

    pub fn register_external_async_handler(
        &mut self,
        deployment_kind: PluginDeploymentKind,
        handler: Arc<dyn AsyncBatchHandler>,
    ) {
        self.async_handlers.push(RegisteredAsyncHandler {
            deployment_kind,
            handler,
        });
    }

    pub fn register_external_runner(
        &mut self,
        deployment_kind: PluginDeploymentKind,
        runner: Box<dyn Runner>,
    ) {
        self.runners.push(RegisteredRunner {
            deployment_kind,
            runner,
        });
    }

    pub fn register_resource_provider(
        &mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn ResourceProviderGateway>,
    ) {
        self.resource_providers.push(RegisteredResourceProvider {
            provider_id: provider_id.into(),
            provider,
        });
    }

    pub fn register_async_resource_provider(
        &mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn AsyncResourceProviderGateway>,
    ) {
        self.async_resource_providers
            .push(RegisteredAsyncResourceProvider {
                provider_id: provider_id.into(),
                provider,
            });
    }

    pub fn into_runtime(self, profile: RuntimeProfile) -> RuntimeResult<CoreRuntime> {
        self.boot_core_runtime(profile)
    }

    pub fn into_host_runtime(self, profile: RuntimeProfile) -> RuntimeResult<HostRuntime> {
        self.into_host_runtime_with_config(profile, HostRuntimeConfig::default())
    }

    pub fn into_host_runtime_with_config(
        self,
        profile: RuntimeProfile,
        config: HostRuntimeConfig,
    ) -> RuntimeResult<HostRuntime> {
        self.prepare_host_runtime_with_config(profile, config)?
            .start()
    }

    pub fn prepare_host_runtime_with_config(
        self,
        profile: RuntimeProfile,
        config: HostRuntimeConfig,
    ) -> RuntimeResult<PreparedHostRuntime> {
        let prepared = self.prepare_runtime(profile)?;
        validate_configured_scheduler_policy(
            &prepared.capabilities,
            config.scheduler_policy.as_ref(),
        )?;
        Ok(PreparedHostRuntime { prepared, config })
    }

    pub fn prepare_reload(
        self,
        profile: RuntimeProfile,
        registry_generation: u64,
    ) -> RuntimeResult<PreparedRuntimeReload> {
        self.prepare_reload_with_limits(profile, registry_generation, None, None)
    }

    /// Prepares a reload and atomically replaces Host scheduler limits with the supplied map.
    ///
    /// The optional form above intentionally preserves the existing limits for callers that do
    /// not participate in ServiceHost's dynamic product configuration boundary.
    pub fn prepare_reload_with_runner_limits(
        self,
        profile: RuntimeProfile,
        registry_generation: u64,
        runner_limits: BTreeMap<mutsuki_runtime_contracts::RunnerId, RunnerLimits>,
    ) -> RuntimeResult<PreparedRuntimeReload> {
        self.prepare_reload_with_limits(profile, registry_generation, Some(runner_limits), None)
    }

    pub fn prepare_targeted_reload_with_runner_limits(
        self,
        profile: RuntimeProfile,
        registry_generation: u64,
        runner_limits: BTreeMap<mutsuki_runtime_contracts::RunnerId, RunnerLimits>,
        affected_plugins: BTreeSet<String>,
    ) -> RuntimeResult<PreparedRuntimeReload> {
        self.prepare_reload_with_limits(
            profile,
            registry_generation,
            Some(runner_limits),
            Some(affected_plugins),
        )
    }

    fn prepare_reload_with_limits(
        self,
        profile: RuntimeProfile,
        registry_generation: u64,
        runner_limits: Option<BTreeMap<mutsuki_runtime_contracts::RunnerId, RunnerLimits>>,
        affected_plugins: Option<BTreeSet<String>>,
    ) -> RuntimeResult<PreparedRuntimeReload> {
        let mut prepared = self.prepare_runtime(profile)?;
        prepared.plan.registry_generation = registry_generation;
        for manifest in &mut prepared.plan.plugins {
            for runner in &mut manifest.provides.runners {
                runner.plugin_generation = registry_generation;
            }
        }
        prepared.runners = prepared
            .runners
            .into_iter()
            .map(|runner| {
                Box::new(GenerationRunner::new(runner, registry_generation)) as Box<dyn Runner>
            })
            .collect();
        prepared.async_handlers = prepared
            .async_handlers
            .into_iter()
            .map(|handler| {
                Arc::new(GenerationAsyncHandler::new(handler, registry_generation))
                    as Arc<dyn AsyncBatchHandler>
            })
            .collect();
        prepared.registry_generation = registry_generation;
        let affected_plugins = affected_plugins.map(|requested| {
            expand_service_reload_domain(
                &prepared.plan,
                &prepared.services,
                requested.into_iter().map(PluginId::from).collect(),
            )
        });
        let scopes = build_plugin_scopes(
            &prepared.plan,
            &prepared.services,
            prepared.host_effects,
            affected_plugins.as_ref(),
        )?;
        Ok(PreparedRuntimeReload {
            plan: prepared.plan,
            runners: prepared.runners,
            async_handlers: prepared.async_handlers,
            capabilities: prepared.capabilities,
            services: prepared.services,
            profile_id: prepared.profile_id,
            registry_generation: prepared.registry_generation,
            runner_limits,
            affected_plugins,
            scopes: Some(scopes),
        })
    }

    fn boot_core_runtime(self, profile: RuntimeProfile) -> RuntimeResult<CoreRuntime> {
        self.boot_host_runtime(profile).map(|booted| booted.core)
    }

    fn boot_host_runtime(self, profile: RuntimeProfile) -> RuntimeResult<BootedRuntime> {
        boot_prepared_runtime(self.prepare_runtime(profile)?)
    }

    fn prepare_runtime(self, profile: RuntimeProfile) -> RuntimeResult<PreparedRuntime> {
        let plan = resolve_load_plan(&self.manifests, &profile)?;
        let profile_id = plan.profile_id.clone();
        let registry_generation = plan.registry_generation;
        let active_resource_providers = plan.capability_graph.active_resource_providers.clone();
        let capabilities = HostCapabilityRegistry::from_load_plan(&plan)?;
        validate_host_startup_capabilities(&plan, &capabilities)?;
        validate_registered_runners(&plan, &self.runners, &self.async_handlers)?;
        validate_registered_resource_providers(&self.resource_providers)?;
        validate_registered_async_resource_providers(
            &self.resource_providers,
            &self.async_resource_providers,
        )?;
        validate_registered_host_services(&plan, &self.host_services)?;
        let services = match self.shared_services {
            Some(services) if self.host_services.is_empty() => services,
            Some(_) => {
                return Err(crate::error::host_failure(
                    "host.services.mixed_registry",
                    "shared and domain-local host services cannot be mixed",
                ));
            }
            None => build_host_service_registry(&self.host_services)?,
        };
        let mut host_effects = self.host_effects;
        let runners: Vec<Box<dyn Runner>> = self
            .runners
            .into_iter()
            .map(|registered| {
                if let Some(management) = registered.runner.management_handle() {
                    host_effects.push(RegisteredHostEffect {
                        owner_plugin_id: registered.runner.descriptor().plugin_id.clone(),
                        effect: RuntimeBootstrapperEffect {
                            kind: HostEffectKind::BackendInstance,
                            effect: Box::new(ManagementHandleEffect(management.clone())),
                        },
                    });
                    Box::new(ScopeOwnedRunner::new(registered.runner, management))
                        as Box<dyn Runner>
                } else {
                    registered.runner
                }
            })
            .collect();
        let async_handlers = self
            .async_handlers
            .into_iter()
            .map(|registered| {
                if let Some(management) = registered.handler.management_handle() {
                    host_effects.push(RegisteredHostEffect {
                        owner_plugin_id: registered.handler.descriptor().plugin_id.clone(),
                        effect: RuntimeBootstrapperEffect {
                            kind: HostEffectKind::BackendInstance,
                            effect: Box::new(ManagementHandleEffect(management.clone())),
                        },
                    });
                    Arc::new(ScopeOwnedAsyncHandler::new(registered.handler, management))
                        as Arc<dyn AsyncBatchHandler>
                } else {
                    registered.handler
                }
            })
            .collect();
        Ok(PreparedRuntime {
            plan,
            runners,
            async_handlers,
            capabilities,
            services,
            host_effects,
            profile_id,
            registry_generation,
            active_resource_providers,
            resource_providers: self.resource_providers,
            async_resource_providers: self.async_resource_providers,
        })
    }
}

fn boot_prepared_runtime(mut prepared: PreparedRuntime) -> RuntimeResult<BootedRuntime> {
    let scopes = build_plugin_scopes(
        &prepared.plan,
        &prepared.services,
        prepared.host_effects,
        None,
    )?;
    append_core_kernel(&mut prepared.plan, &mut prepared.runners);
    let core = match CoreRuntime::boot_with_async_handlers(
        prepared.plan,
        prepared.runners,
        prepared.async_handlers,
    ) {
        Ok(core) => core,
        Err(error) => {
            if let Err(cleanup_error) = scopes
                .manager()
                .rollback_activation_blocking(scopes.root_scope(), Duration::from_secs(30))
            {
                return Err(crate::error::host_failure(
                    "host.scope.boot_rollback_failed",
                    format!("boot failed: {error}; scope rollback failed: {cleanup_error}"),
                ));
            }
            return Err(error);
        }
    };
    if let Err(error) = scopes.commit_activation() {
        if let Err(cleanup_error) = scopes
            .manager()
            .rollback_activation_blocking(scopes.root_scope(), Duration::from_secs(30))
        {
            return Err(crate::error::host_failure(
                "host.scope.boot_activation_rollback_failed",
                format!("activation failed: {error}; scope rollback failed: {cleanup_error}"),
            ));
        }
        return Err(error);
    }
    Ok(BootedRuntime {
        core,
        capabilities: prepared.capabilities,
        services: prepared.services,
        scopes,
        profile_id: prepared.profile_id,
        registry_generation: prepared.registry_generation,
        active_resource_providers: prepared.active_resource_providers,
        resource_providers: prepared.resource_providers,
        async_resource_providers: prepared.async_resource_providers,
    })
}

fn validate_registered_host_services(
    plan: &RuntimeLoadPlan,
    services: &[RegisteredHostService],
) -> RuntimeResult<()> {
    for registered in services {
        let Some(manifest) = plan
            .plugins
            .iter()
            .find(|manifest| manifest.plugin_id == registered.owner_plugin_id)
        else {
            return Err(crate::error::host_failure(
                "host.services.owner_not_enabled",
                registered.owner_plugin_id.as_str(),
            ));
        };
        let service_id = registered.service.service_id.trim();
        if service_id.is_empty() || !manifest.provides.services.iter().any(|id| id == service_id) {
            return Err(crate::error::host_failure(
                "host.services.undeclared_service",
                service_id,
            ));
        }
        let capability = registered.service.capability.trim();
        if capability.is_empty()
            || !manifest
                .provides
                .capabilities
                .iter()
                .any(|provided| provided == capability)
        {
            return Err(crate::error::host_failure(
                "host.services.undeclared_capability",
                capability,
            ));
        }
    }
    Ok(())
}

struct BootedRuntime {
    core: CoreRuntime,
    capabilities: HostCapabilityRegistry,
    services: Arc<HostServiceRegistry>,
    scopes: PluginScopeSet,
    profile_id: String,
    registry_generation: u64,
    active_resource_providers: Vec<String>,
    resource_providers: Vec<RegisteredResourceProvider>,
    async_resource_providers: Vec<RegisteredAsyncResourceProvider>,
}

struct PreparedRuntime {
    plan: RuntimeLoadPlan,
    runners: Vec<Box<dyn Runner>>,
    async_handlers: Vec<Arc<dyn AsyncBatchHandler>>,
    capabilities: HostCapabilityRegistry,
    services: Arc<HostServiceRegistry>,
    host_effects: Vec<RegisteredHostEffect>,
    profile_id: String,
    registry_generation: u64,
    active_resource_providers: Vec<String>,
    resource_providers: Vec<RegisteredResourceProvider>,
    async_resource_providers: Vec<RegisteredAsyncResourceProvider>,
}

fn build_host_service_registry(
    host_services: &[RegisteredHostService],
) -> RuntimeResult<Arc<HostServiceRegistry>> {
    let registry = Arc::new(HostServiceRegistry::new());
    for registered in host_services {
        registry.register_bootstrapper_service(
            registered.owner_plugin_id.as_str(),
            &registered.service,
        )?;
    }
    registry.freeze();
    Ok(registry)
}

fn build_plugin_scopes(
    plan: &RuntimeLoadPlan,
    host_services: &HostServiceRegistry,
    host_effects: Vec<RegisteredHostEffect>,
    affected_plugins: Option<&BTreeSet<PluginId>>,
) -> RuntimeResult<PluginScopeSet> {
    let manager = PluginScopeManager::new();
    let root_scope = manager.create_scope(
        None,
        "host.composition.root",
        plan.registry_generation,
        PluginLifetime::DrainRequired,
    )?;
    let mut plugin_scopes = BTreeMap::new();
    for manifest in &plan.plugins {
        if affected_plugins.is_some_and(|affected| !affected.contains(&manifest.plugin_id)) {
            continue;
        }
        let scope = manager.create_scope(
            Some(&root_scope),
            manifest.plugin_id.clone(),
            plan.registry_generation,
            plugin_lifetime(&manifest.lifecycle),
        )?;
        plugin_scopes.insert(manifest.plugin_id.clone(), scope);
    }
    let discarded_scope = manager.create_scope(
        Some(&root_scope),
        "host.composition.discarded-candidate",
        plan.registry_generation,
        PluginLifetime::DrainRequired,
    )?;

    let activation = (|| -> RuntimeResult<Vec<ScopeId>> {
        for registered in host_effects {
            let owner_scope = plugin_scopes
                .get(&registered.owner_plugin_id)
                .unwrap_or(&discarded_scope);
            let kind = match registered.effect.kind {
                HostEffectKind::HostLocal => crate::scope::EffectKind::HostLocal,
                HostEffectKind::BackendInstance => crate::scope::EffectKind::BackendInstance,
            };
            manager.stage_effect(owner_scope, kind, Box::new(registered.effect))?;
        }

        for manifest in &plan.plugins {
            let Some(scope) = plugin_scopes.get(&manifest.plugin_id) else {
                continue;
            };
            for requirement in &manifest.requires {
                if requirement.kind == ContractSurfaceKind::Service {
                    manager.declare_dependency(
                        scope,
                        ServiceDependency {
                            service_id: requirement.surface_id.to_string(),
                            requirement: match requirement.requirement {
                                RequirementKind::Required => {
                                    crate::scope::ServiceRequirement::Required
                                }
                                RequirementKind::Optional => {
                                    crate::scope::ServiceRequirement::Optional
                                }
                            },
                            binding: match requirement.binding {
                                RequirementBinding::Static => {
                                    crate::scope::ServiceBinding::StaticAtActivation
                                }
                                RequirementBinding::Rebindable => {
                                    crate::scope::ServiceBinding::Rebindable
                                }
                            },
                        },
                    )?;
                }
            }
        }

        for (service_id, owner_plugin_id, service, rebindable) in host_services.owned_entries() {
            if affected_plugins.is_some_and(|affected| {
                !owner_plugin_id.is_empty() && !affected.contains(owner_plugin_id.as_str())
            }) {
                continue;
            }
            let owner_scope = if owner_plugin_id.is_empty() {
                &root_scope
            } else {
                plugin_scopes.get(owner_plugin_id.as_str()).ok_or_else(|| {
                    crate::error::host_failure(
                        "host.scope.service_owner_missing",
                        format!("{service_id}:{owner_plugin_id}"),
                    )
                })?
            };
            manager.stage_published_service_value(
                owner_scope,
                &root_scope,
                &service_id,
                service,
                rebindable,
            )?;
        }

        manager.begin_activation(&root_scope)?;
        let mut activation_order = Vec::new();
        for plugin_id in &plan.load_order {
            if let Some(scope) = plugin_scopes.get(plugin_id) {
                manager.begin_activation(scope)?;
                activation_order.push(scope.clone());
            }
        }
        manager.rollback_activation_blocking(&discarded_scope, Duration::from_secs(30))?;
        Ok(activation_order)
    })();
    let activation_order = match activation {
        Ok(activation_order) => activation_order,
        Err(error) => {
            if let Err(cleanup_error) =
                manager.rollback_activation_blocking(&root_scope, Duration::from_secs(30))
            {
                return Err(crate::error::host_failure(
                    "host.scope.activation_rollback_failed",
                    format!("activation failed: {error}; scope rollback failed: {cleanup_error}"),
                ));
            }
            return Err(error);
        }
    };

    Ok(PluginScopeSet {
        manager,
        root_scope,
        plugin_scopes,
        activation_order,
    })
}

fn expand_service_reload_domain(
    plan: &RuntimeLoadPlan,
    host_services: &HostServiceRegistry,
    mut affected: BTreeSet<PluginId>,
) -> BTreeSet<PluginId> {
    let owners = host_services
        .owned_entries()
        .into_iter()
        .filter_map(|(service_id, owner_plugin_id, _, _)| {
            (!owner_plugin_id.is_empty()).then_some((service_id, PluginId::from(owner_plugin_id)))
        })
        .collect::<BTreeMap<_, _>>();

    loop {
        let mut changed = false;
        for manifest in &plan.plugins {
            for requirement in &manifest.requires {
                if requirement.kind != ContractSurfaceKind::Service {
                    continue;
                }
                let Some(provider) = owners.get(requirement.surface_id.as_str()) else {
                    continue;
                };
                if affected.contains(&manifest.plugin_id) || affected.contains(provider) {
                    changed |= affected.insert(manifest.plugin_id.clone());
                    changed |= affected.insert(provider.clone());
                }
            }
        }
        if !changed {
            return affected;
        }
    }
}

struct ScopeOwnedRunner {
    inner: Box<dyn Runner>,
    management: Arc<dyn RunnerManagementHandle>,
}

impl ScopeOwnedRunner {
    fn new(inner: Box<dyn Runner>, management: Arc<dyn RunnerManagementHandle>) -> Self {
        Self { inner, management }
    }
}

impl Runner for ScopeOwnedRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        self.inner.descriptor()
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.inner.run_batch(ctx, batch)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.inner.cancel(invocation_id)
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        Ok(())
    }

    fn isolation(&self) -> RunnerIsolation {
        self.inner.isolation()
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        Some(Arc::new(ScopeOwnedManagementHandle(
            self.management.clone(),
        )))
    }

    fn recover_after_hard_termination(&mut self) -> RuntimeResult<()> {
        self.inner.recover_after_hard_termination()
    }
}

#[derive(Debug)]
struct ScopeOwnedManagementHandle(Arc<dyn RunnerManagementHandle>);

impl RunnerManagementHandle for ScopeOwnedManagementHandle {
    fn cancel(&self, invocation_id: &str) -> RuntimeResult<()> {
        self.0.cancel(invocation_id)
    }

    fn dispose(&self) -> RuntimeResult<()> {
        Ok(())
    }
}

struct ScopeOwnedAsyncHandler {
    inner: Arc<dyn AsyncBatchHandler>,
    management: Arc<dyn RunnerManagementHandle>,
}

impl ScopeOwnedAsyncHandler {
    fn new(inner: Arc<dyn AsyncBatchHandler>, management: Arc<dyn RunnerManagementHandle>) -> Self {
        Self { inner, management }
    }
}

impl AsyncBatchHandler for ScopeOwnedAsyncHandler {
    fn descriptor(&self) -> &RunnerDescriptor {
        self.inner.descriptor()
    }

    fn run_batch(&self, ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        self.inner.run_batch(ctx, batch)
    }

    fn isolation(&self) -> RunnerIsolation {
        self.inner.isolation()
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        Some(Arc::new(ScopeOwnedManagementHandle(
            self.management.clone(),
        )))
    }
}

struct ManagementHandleEffect(Arc<dyn RunnerManagementHandle>);

impl HostEffect for ManagementHandleEffect {
    fn dispose(&mut self) -> mutsuki_runtime_sdk::HostEffectFuture<'_> {
        let management = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || management.dispose())
                .await
                .map_err(|error| {
                    crate::error::host_failure("host.scope.management_join", error.to_string())
                })?
        })
    }
}

fn plugin_lifetime(lifecycle: &mutsuki_runtime_contracts::LifecyclePolicy) -> PluginLifetime {
    if !lifecycle.supports_dispose {
        return PluginLifetime::RestartRequired;
    }
    match lifecycle.reload_policy.as_str() {
        "required_builtin" => PluginLifetime::RequiredBuiltin,
        "application_lifetime" => PluginLifetime::Application,
        "restart_required" | "static" => PluginLifetime::RestartRequired,
        _ => PluginLifetime::DrainRequired,
    }
}

struct GenerationRunner {
    descriptor: RunnerDescriptor,
    inner: Box<dyn Runner>,
}

impl GenerationRunner {
    fn new(inner: Box<dyn Runner>, generation: u64) -> Self {
        let mut descriptor = inner.descriptor().clone();
        descriptor.plugin_generation = generation;
        Self { descriptor, inner }
    }
}

impl Runner for GenerationRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.inner.run_batch(ctx, batch)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.inner.cancel(invocation_id)
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.inner.dispose()
    }

    fn isolation(&self) -> mutsuki_runtime_core::RunnerIsolation {
        self.inner.isolation()
    }

    fn management_handle(&self) -> Option<Arc<dyn mutsuki_runtime_core::RunnerManagementHandle>> {
        self.inner.management_handle()
    }

    fn recover_after_hard_termination(&mut self) -> RuntimeResult<()> {
        self.inner.recover_after_hard_termination()
    }
}

struct RegisteredAsyncResourceProvider {
    provider_id: String,
    provider: Arc<dyn AsyncResourceProviderGateway>,
}

struct GenerationAsyncHandler {
    descriptor: RunnerDescriptor,
    inner: Arc<dyn AsyncBatchHandler>,
}

impl GenerationAsyncHandler {
    fn new(inner: Arc<dyn AsyncBatchHandler>, generation: u64) -> Self {
        let mut descriptor = inner.descriptor().clone();
        descriptor.plugin_generation = generation;
        Self { descriptor, inner }
    }
}

impl AsyncBatchHandler for GenerationAsyncHandler {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        self.inner.run_batch(ctx, batch)
    }

    fn isolation(&self) -> mutsuki_runtime_core::RunnerIsolation {
        self.inner.isolation()
    }

    fn management_handle(&self) -> Option<Arc<dyn mutsuki_runtime_core::RunnerManagementHandle>> {
        self.inner.management_handle()
    }
}

fn append_core_kernel(plan: &mut RuntimeLoadPlan, runners: &mut Vec<Box<dyn Runner>>) {
    let core_runner = CoreKernelRunner::new(plan.registry_generation);
    plan.plugins
        .push(core_manifest(core_runner.descriptor().clone()));
    plan.contract_surfaces.push(ContractSurface {
        surface_id: "runner:core.kernel".into(),
        kind: ContractSurfaceKind::Runner,
        owner_plugin_id: "core".into(),
        fingerprint: "sha256:core.kernel".into(),
        deprecated: false,
    });
    runners.push(Box::new(core_runner));
}

fn validate_registered_runners(
    plan: &RuntimeLoadPlan,
    runners: &[RegisteredRunner],
    async_handlers: &[RegisteredAsyncHandler],
) -> RuntimeResult<()> {
    let mut registered_runner_ids = BTreeSet::new();
    for registered_runner in runners {
        let descriptor = registered_runner.runner.descriptor();
        validate_runner_deployment(plan, registered_runner, descriptor)?;
        registered_runner_ids.insert(descriptor.runner_id.clone());
    }
    for registered_handler in async_handlers {
        let descriptor = registered_handler.handler.descriptor();
        validate_runner_deployment_kind(plan, &registered_handler.deployment_kind, descriptor)?;
        registered_runner_ids.insert(descriptor.runner_id.clone());
    }
    for manifest in &plan.plugins {
        for runner in &manifest.provides.runners {
            if !registered_runner_ids.contains(&runner.runner_id) {
                return Err(runner_missing_for_deployment(
                    manifest.plugin_id.as_str(),
                    runner.runner_id.as_str(),
                    plan.plugin_deployments
                        .get(&manifest.plugin_id)
                        .expect("enabled plugin has deployment"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_host_startup_capabilities(
    plan: &RuntimeLoadPlan,
    capabilities: &HostCapabilityRegistry,
) -> RuntimeResult<()> {
    let mut runner_deployments = Vec::new();
    for manifest in plan
        .plugins
        .iter()
        .filter(|manifest| !manifest.provides.runners.is_empty())
    {
        let deployment = plan
            .plugin_deployments
            .get(&manifest.plugin_id)
            .expect("enabled plugin has deployment");
        if !runner_deployments.contains(&deployment) {
            runner_deployments.push(deployment);
        }
    }
    for deployment in runner_deployments {
        ensure_active_backend_for_deployment(plan, capabilities, deployment)?;
    }
    Ok(())
}

fn ensure_active_backend_for_deployment(
    plan: &RuntimeLoadPlan,
    capabilities: &HostCapabilityRegistry,
    deployment: &PluginDeploymentKind,
) -> RuntimeResult<()> {
    if capabilities
        .active_plugin_backend_for_deployment(deployment)
        .is_some()
    {
        return Ok(());
    }
    if let Some(backend_id) = declared_backend_for_deployment(plan, deployment) {
        return Err(capability_pruned(&format!("plugin_backend:{backend_id}")));
    }
    Err(capability_provider_missing(&format!(
        "plugin_backend:{deployment:?}"
    )))
}

fn declared_backend_for_deployment(
    plan: &RuntimeLoadPlan,
    deployment: &PluginDeploymentKind,
) -> Option<String> {
    plan.plugins
        .iter()
        .flat_map(|manifest| manifest.provides.plugin_backends.iter())
        .filter(|backend| &backend.deployment_kind == deployment)
        .map(|backend| backend.backend_id.clone())
        .min()
}

fn validate_registered_resource_providers(
    resource_providers: &[RegisteredResourceProvider],
) -> RuntimeResult<()> {
    let mut provider_ids = BTreeSet::new();
    for provider in resource_providers {
        if !provider_ids.insert(provider.provider_id.clone()) {
            return Err(resource_provider_duplicate(&provider.provider_id));
        }
    }
    Ok(())
}

fn validate_registered_async_resource_providers(
    resource_providers: &[RegisteredResourceProvider],
    async_resource_providers: &[RegisteredAsyncResourceProvider],
) -> RuntimeResult<()> {
    let mut provider_ids: BTreeSet<_> = resource_providers
        .iter()
        .map(|provider| provider.provider_id.clone())
        .collect();
    for provider in async_resource_providers {
        if !provider_ids.insert(provider.provider_id.clone()) {
            return Err(resource_provider_duplicate(&provider.provider_id));
        }
    }
    Ok(())
}

fn configure_resource_provider(
    mut config: HostRuntimeConfig,
    active_provider_ids: &[String],
    resource_providers: Vec<RegisteredResourceProvider>,
    async_resource_providers: Vec<RegisteredAsyncResourceProvider>,
) -> RuntimeResult<HostRuntimeConfig> {
    for registered in resource_providers {
        config
            .resource_providers
            .entry(registered.provider_id)
            .or_insert(registered.provider);
    }
    for registered in async_resource_providers {
        config
            .async_resource_providers
            .entry(registered.provider_id)
            .or_insert(registered.provider);
    }

    for provider_id in active_provider_ids {
        if !config.resource_providers.contains_key(provider_id)
            && !config.async_resource_providers.contains_key(provider_id)
        {
            return Err(resource_provider_missing(provider_id));
        }
    }

    Ok(config)
}

fn validate_configured_scheduler_policy(
    capabilities: &HostCapabilityRegistry,
    policy: &dyn SchedulerPolicy,
) -> RuntimeResult<()> {
    let policy_id = policy.policy_id();
    let Some(active_policy_id) = capabilities.active_scheduler_policy_id() else {
        return if policy_id == DefaultScheduler::POLICY_ID {
            Ok(())
        } else {
            capabilities.require_scheduler_policy(policy_id).map(|_| ())
        };
    };

    if policy_id != DefaultScheduler::POLICY_ID {
        return capabilities.require_scheduler_policy(policy_id).map(|_| ());
    }

    Err(capability_provider_missing(&format!(
        "scheduler_policy:{active_policy_id}"
    )))
}

fn validate_runner_deployment(
    plan: &RuntimeLoadPlan,
    registered_runner: &RegisteredRunner,
    descriptor: &RunnerDescriptor,
) -> RuntimeResult<()> {
    validate_runner_deployment_kind(plan, &registered_runner.deployment_kind, descriptor)
}

fn validate_runner_deployment_kind(
    plan: &RuntimeLoadPlan,
    registered_deployment: &PluginDeploymentKind,
    descriptor: &RunnerDescriptor,
) -> RuntimeResult<()> {
    let Some(planned_deployment) = plan.plugin_deployments.get(&descriptor.plugin_id) else {
        return Err(runner_for_disabled_plugin(
            descriptor.plugin_id.as_str(),
            descriptor.runner_id.as_str(),
        ));
    };
    if planned_deployment == registered_deployment {
        return Ok(());
    }
    Err(deployment_mismatch(
        "host.plugin.runner_deployment_mismatch",
        descriptor.plugin_id.as_str(),
        registered_deployment,
        planned_deployment,
    ))
}
