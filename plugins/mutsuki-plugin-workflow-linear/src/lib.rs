use mutsuki_runtime_contracts::{
    CompletionBatch, DispatchLane, DomainEvent, ExecutionClass, OrderingRequirement,
    RunnerBatchCapability, RunnerContext, RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult,
    RunnerSideEffect, RuntimeError, ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.workflow.linear";
pub const RUNNER_ID: &str = "mutsuki.std.workflow.linear.runner";
pub const LINEAR_RUN_PROTOCOL: &str = "mutsuki.workflow.linear.run";

#[derive(Clone)]
pub struct WorkflowLinearRunner {
    descriptor: RunnerDescriptor,
}

impl WorkflowLinearRunner {
    pub fn new() -> Self {
        Self {
            descriptor: runner_descriptor(),
        }
    }
}

impl Default for WorkflowLinearRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for WorkflowLinearRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, workflow_linear_result)
    }
}

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(WorkflowLinearRunner::new()))
        .protocol_handler(protocol_descriptor(), RUNNER_ID, "workflow")
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(LINEAR_RUN_PROTOCOL)
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("workflow_linear".into()),
        )
        .build()
}

fn protocol_descriptor() -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(LINEAR_RUN_PROTOCOL)
        .input_schema(json!({
            "type": "object",
            "required": ["steps"],
            "properties": {
                "sequence_id": {"type": "string"},
                "steps": {"type": "array"}
            }
        }))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({"type": "object"}))
        .build()
}

fn workflow_linear_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    match task.protocol_id.as_str() {
        LINEAR_RUN_PROTOCOL => linear_result(task),
        _ => Err(RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.workflow_linear",
            format!("workflow_linear.protocol.{}", task.protocol_id),
        )),
    }
}

fn linear_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    let steps = task
        .payload
        .get("steps")
        .and_then(Value::as_array)
        .filter(|steps| !steps.is_empty())
        .ok_or_else(|| workflow_error(task, "mutsuki.workflow.invalid_steps", "steps.missing"))?;
    let sequence_id = task
        .payload
        .get("sequence_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("workflow.linear.{}", task.task_id));
    let mut result = RunnerResult::completed(task.task_id.clone());
    for (index, step) in steps.iter().enumerate() {
        result
            .tasks
            .push(step_task(task, step, index, &sequence_id)?);
    }
    result.events.push(DomainEvent {
        event_id: format!("event:{}.workflow_linear", task.task_id),
        kind: LINEAR_RUN_PROTOCOL.into(),
        payload: json!({
            "sequence_id": sequence_id,
            "step_count": steps.len(),
        }),
    });
    Ok(result)
}

fn step_task(
    parent: &Task,
    step: &Value,
    index: usize,
    sequence_id: &str,
) -> Result<Task, RuntimeError> {
    let protocol_id = step
        .get("protocol_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            workflow_error(
                parent,
                "mutsuki.workflow.invalid_step",
                format!("step.{index}.protocol_id"),
            )
        })?;
    let task_id = step
        .get("task_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}:step:{}", parent.task_id, index + 1));
    let payload = step.get("payload").cloned().unwrap_or_else(|| json!({}));
    let mut child = Task::new(task_id, protocol_id, payload);
    child.target_binding_id = step
        .get("target_binding_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    child.runner_hint = step
        .get("runner_hint")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    child.priority = step
        .get("priority")
        .and_then(Value::as_i64)
        .unwrap_or(parent.priority);
    child.dispatch_lane = parent.dispatch_lane.clone();
    if let Some(lane) = step.get("lane").and_then(Value::as_str) {
        child.dispatch_lane = match lane {
            "control" => DispatchLane::Control,
            "interactive" => DispatchLane::Interactive,
            "background" => DispatchLane::Background,
            "bulk" => DispatchLane::Bulk,
            _ => DispatchLane::Normal,
        };
    }
    child.trace_id = parent.trace_id.clone();
    child.correlation_id = parent
        .correlation_id
        .clone()
        .or_else(|| Some(parent.task_id.clone()));
    child.ordering = OrderingRequirement::StrictSequence {
        sequence_id: sequence_id.to_owned(),
    };
    Ok(child)
}

fn workflow_error(
    task: &Task,
    code: impl Into<String>,
    message: impl Into<String>,
) -> RuntimeError {
    let mut error = RuntimeError::new(code, "runtime.workflow_linear", message);
    error
        .evidence
        .insert("task_id".into(), ScalarValue::String(task.task_id.clone()));
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_plugin_declares_linear_protocol_and_runner() {
        let plugin = loaded_plugin();
        assert_eq!(plugin.manifest.plugin_id, PLUGIN_ID);
        assert_eq!(plugin.manifest.provides.runners[0].runner_id, RUNNER_ID);
        assert_eq!(
            plugin.manifest.provides.protocols[0].protocol_id,
            LINEAR_RUN_PROTOCOL
        );
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 1);
    }

    #[test]
    fn linear_runner_derives_ordered_child_tasks() {
        let mut runner = WorkflowLinearRunner::new();
        let task = Task::new(
            "workflow:1",
            LINEAR_RUN_PROTOCOL,
            json!({
                "sequence_id": "seq:test",
                "steps": [
                    {"protocol_id": "mutsuki.dev.echo", "payload": {"value": 1}},
                    {"task_id": "custom-step", "protocol_id": "mutsuki.dev.echo", "payload": {"value": 2}}
                ]
            }),
        );
        let batch = WorkBatch {
            batch_id: "batch:workflow".into(),
            tick_id: "tick:1".into(),
            batch_key: RUNNER_ID.into(),
            entries: vec![mutsuki_runtime_contracts::BatchEntry {
                entry_id: "workflow:1".into(),
                task_id: "workflow:1".into(),
                trace_id: None,
                parent_id: None,
                payload_index: 0,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(0),
                deadline_tick: None,
                priority: 0,
                lane: mutsuki_runtime_contracts::DispatchLane::Normal,
                ordering: mutsuki_runtime_contracts::OrderingRequirement::None,
            }],
            payload: mutsuki_runtime_contracts::BatchPayload::from_task_refs([&task]),
            resource_plan: mutsuki_runtime_contracts::WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        };

        let completion = runner
            .run_batch(
                RunnerContext::new(
                    1,
                    1,
                    "executor:workflow",
                    Vec::<String>::new(),
                    "batch:workflow",
                )
                .with_batch("batch:workflow", 1),
                batch,
            )
            .unwrap();

        let result = completion.results[0].result.as_ref().unwrap();
        assert_eq!(result.tasks.len(), 2);
        assert_eq!(result.tasks[0].task_id, "workflow:1:step:1");
        assert_eq!(result.tasks[1].task_id, "custom-step");
        assert!(matches!(
            result.tasks[0].ordering,
            OrderingRequirement::StrictSequence { ref sequence_id } if sequence_id == "seq:test"
        ));
    }
}
