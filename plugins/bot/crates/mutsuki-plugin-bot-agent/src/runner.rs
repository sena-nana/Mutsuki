#[allow(clippy::wildcard_imports)]
use super::*;

#[must_use]
pub fn agent_bridge_runner(client: RuntimeClientRef, bridge: BotAgentBridge) -> Box<dyn Runner> {
    agent_bridge_runner_with_delivery_policy(client, bridge, default_reply_delivery_policy())
}

#[must_use]
pub fn agent_bridge_runner_with_delivery_policy(
    client: RuntimeClientRef,
    bridge: BotAgentBridge,
    delivery_policy: DeliveryPolicy,
) -> Box<dyn Runner> {
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let bridge = bridge.clone();
        let delivery_policy = delivery_policy.clone();
        let max_concurrency = bridge.config.max_concurrency();
        let permit = bridge.concurrency.try_acquire(max_concurrency);
        Box::pin(async move {
            let Some(_permit) = permit else {
                return Err(bridge_failure(
                    &task,
                    "concurrency_limited",
                    format!("Bot Agent concurrency limit {max_concurrency} is currently occupied"),
                ));
            };
            run_bridge_task(ctx, task, bridge, delivery_policy).await
        })
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(agent_bridge_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

pub(crate) fn default_reply_delivery_policy() -> DeliveryPolicy {
    DeliveryPolicy {
        max_attempts: 3,
        initial_backoff_ms: 1_000,
        max_backoff_ms: 60_000,
        not_before_unix_ms: None,
        expires_at_unix_ms: None,
    }
}

pub(crate) fn agent_bridge_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_BRIDGE_PLUGIN_ID)
        .accepted_protocol(BOT_AGENT_SUBMIT_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_CANCEL_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_RESET_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_FORK_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_STATUS_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_REGENERATE_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_BIND_PROFILE_PROTOCOL_ID)
        .requires_protocol(BOT_REPLY_DELIVERY_PROTOCOL_ID)
        .requires_protocol(BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID)
        .requires_protocol(BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .invocation_mode(InvocationMode::AsyncReentrant)
        .concurrency(RunnerConcurrency::Reentrant {
            max_inflight_batches: 128,
            max_inflight_entries: 128,
        })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 1,
            max_batch_entries: 1,
            max_entry_concurrency: 1,
            max_inflight_batches: 128,
            side_effect: RunnerSideEffect::External,
            ..RunnerBatchCapability::default()
        })
        .control_capability(RunnerControlCapability {
            entry_cancel: true,
            batch_cancel: true,
            timeout_granularity: TimeoutGranularity::Entry,
        })
        .build()
}

pub(crate) async fn run_bridge_task(
    ctx: AsyncRunnerContext,
    task: Task,
    bridge: BotAgentBridge,
    delivery_policy: DeliveryPolicy,
) -> RuntimeResult<RunnerResult> {
    if let Ok(invocation) = task.payload.decode_shared::<BotNodeInvocation>() {
        return run_bridge_node_task(
            ctx,
            task,
            bridge,
            delivery_policy,
            invocation.as_ref().clone(),
        )
        .await;
    }
    let request = decode_bridge_request(&task)?;
    let execution = execute_bridge_request(&ctx, &task, &bridge, request).await?;
    let result = &execution.result;
    let (reply, media_errors) = if let Some(existing) = execution.existing_reply {
        (Some(existing), Vec::new())
    } else {
        let (outgoing, media_errors) = speech_reply_messages(&ctx, &task, result).await;
        if outgoing.is_empty() {
            (None, media_errors)
        } else {
            let request =
                reply_delivery_request(result, &execution.source_event, outgoing, delivery_policy)
                    .map_err(|error| bridge_failure(&task, "delivery.binding", error))?;
            let outcome = ctx
                .call_raw(
                    BOT_REPLY_DELIVERY_PROTOCOL_ID,
                    serde_json::to_value(BotReplyDeliveryCommand::Submit {
                        request: Box::new(request),
                        now_unix_ms: unix_ms(),
                    })
                    .map_err(|error| bridge_failure(&task, "delivery.encode", error))?,
                )
                .await;
            (
                Some(decode_reply_delivery_outcome(&task, outcome)?),
                media_errors,
            )
        }
    };
    if execution.complete_event {
        bridge
            .complete_event(&execution.source_event)
            .await
            .map_err(|error| bridge_failure(&task, "event.complete", error))?;
    }
    let mut completed = RunnerResult::completed(task.task_id);
    completed.output = Some(serde_json::json!({
        "session_id": result.binding.session_id,
        "session_version": result.binding.session_version,
        "last_event_sequence": result.binding.last_event_sequence,
        "turn_id": result.turn_id,
        "reply_delivery": reply,
        "media_errors": media_errors,
    }));
    Ok(completed)
}

