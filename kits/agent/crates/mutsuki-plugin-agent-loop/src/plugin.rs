use mutsuki_agent_contracts::*;
use mutsuki_agent_sdk::{
    AgentContextBuildProtocol, AgentModelGenerateProtocol, AgentModelStreamProtocol,
    AgentRunProtocol, AgentSessionAppendProtocol, AgentSessionGetProtocol,
    AgentToolExecuteProtocol, AgentToolListProtocol, completed_output, orchestration_runner,
    result_event, runtime_failure, task_payload, unsupported_protocol,
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
    let _turn_lease = request
        .session_id
        .as_ref()
        .map(|session_id| {
            let turn_id = request
                .turn_id
                .clone()
                .unwrap_or_else(|| format!("task:{}", ctx.task_id()));
            agent_loop.acquire_turn(session_id, turn_id)
        })
        .transpose()?;
    let advances_turn = !request.messages.is_empty();
    let (persisted_message_count, event_sequence) = if let Some(session_id) = &request.session_id {
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
        (persisted_message_count, session.next_event_sequence)
    } else {
        (0, 0)
    };

    let outcome = ctx
        .call::<AgentContextBuildProtocol>(AgentContextBuildRequest {
            profile_id: request.profile_id.clone(),
            messages: request.messages.clone(),
            session_id: request.session_id.clone(),
            max_context_tokens: request.budget.max_total_tokens,
            metadata: request.metadata.clone(),
        })
        .await
        .map_err(runtime_agent_error)?;
    let context: AgentContext =
        completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
    request.messages = context.messages;
    if let Some(prompt) = context.rendered_prompt
        && !request
            .messages
            .iter()
            .any(|message| message.role == AgentRole::System && message.content == prompt)
    {
        request.messages.insert(0, AgentMessage::system(prompt));
    }

    let turn_id = request
        .turn_id
        .clone()
        .unwrap_or_else(|| format!("turn:ephemeral:{}", event_sequence.saturating_add(1)));
    let mut events = RunEventPublisher::new(
        &ctx,
        request.session_id.clone(),
        turn_id.clone(),
        event_sequence,
    );
    events
        .emit(
            AgentEvent::TurnState {
                turn_id: turn_id.clone(),
                status: "running".into(),
            },
            "turn started",
        )
        .await?;
    let mut result = match execute_run(model, &ctx, &request, &mut events).await {
        Ok(result) => result,
        Err(error) => {
            events
                .emit(
                    AgentEvent::TurnState {
                        turn_id,
                        status: "failed".into(),
                    },
                    "turn failed",
                )
                .await?;
            return Err(error);
        }
    };
    let final_event_start = events.len();
    for approval in &result.pending_approvals {
        events.queue(
            AgentEvent::ApprovalRequest {
                request: approval.clone(),
            },
            "approval requested",
        );
    }
    events.queue(
        AgentEvent::Usage {
            turn_id: turn_id.clone(),
            usage: result.usage.clone(),
        },
        "usage updated",
    );
    if result.status == AgentRunStatus::Completed {
        let summary = result
            .messages
            .iter()
            .rev()
            .find(|message| message.role == AgentRole::Assistant)
            .map(|message| message.content.clone())
            .unwrap_or_default();
        events.queue(
            AgentEvent::FinalResponse {
                turn_id: turn_id.clone(),
                summary,
                result: result.output_resource.clone(),
            },
            "turn completed",
        );
    }
    let status = match result.status {
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::WaitingApproval => "waiting_approval",
        AgentRunStatus::BudgetExceeded => "budget_exceeded",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::Failed => "failed",
    };
    events.queue(
        AgentEvent::TurnState {
            turn_id,
            status: status.into(),
        },
        "turn state changed",
    );
    if let Some(session_id) = &request.session_id {
        let outcome = ctx
            .call::<AgentSessionAppendProtocol>(AgentSessionAppendRequest {
                session_id: session_id.clone(),
                messages: result.messages[persisted_message_count..].to_vec(),
                events: events.after(final_event_start).to_vec(),
                advance_turn: advances_turn,
            })
            .await
            .map_err(runtime_agent_error)?;
        let _: AgentSession =
            completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
    }
    result.events = events.into_events();
    Ok(result)
}

