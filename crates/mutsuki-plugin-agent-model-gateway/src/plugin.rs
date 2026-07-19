use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentModelGenerateProtocol, AgentModelStreamProtocol, orchestration_runner, result_event,
    runtime_failure, task_payload, unsupported_protocol,
};
use mutsuki_runtime_core::{AsyncBatchHandler, AsyncCompletionFuture, RunnerContext};
use mutsuki_runtime_sdk::contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, InvocationMode, RunnerBatchCapability,
    RunnerConcurrency, RunnerMode, RunnerResult, RunnerSideEffect, Task, WorkBatch,
};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult};
use std::sync::Arc;

use crate::{ModelGateway, ModelProviderExecution};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.model_gateway";
pub const RUNNER_ID: &str = "mutsuki.agent.model_gateway.runner";

pub fn plugin(_client: RuntimeClientRef, gateway: ModelGateway) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentModelGenerateProtocol>()
        .protocol::<AgentModelStreamProtocol>()
        .async_handler(Arc::new(ModelAsyncHandler::new(gateway)))
}

pub struct ModelAsyncHandler {
    descriptor: mutsuki_runtime_sdk::contracts::RunnerDescriptor,
    gateway: ModelGateway,
}

impl ModelAsyncHandler {
    pub fn new(gateway: ModelGateway) -> Self {
        Self {
            descriptor: orchestration_runner(RUNNER_ID, PLUGIN_ID)
                .accepts::<AgentModelGenerateProtocol>()
                .accepts::<AgentModelStreamProtocol>()
                .execution_class(ExecutionClass::Io)
                .invocation_mode(InvocationMode::AsyncReentrant)
                .concurrency(RunnerConcurrency::Reentrant {
                    max_inflight_batches: 64,
                    max_inflight_entries: 64,
                })
                .batch_capability(RunnerBatchCapability {
                    mode: RunnerMode::NativeBatch,
                    preferred_batch_size: 1,
                    max_batch_entries: 1,
                    max_inflight_batches: 64,
                    side_effect: RunnerSideEffect::External,
                    ..Default::default()
                })
                .build(),
            gateway,
        }
    }
}

impl AsyncBatchHandler for ModelAsyncHandler {
    fn descriptor(&self) -> &mutsuki_runtime_sdk::contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, _ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        let gateway = self.gateway.clone();
        Box::pin(async move {
            let tasks = match batch.row_payload_tasks() {
                Ok(tasks) => tasks,
                Err(error) => return Ok(CompletionBatch::from_error(&batch, error)),
            };
            let mut results = Vec::with_capacity(batch.entries.len());
            for entry in &batch.entries {
                let task = tasks
                    .iter()
                    .find(|task| task.task_id == entry.task_id)
                    .expect("row payload task should exist for batch entry")
                    .clone();
                let (result, error) = match run_task(gateway.clone(), task).await {
                    Ok(result) => (Some(result), None),
                    Err(error) => (None, Some(error.error().clone())),
                };
                results.push(EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result,
                    error,
                });
            }
            Ok(CompletionBatch::from_results(&batch, results))
        })
    }
}

pub fn async_handler(gateway: ModelGateway) -> Arc<dyn AsyncBatchHandler> {
    Arc::new(ModelAsyncHandler::new(gateway))
}

async fn run_task(gateway: ModelGateway, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_MODEL_GENERATE_PROTOCOL => {
            let request: AgentModelGenerateRequest = task_payload(PLUGIN_ID, &task)?;
            let callback_protocol = request.result_protocol_id.clone();
            let callback_context = request.result_context.clone();
            let session_id = request.session_id.clone();
            let generated = if gateway
                .provider_execution(&request)
                .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?
                == ModelProviderExecution::HttpEffect
            {
                gateway.generate_effect_async(request).await
            } else {
                gateway.generate_async(request).await
            }
            .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?;
            let mut result = result_event(
                task.task_id.clone(),
                "mutsuki.agent.model.generated",
                generated.clone(),
            )?;
            append_callback(
                &task,
                &mut result,
                callback_protocol,
                callback_context,
                session_id,
                generated,
            )?;
            Ok(result)
        }
        AGENT_MODEL_STREAM_PROTOCOL => {
            let request: AgentModelStreamRequest = task_payload(PLUGIN_ID, &task)?;
            let streamed = if gateway
                .provider_execution(&request.request)
                .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?
                == ModelProviderExecution::HttpEffect
            {
                gateway.stream_effect_async(request).await
            } else {
                gateway.stream_async(request).await
            }
            .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?;
            result_event(task.task_id, "mutsuki.agent.model.stream_opened", streamed)
        }
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}

pub(crate) fn append_callback(
    task: &Task,
    result: &mut RunnerResult,
    callback_protocol: Option<String>,
    callback_context: Option<serde_json::Value>,
    session_id: Option<String>,
    generated: AgentModelGenerateResult,
) -> RuntimeResult<()> {
    let Some(protocol_id) = callback_protocol else {
        return Ok(());
    };
    if protocol_id.trim().is_empty() {
        return Err(runtime_failure(
            PLUGIN_ID,
            &task.task_id,
            AgentError::invalid_input("result_protocol_id must not be empty"),
        ));
    }
    let mut callback = Task::new(
        format!("{}:result", task.task_id),
        protocol_id,
        serde_json::to_value(AgentModelResultCallback {
            result: generated,
            context: callback_context,
            session_id,
        })
        .map_err(|error| {
            runtime_failure(
                PLUGIN_ID,
                &task.task_id,
                AgentError::invalid_input(error.to_string()),
            )
        })?,
    );
    callback.trace_id = task.trace_id.clone();
    callback.correlation_id = task.correlation_id.clone();
    callback.registry_generation = task.registry_generation;
    result.tasks.push(callback);
    Ok(())
}
