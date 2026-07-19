use std::collections::BTreeSet;
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    CompletionBatch, ContractSurface, ContractSurfaceKind, EntryCompletion, PluginDeploymentKind,
    PluginManifest, RunnerDescriptor, RunnerResult, RuntimeLoadPlan, RuntimeProfile, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{
    AsyncBatchHandler, AsyncCompletionFuture, CoreKernelRunner, CoreRuntime, Runner, RunnerContext,
    RuntimeResult,
};
use mutsuki_runtime_sdk::{
    AsyncResourceProviderGateway, HostServiceRegistry, LoadedPlugin, PluginLoader,
    ResourceProviderGateway, RuntimeBootstrapperService,
};

use crate::capabilities::HostCapabilityRegistry;
use crate::error::{
    capability_provider_missing, capability_pruned, deployment_mismatch,
    resource_provider_duplicate, resource_provider_missing, runner_for_disabled_plugin,
    runner_missing_for_deployment,
};
use crate::host::{HostRuntime, HostRuntimeConfig};
use crate::resolver::{core_manifest, resolve_load_plan};
use crate::scheduler::{DefaultScheduler, SchedulerPolicy};

pub type NativeEntryHandler =
    Box<dyn FnMut(RunnerContext, Task) -> RuntimeResult<RunnerResult> + Send>;
pub type BorrowedNativeEntryHandler =
    Box<dyn FnMut(&RunnerContext, &Task) -> RuntimeResult<RunnerResult> + Send>;

enum NativeHandler {
    Owned(NativeEntryHandler),
    Borrowed(BorrowedNativeEntryHandler),
}

pub struct NativeRunner {
    descriptor: RunnerDescriptor,
    handler: NativeHandler,
    cancelled: Vec<String>,
    disposed: bool,
}

impl NativeRunner {
    pub fn new(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(RunnerContext, Task) -> RuntimeResult<RunnerResult> + Send + 'static,
    ) -> Self {
        Self {
            descriptor,
            handler: NativeHandler::Owned(Box::new(handler)),
            cancelled: Vec::new(),
            disposed: false,
        }
    }

    /// Creates a builtin runner whose entry handler borrows typed local tasks.
    ///
    /// This is the allocation-free in-process path. Wire-backed payloads are
    /// still decoded to an owned temporary by `BatchPayload::task_at`.
    pub fn new_borrowed(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(&RunnerContext, &Task) -> RuntimeResult<RunnerResult> + Send + 'static,
    ) -> Self {
        Self {
            descriptor,
            handler: NativeHandler::Borrowed(Box::new(handler)),
            cancelled: Vec::new(),
            disposed: false,
        }
    }
}

impl Runner for NativeRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let mut results = Vec::with_capacity(batch.entries.len());
        for entry in &batch.entries {
            let task = match batch.payload_task(entry.payload_index) {
                Ok(task) if task.task_id == entry.task_id => task,
                Ok(_) => {
                    results.push(EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: None,
                        error: Some(mutsuki_runtime_contracts::RuntimeError::new(
                            mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                            "native_runner",
                            format!("batch.entry.{}.payload_task_id", entry.entry_id),
                        )),
                    });
                    continue;
                }
                Err(error) => {
                    results.push(EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: None,
                        error: Some(error),
                    });
                    continue;
                }
            };
            let result = match &mut self.handler {
                NativeHandler::Owned(handler) => handler(ctx.clone(), task.into_owned()),
                NativeHandler::Borrowed(handler) => handler(&ctx, task.as_ref()),
            };
            match result {
                Ok(result) => results.push(EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result: Some(result),
                    error: None,
                }),
                Err(failure) => results.push(EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result: None,
                    error: Some(failure.error().clone()),
                }),
            }
        }
        Ok(CompletionBatch::from_results(&batch, results))
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.cancelled.push(invocation_id.to_string());
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.disposed = true;
        Ok(())
    }
}

#[derive(Default)]
pub struct RuntimeBootstrapper {
    manifests: Vec<PluginManifest>,
    runners: Vec<RegisteredRunner>,
    async_handlers: Vec<RegisteredAsyncHandler>,
    host_services: Vec<RuntimeBootstrapperService>,
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
}

struct RegisteredRunner {
    deployment_kind: PluginDeploymentKind,
    runner: Box<dyn Runner>,
}

