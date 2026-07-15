use mutsuki_agent_protocol::AgentError;
use mutsuki_runtime_sdk::contracts::{
    DomainEvent, ExecutionClass, OrderingRequirement, PayloadLayout, RunnerBatchCapability,
    RunnerControlCapability, RunnerMode, RunnerOrderingCapability, RunnerPayloadCapability,
    RunnerPurity, RunnerResourceCapability, RunnerResult, RunnerSideEffect, RunnerStatus,
    ScalarValue, Task, TimeoutGranularity,
};
use mutsuki_runtime_sdk::{RunnerDescriptorBuilder, RuntimeFailure, RuntimeResult};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub fn runtime_failure(
    source: &'static str,
    route: impl Into<String>,
    error: AgentError,
) -> RuntimeFailure {
    let mut runtime_error =
        mutsuki_runtime_sdk::contracts::RuntimeError::new(error.code, source, route.into());
    runtime_error
        .evidence
        .insert("message".into(), ScalarValue::String(error.message));
    RuntimeFailure::new(runtime_error)
}

pub fn task_payload<T>(source: &'static str, task: &Task) -> RuntimeResult<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(task.payload.clone()).map_err(|error| {
        runtime_failure(
            source,
            &task.task_id,
            AgentError::invalid_input(error.to_string()),
        )
    })
}

pub fn service_result_event<Request, Response>(
    source: &'static str,
    task: &Task,
    event_kind: impl Into<String>,
    service: impl FnOnce(Request) -> mutsuki_agent_protocol::AgentResult<Response>,
) -> RuntimeResult<RunnerResult>
where
    Request: DeserializeOwned,
    Response: Serialize,
{
    let request = task_payload(source, task)?;
    let result = service(request).map_err(|error| runtime_failure(source, &task.task_id, error))?;
    result_event(task.task_id.clone(), event_kind, result)
}

pub fn result_event(
    task_id: impl Into<String>,
    event_kind: impl Into<String>,
    payload: impl Serialize,
) -> RuntimeResult<RunnerResult> {
    let task_id = task_id.into();
    let payload = serde_json::to_value(payload).map_err(|error| {
        RuntimeFailure::new(mutsuki_runtime_sdk::contracts::RuntimeError::new(
            "agent.serialize_failed",
            "mutsuki.agent",
            error.to_string(),
        ))
    })?;
    let mut result = RunnerResult::completed(task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("{task_id}:result"),
        kind: event_kind.into(),
        payload,
    });
    Ok(result)
}

pub fn failed_result(task_id: impl Into<String>, error: AgentError) -> RunnerResult {
    let task_id = task_id.into();
    let mut result = RunnerResult::completed(task_id.clone());
    result.status = RunnerStatus::Failed;
    result.events.push(DomainEvent {
        event_id: format!("{task_id}:error"),
        kind: "mutsuki.agent.error".into(),
        payload: serde_json::to_value(error).unwrap_or_else(|_| serde_json::json!({})),
    });
    result
}

pub fn unsupported_protocol(source: &'static str, task: &Task) -> RuntimeFailure {
    runtime_failure(
        source,
        &task.task_id,
        AgentError::invalid_input(format!(
            "protocol `{}` is not supported by this runner",
            task.protocol_id
        )),
    )
}

/// Batch-first orchestration runner with explicit capabilities.
pub fn orchestration_runner(
    runner_id: impl Into<String>,
    plugin_id: impl Into<String>,
) -> RunnerDescriptorBuilder {
    agent_runner(
        runner_id,
        plugin_id,
        ExecutionClass::Orchestration,
        RunnerSideEffect::None,
    )
}

/// Effectful runner for LLM / external provider boundaries.
pub fn effectful_runner(
    runner_id: impl Into<String>,
    plugin_id: impl Into<String>,
) -> RunnerDescriptorBuilder {
    agent_runner(
        runner_id,
        plugin_id,
        ExecutionClass::Io,
        RunnerSideEffect::External,
    )
    .purity(RunnerPurity::Effectful)
}

fn agent_runner(
    runner_id: impl Into<String>,
    plugin_id: impl Into<String>,
    execution_class: ExecutionClass,
    side_effect: RunnerSideEffect,
) -> RunnerDescriptorBuilder {
    RunnerDescriptorBuilder::new(runner_id, plugin_id)
        .execution_class(execution_class)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            max_batch_entries: 32,
            max_inflight_batches: 1,
            side_effect,
            ..Default::default()
        })
        .payload_capability(RunnerPayloadCapability {
            layouts: vec![PayloadLayout::Row],
            preferred_layout: PayloadLayout::Row,
            zero_copy: false,
        })
        .resource_capability(RunnerResourceCapability {
            requires_resource_plan: false,
            ..Default::default()
        })
        .ordering_capability(RunnerOrderingCapability {
            default: OrderingRequirement::None,
            supports_sequence: true,
            supports_same_resource_order: true,
        })
        .control_capability(RunnerControlCapability {
            entry_cancel: true,
            batch_cancel: true,
            timeout_granularity: TimeoutGranularity::Entry,
        })
}
