use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_agent_client::{AgentClientBackend, InProcessAgentClient, InProcessAgentService};
use mutsuki_agent_contracts::{
    AgentEvent, AgentEventEnvelope, AgentEventPage, AgentSession, AgentWireError, AgentWireRequest,
    AgentWireRequestEnvelope, AgentWireResponse, AgentWireResponseEnvelope, ResourceCellRef,
    SessionVersion,
};
use mutsuki_agent_service_host_integration::{
    AgentConnectionError, AgentConnectionRegistry, AgentConnectorCatalog, AgentConnectorFactory,
    configured_agent_plugin_catalog,
};
use mutsuki_bot_protocol::{
    AgentSessionScope, BOT_EVENT_INGEST_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID, BotAccountRef,
    BotEvent, BotEventKind, BotMessage, BotPlatform, BotSpeechReplyPolicy, BotTarget, BotUser,
    ConversationPolicyMatch, ConversationPolicyPatch, ConversationPolicyRule,
    ConversationPolicyRuleUpsert, DirectMessagePolicy, MessageSegment,
};
use mutsuki_bot_service_host_integration::configured_bot_plugin_catalog_with_agent;
use mutsuki_bot_state_db::{BOT_CONVERSATION_POLICY_SERVICE_ID, BotStateDbRepository};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
    ResourceSealState, ResourceSemantic, RunnerResult, Task, TaskHandle, TaskOutcome, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use mutsuki_service_config::{ConfiguredPluginSelection, HostSecretStore, ServiceConfig};
use mutsuki_service_runtime::ServiceRuntime;
use serde_json::{Value, json};

#[derive(Clone)]
struct FakeConnector {
    state: Arc<Mutex<FakeAgentState>>,
}

impl AgentConnectorFactory for FakeConnector {
    fn connector_id(&self) -> &str {
        "test.agent.connector"
    }

    fn connect(
        &self,
        _config: &Value,
        _secrets: &HostSecretStore,
    ) -> Result<Box<dyn AgentClientBackend + Send>, AgentConnectionError> {
        Ok(Box::new(InProcessAgentClient::new(FakeAgentService {
            state: self.state.clone(),
        })))
    }
}

#[derive(Default)]
struct FakeAgentState {
    sessions: BTreeSet<String>,
    versions: BTreeMap<String, u64>,
    submits: usize,
}

struct FakeAgentService {
    state: Arc<Mutex<FakeAgentState>>,
}

impl InProcessAgentService for FakeAgentService {
    fn dispatch(
        &mut self,
        request: AgentWireRequestEnvelope,
    ) -> Result<AgentWireResponseEnvelope, AgentWireError> {
        let request_id = request.request_id;
        let response = match request.request {
            AgentWireRequest::StartSession { request } => {
                let id = request
                    .session_id
                    .expect("Bot supplies a stable session id");
                let mut state = self.state.lock().unwrap();
                state.sessions.insert(id.clone());
                state.versions.insert(id.clone(), 0);
                AgentWireResponse::Session(agent_session(&id))
            }
            AgentWireRequest::GetSession { session_id } => {
                if !self.state.lock().unwrap().sessions.contains(&session_id) {
                    return Ok(wire_response_error(request_id, "agent.session.not_found"));
                }
                AgentWireResponse::Session(agent_session(&session_id))
            }
            AgentWireRequest::SubmitTurn {
                session_id,
                expected_version,
                ..
            } => {
                let mut state = self.state.lock().unwrap();
                if state.versions.get(&session_id).copied() != Some(expected_version.0) {
                    return Ok(wire_response_error(
                        request_id,
                        "agent.session.version_conflict",
                    ));
                }
                let version = expected_version.0 + 1;
                state.versions.insert(session_id.clone(), version);
                state.submits += 1;
                AgentWireResponse::Accepted {
                    session_id,
                    version: SessionVersion(version),
                }
            }
            AgentWireRequest::SubscribeSessionEvents {
                session_id,
                after_sequence,
                ..
            }
            | AgentWireRequest::ResumeSession {
                session_id,
                after_sequence,
            } => {
                let sequence = after_sequence + 1;
                AgentWireResponse::Events(AgentEventPage {
                    events: vec![AgentEventEnvelope {
                        session_id,
                        sequence,
                        meta: Default::default(),
                        event: AgentEvent::FinalResponse {
                            turn_id: "turn".into(),
                            summary: "configured Agent reply".into(),
                            result: None,
                        },
                    }],
                    next_sequence: sequence,
                    lost: 0,
                    truncated: false,
                })
            }
            other => panic!("unexpected Agent request: {other:?}"),
        };
        Ok(AgentWireResponseEnvelope {
            request_id,
            response: Ok(response),
        })
    }
}

