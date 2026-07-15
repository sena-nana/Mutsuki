use std::collections::{BTreeMap, VecDeque};

use mutsuki_runtime_contracts::{
    ContractSurface, ERR_RUNTIME_ABORTED, ERR_RUNTIME_NOT_ACCEPTING, HandlerBinding,
    ObservabilityPage, ObservabilityProfile, RuntimeError, RuntimeEvent, RuntimeEventKind,
    RuntimeLoadPlan, ScalarValue, TaskStatus, TraceSpan,
};
use serde_json::Value;

use crate::logs::{EventLog, TraceLog};
use crate::registry::{
    HandlerBindingRegistry, PluginGenerationPhase, PluginGenerationState, RegistrySnapshot,
    RunnerRegistry, validate_runtime_descriptors,
};
use crate::runner::Runner;
use crate::state_store::StateStore;
use crate::{ResourceManager, RuntimeFailure, RuntimeResult, TaskPool, TaskPoolStatistics};

mod reload;
mod resource_api;
mod runner_loop;
mod scheduler;
mod task_api;

pub use reload::{InvocationPollution, RunningInvocationDisposition};
pub use runner_loop::{RunnerCompletion, RunnerDispatch};
pub use scheduler::{DispatchBudget, LaneBudget, ScheduleDecision};

