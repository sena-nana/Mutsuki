use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    DomainEvent, ERR_RUNNER_PURITY_VIOLATION, EffectRequest, ResourceRef, RunnerDescriptor,
    RunnerPurity, RunnerResult, RunnerStatus, RuntimeEventKind, ScalarValue, StateDelta, Task,
    TaskAwait, TaskLease, ValueRef,
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
        self.validate_waiting_result(&task_id, &status, task_await.as_ref())?;
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
    ) -> RuntimeResult<()> {
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
        }
        Ok(())
    }

    fn route_result_outputs(
        &mut self,
        runner: &RunnerDescriptor,
        task_id: &str,
        outputs: ResultOutputs,
    ) -> RuntimeResult<()> {
        let generation = self.load_plan.registry_generation;
        if runner.purity == RunnerPurity::Pure {
            for delta in outputs.deltas {
                self.enqueue_task(commit_task(task_id, delta, generation));
            }
            for effect in outputs.effects {
                self.enqueue_task(effect_task(task_id, effect, generation));
            }
        } else if runner.purity == RunnerPurity::Effectful
            && !runner.runner_id.starts_with("effect.")
        {
            return Err(crate::runtime_failure(
                ERR_RUNNER_PURITY_VIOLATION,
                "runtime.result_router",
                format!("runner.{}", runner.runner_id),
            ));
        }
        for event in outputs.events {
            self.enqueue_task(event_task(task_id, event, generation));
        }
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
        for task in outputs.tasks {
            self.enqueue_task(task);
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
            }
            RunnerStatus::Blocked => {
                self.tasks.block(lease, self.current_step)?;
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

fn commit_task(source_task_id: &str, delta: StateDelta, generation: u64) -> Task {
    let mut task = Task::new(
        format!("{source_task_id}:commit"),
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
    let mut task = Task::new(
        format!("{source_task_id}:effect:{}", effect.effect_id),
        effect.kind.clone(),
        serde_json::to_value(effect).expect("EffectRequest serializes"),
    );
    task.registry_generation = generation;
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
