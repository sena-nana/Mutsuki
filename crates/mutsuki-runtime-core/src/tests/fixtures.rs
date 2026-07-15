use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

macro_rules! boxed_runner {
    ($descriptor:expr, |$task:ident| $body:block) => {
        Box::new(super::fixtures::StaticRunner::new($descriptor, |$task| {
            $body
        })) as Box<dyn $crate::Runner>
    };
    ($descriptor:expr, move |$task:ident| $body:block) => {
        Box::new(super::fixtures::StaticRunner::new(
            $descriptor,
            move |$task| $body,
        )) as Box<dyn $crate::Runner>
    };
}

macro_rules! completed_runner {
    ($descriptor:expr) => {
        boxed_runner!($descriptor, |task| {
            mutsuki_runtime_contracts::RunnerResult::completed(task.task_id.clone())
        })
    };
}

macro_rules! kernel_runner {
    ($generation:expr) => {
        Box::new($crate::CoreKernelRunner::new($generation)) as Box<dyn $crate::Runner>
    };
}

macro_rules! runners_with_kernel {
    () => {
        runners_with_kernel!(1;)
    };
    ($($runner:expr),* $(,)?) => {
        runners_with_kernel!(1; $($runner),*)
    };
    ($generation:expr;) => {
        vec![kernel_runner!($generation)]
    };
    ($generation:expr; $($runner:expr),* $(,)?) => {
        vec![$($runner),*, kernel_runner!($generation)]
    };
}

pub(super) fn runner_descriptor(
    id: &str,
    protocol_id: &str,
    purity: RunnerPurity,
) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: id.into(),
        plugin_id: if id == "core.kernel" {
            "core"
        } else {
            "plugin-a"
        }
        .into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![protocol_id.into()],
        purity,
        execution_class: ExecutionClass::Cpu,
        input_schema: json!({}),
        output_schema: json!({}),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{id}")],
    }
}

fn manifest(
    runners: Vec<RunnerDescriptor>,
    handler_bindings: Vec<HandlerBinding>,
) -> PluginManifest {
    PluginManifest {
        plugin_id: "plugin-a".into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "native".into(),
            sha256: "sha256:native".into(),
        },
        provides: PluginProvides {
            runners,
            protocols: Vec::new(),
            handler_bindings,
            effects: vec!["effect.chat.send".into()],
            streams: vec!["chat.events".into()],
            subscriptions: vec!["chat.messages".into()],
            timers: vec!["heartbeat".into()],
            resource_schemas: vec!["bytes.v1".into()],
            resource_providers: vec!["mutsuki.std.resource.memory".into()],
            resource_types: Vec::new(),
            state_schemas: vec!["state.actor.v1".into()],
            host_extensions: Vec::new(),
            plugin_backends: Vec::new(),
            codecs: Vec::new(),
            bridges: Vec::new(),
            scheduler_policies: Vec::new(),
            workflows: Vec::new(),
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: vec!["effect.chat.send".into()],
            resources: vec!["read".into(), "write_own".into()],
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

pub(super) fn load_plan(
    runners: Vec<RunnerDescriptor>,
    handler_bindings: Vec<HandlerBinding>,
) -> RuntimeLoadPlan {
    let mut all_runners = runners;
    all_runners.push(CoreKernelRunner::new(1).descriptor().clone());
    let mut plugins = vec![manifest(all_runners, handler_bindings)];
    plugins[0].provides.runners[0].plugin_id = "plugin-a".into();
    let observability = ObservabilityProfile {
        detailed_scheduler_decisions: true,
        dispatch_spans: true,
        ..ObservabilityProfile::default()
    };
    RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: "default".into(),
        profile_hash: "sha256:profile".into(),
        registry_generation: 1,
        plugins,
        load_order: vec!["plugin-a".into()],
        runner_bindings: BTreeMap::new(),
        plugin_deployments: [("plugin-a".into(), PluginDeploymentKind::Builtin)].into(),
        observability,
        capability_graph: RuntimeCapabilityGraph::default(),
        contract_surfaces: vec![
            ContractSurface {
                surface_id: "runner:orchestrator".into(),
                kind: ContractSurfaceKind::Runner,
                owner_plugin_id: "plugin-a".into(),
                fingerprint: "sha256:orchestrator".into(),
                deprecated: false,
            },
            ContractSurface {
                surface_id: "runner:core.kernel".into(),
                kind: ContractSurfaceKind::Runner,
                owner_plugin_id: "core".into(),
                fingerprint: "sha256:core".into(),
                deprecated: false,
            },
        ],
    }
}

pub(super) fn surface(id: &str, kind: ContractSurfaceKind) -> ContractSurface {
    ContractSurface {
        surface_id: id.into(),
        kind,
        owner_plugin_id: "plugin-a".into(),
        fingerprint: id.into(),
        deprecated: false,
    }
}

pub(super) fn boot_with_kernel(plan: RuntimeLoadPlan) -> CoreRuntime {
    CoreRuntime::boot(plan, runners_with_kernel!()).unwrap()
}

pub(super) fn remove_surfaces(mut plan: RuntimeLoadPlan, surface_ids: &[&str]) -> RuntimeLoadPlan {
    plan.registry_generation = 2;
    plan.contract_surfaces
        .retain(|surface| !surface_ids.contains(&surface.surface_id.as_str()));
    plan
}

pub(super) fn occupancy_handle(
    surface_id: &str,
    kind: SurfaceOccupancyHandleKind,
) -> SurfaceOccupancyHandle {
    SurfaceOccupancyHandle {
        handle_id: format!("{surface_id}:1"),
        surface_id: surface_id.into(),
        owner_plugin_id: "plugin-a".into(),
        plugin_generation: 1,
        registry_generation: 1,
        kind,
    }
}

pub(super) fn external_resource_ref(
    ref_id: &str,
    kind_id: &str,
    schema: &str,
    provider_id: &str,
) -> ResourceRef {
    external_resource_ref_with_semantic(
        ref_id,
        kind_id,
        schema,
        provider_id,
        ResourceSemantic::FrozenValue,
        ResourceLifetime::ExternalManaged,
    )
}

pub(super) fn external_resource_ref_with_semantic(
    ref_id: &str,
    kind_id: &str,
    schema: &str,
    provider_id: &str,
    semantic: ResourceSemantic,
    lifetime: ResourceLifetime,
) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version: 1,
        },
        semantic,
        provider_id: provider_id.into(),
        resource_kind: kind_id.into(),
        schema: schema.into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::ProviderRpc {
            provider_id: provider_id.into(),
            method: "mutsuki.resource.get".into(),
        },
        size_hint: None,
        content_hash: None,
        lifetime,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

