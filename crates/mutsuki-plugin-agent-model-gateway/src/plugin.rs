use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentModelGenerateProtocol, AgentModelHttpEffectProtocol, AgentModelPollProtocol,
    AgentModelStreamProtocol, completed_output, orchestration_runner, result_event,
    runtime_failure, task_payload, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{
    AsyncRunnerAdapter, AsyncRunnerContext, PluginBuilder, RuntimeClientRef, RuntimeResult,
};

use crate::{HttpEffectRunner, ModelGateway, ModelPollRunner, ModelProviderExecution};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.model_gateway";
pub const RUNNER_ID: &str = "mutsuki.agent.model_gateway.runner";

pub fn plugin(client: RuntimeClientRef, gateway: ModelGateway) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentModelGenerateProtocol>()
        .protocol::<AgentModelStreamProtocol>()
        .protocol::<AgentModelHttpEffectProtocol>()
        .protocol::<AgentModelPollProtocol>()
        .runner(Box::new(runner(client, gateway.clone())))
        .runner(Box::new(HttpEffectRunner::blocking(gateway)))
        .runner(Box::new(ModelPollRunner::default()))
}

pub fn runner(client: RuntimeClientRef, gateway: ModelGateway) -> AsyncRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentModelGenerateProtocol>()
        .accepts::<AgentModelStreamProtocol>()
        .build();
    AsyncRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |ctx, task| {
            let gateway = gateway.clone();
            Box::pin(async move { run_task(gateway, ctx, task).await })
        }),
    )
}

async fn run_task(
    gateway: ModelGateway,
    ctx: AsyncRunnerContext,
    task: Task,
) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_MODEL_GENERATE_PROTOCOL => {
            let request: AgentModelGenerateRequest = task_payload(PLUGIN_ID, &task)?;
            let callback_protocol = request.result_protocol_id.clone();
            let callback_context = request.result_context.clone();
            let session_id = request.session_id.clone();
            if gateway
                .provider_execution(&request)
                .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?
                == ModelProviderExecution::HttpEffect
            {
                let outcome = ctx
                    .call::<AgentModelHttpEffectProtocol>(AgentModelHttpEffectRequest::Generate(
                        request,
                    ))
                    .await?;
                let generated: AgentModelGenerateResult =
                    completed_output(PLUGIN_ID, &task.task_id, outcome)?;
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
                return Ok(result);
            }
            let generated = gateway
                .generate_async(request)
                .await
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
            if gateway
                .provider_execution(&request.request)
                .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?
                == ModelProviderExecution::HttpEffect
            {
                let outcome = ctx
                    .call::<AgentModelHttpEffectProtocol>(AgentModelHttpEffectRequest::Stream(
                        request,
                    ))
                    .await?;
                return effect_dispatch_result::<AgentModelStreamResult>(
                    &task,
                    outcome,
                    "mutsuki.agent.model.stream_opened",
                );
            }
            let streamed = gateway
                .stream_async(request)
                .await
                .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?;
            result_event(task.task_id, "mutsuki.agent.model.stream_opened", streamed)
        }
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}

fn effect_dispatch_result<T>(
    task: &Task,
    outcome: mutsuki_runtime_sdk::contracts::TaskOutcome,
    event_kind: &'static str,
) -> RuntimeResult<RunnerResult>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let output: T = completed_output(PLUGIN_ID, &task.task_id, outcome)?;
    result_event(task.task_id.clone(), event_kind, output)
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
