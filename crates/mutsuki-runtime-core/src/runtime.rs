use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ContractSurface, HandlerBinding, RuntimeError, RuntimeEventKind, RuntimeLoadPlan, ScalarValue,
    TaskStatus,
};
use serde_json::Value;

use crate::logs::{EventLog, TraceLog};
use crate::registry::{
    HandlerBindingRegistry, PluginGenerationPhase, PluginGenerationState, RegistrySnapshot,
    RunnerRegistry, validate_runtime_descriptors,
};
use crate::runner::Runner;
use crate::state_store::StateStore;
use crate::{ResourceManager, RuntimeFailure, RuntimeResult, TaskPool};

mod reload;
mod resource_api;
mod runner_loop;
mod scheduler;
mod task_api;

pub use reload::{InvocationPollution, RunningInvocationDisposition};
pub use runner_loop::{RunnerCompletion, RunnerDispatch};
pub use scheduler::ScheduleDecision;

#[derive(Clone, Debug, PartialEq)]
pub struct TaskResultSnapshot {
    pub task_id: String,
    pub status: mutsuki_runtime_contracts::TaskStatus,
    pub output_ref: Option<String>,
    pub continuation_ref: Option<String>,
    pub failure: Option<RuntimeError>,
}

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

    pub fn register_handler_binding(&mut self, binding: HandlerBinding) -> RuntimeResult<()> {
        Err(runtime_failure!(
            mutsuki_runtime_contracts::ERR_REGISTRY_FROZEN,
            "runtime.handler_binding",
            format!("handler_binding.{}", binding.binding_id)
        ))
    }

    pub fn plugin_generation_states(&self) -> &[PluginGenerationState] {
        &self.generation_states
    }

    pub fn current_step(&self) -> u64 {
        self.current_step
    }

    pub fn register_runner(&mut self, runner: Box<dyn Runner>) -> RuntimeResult<()> {
        self.registry.register(runner)
    }

    pub fn unregister_runner(&mut self, runner_id: &str) -> RuntimeResult<()> {
        self.registry.unregister(runner_id)?;
        Ok(())
    }

    pub fn runner_heartbeat(
        &mut self,
        runner_id: &str,
        executor_id: &str,
    ) -> RuntimeResult<crate::registry::RunnerHeartbeat> {
        self.registry
            .heartbeat(runner_id, executor_id, self.current_step)
    }

    pub fn runner_capability(
        &mut self,
        runner_id: &str,
        protocol_ids: Vec<String>,
        capacity: usize,
    ) -> RuntimeResult<crate::registry::RunnerCapabilityDeclaration> {
        self.registry
            .declare_capability(runner_id, protocol_ids, capacity)
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
            return Err(runtime_failure!(
                mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                source,
                format!("surface.deprecated.{surface_id}")
            ));
        }
        Ok(())
    }

    fn ensure_resource_surfaces_not_deprecated(
        &self,
        schema: &str,
        provider_id: Option<&str>,
        source: &str,
    ) -> RuntimeResult<()> {
        self.ensure_surface_not_deprecated(schema, source)?;
        self.ensure_surface_not_deprecated(&format!("resource_schema:{schema}"), source)?;
        if let Some(provider_id) = provider_id {
            self.ensure_surface_not_deprecated(provider_id, source)?;
            self.ensure_surface_not_deprecated(
                &format!("resource_provider:{provider_id}"),
                source,
            )?;
        }
        Ok(())
    }

    pub(crate) fn ensure_task_can_suspend(&self, task_id: &str) -> RuntimeResult<()> {
        let active_mutable = self.resources.active_mutable_lease_routes_for_task(task_id);
        if !active_mutable.is_empty() {
            let mut error = runtime_error!(
                "resource.lease_cross_await",
                "runtime.resource_manager",
                format!("task.await.{task_id}")
            );
            error.evidence.insert(
                "active_mutable_leases".into(),
                ScalarValue::String(active_mutable.join(",")),
            );
            return Err(RuntimeFailure::new(error));
        }
        Ok(())
    }

    pub(crate) fn record_task_terminal_event(
        &mut self,
        task_id: &str,
        name: &str,
        error: Option<RuntimeError>,
    ) {
        self.events.record(
            RuntimeEventKind::Task,
            name,
            Some(task_id.to_string()),
            BTreeMap::new(),
            error,
        );
    }

    pub(crate) fn wake_tasks_waiting_on(&mut self, child_task_id: &str) -> RuntimeResult<usize> {
        let waits = self.tasks.take_waits_for_child(child_task_id);
        let mut woken = 0;
        for task_await in waits {
            if matches!(
                self.task_status(&task_await.parent_task_id),
                Some(TaskStatus::Waiting | TaskStatus::Blocked)
            ) {
                self.tasks.wake(&task_await.parent_task_id)?;
                self.events.record(
                    RuntimeEventKind::Task,
                    "task.wake",
                    Some(task_await.parent_task_id),
                    BTreeMap::from([(
                        "child_task_id".into(),
                        ScalarValue::String(child_task_id.to_string()),
                    )]),
                    None,
                );
                woken += 1;
            }
        }
        Ok(woken)
    }
}
