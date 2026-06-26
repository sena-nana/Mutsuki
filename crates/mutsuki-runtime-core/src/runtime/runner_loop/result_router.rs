use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    DomainEvent, ERR_RUNNER_PURITY_VIOLATION, EffectRequest, RunnerDescriptor, RunnerPurity,
    RunnerResult, RunnerStatus, RuntimeError, RuntimeEventKind, ScalarValue, StateDelta, Task,
};

use crate::{RuntimeFailure, RuntimeResult};

use super::CoreRuntime;

impl CoreRuntime {
    pub(super) fn route_result(
        &mut self,
        runner: &RunnerDescriptor,
        result: RunnerResult,
    ) -> RuntimeResult<usize> {
        if runner.purity == RunnerPurity::Pure {
            for delta in result.deltas {
                self.enqueue_task(commit_task(
                    &result.task_id,
                    delta,
                    self.load_plan.registry_generation,
                ));
            }
            for effect in result.effects {
                self.enqueue_task(effect_task(
                    &result.task_id,
                    effect,
                    self.load_plan.registry_generation,
                ));
            }
        } else if runner.purity == RunnerPurity::Effectful
            && !runner.runner_id.starts_with("effect.")
        {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNNER_PURITY_VIOLATION,
                "runtime.result_router",
                format!("runner.{}", runner.runner_id),
            )));
        }
        for event in result.events {
            self.enqueue_task(event_task(
                &result.task_id,
                event,
                self.load_plan.registry_generation,
            ));
        }
        for value_ref in result.values {
            self.events.record(
                RuntimeEventKind::Resource,
                "value.lineage",
                Some(result.task_id.clone()),
                ref_lineage_attrs(value_ref.ref_id, value_ref.schema, value_ref.generation),
                None,
            );
        }
        for resource_ref in result.resources {
            self.events.record(
                RuntimeEventKind::Resource,
                "resource.lineage",
                Some(result.task_id.clone()),
                ref_lineage_attrs(
                    resource_ref.ref_id,
                    resource_ref.schema,
                    resource_ref.generation,
                ),
                None,
            );
        }
        for task in result.tasks {
            self.enqueue_task(task);
        }
        match result.status {
            RunnerStatus::Completed => {
                self.tasks.complete(&result.task_id, &runner.runner_id)?;
                return Ok(1);
            }
            RunnerStatus::Failed => {
                self.tasks.fail(
                    &result.task_id,
                    &runner.runner_id,
                    RuntimeError::new(
                        "runner.failed",
                        "runtime.result_router",
                        format!("runner.{}", runner.runner_id),
                    ),
                )?;
                return Ok(1);
            }
            RunnerStatus::Cancelled => {
                self.tasks.cancel_task(&result.task_id, &runner.runner_id)?;
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