struct MessageSendRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    sends: Arc<AtomicUsize>,
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
            assert_eq!(message.plain_text(), "configured Agent reply");
            self.sends.fetch_add(1, Ordering::SeqCst);
            Ok(RunnerResult::completed(task.task_id.clone()))
        })
    }
}

#[tokio::test]
async fn configured_unconsumed_messages_fall_back_but_matched_commands_stop() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("logs")).unwrap();
    let mut service = ServiceConfig::default();
    service.ipc.enabled = false;
    service.observe.console = false;
    service.service.home_dir = root.path().into();
    service.service.data_dir = root.path().join("data");
    service.service.log_dir = root.path().join("logs");
    service.service.run_dir = root.path().join("run");
    service.plugins.dynamic_dirs.clear();
    service.plugins.disabled_dir = root.path().join("disabled");
    // The Bot selection intentionally precedes the connection selection: list order is not a
    // connection routing mechanism.
    service.plugins.configured = vec![
        configured(
            "mutsuki.plugin.bot.agent",
            json!({
                "enabled": true,
                "connection_id": "primary",
                "default_profile_id": "profile",
                "streaming": "final_only",
                "max_concurrency": 1,
                "timeout_ms": 10_000,
                "max_message_bytes": 1_800
            }),
        ),
        configured(
            "mutsuki.agent.connections",
            json!({
                "revision": 0,
                "connections": [{
                    "connection_id": "primary",
                    "connector_id": "test.agent.connector",
                    "enabled": true,
                    "config": {}
                }]
            }),
        ),
        configured(
            "mutsuki.bot.command",
            json!({
                "prefixes": ["/"],
                "commands": [{"path": ["ping"], "aliases": [], "arguments": [], "summary": "test"}]
            }),
        ),
        configured("test.qq.send", Value::Null),
    ];

    let agent_state = Arc::new(Mutex::new(FakeAgentState::default()));
    let registry = AgentConnectionRegistry::new();
    let mut connectors = AgentConnectorCatalog::new();
    connectors
        .register(FakeConnector {
            state: agent_state.clone(),
        })
        .unwrap();
    let mut catalog = configured_agent_plugin_catalog(registry.clone(), connectors).unwrap();
    catalog
        .merge(configured_bot_plugin_catalog_with_agent(registry).unwrap())
        .unwrap();

    let sends = Arc::new(AtomicUsize::new(0));
    let descriptor = RunnerDescriptorBuilder::new("test.qq.send.runner", "test.qq.send")
        .accepted_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
        .execution_class(ExecutionClass::Io)
        .build();
    let manifest = PluginBuilder::new("test.qq.send")
        .runner_descriptor(descriptor.clone())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_MESSAGE_SEND_PROTOCOL_ID).build(),
            "test.qq.send.runner",
            "qq-send",
        )
        .build()
        .manifest;
    let runner_descriptor = descriptor.clone();
    let runner_sends = sends.clone();
    let runtime = mutsuki_service_runtime::ServiceRuntimeBuilder::new(service)
        .with_configured_plugin_catalog(catalog)
        .register_builtin_plugin(manifest)
        .register_builtin_runner(move || {
            Box::new(MessageSendRunner {
                descriptor: runner_descriptor.clone(),
                sends: runner_sends.clone(),
            })
        })
        .start()
        .await
        .unwrap();

    let policies = runtime
        .host_service::<BotStateDbRepository>(BOT_CONVERSATION_POLICY_SERVICE_ID)
        .unwrap();
    policies
        .upsert_policy_rule_fenced(ConversationPolicyRuleUpsert {
            actor_id: "test".into(),
            expected_revision: 0,
            rule: ConversationPolicyRule {
                rule_id: "product-default".into(),
                revision: 0,
                matcher: ConversationPolicyMatch::default(),
                patch: ConversationPolicyPatch {
                    enabled: Some(true),
                    agent_enabled: Some(true),
                    direct_message_policy: Some(DirectMessagePolicy::Allow),
                    must_mention: Some(false),
                    wake_words: Some(Vec::new()),
                    allowlist: Some(Vec::new()),
                    denylist: Some(Vec::new()),
                    session_scope: Some(AgentSessionScope::SharedConversation),
                    agent_runtime_profile_id: Some(Some("profile".into())),
                    stt_enabled: Some(false),
                    tts_enabled: Some(false),
                    speech_reply_policy: Some(BotSpeechReplyPolicy::TextOnly),
                    active_delivery_enabled: Some(false),
                    ..ConversationPolicyPatch::default()
                },
            },
        })
        .await
        .unwrap();

    submit_event(&runtime, event("plain", "hello")).await;
    assert_eq!(
        agent_state.lock().unwrap().submits,
        1,
        "tasks: {:#?}",
        runtime.task_snapshots()
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    submit_event(&runtime, event("matched", "/ping")).await;
    assert_eq!(agent_state.lock().unwrap().submits, 1);
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    submit_event_as(
        &runtime,
        "ingest:unknown-replay",
        event("unknown", "/unknown"),
    )
    .await;
    assert_eq!(agent_state.lock().unwrap().submits, 2);
    assert_eq!(sends.load(Ordering::SeqCst), 2);

    // Replaying the same platform event is idempotent across the real state repository.
    submit_event(&runtime, event("unknown", "/unknown")).await;
    assert_eq!(agent_state.lock().unwrap().submits, 2);
    runtime.shutdown().await;
}