pub(super) struct StaticRunner {
    descriptor: RunnerDescriptor,
    result: Box<dyn Fn(&Task) -> RunnerResult + Send>,
}

pub(super) struct ContinuingRunner {
    descriptor: RunnerDescriptor,
    calls: Arc<Mutex<Vec<String>>>,
}

impl ContinuingRunner {
    pub(super) fn new(descriptor: RunnerDescriptor, calls: Arc<Mutex<Vec<String>>>) -> Self {
        Self { descriptor, calls }
    }
}

impl Runner for ContinuingRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        scalar_batch_result(&batch, |task| {
            Ok(RunnerResult {
                task_id: task.task_id.clone(),
                deltas: Vec::new(),
                events: Vec::new(),
                tasks: Vec::new(),
                effects: Vec::new(),
                values: Vec::new(),
                resources: Vec::new(),
                task_await: None,
                status: RunnerStatus::Continue,
            })
        })
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.calls
            .lock()
            .expect("calls mutex poisoned")
            .push(format!("cancel:{invocation_id}"));
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.calls
            .lock()
            .expect("calls mutex poisoned")
            .push(format!("dispose:{}", self.descriptor.runner_id));
        Ok(())
    }
}

impl StaticRunner {
    pub(super) fn new(
        descriptor: RunnerDescriptor,
        result: impl Fn(&Task) -> RunnerResult + Send + 'static,
    ) -> Self {
        Self {
            descriptor,
            result: Box::new(result),
        }
    }
}

impl Runner for StaticRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        scalar_batch_result(&batch, |task| Ok((self.result)(task)))
    }
}

pub(super) fn scalar_batch_result(
    batch: &WorkBatch,
    mut result: impl FnMut(&Task) -> RuntimeResult<RunnerResult>,
) -> RuntimeResult<CompletionBatch> {
    let tasks = match batch.row_payload_tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Ok(CompletionBatch::from_error(batch, error)),
    };
    let mut results = Vec::with_capacity(batch.entries.len());
    for entry in &batch.entries {
        let Some(task) = tasks.iter().find(|task| task.task_id == entry.task_id) else {
            results.push(EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: None,
                error: Some(RuntimeError::new(
                    ERR_TASK_CLAIM_CONFLICT,
                    "test.runner",
                    format!("batch.entry.{}", entry.entry_id),
                )),
            });
            continue;
        };
        match result(task) {
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
    Ok(CompletionBatch::from_results(batch, results))
}
