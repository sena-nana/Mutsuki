use mutsuki_runtime_contracts::{
    CompletionBatch, DomainEvent, ExecutionClass, ResourceAccess, ResourceId, ResourceLifetime,
    ResourceRef, ResourceSealState, ResourceSemantic, RunnerBatchCapability, RunnerContext,
    RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError,
    ScalarValue, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.dev.mock";
pub const RUNNER_ID: &str = "mutsuki.std.dev.mock.runner";

pub const ECHO_PROTOCOL: &str = "mutsuki.dev.echo";
pub const SLEEP_PROTOCOL: &str = "mutsuki.dev.sleep";
pub const FAIL_PROTOCOL: &str = "mutsuki.dev.fail";
pub const RANDOM_FAIL_PROTOCOL: &str = "mutsuki.dev.random_fail";
pub const PRODUCE_RESOURCE_PROTOCOL: &str = "mutsuki.dev.produce_resource";
pub const CONSUME_RESOURCE_PROTOCOL: &str = "mutsuki.dev.consume_resource";

#[derive(Clone)]
pub struct DevMockRunner {
    descriptor: RunnerDescriptor,
}

impl DevMockRunner {
    pub fn new() -> Self {
        Self {
            descriptor: runner_descriptor(),
        }
    }
}

impl Default for DevMockRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for DevMockRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, dev_result)
    }
}

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    let runner = DevMockRunner::new();
    let mut builder = PluginBuilder::new(PLUGIN_ID).runner(Box::new(runner));
    for protocol_id in [
        ECHO_PROTOCOL,
        SLEEP_PROTOCOL,
        FAIL_PROTOCOL,
        RANDOM_FAIL_PROTOCOL,
        PRODUCE_RESOURCE_PROTOCOL,
        CONSUME_RESOURCE_PROTOCOL,
    ] {
        builder = builder.protocol_handler(protocol_descriptor(protocol_id), RUNNER_ID, "dev");
    }
    builder
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(ECHO_PROTOCOL)
        .accepted_protocol(SLEEP_PROTOCOL)
        .accepted_protocol(FAIL_PROTOCOL)
        .accepted_protocol(RANDOM_FAIL_PROTOCOL)
        .accepted_protocol(PRODUCE_RESOURCE_PROTOCOL)
        .accepted_protocol(CONSUME_RESOURCE_PROTOCOL)
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("dev_mock".into()))
        .build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object"}))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({"type": "object"}))
        .build()
}

fn echo_result(task: &mutsuki_runtime_contracts::Task) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}.echo", task.task_id),
        kind: "mutsuki.dev.echo".into(),
        payload: task.payload.to_value(),
    });
    result
}

fn dev_result(task: &mutsuki_runtime_contracts::Task) -> Result<RunnerResult, RuntimeError> {
    match task.protocol_id.as_str() {
        ECHO_PROTOCOL => Ok(echo_result(task)),
        SLEEP_PROTOCOL => Ok(sleep_result(task)),
        FAIL_PROTOCOL => Err(dev_failure(task)),
        RANDOM_FAIL_PROTOCOL => random_fail_result(task),
        PRODUCE_RESOURCE_PROTOCOL => Ok(produce_resource_result(task)),
        CONSUME_RESOURCE_PROTOCOL => Ok(consume_resource_result(task)),
        _ => Err(RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.dev_mock",
            format!("dev_mock.protocol.{}", task.protocol_id),
        )),
    }
}

fn sleep_result(task: &mutsuki_runtime_contracts::Task) -> RunnerResult {
    let duration_ms = task
        .payload
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}.sleep", task.task_id),
        kind: SLEEP_PROTOCOL.into(),
        payload: json!({ "duration_ms": duration_ms }),
    });
    result
}

fn dev_failure(task: &mutsuki_runtime_contracts::Task) -> RuntimeError {
    let mut error = RuntimeError::new(
        "mutsuki.dev.fail",
        "runtime.dev_mock",
        format!("dev_mock.fail.{}", task.task_id),
    );
    error.evidence.insert(
        "payload".into(),
        ScalarValue::String(task.payload.to_string()),
    );
    error
}

fn random_fail_result(
    task: &mutsuki_runtime_contracts::Task,
) -> Result<RunnerResult, RuntimeError> {
    let modulus = task
        .payload
        .get("fail_modulus")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .max(1);
    let seed = task
        .payload
        .get("seed")
        .and_then(Value::as_str)
        .unwrap_or("");
    let failed = stable_hash(&format!("{seed}:{}", task.task_id)) % modulus == 0;
    if failed {
        let mut error = RuntimeError::new(
            "mutsuki.dev.random_fail",
            "runtime.dev_mock",
            format!("dev_mock.random_fail.{}", task.task_id),
        );
        error
            .evidence
            .insert("fail_modulus".into(), ScalarValue::Int(modulus as i64));
        return Err(error);
    }

    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}.random_fail", task.task_id),
        kind: RANDOM_FAIL_PROTOCOL.into(),
        payload: json!({ "failed": false, "fail_modulus": modulus }),
    });
    Ok(result)
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

