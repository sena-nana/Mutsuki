use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_runtime_contracts::{RuntimeEventKind, RuntimeLoadPlan};

use crate::RuntimeResult;
use crate::registry::{
    PluginGenerationPhase, PluginGenerationState, ReloadDecision, RunnerRegistry, compare_surfaces,
    validate_runtime_descriptors,
};
use crate::runner::{AsyncBatchHandler, Runner};

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
    pub task_id: mutsuki_runtime_contracts::TaskId,
    pub invocation_id: String,
    pub runner_id: mutsuki_runtime_contracts::RunnerId,
    pub plugin_id: mutsuki_runtime_contracts::PluginId,
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
        self.reload_with_async_handlers(new_plan, new_runners, Vec::new())
    }

    pub fn reload_with_async_handlers(
        &mut self,
        new_plan: RuntimeLoadPlan,
        new_runners: Vec<Box<dyn Runner>>,
        new_async_handlers: Vec<Arc<dyn AsyncBatchHandler>>,
    ) -> RuntimeResult<ReloadDecision> {
        let runner_descriptors: Vec<_> = new_runners
            .iter()
            .map(|runner| runner.descriptor().clone())
            .chain(
                new_async_handlers
                    .iter()
                    .map(|handler| handler.descriptor().clone()),
            )
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
        for handler in new_async_handlers {
            new_registry.register_async_handler(handler)?;
        }
        new_registry.validate_instance_counts()?;
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
                        Some(disposition.runner_id.to_string()),
                        cancel_attrs(disposition, "reload.cancel_requeue"),
                        None,
                    );
                }
                InvocationPollution::Polluted | InvocationPollution::UnknownDirty => {
                    self.events.record(
                        RuntimeEventKind::Reload,
                        "plugin.reload.drain_invocation",
                        Some(disposition.task_id.to_string()),
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
                plugin_ids: self
                    .load_plan
                    .plugins
                    .iter()
                    .map(|plugin| {
                        mutsuki_runtime_contracts::PluginId::from(plugin.plugin_id.as_str())
                    })
                    .collect(),
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

    pub fn reload_targeted_with_async_handlers(
        &mut self,
        mut new_plan: RuntimeLoadPlan,
        new_runners: Vec<Box<dyn Runner>>,
        new_async_handlers: Vec<Arc<dyn AsyncBatchHandler>>,
        affected_plugins: std::collections::BTreeSet<mutsuki_runtime_contracts::PluginId>,
    ) -> RuntimeResult<ReloadDecision> {
        if affected_plugins.is_empty() {
            return self.reload_load_plan_only(new_plan);
        }

        let old_descriptors = self
            .registry
            .descriptors()
            .into_iter()
            .map(|descriptor| {
                (
                    mutsuki_runtime_contracts::RunnerId::from(descriptor.runner_id.as_str()),
                    descriptor,
                )
            })
            .collect::<BTreeMap<_, _>>();
        for plugin in &mut new_plan.plugins {
            if affected_plugins.contains(plugin.plugin_id.as_str()) {
                continue;
            }
            for runner in &mut plugin.provides.runners {
                if let Some(previous) = old_descriptors.get(runner.runner_id.as_str()) {
                    runner.plugin_generation = previous.plugin_generation;
                }
            }
        }

        let mut candidate_registry = RunnerRegistry::default();
        for mut runner in new_runners {
            if affected_plugins.contains(runner.descriptor().plugin_id.as_str()) {
                candidate_registry.register(runner)?;
            } else {
                runner.dispose()?;
            }
        }
        for handler in new_async_handlers {
            if affected_plugins.contains(handler.descriptor().plugin_id.as_str()) {
                candidate_registry.register_async_handler(handler)?;
            } else if let Some(management) = handler.management_handle() {
                management.dispose()?;
            }
        }
        candidate_registry.validate_instance_counts()?;

        let runner_descriptors = old_descriptors
            .values()
            .filter(|descriptor| !affected_plugins.contains(descriptor.plugin_id.as_str()))
            .cloned()
            .chain(candidate_registry.descriptors())
            .collect::<Vec<_>>();
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

        for shadow_state in
            generation_states_for_plan(&new_plan, PluginGenerationPhase::ShadowStarting)
                .into_iter()
                .filter(|state| affected_plugins.contains(&state.plugin_id))
        {
            if !self.generation_states.iter().any(|state| {
                state.plugin_id == shadow_state.plugin_id
                    && state.generation == shadow_state.generation
            }) {
                self.generation_states.push(shadow_state);
            }
        }

        let old_registry_generation = self.load_plan.registry_generation;
        let dispositions = self
            .classify_running_invocations()
            .into_iter()
            .filter(|disposition| affected_plugins.contains(&disposition.plugin_id))
            .collect::<Vec<_>>();
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
                        Some(disposition.runner_id.to_string()),
                        cancel_attrs(disposition, "reload.cancel_requeue"),
                        None,
                    );
                }
                InvocationPollution::Polluted | InvocationPollution::UnknownDirty => {
                    self.events.record(
                        RuntimeEventKind::Reload,
                        "plugin.reload.drain_invocation",
                        Some(disposition.task_id.to_string()),
                        cancel_attrs(disposition, "reload.drain"),
                        None,
                    );
                }
            }
        }

        let old_registry = std::mem::take(&mut self.registry);
        let (mut retained_registry, mut retired_registry) =
            old_registry.partition_by_plugins(&affected_plugins);
        let retired_runner_ids = retired_registry.runner_ids();
        retained_registry.absorb(candidate_registry);
        retained_registry.validate_instance_counts()?;
        retained_registry.freeze();
        let needs_drain = dispositions.iter().any(|disposition| {
            matches!(
                disposition.pollution,
                InvocationPollution::Polluted | InvocationPollution::UnknownDirty
            )
        });
        if needs_drain {
            self.draining_generations.push(DrainingGeneration {
                registry_generation: old_registry_generation,
                runner_ids: retired_runner_ids,
                plugin_ids: affected_plugins.clone(),
                registry: retired_registry,
            });
        } else {
            let _disposed = retired_registry.dispose_all()?;
            self.mark_generation_phase_for_plugins(
                old_registry_generation,
                &affected_plugins,
                PluginGenerationPhase::Disposed,
            );
        }
        self.events.record(
            RuntimeEventKind::Reload,
            "plugin.reload.swap_generation",
            Some(new_plan.registry_generation.to_string()),
            BTreeMap::new(),
            None,
        );
        self.apply_load_plan(new_plan);
        self.registry = retained_registry;
        self.set_active_generation_states_for_plugins(&affected_plugins);
        self.settle_draining_generations()?;
        Ok(decision)
    }
}
