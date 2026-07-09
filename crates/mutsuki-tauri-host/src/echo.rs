use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, RunnerDescriptor, RunnerPurity, RunnerResult,
    Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use serde_json::json;
use std::collections::BTreeMap;

pub const ECHO_PROTOCOL_ID: &str = "mutsuki.tauri/echo@1";
pub const ECHO_PLUGIN_ID: &str = "mutsuki.tauri.echo";
pub const ECHO_RUNNER_ID: &str = "mutsuki.tauri.echo.runner";

pub struct EchoRunner {
    descriptor: RunnerDescriptor,
}

impl EchoRunner {
    pub fn new() -> Self {
        Self {
            descriptor: echo_descriptor(),
        }
    }
}

impl Default for EchoRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for EchoRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let tasks = match batch.row_payload_tasks() {
            Ok(tasks) => tasks,
            Err(error) => return Ok(CompletionBatch::from_error(&batch, error)),
        };
        let results = batch
            .entries
            .iter()
            .map(
                |entry| match tasks.iter().find(|task| task.task_id == entry.task_id) {
                    Some(task) => EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: Some(echo_result(task)),
                        error: None,
                    },
                    None => EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: None,
                        error: Some(mutsuki_runtime_contracts::RuntimeError::new(
                            mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
                            "mutsuki.tauri.echo",
                            format!("batch.entry.{}", entry.entry_id),
                        )),
                    },
                },
            )
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }
}

fn echo_result(task: &Task) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.values.push(mutsuki_runtime_contracts::ValueRef {
        ref_id: format!("value:{}", result.task_id),
        provider_id: "mutsuki.tauri.echo".into(),
        schema: "application/json".into(),
        version: 1,
        generation: 1,
        size_hint: None,
        content_hash: None,
        lifetime: mutsuki_runtime_contracts::ResourceLifetime::BorrowedUntilTaskEnd,
        storage: mutsuki_runtime_contracts::ValueStorage::InlineSmall,
    });
    result.events.push(mutsuki_runtime_contracts::DomainEvent {
        event_id: format!("event:{}", result.task_id),
        kind: "mutsuki.tauri.echo.completed".into(),
        payload: json!({ "task_id": result.task_id }),
    });
    result
}

pub fn echo_descriptor() -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: ECHO_RUNNER_ID.into(),
        plugin_id: ECHO_PLUGIN_ID.into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![ECHO_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Io,
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("task_protocol:{ECHO_PROTOCOL_ID}")],
    }
}
