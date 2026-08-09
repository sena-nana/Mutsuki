use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mutsuki_agent_contracts::{
    AGENT_SPEECH_SYNTHESIZE_PROTOCOL, AGENT_TRANSCRIBE_PROTOCOL, AgentError, AgentEvent,
    AgentEventEnvelope, AgentEventMeta, AgentEventPage, AgentMessage, AgentSession,
    AgentSessionCreateRequest, AgentWireError, MediaService, ResourceCellRef,
    ScheduleExecutionStatus, ScheduleTargetRef, ScheduledRunResult, SessionVersion,
    SpeechSynthesisResult, TranscriptionResult,
};
use mutsuki_bot_config::{ConfigAction, ConfigApplyRequest, ConfigContext, ConfigValue};
use mutsuki_bot_conversation::{AgentEventClaim, ConversationError, ConversationRepository};
use mutsuki_bot_delivery::{
    BOT_DELIVERY_PLUGIN_ID, BOT_SCHEDULED_DELIVERY_PLUGIN_ID, BOT_SCHEDULED_DELIVERY_PROTOCOL_ID,
    DeliveryError, DeliveryPolicyResolver, DeliveryRepository, QqDeliveryFailure,
    QqDeliveryGateway, QqDeliverySuccess, SCHEDULE_TARGET_KIND_BOT_CONVERSATION_BINDING,
    ScheduledDeliveryPolicyProvider, ScheduledDeliveryRequest, ScheduledDeliveryTargetResolver,
};
use mutsuki_bot_interaction::{
    BOT_INTERACTION_PLUGIN_ID, InteractionConditionMatcher, InteractionError, InteractionRepository,
};
use mutsuki_bot_protocol::{
    AgentSessionBinding, AgentSessionScope, BOT_EVENT_INGEST_PROTOCOL_ID,
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotAccountRef, BotActiveDeliveryRequest, BotDeliveryAttempt,
    BotDeliveryContent, BotDeliveryReceipt, BotEvent, BotEventKind, BotInteractionCommand,
    BotInteractionSession, BotMessage, BotPermissionCheckRequest, BotPermissionCheckResult,
    BotPlatform, BotPropagationPolicy, BotSpeechReplyPolicy, BotTarget, BotUser,
    ConversationPolicy, ConversationPolicyRule, DeliveryStatus, DirectMessagePolicy,
    InteractionScope, InteractionStatus, InteractionWaitSpec, MessageSegment, QqConversationRef,
};
use mutsuki_bot_service_host_integration::QqAiBotPluginBundle;
use mutsuki_bot_web_console::{
    ControlPluginReloadLifecycle, ProductConfigOptions, product_config_service_with_options,
};
use mutsuki_plugin_bot_agent::{
    AgentBridgeClient, BOT_AGENT_BRIDGE_PLUGIN_ID, BOT_AGENT_CONFIG_PROVIDER_ID,
    BOT_AGENT_CONFIG_SERVICE_ID, BotAgentConfigHandle,
};
use mutsuki_plugin_bot_command::BOT_COMMAND_PLUGIN_ID;
use mutsuki_plugin_bot_event_router::{
    BOT_HANDLER_GUARD_PLUGIN_ID, BOT_HANDLER_PIPELINE_PLUGIN_ID, BotPermissionAuthorizer,
};
use mutsuki_plugin_bot_media::BOT_MEDIA_BRIDGE_PLUGIN_ID;
use mutsuki_runtime_contracts::{
    CancelPolicy, CompletionBatch, ExecutionClass, ResourceAccess, ResourceId, ResourceLifetime,
    ResourceRef, ResourceSealState, ResourceSemantic, RunnerResult, Task, TaskHandle, TaskOutcome,
    TaskStatus, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use mutsuki_service_config::{ConfiguredPluginSelection, ConfiguredPluginStore, ServiceConfig};
use mutsuki_service_runtime::{ServiceRuntime, ServiceRuntimeBuilder};

#[derive(Default)]
struct State {
    bindings: Mutex<BTreeMap<String, AgentSessionBinding>>,
    events: Mutex<BTreeMap<(String, String), bool>>,
    delivery_requests: Mutex<BTreeMap<String, BotActiveDeliveryRequest>>,
    delivery_keys: Mutex<BTreeMap<String, String>>,
    delivery_attempts: Mutex<Vec<BotDeliveryAttempt>>,
    delivery_receipts: Mutex<BTreeMap<String, BotDeliveryReceipt>>,
    interactions: Mutex<BTreeMap<String, BotInteractionSession>>,
}

#[async_trait]
impl ConversationRepository for State {
    async fn policy_rules(&self) -> Result<Vec<ConversationPolicyRule>, ConversationError> {
        Ok(Vec::new())
    }

    async fn session_binding(
        &self,
        binding_key: &str,
    ) -> Result<Option<AgentSessionBinding>, ConversationError> {
        Ok(self.bindings.lock().unwrap().get(binding_key).cloned())
    }

    async fn compare_and_set_session_binding(
        &self,
        binding_key: &str,
        expected_generation: Option<u64>,
        binding: AgentSessionBinding,
    ) -> Result<(), ConversationError> {
        let mut bindings = self.bindings.lock().unwrap();
        if bindings.get(binding_key).map(|binding| binding.generation) != expected_generation {
            return Err(ConversationError::GenerationConflict);
        }
        bindings.insert(binding_key.into(), binding);
        Ok(())
    }

    async fn begin_agent_event(
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

    async fn complete_agent_event(
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

#[async_trait]
impl DeliveryRepository for State {
    async fn reserve(
        &self,
        request: &BotActiveDeliveryRequest,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError> {
        if let Some(delivery_id) = self
            .delivery_keys
            .lock()
            .unwrap()
            .get(&request.idempotency_key)
        {
            return Ok(self
                .delivery_receipts
                .lock()
                .unwrap()
                .get(delivery_id)
                .cloned());
        }
        self.delivery_keys
            .lock()
            .unwrap()
            .insert(request.idempotency_key.clone(), request.delivery_id.clone());
        self.delivery_requests
            .lock()
            .unwrap()
            .insert(request.delivery_id.clone(), request.clone());
        self.delivery_receipts.lock().unwrap().insert(
            request.delivery_id.clone(),
            BotDeliveryReceipt {
                delivery_id: request.delivery_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                status: DeliveryStatus::Pending,
                attempt_count: 0,
                platform_message_ids: Vec::new(),
                part_receipts: Vec::new(),
                delivered_at_unix_ms: None,
                error_code: None,
                generation: 0,
                lease_expires_at_unix_ms: None,
            },
        );
        Ok(None)
    }

    async fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
        self.delivery_requests
            .lock()
            .unwrap()
            .get(delivery_id)
            .cloned()
            .ok_or(DeliveryError::NotFound)
    }

    async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        self.delivery_receipts
            .lock()
            .unwrap()
            .get(delivery_id)
            .cloned()
            .ok_or(DeliveryError::NotFound)
    }

    async fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
        Ok(self
            .delivery_attempts
            .lock()
            .unwrap()
            .iter()
            .filter(|attempt| attempt.delivery_id == delivery_id)
            .cloned()
            .collect())
    }

    async fn save_outcome(
        &self,
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
    ) -> Result<(), DeliveryError> {
        self.delivery_attempts.lock().unwrap().push(attempt);
        self.delivery_receipts
            .lock()
            .unwrap()
            .insert(receipt.delivery_id.clone(), receipt);
        Ok(())
    }

    async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
        self.delivery_receipts
            .lock()
            .unwrap()
            .insert(receipt.delivery_id.clone(), receipt);
        Ok(())
    }

    async fn claim_due_delivery_ids(
        &self,
        _now_unix_ms: u64,
    ) -> Result<Vec<String>, DeliveryError> {
        Ok(Vec::new())
    }

    async fn begin_send(
        &self,
        delivery_id: &str,
        attempt: BotDeliveryAttempt,
        now_unix_ms: u64,
        lease_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        self.delivery_attempts.lock().unwrap().push(attempt);
        let mut receipts = self.delivery_receipts.lock().unwrap();
        let receipt =
            receipts
                .entry(delivery_id.to_owned())
                .or_insert_with(|| BotDeliveryReceipt {
                    delivery_id: delivery_id.to_owned(),
                    idempotency_key: delivery_id.to_owned(),
                    status: DeliveryStatus::Pending,
                    attempt_count: 0,
                    platform_message_ids: Vec::new(),
                    part_receipts: Vec::new(),
                    delivered_at_unix_ms: None,
                    error_code: None,
                    generation: 0,
                    lease_expires_at_unix_ms: None,
                });
        receipt.status = DeliveryStatus::Sending;
        receipt.generation = receipt.generation.saturating_add(1);
        receipt.lease_expires_at_unix_ms = Some(now_unix_ms.saturating_add(lease_ms));
        Ok(receipt.clone())
    }
}

#[async_trait]
impl InteractionRepository for State {
    async fn create(&self, session: BotInteractionSession) -> Result<(), InteractionError> {
        self.interactions
            .lock()
            .unwrap()
            .insert(session.session_id.clone(), session);
        Ok(())
    }

    async fn active_for_origin(
        &self,
        origin_key: &str,
    ) -> Result<Vec<BotInteractionSession>, InteractionError> {
        Ok(self
            .interactions
            .lock()
            .unwrap()
            .values()
            .filter(|session| {
                session.status == mutsuki_bot_protocol::InteractionStatus::Waiting
                    && session.conversation.origin_key() == origin_key
            })
            .cloned()
            .collect())
    }

    async fn compare_and_set(
        &self,
        expected_version: u64,
        session: BotInteractionSession,
    ) -> Result<(), InteractionError> {
        let mut interactions = self.interactions.lock().unwrap();
        if interactions
            .get(&session.session_id)
            .map(|current| current.version)
            != Some(expected_version)
        {
            return Err(InteractionError::GenerationConflict);
        }
        interactions.insert(session.session_id.clone(), session);
        Ok(())
    }

    async fn recover_waiting(&self) -> Result<Vec<BotInteractionSession>, InteractionError> {
        Ok(self
            .interactions
            .lock()
            .unwrap()
            .values()
            .filter(|session| session.status == mutsuki_bot_protocol::InteractionStatus::Waiting)
            .cloned()
            .collect())
    }
}

struct Agent {
    sessions: BTreeSet<String>,
    versions: BTreeMap<String, u64>,
    submits: Arc<AtomicUsize>,
    inputs: Arc<Mutex<Vec<String>>>,
    event_after_sequences: Arc<Mutex<Vec<u64>>>,
}

impl AgentBridgeClient for Agent {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.sessions
            .contains(session_id)
            .then(|| agent_session(session_id))
            .ok_or_else(|| wire_error("agent.session.not_found"))
    }

    fn start_session(
        &mut self,
        request: AgentSessionCreateRequest,
    ) -> Result<AgentSession, AgentWireError> {
        let session_id = request
            .session_id
            .ok_or_else(|| wire_error("agent.session.id_missing"))?;
        self.sessions.insert(session_id.clone());
        self.versions.insert(session_id.clone(), 0);
        Ok(agent_session(&session_id))
    }

    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        _turn_id: &str,
        messages: Vec<AgentMessage>,
        _idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        if self.versions.get(session_id).copied() != Some(expected_version.0) {
            return Err(wire_error("agent.session.version_conflict"));
        }
        let next = expected_version.0 + 1;
        self.versions.insert(session_id.into(), next);
        self.submits.fetch_add(1, Ordering::SeqCst);
        self.inputs
            .lock()
            .unwrap()
            .extend(messages.into_iter().map(|message| message.content));
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
        if self.versions.get(source_session_id).copied() != Some(expected_version.0) {
            return Err(wire_error("agent.session.version_conflict"));
        }
        self.sessions.insert(target_session_id.into());
        self.versions.insert(target_session_id.into(), 1);
        Ok(SessionVersion(1))
    }

    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
        self.event_after_sequences
            .lock()
            .unwrap()
            .push(after_sequence);
        let sequence = after_sequence + 1;
        Ok(AgentEventPage {
            events: vec![AgentEventEnvelope {
                session_id: session_id.into(),
                sequence,
                meta: AgentEventMeta::default(),
                event: AgentEvent::FinalResponse {
                    turn_id: "turn".into(),
                    summary: "Agent reply".into(),
                    result: None,
                },
            }],
            next_sequence: sequence,
            lost: 0,
            truncated: false,
        })
    }
}

