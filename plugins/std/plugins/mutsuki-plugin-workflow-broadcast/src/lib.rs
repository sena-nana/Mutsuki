use mutsuki_runtime_contracts::{
    CompletionBatch, DispatchLane, DomainEvent, ExecutionClass, RunnerBatchCapability,
    RunnerContext, RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect,
    RuntimeError, ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.workflow.broadcast";
pub const RUNNER_ID: &str = "mutsuki.std.workflow.broadcast.runner";
pub const BROADCAST_EMIT_PROTOCOL: &str = "mutsuki.workflow.broadcast.emit";

#[derive(Clone)]
pub struct WorkflowBroadcastRunner {
    descriptor: RunnerDescriptor,
}

impl WorkflowBroadcastRunner {
    pub fn new() -> Self {
        Self {
            descriptor: runner_descriptor(),
        }
    }
}

impl Default for WorkflowBroadcastRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for WorkflowBroadcastRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, workflow_broadcast_result)
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
        .runner(Box::new(WorkflowBroadcastRunner::new()))
        .protocol_handler(protocol_descriptor(), RUNNER_ID, "workflow")
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(BROADCAST_EMIT_PROTOCOL)
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("workflow_broadcast".into()),
        )
        .build()
}

fn protocol_descriptor() -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(BROADCAST_EMIT_PROTOCOL)
        .input_schema(json!({
            "type": "object",
            "required": ["targets"],
            "properties": {
                "targets": {"type": "array"},
                "mode": {"type": "string"},
                "concurrency_limit": {"type": "integer"}
            }
        }))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({"type": "object"}))
        .build()
}

fn workflow_broadcast_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    match task.protocol_id.as_str() {
        BROADCAST_EMIT_PROTOCOL => broadcast_result(task),
        _ => Err(RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.workflow_broadcast",
            format!("workflow_broadcast.protocol.{}", task.protocol_id),
        )),
    }
}

fn broadcast_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    let targets = task
        .payload
        .get("targets")
        .and_then(Value::as_array)
        .filter(|targets| !targets.is_empty())
        .ok_or_else(|| {
            broadcast_error(task, "mutsuki.workflow.invalid_targets", "targets.missing")
        })?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    for (index, target) in targets.iter().enumerate() {
        result.tasks.push(target_task(task, target, index)?);
    }
    result.events.push(DomainEvent {
        event_id: format!("event:{}.workflow_broadcast", task.task_id),
        kind: BROADCAST_EMIT_PROTOCOL.into(),
        payload: json!({
            "target_count": targets.len(),
            "mode": task.payload.get("mode").and_then(Value::as_str).unwrap_or("fire_and_forget"),
            "concurrency_limit": task.payload.get("concurrency_limit").and_then(Value::as_u64),
        }),
    });
    Ok(result)
}

fn target_task(parent: &Task, target: &Value, index: usize) -> Result<Task, RuntimeError> {
    let protocol_id = target
        .get("protocol_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            broadcast_error(
                parent,
                "mutsuki.workflow.invalid_target",
                format!("target.{index}.protocol_id"),
            )
        })?;
    let task_id = target
        .get("task_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}:target:{}", parent.task_id, index + 1));
    let payload = target.get("payload").cloned().unwrap_or_else(|| json!({}));
    let mut child = Task::new(task_id, protocol_id, payload);
    child.target_binding_id = target
        .get("target_binding_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    child.runner_hint = target
        .get("runner_hint")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    child.priority = target
        .get("priority")
        .and_then(Value::as_i64)
        .unwrap_or(parent.priority);
    child.dispatch_lane = match target.get("lane").and_then(Value::as_str) {
        Some("control") => DispatchLane::Control,
        Some("interactive") => DispatchLane::Interactive,
        Some("background") => DispatchLane::Background,
        Some("bulk") => DispatchLane::Bulk,
        _ => parent.dispatch_lane.clone(),
    };
    child.trace_id = parent.trace_id.clone();
    child.correlation_id = parent
        .correlation_id
        .clone()
        .or_else(|| Some(parent.task_id.clone()));
    Ok(child)
}

fn broadcast_error(
    task: &Task,
    code: impl Into<String>,
    message: impl Into<String>,
) -> RuntimeError {
    let mut error = RuntimeError::new(code, "runtime.workflow_broadcast", message);
    error
        .evidence
        .insert("task_id".into(), ScalarValue::String(task.task_id.clone()));
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_plugin_declares_broadcast_protocol_and_runner() {
        let plugin = loaded_plugin();
        assert_eq!(plugin.manifest.plugin_id, PLUGIN_ID);
        assert_eq!(plugin.manifest.provides.runners[0].runner_id, RUNNER_ID);
        assert_eq!(
            plugin.manifest.provides.protocols[0].protocol_id,
            BROADCAST_EMIT_PROTOCOL
        );
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 1);
    }

    #[test]
    fn broadcast_runner_derives_targeted_child_tasks() {
        let mut runner = WorkflowBroadcastRunner::new();
        let task = Task::new(
            "broadcast:1",
            BROADCAST_EMIT_PROTOCOL,
            json!({
                "mode": "fire_and_forget",
                "targets": [
                    {"protocol_id": "mutsuki.dev.echo", "target_binding_id": "binding:mutsuki.dev.echo", "payload": {"value": 1}},
                    {"task_id": "custom-target", "protocol_id": "mutsuki.dev.echo", "payload": {"value": 2}}
                ]
            }),
        );
        let batch = WorkBatch {
            batch_id: "batch:broadcast".into(),
            tick_id: "tick:1".into(),
            batch_key: RUNNER_ID.into(),
            entries: vec![mutsuki_runtime_contracts::BatchEntry {
                entry_id: "broadcast:1".into(),
                task_id: "broadcast:1".into(),
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
                    "executor:broadcast",
                    Vec::<String>::new(),
                    "batch:broadcast",
                )
                .with_batch("batch:broadcast", 1),
                batch,
            )
            .unwrap();

        let result = completion.results[0].result.as_ref().unwrap();
        assert_eq!(result.tasks.len(), 2);
        assert_eq!(result.tasks[0].task_id, "broadcast:1:target:1");
        assert_eq!(
            result.tasks[0].target_binding_id.as_deref(),
            Some("binding:mutsuki.dev.echo")
        );
        assert_eq!(result.tasks[1].task_id, "custom-target");
    }
}
