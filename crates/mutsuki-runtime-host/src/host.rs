use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ArtifactType, ContractSurface, ContractSurfaceKind, LifecyclePolicy, PermissionGrant,
    PluginArtifact, PluginManifest, PluginProvides, RunnerDescriptor, RunnerResult,
    RuntimeLoadPlan, RuntimeProfile, Task,
};
use mutsuki_runtime_core::{CoreKernelRunner, CoreRuntime, Runner, RunnerContext, RuntimeResult};

pub type NativeStepHandler =
    Box<dyn FnMut(RunnerContext, Vec<Task>) -> RuntimeResult<Vec<RunnerResult>>>;

pub struct NativeRunner {
    descriptor: RunnerDescriptor,
    handler: NativeStepHandler,
    cancelled: Vec<String>,
    disposed: bool,
}

impl NativeRunner {
    pub fn new(
        descriptor: RunnerDescriptor,
        handler: impl FnMut(RunnerContext, Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> + 'static,
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
pub struct NativePluginHost {
    manifests: Vec<PluginManifest>,
    runners: Vec<Box<dyn Runner>>,
}

impl NativePluginHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_manifest(&mut self, manifest: PluginManifest) {
        self.manifests.push(manifest);
    }

    pub fn register_runner(&mut self, runner: Box<dyn Runner>) {
        self.runners.push(runner);
    }

    pub fn into_runtime(mut self, profile: RuntimeProfile) -> RuntimeResult<CoreRuntime> {
        let mut plan = resolve_load_plan(&self.manifests, &profile);
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
        self.runners.push(Box::new(core_runner));
        CoreRuntime::boot(plan, self.runners)
    }
}

pub fn resolve_load_plan(
    manifests: &[PluginManifest],
    profile: &RuntimeProfile,
) -> RuntimeLoadPlan {
    let enabled: Vec<PluginManifest> = manifests
        .iter()
        .filter(|manifest| profile.enabled_plugins.contains(&manifest.plugin_id))
        .cloned()
        .collect();
    let mut runner_bindings = profile.bindings.clone();
    for manifest in &enabled {
        for runner in &manifest.provides.runners {
            for kind in &runner.accepted_task_kinds {
                runner_bindings
                    .entry(kind.clone())
                    .or_insert_with(|| runner.runner_id.clone());
            }
        }
    }
    RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: profile.profile_id.clone(),
        profile_hash: format!(
            "profile:{}:{}",
            profile.profile_id,
            profile.enabled_plugins.len()
        ),
        registry_generation: 1,
        plugins: enabled.clone(),
        load_order: profile.enabled_plugins.clone(),
        runner_bindings,
        contract_surfaces: surfaces_for(&enabled),
    }
}

fn surfaces_for(manifests: &[PluginManifest]) -> Vec<ContractSurface> {
    let mut surfaces = Vec::new();
    for manifest in manifests {
        for runner in &manifest.provides.runners {
            surfaces.push(ContractSurface {
                surface_id: format!("runner:{}", runner.runner_id),
                kind: ContractSurfaceKind::Runner,
                owner_plugin_id: manifest.plugin_id.clone(),
                fingerprint: format!("runner:{}:{}", runner.runner_id, runner.plugin_generation),
                deprecated: false,
            });
            for kind in &runner.accepted_task_kinds {
                surfaces.push(ContractSurface {
                    surface_id: format!("task_kind:{kind}"),
                    kind: ContractSurfaceKind::TaskKind,
                    owner_plugin_id: manifest.plugin_id.clone(),
                    fingerprint: format!("task_kind:{kind}"),
                    deprecated: false,
                });
            }
        }
        for demand in &manifest.provides.task_demands {
            surfaces.push(ContractSurface {
                surface_id: format!("task_demand:{}", demand.demand_id),
                kind: ContractSurfaceKind::TaskDemand,
                owner_plugin_id: manifest.plugin_id.clone(),
                fingerprint: format!("demand:{}", demand.demand_id),
                deprecated: false,
            });
        }
    }
    surfaces
}

fn core_manifest(runner: RunnerDescriptor) -> PluginManifest {
    PluginManifest {
        plugin_id: "core".into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "core".into(),
            sha256: "sha256:core".into(),
        },
        provides: PluginProvides {
            runners: vec![runner],
            task_demands: Vec::new(),
            resource_schemas: Vec::new(),
            resource_providers: Vec::new(),
            effects: Vec::new(),
            state_schemas: Vec::new(),
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: Vec::new(),
            resources: Vec::new(),
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "core".into(),
            unload_timeout_ms: 0,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: true,
        },
        metadata: BTreeMap::new(),
    }
}

pub fn runner_manifest(plugin_id: &str, runners: Vec<RunnerDescriptor>) -> PluginManifest {
    PluginManifest {
        plugin_id: plugin_id.into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "native".into(),
            sha256: "sha256:native".into(),
        },
        provides: PluginProvides {
            runners,
            task_demands: Vec::new(),
            resource_schemas: Vec::new(),
            resource_providers: Vec::new(),
            effects: Vec::new(),
            state_schemas: Vec::new(),
        },
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
    }
}
