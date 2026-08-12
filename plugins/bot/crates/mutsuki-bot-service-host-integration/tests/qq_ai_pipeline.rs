use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_agent_contracts::{
    AgentError, AgentEvent, AgentEventEnvelope, AgentEventMeta, AgentEventPage, AgentMessage,
    AgentSession, AgentSessionCreateRequest, AgentWireError, MediaService, ResourceCellRef,
    SessionVersion,
};
use mutsuki_bot_delivery::{
    DeliveryError, DeliveryPolicyResolver, QqDeliveryFailure, QqDeliveryGateway, QqDeliverySuccess,
    bot_reply_delivery_manifest,
};
use mutsuki_bot_flow::{
    BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowConfigProvider, BotFlowRegistry, BotNodeCatalog,
};
use mutsuki_bot_interaction::{InteractionConditionMatcher, InteractionError};
use mutsuki_bot_protocol::{
    AgentSessionScope, BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE,
    BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID, BotAccountRef, BotDeliveryContent,
    BotEvent, BotEventKind, BotFlowContext, BotFlowDocument, BotFlowEdge, BotFlowEdgeKind,
    BotFlowEventEnvelope, BotFlowNode, BotFlowNodePosition, BotFlowPayload, BotFlowSourceSelector,
    BotFlowTypeRef, BotMessage, BotNodeCatalogFragment, BotNodeDescriptor, BotNodePortDescriptor,
    BotNodePortDirection, BotNodeRole, BotPlatform, BotSpeechReplyPolicy, BotTarget, BotUser,
    ConversationPolicy, QqConversationRef,
};
use mutsuki_bot_service_host_integration::QqAiBotPluginBundle;
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_config_service::{ConfigContext, ConfigProviderRegistry, ConfigService, ConfigValue};
use mutsuki_plugin_bot_agent::{
    AgentBridgeClient, BOT_AGENT_BRIDGE_PLUGIN_ID, BOT_AGENT_NODE_SUBMIT, bot_agent_bridge_manifest,
};
use mutsuki_plugin_bot_event_router::{
    BOT_FLOW_REGISTRY_SERVICE_ID, BOT_FLOW_ROUTER_PLUGIN_ID, BotFlowMatchRunner,
    flow_ingress_runner, flow_node_runner, flow_router_manifest,
};
use mutsuki_plugin_config_sqlite::SqliteConfigRepository;
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
    ResourceSealState, ResourceSemantic, RunnerResult, Task, TaskHandle, TaskOutcome, TaskStatus,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
use mutsuki_service_runtime::{LoadPlanLifecycleHook, ServiceRuntime, ServiceRuntimeBuilder};
use serde_json::json;

const TEST_SOURCE_PLUGIN_ID: &str = "test.qq.flow.source";
const TEST_SOURCE_NODE_TYPE: &str = "test.qq.flow.source";
const TEST_SEND_PLUGIN_ID: &str = "test.qq.flow.send";
const TEST_SEND_RUNNER_ID: &str = "test.qq.flow.send.runner";

struct FlowHook(Arc<BotFlowRegistry>);

impl LoadPlanLifecycleHook for FlowHook {
    fn validate(&self, plan: &mutsuki_runtime_contracts::RuntimeLoadPlan) -> Result<(), String> {
        self.0
            .validate_load_plan(plan)
            .map_err(|error| error.to_string())
    }