pub(crate) async fn run_bridge_node_task(
    ctx: AsyncRunnerContext,
    task: Task,
    bridge: BotAgentBridge,
    delivery_policy: DeliveryPolicy,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    if task.protocol_id == BOT_AGENT_BIND_PROFILE_PROTOCOL_ID {
        return bind_profile_node(&task, invocation);
    }
    let request = flow_bridge_request(&task, &invocation)?;
    let execution = execute_bridge_request(&ctx, &task, &bridge, request).await?;
    if execution.existing_reply.is_some() {
        let mut completed = RunnerResult::completed(task.task_id.clone());
        completed.output = Some(
            serde_json::to_value(BotNodeResult {
                outputs: Vec::new(),
                metadata: std::collections::BTreeMap::from([(
                    "delivery_already_reserved".into(),
                    serde_json::Value::Bool(true),
                )]),
            })
            .map_err(|error| bridge_failure(&task, "node.output", error))?,
        );
        return Ok(completed);
    }
    let (outgoing, media_errors) = speech_reply_messages(&ctx, &task, &execution.result).await;
    let outputs = if outgoing.is_empty() {
        if execution.complete_event {
            bridge
                .complete_event(&execution.source_event)
                .await
                .map_err(|error| bridge_failure(&task, "event.complete", error))?;
        }
        Vec::new()
    } else {
        let mut request = reply_delivery_request(
            &execution.result,
            &execution.source_event,
            outgoing,
            delivery_policy,
        )
        .map_err(|error| bridge_failure(&task, "delivery.binding", error))?;
        request.occupancy_only = true;
        reserve_reply_delivery(&ctx, &task, &request).await?;
        vec![BotNodeOutput {
            port_id: "reply".into(),
            event: BotFlowEventEnvelope {
                event_id: request.reply_id.clone(),
                protocol_id: BOT_REPLY_DELIVERY_PROTOCOL_ID.into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                    value: serde_json::to_value(&request)
                        .map_err(|error| bridge_failure(&task, "delivery.encode", error))?,
                },
                context: overlay_reply_source_context(
                    invocation.input.context.clone(),
                    &execution.source_event,
                ),
                trace_id: invocation.input.trace_id.clone(),
                correlation_id: invocation.input.correlation_id.clone(),
            },
        }]
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert(
        "session_id".into(),
        serde_json::Value::String(execution.result.binding.session_id.clone()),
    );
    metadata.insert(
        "turn_id".into(),
        serde_json::Value::String(execution.result.turn_id.clone()),
    );
    metadata.insert(
        "media_errors".into(),
        serde_json::to_value(media_errors)
            .map_err(|error| bridge_failure(&task, "node.metadata", error))?,
    );
    let mut completed = RunnerResult::completed(task.task_id.clone());
    completed.output = Some(
        serde_json::to_value(BotNodeResult { outputs, metadata })
            .map_err(|error| bridge_failure(&task, "node.output", error))?,
    );
    Ok(completed)
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn bind_profile_node(
    task: &Task,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    let mut event: BotEvent = serde::Deserialize::deserialize(&invocation.input.payload.value)
        .map_err(|error| bridge_failure(task, "node.event", error))?;
    let override_existing = invocation
        .config
        .get("override")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let has_existing_profile = event
        .ext
        .get(BOT_EXT_AGENT_PROFILE_ID)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if let Some(profile_id) = invocation
        .config
        .get("profile_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && (!has_existing_profile || override_existing)
    {
        event.ext.insert(
            BOT_EXT_AGENT_PROFILE_ID.into(),
            serde_json::Value::String(profile_id.to_owned()),
        );
    }
    let mut output = invocation.input.clone();
    output.payload.value =
        serde_json::to_value(&event).map_err(|error| bridge_failure(task, "node.encode", error))?;
    let mut completed = RunnerResult::completed(task.task_id.clone());
    completed.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![BotNodeOutput {
                port_id: "output".into(),
                event: output,
            }],
            metadata: BTreeMap::default(),
        })
        .map_err(|error| bridge_failure(task, "node.output", error))?,
    );
    Ok(completed)
}

