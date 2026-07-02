use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    ContractSurface, ContractSurfaceKind, PluginDeploymentKind, PluginManifest, RunnerDescriptor,
    RunnerResult, RuntimeLoadPlan, RuntimeProfile, Task,
};
use mutsuki_runtime_core::{CoreKernelRunner, CoreRuntime, Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{LoadedPlugin, ResourceProviderGateway};

use crate::capabilities::HostCapabilityRegistry;
use crate::error::{
    capability_provider_missing, capability_pruned, deployment_mismatch,
    resource_provider_duplicate, resource_provider_missing, resource_provider_unsupported,
    runner_for_disabled_plugin, runner_missing_for_deployment,
};
use crate::host::{HostRuntime, HostRuntimeConfig};
use crate::resolver::{core_manifest, resolve_load_plan};

pub type NativeStepHandler =
    Box<dyn FnMut(RunnerContext, Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> + Send>;

pub struct NativeRunner {
    descriptor: RunnerDescriptor,
    handler: NativeStepHandler,
    cancelled: Vec<String>,
    disposed: bool,
}

impl NativeRunner {
    pub fn new(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(RunnerContext, Vec<Task>) -> RuntimeResult<Vec<RunnerResult>>
        + Send
        + 'static,
    ) -> Self {
        Self {
            descriptor,
            handler: Box::new(handler),
            cancelled: Vec::new(),
            disposed: false,
        }
    }
}

impl Runner for NativeRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        (self.handler)(ctx, tasks)
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
    resource_providers: Vec<RegisteredResourceProvider>,
}

pub struct PreparedRuntimeReload {
    pub(crate) plan: RuntimeLoadPlan,
    pub(crate) runners: Vec<Box<dyn Runner>>,
    pub(crate) capabilities: HostCapabilityRegistry,
    pub(crate) profile_id: String,
    pub(crate) registry_generation: u64,
}

struct RegisteredRunner {
    deployment_kind: PluginDeploymentKind,
    runner: Box<dyn Runner>,
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
            host_services: _,
            resource_providers,
        } = plugin;
        self.register_manifest(manifest);
        for runner in runners {
            self.register_builtin_runner(runner);
        }
        for resource_provider in resource_providers {
            self.resource_providers.push(RegisteredResourceProvider {
                provider_id: resource_provider.provider_id,
                provider: resource_provider.provider,
            });
        }
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
        let booted = self.boot_host_runtime(profile)?;
        let config = configure_resource_provider(
            config,
            &booted.active_resource_providers,
            booted.resource_providers,
        )?;
        HostRuntime::start(
            booted.core,
            config,
            booted.capabilities,
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
        append_core_kernel(&mut prepared.plan, &mut prepared.runners);
        prepared.registry_generation = registry_generation;
        Ok(PreparedRuntimeReload {
            plan: prepared.plan,
            runners: prepared.runners,
            capabilities: prepared.capabilities,
            profile_id: prepared.profile_id,
            registry_generation: prepared.registry_generation,
        })
    }

    fn boot_core_runtime(self, profile: RuntimeProfile) -> RuntimeResult<CoreRuntime> {
        self.boot_host_runtime(profile).map(|booted| booted.core)
    }

    fn boot_host_runtime(self, profile: RuntimeProfile) -> RuntimeResult<BootedRuntime> {
        let mut prepared = self.prepare_runtime(profile)?;
        append_core_kernel(&mut prepared.plan, &mut prepared.runners);
        let core = CoreRuntime::boot(prepared.plan, prepared.runners)?;
        Ok(BootedRuntime {
            core,
            capabilities: prepared.capabilities,
            profile_id: prepared.profile_id,
            registry_generation: prepared.registry_generation,
            active_resource_providers: prepared.active_resource_providers,
            resource_providers: prepared.resource_providers,
        })
    }

    fn prepare_runtime(self, profile: RuntimeProfile) -> RuntimeResult<PreparedRuntime> {
        let plan = resolve_load_plan(&self.manifests, &profile)?;
        let profile_id = plan.profile_id.clone();
        let registry_generation = plan.registry_generation;
        let active_resource_providers = plan.capability_graph.active_resource_providers.clone();
        let capabilities = HostCapabilityRegistry::from_load_plan(&plan)?;
        validate_host_startup_capabilities(&plan, &capabilities)?;
        validate_registered_runners(&plan, &self.runners)?;
        validate_registered_resource_providers(&self.resource_providers)?;
        let runners: Vec<Box<dyn Runner>> = self
            .runners
            .into_iter()
            .map(|registered| registered.runner)
            .collect();
        Ok(PreparedRuntime {
            plan,
            runners,
            capabilities,
            profile_id,
            registry_generation,
            active_resource_providers,
            resource_providers: self.resource_providers,
        })
    }
}

struct BootedRuntime {
    core: CoreRuntime,
    capabilities: HostCapabilityRegistry,
    profile_id: String,
    registry_generation: u64,
    active_resource_providers: Vec<String>,
    resource_providers: Vec<RegisteredResourceProvider>,
}

struct PreparedRuntime {
    plan: RuntimeLoadPlan,
    runners: Vec<Box<dyn Runner>>,
    capabilities: HostCapabilityRegistry,
    profile_id: String,
    registry_generation: u64,
    active_resource_providers: Vec<String>,
    resource_providers: Vec<RegisteredResourceProvider>,
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

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        self.inner.step(ctx, tasks)
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.inner.cancel(invocation_id)
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.inner.dispose()
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
) -> RuntimeResult<()> {
    let mut registered_runner_ids = BTreeSet::new();
    for registered_runner in runners {
        let descriptor = registered_runner.runner.descriptor();
        validate_runner_deployment(plan, registered_runner, descriptor)?;
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

fn configure_resource_provider(
    mut config: HostRuntimeConfig,
    active_provider_ids: &[String],
    resource_providers: Vec<RegisteredResourceProvider>,
) -> RuntimeResult<HostRuntimeConfig> {
    if config.resource_provider.is_some() {
        return Ok(config);
    }

    match active_provider_ids {
        [] => Ok(config),
        [provider_id] => {
            let mut providers = BTreeMap::new();
            for registered in resource_providers {
                providers.insert(registered.provider_id, registered.provider);
            }
            let provider = providers
                .remove(provider_id)
                .ok_or_else(|| resource_provider_missing(provider_id))?;
            config.resource_provider = Some(provider);
            Ok(config)
        }
        providers => Err(resource_provider_unsupported(format!(
            "host config accepts one resource provider, load plan selected {}: {}",
            providers.len(),
            providers.join(",")
        ))),
    }
}

fn validate_runner_deployment(
    plan: &RuntimeLoadPlan,
    registered_runner: &RegisteredRunner,
    descriptor: &RunnerDescriptor,
) -> RuntimeResult<()> {
    let Some(planned_deployment) = plan.plugin_deployments.get(&descriptor.plugin_id) else {
        return Err(runner_for_disabled_plugin(
            &descriptor.plugin_id,
            &descriptor.runner_id,
        ));
    };
    if planned_deployment == &registered_runner.deployment_kind {
        return Ok(());
    }
    Err(deployment_mismatch(
        "host.plugin.runner_deployment_mismatch",
        &descriptor.plugin_id,
        &registered_runner.deployment_kind,
        planned_deployment,
    ))
}