    fn activate(&self, plan: &mutsuki_runtime_contracts::RuntimeLoadPlan) {
        self.0
            .activate_load_plan(plan)
            .expect("validated flow plan activates");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn published_flow_routes_agent_reply_once_and_recovers_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let submits = Arc::new(AtomicUsize::new(0));
    let sends = Arc::new(AtomicUsize::new(0));
    let agent_state = Arc::new(Mutex::new(TestAgentState::default()));

    let runtime = start_runtime(
        root.path(),
        submits.clone(),
        sends.clone(),
        agent_state.clone(),
        true,
    )
    .await;
    submit_event(&runtime, "event-1", "wake hello").await;
    wait_for(&submits, 1).await;
    wait_for(&sends, 1).await;

    // Replaying the same external event cannot cross the durable Agent/delivery fencing twice.
    submit_event(&runtime, "event-1", "wake hello").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(submits.load(Ordering::SeqCst), 1);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    wait_for_flow_tasks(&runtime).await;
    runtime.shutdown().await;

    let runtime = start_runtime(
        root.path(),
        submits.clone(),
        sends.clone(),
        agent_state,
        false,
    )
    .await;
    let registry = runtime
        .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
        .unwrap();
    assert_eq!(registry.active().revision, 1);
    submit_event(&runtime, "event-2", "wake after restart").await;
    wait_for(&submits, 2).await;
    wait_for(&sends, 2).await;
    wait_for_flow_tasks(&runtime).await;
    runtime.shutdown().await;
}

async fn start_runtime(
    root: &std::path::Path,
    submits: Arc<AtomicUsize>,
    sends: Arc<AtomicUsize>,
    agent_state: Arc<Mutex<TestAgentState>>,
    publish: bool,
) -> ServiceRuntime {
    let data_dir = root.join("data");
    let state_dir = data_dir.join("bot");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::create_dir_all(root.join("logs")).unwrap();
    std::fs::create_dir_all(root.join("run")).unwrap();
    let repository = Arc::new(BotStateDbRepository::open(state_dir.join("state.sqlite3")).unwrap());
    let source_manifest = source_manifest();
    let flow_manifest = flow_router_manifest();
    let agent_manifest = bot_agent_bridge_manifest();
    let reply_manifest = bot_reply_delivery_manifest();
    let catalog = BotNodeCatalog::from_manifests(&[
        source_manifest.clone(),
        flow_manifest.clone(),
        agent_manifest,
        reply_manifest,
    ])
    .unwrap();
    let registry = Arc::new(BotFlowRegistry::new(catalog));
    let providers = Arc::new(ConfigProviderRegistry::default());
    providers
        .register(Arc::new(BotFlowConfigProvider::new(registry.clone())))
        .unwrap();
    let flow_config = Arc::new(
        ConfigService::new(
            providers,
            Arc::new(
                SqliteConfigRepository::open(root.join("flow-config.sqlite3"), "qq-ai-pipeline")
                    .unwrap(),
            ),
        )
        .unwrap(),
    );
    if publish {
        flow_config
            .create_if_absent(
                BOT_FLOW_CONFIG_PROVIDER_ID,
                ConfigValue::from_json(&json!({ "flows": [agent_flow()] })),
                ConfigContext::global(),
            )
            .await
            .unwrap();
    } else {
        flow_config
            .restore(BOT_FLOW_CONFIG_PROVIDER_ID, ConfigContext::global())
            .await
            .unwrap();
    }

    let send_descriptor = RunnerDescriptorBuilder::new(TEST_SEND_RUNNER_ID, TEST_SEND_PLUGIN_ID)
        .accepted_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
        .execution_class(ExecutionClass::Io)
        .build();
    let send_manifest = PluginBuilder::new(TEST_SEND_PLUGIN_ID)
        .runner_descriptor(send_descriptor.clone())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_MESSAGE_SEND_PROTOCOL_ID).build(),
            TEST_SEND_RUNNER_ID,
            "test-send",
        )
        .build()
        .manifest;
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = root.into();
    config.service.data_dir = data_dir;
    config.service.log_dir = root.join("logs");
    config.service.run_dir = root.join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = root.join("disabled");
    config.plugins.configured = [
        TEST_SOURCE_PLUGIN_ID,
        BOT_FLOW_ROUTER_PLUGIN_ID,
        BOT_AGENT_BRIDGE_PLUGIN_ID,
        mutsuki_plugin_bot_command::BOT_COMMAND_PLUGIN_ID,
        mutsuki_plugin_bot_media::BOT_MEDIA_BRIDGE_PLUGIN_ID,
        mutsuki_bot_delivery::BOT_DELIVERY_PLUGIN_ID,
        mutsuki_bot_delivery::BOT_REPLY_DELIVERY_PLUGIN_ID,
        mutsuki_bot_interaction::BOT_INTERACTION_PLUGIN_ID,
        TEST_SEND_PLUGIN_ID,
    ]
    .into_iter()
    .map(|id| ConfiguredPluginSelection {
        id: id.into(),
        enabled: true,
        config: serde_json::Value::Null,
    })
    .collect();

    let ingress_registry = registry.clone();
    let node_registry = registry.clone();
    let service_registry = registry.clone();
    let flow_loaded_manifest = flow_manifest.clone();
    let builder = ServiceRuntimeBuilder::new(config)
        .register_builtin_plugin(source_manifest)
        .register_builtin_plugin(send_manifest)
        .register_builtin_loaded_plugin_factory(flow_manifest, move || {
            Ok::<mutsuki_runtime_sdk::LoadedPlugin, String>(mutsuki_runtime_sdk::LoadedPlugin {
                manifest: flow_loaded_manifest.clone(),
                runners: Vec::new(),
                async_handlers: Vec::new(),
                host_services: vec![mutsuki_runtime_sdk::RuntimeBootstrapperService {
                    service_id: BOT_FLOW_REGISTRY_SERVICE_ID.into(),
                    capability: Some("bot.flow".into()),
                    service: service_registry.clone(),
                }],
                resource_providers: Vec::new(),
                async_resource_providers: Vec::new(),
            })
        })
        .register_builtin_runner(move || {
            Box::new(MessageSendRunner {
                descriptor: send_descriptor.clone(),
                sends: sends.clone(),
            })
        })
        .register_builtin_runner(move || flow_ingress_runner(ingress_registry.clone()))
        .register_builtin_runner(|| Box::new(BotFlowMatchRunner::default()))
        .register_runtime_client_runner(move |client| {
            flow_node_runner(client, node_registry.clone())
        })
        .register_load_plan_hook(
            BOT_FLOW_REGISTRY_SERVICE_ID,
            Arc::new(FlowHook(registry.clone())),
        );

    let policy = ConversationPolicy {
        revision: 1,
        session_scope: AgentSessionScope::SharedConversation,
        business_profile_binding_id: None,
        agent_runtime_profile_id: Some("profile".into()),
        stt_enabled: false,
        tts_enabled: false,
        speech_reply_policy: BotSpeechReplyPolicy::TextOnly,
        stt_selector_id: None,
        tts_selector_id: None,
        active_delivery_enabled: true,
    };
    let bundle = QqAiBotPluginBundle::new(
        repository.clone(),
        repository.clone(),
        repository,
        policy,
        Box::new(TestAgent::new(submits, agent_state)),
        Arc::new(TestMedia),
        Arc::new(TestDeliveryGateway),
        Arc::new(Allow),
        Arc::new(Allow),
    );
    bundle.install(builder).start().await.unwrap()
}