pub(crate) fn flow_bridge_request(
    task: &Task,
    invocation: &BotNodeInvocation,
) -> RuntimeResult<BotAgentBridgeRequest> {
    if task.protocol_id == BOT_AGENT_SUBMIT_PROTOCOL_ID {
        let event: BotEvent = serde::Deserialize::deserialize(&invocation.input.payload.value)
            .map_err(|error| bridge_failure(task, "node.event", error))?;
        return Ok(BotAgentBridgeRequest::Submit { event });
    }
    let command: BotCommandEvent = serde::Deserialize::deserialize(&invocation.input.payload.value)
        .map_err(|error| bridge_failure(task, "node.command", error))?;
    let event = command.source;
    match task.protocol_id.as_str() {
        BOT_AGENT_CANCEL_PROTOCOL_ID => {
            let turn_id = command
                .typed_args
                .get("turn_id")
                .and_then(|value| match value {
                    mutsuki_bot_protocol::BotCommandArgumentValue::String(value) => {
                        Some(value.clone())
                    }
                    _ => None,
                })
                .or_else(|| command.args.first().cloned())
                .ok_or_else(|| bridge_failure(task, "node.cancel", "turn_id is required"))?;
            Ok(BotAgentBridgeRequest::Cancel { event, turn_id })
        }
        BOT_AGENT_RESET_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Reset { event }),
        BOT_AGENT_FORK_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Fork { event }),
        BOT_AGENT_STATUS_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Status { event }),
        BOT_AGENT_REGENERATE_PROTOCOL_ID => Ok(BotAgentBridgeRequest::Regenerate { event }),
        protocol => Err(bridge_failure(task, "node.protocol", protocol)),
    }
}

pub(crate) struct BridgeExecution {
    pub(crate) result: BotAgentBridgeResult,
    pub(crate) source_event: BotEvent,
    pub(crate) complete_event: bool,
    pub(crate) existing_reply: Option<BotReplyDeliveryReceipt>,
}

pub(crate) fn decode_bridge_request(task: &Task) -> RuntimeResult<BotAgentBridgeRequest> {
    let payload = task.payload.to_value();
    if let Ok(request) = <BotAgentBridgeRequest as serde::Deserialize>::deserialize(&payload) {
        return Ok(request);
    }
    if let Ok(command) = <BotCommandEvent as serde::Deserialize>::deserialize(&payload) {
        return bridge_request_from_command(command)
            .map_err(|error| bridge_failure(task, "command.decode", error));
    }
    serde_json::from_value::<BotEvent>(payload)
        .map(|event| BotAgentBridgeRequest::Submit { event })
        .map_err(|error| bridge_failure(task, "request.decode", error))
}

