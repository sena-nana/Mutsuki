use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use mutsuki_agent_client::{AgentClient, AgentClientBackend};
use mutsuki_agent_contracts::{
    AgentContentPart, AgentEvent, AgentEventPage, AgentMessage, AgentSession,
    AgentSessionCreateRequest, AgentWireError, SessionSnapshotRef, SessionVersion,
};
use mutsuki_bot_conversation::{
    AgentEventClaim, ConversationAdmissionError, ConversationError, ConversationService,
    qq_conversation_from_event,
};
use mutsuki_bot_protocol::{
    AgentSessionBinding, BOT_AGENT_BRIDGE_PROTOCOL_ID, BOT_COMMAND_HANDLE_PROTOCOL_ID,
    BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID, BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID,
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotAgentBridgeRequest, BotCommandArgumentDescriptor,
    BotCommandArgumentKind, BotCommandDescriptor, BotCommandEvent, BotEvent, BotMediaKind,
    BotMediaSynthesizeRequest, BotMediaSynthesizeResult, BotMediaTranscribeRequest,
    BotMediaTranscribeResult, BotMessage, BotSpeechReplyPolicy, MessageSegment,
    QqStreamingStrategy, ResolvedConversationPolicy, bot_command_binding_id,
};
use mutsuki_runtime_contracts::{ExecutionClass, PluginManifest, RunnerResult, Task, TaskOutcome};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, BoxedTaskAwaitRunner, HandlerBindingBuilder, PluginBuilder,
    ProtocolDescriptorBuilder, RunnerDescriptorBuilder, RuntimeClientRef, RuntimeFailure,
    RuntimeResult, TaskAwaitRunnerAdapter,
};
use thiserror::Error;

pub const BOT_AGENT_BRIDGE_PLUGIN_ID: &str = "mutsuki.plugin.bot.agent";
pub const BOT_AGENT_BRIDGE_RUNNER_ID: &str = "mutsuki.bot.agent.bridge";

#[must_use]
pub fn bot_agent_bridge_manifest() -> PluginManifest {
    let mut builder = PluginBuilder::new(BOT_AGENT_BRIDGE_PLUGIN_ID)
        .runner_descriptor(agent_bridge_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_AGENT_BRIDGE_PROTOCOL_ID).build(),
            BOT_AGENT_BRIDGE_RUNNER_ID,
            "bot-agent",
        );
    for command in [
        "ask",
        "chat",
        "cancel",
        "reset",
        "fork",
        "status",
        "regenerate",
    ] {
        builder = builder.handler_binding(
            HandlerBindingBuilder::new(
                bot_command_binding_id(command),
                BOT_AGENT_BRIDGE_PLUGIN_ID,
                BOT_COMMAND_HANDLE_PROTOCOL_ID,
                BOT_COMMAND_HANDLE_PROTOCOL_ID,
            )
            .target_runner_hint(BOT_AGENT_BRIDGE_RUNNER_ID)
            .pool_id("bot-agent-command")
            .build(),
        );
    }
    builder.build().manifest
}

#[must_use]
pub fn bot_agent_command_descriptors() -> Vec<BotCommandDescriptor> {
    let prompt = BotCommandArgumentDescriptor {
        name: "prompt".into(),
        kind: BotCommandArgumentKind::String,
        optional: false,
        variadic: true,
        default: None,
    };
    vec![
        BotCommandDescriptor {
            path: vec!["ask".into()],
            aliases: vec![vec!["ai".into()]],
            arguments: vec![prompt.clone()],
            summary: Some("向当前 QQ Agent 会话提问".into()),
        },
        BotCommandDescriptor {
            path: vec!["chat".into()],
            aliases: Vec::new(),
            arguments: vec![prompt],
            summary: Some("向当前 QQ Agent 会话发送消息".into()),
        },
        BotCommandDescriptor {
            path: vec!["cancel".into()],
            aliases: Vec::new(),
            arguments: vec![BotCommandArgumentDescriptor {
                name: "turn_id".into(),
                kind: BotCommandArgumentKind::String,
                optional: false,
                variadic: false,
                default: None,
            }],
            summary: Some("取消指定 Agent turn".into()),
        },
        simple_agent_command("reset", "重置当前 QQ Agent 会话"),
        simple_agent_command("fork", "分叉当前 QQ Agent 会话并保留历史"),
        simple_agent_command("status", "查看当前 QQ Agent 会话状态"),
        simple_agent_command("regenerate", "重新生成上一轮 Agent 回复"),
    ]
}

fn simple_agent_command(name: &str, summary: &str) -> BotCommandDescriptor {
    BotCommandDescriptor {
        path: vec![name.into()],
        aliases: Vec::new(),
        arguments: Vec::new(),
        summary: Some(summary.into()),
    }
}

