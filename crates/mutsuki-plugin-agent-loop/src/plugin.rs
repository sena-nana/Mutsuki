use mutsuki_agent_protocol::*;
use mutsuki_agent_sdk::{
    AgentModelGenerateProtocol, AgentModelStreamProtocol, AgentRunProtocol,
    AgentSessionAppendProtocol, AgentSessionGetProtocol, AgentToolExecuteProtocol,
    completed_output, orchestration_runner, result_event, runtime_failure, task_payload,
    unsupported_protocol,
};
use mutsuki_runtime_sdk::AsyncRunnerContext;
use mutsuki_runtime_sdk::contracts::{RunnerResult, ScalarValue, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

use crate::AgentLoop;

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.loop";
pub const RUNNER_ID: &str = "mutsuki.agent.loop.runner";

pub fn plugin(client: RuntimeClientRef, agent_loop: AgentLoop) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentRunProtocol>()
        .runner(Box::new(runner(client, agent_loop)))
}

pub fn runner(client: RuntimeClientRef, agent_loop: AgentLoop) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentRunProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |ctx, task| {
            let agent_loop = agent_loop.clone();
            Box::pin(async move { run_task(agent_loop, ctx, task).await })
        }),
    )
}

async fn run_task(
    agent_loop: AgentLoop,
    ctx: AsyncRunnerContext,
    task: Task,
) -> RuntimeResult<RunnerResult> {
    if task.protocol_id != AGENT_RUN_PROTOCOL {
        return Err(unsupported_protocol(PLUGIN_ID, &task));
    }
    let request: AgentRunRequest = task_payload(PLUGIN_ID, &task)?;
    let callback_protocol = request.result_protocol_id.clone();
    let callback_context = request.result_context.clone();
    let session_id = request.session_id.clone();
    let result = execute(agent_loop, ctx, request)
        .await
        .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?;
    let mut runner_result = result_event(
        task.task_id.clone(),
        "mutsuki.agent.run.completed",
        result.clone(),
    )?;
    append_callback(
        &task,
        &mut runner_result,
        callback_protocol,
        callback_context,
        session_id,
        result,
    )?;
    Ok(runner_result)
}

async fn execute(
    agent_loop: AgentLoop,
    ctx: AsyncRunnerContext,
    mut request: AgentRunRequest,
) -> AgentResult<AgentRunResult> {
    let model = agent_loop.validate(&request)?;
    let persisted_message_count = if let Some(session_id) = &request.session_id {
        let outcome = ctx
            .call::<AgentSessionGetProtocol>(AgentSessionGetRequest {
                session_id: session_id.clone(),
            })
            .await
            .map_err(runtime_agent_error)?;
        let session: AgentSession =
            completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
        if session.profile_id != request.profile_id {
            return Err(AgentError::invalid_input(format!(
                "session profile `{}` does not match requested profile `{}`",
                session.profile_id, request.profile_id
            )));
        }
        let persisted_message_count = session.messages.len();
        let mut messages = session.messages;
        messages.append(&mut request.messages);
        request.messages = messages;
        persisted_message_count
    } else {
        0
    };

    let result = execute_run(model, &ctx, &request).await?;
    if let Some(session_id) = &request.session_id {
        let outcome = ctx
            .call::<AgentSessionAppendProtocol>(AgentSessionAppendRequest {
                session_id: session_id.clone(),
                messages: result.messages[persisted_message_count..].to_vec(),
            })
            .await
            .map_err(runtime_agent_error)?;
        let _: AgentSession =
            completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
    }
    Ok(result)
}

