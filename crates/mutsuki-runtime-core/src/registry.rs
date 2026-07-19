use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use mutsuki_runtime_contracts::{
    ContractSurface, ERR_REGISTRY_FROZEN, ERR_REGISTRY_UNAUTHORIZED, ERR_RELOAD_BLOCKED,
    ExecutionClass, HandlerBinding, InvocationMode, OrderingRequirement, PayloadLayout,
    ProtocolClass, RunnerConcurrency, RunnerDescriptor, RunnerPurity, RuntimeLoadPlan,
    SurfaceCompatibility, SurfaceOccupancy,
};

use crate::{AsyncBatchHandler, Runner, RuntimeResult};

#[derive(Default)]
pub struct RunnerRegistry {
    runners: HashMap<String, Vec<Box<dyn Runner>>>,
    async_handlers: HashMap<String, Arc<dyn AsyncBatchHandler>>,
    descriptors: HashMap<String, RunnerDescriptor>,
    descriptor_snapshot: Arc<[RunnerDescriptor]>,
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
        let descriptor = normalized_descriptor(runner.descriptor().clone());
        if self.async_handlers.contains_key(&runner_id) {
            return Err(duplicate_runner(&runner_id));
        }
        if let Some(existing) = self.descriptors.get(&runner_id)
            && existing != &descriptor
        {
            return Err(duplicate_runner(&runner_id));
        }
        let runners = self.runners.entry(runner_id.clone()).or_default();
        let allowed_instances = match descriptor.concurrency {
            RunnerConcurrency::Sharded { instances } => instances,
            _ => 1,
        };
        if runners.len() >= allowed_instances {
            return Err(duplicate_runner(&runner_id));
        }
        runners.push(runner);
        self.descriptors.insert(runner_id, descriptor);
        self.rebuild_descriptor_snapshot();
        Ok(())
    }

    pub fn register_async_handler(
        &mut self,
        handler: Arc<dyn AsyncBatchHandler>,
    ) -> RuntimeResult<()> {
        if self.frozen {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_FROZEN,
                "runtime.runner_registry",
                "async_handler.register",
            ));
        }
        let descriptor = normalized_descriptor(handler.descriptor().clone());
        let runner_id = descriptor.runner_id.clone();
        if self.descriptors.contains_key(&runner_id)
            || self.runners.contains_key(&runner_id)
            || self.async_handlers.contains_key(&runner_id)
        {
            return Err(duplicate_runner(&runner_id));
        }
        if !matches!(
            descriptor.invocation_mode,
            InvocationMode::AsyncReentrant | InvocationMode::AsyncExclusive
        ) {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.runner_registry",
                format!("runner.{runner_id}.async_invocation_mode"),
            ));
        }
        self.descriptors.insert(runner_id.clone(), descriptor);
        self.async_handlers.insert(runner_id, handler);
        self.rebuild_descriptor_snapshot();
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
        self.async_handlers.remove(runner_id);
        self.descriptors.remove(runner_id);
        self.rebuild_descriptor_snapshot();
        Ok(self
            .runners
            .remove(runner_id)
            .and_then(|mut runners| runners.pop()))
    }

    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    pub(crate) fn validate_instance_counts(&self) -> RuntimeResult<()> {
        for descriptor in self.descriptors.values() {
            let expected = match descriptor.concurrency {
                RunnerConcurrency::Sharded { instances } => instances,
                _ => 1,
            };
            let actual = self
                .runners
                .get(&descriptor.runner_id)
                .map(Vec::len)
                .unwrap_or_else(|| {
                    usize::from(self.async_handlers.contains_key(&descriptor.runner_id))
                });
            if actual != expected {
                return Err(crate::runtime_failure(
                    ERR_REGISTRY_UNAUTHORIZED,
                    "runtime.runner_registry",
                    format!(
                        "runner.{}.instances.{actual}.expected.{expected}",
                        descriptor.runner_id
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn descriptors(&self) -> Vec<mutsuki_runtime_contracts::RunnerDescriptor> {
        self.descriptor_snapshot.to_vec()
    }

    pub(crate) fn descriptor_snapshot(&self) -> Arc<[RunnerDescriptor]> {
        Arc::clone(&self.descriptor_snapshot)
    }

    pub fn descriptor(
        &self,
        runner_id: &str,
    ) -> Option<mutsuki_runtime_contracts::RunnerDescriptor> {
        self.descriptors.get(runner_id).cloned()
    }

    pub fn runner_ids(&self) -> Vec<String> {
        self.descriptor_snapshot
            .iter()
            .map(|descriptor| descriptor.runner_id.clone())
            .collect()
    }

    pub fn heartbeat(
        &mut self,
        runner_id: &str,
        executor_id: &str,
        current_step: u64,
    ) -> RuntimeResult<RunnerHeartbeat> {
        if !self.descriptors.contains_key(runner_id) {
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
        let descriptor = self.descriptors.get(runner_id).ok_or_else(|| {
            crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                "runtime.runner_registry",
                format!("runner.capability.{runner_id}"),
            )
        })?;
        let authorized = &descriptor.accepted_protocol_ids;
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
        self.runners.get_mut(runner_id)?.pop()
    }

    pub(crate) fn put_runner(&mut self, runner: Box<dyn Runner>) {
        let runner_id = runner.descriptor().runner_id.clone();
        self.runners.entry(runner_id).or_default().push(runner);
    }

    pub(crate) fn async_handler(&self, runner_id: &str) -> Option<Arc<dyn AsyncBatchHandler>> {
        self.async_handlers.get(runner_id).cloned()
    }

    fn rebuild_descriptor_snapshot(&mut self) {
        let mut descriptors = self.descriptors.values().cloned().collect::<Vec<_>>();
        descriptors.sort_by(|left, right| left.runner_id.cmp(&right.runner_id));
        self.descriptor_snapshot = descriptors.into();
    }

    pub fn cancel_runner(&mut self, runner_id: &str, invocation_id: &str) -> RuntimeResult<()> {
        let runner = self
            .runners
            .get_mut(runner_id)
            .and_then(|runners| runners.last_mut())
            .ok_or_else(|| {
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
        for runners in self.runners.values_mut() {
            for runner in runners {
                runner.dispose()?;
                bag.disposed.push(runner.descriptor().runner_id.clone());
            }
        }
        for handler in self.async_handlers.values() {
            if let Some(management) = handler.management_handle() {
                management.dispose()?;
            }
            bag.disposed.push(handler.descriptor().runner_id.clone());
        }
        Ok(bag)
    }
}

fn normalized_descriptor(mut descriptor: RunnerDescriptor) -> RunnerDescriptor {
    descriptor.accepted_protocol_ids.sort();
    descriptor.accepted_protocol_ids.dedup();
    descriptor
}

fn duplicate_runner(runner_id: &str) -> crate::RuntimeFailure {
    crate::runtime_failure(
        ERR_REGISTRY_UNAUTHORIZED,
        "runtime.runner_registry",
        format!("runner.{runner_id}.duplicate"),
    )
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
    for runner in &planned {
        validate_runner_privilege(runner)?;
        validate_runner_protocol_classes(load_plan, runner)?;
    }
    for runner in runners {
        let Some(planned_runner) = planned
            .iter()
            .find(|planned_runner| planned_runner.runner_id == runner.runner_id)
        else {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan",
                format!("runner.{}", runner.runner_id),
            ));
        };
        if planned_runner.invocation_mode != runner.invocation_mode
            || planned_runner.concurrency != runner.concurrency
        {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan",
                format!("runner.{}.invocation", runner.runner_id),
            ));
        }
        validate_runner_privilege(runner)?;
        validate_runner_protocol_classes(load_plan, runner)?;
    }
    validate_handler_bindings(load_plan)?;
    Ok(())
}

fn validate_runner_protocol_classes(
    load_plan: &RuntimeLoadPlan,
    runner: &RunnerDescriptor,
) -> RuntimeResult<()> {
    for protocol_id in &runner.accepted_protocol_ids {
        let class = load_plan
            .plugins
            .iter()
            .find_map(|plugin| plugin.provides.protocol_classes.get(protocol_id))
            .ok_or_else(|| {
                crate::runtime_failure(
                    ERR_REGISTRY_UNAUTHORIZED,
                    "runtime.load_plan.protocol_class",
                    format!("runner.{}.protocol.{protocol_id}.unknown", runner.runner_id),
                )
            })?;
        let matches_purity = matches!(
            (&runner.purity, class),
            (RunnerPurity::Pure, ProtocolClass::Domain)
                | (RunnerPurity::Effectful, ProtocolClass::Effect)
                | (RunnerPurity::Committer, ProtocolClass::Core)
                | (RunnerPurity::Committer, ProtocolClass::Control)
        );
        if !matches_purity
            || (class == &ProtocolClass::Control
                && runner.execution_class != ExecutionClass::Control)
        {
            return Err(crate::runtime_failure(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan.protocol_class",
                format!(
                    "runner.{}.protocol.{protocol_id}.purity_conflict",
                    runner.runner_id
                ),
            ));
        }
    }
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
    let declared_batches = runner.concurrency.max_inflight_batches();
    let declared_entries = runner
        .concurrency
        .max_inflight_entries(runner.batch.max_batch_entries);
    if runner.batch.preferred_batch_size == 0
        || runner.batch.max_batch_entries == 0
        || runner.batch.max_entry_concurrency == 0
        || runner.batch.max_inflight_batches == 0
        || runner.batch.max_inflight_batches != declared_batches
        || declared_batches == 0
        || declared_entries == 0
        || runner.batch.preferred_batch_size > runner.batch.max_batch_entries
        || runner.batch.max_entry_concurrency > runner.batch.max_batch_entries
    {
        return Err(crate::runtime_failure(
            ERR_REGISTRY_UNAUTHORIZED,
            "runtime.load_plan",
            format!("runner.{}.batch", runner.runner_id),
        ));
    }
    let invocation_matches_concurrency = matches!(
        (&runner.invocation_mode, &runner.concurrency),
        (InvocationMode::SyncExclusive, RunnerConcurrency::Exclusive)
            | (
                InvocationMode::SyncExclusive,
                RunnerConcurrency::Sharded { .. }
            )
            | (InvocationMode::AsyncExclusive, RunnerConcurrency::Exclusive)
            | (
                InvocationMode::AsyncReentrant,
                RunnerConcurrency::Reentrant { .. }
            )
            | (
                InvocationMode::ExternalProcess,
                RunnerConcurrency::Exclusive
            )
            | (
                InvocationMode::ExternalProcess,
                RunnerConcurrency::Sharded { .. }
            )
    );
    if !invocation_matches_concurrency {
        return Err(crate::runtime_failure(
            ERR_REGISTRY_UNAUTHORIZED,
            "runtime.load_plan",
            format!("runner.{}.concurrency", runner.runner_id),
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
