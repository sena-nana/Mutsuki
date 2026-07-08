use mutsuki_runtime_contracts::{
    CompletionBatch, DomainEvent, ExecutionClass, RunnerBatchCapability, RunnerContext,
    RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError,
    ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use serde_json::json;

pub const PLUGIN_ID: &str = "mutsuki.std.observe.log";
pub const RUNNER_ID: &str = "mutsuki.std.observe.log.runner";

pub const LOG_EMIT_PROTOCOL: &str = "mutsuki.log.emit";
pub const TRACE_SPAN_START_PROTOCOL: &str = "mutsuki.trace.span_start";
pub const TRACE_SPAN_END_PROTOCOL: &str = "mutsuki.trace.span_end";
pub const TRACE_EVENT_PROTOCOL: &str = "mutsuki.trace.event";

#[derive(Clone)]
pub struct ObserveLogRunner {
    descriptor: RunnerDescriptor,
}

impl ObserveLogRunner {
    pub fn new() -> Self {
        Self {
            descriptor: runner_descriptor(),
        }
    }
}

impl Default for ObserveLogRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for ObserveLogRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, observe_result)
    }
}

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    let runner = ObserveLogRunner::new();
    let mut builder = PluginBuilder::new(PLUGIN_ID).runner(Box::new(runner));
    for protocol_id in [
        LOG_EMIT_PROTOCOL,
        TRACE_SPAN_START_PROTOCOL,
        TRACE_SPAN_END_PROTOCOL,
        TRACE_EVENT_PROTOCOL,
    ] {
        builder = builder.protocol_handler(protocol_descriptor(protocol_id), RUNNER_ID, "observe");
    }
    builder
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(LOG_EMIT_PROTOCOL)
        .accepted_protocol(TRACE_SPAN_START_PROTOCOL)
        .accepted_protocol(TRACE_SPAN_END_PROTOCOL)
        .accepted_protocol(TRACE_EVENT_PROTOCOL)
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("observe_log".into()))
        .build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object"}))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({"type": "object"}))
        .build()
}

fn accepts_protocol(protocol_id: &str) -> bool {
    matches!(
        protocol_id,
        LOG_EMIT_PROTOCOL
            | TRACE_SPAN_START_PROTOCOL
            | TRACE_SPAN_END_PROTOCOL
            | TRACE_EVENT_PROTOCOL
    )
}

fn observe_result(task: &mutsuki_runtime_contracts::Task) -> Result<RunnerResult, RuntimeError> {
    if accepts_protocol(&task.protocol_id) {
        return Ok(event_result(task));
    }
    Err(RuntimeError::new(
        mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
        "runtime.observe_log",
        format!("observe_log.protocol.{}", task.protocol_id),
    ))
}

fn event_result(task: &mutsuki_runtime_contracts::Task) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}:{}", task.protocol_id, task.task_id),
        kind: task.protocol_id.clone(),
        payload: task.payload.clone(),
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::Task;

    #[test]
    fn loaded_plugin_declares_observe_protocols_and_runner() {
        let plugin = loaded_plugin();
        assert_eq!(plugin.manifest.plugin_id, PLUGIN_ID);
        assert_eq!(plugin.manifest.provides.runners[0].runner_id, RUNNER_ID);
        assert_eq!(plugin.manifest.provides.protocols.len(), 4);
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 4);
    }

    #[test]
    fn observe_runner_maps_log_task_to_domain_event() {
        let mut runner = ObserveLogRunner::new();
        let task = Task::new("log-task", LOG_EMIT_PROTOCOL, json!({"message": "hello"}));
        let batch = WorkBatch {
            batch_id: "batch:observe".into(),
            tick_id: "tick:1".into(),
            batch_key: RUNNER_ID.into(),
            entries: vec![mutsuki_runtime_contracts::BatchEntry {
                entry_id: "log-task".into(),
                task_id: "log-task".into(),
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
                    "executor:observe",
                    Vec::<String>::new(),
                    "batch:observe",
                )
                .with_batch("batch:observe", 1),
                batch,
            )
            .unwrap();

        let event = &completion.results[0].result.as_ref().unwrap().events[0];
        assert_eq!(event.kind, LOG_EMIT_PROTOCOL);
        assert_eq!(event.payload, json!({"message": "hello"}));
    }
}