struct Media;

impl MediaService for Media {
    fn transcode(
        &self,
        source: &ResourceRef,
        target_mime: &str,
    ) -> Result<ResourceRef, AgentError> {
        let mut output = source.clone();
        output.schema = target_mime.into();
        Ok(output)
    }

    fn resample(
        &self,
        source: &ResourceRef,
        _sample_rate_hz: u32,
    ) -> Result<ResourceRef, AgentError> {
        Ok(source.clone())
    }
}

struct DeliveryGateway {
    sends: Arc<AtomicUsize>,
}

impl QqDeliveryGateway for DeliveryGateway {
    fn send(
        &self,
        _conversation: &QqConversationRef,
        _content: &BotDeliveryContent,
    ) -> Result<QqDeliverySuccess, QqDeliveryFailure> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        Ok(QqDeliverySuccess {
            platform_message_ids: vec!["scheduled-qq-message".into()],
            part_receipts: Vec::new(),
        })
    }
}

struct Allow;

impl DeliveryPolicyResolver for Allow {
    fn active_delivery_allowed(
        &self,
        _conversation: &QqConversationRef,
    ) -> Result<bool, DeliveryError> {
        Ok(true)
    }
}

impl ScheduledDeliveryTargetResolver for Allow {
    fn resolve_binding(&self, binding_id: &str) -> Result<QqConversationRef, DeliveryError> {
        if binding_id != "daily-target" {
            return Err(DeliveryError::InvalidScheduleTarget);
        }
        Ok(QqConversationRef {
            version: mutsuki_bot_protocol::QQ_CONVERSATION_REF_VERSION,
            account_id: "main".into(),
            kind: mutsuki_bot_protocol::BotConversationKind::Private,
            user_id: Some("actor".into()),
            group_id: None,
            guild_id: None,
            channel_id: None,
            thread_id: None,
        })
    }
}