fn produce_resource_result(task: &mutsuki_runtime_contracts::Task) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.resources.push(dev_resource_ref(task));
    result
}

fn consume_resource_result(task: &mutsuki_runtime_contracts::Task) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    let consumed = task
        .payload
        .get("ref_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    result.events.push(DomainEvent {
        event_id: format!("event:{}.consume_resource", task.task_id),
        kind: "mutsuki.dev.consume_resource".into(),
        payload: json!({ "ref_id": consumed }),
    });
    result
}

fn dev_resource_ref(task: &mutsuki_runtime_contracts::Task) -> ResourceRef {
    let ref_id = task
        .payload
        .get("ref_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("dev-resource-{}", task.task_id));
    ResourceRef {
        ref_id: ref_id.clone(),
        resource_id: ResourceId {
            kind_id: "mutsuki.dev.mock.resource".into(),
            slot_id: ref_id.clone(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: PLUGIN_ID.into(),
        resource_kind: "mutsuki.dev.mock.resource".into(),
        schema: "mutsuki.dev.resource.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: None,
        content_hash: None,
        lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::Task;

    #[test]
    fn loaded_plugin_declares_dev_protocols_and_runner() {
        let plugin = loaded_plugin();
        assert_eq!(plugin.manifest.plugin_id, PLUGIN_ID);
        assert_eq!(plugin.manifest.provides.runners[0].runner_id, RUNNER_ID);
        let protocols: Vec<_> = plugin
            .manifest
            .provides
            .protocols
            .iter()
            .map(|protocol| protocol.protocol_id.as_str())
            .collect();
        assert_eq!(
            protocols,
            vec![
                ECHO_PROTOCOL,
                SLEEP_PROTOCOL,
                FAIL_PROTOCOL,
                RANDOM_FAIL_PROTOCOL,
                PRODUCE_RESOURCE_PROTOCOL,
                CONSUME_RESOURCE_PROTOCOL,
            ]
        );
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 6);
    }

    #[test]
    fn dev_runner_echo_sleep_fail_and_random_fail_are_batch_entries() {
        let mut runner = DevMockRunner::new();
        let tasks = vec![
            Task::new("task:echo", ECHO_PROTOCOL, json!({"value": 1})),
            Task::new("task:sleep", SLEEP_PROTOCOL, json!({"duration_ms": 25})),
            Task::new("task:fail", FAIL_PROTOCOL, json!({"reason": "expected"})),
            Task::new(
                "task:random-fail",
                RANDOM_FAIL_PROTOCOL,
                json!({"fail_modulus": 1, "seed": "deterministic"}),
            ),
        ];
        let batch = WorkBatch {
            batch_id: "batch:dev".into(),
            tick_id: "tick:1".into(),
            batch_key: RUNNER_ID.into(),
            entries: vec![
                batch_entry("task:echo", ECHO_PROTOCOL, 0),
                batch_entry("task:sleep", SLEEP_PROTOCOL, 1),
                batch_entry("task:fail", FAIL_PROTOCOL, 2),
                batch_entry("task:random-fail", RANDOM_FAIL_PROTOCOL, 3),
            ],
            payload: mutsuki_runtime_contracts::BatchPayload::from_task_refs(tasks.iter()),
            resource_plan: mutsuki_runtime_contracts::WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        };

        let completion = runner
            .run_batch(
                RunnerContext::new(1, 1, "executor:dev", Vec::<String>::new(), "batch:dev")
                    .with_batch("batch:dev", 4),
                batch,
            )
            .unwrap();

        assert_eq!(completion.results.len(), 4);
        assert_eq!(
            completion.results[0].result.as_ref().unwrap().events[0].payload,
            json!({"value": 1})
        );
        assert_eq!(
            completion.results[1].result.as_ref().unwrap().events[0].payload,
            json!({"duration_ms": 25})
        );
        assert_eq!(
            completion.results[2].error.as_ref().unwrap().code,
            "mutsuki.dev.fail"
        );
        assert_eq!(
            completion.results[3].error.as_ref().unwrap().code,
            "mutsuki.dev.random_fail"
        );
    }

    fn batch_entry(
        task_id: &str,
        _protocol_id: &str,
        payload_index: usize,
    ) -> mutsuki_runtime_contracts::BatchEntry {
        mutsuki_runtime_contracts::BatchEntry {
            entry_id: task_id.into(),
            task_id: task_id.into(),
            trace_id: None,
            parent_id: None,
            payload_index,
            resource_requirement_indices: Vec::new(),
            cancel_index: Some(payload_index),
            deadline_tick: None,
            priority: 0,
            lane: mutsuki_runtime_contracts::DispatchLane::Normal,
            ordering: mutsuki_runtime_contracts::OrderingRequirement::None,
        }
    }
}