async fn submit_event(runtime: &ServiceRuntime, event_id: &str, text: &str) {
    let event = bot_event(event_id, text);
    let envelope = BotFlowEventEnvelope {
        event_id: event.event_id.clone(),
        protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
        payload: BotFlowPayload {
            event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
            value: serde_json::to_value(&event).unwrap(),
        },
        context: BotFlowContext {
            bot: Some(event.bot.clone()),
            target: Some(event.target.clone()),
            actor: event.actor.clone(),
            ext: event.ext.clone(),
        },
        trace_id: Some(format!("trace-{event_id}")),
        correlation_id: Some(format!("correlation-{event_id}")),
    };
    let handle = runtime
        .submit_task(Task::new(
            format!("ingress-{event_id}-{}", fastrand::u64(..)),
            BOT_FLOW_INGRESS_PROTOCOL_ID,
            serde_json::to_value(envelope).unwrap(),
        ))
        .unwrap();
    assert!(matches!(
        wait_outcome(runtime, &handle).await,
        TaskOutcome::Completed { .. }
    ));
}

fn agent_flow() -> BotFlowDocument {
    BotFlowDocument {
        flow_id: "qq-agent".into(),
        name: "QQ Agent".into(),
        enabled: true,
        nodes: vec![
            flow_node(
                "source",
                TEST_SOURCE_NODE_TYPE,
                json!({}),
                Some(BotFlowSourceSelector {
                    protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                    event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
                }),
            ),
            flow_node(
                "match",
                "mutsuki.bot.match.event",
                json!({
                    "event_kinds": ["message_created"],
                    "conversation_kinds": ["private"],
                    "wake_words": ["wake"]
                }),
                None,
            ),
            flow_node("agent", BOT_AGENT_NODE_SUBMIT, json!({}), None),
            flow_node("delivery", "mutsuki.bot.delivery.reply", json!({}), None),
        ],
        edges: vec![
            flow_edge("source-match", "source", "event", "match", "event"),
            flow_edge("match-agent", "match", "matched", "agent", "input"),
            flow_edge("agent-delivery", "agent", "reply", "delivery", "reply"),
        ],
    }
}

fn source_manifest() -> mutsuki_runtime_contracts::PluginManifest {
    PluginBuilder::new(TEST_SOURCE_PLUGIN_ID)
        .extension(
            BotNodeCatalogFragment {
                nodes: vec![BotNodeDescriptor {
                    node_type_id: TEST_SOURCE_NODE_TYPE.into(),
                    version: 1,
                    title: "QQ 测试事件".into(),
                    category: "QQ".into(),
                    role: BotNodeRole::Source,
                    binding: None,
                    ports: vec![BotNodePortDescriptor {
                        port_id: "event".into(),
                        title: "事件".into(),
                        direction: BotNodePortDirection::Output,
                        event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                        required: false,
                    }],
                    config_schema: json!({"type": "object", "additionalProperties": false}),
                }],
            }
            .into_plugin_extension()
            .unwrap(),
        )
        .build()
        .manifest
}

fn flow_node(
    node_id: &str,
    node_type_id: &str,
    config: serde_json::Value,
    source: Option<BotFlowSourceSelector>,
) -> BotFlowNode {
    BotFlowNode {
        node_id: node_id.into(),
        node_type_id: node_type_id.into(),
        node_type_version: 1,
        config,
        source,
        position: BotFlowNodePosition::default(),
    }
}