impl ScheduledDeliveryPolicyProvider for Allow {
    fn delivery_policy(
        &self,
        _conversation: &QqConversationRef,
    ) -> Result<mutsuki_bot_protocol::DeliveryPolicy, DeliveryError> {
        Ok(mutsuki_bot_protocol::DeliveryPolicy {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 1_000,
            not_before_unix_ms: None,
            expires_at_unix_ms: None,
        })
    }
}

impl InteractionConditionMatcher for Allow {
    fn command_matches(&self, _command: &str, _event: &BotEvent) -> Result<bool, InteractionError> {
        Ok(true)
    }

    fn predicate_matches(
        &self,
        _service_id: &str,
        _event: &BotEvent,
    ) -> Result<bool, InteractionError> {
        Ok(true)
    }
}

impl BotPermissionAuthorizer for Allow {
    fn check(
        &self,
        _request: &BotPermissionCheckRequest,
    ) -> Result<BotPermissionCheckResult, String> {
        Ok(BotPermissionCheckResult {
            allowed: true,
            denial_code: None,
            audit_reason: None,
        })
    }
}

struct MessageSendRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    sends: Arc<AtomicUsize>,
    audio_sends: Arc<AtomicUsize>,
    failures: Arc<AtomicBool>,
}

impl Runner for MessageSendRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let message: BotMessage = serde_json::from_value(task.payload.to_value()).unwrap();
            if self.failures.load(Ordering::SeqCst) {
                return Err(mutsuki_runtime_contracts::RuntimeError::new(
                    "test.qq.send.failed",
                    "test.qq.send",
                    "configured delivery failure",
                ));
            }
            if message
                .segments
                .iter()
                .any(|segment| matches!(segment, MessageSegment::Audio { .. }))
            {
                self.audio_sends.fetch_add(1, Ordering::SeqCst);
            } else {
                assert_eq!(message.plain_text(), "Agent reply");
            }
            self.sends.fetch_add(1, Ordering::SeqCst);
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(serde_json::json!({"id": "qq-message"}));
            Ok(result)
        })
    }
}

