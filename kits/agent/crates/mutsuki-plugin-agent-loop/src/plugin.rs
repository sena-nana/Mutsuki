use std::time::{SystemTime, UNIX_EPOCH};

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
    let resuming_existing_turn = request.messages.is_empty();
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
    let (mut persisted_message_count, event_sequence, ambiguous_tool_calls) =
        if let Some(session_id) = &request.session_id {
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
            let ambiguous_tool_calls =
                unresolved_started_tool_calls(&session.events, request.turn_id.as_deref());
            validate_resume_request(&session.messages, &request, &ambiguous_tool_calls)?;
            let persisted_message_count = session.messages.len();
            let mut messages = session.messages;
            messages.append(&mut request.messages);
            request.messages = messages;
            (
                persisted_message_count,
                session.next_event_sequence,
                ambiguous_tool_calls,
            )
        } else {
            validate_resume_request(&[], &request, &[])?;
            (0, 0, Vec::new())
        };

    let new_user_messages = request.messages[persisted_message_count..]
        .iter()
        .filter(|message| message.role == AgentRole::User)
        .cloned()
        .collect::<Vec<_>>();

    let turn_id = request
        .turn_id
        .clone()
        .unwrap_or_else(|| format!("turn:ephemeral:{}", event_sequence.saturating_add(1)));
    let compaction = agent_loop
        .context_policy(&request.profile_id)?
        .and_then(|policy| policy.compaction_service)
        .filter(|service_id| !service_id.trim().is_empty())
        .map(|service_id| AgentContextCompactionConfig {
            service_id,
            model: model.clone(),
            provider_hint: request.provider_hint.clone(),
        });

    let outcome = ctx
        .call::<AgentContextBuildProtocol>(AgentContextBuildRequest {
            profile_id: request.profile_id.clone(),
            messages: request.messages.clone(),
            session_id: request.session_id.clone(),
            turn_id: Some(turn_id.clone()),
            max_context_tokens: request.budget.max_context_tokens,
            compaction: if resuming_existing_turn {
                None
            } else {
                compaction.clone()
            },
            metadata: request.metadata.clone(),
        })
        .await
        .map_err(runtime_agent_error)?;
    let context: AgentContext =
        completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
    let preparation_usage = context.preparation_usage.clone();
    let preparation_cost_microunits = context.preparation_cost_microunits;
    let mut model_messages = context.messages;
    if let Some(prompt) = context.rendered_prompt {
        if !request
            .messages
            .iter()
            .any(|message| message.role == AgentRole::System && message.content == prompt)
        {
            request
                .messages
                .insert(0, AgentMessage::system(prompt.clone()));
        }
        if !model_messages
            .iter()
            .any(|message| message.role == AgentRole::System && message.content == prompt)
        {
            model_messages.insert(0, AgentMessage::system(prompt));
        }
    }

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
    for message in new_user_messages {
        events
            .emit(
                AgentEvent::UserMessage {
                    turn_id: turn_id.clone(),
                    content: message.content,
                    metadata: message.metadata,
                },
                "user message",
            )
            .await?;
    }
    let mut result = match execute_run(
        ModelContextRouting {
            model,
            compaction,
            preparation_usage,
            preparation_cost_microunits,
        },
        &ctx,
        &request,
        model_messages,
        &mut events,
        &mut persisted_message_count,
        resuming_existing_turn,
        ambiguous_tool_calls,
    )
    .await
    {
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
    for interaction in &result.pending_interactions {
        events.queue(
            AgentEvent::InteractionRequested {
                turn_id: turn_id.clone(),
                interaction: interaction.clone(),
            },
            "interaction requested",
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
        AgentRunStatus::WaitingInteraction => "waiting_interaction",
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

struct ModelContextRouting {
    model: String,
    compaction: Option<AgentContextCompactionConfig>,
    preparation_usage: AgentUsage,
    preparation_cost_microunits: u64,
}

async fn execute_run(
    routing: ModelContextRouting,
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
    mut model_messages: Vec<AgentMessage>,
    events: &mut RunEventPublisher<'_>,
    persisted_message_count: &mut usize,
    resuming_existing_turn: bool,
    ambiguous_tool_calls: Vec<AgentToolCall>,
) -> AgentResult<AgentRunResult> {
    let ModelContextRouting {
        model,
        compaction,
        preparation_usage,
        preparation_cost_microunits,
    } = routing;
    let mut messages = request.messages.clone();
    let mut model_synced_message_count = if resuming_existing_turn {
        0
    } else {
        messages.len()
    };
    let mut steps = Vec::new();
    if let Some(compaction) = model_messages.iter().find_map(|message| {
        message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("context_compaction"))
            .cloned()
    }) {
        steps.push(AgentStepRecord {
            step_index: 0,
            kind: "context_compaction".into(),
            detail: Some(compaction),
        });
    }
    let mut usage = preparation_usage.clone();
    let mut cost_microunits = preparation_cost_microunits;
    let mut output_resource = None;
    let mut first_model_step = 0;
    let mut max_steps = request.max_steps;
    let mut budget = request.budget.clone();
    if !ambiguous_tool_calls.is_empty() {
        let continuation = run_continuation(&messages)?.unwrap_or_else(|| PendingRunContinuation {
            next_step_index: 1,
            max_steps: request.max_steps,
            budget: request.budget.clone(),
            usage: AgentUsage::default(),
            cost_microunits: 0,
            output_resource: None,
        });
        first_model_step = continuation.next_step_index;
        max_steps = continuation.max_steps;
        budget = continuation.budget.clone();
        usage = continuation.usage.clone();
        usage.add(&preparation_usage);
        cost_microunits = continuation
            .cost_microunits
            .saturating_add(preparation_cost_microunits);
        output_resource = continuation.output_resource.clone();
        let action_step_index = first_model_step.saturating_sub(1);
        let pending = tool_recovery_interactions(request, messages.len(), &ambiguous_tool_calls)?;
        match resolve_interactions(&pending, &request.interaction_resolutions)? {
            InteractionResolutionState::Waiting => {
                return Ok(waiting_interaction_result(
                    messages,
                    steps,
                    usage,
                    cost_microunits,
                    output_resource,
                    pending,
                ));
            }
            InteractionResolutionState::Ready {
                resolutions,
                cancelled,
            } => {
                let appended_from = messages.len();
                append_interaction_results(
                    &mut messages,
                    &ambiguous_tool_calls,
                    &resolutions,
                    &mut steps,
                    action_step_index,
                )?;
                attach_interaction_resume_receipt(
                    &mut messages,
                    &InteractionResumeReceipt {
                        resolutions: resolutions.clone(),
                        continuation: continuation.clone(),
                    },
                )?;
                attach_tool_execution_resume_receipt(
                    &mut messages,
                    &ToolExecutionResumeReceipt {
                        turn_id: events.turn_id().to_owned(),
                        call_ids: ambiguous_tool_calls
                            .iter()
                            .map(|call| call.call_id.clone())
                            .collect(),
                        continuation,
                    },
                )?;
                let mut recovery_events =
                    interaction_resolution_events(events.turn_id(), &resolutions);
                recovery_events.extend(tool_completion_events(
                    events.turn_id(),
                    &ambiguous_tool_calls,
                    "操作状态已由用户确认",
                ));
                events
                    .commit(messages[appended_from..].to_vec(), recovery_events)
                    .await?;
                *persisted_message_count = (*persisted_message_count)
                    .saturating_add(messages.len().saturating_sub(appended_from));
                if cancelled {
                    return Ok(run_result(
                        AgentRunStatus::Cancelled,
                        messages,
                        steps,
                        usage,
                        cost_microunits,
                        output_resource,
                    ));
                }
            }
        }
    } else if resuming_existing_turn
        && let Some(receipt) = tool_execution_resume_receipt(&messages)?
        && request.turn_id.as_deref() == Some(receipt.turn_id.as_str())
    {
        first_model_step = receipt.continuation.next_step_index;
        max_steps = receipt.continuation.max_steps;
        budget = receipt.continuation.budget;
        usage = receipt.continuation.usage;
        usage.add(&preparation_usage);
        cost_microunits = receipt
            .continuation
            .cost_microunits
            .saturating_add(preparation_cost_microunits);
        output_resource = receipt.continuation.output_resource;
    } else if let Some(receipt) = interaction_resume_receipt(&messages)?
        && receipt.resolutions == request.interaction_resolutions
    {
        first_model_step = receipt.continuation.next_step_index;
        max_steps = receipt.continuation.max_steps;
        budget = receipt.continuation.budget;
        usage = receipt.continuation.usage;
        usage.add(&preparation_usage);
        cost_microunits = receipt
            .continuation
            .cost_microunits
            .saturating_add(preparation_cost_microunits);
        output_resource = receipt.continuation.output_resource;
    } else if let Some(pending) = pending_tool_batch(&messages)? {
        let continuation = pending
            .continuation
            .unwrap_or_else(|| PendingRunContinuation {
                next_step_index: 1,
                max_steps: request.max_steps,
                budget: request.budget.clone(),
                usage: AgentUsage::default(),
                cost_microunits: 0,
                output_resource: None,
            });
        first_model_step = continuation.next_step_index;
        max_steps = continuation.max_steps;
        budget = continuation.budget.clone();
        usage = continuation.usage.clone();
        usage.add(&preparation_usage);
        cost_microunits = continuation
            .cost_microunits
            .saturating_add(preparation_cost_microunits);
        output_resource = continuation.output_resource.clone();
        let action_step_index = first_model_step.saturating_sub(1);
        if !pending.interactions.is_empty() {
            match resolve_interactions(&pending.interactions, &request.interaction_resolutions)? {
                InteractionResolutionState::Waiting => {
                    return Ok(waiting_interaction_result(
                        messages,
                        steps,
                        usage,
                        cost_microunits,
                        output_resource,
                        pending.interactions,
                    ));
                }
                InteractionResolutionState::Ready {
                    resolutions,
                    cancelled,
                } => {
                    let appended_from = messages.len();
                    append_interaction_results(
                        &mut messages,
                        &pending.tool_calls,
                        &resolutions,
                        &mut steps,
                        action_step_index,
                    )?;
                    attach_interaction_resume_receipt(
                        &mut messages,
                        &InteractionResumeReceipt {
                            resolutions: resolutions.clone(),
                            continuation: continuation.clone(),
                        },
                    )?;
                    let turn_id = events.turn_id().to_string();
                    events
                        .commit(
                            messages[appended_from..].to_vec(),
                            interaction_resolution_events(&turn_id, &resolutions),
                        )
                        .await?;
                    *persisted_message_count = (*persisted_message_count)
                        .saturating_add(messages.len().saturating_sub(appended_from));
                    if cancelled {
                        return Ok(run_result(
                            AgentRunStatus::Cancelled,
                            messages,
                            steps,
                            usage,
                            cost_microunits,
                            output_resource,
                        ));
                    }
                }
            }
        } else {
            match resolve_approvals(
                &pending.approvals,
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
                        pending.approvals,
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
                    if let Some(pending) = execute_tool_batch_recoverably(
                        ctx,
                        request,
                        RecoverableToolBatch {
                            tool_calls: pending.tool_calls,
                            approvals,
                            blocked,
                            step_index: action_step_index,
                        },
                        &mut messages,
                        &mut steps,
                        events,
                        persisted_message_count,
                        &continuation,
                    )
                    .await?
                    {
                        return Ok(waiting_interaction_result(
                            messages,
                            steps,
                            usage,
                            cost_microunits,
                            output_resource,
                            pending,
                        ));
                    }
                }
            }
        }
    }

    if first_model_step >= max_steps {
        return Ok(run_result(
            AgentRunStatus::BudgetExceeded,
            messages,
            steps,
            usage,
            cost_microunits,
            output_resource,
        ));
    }

    for step_index in first_model_step..max_steps {
        if model_synced_message_count < messages.len() {
            let rebuilt = rebuild_model_context(
                ctx,
                request,
                events.turn_id(),
                compaction.clone(),
                &messages,
            )
            .await?;
            model_messages = rebuilt.messages;
            usage.add(&rebuilt.usage);
            cost_microunits = cost_microunits.saturating_add(rebuilt.cost_microunits);
            if exceeds_budget(&budget, &usage, cost_microunits) {
                return Ok(run_result(
                    AgentRunStatus::BudgetExceeded,
                    messages,
                    steps,
                    usage,
                    cost_microunits,
                    output_resource,
                ));
            }
        }
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
            messages: model_messages.clone(),
            temperature: None,
            max_output_tokens: budget
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
        model_messages.push(assistant.clone());
        messages.push(assistant);
        model_synced_message_count = messages.len();

        if exceeds_budget(&budget, &usage, cost_microunits)
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

        if budget_exhausted_for_followup(&budget, &usage, cost_microunits) {
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
        let pending_interactions =
            interaction_requests(ctx, request, &messages, &tool_calls).await?;
        if !pending_interactions.is_empty() {
            if pending_interactions.len() != tool_calls.len() {
                return Err(AgentError::new(
                    "agent.interaction.mixed_tool_batch",
                    "user interactions cannot be mixed with routed tool calls in one model step",
                ));
            }
            attach_pending_interactions(&mut messages, &pending_interactions)?;
            attach_run_continuation(
                &mut messages,
                &PendingRunContinuation {
                    next_step_index: step_index.saturating_add(1),
                    max_steps,
                    budget: budget.clone(),
                    usage: usage.clone(),
                    cost_microunits,
                    output_resource: output_resource.clone(),
                },
            )?;
            steps.push(AgentStepRecord {
                step_index,
                kind: "interaction_requested".into(),
                detail: Some(serde_json::json!({
                    "interactions": pending_interactions
                        .iter()
                        .map(|interaction| &interaction.interaction_id)
                        .collect::<Vec<_>>()
                })),
            });
            return Ok(waiting_interaction_result(
                messages,
                steps,
                usage,
                cost_microunits,
                output_resource,
                pending_interactions,
            ));
        }
        let pending = approval_requests(ctx, request, &messages, &tool_calls).await?;
        match resolve_approvals(
            &pending,
            &request.permission_decisions,
            request.permission_mode,
        )? {
            ApprovalResolution::Waiting => {
                attach_pending_approvals(&mut messages, &pending)?;
                attach_run_continuation(
                    &mut messages,
                    &PendingRunContinuation {
                        next_step_index: step_index.saturating_add(1),
                        max_steps,
                        budget: budget.clone(),
                        usage: usage.clone(),
                        cost_microunits,
                        output_resource: output_resource.clone(),
                    },
                )?;
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
                let continuation = PendingRunContinuation {
                    next_step_index: step_index.saturating_add(1),
                    max_steps,
                    budget: budget.clone(),
                    usage: usage.clone(),
                    cost_microunits,
                    output_resource: output_resource.clone(),
                };
                if let Some(pending) = execute_tool_batch_recoverably(
                    ctx,
                    request,
                    RecoverableToolBatch {
                        tool_calls,
                        approvals,
                        blocked,
                        step_index,
                    },
                    &mut messages,
                    &mut steps,
                    events,
                    persisted_message_count,
                    &continuation,
                )
                .await?
                {
                    return Ok(waiting_interaction_result(
                        messages,
                        steps,
                        usage,
                        cost_microunits,
                        output_resource,
                        pending,
                    ));
                }
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

#[derive(Debug)]
enum InteractionResolutionState {
    Waiting,
    Ready {
        resolutions: Vec<InteractionResolution>,
        cancelled: bool,
    },
}

async fn interaction_requests(
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
    messages: &[AgentMessage],
    tool_calls: &[AgentToolCall],
) -> AgentResult<Vec<InteractionRequest>> {
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
        let AgentToolExecution::Interaction { interaction_kind } = &descriptor.execution else {
            continue;
        };
        let session_id = session_id.ok_or_else(|| {
            AgentError::new(
                "agent.interaction.session_required",
                "user interactions require a durable session",
            )
        })?;
        let turn_id = request
            .turn_id
            .clone()
            .unwrap_or_else(|| format!("turn:{session_id}:{transcript_version}"));
        pending.push(InteractionRequest {
            session_id: session_id.to_owned(),
            turn_id,
            version: transcript_version,
            interaction_id: call.call_id.clone(),
            kind: interaction_kind.clone(),
            source_tool: Some(call.name.clone()),
            permission_mode: request.permission_mode,
            prompt: interaction_prompt(interaction_kind, &call.input),
            options: call.input.clone(),
            context: request.metadata.clone(),
            details: None,
        });
    }
    Ok(pending)
}

fn interaction_prompt(kind: &InteractionKind, input: &serde_json::Value) -> String {
    let direct = ["prompt", "question", "title"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(prompt) = direct {
        return prompt.to_owned();
    }
    if let Some(prompt) = input
        .get("questions")
        .and_then(serde_json::Value::as_array)
        .and_then(|questions| questions.first())
        .and_then(|question| question.get("question").or_else(|| question.get("text")))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return prompt.to_owned();
    }
    if matches!(kind, InteractionKind::PlanConfirm)
        && let Some(plan) = input
            .get("plan")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        return format!("确认执行以下计划？\n\n{plan}");
    }
    match kind {
        InteractionKind::PlanConfirm => "确认执行当前计划？".into(),
        _ => "需要你的输入才能继续。".into(),
    }
}

fn tool_recovery_interactions(
    request: &AgentRunRequest,
    transcript_version: usize,
    tool_calls: &[AgentToolCall],
) -> AgentResult<Vec<InteractionRequest>> {
    let session_id = request.session_id.as_deref().ok_or_else(|| {
        AgentError::new(
            "agent.tool.recovery_session_required",
            "interrupted tool recovery requires a durable session",
        )
    })?;
    let turn_id = request.turn_id.as_deref().ok_or_else(|| {
        AgentError::new(
            "agent.tool.recovery_turn_required",
            "interrupted tool recovery requires a durable turn id",
        )
    })?;
    Ok(tool_calls
        .iter()
        .map(|call| InteractionRequest {
            session_id: session_id.to_owned(),
            turn_id: turn_id.to_owned(),
            version: transcript_version as u64,
            interaction_id: call.call_id.clone(),
            kind: InteractionKind::Custom,
            source_tool: Some(call.name.clone()),
            permission_mode: request.permission_mode,
            prompt: format!(
                "上次运行在“{}”确认结果前中断。请先检查操作结果，系统不会自动重复执行。",
                call.name
            ),
            options: serde_json::json!({
                "recovery": "ambiguous_tool_execution",
                "tool": call.name,
                "choices": [
                    {
                        "label": "操作已完成",
                        "value": { "status": "completed" }
                    },
                    {
                        "label": "确认未完成，允许重新执行",
                        "value": { "status": "retry_allowed" }
                    }
                ]
            }),
            context: request.metadata.clone(),
            details: None,
        })
        .collect())
}

fn resolve_interactions(
    pending: &[InteractionRequest],
    decisions: &[InteractionResolution],
) -> AgentResult<InteractionResolutionState> {
    let mut seen = std::collections::BTreeSet::new();
    if decisions
        .iter()
        .any(|decision| !seen.insert(decision.interaction_id.as_str()))
    {
        return Err(AgentError::new(
            "agent.interaction.duplicate_resolution",
            "an interaction may be resolved only once per resume request",
        ));
    }
    if decisions.iter().any(|decision| {
        !pending.iter().any(|request| {
            request.session_id == decision.session_id
                && request.turn_id == decision.turn_id
                && request.version == decision.version
                && request.interaction_id == decision.interaction_id
        })
    }) {
        return Err(AgentError::new(
            "agent.interaction.stale",
            "interaction resolution does not match the pending session, turn or version",
        ));
    }
    let mut resolutions = Vec::with_capacity(pending.len());
    for request in pending {
        let Some(decision) = decisions.iter().find(|decision| {
            request.session_id == decision.session_id
                && request.turn_id == decision.turn_id
                && request.version == decision.version
                && request.interaction_id == decision.interaction_id
        }) else {
            return Ok(InteractionResolutionState::Waiting);
        };
        resolutions.push(decision.clone());
    }
    let cancelled = resolutions.iter().any(|resolution| !resolution.accepted);
    Ok(InteractionResolutionState::Ready {
        resolutions,
        cancelled,
    })
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
            PermissionDecisionKind::Rejected => {
                return Ok(ApprovalResolution::Stopped(AgentRunStatus::Cancelled));
            }
            PermissionDecisionKind::TimedOut => {
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

fn attach_pending_interactions(
    messages: &mut [AgentMessage],
    pending: &[InteractionRequest],
) -> AgentResult<()> {
    let message = messages
        .last_mut()
        .filter(|message| message.role == AgentRole::Assistant)
        .ok_or_else(|| AgentError::invalid_input("interaction requires an assistant tool call"))?;
    let metadata = message
        .metadata
        .get_or_insert_with(|| serde_json::json!({}));
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| AgentError::invalid_input("assistant metadata must be an object"))?;
    object.insert(
        "pending_interactions".into(),
        serde_json::to_value(pending)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?,
    );
    Ok(())
}

struct PendingToolBatch {
    tool_calls: Vec<AgentToolCall>,
    approvals: Vec<PermissionRequest>,
    interactions: Vec<InteractionRequest>,
    continuation: Option<PendingRunContinuation>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PendingRunContinuation {
    next_step_index: u32,
    max_steps: u32,
    budget: AgentRunBudget,
    usage: AgentUsage,
    cost_microunits: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_resource: Option<ResourceRef>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct InteractionResumeReceipt {
    resolutions: Vec<InteractionResolution>,
    continuation: PendingRunContinuation,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ToolExecutionResumeReceipt {
    turn_id: String,
    call_ids: Vec<String>,
    continuation: PendingRunContinuation,
}

fn unresolved_started_tool_calls(
    events: &[AgentEventEnvelope],
    turn_id: Option<&str>,
) -> Vec<AgentToolCall> {
    let mut pending = std::collections::BTreeMap::new();
    for envelope in events
        .iter()
        .filter(|event| event.meta.turn_id.as_deref() == turn_id)
    {
        match &envelope.event {
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                input,
                ..
            } => {
                pending.insert(
                    call_id.clone(),
                    AgentToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                );
            }
            AgentEvent::ToolCallCompleted { call_id, .. } => {
                pending.remove(call_id);
            }
            _ => {}
        }
    }
    pending.into_values().collect()
}

fn run_continuation(messages: &[AgentMessage]) -> AgentResult<Option<PendingRunContinuation>> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == AgentRole::Assistant)
        .and_then(|message| message.metadata.as_ref())
        .and_then(|metadata| metadata.get("run_continuation"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| AgentError::invalid_input(error.to_string()))
}

fn tool_execution_resume_receipt(
    messages: &[AgentMessage],
) -> AgentResult<Option<ToolExecutionResumeReceipt>> {
    messages
        .last()
        .filter(|message| message.role == AgentRole::Tool)
        .and_then(|message| message.metadata.as_ref())
        .and_then(|metadata| metadata.get("tool_execution_resume_receipt"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| AgentError::invalid_input(error.to_string()))
}

fn attach_tool_execution_resume_receipt(
    messages: &mut [AgentMessage],
    receipt: &ToolExecutionResumeReceipt,
) -> AgentResult<()> {
    let metadata = messages
        .last_mut()
        .filter(|message| message.role == AgentRole::Tool)
        .and_then(|message| message.metadata.as_mut())
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| AgentError::invalid_input("tool result metadata must be an object"))?;
    metadata.insert(
        "tool_execution_resume_receipt".into(),
        serde_json::to_value(receipt)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?,
    );
    Ok(())
}

fn interaction_resume_receipt(
    messages: &[AgentMessage],
) -> AgentResult<Option<InteractionResumeReceipt>> {
    messages
        .last()
        .filter(|message| message.role == AgentRole::Tool)
        .and_then(|message| message.metadata.as_ref())
        .and_then(|metadata| metadata.get("interaction_resume_receipt"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| AgentError::invalid_input(error.to_string()))
}

fn attach_interaction_resume_receipt(
    messages: &mut [AgentMessage],
    receipt: &InteractionResumeReceipt,
) -> AgentResult<()> {
    let metadata = messages
        .last_mut()
        .filter(|message| message.role == AgentRole::Tool)
        .and_then(|message| message.metadata.as_mut())
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            AgentError::invalid_input("interaction result metadata must be an object")
        })?;
    metadata.insert(
        "interaction_resume_receipt".into(),
        serde_json::to_value(receipt)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?,
    );
    Ok(())
}

fn attach_run_continuation(
    messages: &mut [AgentMessage],
    continuation: &PendingRunContinuation,
) -> AgentResult<()> {
    let metadata = messages
        .last_mut()
        .filter(|message| message.role == AgentRole::Assistant)
        .and_then(|message| message.metadata.as_mut())
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| AgentError::invalid_input("pending action metadata must be an object"))?;
    metadata.insert(
        "run_continuation".into(),
        serde_json::to_value(continuation)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?,
    );
    Ok(())
}

fn validate_resume_request(
    persisted_messages: &[AgentMessage],
    request: &AgentRunRequest,
    ambiguous_tool_calls: &[AgentToolCall],
) -> AgentResult<()> {
    if !ambiguous_tool_calls.is_empty() {
        if !request.messages.is_empty() {
            return Err(AgentError::new(
                "agent.tool.recovery_messages_not_allowed",
                "an interrupted tool batch must be resolved before appending new user messages",
            ));
        }
        if !request.permission_decisions.is_empty() {
            return Err(AgentError::new(
                "agent.tool.recovery_invalid_resume",
                "an interrupted tool batch cannot be resumed with permission decisions",
            ));
        }
        return Ok(());
    }
    let Some(pending) = pending_tool_batch(persisted_messages)? else {
        if !request.interaction_resolutions.is_empty() {
            if let Some(receipt) = interaction_resume_receipt(persisted_messages)? {
                if receipt.resolutions == request.interaction_resolutions
                    && request.messages.is_empty()
                    && receipt.resolutions.iter().all(|resolution| {
                        request.turn_id.as_deref() == Some(resolution.turn_id.as_str())
                            && request.session_id.as_deref() == Some(resolution.session_id.as_str())
                    })
                {
                    return Ok(());
                }
                return Err(AgentError::new(
                    "agent.interaction.idempotency_conflict",
                    "interaction retry does not match the committed resolution receipt",
                ));
            }
            return Err(AgentError::new(
                "agent.interaction.not_pending",
                "interaction resolutions require a pending interaction",
            ));
        }
        if !request.permission_decisions.is_empty() {
            return Err(AgentError::new(
                "agent.approval.not_pending",
                "permission decisions require a pending approval",
            ));
        }
        return Ok(());
    };
    if !request.messages.is_empty() {
        return Err(AgentError::new(
            "agent.run.resume_messages_not_allowed",
            "a waiting turn must be resumed without appending new user messages",
        ));
    }
    if !pending.interactions.is_empty() {
        if !request.permission_decisions.is_empty() {
            return Err(AgentError::new(
                "agent.interaction.invalid_resume",
                "an interaction cannot be resumed with permission decisions",
            ));
        }
        let turn_id = &pending.interactions[0].turn_id;
        if request.turn_id.as_deref() != Some(turn_id.as_str())
            || pending
                .interactions
                .iter()
                .any(|interaction| interaction.turn_id != *turn_id)
        {
            return Err(AgentError::new(
                "agent.interaction.stale",
                "interaction resume must bind the pending turn",
            ));
        }
    } else {
        if !request.interaction_resolutions.is_empty() {
            return Err(AgentError::new(
                "agent.approval.invalid_resume",
                "an approval cannot be resumed with interaction resolutions",
            ));
        }
        let turn_id = &pending.approvals[0].turn_id;
        if request.turn_id.as_deref() != Some(turn_id.as_str())
            || pending
                .approvals
                .iter()
                .any(|approval| approval.turn_id != *turn_id)
        {
            return Err(AgentError::new(
                "agent.approval.stale",
                "approval resume must bind the pending turn",
            ));
        }
    }
    Ok(())
}

fn pending_tool_batch(messages: &[AgentMessage]) -> AgentResult<Option<PendingToolBatch>> {
    let Some(metadata) = messages
        .last()
        .filter(|message| message.role == AgentRole::Assistant)
        .and_then(|message| message.metadata.as_ref())
    else {
        return Ok(None);
    };
    let approvals = metadata
        .get("pending_approvals")
        .cloned()
        .map(serde_json::from_value::<Vec<PermissionRequest>>)
        .transpose()
        .map_err(|error| AgentError::invalid_input(error.to_string()))?
        .unwrap_or_default();
    let interactions = metadata
        .get("pending_interactions")
        .cloned()
        .map(serde_json::from_value::<Vec<InteractionRequest>>)
        .transpose()
        .map_err(|error| AgentError::invalid_input(error.to_string()))?
        .unwrap_or_default();
    let continuation = metadata
        .get("run_continuation")
        .cloned()
        .map(serde_json::from_value::<PendingRunContinuation>)
        .transpose()
        .map_err(|error| AgentError::invalid_input(error.to_string()))?;
    if approvals.is_empty() && interactions.is_empty() {
        return Ok(None);
    }
    if !approvals.is_empty() && !interactions.is_empty() {
        return Err(AgentError::new(
            "agent.interaction.invalid_pending_batch",
            "one assistant tool batch cannot wait for approval and user interaction together",
        ));
    }
    let calls = metadata
        .get("tool_calls")
        .cloned()
        .ok_or_else(|| AgentError::invalid_input("pending action is missing tool calls"))
        .and_then(|calls| {
            serde_json::from_value::<Vec<AgentToolCall>>(calls)
                .map_err(|error| AgentError::invalid_input(error.to_string()))
        })?;
    Ok(Some(PendingToolBatch {
        tool_calls: calls,
        approvals,
        interactions,
        continuation,
    }))
}

fn append_interaction_results(
    messages: &mut Vec<AgentMessage>,
    tool_calls: &[AgentToolCall],
    resolutions: &[InteractionResolution],
    steps: &mut Vec<AgentStepRecord>,
    step_index: u32,
) -> AgentResult<()> {
    if tool_calls.len() != resolutions.len() {
        return Err(AgentError::new(
            "agent.interaction.result_missing",
            "every pending interaction must have one matching resolution",
        ));
    }
    for call in tool_calls {
        let resolution = resolutions
            .iter()
            .find(|resolution| resolution.interaction_id == call.call_id)
            .ok_or_else(|| {
                AgentError::new(
                    "agent.interaction.result_missing",
                    "interaction resolution does not match the requested tool call",
                )
            })?;
        let error = (!resolution.accepted).then(|| {
            AgentError::new(
                "agent.interaction.cancelled",
                "the user cancelled this interaction",
            )
        });
        let content = if resolution.accepted {
            resolution.response.to_string()
        } else {
            serde_json::json!({
                "cancelled": true,
                "response": resolution.response,
            })
            .to_string()
        };
        messages.push(AgentMessage {
            role: AgentRole::Tool,
            content,
            name: Some(call.name.clone()),
            metadata: Some(
                serde_json::to_value(AgentToolResultMetadata {
                    call_id: call.call_id.clone(),
                    output_ref: None,
                    is_error: error.is_some(),
                    error,
                })
                .map_err(|error| AgentError::invalid_input(error.to_string()))?,
            ),
            parts: Vec::new(),
        });
        steps.push(AgentStepRecord {
            step_index,
            kind: if resolution.accepted {
                "interaction_resolved".into()
            } else {
                "interaction_cancelled".into()
            },
            detail: Some(serde_json::json!({
                "interaction_id": resolution.interaction_id,
                "accepted": resolution.accepted,
            })),
        });
    }
    Ok(())
}

struct RecoverableToolBatch {
    tool_calls: Vec<AgentToolCall>,
    approvals: std::collections::BTreeMap<String, AgentToolApproval>,
    blocked: std::collections::BTreeSet<String>,
    step_index: u32,
}

async fn execute_tool_batch_recoverably(
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
    batch: RecoverableToolBatch,
    messages: &mut Vec<AgentMessage>,
    steps: &mut Vec<AgentStepRecord>,
    events: &mut RunEventPublisher<'_>,
    persisted_message_count: &mut usize,
    continuation: &PendingRunContinuation,
) -> AgentResult<Option<Vec<InteractionRequest>>> {
    let RecoverableToolBatch {
        tool_calls,
        approvals,
        blocked,
        step_index,
    } = batch;
    attach_run_continuation(messages, continuation)?;
    let blocked_calls = tool_calls
        .iter()
        .filter(|call| blocked.contains(&call.call_id))
        .cloned()
        .collect::<Vec<_>>();
    let executable = tool_calls
        .into_iter()
        .filter(|call| !blocked.contains(&call.call_id))
        .collect::<Vec<_>>();
    append_blocked_tool_results(&blocked_calls, messages, steps, step_index)?;
    if executable.is_empty() && !blocked_calls.is_empty() {
        attach_tool_execution_resume_receipt(
            messages,
            &ToolExecutionResumeReceipt {
                turn_id: events.turn_id().to_owned(),
                call_ids: blocked_calls
                    .iter()
                    .map(|call| call.call_id.clone())
                    .collect(),
                continuation: continuation.clone(),
            },
        )?;
    }

    let mut boundary_events =
        tool_completion_events(events.turn_id(), &blocked_calls, "操作已被权限策略阻止");
    boundary_events.extend(tool_start_events(events.turn_id(), &executable));
    events
        .commit(
            messages[*persisted_message_count..].to_vec(),
            boundary_events,
        )
        .await?;
    *persisted_message_count = messages.len();
    if executable.is_empty() {
        return Ok(None);
    }

    let appended_from = messages.len();
    let steps_from = steps.len();
    if let Err(_error) = execute_tool_batch(
        ctx,
        request,
        executable.clone(),
        approvals,
        std::collections::BTreeSet::new(),
        messages,
        steps,
        step_index,
    )
    .await
    {
        messages.truncate(appended_from);
        steps.truncate(steps_from);
        return tool_recovery_interactions(request, messages.len(), &executable).map(Some);
    }
    attach_tool_execution_resume_receipt(
        messages,
        &ToolExecutionResumeReceipt {
            turn_id: events.turn_id().to_owned(),
            call_ids: executable.iter().map(|call| call.call_id.clone()).collect(),
            continuation: continuation.clone(),
        },
    )?;
    events
        .commit(
            messages[appended_from..].to_vec(),
            tool_completion_events(events.turn_id(), &executable, "操作已完成"),
        )
        .await?;
    *persisted_message_count = messages.len();
    Ok(None)
}

fn append_blocked_tool_results(
    tool_calls: &[AgentToolCall],
    messages: &mut Vec<AgentMessage>,
    steps: &mut Vec<AgentStepRecord>,
    step_index: u32,
) -> AgentResult<()> {
    for tool_call in tool_calls {
        let error = AgentError::new(
            "agent.permission.read_only",
            format!("tool `{}` is blocked by read-only policy", tool_call.name),
        );
        messages.push(AgentMessage {
            role: AgentRole::Tool,
            content: serde_json::to_string(&error)
                .map_err(|error| AgentError::invalid_input(error.to_string()))?,
            name: Some(tool_call.name.clone()),
            metadata: Some(
                serde_json::to_value(AgentToolResultMetadata {
                    call_id: tool_call.call_id.clone(),
                    output_ref: None,
                    is_error: true,
                    error: Some(error),
                })
                .map_err(|error| AgentError::invalid_input(error.to_string()))?,
            ),
            parts: Vec::new(),
        });
        steps.push(AgentStepRecord {
            step_index,
            kind: "tool_blocked".into(),
            detail: Some(serde_json::json!({
                "call_id": tool_call.call_id,
                "name": tool_call.name,
                "is_error": true,
            })),
        });
    }
    Ok(())
}

fn tool_start_events(turn_id: &str, tool_calls: &[AgentToolCall]) -> Vec<(AgentEvent, String)> {
    tool_calls
        .iter()
        .map(|call| {
            (
                AgentEvent::ToolCallStarted {
                    turn_id: turn_id.to_owned(),
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                },
                "tool call started".to_owned(),
            )
        })
        .collect()
}

fn interaction_resolution_events(
    turn_id: &str,
    resolutions: &[InteractionResolution],
) -> Vec<(AgentEvent, String)> {
    resolutions
        .iter()
        .cloned()
        .map(|resolution| {
            (
                AgentEvent::InteractionResolved {
                    turn_id: turn_id.to_owned(),
                    resolution,
                },
                "interaction resolved".to_owned(),
            )
        })
        .collect()
}

fn tool_completion_events(
    turn_id: &str,
    tool_calls: &[AgentToolCall],
    summary: &str,
) -> Vec<(AgentEvent, String)> {
    tool_calls
        .iter()
        .map(|call| {
            (
                AgentEvent::ToolCallCompleted {
                    turn_id: turn_id.to_owned(),
                    call_id: call.call_id.clone(),
                    summary: call.name.clone(),
                    details: None,
                },
                summary.to_owned(),
            )
        })
        .collect()
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

fn waiting_interaction_result(
    messages: Vec<AgentMessage>,
    steps: Vec<AgentStepRecord>,
    usage: AgentUsage,
    cost_microunits: u64,
    output_resource: Option<ResourceRef>,
    pending_interactions: Vec<InteractionRequest>,
) -> AgentRunResult {
    let mut result = run_result(
        AgentRunStatus::WaitingInteraction,
        messages,
        steps,
        usage,
        cost_microunits,
        output_resource,
    );
    result.pending_interactions = pending_interactions;
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

struct PreparedModelContext {
    messages: Vec<AgentMessage>,
    usage: AgentUsage,
    cost_microunits: u64,
}

async fn rebuild_model_context(
    ctx: &AsyncRunnerContext,
    request: &AgentRunRequest,
    turn_id: &str,
    compaction: Option<AgentContextCompactionConfig>,
    messages: &[AgentMessage],
) -> AgentResult<PreparedModelContext> {
    let outcome = ctx
        .call::<AgentContextBuildProtocol>(AgentContextBuildRequest {
            profile_id: request.profile_id.clone(),
            messages: messages.to_vec(),
            session_id: request.session_id.clone(),
            turn_id: Some(turn_id.to_owned()),
            max_context_tokens: request.budget.max_context_tokens,
            compaction,
            metadata: request.metadata.clone(),
        })
        .await
        .map_err(runtime_agent_error)?;
    let context: AgentContext =
        completed_output(PLUGIN_ID, ctx.task_id(), outcome).map_err(runtime_agent_error)?;
    let mut model_messages = context.messages;
    if let Some(prompt) = context.rendered_prompt
        && !model_messages
            .iter()
            .any(|message| message.role == AgentRole::System && message.content == prompt)
    {
        model_messages.insert(0, AgentMessage::system(prompt));
    }
    Ok(PreparedModelContext {
        messages: model_messages,
        usage: context.preparation_usage,
        cost_microunits: context.preparation_cost_microunits,
    })
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
        pending_interactions: Vec::new(),
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
        self.commit(Vec::new(), vec![(event, summary.to_string())])
            .await
    }

    async fn commit(
        &mut self,
        messages: Vec<AgentMessage>,
        events: Vec<(AgentEvent, String)>,
    ) -> AgentResult<()> {
        let envelopes = events
            .into_iter()
            .map(|(event, summary)| self.envelope(event, &summary))
            .collect::<Vec<_>>();
        if let Some(session_id) = &self.session_id {
            let outcome = self
                .ctx
                .call::<AgentSessionAppendProtocol>(AgentSessionAppendRequest {
                    session_id: session_id.clone(),
                    messages,
                    events: envelopes.clone(),
                    advance_turn: false,
                })
                .await
                .map_err(runtime_agent_error)?;
            let _: AgentSession = completed_output(PLUGIN_ID, self.ctx.task_id(), outcome)
                .map_err(runtime_agent_error)?;
        }
        self.events.extend(envelopes);
        Ok(())
    }

    fn queue(&mut self, event: AgentEvent, summary: &str) {
        let envelope = self.envelope(event, summary);
        self.events.push(envelope);
    }

    fn envelope(&mut self, event: AgentEvent, summary: &str) -> AgentEventEnvelope {
        self.sequence = self.sequence.saturating_add(1);
        let mut meta = AgentEventMeta::new(format!("{}:{}", self.turn_id, self.sequence), summary)
            .with_turn(&self.turn_id);
        meta.timestamp_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        AgentEventEnvelope {
            session_id: self.session_id.clone().unwrap_or_default(),
            sequence: self.sequence,
            meta,
            event,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn interaction(id: &str) -> InteractionRequest {
        InteractionRequest {
            session_id: "session".into(),
            turn_id: "turn".into(),
            version: 3,
            interaction_id: id.into(),
            kind: InteractionKind::Clarification,
            source_tool: Some("ask_user_question".into()),
            permission_mode: AgentPermissionMode::Ask,
            prompt: "Which target?".into(),
            options: serde_json::json!({ "options": ["A", "B"] }),
            context: None,
            details: None,
        }
    }

    fn resolution(id: &str, accepted: bool) -> InteractionResolution {
        InteractionResolution {
            session_id: "session".into(),
            turn_id: "turn".into(),
            version: 3,
            interaction_id: id.into(),
            accepted,
            response: serde_json::json!({ "answer": "A" }),
        }
    }

    fn waiting_interaction_messages() -> Vec<AgentMessage> {
        let call = AgentToolCall {
            call_id: "ask-1".into(),
            name: "ask_user_question".into(),
            input: serde_json::json!({ "question": "Which target?" }),
        };
        let mut assistant = AgentMessage::assistant(String::new());
        assistant.metadata = Some(serde_json::json!({ "tool_calls": [call] }));
        let mut messages = vec![assistant];
        attach_pending_interactions(&mut messages, &[interaction("ask-1")]).unwrap();
        messages
    }

    #[test]
    fn rejected_permission_is_a_user_cancellation_while_timeout_is_a_failure() {
        let pending = PermissionRequest {
            session_id: "session".into(),
            turn_id: "turn".into(),
            action_id: "write-1".into(),
            tool: "workspace.write".into(),
            side_effect: ToolSideEffect::WorkspaceWrite,
            summary: "Allow write".into(),
            version: 2,
        };
        let decision = |decision| PermissionDecision {
            session_id: pending.session_id.clone(),
            turn_id: pending.turn_id.clone(),
            action_id: pending.action_id.clone(),
            version: pending.version,
            decision,
        };

        assert!(matches!(
            resolve_approvals(
                std::slice::from_ref(&pending),
                &[decision(PermissionDecisionKind::Rejected)],
                AgentPermissionMode::Ask
            )
            .unwrap(),
            ApprovalResolution::Stopped(AgentRunStatus::Cancelled)
        ));
        assert!(matches!(
            resolve_approvals(
                std::slice::from_ref(&pending),
                &[decision(PermissionDecisionKind::TimedOut)],
                AgentPermissionMode::Ask
            )
            .unwrap(),
            ApprovalResolution::Stopped(AgentRunStatus::Failed)
        ));
    }

    #[test]
    fn interaction_resume_requires_every_exactly_bound_resolution() {
        let pending = vec![interaction("ask-1"), interaction("ask-2")];
        assert!(matches!(
            resolve_interactions(&pending, &[resolution("ask-1", true)]).unwrap(),
            InteractionResolutionState::Waiting
        ));

        let mut stale = resolution("ask-1", true);
        stale.turn_id = "other-turn".into();
        assert_eq!(
            resolve_interactions(&pending, &[stale]).unwrap_err().code,
            "agent.interaction.stale"
        );

        let resolved = resolve_interactions(
            &pending,
            &[resolution("ask-1", true), resolution("ask-2", false)],
        )
        .unwrap();
        let InteractionResolutionState::Ready {
            resolutions,
            cancelled,
        } = resolved
        else {
            panic!("all pending interactions should resolve");
        };
        assert_eq!(resolutions.len(), 2);
        assert!(cancelled);
    }

    #[test]
    fn waiting_interaction_rejects_wrong_turn_new_messages_and_orphan_resolutions() {
        let messages = waiting_interaction_messages();
        let mut request = AgentRunRequest::new("profile", Vec::new());
        request.session_id = Some("session".into());
        request.turn_id = Some("turn".into());
        request.interaction_resolutions = vec![resolution("ask-1", true)];
        validate_resume_request(&messages, &request, &[]).unwrap();

        request.turn_id = Some("other-turn".into());
        assert_eq!(
            validate_resume_request(&messages, &request, &[])
                .unwrap_err()
                .code,
            "agent.interaction.stale"
        );
        request.turn_id = Some("turn".into());
        request.messages = vec![AgentMessage::user("start a new turn")];
        assert_eq!(
            validate_resume_request(&messages, &request, &[])
                .unwrap_err()
                .code,
            "agent.run.resume_messages_not_allowed"
        );
        request.messages.clear();
        assert_eq!(
            validate_resume_request(&[], &request, &[])
                .unwrap_err()
                .code,
            "agent.interaction.not_pending"
        );
    }

    #[test]
    fn committed_interaction_receipt_allows_only_identical_retry() {
        let mut messages = waiting_interaction_messages();
        let resolutions = vec![resolution("ask-1", true)];
        let mut steps = Vec::new();
        append_interaction_results(
            &mut messages,
            &[AgentToolCall {
                call_id: "ask-1".into(),
                name: "ask_user_question".into(),
                input: serde_json::json!({ "question": "Which target?" }),
            }],
            &resolutions,
            &mut steps,
            2,
        )
        .unwrap();
        attach_interaction_resume_receipt(
            &mut messages,
            &InteractionResumeReceipt {
                resolutions: resolutions.clone(),
                continuation: PendingRunContinuation {
                    next_step_index: 3,
                    max_steps: 5,
                    budget: AgentRunBudget::default(),
                    usage: AgentUsage::default(),
                    cost_microunits: 0,
                    output_resource: None,
                },
            },
        )
        .unwrap();

        let mut request = AgentRunRequest::new("profile", Vec::new());
        request.session_id = Some("session".into());
        request.turn_id = Some("turn".into());
        request.interaction_resolutions = resolutions;
        validate_resume_request(&messages, &request, &[]).unwrap();
        assert_eq!(
            interaction_resume_receipt(&messages)
                .unwrap()
                .unwrap()
                .continuation
                .next_step_index,
            3
        );

        request.interaction_resolutions[0].response = serde_json::json!({ "answer": "B" });
        assert_eq!(
            validate_resume_request(&messages, &request, &[])
                .unwrap_err()
                .code,
            "agent.interaction.idempotency_conflict"
        );
    }

    #[test]
    fn unresolved_tool_start_is_scoped_by_turn_and_closed_only_by_matching_completion() {
        let event = |sequence, turn_id: &str, event| AgentEventEnvelope {
            session_id: "session".into(),
            sequence,
            meta: AgentEventMeta::new(format!("event-{sequence}"), "tool").with_turn(turn_id),
            event,
        };
        let events = vec![
            event(
                1,
                "turn-a",
                AgentEvent::ToolCallStarted {
                    turn_id: "turn-a".into(),
                    call_id: "call-a".into(),
                    name: "shell".into(),
                    input: serde_json::json!({"command": "publish"}),
                },
            ),
            event(
                2,
                "turn-b",
                AgentEvent::ToolCallStarted {
                    turn_id: "turn-b".into(),
                    call_id: "call-b".into(),
                    name: "read".into(),
                    input: serde_json::json!({"path": "README.md"}),
                },
            ),
            event(
                3,
                "turn-b",
                AgentEvent::ToolCallCompleted {
                    turn_id: "turn-b".into(),
                    call_id: "call-b".into(),
                    summary: "read".into(),
                    details: None,
                },
            ),
        ];

        let pending = unresolved_started_tool_calls(&events, Some("turn-a"));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].call_id, "call-a");
        assert!(unresolved_started_tool_calls(&events, Some("turn-b")).is_empty());
    }

    #[test]
    fn interrupted_tool_batch_rejects_new_messages_and_exposes_recovery_choices() {
        let call = AgentToolCall {
            call_id: "call-side-effect".into(),
            name: "shell".into(),
            input: serde_json::json!({"command": "publish"}),
        };
        let mut request = AgentRunRequest::new("profile", vec![AgentMessage::user("retry")]);
        request.session_id = Some("session".into());
        request.turn_id = Some("turn".into());
        assert_eq!(
            validate_resume_request(&[], &request, std::slice::from_ref(&call))
                .unwrap_err()
                .code,
            "agent.tool.recovery_messages_not_allowed"
        );
        request.messages.clear();
        validate_resume_request(&[], &request, std::slice::from_ref(&call)).unwrap();
        let interactions = tool_recovery_interactions(&request, 4, &[call]).unwrap();
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].kind, InteractionKind::Custom);
        assert_eq!(
            interactions[0].options["recovery"],
            "ambiguous_tool_execution"
        );
        assert_eq!(
            interactions[0].options["choices"].as_array().unwrap().len(),
            2
        );
        let resolved = resolution("call-side-effect", true);
        let events = interaction_resolution_events("turn", std::slice::from_ref(&resolved));
        assert!(matches!(
            &events[0].0,
            AgentEvent::InteractionResolved {
                turn_id,
                resolution
            } if turn_id == "turn" && resolution == &resolved
        ));
    }
}
