use std::collections::{BTreeMap, BTreeSet};

use mutsuki_runtime_contracts::{
    DomainEvent, ERR_RUNNER_PURITY_VIOLATION, ERR_TASK_DUPLICATE, ERR_TASK_NOT_FOUND,
    EffectRequest, ResourceRef, RunnerDescriptor, RunnerPurity, RunnerResult, RunnerStatus,
    RuntimeEventKind, ScalarValue, StateDelta, Task, TaskAwait, TaskLease, TaskStatus, ValueRef,
    VersionExpectation,
};

use crate::RuntimeResult;

use super::CoreRuntime;

struct ResultOutputs {
    deltas: Vec<StateDelta>,
    events: Vec<DomainEvent>,
    tasks: Vec<Task>,
    effects: Vec<EffectRequest>,
    values: Vec<ValueRef>,
    resources: Vec<ResourceRef>,
}

impl CoreRuntime {
    pub(super) fn route_result(
        &mut self,
        runner: &RunnerDescriptor,
        lease: &TaskLease,
        result: RunnerResult,
    ) -> RuntimeResult<usize> {
        let RunnerResult {
            task_id,
            deltas,
            events,
            tasks,
            effects,
            values,
            resources,
            task_await,
            status,
        } = result;
        self.tasks
            .ensure_active_lease(&task_id, lease, self.current_step, "route")?;
        self.validate_waiting_result(&task_id, &status, task_await.as_ref(), &tasks)?;
        validate_continue_outputs(
            &task_id, &status, &deltas, &events, &tasks, &effects, &values, &resources,
        )?;
        self.route_result_outputs(
            runner,
            &task_id,
            ResultOutputs {
                deltas,
                events,
                tasks,
                effects,
                values,
                resources,
            },
        )?;
        self.commit_result_status(runner, lease, task_id, status, task_await)
    }

