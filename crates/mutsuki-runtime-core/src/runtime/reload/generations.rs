use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{RuntimeEventKind, RuntimeLoadPlan};

use crate::RuntimeResult;
use crate::registry::{
    DisposeBag, HandlerBindingRegistry, PluginGenerationPhase, PluginGenerationState,
};

use super::CoreRuntime;

impl CoreRuntime {
    pub fn dispose_plugins(&mut self) -> RuntimeResult<DisposeBag> {
        let bag = self.registry.dispose_all()?;
        self.mark_generation_phase(
            self.load_plan.registry_generation,
            PluginGenerationPhase::Disposed,
        );
        self.events.record(
            RuntimeEventKind::Plugin,
            "plugin.dispose",
            None,
            BTreeMap::new(),
            None,
        );
        Ok(bag)
    }

    pub fn settle_draining_generations(&mut self) -> RuntimeResult<DisposeBag> {
        let mut disposed = DisposeBag::default();
        let mut remaining = Vec::new();
        let mut disposed_generations = Vec::new();
        for mut generation in self.draining_generations.drain(..) {
            let still_running = generation
                .runner_ids
                .iter()
                .any(|runner_id| !self.tasks.running_records_for_runner(runner_id).is_empty());
            if still_running {
                remaining.push(generation);
            } else {
                let bag = generation.registry.dispose_all()?;
                disposed.disposed.extend(bag.disposed);
                disposed_generations.push(generation.registry_generation);
            }
        }
        self.draining_generations = remaining;
        for registry_generation in disposed_generations {
            self.mark_generation_phase(registry_generation, PluginGenerationPhase::Disposed);
        }
        Ok(disposed)
    }

    pub(super) fn mark_generation_phase(
        &mut self,
        registry_generation: u64,
        phase: PluginGenerationPhase,
    ) {
        for state in &mut self.generation_states {
            if state.generation == registry_generation {
                state.phase = phase.clone();
            }
        }
    }

    pub(super) fn set_active_generation_states(&mut self) {
        let active_generation = self.load_plan.registry_generation;
        for state in &mut self.generation_states {
            if state.generation == active_generation {
                state.phase = PluginGenerationPhase::Active;
            } else if state.phase == PluginGenerationPhase::Active {
                state.phase = PluginGenerationPhase::Draining;
            }
        }
        for new_state in generation_states_for_plan(&self.load_plan, PluginGenerationPhase::Active)
        {
            if !self.generation_states.iter().any(|state| {
                state.plugin_id == new_state.plugin_id && state.generation == new_state.generation
            }) {
                self.generation_states.push(new_state);
            }
        }
    }

    pub(super) fn apply_load_plan(&mut self, new_plan: RuntimeLoadPlan) {
        self.tasks.rebind_ready_generation(
            self.load_plan.registry_generation,
            new_plan.registry_generation,
        );
        self.handler_bindings = HandlerBindingRegistry::from_load_plan(&new_plan);
        self.surfaces = new_plan.contract_surfaces.clone();
        self.protocol_classes = super::super::protocol_classes_for_plan(&new_plan);
        self.load_plan = new_plan;
    }
}

pub(super) fn generation_states_for_plan(
    load_plan: &RuntimeLoadPlan,
    phase: PluginGenerationPhase,
) -> Vec<PluginGenerationState> {
    load_plan
        .plugins
        .iter()
        .map(|plugin| PluginGenerationState {
            plugin_id: plugin.plugin_id.clone(),
            generation: load_plan.registry_generation,
            phase: phase.clone(),
        })
        .collect()
}