fn flow_edge(
    edge_id: &str,
    from_node_id: &str,
    from_port_id: &str,
    to_node_id: &str,
    to_port_id: &str,
) -> BotFlowEdge {
    BotFlowEdge {
        edge_id: edge_id.into(),
        from_node_id: from_node_id.into(),
        from_port_id: from_port_id.into(),
        to_node_id: to_node_id.into(),
        to_port_id: to_port_id.into(),
        kind: BotFlowEdgeKind::Event,
    }
}

fn bot_event(event_id: &str, text: &str) -> BotEvent {
    let target = BotTarget::User {
        user_id: "actor".into(),
    };
    BotEvent {
        event_id: event_id.into(),
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
        message: Some(BotMessage::text(target, text)),
        raw: None,
        ext: BTreeMap::new(),
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
            assert_eq!(message.plain_text(), "Agent reply");
            self.sends.fetch_add(1, Ordering::SeqCst);
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(json!({"id": "qq-message"}));
            Ok(result)
        })
    }
}

struct TestAgent {
    submits: Arc<AtomicUsize>,
    state: Arc<Mutex<TestAgentState>>,
}

#[derive(Default)]
struct TestAgentState {
    sessions: BTreeSet<String>,
    versions: BTreeMap<String, u64>,
}

impl TestAgent {
    fn new(submits: Arc<AtomicUsize>, state: Arc<Mutex<TestAgentState>>) -> Self {
        Self { submits, state }
    }
}

impl AgentBridgeClient for TestAgent {
    fn get_session(&mut self, session_id: &str) -> Result<AgentSession, AgentWireError> {
        self.state
            .lock()
            .unwrap()
            .sessions
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
        let mut state = self.state.lock().unwrap();
        state.sessions.insert(session_id.clone());
        state.versions.insert(session_id.clone(), 0);
        Ok(agent_session(&session_id))
    }

    fn submit_turn(
        &mut self,
        session_id: &str,
        expected_version: SessionVersion,
        _turn_id: &str,
        _messages: Vec<AgentMessage>,
        _idempotency_key: &str,
    ) -> Result<SessionVersion, AgentWireError> {
        let mut state = self.state.lock().unwrap();
        if state.versions.get(session_id).copied() != Some(expected_version.0) {
            return Err(wire_error("agent.session.version_conflict"));
        }
        let next = expected_version.0 + 1;
        state.versions.insert(session_id.into(), next);
        self.submits.fetch_add(1, Ordering::SeqCst);
        Ok(SessionVersion(next))
    }

    fn cancel_turn(
        &mut self,
        session_id: &str,
        _turn_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let next = expected_version.0 + 1;
        self.state
            .lock()
            .unwrap()
            .versions
            .insert(session_id.into(), next);
        Ok(SessionVersion(next))
    }

    fn fork_session(
        &mut self,
        source_session_id: &str,
        target_session_id: &str,
        expected_version: SessionVersion,
    ) -> Result<SessionVersion, AgentWireError> {
        let mut state = self.state.lock().unwrap();
        if state.versions.get(source_session_id).copied() != Some(expected_version.0) {
            return Err(wire_error("agent.session.version_conflict"));
        }
        state.sessions.insert(target_session_id.into());
        state.versions.insert(target_session_id.into(), 1);
        Ok(SessionVersion(1))
    }

    fn events(
        &mut self,
        session_id: &str,
        after_sequence: u64,
    ) -> Result<AgentEventPage, AgentWireError> {
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

struct TestMedia;

impl MediaService for TestMedia {
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

struct TestDeliveryGateway;

impl QqDeliveryGateway for TestDeliveryGateway {
    fn send(
        &self,
        _conversation: &QqConversationRef,
        _content: &BotDeliveryContent,
    ) -> Result<QqDeliverySuccess, QqDeliveryFailure> {
        Ok(QqDeliverySuccess {
            platform_message_ids: vec!["qq-message".into()],
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

async fn wait_outcome(runtime: &ServiceRuntime, handle: &TaskHandle) -> TaskOutcome {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(outcome) = runtime.task_outcome(handle).unwrap() {
                return outcome;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("task completes")
}

async fn wait_for(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if counter.load(Ordering::SeqCst) >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("counter reaches expected value");
}

async fn wait_for_flow_tasks(runtime: &ServiceRuntime) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshots = runtime.task_snapshots().unwrap();
            if snapshots.iter().all(|snapshot| {
                matches!(
                    snapshot.status,
                    TaskStatus::Completed
                        | TaskStatus::Failed
                        | TaskStatus::Cancelled
                        | TaskStatus::Expired
                        | TaskStatus::DeadLetter
                )
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Flow tasks did not finish: {:#?}", runtime.task_snapshots()));
}