struct AgentMediaRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    transcriptions: Arc<AtomicUsize>,
    syntheses: Arc<AtomicUsize>,
}

impl Runner for AgentMediaRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let output = match task.protocol_id.as_str() {
                AGENT_TRANSCRIBE_PROTOCOL => {
                    self.transcriptions.fetch_add(1, Ordering::SeqCst);
                    serde_json::to_value(TranscriptionResult {
                        text: "voice transcript".into(),
                        language: Some("zh".into()),
                        segments: Vec::new(),
                        transcript_ref: None,
                    })
                    .unwrap()
                }
                AGENT_SPEECH_SYNTHESIZE_PROTOCOL => {
                    self.syntheses.fetch_add(1, Ordering::SeqCst);
                    serde_json::to_value(SpeechSynthesisResult {
                        audio: media_resource("tts", "audio/wav"),
                        format: "wav".into(),
                        sample_rate_hz: Some(24_000),
                        duration_ms: Some(100),
                    })
                    .unwrap()
                }
                protocol => panic!("unexpected Agent media protocol {protocol}"),
            };
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(output);
            Ok(result)
        })
    }
}

#[tokio::test]
async fn service_runtime_routes_qq_event_through_agent_and_suppresses_replay() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("logs")).unwrap();
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.ipc.token = Some("issue-144-test-control-token".into());
    config.observe.console = false;
    config.service.home_dir = root.path().into();
    config.service.data_dir = root.path().join("data");
    config.service.log_dir = root.path().join("logs");
    config.service.run_dir = root.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = root.path().join("disabled");
    config.plugins.configured = [
        BOT_HANDLER_PIPELINE_PLUGIN_ID,
        BOT_HANDLER_GUARD_PLUGIN_ID,
        BOT_COMMAND_PLUGIN_ID,
        BOT_AGENT_BRIDGE_PLUGIN_ID,
        BOT_MEDIA_BRIDGE_PLUGIN_ID,
        BOT_DELIVERY_PLUGIN_ID,
        BOT_SCHEDULED_DELIVERY_PLUGIN_ID,
        BOT_INTERACTION_PLUGIN_ID,
        "test.qq.send",
        "test.agent.media",
    ]
    .into_iter()
    .map(|id| ConfiguredPluginSelection {
        id: id.into(),
        enabled: true,
        config: serde_json::Value::Null,
    })
    .collect();

    let state = Arc::new(State::default());
    let agent_submits = Arc::new(AtomicUsize::new(0));
    let agent_inputs = Arc::new(Mutex::new(Vec::new()));
    let event_after_sequences = Arc::new(Mutex::new(Vec::new()));
    let qq_sends = Arc::new(AtomicUsize::new(0));
    let qq_audio_sends = Arc::new(AtomicUsize::new(0));
    let qq_failures = Arc::new(AtomicBool::new(false));
    let transcriptions = Arc::new(AtomicUsize::new(0));
    let syntheses = Arc::new(AtomicUsize::new(0));
    let scheduled_sends = Arc::new(AtomicUsize::new(0));
    let send_descriptor = RunnerDescriptorBuilder::new("test.qq.send.runner", "test.qq.send")
        .accepted_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
        .execution_class(ExecutionClass::Io)
        .build();
    let send_manifest = PluginBuilder::new("test.qq.send")
        .runner_descriptor(send_descriptor.clone())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_MESSAGE_SEND_PROTOCOL_ID).build(),
            "test.qq.send.runner",
            "qq-send",
        )
        .build()
        .manifest;
    let send_factory_descriptor = send_descriptor.clone();
    let send_factory_count = qq_sends.clone();
    let send_factory_audio_count = qq_audio_sends.clone();
    let send_factory_failures = qq_failures.clone();
    let agent_media_descriptor =
        RunnerDescriptorBuilder::new("test.agent.media.runner", "test.agent.media")
            .accepted_protocol(AGENT_TRANSCRIBE_PROTOCOL)
            .accepted_protocol(AGENT_SPEECH_SYNTHESIZE_PROTOCOL)
            .execution_class(ExecutionClass::Io)
            .build();
    let agent_media_manifest = PluginBuilder::new("test.agent.media")
        .runner_descriptor(agent_media_descriptor.clone())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(AGENT_TRANSCRIBE_PROTOCOL).build(),
            "test.agent.media.runner",
            "agent-transcribe",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(AGENT_SPEECH_SYNTHESIZE_PROTOCOL).build(),
            "test.agent.media.runner",
            "agent-synthesize",
        )
        .build()
        .manifest;
    let media_factory_descriptor = agent_media_descriptor.clone();
    let media_factory_transcriptions = transcriptions.clone();
    let media_factory_syntheses = syntheses.clone();

    let policy = ConversationPolicy {
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
        stt_enabled: true,
        tts_enabled: true,
        speech_reply_policy: BotSpeechReplyPolicy::TextAndVoice,
        stt_selector_id: None,
        tts_selector_id: None,
        active_delivery_enabled: true,
    };
    let agent = Agent {
        sessions: BTreeSet::new(),
        versions: BTreeMap::new(),
        submits: agent_submits.clone(),
        inputs: agent_inputs.clone(),
        event_after_sequences: event_after_sequences.clone(),
    };
    let builder = QqAiBotPluginBundle::new(
        state.clone(),
        state.clone(),
        state.clone(),
        policy,
        Box::new(agent),
        Arc::new(Media),
        Arc::new(DeliveryGateway {
            sends: scheduled_sends.clone(),
        }),
        Arc::new(Allow),
        Arc::new(Allow),
        Arc::new(Allow),
    )
    .with_scheduled_delivery(Arc::new(Allow), Arc::new(Allow))
    .install(
        ServiceRuntimeBuilder::new(config)
            .register_builtin_plugin(send_manifest)
            .register_builtin_plugin(agent_media_manifest)
            .register_builtin_runner(move || {
                Box::new(MessageSendRunner {
                    descriptor: send_factory_descriptor.clone(),
                    sends: send_factory_count.clone(),
                    audio_sends: send_factory_audio_count.clone(),
                    failures: send_factory_failures.clone(),
                })
            })
            .register_builtin_runner(move || {
                Box::new(AgentMediaRunner {
                    descriptor: media_factory_descriptor.clone(),
                    transcriptions: media_factory_transcriptions.clone(),
                    syntheses: media_factory_syntheses.clone(),
                })
            }),
    );
    let runtime = builder.start().await.unwrap();
    let live_agent_config = runtime
        .host_service::<BotAgentConfigHandle>(BOT_AGENT_CONFIG_SERVICE_ID)
        .unwrap();
    assert!(live_agent_config.snapshot().enabled);
    let product_config_path = root.path().join("product.toml");
    std::fs::write(
        &product_config_path,
        r#"
[[plugins.configured]]
id = "mutsuki.plugin.bot.agent"
config = { enabled = true, connection_id = "injected", default_profile_id = "", streaming = "final_only", max_concurrency = 1, timeout_ms = 120000, max_message_bytes = 1800 }
"#,
    )
    .unwrap();
    let config_service = product_config_service_with_options(
        &product_config_path,
        ProductConfigOptions {
            store: Some(ConfiguredPluginStore::open(&product_config_path)),
            lifecycle: Some(Arc::new(ControlPluginReloadLifecycle::new(
                runtime.control_handler(),
                runtime.control_token(),
            ))),
            bot_agent_config: Some((*live_agent_config).clone()),
            agent_connections: None,
        },
    )
    .unwrap();
    let config_caps = vec!["*".into()];
    let snapshot = config_service
        .read(
            BOT_AGENT_CONFIG_PROVIDER_ID,
            ConfigContext::plugin_instance("default"),
            &config_caps,
        )
        .await
        .unwrap();
    let mut candidate = snapshot.value;
    candidate
        .as_object_mut()
        .unwrap()
        .insert("max_concurrency".into(), ConfigValue::Integer(2));
    candidate
        .as_object_mut()
        .unwrap()
        .insert("timeout_ms".into(), ConfigValue::Integer(10_000));
    let applied = config_service
        .apply(
            BOT_AGENT_CONFIG_PROVIDER_ID,
            ConfigApplyRequest {
                candidate,
                expected_revision: snapshot.revision,
                dry_run: false,
            },
            ConfigContext::plugin_instance("default"),
            &config_caps,
        )
        .await
        .unwrap();
    assert!(applied.applied);
    assert!(
        applied
            .actions
            .iter()
            .any(|action| matches!(action, ConfigAction::PluginReloaded))
    );
    assert_eq!(live_agent_config.snapshot().max_concurrency, 2);
    assert_eq!(live_agent_config.snapshot().timeout_ms, 10_000);
    let persisted: toml::Value =
        toml::from_str(&std::fs::read_to_string(&product_config_path).unwrap()).unwrap();
    assert_eq!(
        persisted["plugins"]["configured"][0]["config"]["max_concurrency"].as_integer(),
        Some(2)
    );
    let event = event();

    let conversation = mutsuki_bot_conversation::qq_conversation_from_event(&event).unwrap();
    let interaction = BotInteractionSession {
        session_id: "verification-flow".into(),
        conversation,
        scope: InteractionScope::ActorInConversation,
        actor_id: Some("actor".into()),
        state_ref_id: "verify-code".into(),
        wait: InteractionWaitSpec {
            event_kinds: vec![BotEventKind::MessageCreated],
            command: None,
            predicate_service_id: None,
            timeout_at_unix_ms: 10_000,
            propagation: BotPropagationPolicy::ConsumeOnSuccess,
            retry_prompt: None,
        },
        status: InteractionStatus::Waiting,
        generation: 1,
        version: 1,
        exclusive: true,
        retries_remaining: 2,
    };
    let create = runtime
        .submit_task(Task::new(
            "interaction-create",
            mutsuki_bot_protocol::BOT_INTERACTION_SESSION_PROTOCOL_ID,
            serde_json::to_value(BotInteractionCommand::Create {
                session: interaction,
            })
            .unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &create).await,
        TaskOutcome::Completed { .. }
    ));
    let mut step_one = event.clone();
    step_one.event_id = "verification-step-one".into();
    let step_one = runtime
        .submit_task(Task::new(
            "interaction-step-one",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(step_one).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &step_one).await,
        TaskOutcome::Completed { .. }
    ));
    assert_eq!(agent_submits.load(Ordering::SeqCst), 0);
    let completed_step = state.interactions.lock().unwrap()["verification-flow"].clone();
    assert_eq!(completed_step.status, InteractionStatus::Completed);
    let transition = runtime
        .submit_task(Task::new(
            "interaction-transition",
            mutsuki_bot_protocol::BOT_INTERACTION_SESSION_PROTOCOL_ID,
            serde_json::to_value(BotInteractionCommand::Transition {
                session: completed_step,
                next_state_ref_id: "confirm-account".into(),
                next_wait: InteractionWaitSpec {
                    event_kinds: vec![BotEventKind::MessageCreated],
                    command: None,
                    predicate_service_id: None,
                    timeout_at_unix_ms: 10_000,
                    propagation: BotPropagationPolicy::ConsumeOnSuccess,
                    retry_prompt: None,
                },
                retries_remaining: 1,
            })
            .unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &transition).await,
        TaskOutcome::Completed { .. }
    ));
    let mut step_two = event.clone();
    step_two.event_id = "verification-step-two".into();
    let step_two = runtime
        .submit_task(Task::new(
            "interaction-step-two",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(step_two).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &step_two).await,
        TaskOutcome::Completed { .. }
    ));
    let interaction = state.interactions.lock().unwrap()["verification-flow"].clone();
    assert_eq!(interaction.status, InteractionStatus::Completed);
    assert_eq!(interaction.state_ref_id, "confirm-account");
    assert_eq!(agent_submits.load(Ordering::SeqCst), 0);

    let first = runtime
        .submit_task(Task::new(
            "qq-ingest-1",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(&event).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &first).await,
        TaskOutcome::Completed { .. }
    ));

    let replay = runtime
        .submit_task(Task::new(
            "qq-ingest-2",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(&event).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &replay).await,
        TaskOutcome::Completed { .. }
    ));
    assert_eq!(agent_submits.load(Ordering::SeqCst), 1);
    assert_eq!(agent_inputs.lock().unwrap()[0], "hello\nvoice transcript");
    assert_eq!(transcriptions.load(Ordering::SeqCst), 1);
    assert_eq!(syntheses.load(Ordering::SeqCst), 1);
    assert_eq!(qq_sends.load(Ordering::SeqCst), 2);
    assert_eq!(qq_audio_sends.load(Ordering::SeqCst), 1);
    assert_eq!(
        state
            .bindings
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .session_version,
        1
    );

    let mut private_second = event.clone();
    private_second.event_id = "qq-private-event-2".into();
    private_second.message = Some(BotMessage::text(
        private_second.target.clone(),
        "private two",
    ));
    let private_second_handle = runtime
        .submit_task(Task::new(
            "qq-private-ingest-2",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(private_second).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &private_second_handle).await,
        TaskOutcome::Completed { .. }
    ));
    wait_for_count(&agent_submits, 2).await;
    wait_for_count(&qq_sends, 4).await;
    assert_eq!(agent_inputs.lock().unwrap()[1], "private two");
    assert_eq!(syntheses.load(Ordering::SeqCst), 2);

    let mut command = event;
    command.event_id = "qq-command-event".into();
    command.message = Some(BotMessage::text(
        command.target.clone(),
        "/ask command prompt",
    ));
    let command_handle = runtime
        .submit_task(Task::new(
            "qq-command-ingest",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(command).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &command_handle).await,
        TaskOutcome::Completed { .. }
    ));
    wait_for_count(&agent_submits, 3).await;
    wait_for_count(&qq_sends, 6).await;
    assert_eq!(agent_inputs.lock().unwrap()[2], "command prompt");
    assert_eq!(syntheses.load(Ordering::SeqCst), 3);
    assert_eq!(qq_sends.load(Ordering::SeqCst), 6);

    let group_first = runtime
        .submit_task(Task::new(
            "qq-group-ingest-1",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(group_event("qq-group-event-1", "group one")).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &group_first).await,
        TaskOutcome::Completed { .. }
    ));
    let group_second = runtime
        .submit_task(Task::new(
            "qq-group-ingest-2",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(group_event("qq-group-event-2", "group two")).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(&runtime, &group_second).await,
        TaskOutcome::Completed { .. }
    ));
    assert_eq!(agent_submits.load(Ordering::SeqCst), 5);
    {
        let agent_inputs = agent_inputs.lock().unwrap();
        assert_eq!(agent_inputs[3], "group one");
        assert_eq!(agent_inputs[4], "group two");
    }
    let group_bindings = state
        .bindings
        .lock()
        .unwrap()
        .values()
        .filter(|binding| binding.origin_key.contains("group"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(group_bindings.len(), 1);
    assert_eq!(group_bindings[0].session_version, 2);

    let qq_sends_before_failure = qq_sends.load(Ordering::SeqCst);
    qq_failures.store(true, Ordering::SeqCst);
    let failed_delivery = runtime
        .submit_task(Task::new(
            "qq-delivery-failure-ingest",
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(group_event("qq-delivery-failure", "delivery failure")).unwrap(),
        ))
        .unwrap();
    let failed_delivery_outcome = wait_outcome(&runtime, &failed_delivery).await;
    assert!(matches!(
        failed_delivery_outcome,
        TaskOutcome::Completed { .. }
    ));
    let agent_task = runtime
        .task_snapshots()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.task_id == "qq-delivery-failure-ingest:call:3")
        .expect("Agent bridge child task is present");
    assert!(matches!(agent_task.status, TaskStatus::Completed));
    let agent_outcome = runtime
        .task_outcome(&TaskHandle {
            task_id: agent_task.task_id.clone(),
            protocol_id: agent_task.protocol_id.clone(),
            target_binding_id: agent_task.target_binding_id.clone(),
            cancel_policy: CancelPolicy::Cascade,
            trace_id: agent_task.trace_id.clone(),
            correlation_id: agent_task.correlation_id.clone(),
        })
        .unwrap()
        .unwrap();
    let TaskOutcome::Completed {
        output: Some(output),
        ..
    } = agent_outcome
    else {
        panic!("Agent bridge child must retain a completed output");
    };
    assert!(
        output["delivery_errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error == "test.qq.send.failed")
    );
    wait_for_count(&agent_submits, 6).await;
    assert_eq!(qq_sends.load(Ordering::SeqCst), qq_sends_before_failure);
    assert_eq!(
        *event_after_sequences.lock().unwrap(),
        vec![0, 1, 2, 0, 1, 2]
    );

    let scheduled = ScheduledRunResult {
        schedule_id: "daily".into(),
        execution_id: "execution-stable".into(),
        status: ScheduleExecutionStatus::Succeeded,
        summary: "scheduled result".into(),
        output_ref: None,
        target: Some(ScheduleTargetRef {
            target_id: "daily-target".into(),
            kind: SCHEDULE_TARGET_KIND_BOT_CONVERSATION_BINDING.into(),
            metadata: serde_json::json!({"qq_id": "must-not-be-trusted"}),
        }),
    };
    for task_id in ["scheduled-delivery-1", "scheduled-delivery-replay"] {
        let handle = runtime
            .submit_task(Task::new(
                task_id,
                BOT_SCHEDULED_DELIVERY_PROTOCOL_ID,
                serde_json::to_value(ScheduledDeliveryRequest {
                    result: scheduled.clone(),
                    now_unix_ms: 1_000,
                })
                .unwrap(),
            ))
            .unwrap();
        assert!(matches!(
            wait_outcome(&runtime, &handle).await,
            TaskOutcome::Completed { .. }
        ));
    }
    assert_eq!(scheduled_sends.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}

async fn wait_outcome(runtime: &ServiceRuntime, handle: &TaskHandle) -> TaskOutcome {
    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(outcome) = runtime.task_outcome(handle).unwrap() {
                return outcome;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    match outcome {
        Ok(outcome) => outcome,
        Err(_) => panic!("task did not complete: {:?}", runtime.task_snapshots()),
    }
}

async fn wait_for_count(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while counter.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("counter reaches expected value");
}

fn event() -> BotEvent {
    BotEvent {
        event_id: "qq-event-stable".into(),
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            account_id: "main".into(),
            platform: BotPlatform::QqBot,
        },
        kind: BotEventKind::MessageCreated,
        time_ms: 1,
        target: BotTarget::User {
            user_id: "actor".into(),
        },
        actor: Some(BotUser {
            user_id: "actor".into(),
            display_name: None,
            avatar_url: None,
        }),
        message: Some(BotMessage {
            message_id: Some("message".into()),
            target: BotTarget::User {
                user_id: "actor".into(),
            },
            sender: None,
            segments: vec![
                MessageSegment::text("hello"),
                MessageSegment::Audio {
                    resource: media_resource("incoming", "audio/silk"),
                },
            ],
            reply_to: None,
            time_ms: Some(1),
            ext: BTreeMap::new(),
        }),
        raw: None,
        ext: BTreeMap::new(),
    }
}

fn group_event(event_id: &str, text: &str) -> BotEvent {
    let mut event = event();
    event.event_id = event_id.into();
    event.target = BotTarget::Group {
        group_id: "group".into(),
    };
    event.message = Some(BotMessage::text(event.target.clone(), text));
    event
}

fn agent_session(id: &str) -> AgentSession {
    AgentSession::new(
        id,
        "profile",
        ResourceRef {
            resource_id: ResourceId {
                kind_id: "agent.session".into(),
                slot_id: id.into(),
                generation: 1,
                version: 1,
            },
            ref_id: format!("ref-{id}"),
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
        },
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

fn wire_error(code: &str) -> AgentWireError {
    AgentWireError {
        code: code.into(),
        message: code.into(),
        retryable: false,
    }
}

fn media_resource(id: &str, schema: &str) -> ResourceRef {
    let mut resource = agent_session(id).resource;
    resource.resource_kind = "blob".into();
    resource.schema = schema.into();
    resource.size_hint = Some(4);
    resource.content_hash = Some(format!("sha256:{id}"));
    resource
}
