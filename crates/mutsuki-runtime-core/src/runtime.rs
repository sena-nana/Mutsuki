use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    CancelPolicy, ContractSurface, ExclusiveWriteLease, HandlerBinding, ResourceCellRef,
    ResourceLease, ResourceRef, RuntimeError, RuntimeEvent, RuntimeEventKind, RuntimeLoadPlan,
    ScalarValue, SurfaceOccupancyHandle, Task, TaskHandle, TaskOutcome, TaskStatus,
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
pub use runner_loop::{RunnerCompletion, RunnerDispatch};

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
        let authorized = self
            .load_plan
            .plugins
            .iter()
            .flat_map(|plugin| plugin.provides.handler_bindings.iter())
            .any(|declared| declared == &binding);
        if !authorized {
            return Err(RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
                "runtime.handler_binding",
                format!("handler_binding.{}", binding.binding_id),
            )));
        }
        self.handler_bindings.register_authorized(binding);
        Ok(())
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

    pub fn create_blob_resource(&mut self, schema: &str, bytes: Vec<u8>) -> ResourceRef {
        self.resources.create_blob_resource(schema, bytes)
    }

    pub fn create_mmap_resource(
        &mut self,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.resources.create_mmap_resource(schema, bytes)
    }

    pub fn open_resource(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.resources.open_resource(ref_id)
    }

    pub fn read_resource(&self, ref_id: &str) -> RuntimeResult<Vec<u8>> {
        self.resources.read_resource_by_id(ref_id)
    }

    pub fn map_resource(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
        self.resources.map_resource(ref_id)
    }

    pub fn lock_resource(
        &mut self,
        ref_id: &str,
        owner: &str,
        expires_at_step: Option<u64>,
    ) -> RuntimeResult<ExclusiveWriteLease> {
        self.resources
            .acquire_write_lease(ref_id, owner, expires_at_step)
    }

    pub fn write_resource(
        &mut self,
        lease: &ExclusiveWriteLease,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.resources
            .write_with_lease(lease, bytes, self.current_step)
    }

    pub fn create_resource_cell(
        &mut self,
        cell_id: &str,
        resource_kind: &str,
        owner_plugin_id: &str,
        schema: &str,
        reload_policy: &str,
    ) -> ResourceCellRef {
        self.resources.create_resource_cell(
            cell_id,
            resource_kind,
            owner_plugin_id,
            schema,
            reload_policy,
        )
    }

    pub fn acquire_resource_lease(
        &mut self,
        cell_id: &str,
        borrower_task_id: &str,
        borrower_executor_id: &str,
        mode: &str,
        expires_at_step: Option<u64>,
    ) -> RuntimeResult<ResourceLease> {
        self.resources.acquire_resource_lease(
            cell_id,
            borrower_task_id,
            borrower_executor_id,
            mode,
            expires_at_step,
        )
    }

    pub fn release_resource_lease(&mut self, lease: &ResourceLease) -> RuntimeResult<()> {
        self.resources.release_resource_lease(lease)
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

    pub fn submit_task(&mut self, task: Task) -> String {
        self.enqueue_task(task)
    }

    pub fn submit_task_handle(&mut self, task: Task) -> RuntimeResult<TaskHandle> {
        let task_id = self.enqueue_task(task);
        self.task_handle(&task_id)
    }

    pub fn submit_targeted_task(
        &mut self,
        task_id: &str,
        binding_id: &str,
        payload: Value,
    ) -> RuntimeResult<String> {
        let binding = self
            .handler_bindings
            .all()
            .iter()
            .find(|binding| binding.binding_id == binding_id)
            .ok_or_else(|| {
                RuntimeFailure::new(RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
                    "runtime.handler_binding",
                    format!("handler_binding.{binding_id}"),
                ))
            })?;
        let mut task = Task::new(task_id, &binding.target_protocol_id, payload);
        task.target_binding_id = Some(binding.binding_id.clone());
        task.runner_hint = binding.target_runner_hint.clone();
        Ok(self.enqueue_task(task))
    }

    pub fn submit_targeted_task_handle(
        &mut self,
        task_id: &str,
        binding_id: &str,
        payload: Value,
    ) -> RuntimeResult<TaskHandle> {
        let task_id = self.submit_targeted_task(task_id, binding_id, payload)?;
        self.task_handle(&task_id)
    }

    pub fn task_handle(&self, task_id: &str) -> RuntimeResult<TaskHandle> {
        let record = self.tasks.get(task_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_TASK_NOT_FOUND,
                "runtime.task",
                format!("task.handle.{task_id}"),
            ))
        })?;
        Ok(TaskHandle {
            task_id: record.task.task_id.clone(),
            protocol_id: record.task.protocol_id.clone(),
            target_binding_id: record.task.target_binding_id.clone(),
            cancel_policy: CancelPolicy::Cascade,
            trace_id: record.task.trace_id.clone(),
            correlation_id: record.task.correlation_id.clone(),
        })
    }

    pub fn task_status(&self, task_id: &str) -> Option<mutsuki_runtime_contracts::TaskStatus> {
        self.tasks.get(task_id).map(|record| record.status.clone())
    }

    pub fn task_handle_status(&self, handle: &TaskHandle) -> Option<TaskStatus> {
        self.task_status(&handle.task_id)
    }

    pub fn task_result(&self, task_id: &str) -> Option<TaskResultSnapshot> {
        self.tasks.get(task_id).map(|record| TaskResultSnapshot {
            task_id: record.task.task_id.clone(),
            status: record.status.clone(),
            output_ref: record.task.output_ref.clone(),
            continuation_ref: record.task.continuation_ref.clone(),
            failure: record.failure.clone(),
        })
    }

    pub fn task_handle_result(&self, handle: &TaskHandle) -> Option<TaskResultSnapshot> {
        self.task_result(&handle.task_id)
    }

    pub fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
        let record = self.tasks.get(task_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_TASK_NOT_FOUND,
                "runtime.task",
                format!("task.outcome.{task_id}"),
            ))
        })?;
        Ok(match record.status {
            TaskStatus::Completed => Some(TaskOutcome::Completed {
                task_id: record.task.task_id.clone(),
                output_ref: record.task.output_ref.clone(),
            }),
            TaskStatus::Failed => Some(TaskOutcome::Failed {
                task_id: record.task.task_id.clone(),
                error: record.failure.clone().unwrap_or_else(|| {
                    RuntimeError::new(
                        "runner.failed",
                        "runtime.task",
                        format!("task.outcome.{task_id}"),
                    )
                }),
            }),
            TaskStatus::Cancelled => Some(TaskOutcome::Cancelled {
                task_id: record.task.task_id.clone(),
                reason: record.failure.as_ref().map(|error| error.route.clone()),
            }),
            TaskStatus::Expired => Some(TaskOutcome::Expired {
                task_id: record.task.task_id.clone(),
                reason: record.failure.as_ref().map(|error| error.route.clone()),
            }),
            TaskStatus::DeadLetter => Some(TaskOutcome::DeadLetter {
                task_id: record.task.task_id.clone(),
                reason: record.failure.as_ref().map(|error| error.route.clone()),
            }),
            _ => None,
        })
    }

    pub fn task_handle_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.task_outcome(&handle.task_id)
    }

    pub fn task_events(&self, task_id: &str) -> Vec<&RuntimeEvent> {
        self.events
            .snapshot()
            .iter()
            .filter(|event| event.subject_id.as_deref() == Some(task_id))
            .collect()
    }

    pub fn task_handle_events(&self, handle: &TaskHandle) -> Vec<&RuntimeEvent> {
        self.task_events(&handle.task_id)
    }

    pub fn events_after(&self, sequence: u64) -> Vec<&RuntimeEvent> {
        self.events
            .snapshot()
            .iter()
            .filter(|event| event.sequence > sequence)
            .collect()
    }

    pub fn cancel_task(&mut self, task_id: &str) -> RuntimeResult<()> {
        let awaits = self.tasks.awaits_for_parent(task_id);
        if awaits
            .iter()
            .any(|task_await| task_await.cancel_policy != CancelPolicy::Cascade)
        {
            return Err(RuntimeFailure::new(RuntimeError::new(
                "task.cancel_policy_unsupported",
                "runtime.task",
                format!("task.cancel.{task_id}"),
            )));
        }
        self.tasks.cancel_by_core(task_id)?;
        self.record_task_terminal_event(task_id, "task.cancelled", None);
        for task_await in awaits {
            if matches!(
                self.task_status(&task_await.child.task_id),
                Some(
                    TaskStatus::Created
                        | TaskStatus::Ready
                        | TaskStatus::Running
                        | TaskStatus::Waiting
                        | TaskStatus::Blocked
                )
            ) {
                self.cancel_task(&task_await.child.task_id)?;
            }
        }
        self.wake_tasks_waiting_on(task_id)?;
        Ok(())
    }

    pub fn cancel_task_handle(&mut self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.cancel_task(&handle.task_id)
    }

    pub fn wake_task(&mut self, task_id: &str) -> RuntimeResult<()> {
        self.tasks.wake(task_id)
    }

    pub fn wake_task_handle(&mut self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.wake_task(&handle.task_id)
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

    pub(crate) fn ensure_task_can_suspend(&self, task_id: &str) -> RuntimeResult<()> {
        let active_mutable = self.resources.active_mutable_lease_routes_for_task(task_id);
        if !active_mutable.is_empty() {
            let mut error = RuntimeError::new(
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
