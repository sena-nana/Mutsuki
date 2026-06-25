use std::collections::{BTreeMap, BTreeSet, HashMap};

use mutsuki_runtime_contracts::{
    ContractSurface, ERR_REGISTRY_FROZEN, ERR_REGISTRY_UNAUTHORIZED, ERR_RELOAD_BLOCKED,
    RuntimeError, RuntimeLoadPlan, SurfaceCompatibility, SurfaceOccupancy, TaskDemand,
};

use crate::{Runner, RuntimeFailure, RuntimeResult};

#[derive(Default)]
pub struct RunnerRegistry {
    runners: HashMap<String, Box<dyn Runner>>,
    frozen: bool,
}

impl RunnerRegistry {
    pub fn register(&mut self, runner: Box<dyn Runner>) -> RuntimeResult<()> {
        if self.frozen {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_REGISTRY_FROZEN,
                "runtime.runner_registry",
                "runner.register",
            )));
        }
        let runner_id = runner.descriptor().runner_id.clone();
        self.runners.insert(runner_id, runner);
        Ok(())
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

    pub fn take_runner(&mut self, runner_id: &str) -> Option<Box<dyn Runner>> {
        self.runners.remove(runner_id)
    }

    pub fn put_runner(&mut self, runner: Box<dyn Runner>) {
        let runner_id = runner.descriptor().runner_id.clone();
        self.runners.insert(runner_id, runner);
    }

    pub fn cancel_runner(&mut self, runner_id: &str, invocation_id: &str) -> RuntimeResult<()> {
        let runner = self.runners.get_mut(runner_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                "runtime.runner_registry",
                format!("runner.cancel.{runner_id}"),
            ))
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
    pub task_demands: Vec<TaskDemand>,
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
    runners: &[mutsuki_runtime_contracts::RunnerDescriptor],
) -> RuntimeResult<()> {
    let authorized: BTreeSet<String> = load_plan
        .plugins
        .iter()
        .flat_map(|plugin| plugin.provides.runners.iter())
        .map(|runner| runner.runner_id.clone())
        .collect();
    for runner in runners {
        if !authorized.contains(&runner.runner_id) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_REGISTRY_UNAUTHORIZED,
                "runtime.load_plan",
                format!("runner.{}", runner.runner_id),
            )));
        }
    }
    Ok(())
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
                    return Err(RuntimeFailure::new(RuntimeError::new(
                        ERR_RELOAD_BLOCKED,
                        "runtime.reload",
                        format!("surface.remove.{surface_id}"),
                    )));
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