async fn execute_run(
    model: String,
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
    events: &mut RunEventPublisher<'_>,
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

    let mut first_model_step = 0;
    if let Some((tool_calls, pending)) = pending_tool_batch(&messages)? {
        match resolve_approvals(
            &pending,
            &request.permission_decisions,
            request.permission_mode,
        )? {
            ApprovalResolution::Waiting => {
                return Ok(waiting_approval_result(
                    messages,
                    steps,
                    usage,
                    cost_microunits,
                    output_resource,
                    pending,
                ));
            }
            ApprovalResolution::Stopped(status) => {
                return Ok(run_result(
                    status,
                    messages,
                    steps,
                    usage,
                    cost_microunits,
                    output_resource,
                ));
            }
            ApprovalResolution::Ready { approvals, blocked } => {
                let first_new_step = steps.len();
                execute_tool_batch(
                    ctx,
                    request,
                    tool_calls,
                    approvals,
                    blocked,
                    &mut messages,
                    &mut steps,
                    0,
                )
                .await?;
                publish_tool_steps(events, &steps[first_new_step..]).await?;
                first_model_step = 1;
            }
        }
    }

    for model_step in 0..request.max_steps {
        let step_index = first_model_step + model_step;
        events
            .emit(
                AgentEvent::StepState {
                    turn_id: events.turn_id().to_string(),
                    step_index,
                    status: "model_started".into(),
                },
                "model step started",
            )
            .await?;
        let model_request = AgentModelGenerateRequest {
            model: model.clone(),
            messages: messages.clone(),
            temperature: None,
            max_output_tokens: request
                .budget
                .max_total_tokens
                .map(|limit| limit.saturating_sub(usage.total_tokens)),
            provider_hint: request.provider_hint.clone(),
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
        events
            .emit(
                AgentEvent::StepState {
                    turn_id: events.turn_id().to_string(),
                    step_index,
                    status: "model_completed".into(),
                },
                "model step completed",
            )
            .await?;

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

        let tool_calls = generated.tool_calls;
        let pending = approval_requests(ctx, request, &messages, &tool_calls).await?;
        match resolve_approvals(
            &pending,
            &request.permission_decisions,
            request.permission_mode,
        )? {
            ApprovalResolution::Waiting => {
                attach_pending_approvals(&mut messages, &pending)?;
                steps.push(AgentStepRecord {
                    step_index,
                    kind: "approval_requested".into(),
                    detail: Some(serde_json::json!({
                        "actions": pending.iter().map(|request| &request.action_id).collect::<Vec<_>>()
                    })),
                });
                return Ok(waiting_approval_result(
                    messages,
                    steps,
                    usage,
                    cost_microunits,
                    output_resource,
                    pending,
                ));
            }
            ApprovalResolution::Stopped(status) => {
                return Ok(run_result(
                    status,
                    messages,
                    steps,
                    usage,
                    cost_microunits,
                    output_resource,
                ));
            }
            ApprovalResolution::Ready { approvals, blocked } => {
                let first_new_step = steps.len();
                execute_tool_batch(
                    ctx,
                    request,
                    tool_calls,
                    approvals,
                    blocked,
                    &mut messages,
                    &mut steps,
                    step_index,
                )
                .await?;
                publish_tool_steps(events, &steps[first_new_step..]).await?;
            }
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

enum ApprovalResolution {
    Ready {
        approvals: std::collections::BTreeMap<String, AgentToolApproval>,
        blocked: std::collections::BTreeSet<String>,
    },
    Waiting,
    Stopped(AgentRunStatus),
}

async fn approval_requests(
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
    messages: &[AgentMessage],
    tool_calls: &[AgentToolCall],
) -> AgentResult<Vec<PermissionRequest>> {
    let outcome = ctx
        .call::<AgentToolListProtocol>(AgentToolListRequest {
            profile_id: Some(request.profile_id.clone()),
        })
        .await
        .map_err(runtime_agent_error)?;
    let listed: AgentToolListResult =
        completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
    let descriptors = listed
        .tools
        .into_iter()
        .map(|descriptor| (descriptor.name.clone(), descriptor))
        .collect::<std::collections::BTreeMap<_, _>>();
    let session_id = request.session_id.as_deref();
    let transcript_version = messages.len() as u64;
    let mut pending = Vec::new();
    for call in tool_calls {
        let descriptor = descriptors
            .get(&call.name)
            .ok_or_else(|| AgentError::not_found(format!("tool `{}` not registered", call.name)))?;
        if descriptor.requires_approval {
            let session_id = session_id.ok_or_else(|| {
                AgentError::new(
                    "agent.approval.session_required",
                    "approval-bound tools require a durable session",
                )
            })?;
            pending.push(PermissionRequest {
                session_id: session_id.into(),
                turn_id: request
                    .turn_id
                    .clone()
                    .unwrap_or_else(|| format!("turn:{session_id}:{transcript_version}")),
                action_id: call.call_id.clone(),
                tool: call.name.clone(),
                side_effect: descriptor.side_effect.clone(),
                summary: format!("Allow `{}` for this coding action", descriptor.name),
                version: transcript_version,
            });
        }
    }
    Ok(pending)
}

fn resolve_approvals(
    pending: &[PermissionRequest],
    decisions: &[PermissionDecision],
    permission_mode: AgentPermissionMode,
) -> AgentResult<ApprovalResolution> {
    let mut approvals = std::collections::BTreeMap::new();
    let mut blocked = std::collections::BTreeSet::new();
    for request in pending {
        let exact = decisions.iter().find(|decision| {
            decision.session_id == request.session_id
                && decision.turn_id == request.turn_id
                && decision.action_id == request.action_id
                && decision.version == request.version
        });
        let synthesized;
        let decision = if let Some(decision) = exact {
            decision
        } else if permission_mode == AgentPermissionMode::Full
            || (permission_mode == AgentPermissionMode::ReadOnly
                && readonly_side_effect(&request.side_effect))
        {
            synthesized = PermissionDecision {
                session_id: request.session_id.clone(),
                turn_id: request.turn_id.clone(),
                action_id: request.action_id.clone(),
                version: request.version,
                decision: PermissionDecisionKind::Approved,
            };
            &synthesized
        } else if permission_mode == AgentPermissionMode::ReadOnly {
            blocked.insert(request.action_id.clone());
            continue;
        } else {
            if decisions
                .iter()
                .any(|decision| decision.action_id == request.action_id)
            {
                return Err(AgentError::new(
                    "agent.approval.stale",
                    "approval decision does not match the pending session, turn or version",
                ));
            }
            return Ok(ApprovalResolution::Waiting);
        };
        match decision.decision {
            PermissionDecisionKind::Approved => {
                approvals.insert(
                    request.action_id.clone(),
                    AgentToolApproval {
                        request: request.clone(),
                        decision: decision.clone(),
                    },
                );
            }
            PermissionDecisionKind::Cancelled => {
                return Ok(ApprovalResolution::Stopped(AgentRunStatus::Cancelled));
            }
            PermissionDecisionKind::Rejected | PermissionDecisionKind::TimedOut => {
                return Ok(ApprovalResolution::Stopped(AgentRunStatus::Failed));
            }
        }
    }
    Ok(ApprovalResolution::Ready { approvals, blocked })
}

fn readonly_side_effect(side_effect: &ToolSideEffect) -> bool {
    matches!(
        side_effect,
        ToolSideEffect::None | ToolSideEffect::WorkspaceRead | ToolSideEffect::ExternalRead
    )
}

fn attach_pending_approvals(
    messages: &mut [AgentMessage],
    pending: &[PermissionRequest],
) -> AgentResult<()> {
    let message = messages
        .last_mut()
        .filter(|message| message.role == AgentRole::Assistant)
        .ok_or_else(|| AgentError::invalid_input("approval requires an assistant tool call"))?;
    let metadata = message
        .metadata
        .get_or_insert_with(|| serde_json::json!({}));
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| AgentError::invalid_input("assistant metadata must be an object"))?;
    object.insert(
        "pending_approvals".into(),
        serde_json::to_value(pending)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?,
    );
    Ok(())
}

fn pending_tool_batch(
    messages: &[AgentMessage],
) -> AgentResult<Option<(Vec<AgentToolCall>, Vec<PermissionRequest>)>> {
    let Some(metadata) = messages
        .last()
        .filter(|message| message.role == AgentRole::Assistant)
        .and_then(|message| message.metadata.as_ref())
    else {
        return Ok(None);
    };
    let Some(pending) = metadata.get("pending_approvals") else {
        return Ok(None);
    };
    let pending = serde_json::from_value::<Vec<PermissionRequest>>(pending.clone())
        .map_err(|error| AgentError::invalid_input(error.to_string()))?;
    if pending.is_empty() {
        return Ok(None);
    }
    let calls = metadata
        .get("tool_calls")
        .cloned()
        .ok_or_else(|| AgentError::invalid_input("pending approval is missing tool calls"))
        .and_then(|calls| {
            serde_json::from_value::<Vec<AgentToolCall>>(calls)
                .map_err(|error| AgentError::invalid_input(error.to_string()))
        })?;
    Ok(Some((calls, pending)))
}

async fn execute_tool_batch(
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
    tool_calls: Vec<AgentToolCall>,
    mut approvals: std::collections::BTreeMap<String, AgentToolApproval>,
    blocked: std::collections::BTreeSet<String>,
    messages: &mut Vec<AgentMessage>,
    steps: &mut Vec<AgentStepRecord>,
    step_index: u32,
) -> AgentResult<()> {
    let executable = tool_calls
        .iter()
        .filter(|tool_call| !blocked.contains(&tool_call.call_id))
        .cloned()
        .collect::<Vec<_>>();
    let outcomes = ctx
        .call_batch::<AgentToolExecuteProtocol, _>(executable.iter().map(|tool_call| {
            AgentToolExecuteRequest {
                call_id: Some(tool_call.call_id.clone()),
                name: tool_call.name.clone(),
                input: tool_call.input.clone(),
                session_id: request.session_id.clone(),
                approval: approvals.remove(&tool_call.call_id),
                context: request.metadata.clone(),
            }
        }))
        .await
        .map_err(runtime_agent_error)?;
    let mut completed = executable
        .into_iter()
        .zip(outcomes)
        .map(|(tool_call, outcome)| {
            let result: AgentToolExecuteResult =
                completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
            Ok((tool_call.call_id, result))
        })
        .collect::<AgentResult<std::collections::BTreeMap<_, _>>>()?;
    for tool_call in tool_calls {
        let tool_result = if blocked.contains(&tool_call.call_id) {
            AgentToolExecuteResult {
                call_id: Some(tool_call.call_id.clone()),
                name: tool_call.name.clone(),
                output: None,
                output_ref: None,
                error: Some(AgentError::new(
                    "agent.permission.read_only",
                    format!("tool `{}` is blocked by read-only policy", tool_call.name),
                )),
                approved: false,
            }
        } else {
            completed.remove(&tool_call.call_id).ok_or_else(|| {
                AgentError::new(
                    "agent.tool.result_missing",
                    "tool batch result does not match the requested call",
                )
            })?
        };
        if tool_result.call_id.as_deref() != Some(tool_call.call_id.as_str()) {
            return Err(AgentError::new(
                "agent.tool.invalid_result",
                "tool result call_id does not match the requested tool call",
            ));
        }
        if tool_result.error.is_some()
            && (tool_result.output.is_some() || tool_result.output_ref.is_some())
        {
            return Err(AgentError::new(
                "agent.tool.invalid_result",
                "tool result cannot contain both output and error",
            ));
        }
        if tool_result.error.is_none()
            && tool_result.output.is_none()
            && tool_result.output_ref.is_none()
        {
            return Err(AgentError::new(
                "agent.tool.invalid_result",
                "tool result must contain output, output_ref or error",
            ));
        }
        let content = if let Some(error) = &tool_result.error {
            serde_json::to_string(error)
                .map_err(|error| AgentError::invalid_input(error.to_string()))?
        } else if let Some(output) = &tool_result.output {
            output.to_string()
        } else {
            serde_json::json!({"output_ref": tool_result.output_ref}).to_string()
        };
        let metadata = AgentToolResultMetadata {
            call_id: tool_call.call_id.clone(),
            output_ref: tool_result.output_ref.clone(),
            is_error: tool_result.error.is_some(),
            error: tool_result.error.clone(),
        };
        messages.push(AgentMessage {
            role: AgentRole::Tool,
            content,
            name: Some(tool_result.name.clone()),
            metadata: Some(
                serde_json::to_value(metadata)
                    .map_err(|error| AgentError::invalid_input(error.to_string()))?,
            ),
            parts: Vec::new(),
        });
        steps.push(AgentStepRecord {
            step_index,
            kind: if blocked.contains(&tool_call.call_id) {
                "tool_blocked".into()
            } else {
                "tool_execute".into()
            },
            detail: Some(serde_json::json!({
                "call_id": tool_call.call_id,
                "name": tool_result.name,
                "is_error": tool_result.error.is_some(),
            })),
        });
    }
    Ok(())
}

fn waiting_approval_result(
    messages: Vec<AgentMessage>,
    steps: Vec<AgentStepRecord>,
    usage: AgentUsage,
    cost_microunits: u64,
    output_resource: Option<ResourceRef>,
    pending_approvals: Vec<PermissionRequest>,
) -> AgentRunResult {
    let mut result = run_result(
        AgentRunStatus::WaitingApproval,
        messages,
        steps,
        usage,
        cost_microunits,
        output_resource,
    );
    result.pending_approvals = pending_approvals;
    result
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
        pending_approvals: Vec::new(),
        events: Vec::new(),
    }
}

struct RunEventPublisher<'a> {
    ctx: &'a AsyncRunnerContext,
    session_id: Option<String>,
    turn_id: String,
    sequence: u64,
    events: Vec<AgentEventEnvelope>,
}

impl<'a> RunEventPublisher<'a> {
    fn new(
        ctx: &'a AsyncRunnerContext,
        session_id: Option<String>,
        turn_id: String,
        sequence: u64,
    ) -> Self {
        Self {
            ctx,
            session_id,
            turn_id,
            sequence,
            events: Vec::new(),
        }
    }

    fn turn_id(&self) -> &str {
        &self.turn_id
    }

    fn len(&self) -> usize {
        self.events.len()
    }

    fn after(&self, index: usize) -> &[AgentEventEnvelope] {
        &self.events[index..]
    }

    fn into_events(self) -> Vec<AgentEventEnvelope> {
        self.events
    }

    async fn emit(&mut self, event: AgentEvent, summary: &str) -> AgentResult<()> {
        let envelope = self.envelope(event, summary);
        if let Some(session_id) = &self.session_id {
            let outcome = self
                .ctx
                .call::<AgentSessionAppendProtocol>(AgentSessionAppendRequest {
                    session_id: session_id.clone(),
                    messages: Vec::new(),
                    events: vec![envelope.clone()],
                    advance_turn: false,
                })
                .await
                .map_err(runtime_agent_error)?;
            let _: AgentSession = completed_output(PLUGIN_ID, self.ctx.task_id(), outcome)
                .map_err(runtime_agent_error)?;
        }
        self.events.push(envelope);
        Ok(())
    }

    fn queue(&mut self, event: AgentEvent, summary: &str) {
        let envelope = self.envelope(event, summary);
        self.events.push(envelope);
    }

    fn envelope(&mut self, event: AgentEvent, summary: &str) -> AgentEventEnvelope {
        self.sequence = self.sequence.saturating_add(1);
        AgentEventEnvelope {
            session_id: self.session_id.clone().unwrap_or_default(),
            sequence: self.sequence,
            meta: AgentEventMeta::new(format!("{}:{}", self.turn_id, self.sequence), summary)
                .with_turn(&self.turn_id),
            event,
        }
    }
}

async fn publish_tool_steps(
    events: &mut RunEventPublisher<'_>,
    steps: &[AgentStepRecord],
) -> AgentResult<()> {
    for step in steps {
        if !matches!(step.kind.as_str(), "tool_execute" | "tool_blocked") {
            continue;
        }
        let detail = step.detail.as_ref();
        events
            .emit(
                AgentEvent::ToolCallCompleted {
                    turn_id: events.turn_id().to_string(),
                    call_id: detail
                        .and_then(|value| value.get("call_id"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    summary: detail
                        .and_then(|value| value.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("tool")
                        .into(),
                    details: None,
                },
                "tool call completed",
            )
            .await?;
    }
    Ok(())
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