#[must_use]
pub fn agent_bridge_runner(client: RuntimeClientRef, bridge: BotAgentBridge) -> Box<dyn Runner> {
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let bridge = bridge.clone();
        Box::pin(run_bridge_task(ctx, task, bridge))
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(agent_bridge_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

fn agent_bridge_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_AGENT_BRIDGE_RUNNER_ID, BOT_AGENT_BRIDGE_PLUGIN_ID)
        .accepted_protocol(BOT_AGENT_BRIDGE_PROTOCOL_ID)
        .accepted_protocol(BOT_COMMAND_HANDLE_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

async fn run_bridge_task(
    ctx: AsyncRunnerContext,
    task: Task,
    bridge: BotAgentBridge,
) -> RuntimeResult<RunnerResult> {
    let payload = task.payload.to_value();
    let request =
        if let Ok(request) = serde_json::from_value::<BotAgentBridgeRequest>(payload.clone()) {
            request
        } else if let Ok(command) = serde_json::from_value::<BotCommandEvent>(payload.clone()) {
            bridge_request_from_command(command)
                .map_err(|error| bridge_failure(&task, "command.decode", error))?
        } else {
            serde_json::from_value::<BotEvent>(payload)
                .map(|event| BotAgentBridgeRequest::Submit { event })
                .map_err(|error| bridge_failure(&task, "request.decode", error))?
        };
    let result = match request {
        BotAgentBridgeRequest::Submit { event } => {
            if let Some(completed) = bridge
                .claim_event_before_media(&event)
                .map_err(|error| bridge_failure(&task, "claim", error))?
            {
                Ok(completed)
            } else {
                let event = transcribe_event_audio(&ctx, &task, &bridge, event).await?;
                bridge.submit_event_with_trace(&event, trace_context(&task).as_ref())
            }
        }
        BotAgentBridgeRequest::Regenerate { mut event } => {
            let binding = bridge.status(&event);
            match binding {
                Ok(binding) => {
                    event.event_id =
                        format!("{}:regenerate:{}", event.event_id, binding.generation);
                    if let Some(completed) = bridge
                        .claim_event_before_media(&event)
                        .map_err(|error| bridge_failure(&task, "claim", error))?
                    {
                        Ok(completed)
                    } else {
                        let event = transcribe_event_audio(&ctx, &task, &bridge, event).await?;
                        bridge.submit_event_with_trace(&event, trace_context(&task).as_ref())
                    }
                }
                Err(error) => Err(error),
            }
        }
        BotAgentBridgeRequest::Cancel { event, turn_id } => (|| {
            let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
            let resolved = bridge.resolve_admitted(&event)?;
            let binding = bridge.status(&event)?;
            let binding = bridge.cancel(&resolved, &binding, actor_id, &turn_id)?;
            Ok(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: Vec::new(),
            })
        })(),
        BotAgentBridgeRequest::Reset { event } => (|| {
            let resolved = bridge.resolve_admitted(&event)?;
            let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
            let binding = bridge
                .conversations
                .reset_session_binding(&resolved, actor_id)?;
            Ok(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing: Vec::new(),
            })
        })(),
        BotAgentBridgeRequest::Fork { event } => (|| {
            let resolved = bridge.resolve_admitted(&event)?;
            let binding = bridge.fork(&event)?;
            Ok(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing: Vec::new(),
            })
        })(),
        BotAgentBridgeRequest::Status { event } => (|| {
            let resolved = bridge.resolve_admitted(&event)?;
            let binding = bridge.status(&event)?;
            Ok(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id: String::new(),
                outgoing: Vec::new(),
            })
        })(),
    }
    .map_err(|error: BotAgentError| bridge_failure(&task, "action", error))?;
    let (outgoing, media_errors) = speech_reply_messages(&ctx, &task, &result).await;
    let mut delivered = 0_u64;
    let mut delivery_errors = Vec::new();
    for message in outgoing {
        match ctx
            .call_raw(
                BOT_MESSAGE_SEND_PROTOCOL_ID,
                serde_json::to_value(message)
                    .map_err(|error| bridge_failure(&task, "message.encode", error))?,
            )
            .await
        {
            Ok(_) => delivered += 1,
            Err(error) => delivery_errors.push(error.error().code.clone()),
        }
    }
    let mut completed = RunnerResult::completed(task.task_id);
    completed.output = Some(serde_json::json!({
        "session_id": result.binding.session_id,
        "session_version": result.binding.session_version,
        "last_event_sequence": result.binding.last_event_sequence,
        "turn_id": result.turn_id,
        "delivered_messages": delivered,
        "delivery_errors": delivery_errors,
        "media_errors": media_errors,
    }));
    Ok(completed)
}

