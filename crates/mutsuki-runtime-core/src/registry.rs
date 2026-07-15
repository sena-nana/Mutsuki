use std::collections::{BTreeMap, BTreeSet, HashMap};

use mutsuki_runtime_contracts::{
    ContractSurface, ERR_REGISTRY_FROZEN, ERR_REGISTRY_UNAUTHORIZED, ERR_RELOAD_BLOCKED,
    ExecutionClass, HandlerBinding, OrderingRequirement, PayloadLayout, RunnerDescriptor,
    RunnerPurity, RuntimeLoadPlan, SurfaceCompatibility, SurfaceOccupancy,
};

use crate::{Runner, RuntimeResult};

#[derive(Default)]
pub struct RunnerRegistry {
    runners: HashMap<String, Box<dyn Runner>>,
    heartbeats: HashMap<String, RunnerHeartbeat>,
    capabilities: HashMap<String, RunnerCapabilityDeclaration>,
    frozen: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerHeartbeat {
    pub runner_id: String,
    pub executor_id: String,
    pub last_seen_step: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerCapabilityDeclaration {
    pub runner_id: String,
    pub protocol_ids: Vec<String>,
    pub capacity: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HandlerBindingRegistry {
    bindings: Vec<HandlerBinding>,
}

impl HandlerBindingRegistry {
    pub fn from_load_plan(load_plan: &RuntimeLoadPlan) -> Self {
        let mut bindings: Vec<_> = load_plan
            .plugins
            .iter()
            .flat_map(|plugin| plugin.provides.handler_bindings.iter().cloned())
            .collect();
        Self::sort_bindings(&mut bindings);
        Self { bindings }
    }

    pub fn query_protocol(&self, protocol_id: &str) -> Vec<&HandlerBinding> {
        self.bindings
            .iter()
            .filter(|binding| binding.protocol_id == protocol_id)
            .collect()
    }

    pub fn all(&self) -> &[HandlerBinding] {
        &self.bindings
    }

    pub fn register_authorized(&mut self, binding: HandlerBinding) {
        if self
            .bindings
            .iter()
            .any(|existing| existing.binding_id == binding.binding_id)
        {
            return;
        }
        self.bindings.push(binding);
        Self::sort_bindings(&mut self.bindings);
    }

    fn sort_bindings(bindings: &mut [HandlerBinding]) {
        bindings.sort_by(|a, b| {
            a.protocol_id
                .cmp(&b.protocol_id)
                .then_with(|| b.priority.cmp(&a.priority))
                .then_with(|| a.binding_id.cmp(&b.binding_id))
        });
    }
}

impl RunnerRegistry {
    pub fn register(&mut self, runner: Box<dyn Runner>) -> RuntimeResult<()> {
        if self.frozen {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_FROZEN,
                "runtime.runner_registry",
                "runner.register",
            ));
        }
        let runner_id = runner.descriptor().runner_id.clone();
        self.runners.insert(runner_id, runner);
        Ok(())
    }

    pub fn unregister(&mut self, runner_id: &str) -> RuntimeResult<Option<Box<dyn Runner>>> {
        if self.frozen {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_FROZEN,
                "runtime.runner_registry",
                format!("runner.unregister.{runner_id}"),
            ));
        }
        self.heartbeats.remove(runner_id);
        self.capabilities.remove(runner_id);
        Ok(self.runners.remove(runner_id))
    }

    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    pub fn descriptors(&self) -> Vec<mutsuki_runtime_contracts::RunnerDescriptor> {
        let mut descriptors: Vec<_> = self
            .runners
            .values()
            .map(|runner| runner.descriptor().clone())
            .collect();
        descriptors.sort_by(|a, b| a.runner_id.cmp(&b.runner_id));
        descriptors
    }

    pub fn descriptor(
        &self,
        runner_id: &str,
    ) -> Option<mutsuki_runtime_contracts::RunnerDescriptor> {
        self.runners
            .get(runner_id)
            .map(|runner| runner.descriptor().clone())
    }

    pub fn runner_ids(&self) -> Vec<String> {
        let mut runner_ids: Vec<_> = self.runners.keys().cloned().collect();
        runner_ids.sort();
        runner_ids
    }

    pub fn heartbeat(
        &mut self,
        runner_id: &str,
        executor_id: &str,
        current_step: u64,
    ) -> RuntimeResult<RunnerHeartbeat> {
        if !self.runners.contains_key(runner_id) {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                "runtime.runner_registry",
                format!("runner.heartbeat.{runner_id}"),
            ));
        }
        let heartbeat = RunnerHeartbeat {
            runner_id: runner_id.into(),
            executor_id: executor_id.into(),
            last_seen_step: current_step,
        };
        self.heartbeats.insert(runner_id.into(), heartbeat.clone());
        Ok(heartbeat)
    }

    pub fn runner_heartbeat(&self, runner_id: &str) -> Option<&RunnerHeartbeat> {
        self.heartbeats.get(runner_id)
    }

    pub fn declare_capability(
        &mut self,
        runner_id: &str,
        protocol_ids: Vec<String>,
        capacity: usize,
    ) -> RuntimeResult<RunnerCapabilityDeclaration> {
        let descriptor = self.runners.get(runner_id).ok_or_else(|| {
            crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                "runtime.runner_registry",
                format!("runner.capability.{runner_id}"),
            )
        })?;
        let authorized = &descriptor.descriptor().accepted_protocol_ids;
        if protocol_ids
            .iter()
            .any(|protocol_id| !authorized.contains(protocol_id))
        {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.runner_registry",
                format!("runner.capability.{runner_id}"),
            ));
        }
        let declaration = RunnerCapabilityDeclaration {
            runner_id: runner_id.into(),
            protocol_ids,
            capacity,
        };
        self.capabilities
            .insert(runner_id.into(), declaration.clone());
        Ok(declaration)
    }

    pub fn runner_capability(&self, runner_id: &str) -> Option<&RunnerCapabilityDeclaration> {
        self.capabilities.get(runner_id)
    }

    pub(crate) fn take_runner(&mut self, runner_id: &str) -> Option<Box<dyn Runner>> {
        self.runners.remove(runner_id)
    }

    pub(crate) fn put_runner(&mut self, runner: Box<dyn Runner>) {
        let runner_id = runner.descriptor().runner_id.clone();
        self.runners.insert(runner_id, runner);
    }

    pub fn cancel_runner(&mut self, runner_id: &str, invocation_id: &str) -> RuntimeResult<()> {
        let runner = self.runners.get_mut(runner_id).ok_or_else(|| {
            crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                "runtime.runner_registry",
                format!("runner.cancel.{runner_id}"),
            )
        })?;
        runner.cancel(invocation_id)
    }

    pub fn dispose_all(&mut self) -> RuntimeResult<DisposeBag> {
        let mut bag = DisposeBag::default();
        for runner in self.runners.values_mut() {
            runner.dispose()?;
            bag.disposed.push(runner.descriptor().runner_id.clone());
        }
        Ok(bag)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisposeBag {
    pub disposed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginGenerationPhase {
    ShadowStarting,
    Active,
    Draining,
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginGenerationState {
    pub plugin_id: String,
    pub generation: u64,
    pub phase: PluginGenerationPhase,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegistrySnapshot {
    pub generation: u64,
    pub frozen: bool,
    pub runners: Vec<mutsuki_runtime_contracts::RunnerDescriptor>,
    pub handler_bindings: Vec<HandlerBinding>,
    pub surfaces: Vec<ContractSurface>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractChange {
    pub surface_id: String,
    pub compatibility: SurfaceCompatibility,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadDecision {
    pub changes: Vec<ContractChange>,
    pub blocked: bool,
}

pub fn validate_runtime_descriptors(
    load_plan: &RuntimeLoadPlan,
    runners: &[RunnerDescriptor],
) -> RuntimeResult<()> {
    let planned: Vec<_> = load_plan
        .plugins
        .iter()
        .flat_map(|plugin| plugin.provides.runners.iter())
        .collect();
    let authorized: BTreeSet<String> = planned
        .iter()
        .map(|runner| runner.runner_id.clone())
        .collect();
    for runner in planned {
        validate_runner_privilege(runner)?;
    }
    for runner in runners {
        if !authorized.contains(&runner.runner_id) {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan",
                format!("runner.{}", runner.runner_id),
            ));
        }
        validate_runner_privilege(runner)?;
    }
    validate_handler_bindings(load_plan)?;
    Ok(())
}

fn validate_runner_privilege(runner: &RunnerDescriptor) -> RuntimeResult<()> {
    if runner.purity == RunnerPurity::Committer && runner.runner_id != "core.kernel" {
        return Err(crate::runtime_failure(
            ERR_REGISTRY_UNAUTHORIZED,
            "runtime.load_plan",
            format!("runner.{}.committer", runner.runner_id),
        ));
    }
    if runner.execution_class == ExecutionClass::Control && runner.runner_id != "core.kernel" {
        return Err(crate::runtime_failure(
            ERR_REGISTRY_UNAUTHORIZED,
            "runtime.load_plan",
            format!("runner.{}.control", runner.runner_id),
        ));
    }
    validate_runner_batch_capabilities(runner)?;
    Ok(())
}

fn validate_runner_batch_capabilities(runner: &RunnerDescriptor) -> RuntimeResult<()> {
    if runner.batch.preferred_batch_size == 0
        || runner.batch.max_batch_entries == 0
        || runner.batch.max_entry_concurrency == 0
        || runner.batch.max_inflight_batches != 1
        || runner.batch.preferred_batch_size > runner.batch.max_batch_entries
        || runner.batch.max_entry_concurrency > runner.batch.max_batch_entries
    {
        return Err(crate::runtime_failure(
            ERR_REGISTRY_UNAUTHORIZED,
            "runtime.load_plan",
            format!("runner.{}.batch", runner.runner_id),
        ));
    }
    if runner.payload.layouts.is_empty()
        || !runner
            .payload
            .layouts
            .iter()
            .any(|layout| layout == &runner.payload.preferred_layout)
        || !runner
            .payload
            .layouts
            .iter()
            .any(|layout| layout == &PayloadLayout::Row)
    {
        return Err(crate::runtime_failure(
            ERR_REGISTRY_UNAUTHORIZED,
            "runtime.load_plan",
            format!("runner.{}.payload", runner.runner_id),
        ));
    }
    match &runner.ordering.default {
        OrderingRequirement::StrictSequence { .. } if !runner.ordering.supports_sequence => {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan",
                format!("runner.{}.ordering.sequence", runner.runner_id),
            ));
        }
        OrderingRequirement::SameResourceOrder { .. }
            if !runner.ordering.supports_same_resource_order =>
        {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan",
                format!("runner.{}.ordering.resource", runner.runner_id),
            ));
        }
        _ => {}
    }
    Ok(())
}