async fn execute_run(
    model: String,
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
) -> AgentResult<AgentRunResult> {
    let mut messages = request.messages.clone();
    let mut steps = Vec::new();
    let mut usage = AgentUsage::default();
    let mut cost_microunits = 0_u64;
    let mut output_resource = None;

    if request.max_steps == 0 {
        return Ok(run_result(
            AgentRunStatus::BudgetExceeded,
            messages,
            steps,
            usage,
            cost_microunits,
            output_resource,
        ));
    }

    for step_index in 0..request.max_steps {
        let model_request = AgentModelGenerateRequest {
            model: model.clone(),
            messages: messages.clone(),
            temperature: None,
            max_output_tokens: request
                .budget
                .max_total_tokens
                .map(|limit| limit.saturating_sub(usage.total_tokens)),
            provider_hint: None,
            metadata: request.metadata.clone(),
            result_protocol_id: None,
            result_context: None,
            session_id: request.session_id.clone(),
        };
        let generated = if request.stream {
            let outcome = ctx
                .call::<AgentModelStreamProtocol>(AgentModelStreamRequest {
                    request: model_request,
                })
                .await
                .map_err(runtime_agent_error)?;
            let streamed: AgentModelStreamResult =
                completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
            steps.push(AgentStepRecord {
                step_index,
                kind: "model_stream".into(),
                detail: Some(serde_json::json!({"stream": streamed.stream.clone()})),
            });
            AgentModelGenerateResult {
                message: AgentMessage::assistant(""),
                stop_reason: streamed.stop_reason,
                tool_calls: streamed.tool_calls,
                usage: streamed.usage,
                cost_microunits: streamed.cost_microunits,
                raw: None,
                output_resource: Some(streamed.stream),
            }
        } else {
            let outcome = ctx
                .call::<AgentModelGenerateProtocol>(model_request)
                .await
                .map_err(runtime_agent_error)?;
            let generated: AgentModelGenerateResult =
                completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
            steps.push(AgentStepRecord {
                step_index,
                kind: "model_generate".into(),
                detail: Some(serde_json::json!({
                    "model": model,
                    "stop_reason": generated.stop_reason,
                })),
            });
            generated
        };

        usage.add(&generated.usage);
        cost_microunits = cost_microunits.saturating_add(generated.cost_microunits);
        output_resource = generated.output_resource.clone().or(output_resource);
        let mut assistant = generated.message;
        if !generated.tool_calls.is_empty() {
            assistant.metadata = Some(serde_json::json!({"tool_calls": generated.tool_calls}));
        }
        messages.push(assistant);

        if exceeds_budget(&request.budget, &usage, cost_microunits)
            || generated.stop_reason == AgentModelStopReason::Length
        {
            return Ok(run_result(
                AgentRunStatus::BudgetExceeded,
                messages,
                steps,
                usage,
                cost_microunits,
                output_resource,
            ));
        }
        if generated.stop_reason == AgentModelStopReason::ContentFilter {
            return Err(AgentError::new(
                "agent.model.content_filtered",
                "model stopped because content was filtered",
            ));
        }
        if generated.tool_calls.is_empty() {
            if generated.stop_reason == AgentModelStopReason::ToolCalls {
                return Err(AgentError::new(
                    "agent.model.invalid_result",
                    "model declared tool_calls without returning a tool call",
                ));
            }
            return Ok(run_result(
                AgentRunStatus::Completed,
                messages,
                steps,
                usage,
                cost_microunits,
                output_resource,
            ));
        }

        if budget_exhausted_for_followup(&request.budget, &usage, cost_microunits) {
            return Ok(run_result(
                AgentRunStatus::BudgetExceeded,
                messages,
                steps,
                usage,
                cost_microunits,
                output_resource,
            ));
        }

        for tool_call in generated.tool_calls {
            let outcome = ctx
                .call::<AgentToolExecuteProtocol>(AgentToolExecuteRequest {
                    call_id: Some(tool_call.call_id.clone()),
                    name: tool_call.name.clone(),
                    input: tool_call.input,
                    session_id: request.session_id.clone(),
                })
                .await
                .map_err(runtime_agent_error)?;
            let tool_result: AgentToolExecuteResult =
                completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
            let content = tool_result
                .output
                .as_ref()
                .map(serde_json::Value::to_string)
                .unwrap_or_default();
            messages.push(AgentMessage {
                role: AgentRole::Tool,
                content,
                name: Some(tool_result.name.clone()),
                metadata: Some(serde_json::json!({
                    "call_id": tool_result.call_id,
                    "output_ref": tool_result.output_ref,
                })),
            });
            steps.push(AgentStepRecord {
                step_index,
                kind: "tool_execute".into(),
                detail: Some(serde_json::json!({
                    "call_id": tool_call.call_id,
                    "name": tool_result.name,
                })),
            });
        }
    }

    Ok(run_result(
        AgentRunStatus::BudgetExceeded,
        messages,
        steps,
        usage,
        cost_microunits,
        output_resource,
    ))
}

fn budget_exhausted_for_followup(
    budget: &AgentRunBudget,
    usage: &AgentUsage,
    cost_microunits: u64,
) -> bool {
    budget
        .max_total_tokens
        .is_some_and(|limit| usage.total_tokens >= limit)
        || budget
            .max_cost_microunits
            .is_some_and(|limit| cost_microunits >= limit)
}

fn exceeds_budget(budget: &AgentRunBudget, usage: &AgentUsage, cost_microunits: u64) -> bool {
    budget
        .max_total_tokens
        .is_some_and(|limit| usage.total_tokens > limit)
        || budget
            .max_cost_microunits
            .is_some_and(|limit| cost_microunits > limit)
}

fn run_result(
    status: AgentRunStatus,
    messages: Vec<AgentMessage>,
    steps: Vec<AgentStepRecord>,
    usage: AgentUsage,
    cost_microunits: u64,
    output_resource: Option<ResourceRef>,
) -> AgentRunResult {
    AgentRunResult {
        status,
        messages,
        steps,
        usage,
        cost_microunits,
        output_resource,
    }
}

fn append_callback(
    task: &Task,
    result: &mut RunnerResult,
    callback_protocol: Option<String>,
    context: Option<serde_json::Value>,
    session_id: Option<String>,
    run_result: AgentRunResult,
) -> RuntimeResult<()> {
    let Some(protocol_id) = callback_protocol else {
        return Ok(());
    };
    if protocol_id.trim().is_empty() || protocol_id == AGENT_RUN_PROTOCOL {
        return Err(runtime_failure(
            PLUGIN_ID,
            &task.task_id,
            AgentError::invalid_input("result_protocol_id must be non-empty and non-recursive"),
        ));
    }
    let mut callback = Task::new(
        format!("{}:result", task.task_id),
        protocol_id,
        serde_json::to_value(AgentRunResultCallback {
            result: run_result,
            context,
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

fn runtime_agent_error(error: mutsuki_runtime_sdk::RuntimeFailure) -> AgentError {
    let runtime = error.error();
    AgentError::new(
        runtime.code.clone(),
        match runtime.evidence.get("message") {
            Some(ScalarValue::String(message)) => message.clone(),
            _ => runtime.route.clone(),
        },
    )
}