pub(crate) async fn execute_bridge_request(
    ctx: &AsyncRunnerContext,
    task: &Task,
    bridge: &BotAgentBridge,
    request: BotAgentBridgeRequest,
) -> RuntimeResult<BridgeExecution> {
    match request {
        BotAgentBridgeRequest::Submit { event } => {
            submit_claimed_event(ctx, task, bridge, event).await
        }
        BotAgentBridgeRequest::Regenerate { mut event } => {
            let binding = bridge
                .status(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            event.event_id = format!("{}:regenerate:{}", event.event_id, binding.generation);
            submit_claimed_event(ctx, task, bridge, event).await
        }
        BotAgentBridgeRequest::Cancel { event, turn_id } => {
            let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .status(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .cancel(&resolved, &binding, actor_id, &turn_id)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: command_confirmation(&event.target, "已取消当前 Agent 回复"),
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
        BotAgentBridgeRequest::Reset { event } => {
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
            let binding = bridge
                .conversations
                .reset_session_binding(&resolved, actor_id)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing: command_confirmation(&event.target, "已开启新的 Agent 会话"),
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
        BotAgentBridgeRequest::Fork { event } => {
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .fork(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing: command_confirmation(&event.target, "已分叉 Agent 会话，历史记录已保留"),
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
        BotAgentBridgeRequest::Status { event } => {
            let resolved = bridge
                .resolve_admitted(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let binding = bridge
                .status(&event)
                .await
                .map_err(|error| bridge_failure(task, "action", error))?;
            let outgoing = status_confirmation(&event.target, &binding);
            let result = BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing,
            };
            Ok(BridgeExecution {
                result,
                source_event: event,
                complete_event: false,
                existing_reply: None,
            })
        }
    }
}

pub(crate) async fn submit_claimed_event(
    ctx: &AsyncRunnerContext,
    task: &Task,
    bridge: &BotAgentBridge,
    event: BotEvent,
) -> RuntimeResult<BridgeExecution> {
    let (claim, claimed) = bridge
        .claim_event_state(&event)
        .await
        .map_err(|error| bridge_failure(task, "claim", error))?;
    if claim != AgentEventClaim::New {
        let reply_id = stable_reply_id(
            &claimed.resolved.conversation.origin_key(),
            &event.event_id,
            &claimed.turn_id,
        );
        match inspect_reply_delivery(ctx, task, reply_id).await? {
            Some(receipt) => {
                return Ok(BridgeExecution {
                    result: claimed,
                    source_event: event,
                    complete_event: claim == AgentEventClaim::ResumePending,
                    existing_reply: Some(receipt),
                });
            }
            None if claim == AgentEventClaim::Completed => {
                return Ok(BridgeExecution {
                    result: claimed,
                    source_event: event,
                    complete_event: false,
                    existing_reply: None,
                });
            }
            None => {}
        }
    }
    let event = transcribe_event_audio(ctx, task, bridge, event).await?;
    let result = bridge
        .submit_event_with_trace_deferred(&event, trace_context(task).as_ref())
        .await
        .map_err(|error| bridge_failure(task, "action", error))?;
    Ok(BridgeExecution {
        result,
        source_event: event,
        complete_event: true,
        existing_reply: None,
    })
}

pub(crate) async fn inspect_reply_delivery(
    ctx: &AsyncRunnerContext,
    task: &Task,
    reply_id: String,
) -> RuntimeResult<Option<BotReplyDeliveryReceipt>> {
    let outcome = ctx
        .call_raw(
            BOT_REPLY_DELIVERY_PROTOCOL_ID,
            serde_json::to_value(BotReplyDeliveryCommand::Inspect { reply_id })
                .map_err(|error| bridge_failure(task, "delivery.inspect.encode", error))?,
        )
        .await
        .map(|value| value.into_outcome());
    match outcome {
        Ok(TaskOutcome::Completed {
            output: Some(output),
            ..
        }) => serde_json::from_value(output)
            .map(Some)
            .map_err(|error| bridge_failure(task, "delivery.inspect.decode", error)),
        Ok(TaskOutcome::Failed { error, .. }) if error.code == "delivery.not_found" => Ok(None),
        Ok(TaskOutcome::Failed { error, .. }) => {
            Err(bridge_failure(task, "delivery.inspect", error.code))
        }
        Ok(outcome) => Err(bridge_failure(
            task,
            "delivery.inspect",
            format!("unexpected delivery outcome {outcome:?}"),
        )),
        Err(error) => Err(error),
    }
}

pub(crate) async fn reserve_reply_delivery(
    ctx: &AsyncRunnerContext,
    task: &Task,
    request: &BotReplyDeliveryRequest,
) -> RuntimeResult<BotReplyDeliveryReceipt> {
    let outcome = ctx
        .call_raw(
            BOT_REPLY_DELIVERY_PROTOCOL_ID,
            serde_json::to_value(BotReplyDeliveryCommand::Reserve {
                request: Box::new(request.clone()),
            })
            .map_err(|error| bridge_failure(task, "delivery.reserve.encode", error))?,
        )
        .await;
    decode_reply_delivery_outcome(task, outcome)
}

pub(crate) fn overlay_reply_source_context(
    mut context: BotFlowContext,
    event: &BotEvent,
) -> BotFlowContext {
    if let Some(message_id) = event
        .message
        .as_ref()
        .and_then(|message| message.message_id.clone())
    {
        context.ext.insert(
            BOT_EXT_REPLY_SOURCE_MESSAGE_ID.into(),
            serde_json::Value::String(message_id),
        );
    }
    if let Some(user_id) = event.actor.as_ref().map(|actor| actor.user_id.clone()) {
        context.ext.insert(
            BOT_EXT_REPLY_SOURCE_USER_ID.into(),
            serde_json::Value::String(user_id),
        );
    }
    context
}

pub(crate) fn decode_reply_delivery_outcome(
    task: &Task,
    outcome: RuntimeResult<impl Into<TaskOutcome>>,
) -> RuntimeResult<BotReplyDeliveryReceipt> {
    match outcome?.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output)
            .map_err(|error| bridge_failure(task, "delivery.decode", error)),
        TaskOutcome::Failed { error, .. } => {
            Err(bridge_failure(task, "delivery.submit", error.code))
        }
        outcome => Err(bridge_failure(
            task,
            "delivery.submit",
            format!("unexpected delivery outcome {outcome:?}"),
        )),
    }
}

pub(crate) fn reply_delivery_request(
    result: &BotAgentBridgeResult,
    event: &BotEvent,
    outgoing: Vec<BotMessage>,
    policy: DeliveryPolicy,
) -> Result<BotReplyDeliveryRequest, ConversationError> {
    let turn_id = if result.turn_id.trim().is_empty() {
        format!("bot-action:{}", event.event_id)
    } else {
        result.turn_id.clone()
    };
    let reply_id = stable_reply_id(
        &result.resolved.conversation.origin_key(),
        &event.event_id,
        &turn_id,
    );
    let parts = outgoing
        .into_iter()
        .enumerate()
        .map(|(index, message)| BotReplyDeliveryPart {
            part_id: format!("{reply_id}:part:{index}"),
            content: BotDeliveryContent {
                segments: message.segments,
                summary: None,
                reply_to: message.reply_to,
            },
            not_before_unix_ms: None,
        })
        .collect();
    let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
    let source_binding_key = session_binding_key(
        &result.resolved.conversation,
        result.resolved.policy.session_scope,
        actor_id,
    )?;
    Ok(BotReplyDeliveryRequest {
        idempotency_key: reply_id.clone(),
        reply_id,
        conversation: result.resolved.conversation.clone(),
        parts,
        policy,
        source_event_id: event.event_id.clone(),
        source_turn_id: turn_id,
        source_binding_key: Some(source_binding_key),
        occupancy_only: false,
    })
}

pub(crate) fn stable_reply_id(origin_key: &str, event_id: &str, turn_id: &str) -> String {
    let mut digest = Sha256::new();
    for value in [origin_key, event_id, turn_id] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    let mut id = String::from("agent-reply:");
    for byte in digest.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to String cannot fail");
    }
    id
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub(crate) fn bridge_request_from_command(
    command: BotCommandEvent,
) -> Result<BotAgentBridgeRequest, BotAgentError> {
    let mut event = command.source;
    match command.name.as_str() {
        "ask" | "chat" => {
            let prompt = command.args.join(" ");
            if prompt.trim().is_empty() {
                return Err(BotAgentError::InvalidCommand);
            }
            event.message = Some(BotMessage::text(event.target.clone(), prompt));
            Ok(BotAgentBridgeRequest::Submit { event })
        }
        "cancel" => command
            .args
            .first()
            .filter(|turn_id| !turn_id.trim().is_empty())
            .cloned()
            .map(|turn_id| BotAgentBridgeRequest::Cancel { event, turn_id })
            .ok_or(BotAgentError::InvalidCommand),
        "reset" => Ok(BotAgentBridgeRequest::Reset { event }),
        "fork" => Ok(BotAgentBridgeRequest::Fork { event }),
        "status" => Ok(BotAgentBridgeRequest::Status { event }),
        "regenerate" => Ok(BotAgentBridgeRequest::Regenerate { event }),
        _ => Err(BotAgentError::InvalidCommand),
    }
}

pub(crate) fn command_confirmation(target: &BotTarget, text: &str) -> Vec<BotMessage> {
    vec![BotMessage::text(target.clone(), text)]
}

pub(crate) fn status_confirmation(
    target: &BotTarget,
    binding: &AgentSessionBinding,
) -> Vec<BotMessage> {
    let text = if binding.session_version == 0 {
        "当前 Agent 会话尚未开始".to_owned()
    } else {
        format!("当前 Agent 会话已完成 {} 轮对话", binding.session_version)
    };
    command_confirmation(target, &text)
}

pub(crate) async fn transcribe_event_audio(
    ctx: &AsyncRunnerContext,
    task: &Task,
    bridge: &BotAgentBridge,
    mut event: BotEvent,
) -> RuntimeResult<BotEvent> {
    let resolved = bridge
        .resolve_admitted(&event)
        .await
        .map_err(|error| bridge_failure(task, "policy", error))?;
    if !resolved.policy.stt_enabled {
        return Ok(event);
    }
    let audio = event
        .message
        .as_ref()
        .into_iter()
        .flat_map(|message| &message.segments)
        .filter_map(|segment| match segment {
            MessageSegment::Audio { resource } => Some(resource.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut transcripts = Vec::with_capacity(audio.len());
    for resource in audio {
        let outcome = ctx
            .call_raw(
                BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID,
                serde_json::to_value(BotMediaTranscribeRequest {
                    audio: resource,
                    language: None,
                    prompt_hint: None,
                    selector_id: resolved.policy.stt_selector_id.clone(),
                    timeout_ms: None,
                })
                .map_err(|error| bridge_failure(task, "stt.encode", error))?,
            )
            .await?;
        transcripts
            .push(decode_child_output::<BotMediaTranscribeResult>(outcome, task, "stt")?.text);
    }
    if let (Some(message), false) = (event.message.as_mut(), transcripts.is_empty()) {
        let transcript = transcripts.join("\n");
        let transcript = if message.plain_text().is_empty() {
            transcript
        } else {
            format!("\n{transcript}")
        };
        message.segments.push(MessageSegment::text(transcript));
    }
    Ok(event)
}

pub(crate) async fn speech_reply_messages(
    ctx: &AsyncRunnerContext,
    task: &Task,
    result: &BotAgentBridgeResult,
) -> (Vec<BotMessage>, Vec<String>) {
    if !result.resolved.policy.tts_enabled {
        return (result.outgoing.clone(), Vec::new());
    }
    let policy = result.resolved.policy.speech_reply_policy;
    let mut outgoing = Vec::new();
    let mut errors = Vec::new();
    for message in &result.outgoing {
        let text = message.plain_text();
        if text.is_empty() {
            outgoing.push(message.clone());
            continue;
        }
        let synthesis = ctx
            .call_raw(
                BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID,
                match serde_json::to_value(BotMediaSynthesizeRequest {
                    target: message.target.clone(),
                    text: text.clone(),
                    policy,
                    voice_selector_id: result.resolved.policy.tts_selector_id.clone(),
                    timeout_ms: None,
                    qq_mime_type: "audio/silk".into(),
                }) {
                    Ok(value) => value,
                    Err(error) => {
                        errors.push(error.to_string());
                        if policy != BotSpeechReplyPolicy::VoiceOnly {
                            outgoing.push(message.clone());
                        }
                        continue;
                    }
                },
            )
            .await;
        let synthesized = synthesis.and_then(|outcome| {
            decode_child_output::<BotMediaSynthesizeResult>(outcome, task, "tts")
        });
        match synthesized {
            Ok(synthesized) => {
                if let Some(text) = synthesized.text {
                    outgoing.push(BotMessage::text(message.target.clone(), text));
                }
                if let Some(upload) = synthesized.upload {
                    let segment = match upload.kind {
                        BotMediaKind::Image => MessageSegment::Image {
                            resource: upload.resource,
                        },
                        BotMediaKind::Audio => MessageSegment::Audio {
                            resource: upload.resource,
                        },
                        BotMediaKind::Video => MessageSegment::Video {
                            resource: upload.resource,
                        },
                        BotMediaKind::File => MessageSegment::File {
                            resource: upload.resource,
                            name: upload.file_name,
                        },
                    };
                    outgoing.push(BotMessage {
                        message_id: None,
                        target: message.target.clone(),
                        sender: None,
                        segments: vec![segment],
                        reply_to: message.reply_to.clone(),
                        time_ms: None,
                        ext: std::collections::BTreeMap::default(),
                    });
                }
            }
            Err(error) => {
                errors.push(error.error().code.clone());
                if policy != BotSpeechReplyPolicy::VoiceOnly {
                    outgoing.push(message.clone());
                }
            }
        }
    }
    (outgoing, errors)
}

pub(crate) fn decode_child_output<T: serde::de::DeserializeOwned>(
    outcome: impl Into<TaskOutcome>,
    task: &Task,
    route: &str,
) -> RuntimeResult<T> {
    match outcome.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output)
            .map_err(|error| bridge_failure(task, &format!("{route}.output"), error)),
        TaskOutcome::Failed { error, .. } => Err(RuntimeFailure::new(error)),
        TaskOutcome::Completed { output: None, .. }
        | TaskOutcome::Cancelled { .. }
        | TaskOutcome::Expired { .. }
        | TaskOutcome::DeadLetter { .. } => {
            Err(bridge_failure(task, route, "child did not complete"))
        }
    }
}

pub(crate) fn bridge_failure(
    task: &Task,
    route: &str,
    error: impl std::fmt::Display,
) -> RuntimeFailure {
    let mut runtime_error = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_AGENT_BRIDGE_PLUGIN_ID,
        format!("{}.{}", task.task_id, route),
    );
    runtime_error.evidence.insert(
        "reason".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    RuntimeFailure::new(runtime_error)
}