#[derive(Clone, Debug, PartialEq)]
pub struct TaskResultSnapshot {
    pub task_id: String,
    pub status: mutsuki_runtime_contracts::TaskStatus,
    pub output_ref: Option<String>,
    pub continuation_ref: Option<String>,
    pub failure: Option<RuntimeError>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeStopState {
    #[default]
    Running,
    Draining,
    Aborted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStatistics {
    pub tasks: TaskPoolStatistics,
    pub retained_events: usize,
    pub dropped_events: u64,
    pub retained_traces: usize,
    pub dropped_traces: u64,
    pub scheduler_decisions: u64,
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
    scheduler_decisions: u64,
    current_step: u64,
    stop_state: RuntimeStopState,
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
        let events = EventLog::with_profile(load_plan.observability.events.clone());
        let traces = TraceLog::with_profile(load_plan.observability.traces.clone());
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
            events,
            traces,
            scheduler_decisions: 0,
            current_step: 0,
            stop_state: RuntimeStopState::Running,
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
        Err(crate::runtime_failure(
            mutsuki_runtime_contracts::ERR_REGISTRY_FROZEN,
            "runtime.handler_binding",
            format!("handler_binding.{}", binding.binding_id),
        ))
    }

    pub fn plugin_generation_states(&self) -> &[PluginGenerationState] {
        &self.generation_states
    }

    pub fn current_step(&self) -> u64 {
        self.current_step
    }

    pub fn stop_state(&self) -> RuntimeStopState {
        self.stop_state
    }

    pub fn begin_drain(&mut self) -> RuntimeResult<RuntimeStopState> {
        match self.stop_state {
            RuntimeStopState::Running => {
                self.stop_state = RuntimeStopState::Draining;
                self.events.record(
                    RuntimeEventKind::Lifecycle,
                    "runtime.drain_started",
                    None,
                    BTreeMap::new(),
                    None,
                );
                Ok(self.stop_state)
            }
            RuntimeStopState::Draining => Ok(self.stop_state),
            RuntimeStopState::Aborted => Err(crate::runtime_failure(
                ERR_RUNTIME_ABORTED,
                "runtime.lifecycle",
                "runtime.drain.aborted",
            )),
        }
    }

    pub fn abort(&mut self, reason: impl Into<String>) -> RuntimeResult<usize> {
        if self.stop_state == RuntimeStopState::Aborted {
            return Ok(0);
        }
        let reason = reason.into();
        self.stop_state = RuntimeStopState::Aborted;
        let mut failure =
            crate::runtime_error(ERR_RUNTIME_ABORTED, "runtime.lifecycle", "runtime.abort");
        failure
            .evidence
            .insert("reason".into(), ScalarValue::String(reason.clone()));
        let aborted = self.tasks.abort_all(self.current_step, failure.clone());
        for task_id in &aborted {
            self.record_task_terminal_event(task_id, "task.cancelled", Some(failure.clone()));
        }
        self.events.record(
            RuntimeEventKind::Lifecycle,
            "runtime.aborted",
            None,
            BTreeMap::from([
                ("reason".into(), ScalarValue::String(reason)),
                (
                    "cancelled_tasks".into(),
                    ScalarValue::Int(aborted.len() as i64),
                ),
            ]),
            Some(failure),
        );
        Ok(aborted.len())
    }

    pub fn is_drained(&self) -> bool {
        let statistics = self.tasks.statistics();
        self.stop_state == RuntimeStopState::Draining
            && statistics.ready == 0
            && statistics.running == 0
            && statistics.waiting == 0
            && statistics.blocked == 0
    }

    pub fn configure_event_capacity(&mut self, capacity: usize) {
        self.load_plan.observability.events.capacity = capacity;
        self.events.set_capacity(capacity);
    }

    pub fn configure_observability(&mut self, profile: ObservabilityProfile) {
        self.events.configure(profile.events.clone());
        self.traces.configure(profile.traces.clone());
        self.load_plan.observability = profile;
    }

    pub fn statistics(&self) -> RuntimeStatistics {
        RuntimeStatistics {
            tasks: self.tasks.statistics(),
            retained_events: self.events.retained(),
            dropped_events: self.events.dropped(),
            retained_traces: self.traces.retained(),
            dropped_traces: self.traces.dropped(),
            scheduler_decisions: self.scheduler_decisions,
        }
    }

    pub(crate) fn ensure_accepting_external_tasks(&self) -> RuntimeResult<()> {
        if self.stop_state == RuntimeStopState::Running {
            return Ok(());
        }
        Err(crate::runtime_failure(
            match self.stop_state {
                RuntimeStopState::Aborted => ERR_RUNTIME_ABORTED,
                RuntimeStopState::Draining => ERR_RUNTIME_NOT_ACCEPTING,
                RuntimeStopState::Running => unreachable!(),
            },
            "runtime.lifecycle",
            "runtime.submit",
        ))
    }

    pub(crate) fn ensure_not_aborted(&self) -> RuntimeResult<()> {
        if self.stop_state != RuntimeStopState::Aborted {
            return Ok(());
        }
        Err(crate::runtime_failure(
            ERR_RUNTIME_ABORTED,
            "runtime.lifecycle",
            "runtime.execute.aborted",
        ))
    }

    #[cfg(test)]
    pub(crate) fn register_runner(&mut self, runner: Box<dyn Runner>) -> RuntimeResult<()> {
        self.registry.register(runner)
    }

    #[cfg(test)]
    pub(crate) fn unregister_runner(&mut self, runner_id: &str) -> RuntimeResult<()> {
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

    pub fn events(&self) -> &VecDeque<RuntimeEvent> {
        self.events.snapshot()
    }

    pub fn trace_spans(&self) -> &VecDeque<TraceSpan> {
        self.traces.spans()
    }

    pub fn trace_spans_after(&self, sequence: u64, limit: usize) -> ObservabilityPage<TraceSpan> {
        self.traces.page_after(sequence, limit)
    }

    fn is_surface_deprecated(&self, surface_id: &str) -> bool {
        self.surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.deprecated)
    }

    fn ensure_surface_not_deprecated(&self, surface_id: &str, source: &str) -> RuntimeResult<()> {
        if self.is_surface_deprecated(surface_id) {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                source,
                format!("surface.deprecated.{surface_id}"),
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
            let mut error = crate::runtime_error(
                "resource.lease_cross_await",
                "runtime.resource_manager",
                format!("task.await.{task_id}"),
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
                self.tasks
                    .wake(&task_await.parent_task_id, self.current_step)?;
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

    pub(crate) fn wake_due_tasks(&mut self) -> usize {
        let due_tasks = self.tasks.wake_due_tasks(self.current_step);
        for (task_id, ready_at_step) in &due_tasks {
            self.events.record(
                RuntimeEventKind::Task,
                "task.wake",
                Some(task_id.clone()),
                BTreeMap::from([
                    ("reason".into(), ScalarValue::String("ready_at_step".into())),
                    (
                        "ready_at_step".into(),
                        ScalarValue::Int(*ready_at_step as i64),
                    ),
                ]),
                None,
            );
        }
        due_tasks.len()
    }

    pub(crate) fn reject_stale_ready_tasks(&mut self) -> RuntimeResult<usize> {
        let stale_tasks: Vec<_> = self
            .tasks
            .stale_expectation_task_ids()
            .into_iter()
            .filter_map(|task_id| {
                let record = self.tasks.get(&task_id)?;
                self.states
                    .validate_expectations(
                        &record.task.expected_versions,
                        format!("task.precondition.{task_id}"),
                    )
                    .err()
                    .map(|failure| (task_id, failure.error().clone()))
            })
            .collect();
        let mut rejected = 0;
        for (task_id, failure) in stale_tasks {
            self.tasks.reject_ready(&task_id, failure.clone())?;
            self.record_task_terminal_event(&task_id, "task.failed", Some(failure));
            self.wake_tasks_waiting_on(&task_id)?;
            rejected += 1;
        }
        Ok(rejected)
    }
}