fn validate_handler_bindings(load_plan: &RuntimeLoadPlan) -> RuntimeResult<()> {
    let runners: Vec<_> = load_plan
        .plugins
        .iter()
        .flat_map(|plugin| plugin.provides.runners.iter())
        .collect();

    for binding in load_plan
        .plugins
        .iter()
        .flat_map(|plugin| plugin.provides.handler_bindings.iter())
    {
        if !runners
            .iter()
            .any(|runner| runner_accepts_protocol(runner, &binding.target_protocol_id))
        {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan",
                format!(
                    "handler_binding.{}.target_protocol_id.{}",
                    binding.binding_id, binding.target_protocol_id
                ),
            ));
        }
        if let Some(runner_hint) = &binding.target_runner_hint {
            let Some(runner) = runners
                .iter()
                .find(|runner| runner.runner_id == *runner_hint)
            else {
                return Err(crate::runtime_failure(
                    ERR_REGISTRY_UNAUTHORIZED,
                    "runtime.load_plan",
                    format!(
                        "handler_binding.{}.target_runner_hint.{}",
                        binding.binding_id, runner_hint
                    ),
                ));
            };
            if !runner_accepts_protocol(runner, &binding.target_protocol_id) {
                return Err(crate::runtime_failure(
                    ERR_REGISTRY_UNAUTHORIZED,
                    "runtime.load_plan",
                    format!(
                        "handler_binding.{}.target_runner_hint.{}.target_protocol_id.{}",
                        binding.binding_id, runner_hint, binding.target_protocol_id
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn runner_accepts_protocol(
    runner: &mutsuki_runtime_contracts::RunnerDescriptor,
    protocol_id: &str,
) -> bool {
    runner
        .accepted_protocol_ids
        .iter()
        .any(|accepted| accepted == protocol_id)
}

pub fn compare_surfaces(
    old: &[ContractSurface],
    new: &[ContractSurface],
    occupancy: &[SurfaceOccupancy],
) -> RuntimeResult<ReloadDecision> {
    let old_by_id: BTreeMap<_, _> = old
        .iter()
        .map(|surface| (surface.surface_id.clone(), surface))
        .collect();
    let new_by_id: BTreeMap<_, _> = new
        .iter()
        .map(|surface| (surface.surface_id.clone(), surface))
        .collect();
    let occupancy_by_id: BTreeMap<_, _> = occupancy
        .iter()
        .map(|item| (item.surface_id.clone(), item))
        .collect();

    let mut changes = Vec::new();
    for (surface_id, old_surface) in &old_by_id {
        match new_by_id.get(surface_id) {
            Some(new_surface) if new_surface.fingerprint == old_surface.fingerprint => {
                changes.push(ContractChange {
                    surface_id: surface_id.clone(),
                    compatibility: if new_surface.deprecated && !old_surface.deprecated {
                        SurfaceCompatibility::Deprecated
                    } else {
                        SurfaceCompatibility::Identical
                    },
                });
            }
            Some(_) => changes.push(ContractChange {
                surface_id: surface_id.clone(),
                compatibility: SurfaceCompatibility::Breaking,
            }),
            None => {
                let zero = occupancy_by_id
                    .get(surface_id)
                    .is_none_or(|occupancy| occupancy.is_zero());
                if !zero {
                    return Err(crate::runtime_failure(
                        ERR_RELOAD_BLOCKED,
                        "runtime.reload",
                        format!("surface.remove.{surface_id}"),
                    ));
                }
                changes.push(ContractChange {
                    surface_id: surface_id.clone(),
                    compatibility: SurfaceCompatibility::Removed,
                });
            }
        }
    }
    for surface_id in new_by_id.keys() {
        if !old_by_id.contains_key(surface_id) {
            changes.push(ContractChange {
                surface_id: surface_id.clone(),
                compatibility: SurfaceCompatibility::Additive,
            });
        }
    }
    let blocked = changes
        .iter()
        .any(|change| change.compatibility == SurfaceCompatibility::Breaking);
    Ok(ReloadDecision { changes, blocked })
}