    fn validate_waiting_result(
        &mut self,
        task_id: &str,
        status: &RunnerStatus,
        task_await: Option<&TaskAwait>,
        output_tasks: &[Task],
    ) -> RuntimeResult<()> {
        if task_await.is_some() && !matches!(status, RunnerStatus::Waiting) {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                "runtime.result_router",
                format!("task.await.status.{task_id}"),
            ));
        }
        if matches!(status, RunnerStatus::Waiting)
            && let Some(task_await) = task_await
        {
            self.ensure_task_can_suspend(task_id)?;
            if task_await.parent_task_id != task_id {
                return Err(crate::runtime_failure(
                    mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                    "runtime.result_router",
                    format!("task.await.parent.{task_id}"),
                ));
            }
            self.validate_await_child_descriptor(task_id, task_await, output_tasks)?;
        }
        Ok(())
    }

    fn validate_await_child_descriptor(
        &self,
        parent_task_id: &str,
        task_await: &TaskAwait,
        output_tasks: &[Task],
    ) -> RuntimeResult<()> {
        let parent = self.tasks.get(parent_task_id).ok_or_else(|| {
            crate::runtime_failure(
                ERR_TASK_NOT_FOUND,
                "runtime.result_router",
                format!("task.await.parent.{parent_task_id}"),
            )
        })?;
        if let Some(output_task) = output_tasks
            .iter()
            .find(|task| task.task_id == task_await.child.task_id)
        {
            return validate_await_child_task(
                parent_task_id,
                &parent.task,
                task_await,
                output_task,
            );
        }
        let child = self.tasks.get(&task_await.child.task_id).ok_or_else(|| {
            crate::runtime_failure(
                ERR_TASK_NOT_FOUND,
                "runtime.result_router",
                format!("task.await.child.{}", task_await.child.task_id),
            )
        })?;
        if is_terminal_task_status(&child.status) {
            return Err(crate::runtime_failure(
                mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                "runtime.result_router",
                format!("task.await.child_terminal.{}", task_await.child.task_id),
            ));
        }
        validate_await_child_task(parent_task_id, &parent.task, task_await, &child.task)
    }

    fn route_result_outputs(
        &mut self,
        runner: &RunnerDescriptor,
        task_id: &str,
        outputs: ResultOutputs,
    ) -> RuntimeResult<()> {
        let generation = self.load_plan.registry_generation;
        if runner.purity != RunnerPurity::Pure
            && (!outputs.deltas.is_empty() || !outputs.effects.is_empty())
        {
            return Err(crate::runtime_failure(
                ERR_RUNNER_PURITY_VIOLATION,
                "runtime.result_router",
                format!("runner.{}.core_derivation", runner.runner_id),
            ));
        }
        if runner.purity == RunnerPurity::Effectful && !runner.runner_id.starts_with("effect.") {
            return Err(crate::runtime_failure(
                ERR_RUNNER_PURITY_VIOLATION,
                "runtime.result_router",
                format!("runner.{}", runner.runner_id),
            ));
        }
        let pending_tasks = pending_output_tasks(task_id, generation, &runner.purity, &outputs);
        self.ensure_output_task_ids_available(&pending_tasks)?;
        for value_ref in outputs.values {
            self.events.record(
                RuntimeEventKind::Resource,
                "value.lineage",
                Some(task_id.to_string()),
                ref_lineage_attrs(value_ref.ref_id, value_ref.schema, value_ref.generation),
                None,
            );
        }
        for resource_ref in outputs.resources {
            self.events.record(
                RuntimeEventKind::Resource,
                "resource.lineage",
                Some(task_id.to_string()),
                ref_lineage_attrs(
                    resource_ref.ref_id,
                    resource_ref.schema,
                    resource_ref.generation,
                ),
                None,
            );
        }
        for task in pending_tasks {
            self.enqueue_task(task)?;
        }
        Ok(())
    }

    fn ensure_output_task_ids_available(&self, pending_tasks: &[Task]) -> RuntimeResult<()> {
        let mut seen = BTreeSet::new();
        for task in pending_tasks {
            if !seen.insert(task.task_id.clone()) || self.tasks.get(&task.task_id).is_some() {
                return Err(crate::runtime_failure(
                    ERR_TASK_DUPLICATE,
                    "runtime.result_router",
                    format!("task.enqueue.{}", task.task_id),
                ));
            }
        }
        Ok(())
    }

    fn commit_result_status(
        &mut self,
        runner: &RunnerDescriptor,
        lease: &TaskLease,
        task_id: String,
        status: RunnerStatus,
        task_await: Option<TaskAwait>,
    ) -> RuntimeResult<usize> {
        match status {
            RunnerStatus::Completed => {
                self.tasks.complete(lease, self.current_step)?;
                self.record_task_terminal_event(&task_id, "task.completed", None);
                self.wake_tasks_waiting_on(&task_id)?;
                return Ok(1);
            }
            RunnerStatus::Waiting => {
                if let Some(task_await) = task_await {
                    self.tasks
                        .wait_on_task(lease, self.current_step, task_await)?;
                } else {
                    self.tasks.wait(lease, self.current_step, None)?;
                }
                self.events.record(
                    RuntimeEventKind::Task,
                    "task.progress",
                    Some(task_id.clone()),
                    BTreeMap::from([("status".into(), ScalarValue::String("waiting".into()))]),
                    None,
                );
            }
            RunnerStatus::Blocked => {
                self.tasks.block(lease, self.current_step)?;
                self.events.record(
                    RuntimeEventKind::Task,
                    "task.progress",
                    Some(task_id.clone()),
                    BTreeMap::from([("status".into(), ScalarValue::String("blocked".into()))]),
                    None,
                );
            }
            RunnerStatus::Failed => {
                let failure = crate::runtime_error(
                    "runner.failed",
                    "runtime.result_router",
                    format!("runner.{}", runner.runner_id),
                );
                self.tasks.fail(lease, self.current_step, failure.clone())?;
                self.record_task_terminal_event(&task_id, "task.failed", Some(failure));
                self.wake_tasks_waiting_on(&task_id)?;
                return Ok(1);
            }
            RunnerStatus::Cancelled => {
                self.tasks.cancel_task(lease, self.current_step)?;
                self.record_task_terminal_event(&task_id, "task.cancelled", None);
                self.wake_tasks_waiting_on(&task_id)?;
                return Ok(1);
            }
            RunnerStatus::Continue => {}
        }
        Ok(0)
    }
}