fn configured(id: &str, config: Value) -> ConfiguredPluginSelection {
    ConfiguredPluginSelection {
        id: id.into(),
        enabled: true,
        config,
    }
}

async fn submit_event(runtime: &ServiceRuntime, event: BotEvent) {
    let task_id = format!("ingest:{}", event.event_id);
    submit_event_as(runtime, &task_id, event).await;
}

async fn submit_event_as(runtime: &ServiceRuntime, task_id: &str, event: BotEvent) {
    let handle = runtime
        .submit_task(Task::new(
            task_id,
            BOT_EVENT_INGEST_PROTOCOL_ID,
            serde_json::to_value(event).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(runtime, &handle).await,
        TaskOutcome::Completed { .. }
    ));
}

async fn wait_outcome(runtime: &ServiceRuntime, handle: &TaskHandle) -> TaskOutcome {
    for _ in 0..300 {
        if let Some(outcome) = runtime.task_outcome(handle).unwrap() {
            return outcome;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task {} did not complete", handle.task_id)
}

fn event(event_id: &str, text: &str) -> BotEvent {
    BotEvent {
        event_id: event_id.into(),
        kind: BotEventKind::MessageCreated,
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            platform: BotPlatform::QqBot,
            account_id: "main".into(),
        },
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
            message_id: Some(event_id.into()),
            target: BotTarget::User {
                user_id: "actor".into(),
            },
            sender: None,
            segments: vec![MessageSegment::text(text)],
            reply_to: None,
            time_ms: Some(1),
            ext: BTreeMap::new(),
        }),
        raw: None,
        ext: BTreeMap::new(),
    }
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

fn wire_response_error(request_id: u64, code: &str) -> AgentWireResponseEnvelope {
    AgentWireResponseEnvelope {
        request_id,
        response: Err(AgentWireError {
            code: code.into(),
            message: code.into(),
            retryable: false,
        }),
    }
}