fn bridge_request_from_command(
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

async fn transcribe_event_audio(
    ctx: &AsyncRunnerContext,
    task: &Task,
    bridge: &BotAgentBridge,
    mut event: BotEvent,
) -> RuntimeResult<BotEvent> {
    let resolved = bridge
        .resolve_admitted(&event)
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

async fn speech_reply_messages(
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

fn decode_child_output<T: serde::de::DeserializeOwned>(
    outcome: TaskOutcome,
    task: &Task,
    route: &str,
) -> RuntimeResult<T> {
    match outcome {
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

fn bridge_failure(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeFailure {
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

pub trait AgentBridgeClient: Send {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError>;
    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError>;
    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError>;
    fn cancel_turn(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError>;
    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError>;
    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError>;
}

impl<B: AgentClientBackend + Send> AgentBridgeClient for AgentClient<B> {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.get_session(session_id)
    }

    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        self.start_session(request)
    }

    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        turn_id: &str,
        messages: Vec<AgentMessage>,
        idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        self.submit_turn(
            session_id,
            expected_version,
            turn_id,
            messages,
            idempotency_key,
        )
    }

    fn cancel_turn(
        &mut self,
        session_id: &str,
        turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        self.cancel_turn(session_id, turn_id, expected_version)
    }

    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let session = self.get_session(source_session_id)?;
        self.fork_session(
            source_session_id,
            target_session_id,
            SessionSnapshotRef {
                session_id: source_session_id.into(),
                version: expected_version,
                snapshot: session.resource,
                base: None,
                deltas: Vec::new(),
            },
        )
    }

    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
        self.resume_session_events(session_id, after_sequence)
    }
}

#[derive(Clone)]
pub struct BotAgentBridge {
    conversations: ConversationService,
    client: Arc<Mutex<Box<dyn AgentBridgeClient>>>,
    streaming: QqStreamingStrategy,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotAgentTraceContext {
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
}

impl BotAgentBridge {
    #[must_use]
    pub fn new(
        conversations: ConversationService,
        client: Box<dyn AgentBridgeClient>,
        streaming: QqStreamingStrategy,
    ) -> Self {
        Self {
            conversations,
            client: Arc::new(Mutex::new(client)),
            streaming,
        }
    }

    pub fn submit_event(&self, event: &BotEvent) -> Result<BotAgentBridgeResult, BotAgentError> {
        self.submit_event_with_trace(event, None)
    }

    pub fn submit_event_with_trace(
        &self,
        event: &BotEvent,
        trace: Option<&BotAgentTraceContext>,
    ) -> Result<BotAgentBridgeResult, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event)?;
        let profile_id = resolved
            .policy
            .agent_runtime_profile_id
            .clone()
            .ok_or(BotAgentError::AgentProfileMissing)?;
        let message = event_message(event, trace)?;
        let turn_id = format!("qq:{}", event.event_id);
        let mut client = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?;
        let binding = self
            .conversations
            .get_or_create_session_binding(&resolved, actor_id)?;
        if self
            .conversations
            .begin_agent_event(&resolved, actor_id, &event.event_id, &turn_id)?
            == AgentEventClaim::Completed
        {
            return Ok(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: Vec::new(),
            });
        }
        match client.get_session(&binding.session_id) {
            Ok(_) => {}
            Err(error) if error.code == "agent.session.not_found" => {
                client.start_session(AgentSessionCreateRequest {
                    session_id: Some(binding.session_id.clone()),
                    profile_id,
                    title: Some(format!("QQ {}", resolved.conversation.origin_key())),
                })?;
            }
            Err(error) => return Err(error.into()),
        }
        let next_version = client.submit_turn(
            &binding.session_id,
            SessionVersion(binding.session_version),
            &turn_id,
            vec![message],
            &event.event_id,
        )?;
        let binding = self.conversations.advance_session(
            &resolved,
            actor_id,
            binding.session_version,
            next_version.0,
        )?;
        let page = client.events(&binding.session_id, binding.last_event_sequence)?;
        let outgoing = outgoing_messages(event, &page, &self.streaming);
        let binding = self.conversations.advance_event_sequence(
            &resolved,
            actor_id,
            binding.last_event_sequence,
            page.next_sequence,
        )?;
        self.conversations
            .complete_agent_event(&resolved, actor_id, &event.event_id)?;
        Ok(BotAgentBridgeResult {
            resolved,
            binding,
            turn_id,
            outgoing,
        })
    }

    /// Claims a QQ event before STT/TTS or any other externally metered media work.
    ///
    /// A pending claim remains resumable after a Host restart; only a completed claim bypasses
    /// the rest of the bridge.
    pub fn claim_event_before_media(
        &self,
        event: &BotEvent,
    ) -> Result<Option<BotAgentBridgeResult>, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event)?;
        let binding = self
            .conversations
            .get_or_create_session_binding(&resolved, actor_id)?;
        let turn_id = format!("qq:{}", event.event_id);
        let claim =
            self.conversations
                .begin_agent_event(&resolved, actor_id, &event.event_id, &turn_id)?;
        Ok(
            (claim == AgentEventClaim::Completed).then_some(BotAgentBridgeResult {
                resolved,
                binding,
                turn_id,
                outgoing: Vec::new(),
            }),
        )
    }

    pub fn resolve(&self, event: &BotEvent) -> Result<ResolvedConversationPolicy, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        Ok(self
            .conversations
            .resolve_policy(qq_conversation_from_event(event)?, actor_id)?)
    }

    pub fn resolve_admitted(
        &self,
        event: &BotEvent,
    ) -> Result<ResolvedConversationPolicy, BotAgentError> {
        let resolved = self.resolve(event)?;
        self.conversations.admit_event(&resolved, event)?;
        if !resolved.policy.agent_enabled {
            return Err(BotAgentError::AgentDisabled);
        }
        Ok(resolved)
    }

    pub fn status(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve(event)?;
        self.conversations
            .session_binding(&resolved, actor_id)?
            .ok_or(BotAgentError::SessionBindingMissing)
    }

    pub fn cancel(
        &self,
        resolved: &ResolvedConversationPolicy,
        binding: &AgentSessionBinding,
        actor_id: Option<&str>,
        turn_id: &str,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        let version = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .cancel_turn(
                &binding.session_id,
                turn_id,
                SessionVersion(binding.session_version),
            )?;
        Ok(self.conversations.advance_session(
            resolved,
            actor_id,
            binding.session_version,
            version.0,
        )?)
    }

    pub fn cancel_event(
        &self,
        event: &BotEvent,
        turn_id: &str,
    ) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event)?;
        let binding = self.status(event)?;
        self.cancel(&resolved, &binding, actor_id, turn_id)
    }

    pub fn reset(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event)?;
        Ok(self
            .conversations
            .reset_session_binding(&resolved, actor_id)?)
    }

    pub fn fork(&self, event: &BotEvent) -> Result<AgentSessionBinding, BotAgentError> {
        let actor_id = event.actor.as_ref().map(|actor| actor.user_id.as_str());
        let resolved = self.resolve_admitted(event)?;
        let (source, target_session_id) = self
            .conversations
            .prepare_session_fork(&resolved, actor_id)?;
        let target_version = self
            .client
            .lock()
            .map_err(|_| BotAgentError::ClientPoisoned)?
            .fork_session(
                &source.session_id,
                &target_session_id,
                SessionVersion(source.session_version),
            )?;
        Ok(self.conversations.commit_session_fork(
            &resolved,
            actor_id,
            &source,
            target_session_id,
            target_version.0,
        )?)
    }

    pub fn regenerate(&self, event: &BotEvent) -> Result<BotAgentBridgeResult, BotAgentError> {
        let binding = self.status(event)?;
        let mut retry = event.clone();
        retry.event_id = format!("{}:regenerate:{}", event.event_id, binding.generation);
        self.submit_event(&retry)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BotAgentBridgeResult {
    pub resolved: ResolvedConversationPolicy,
    pub binding: AgentSessionBinding,
    pub turn_id: String,
    pub outgoing: Vec<BotMessage>,
}

fn event_message(
    event: &BotEvent,
    trace: Option<&BotAgentTraceContext>,
) -> Result<AgentMessage, BotAgentError> {
    let message = event
        .message
        .as_ref()
        .ok_or(BotAgentError::MessageMissing)?;
    let mut parts = Vec::new();
    for segment in &message.segments {
        match segment {
            MessageSegment::Image { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Image {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    width: None,
                    height: None,
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::Audio { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Audio {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    duration_ms: None,
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::File { resource, name } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Document {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    filename: name.clone(),
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            MessageSegment::Video { resource } => {
                validate_agent_media(resource)?;
                parts.push(AgentContentPart::Document {
                    resource: resource.clone(),
                    mime_type: resource.schema.clone(),
                    filename: Some("qq-video".into()),
                    size_bytes: resource.size_hint,
                    provenance: Some("qq".into()),
                });
            }
            _ => {}
        }
    }
    let mut agent_message = AgentMessage::user(message.plain_text()).with_parts(parts);
    agent_message.metadata = Some(serde_json::json!({
        "source": "qq",
        "account_id": &event.bot.account_id,
        "event_id": &event.event_id,
        "message_id": message.message_id.as_deref(),
        "reply_to": message.reply_to.as_deref(),
        "trace_id": trace.and_then(|trace| trace.trace_id.as_deref()),
        "correlation_id": trace.and_then(|trace| trace.correlation_id.as_deref()),
    }));
    Ok(agent_message)
}

fn validate_agent_media(
    resource: &mutsuki_runtime_contracts::ResourceRef,
) -> Result<(), BotAgentError> {
    if resource.provider_id.trim().is_empty()
        || resource.schema.trim().is_empty()
        || resource.size_hint.is_none()
        || resource.content_hash.as_deref().is_none_or(str::is_empty)
    {
        return Err(BotAgentError::MediaResourceUnvalidated);
    }
    Ok(())
}

fn outgoing_messages(
    source: &BotEvent,
    page: &AgentEventPage,
    strategy: &QqStreamingStrategy,
) -> Vec<BotMessage> {
    let mut deltas = String::new();
    let mut final_text = None;
    let mut supplements = String::new();
    let mut artifacts = Vec::new();
    for envelope in &page.events {
        match &envelope.event {
            AgentEvent::ModelDelta { text, .. } => deltas.push_str(text),
            AgentEvent::FinalResponse { summary, .. } => final_text = Some(summary.clone()),
            AgentEvent::ArtifactProduced { artifact, .. } => {
                if let Some(resource) = artifact.content_ref.clone() {
                    if resource.size_hint.is_some()
                        && resource
                            .content_hash
                            .as_deref()
                            .is_some_and(|hash| !hash.is_empty())
                    {
                        let segment = if artifact.media_type.starts_with("image/") {
                            MessageSegment::Image { resource }
                        } else if artifact.media_type.starts_with("audio/") {
                            MessageSegment::Audio { resource }
                        } else if artifact.media_type.starts_with("video/") {
                            MessageSegment::Video { resource }
                        } else {
                            MessageSegment::File {
                                resource,
                                name: Some(artifact.artifact_id.clone()),
                            }
                        };
                        artifacts.push(BotMessage {
                            message_id: None,
                            target: source.target.clone(),
                            sender: None,
                            segments: vec![segment],
                            reply_to: None,
                            time_ms: None,
                            ext: std::collections::BTreeMap::default(),
                        });
                    } else {
                        let _ = write!(
                            supplements,
                            "\n\n[{}: unavailable resource]",
                            artifact.summary
                        );
                    }
                } else if !artifact.summary.is_empty() {
                    let _ = write!(supplements, "\n\n{}", artifact.summary);
                }
            }
            _ => {}
        }
    }
    let mut text = final_text.unwrap_or(deltas);
    text.push_str(&supplements);
    if text.is_empty() {
        return artifacts;
    }
    let mut messages = match strategy {
        QqStreamingStrategy::SegmentMessages => split_text(&text, 1_800)
            .into_iter()
            .map(|chunk| BotMessage::text(source.target.clone(), chunk))
            .collect(),
        QqStreamingStrategy::FinalOnly => {
            vec![BotMessage::text(source.target.clone(), text)]
        }
    };
    messages.extend(artifacts);
    messages
}

fn trace_context(task: &Task) -> Option<BotAgentTraceContext> {
    (task.trace_id.is_some() || task.correlation_id.is_some()).then(|| BotAgentTraceContext {
        trace_id: task.trace_id.clone(),
        correlation_id: task.correlation_id.clone(),
    })
}

fn split_text(text: &str, max_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(text[start..end].to_owned());
        start = end;
    }
    chunks
}

#[derive(Debug, Error)]
pub enum BotAgentError {
    #[error(transparent)]
    Conversation(#[from] ConversationError),
    #[error(transparent)]
    Admission(#[from] ConversationAdmissionError),
    #[error("Agent client failed: {code}: {message}")]
    AgentClient { code: String, message: String },
    #[error("Agent is disabled for this conversation")]
    AgentDisabled,
    #[error("Agent runtime profile binding is missing")]
    AgentProfileMissing,
    #[error("Bot message is missing")]
    MessageMissing,
    #[error("Bot media ResourceRef is missing provider, MIME schema, size, or content hash")]
    MediaResourceUnvalidated,
    #[error("Agent client mutex was poisoned")]
    ClientPoisoned,
    #[error("Agent session binding is missing")]
    SessionBindingMissing,
    #[error("Agent command is invalid or unsupported")]
    InvalidCommand,
}

impl From<AgentWireError> for BotAgentError {
    fn from(error: AgentWireError) -> Self {
        Self::AgentClient {
            code: error.code,
            message: error.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use mutsuki_agent_contracts::{
        AgentEventEnvelope, AgentEventMeta, ArtifactRef, ResourceCellRef, ResourceRef,
    };
    use mutsuki_bot_conversation::ConversationRepository;
    use mutsuki_bot_protocol::{
        AgentSessionScope, BotAccountRef, BotEventKind, BotPlatform, BotTarget, BotUser,
        ConversationPolicy, ConversationPolicyRule, DirectMessagePolicy,
    };
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    #[derive(Default)]
    struct Repository {
        bindings: Mutex<BTreeMap<String, AgentSessionBinding>>,
        events: Mutex<BTreeMap<(String, String), bool>>,
    }

    impl ConversationRepository for Repository {
        fn policy_rules(&self) -> Result<Vec<ConversationPolicyRule>, ConversationError> {
            Ok(Vec::new())
        }

        fn session_binding(
            &self,
            key: &str,
        ) -> Result<Option<AgentSessionBinding>, ConversationError> {
            Ok(self.bindings.lock().unwrap().get(key).cloned())
        }

        fn compare_and_set_session_binding(
            &self,
            key: &str,
            expected: Option<u64>,
            binding: AgentSessionBinding,
        ) -> Result<(), ConversationError> {
            let mut bindings = self.bindings.lock().unwrap();
            if bindings.get(key).map(|value| value.generation) != expected {
                return Err(ConversationError::GenerationConflict);
            }
            bindings.insert(key.into(), binding);
            Ok(())
        }

        fn begin_agent_event(
            &self,
            binding_key: &str,
            event_id: &str,
            _turn_id: &str,
        ) -> Result<AgentEventClaim, ConversationError> {
            let mut events = self.events.lock().unwrap();
            let key = (binding_key.into(), event_id.into());
            Ok(match events.get(&key) {
                Some(true) => AgentEventClaim::Completed,
                Some(false) => AgentEventClaim::ResumePending,
                None => {
                    events.insert(key, false);
                    AgentEventClaim::New
                }
            })
        }

        fn complete_agent_event(
            &self,
            binding_key: &str,
            event_id: &str,
        ) -> Result<(), ConversationError> {
            self.events
                .lock()
                .unwrap()
                .insert((binding_key.into(), event_id.into()), true);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeAgentClient {
        sessions: BTreeSet<String>,
        versions: BTreeMap<String, u64>,
        next_events: BTreeMap<String, AgentEventPage>,
        submitted: Vec<(String, String)>,
    }

    impl AgentBridgeClient for FakeAgentClient {
        fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
            if !self.sessions.contains(session_id) {
                return Err(wire_error("agent.session.not_found"));
            }
            Ok(session(session_id))
        }

        fn start_session(
            &mut self,
            request: AgentSessionCreateRequest,
        ) -> Result<AgentSession, AgentWireError> {
            let id = request.session_id.unwrap();
            self.sessions.insert(id.clone());
            self.versions.insert(id.clone(), 0);
            Ok(session(&id))
        }

        fn submit_turn(
            &mut self,
            session_id: &str,
            expected_version: SessionVersion,
            turn_id: &str,
            _messages: Vec<AgentMessage>,
            idempotency_key: &str,
        ) -> Result<SessionVersion, AgentWireError> {
            assert_eq!(self.versions[session_id], expected_version.0);
            let next = expected_version.0 + 1;
            self.versions.insert(session_id.into(), next);
            self.submitted
                .push((session_id.into(), idempotency_key.into()));
            self.next_events.insert(
                session_id.into(),
                AgentEventPage {
                    events: vec![AgentEventEnvelope {
                        session_id: session_id.into(),
                        sequence: next,
                        meta: AgentEventMeta::default(),
                        event: AgentEvent::FinalResponse {
                            turn_id: turn_id.into(),
                            summary: format!("reply-{next}"),
                            result: None,
                        },
                    }],
                    next_sequence: next,
                    lost: 0,
                    truncated: false,
                },
            );
            Ok(SessionVersion(next))
        }

        fn cancel_turn(
            &mut self,
            session_id: &str,
            _turn_id: &str,
            expected_version: SessionVersion,
        ) -> Result<SessionVersion, AgentWireError> {
            let next = expected_version.0 + 1;
            self.versions.insert(session_id.into(), next);
            Ok(SessionVersion(next))
        }

        fn fork_session(
            &mut self,
            source_session_id: &str,
            target_session_id: &str,
            expected_version: SessionVersion,
        ) -> Result<SessionVersion, AgentWireError> {
            assert_eq!(self.versions[source_session_id], expected_version.0);
            self.sessions.insert(target_session_id.into());
            self.versions.insert(target_session_id.into(), 1);
            Ok(SessionVersion(1))
        }

        fn events(
            &mut self,
            session_id: &str,
            _after_sequence: u64,
        ) -> Result<AgentEventPage, AgentWireError> {
            Ok(self.next_events.remove(session_id).unwrap())
        }
    }

    #[test]
    fn private_and_group_two_turns_reuse_their_stable_sessions() {
        for target in [
            BotTarget::User {
                user_id: "user".into(),
            },
            BotTarget::Group {
                group_id: "group".into(),
            },
        ] {
            let repository = Arc::new(Repository::default());
            let bridge = BotAgentBridge::new(
                ConversationService::new(repository, policy()),
                Box::new(FakeAgentClient::default()),
                QqStreamingStrategy::FinalOnly,
            );
            let first = bridge.submit_event(&event("one", target.clone())).unwrap();
            let second = bridge.submit_event(&event("two", target)).unwrap();
            assert_eq!(first.binding.session_id, second.binding.session_id);
            assert_eq!(second.binding.session_version, 2);
            assert_eq!(second.binding.last_event_sequence, 2);
            assert_eq!(second.outgoing[0].plain_text(), "reply-2");
        }
    }

    #[test]
    fn denied_event_does_not_create_or_advance_an_agent_session() {
        let repository = Arc::new(Repository::default());
        let mut denied_policy = policy();
        denied_policy.direct_message_policy = DirectMessagePolicy::Deny;
        let bridge = BotAgentBridge::new(
            ConversationService::new(repository.clone(), denied_policy),
            Box::new(FakeAgentClient::default()),
            QqStreamingStrategy::FinalOnly,
        );
        let result = bridge.submit_event(&event(
            "denied",
            BotTarget::User {
                user_id: "actor".into(),
            },
        ));
        assert!(matches!(
            result,
            Err(BotAgentError::Admission(
                ConversationAdmissionError::DirectMessageDenied
            ))
        ));
        assert!(repository.bindings.lock().unwrap().is_empty());
    }

    #[test]
    fn final_response_keeps_non_resource_artifact_summary() {
        let source = event(
            "artifact",
            BotTarget::User {
                user_id: "actor".into(),
            },
        );
        let page = AgentEventPage {
            events: vec![
                AgentEventEnvelope {
                    session_id: "session".into(),
                    sequence: 1,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::FinalResponse {
                        turn_id: "turn".into(),
                        summary: "answer".into(),
                        result: None,
                    },
                },
                AgentEventEnvelope {
                    session_id: "session".into(),
                    sequence: 2,
                    meta: AgentEventMeta::default(),
                    event: AgentEvent::ArtifactProduced {
                        turn_id: "turn".into(),
                        artifact: ArtifactRef {
                            artifact_id: "citation".into(),
                            media_type: "text/plain".into(),
                            summary: "citation summary".into(),
                            content_ref: None,
                            kind: Some("citation".into()),
                            size_bytes: None,
                            content_hash: None,
                            provenance: None,
                            open_hint: None,
                            action_hint: None,
                        },
                    },
                },
            ],
            next_sequence: 2,
            lost: 0,
            truncated: false,
        };

        let outgoing = outgoing_messages(&source, &page, &QqStreamingStrategy::FinalOnly);

        assert_eq!(outgoing[0].plain_text(), "answer\n\ncitation summary");
    }

    #[test]
    fn completed_event_is_suppressed_before_agent_submit_after_bridge_reload() {
        let repository = Arc::new(Repository::default());
        let event = event(
            "duplicate",
            BotTarget::Group {
                group_id: "group".into(),
            },
        );
        let first_bridge = BotAgentBridge::new(
            ConversationService::new(repository.clone(), policy()),
            Box::new(FakeAgentClient::default()),
            QqStreamingStrategy::FinalOnly,
        );
        let first = first_bridge.submit_event(&event).unwrap();
        assert_eq!(first.binding.session_version, 1);
        let reloaded_bridge = BotAgentBridge::new(
            ConversationService::new(repository, policy()),
            Box::new(FakeAgentClient::default()),
            QqStreamingStrategy::FinalOnly,
        );
        let duplicate = reloaded_bridge.submit_event(&event).unwrap();
        assert_eq!(duplicate.binding.session_version, 1);
        assert!(duplicate.outgoing.is_empty());
    }

    #[test]
    fn cancel_fork_reset_and_regenerate_have_fenced_session_transitions() {
        let repository = Arc::new(Repository::default());
        let bridge = BotAgentBridge::new(
            ConversationService::new(repository, policy()),
            Box::new(FakeAgentClient::default()),
            QqStreamingStrategy::FinalOnly,
        );
        let event = event(
            "actions",
            BotTarget::Group {
                group_id: "group".into(),
            },
        );
        let submitted = bridge.submit_event(&event).unwrap();
        let cancelled = bridge.cancel_event(&event, &submitted.turn_id).unwrap();
        assert_eq!(cancelled.session_version, 2);
        let forked = bridge.fork(&event).unwrap();
        assert_ne!(forked.session_id, cancelled.session_id);
        assert_eq!(forked.session_version, 1);
        assert!(forked.generation > cancelled.generation);
        let reset = bridge.reset(&event).unwrap();
        assert_ne!(reset.session_id, submitted.binding.session_id);
        assert_eq!(reset.session_version, 0);
        assert!(reset.generation > forked.generation);
        let regenerated = bridge.regenerate(&event).unwrap();
        assert_eq!(regenerated.binding.session_id, reset.session_id);
        assert_eq!(regenerated.binding.session_version, 1);
        assert!(regenerated.turn_id.contains(":regenerate:"));
    }

    #[test]
    fn validated_image_and_trace_metadata_enter_the_public_agent_message() {
        let mut event = event(
            "media",
            BotTarget::User {
                user_id: "user".into(),
            },
        );
        event
            .message
            .as_mut()
            .unwrap()
            .segments
            .push(MessageSegment::Image {
                resource: media_resource("image", "image/png"),
            });
        let message = event_message(
            &event,
            Some(&BotAgentTraceContext {
                trace_id: Some("trace".into()),
                correlation_id: Some("correlation".into()),
            }),
        )
        .unwrap();
        assert!(matches!(message.parts[0], AgentContentPart::Image { .. }));
        assert_eq!(message.metadata.as_ref().unwrap()["event_id"], "media");
        assert_eq!(message.metadata.as_ref().unwrap()["trace_id"], "trace");

        let MessageSegment::Image { resource } =
            event.message.as_mut().unwrap().segments.last_mut().unwrap()
        else {
            unreachable!()
        };
        resource.content_hash = None;
        assert!(matches!(
            event_message(&event, None),
            Err(BotAgentError::MediaResourceUnvalidated)
        ));
    }

    fn event(id: &str, target: BotTarget) -> BotEvent {
        BotEvent {
            event_id: id.into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: target.clone(),
            actor: Some(BotUser {
                user_id: "actor".into(),
                display_name: None,
                avatar_url: None,
            }),
            message: Some(BotMessage::text(target, "hello")),
            raw: None,
            ext: BTreeMap::new(),
        }
    }

    fn policy() -> ConversationPolicy {
        ConversationPolicy {
            revision: 1,
            enabled: true,
            agent_enabled: true,
            direct_message_policy: DirectMessagePolicy::Allow,
            must_mention: false,
            wake_words: Vec::new(),
            allowlist: Vec::new(),
            denylist: Vec::new(),
            rate_limit_profile_id: None,
            session_scope: AgentSessionScope::SharedConversation,
            business_profile_binding_id: None,
            agent_runtime_profile_id: Some("profile".into()),
            stt_enabled: false,
            tts_enabled: false,
            speech_reply_policy: BotSpeechReplyPolicy::default(),
            stt_selector_id: None,
            tts_selector_id: None,
            active_delivery_enabled: false,
        }
    }

    fn session(id: &str) -> AgentSession {
        AgentSession::new(
            id,
            "profile",
            resource(id),
            ResourceCellRef {
                cell_id: format!("cell-{id}"),
                resource_kind: "agent.session".into(),
                owner_plugin_id: "test".into(),
                schema: "agent.session.v1".into(),
                generation: 1,
                health: "ready".into(),
                reload_policy: "retain".into(),
            },
        )
    }

    fn resource(id: &str) -> ResourceRef {
        ResourceRef {
            ref_id: format!("ref-{id}"),
            resource_id: ResourceId {
                kind_id: "agent.session".into(),
                slot_id: id.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::VersionedSnapshot,
            provider_id: "test".into(),
            resource_kind: "agent.session".into(),
            schema: "agent.session.v1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn media_resource(id: &str, schema: &str) -> ResourceRef {
        let mut resource = resource(id);
        resource.resource_kind = "blob".into();
        resource.schema = schema.into();
        resource.size_hint = Some(4);
        resource.content_hash = Some(format!("sha256:{id}"));
        resource
    }

    fn wire_error(code: &str) -> AgentWireError {
        AgentWireError {
            code: code.into(),
            message: code.into(),
            retryable: false,
        }
    }
}
