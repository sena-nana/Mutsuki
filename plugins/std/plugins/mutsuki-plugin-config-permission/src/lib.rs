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
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.config.permission";
pub const RUNNER_ID: &str = "mutsuki.std.config.permission.runner";

pub const CONFIG_DESCRIBE_PROTOCOL: &str = "mutsuki.config.describe";
pub const PERMISSION_CHECK_PROTOCOL: &str = "mutsuki.permission.check";

#[derive(Clone)]
pub struct ConfigPermissionRunner {
    descriptor: RunnerDescriptor,
}

impl ConfigPermissionRunner {
    pub fn new() -> Self {
        Self {
            descriptor: runner_descriptor(),
        }
    }
}

impl Default for ConfigPermissionRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for ConfigPermissionRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, config_permission_result)
    }
}

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    let runner = ConfigPermissionRunner::new();
    let mut builder = PluginBuilder::new(PLUGIN_ID).runner(Box::new(runner));
    for protocol_id in [CONFIG_DESCRIBE_PROTOCOL, PERMISSION_CHECK_PROTOCOL] {
        builder = builder.protocol_handler(protocol_descriptor(protocol_id), RUNNER_ID, "config");
    }
    builder
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(CONFIG_DESCRIBE_PROTOCOL)
        .accepted_protocol(PERMISSION_CHECK_PROTOCOL)
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("config_permission".into()),
        )
        .build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object"}))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({"type": "object"}))
        .build()
}

fn config_describe_result(task: &mutsuki_runtime_contracts::Task) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}.config_describe", task.task_id),
        kind: CONFIG_DESCRIBE_PROTOCOL.into(),
        payload: json!({
            "plugin_id": PLUGIN_ID,
            "protocols": [
                CONFIG_DESCRIBE_PROTOCOL,
                PERMISSION_CHECK_PROTOCOL,
            ],
            "request": task.payload,
        }),
    });
    result
}

fn config_permission_result(
    task: &mutsuki_runtime_contracts::Task,
) -> Result<RunnerResult, RuntimeError> {
    match task.protocol_id.as_str() {
        CONFIG_DESCRIBE_PROTOCOL => Ok(config_describe_result(task)),
        PERMISSION_CHECK_PROTOCOL => permission_check_result(task),
        _ => Err(RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.config_permission",
            format!("config_permission.protocol.{}", task.protocol_id),
        )),
    }
}

fn permission_check_result(
    task: &mutsuki_runtime_contracts::Task,
) -> Result<RunnerResult, RuntimeError> {
    let request = task.payload.get("request").ok_or_else(|| {
        permission_error(
            task,
            "mutsuki.permission.invalid_request",
            "permission.request.missing",
        )
    })?;
    let grants = task.payload.get("grants").ok_or_else(|| {
        permission_error(
            task,
            "mutsuki.permission.invalid_request",
            "permission.grants.missing",
        )
    })?;
    let kind = string_field(request, "kind").ok_or_else(|| {
        permission_error(
            task,
            "mutsuki.permission.invalid_request",
            "permission.request.kind_missing",
        )
    })?;
    let allowed = match kind {
        "fs" => field_allowed_by_prefix(request, grants, "path", "fs_paths"),
        "http" => field_allowed_exact(request, grants, "domain", "http_domains"),
        "db" => field_allowed_by_prefix(request, grants, "path", "db_paths"),
        "resource" => field_allowed_exact(request, grants, "resource_kind", "resource_kinds"),
        _ => false,
    };
    if !allowed {
        return Err(permission_error(
            task,
            "mutsuki.permission.denied",
            format!("permission.denied.{kind}"),
        ));
    }

    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}.permission_check", task.task_id),
        kind: PERMISSION_CHECK_PROTOCOL.into(),
        payload: json!({
            "allowed": true,
            "request": request,
        }),
    });
    Ok(result)
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn field_allowed_exact(
    request: &Value,
    grants: &Value,
    request_field: &str,
    grant_field: &str,
) -> bool {
    let Some(requested) = string_field(request, request_field) else {
        return false;
    };
    granted_values(grants, grant_field).any(|grant| grant == requested)
}

fn field_allowed_by_prefix(
    request: &Value,
    grants: &Value,
    request_field: &str,
    grant_field: &str,
) -> bool {
    let Some(requested) = string_field(request, request_field) else {
        return false;
    };
    granted_values(grants, grant_field).any(|grant| requested.starts_with(grant))
}

fn granted_values<'a>(grants: &'a Value, field: &str) -> impl Iterator<Item = &'a str> {
    grants
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

fn permission_error(
    task: &mutsuki_runtime_contracts::Task,
    code: impl Into<String>,
    message: impl Into<String>,
) -> RuntimeError {
    let mut error = RuntimeError::new(code, "runtime.config_permission", message);
    error
        .evidence
        .insert("task_id".into(), ScalarValue::String(task.task_id.clone()));
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::Task;

    #[test]
    fn loaded_plugin_declares_config_permission_protocols_and_runner() {
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
            vec![CONFIG_DESCRIBE_PROTOCOL, PERMISSION_CHECK_PROTOCOL]
        );
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 2);
    }

    #[test]
    fn permission_runner_allows_granted_request_and_denies_missing_grant() {
        let mut runner = ConfigPermissionRunner::new();
        let tasks = vec![
            Task::new(
                "permission:allowed",
                PERMISSION_CHECK_PROTOCOL,
                json!({
                    "request": {"kind": "fs", "path": "C:/workspace/project/file.txt"},
                    "grants": {"fs_paths": ["C:/workspace/project"]},
                }),
            ),
            Task::new(
                "permission:denied",
                PERMISSION_CHECK_PROTOCOL,
                json!({
                    "request": {"kind": "http", "domain": "example.invalid"},
                    "grants": {"http_domains": ["example.com"]},
                }),
            ),
        ];
        let batch = WorkBatch {
            batch_id: "batch:permission".into(),
            tick_id: "tick:1".into(),
            batch_key: RUNNER_ID.into(),
            entries: vec![
                batch_entry("permission:allowed", 0),
                batch_entry("permission:denied", 1),
            ],
            payload: mutsuki_runtime_contracts::BatchPayload::from_task_refs(tasks.iter()),
            resource_plan: mutsuki_runtime_contracts::WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        };

        let completion = runner
            .run_batch(
                RunnerContext::new(
                    1,
                    1,
                    "executor:config",
                    Vec::<String>::new(),
                    "batch:permission",
                )
                .with_batch("batch:permission", 2),
                batch,
            )
            .unwrap();

        assert_eq!(completion.results.len(), 2);
        let event = &completion.results[0].result.as_ref().unwrap().events[0];
        assert_eq!(event.kind, PERMISSION_CHECK_PROTOCOL);
        assert_eq!(event.payload["allowed"], true);
        assert_eq!(
            completion.results[1].error.as_ref().unwrap().code,
            "mutsuki.permission.denied"
        );
    }

    fn batch_entry(task_id: &str, payload_index: usize) -> mutsuki_runtime_contracts::BatchEntry {
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
