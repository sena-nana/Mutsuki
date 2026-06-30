use std::collections::BTreeSet;

use mutsuki_runtime_contracts::{
    ContractSurface, ContractSurfaceKind, PluginDeploymentKind, PluginManifest, RunnerDescriptor,
    RunnerResult, RuntimeLoadPlan, RuntimeProfile, Task,
};
use mutsuki_runtime_core::{CoreKernelRunner, CoreRuntime, Runner, RunnerContext, RuntimeResult};

use crate::capabilities::HostCapabilityRegistry;
use crate::error::{
    deployment_mismatch, runner_for_disabled_plugin, runner_missing_for_deployment,
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
}

struct RegisteredRunner {
    deployment_kind: PluginDeploymentKind,
    runner: Box<dyn Runner>,
}

impl RuntimeBootstrapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_manifest(&mut self, manifest: PluginManifest) {
        self.manifests.push(manifest);
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
        HostRuntime::start(
            booted.core,
            config,
            booted.capabilities,
            booted.profile_id,
            booted.registry_generation,
        )
    }

    fn boot_core_runtime(self, profile: RuntimeProfile) -> RuntimeResult<CoreRuntime> {
        self.boot_host_runtime(profile).map(|booted| booted.core)
    }

    fn boot_host_runtime(self, profile: RuntimeProfile) -> RuntimeResult<BootedRuntime> {
        let mut plan = resolve_load_plan(&self.manifests, &profile)?;
        let profile_id = plan.profile_id.clone();
        let registry_generation = plan.registry_generation;
        let capabilities = HostCapabilityRegistry::from_load_plan(&plan)?;
        validate_registered_runners(&plan, &self.runners)?;
        let mut runners: Vec<Box<dyn Runner>> = self
            .runners
            .into_iter()
            .map(|registered| registered.runner)
            .collect();
        append_core_kernel(&mut plan, &mut runners);
        let core = CoreRuntime::boot(plan, runners)?;
        Ok(BootedRuntime {
            core,
            capabilities,
            profile_id,
            registry_generation,
        })
    }
}

struct BootedRuntime {
    core: CoreRuntime,
    capabilities: HostCapabilityRegistry,
    profile_id: String,
    registry_generation: u64,
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