struct RegisteredAsyncHandler {
    deployment_kind: PluginDeploymentKind,
    handler: Arc<dyn AsyncBatchHandler>,
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

    pub fn register_loaded_plugin(&mut self, plugin: LoadedPlugin) {
        let LoadedPlugin {
            manifest,
            runners,
            async_handlers,
            host_services,
            resource_providers,
            async_resource_providers,
        } = plugin;
        let deployment_kind =
            PluginDeploymentKind::default_for_artifact(&manifest.artifact.artifact_type);
        self.register_manifest(manifest);
        for runner in runners {
            self.register_external_runner(deployment_kind.clone(), runner);
        }
        for handler in async_handlers {
            self.register_external_async_handler(deployment_kind.clone(), handler);
        }
        self.host_services.extend(host_services);
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
        let prepared = self.prepare_runtime(profile)?;
        validate_configured_scheduler_policy(
            &prepared.capabilities,
            config.scheduler_policy.as_ref(),
        )?;
        let booted = boot_prepared_runtime(prepared)?;
        let config = configure_resource_provider(
            config,
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
        )
    }

    pub fn prepare_reload(
        self,
        profile: RuntimeProfile,
        registry_generation: u64,
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
        append_core_kernel(&mut prepared.plan, &mut prepared.runners);
        prepared.registry_generation = registry_generation;
        Ok(PreparedRuntimeReload {
            plan: prepared.plan,
            runners: prepared.runners,
            async_handlers: prepared.async_handlers,
            capabilities: prepared.capabilities,
            services: prepared.services,
            profile_id: prepared.profile_id,
            registry_generation: prepared.registry_generation,
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
        let services = build_host_service_registry(self.host_services)?;
        let runners: Vec<Box<dyn Runner>> = self
            .runners
            .into_iter()
            .map(|registered| registered.runner)
            .collect();
        let async_handlers = self
            .async_handlers
            .into_iter()
            .map(|registered| registered.handler)
            .collect();
        Ok(PreparedRuntime {
            plan,
            runners,
            async_handlers,
            capabilities,
            services,
            profile_id,
            registry_generation,
            active_resource_providers,
            resource_providers: self.resource_providers,
            async_resource_providers: self.async_resource_providers,
        })
    }
}

fn boot_prepared_runtime(mut prepared: PreparedRuntime) -> RuntimeResult<BootedRuntime> {
    append_core_kernel(&mut prepared.plan, &mut prepared.runners);
    let core = CoreRuntime::boot_with_async_handlers(
        prepared.plan,
        prepared.runners,
        prepared.async_handlers,
    )?;
    Ok(BootedRuntime {
        core,
        capabilities: prepared.capabilities,
        services: prepared.services,
        profile_id: prepared.profile_id,
        registry_generation: prepared.registry_generation,
        active_resource_providers: prepared.active_resource_providers,
        resource_providers: prepared.resource_providers,
        async_resource_providers: prepared.async_resource_providers,
    })
}

struct BootedRuntime {
    core: CoreRuntime,
    capabilities: HostCapabilityRegistry,
    services: Arc<HostServiceRegistry>,
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
    profile_id: String,
    registry_generation: u64,
    active_resource_providers: Vec<String>,
    resource_providers: Vec<RegisteredResourceProvider>,
    async_resource_providers: Vec<RegisteredAsyncResourceProvider>,
}

fn build_host_service_registry(
    host_services: Vec<RuntimeBootstrapperService>,
) -> RuntimeResult<Arc<HostServiceRegistry>> {
    let registry = Arc::new(HostServiceRegistry::new());
    for service in host_services {
        let RuntimeBootstrapperService {
            service_id,
            service,
            ..
        } = service;
        registry.register_erased(service_id, service)?;
    }
    registry.freeze();
    Ok(registry)
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
                    &manifest.plugin_id,
                    &runner.runner_id,
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
            &descriptor.plugin_id,
            &descriptor.runner_id,
        ));
    };
    if planned_deployment == registered_deployment {
        return Ok(());
    }
    Err(deployment_mismatch(
        "host.plugin.runner_deployment_mismatch",
        &descriptor.plugin_id,
        registered_deployment,
        planned_deployment,
    ))
}
