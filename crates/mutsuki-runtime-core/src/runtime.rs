use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ContractSurface, ResourceRef, RuntimeError, RuntimeEventKind, RuntimeLoadPlan,
    SurfaceOccupancyHandle, Task,
};
use serde_json::Value;

use crate::logs::{EventLog, TraceLog};
use crate::registry::{
    HandlerBindingRegistry, PluginGenerationPhase, PluginGenerationState, RegistrySnapshot,
    RunnerRegistry, validate_runtime_descriptors,
};
use crate::runner::Runner;
use crate::state_store::StateStore;
use crate::task_pool::surface_ids_for_task;
use crate::{ResourceManager, RuntimeFailure, RuntimeResult, TaskPool};

mod reload;
mod runner_loop;

pub use reload::{InvocationPollution, RunningInvocationDisposition};

struct DrainingGeneration {
    registry_generation: u64,
    runner_ids: Vec<String>,
    registry: RunnerRegistry,
}

pub struct CoreRuntime {
    load_plan: RuntimeLoadPlan,
    handler_bindings: HandlerBindingRegistry,
    surfaces: Vec<ContractSurface>,
    registry: RunnerRegistry,
    draining_generations: Vec<DrainingGeneration>,
    generation_states: Vec<PluginGenerationState>,
    tasks: TaskPool,
    resources: ResourceManager,
    states: StateStore,
    events: EventLog,
    traces: TraceLog,
    current_step: u64,
}

impl CoreRuntime {
    pub fn boot(load_plan: RuntimeLoadPlan, runners: Vec<Box<dyn Runner>>) -> RuntimeResult<Self> {
        let runner_descriptors: Vec<_> = runners
            .iter()
            .map(|runner| runner.descriptor().clone())
            .collect();
        validate_runtime_descriptors(&load_plan, &runner_descriptors)?;
        let mut registry = RunnerRegistry::default();
        for runner in runners {
            registry.register(runner)?;
        }
        registry.freeze();
        let handler_bindings = HandlerBindingRegistry::from_load_plan(&load_plan);
        let generation_states =
            reload::generation_states_for_plan(&load_plan, PluginGenerationPhase::Active);
        Ok(Self {
            surfaces: load_plan.contract_surfaces.clone(),
            load_plan,
            handler_bindings,
            registry,
            draining_generations: Vec::new(),
            generation_states,
            tasks: TaskPool::default(),
            resources: ResourceManager::new(),
            states: StateStore::default(),
            events: EventLog::default(),
            traces: TraceLog::default(),
            current_step: 0,
        })
    }

    pub fn registry_snapshot(&self) -> RegistrySnapshot {
        RegistrySnapshot {
            generation: self.load_plan.registry_generation,
            frozen: true,
            runners: self.registry.descriptors(),
            handler_bindings: self.handler_bindings.all().to_vec(),
            surfaces: self.surfaces.clone(),
        }
    }

    pub fn handler_bindings(
        &self,
        protocol_id: &str,
    ) -> Vec<&mutsuki_runtime_contracts::HandlerBinding> {
        self.handler_bindings.query_protocol(protocol_id)
    }

    pub fn plugin_generation_states(&self) -> &[PluginGenerationState] {
        &self.generation_states
    }

    pub fn enqueue_task(&mut self, mut task: Task) -> String {
        if task.registry_generation == 0 {
            task.registry_generation = self.load_plan.registry_generation;
        }
        let deprecated_surface = surface_ids_for_task(&task)
            .into_iter()
            .find(|surface_id| self.is_surface_deprecated(surface_id));
        let task_id = self.tasks.enqueue(task);
        if let Some(surface_id) = deprecated_surface {
            let _ = self.tasks.reject_ready(
                &task_id,
                RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                    "runtime.result_router",
                    format!("surface.deprecated.{surface_id}"),
                ),
            );
        }
        self.events.record(
            RuntimeEventKind::Task,
            "task.enqueue",
            Some(task_id.clone()),
            BTreeMap::new(),
            None,
        );
        task_id
    }

    pub fn open_stream(
        &mut self,
        stream_id: &str,
        schema: &str,
        provider_id: &str,
        endpoint: &str,
    ) -> RuntimeResult<ResourceRef> {
        let surface_id = format!("stream:{stream_id}");
        self.ensure_surface_not_deprecated(&surface_id, "runtime.resource_manager")?;
        Ok(self
            .resources
            .create_stream_resource(stream_id, schema, provider_id, endpoint))
    }

    pub fn close_stream(&mut self, ref_id: &str) -> RuntimeResult<()> {
        self.resources.close_stream_resource(ref_id)?;
        Ok(())
    }

    pub fn register_surface_occupancy(
        &mut self,
        handle: SurfaceOccupancyHandle,
    ) -> RuntimeResult<()> {
        self.ensure_surface_not_deprecated(&handle.surface_id, "runtime.resource_manager")?;
        self.resources.register_surface_occupancy(handle)
    }

    pub fn release_surface_occupancy(
        &mut self,
        handle_id: &str,
    ) -> RuntimeResult<SurfaceOccupancyHandle> {
        self.resources.release_surface_occupancy(handle_id)
    }

    pub fn publish_raw_input(&mut self, task_id: &str, kind: &str, payload: Value) -> String {
        self.enqueue_task(Task::new(task_id, kind, payload))
    }

    pub fn tasks(&self) -> &TaskPool {
        &self.tasks
    }

    pub fn resources(&self) -> &ResourceManager {
        &self.resources
    }

    pub fn resources_mut(&mut self) -> &mut ResourceManager {
        &mut self.resources
    }

    pub fn state_value(&self, ref_id: &str) -> Option<&(u64, Value)> {
        self.states.get(ref_id)
    }

    pub fn events(&self) -> &[mutsuki_runtime_contracts::RuntimeEvent] {
        self.events.snapshot()
    }

    pub fn trace_spans(&self) -> &[mutsuki_runtime_contracts::TraceSpan] {
        self.traces.spans()
    }

    fn is_surface_deprecated(&self, surface_id: &str) -> bool {
        self.surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.deprecated)
    }

    fn ensure_surface_not_deprecated(&self, surface_id: &str, source: &str) -> RuntimeResult<()> {
        if self.is_surface_deprecated(surface_id) {
            return Err(RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                source,
                format!("surface.deprecated.{surface_id}"),
            )));
        }
        Ok(())
    }
}
