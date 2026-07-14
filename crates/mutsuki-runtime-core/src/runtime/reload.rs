use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{RuntimeEventKind, RuntimeLoadPlan};

use crate::RuntimeResult;
use crate::registry::{
    PluginGenerationPhase, PluginGenerationState, ReloadDecision, RunnerRegistry, compare_surfaces,
    validate_runtime_descriptors,
};
use crate::runner::Runner;

use super::{CoreRuntime, DrainingGeneration};
use invocation::cancel_attrs;

mod generations;
mod invocation;
mod occupancy;

pub(super) fn generation_states_for_plan(
    load_plan: &RuntimeLoadPlan,
    phase: PluginGenerationPhase,
) -> Vec<PluginGenerationState> {
    generations::generation_states_for_plan(load_plan, phase)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationPollution {
    Clean,
    LocalDirty,
    Polluted,
    UnknownDirty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningInvocationDisposition {
    pub task_id: String,
    pub invocation_id: String,
    pub runner_id: String,
    pub plugin_id: String,
    pub plugin_generation: u64,
    pub pollution: InvocationPollution,
}

impl CoreRuntime {
    pub fn draining_generation_count(&self) -> usize {
        self.draining_generations.len()
    }

    pub fn reload_load_plan_only(
        &mut self,
        new_plan: RuntimeLoadPlan,
    ) -> RuntimeResult<ReloadDecision> {
        let occupancy = self.surface_occupancy();
        let decision = compare_surfaces(&self.surfaces, &new_plan.contract_surfaces, &occupancy)?;
        if decision.blocked {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                "runtime.reload",
                "reload.breaking",
            ));
        }
        self.events.record(
            RuntimeEventKind::Reload,
            "plugin.reload",
            Some(new_plan.registry_generation.to_string()),
            BTreeMap::new(),
            None,
        );
        self.apply_load_plan(new_plan);
        Ok(decision)
    }

    pub fn reload_with_runners(
        &mut self,
        new_plan: RuntimeLoadPlan,
        new_runners: Vec<Box<dyn Runner>>,
    ) -> RuntimeResult<ReloadDecision> {
        let runner_descriptors: Vec<_> = new_runners
            .iter()
            .map(|runner| runner.descriptor().clone())
            .collect();
        validate_runtime_descriptors(&new_plan, &runner_descriptors)?;
        let occupancy = self.surface_occupancy();
        let decision = compare_surfaces(&self.surfaces, &new_plan.contract_surfaces, &occupancy)?;
        if decision.blocked {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                "runtime.reload",
                "reload.breaking",
            ));
        }

        let mut new_registry = RunnerRegistry::default();
        for runner in new_runners {
            new_registry.register(runner)?;
        }
        new_registry.freeze();
        for shadow_state in
            generation_states_for_plan(&new_plan, PluginGenerationPhase::ShadowStarting)
        {
            if !self.generation_states.iter().any(|state| {
                state.plugin_id == shadow_state.plugin_id
                    && state.generation == shadow_state.generation
            }) {
                self.generation_states.push(shadow_state);
            }
        }

        let old_registry_generation = self.load_plan.registry_generation;
        let dispositions = self.classify_running_invocations();
        let old_runner_ids = self.registry.runner_ids();
        for disposition in &dispositions {
            match disposition.pollution {
                InvocationPollution::Clean | InvocationPollution::LocalDirty => {
                    self.registry
                        .cancel_runner(&disposition.runner_id, &disposition.invocation_id)?;
                    self.tasks.cancel_running_invocation(
                        &disposition.runner_id,
                        &disposition.invocation_id,
                        self.current_step,
                    );
                    self.events.record(
                        RuntimeEventKind::Runner,
                        "runner.cancel",
                        Some(disposition.runner_id.clone()),
                        cancel_attrs(disposition, "reload.cancel_requeue"),
                        None,
                    );
                }
                InvocationPollution::Polluted | InvocationPollution::UnknownDirty => {
                    self.events.record(
                        RuntimeEventKind::Reload,
                        "plugin.reload.drain_invocation",
                        Some(disposition.task_id.clone()),
                        cancel_attrs(disposition, "reload.drain"),
                        None,
                    );
                }
            }
        }
        let mut old_registry = std::mem::take(&mut self.registry);
        let needs_drain = dispositions.iter().any(|disposition| {
            matches!(
                disposition.pollution,
                InvocationPollution::Polluted | InvocationPollution::UnknownDirty
            )
        });
        if needs_drain {
            self.draining_generations.push(DrainingGeneration {
                registry_generation: old_registry_generation,
                runner_ids: old_runner_ids,
                registry: old_registry,
            });
        } else {
            let _disposed = old_registry.dispose_all()?;
            self.mark_generation_phase(old_registry_generation, PluginGenerationPhase::Disposed);
        }
        self.events.record(
            RuntimeEventKind::Reload,
            "plugin.reload.swap_generation",
            Some(new_plan.registry_generation.to_string()),
            BTreeMap::new(),
            None,
        );
        self.apply_load_plan(new_plan);
        self.registry = new_registry;
        self.set_active_generation_states();
        self.settle_draining_generations()?;
        Ok(decision)
    }
}