fn pending_output_tasks(
    source_task_id: &str,
    generation: u64,
    runner_purity: &RunnerPurity,
    outputs: &ResultOutputs,
) -> Vec<Task> {
    let mut tasks = Vec::new();
    if *runner_purity == RunnerPurity::Pure {
        tasks.extend(
            outputs
                .deltas
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, delta)| commit_task(source_task_id, index, delta, generation)),
        );
        tasks.extend(
            outputs
                .effects
                .iter()
                .cloned()
                .map(|effect| effect_task(source_task_id, effect, generation)),
        );
    }
    tasks.extend(
        outputs
            .events
            .iter()
            .cloned()
            .map(|event| event_task(source_task_id, event, generation)),
    );
    tasks.extend(outputs.tasks.iter().cloned());
    tasks
}

// The explicit output slices mirror RunnerResult and keep validation allocation-free.
#[allow(clippy::too_many_arguments)]
fn validate_continue_outputs(
    task_id: &str,
    status: &RunnerStatus,
    deltas: &[StateDelta],
    events: &[DomainEvent],
    tasks: &[Task],
    effects: &[EffectRequest],
    values: &[ValueRef],
    resources: &[ResourceRef],
) -> RuntimeResult<()> {
    if !matches!(status, RunnerStatus::Continue) {
        return Ok(());
    }
    if deltas.is_empty()
        && events.is_empty()
        && tasks.is_empty()
        && effects.is_empty()
        && values.is_empty()
        && resources.is_empty()
    {
        return Ok(());
    }
    Err(crate::runtime_failure(
        mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
        "runtime.result_router",
        format!("task.continue.outputs.{task_id}"),
    ))
}

fn validate_await_child_task(
    parent_task_id: &str,
    parent_task: &Task,
    task_await: &TaskAwait,
    child_task: &Task,
) -> RuntimeResult<()> {
    let child = &task_await.child;
    let child_matches_handle = child.protocol_id == child_task.protocol_id
        && child.target_binding_id == child_task.target_binding_id
        && child.trace_id == child_task.trace_id
        && child.correlation_id == child_task.correlation_id;
    let child_inherits_parent_context = child_task.trace_id == parent_task.trace_id
        && child_task.correlation_id == parent_task.correlation_id;
    if child_matches_handle && child_inherits_parent_context {
        return Ok(());
    }
    Err(crate::runtime_failure(
        mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
        "runtime.result_router",
        format!("task.await.child_descriptor.{parent_task_id}"),
    ))
}

fn is_terminal_task_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed
            | TaskStatus::Failed
            | TaskStatus::Cancelled
            | TaskStatus::Expired
            | TaskStatus::DeadLetter
    )
}

fn commit_task(source_task_id: &str, index: usize, delta: StateDelta, generation: u64) -> Task {
    let mut task = Task::new(
        format!("{source_task_id}:commit:{index}"),
        "core.commit",
        serde_json::to_value(delta).expect("StateDelta serializes"),
    );
    task.registry_generation = generation;
    task
}

fn event_task(source_task_id: &str, event: DomainEvent, generation: u64) -> Task {
    let mut task = Task::new(
        format!("{source_task_id}:event:{}", event.event_id),
        "core.event.append",
        serde_json::to_value(event).expect("DomainEvent serializes"),
    );
    task.registry_generation = generation;
    task
}

fn effect_task(source_task_id: &str, effect: EffectRequest, generation: u64) -> Task {
    let expected_versions = effect
        .preconditions
        .iter()
        .map(|precondition| VersionExpectation {
            ref_id: precondition.ref_id.clone(),
            expected_version: precondition.expected_version,
        })
        .collect();
    let mut task = Task::new(
        format!("{source_task_id}:effect:{}", effect.effect_id),
        effect.kind.clone(),
        serde_json::to_value(effect).expect("EffectRequest serializes"),
    );
    task.registry_generation = generation;
    task.expected_versions = expected_versions;
    task
}

fn ref_lineage_attrs(
    ref_id: String,
    schema: String,
    generation: u64,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert("ref_id".into(), ScalarValue::String(ref_id));
    attrs.insert("schema".into(), ScalarValue::String(schema));
    attrs.insert("generation".into(), ScalarValue::Int(generation as i64));
    attrs
}
