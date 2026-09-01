// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::duration_suboptimal_units, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_agent_contracts::{
    AGENT_SPEECH_SYNTHESIZE_PROTOCOL, AGENT_TRANSCRIBE_PROTOCOL, AgentError, AgentEvent,
    AgentEventEnvelope, AgentEventMeta, AgentEventPage, AgentMessage, AgentSession,
    AgentSessionCreateRequest, AgentWireError, MediaService, ResourceCellRef, SessionVersion,
};
use mutsuki_bot_delivery::{
    DeliveryError, DeliveryFailure, DeliveryGateway, DeliveryPolicyResolver, DeliverySuccess,
    ReplyDeliveryRepository,
};
use mutsuki_bot_flow::{
    BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowConfigProvider, BotFlowRegistry, BotNodeCatalog,
};
use mutsuki_bot_interaction::{InteractionConditionMatcher, InteractionError};
use mutsuki_bot_protocol::{
    AgentSessionScope, BOT_AGENT_SUBMIT_PROTOCOL_ID, BOT_EVENT_INGEST_PROTOCOL_ID,
    BOT_FLOW_BOT_EVENT_TYPE, BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID, BOT_REPLY_DELIVERY_PROTOCOL_ID, BotAccountRef,
    BotDeliveryContent, BotEvent, BotEventKind, BotFlowContext, BotFlowDocument, BotFlowEdge,
    BotFlowEdgeKind, BotFlowEventEnvelope, BotFlowNode, BotFlowNodePosition, BotFlowPayload,
    BotFlowSourceSelector, BotFlowTypeRef, BotMessage, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodeInvocation, BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult,
    BotNodeRole, BotNodeWiring, BotPlatform, BotReplyDeliveryCommand, BotReplyDeliveryPart,
    BotReplyDeliveryRequest, BotSpeechReplyPolicy, BotTarget, BotUser, ConversationPolicy,
    DeliveryPolicy, DeliveryStatus, MessageSegment, QqConversationRef,
};
use mutsuki_bot_service_host_integration::{
    QqAiBotPluginBundle, qq_ai_orchestrated_flow_with_source,
};
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_config_service::{ConfigContext, ConfigProviderRegistry, ConfigService, ConfigValue};
use mutsuki_plugin_bot_agent::{
    AgentBridgeClient, BOT_AGENT_BRIDGE_PLUGIN_ID, BOT_AGENT_NODE_SUBMIT, bot_agent_bridge_manifest,
};
use mutsuki_plugin_bot_conversation_context::{
    MemoryConversationContextStore, bot_conversation_context_manifest,
};
use mutsuki_plugin_bot_delivery::{
    BOT_DELIVERY_PLUGIN_ID, BOT_REPLY_DELIVERY_PLUGIN_ID, bot_reply_delivery_manifest,
};
use mutsuki_plugin_bot_event_router::{
    BOT_FLOW_REGISTRY_SERVICE_ID, BOT_FLOW_ROUTER_PLUGIN_ID, BotFlowMatchRunner,
    flow_ingress_runner, flow_node_runner, flow_router_manifest,
};
use mutsuki_plugin_bot_interaction::{BOT_INTERACTION_PLUGIN_ID, bot_interaction_manifest};
use mutsuki_plugin_bot_persona::bot_persona_manifest;
use mutsuki_plugin_bot_reply::bot_reply_manifest;
use mutsuki_plugin_config_sqlite::SqliteConfigRepository;
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, InvocationMode, ResourceAccess, ResourceId, ResourceLifetime,
    ResourceRef, ResourceSealState, ResourceSemantic, RunnerConcurrency, RunnerResult,
    RunnerStatus, ScalarValue, Task, TaskHandle, TaskOutcome, TaskStatus, WorkBatch,
};
use mutsuki_runtime_core::{
    AsyncBatchHandler, AsyncCompletionFuture, Runner, RunnerContext, RuntimeResult,
};
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
const TEST_MEDIA_PLUGIN_ID: &str = "test.agent.media";
const TEST_MEDIA_RUNNER_ID: &str = "test.agent.media.runner";
const TEST_FORWARD_FOLD_PLUGIN_ID: &str = "test.qq.forward-fold.catalog";
static RUNTIME_E2E_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

struct RuntimeFixture {
    root: tempfile::TempDir,
    submits: Arc<AtomicUsize>,
    sends: Arc<AtomicUsize>,
    agent_state: Arc<Mutex<TestAgentState>>,
    send_plan: Arc<TestSendPlan>,
    conversation_context: Arc<MemoryConversationContextStore>,
    last_send: Arc<Mutex<Option<BotMessage>>>,
}

impl RuntimeFixture {
    fn new(outcomes: impl IntoIterator<Item = TestSendOutcome>) -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            submits: Arc::new(AtomicUsize::new(0)),
            sends: Arc::new(AtomicUsize::new(0)),
            agent_state: Arc::new(Mutex::new(TestAgentState::default())),
            send_plan: Arc::new(TestSendPlan::new(outcomes)),
            conversation_context: Arc::new(MemoryConversationContextStore::default()),
            last_send: Arc::new(Mutex::new(None)),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn published_flow_routes_agent_reply_once_and_recovers_after_restart() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([]);
    let runtime = start_runtime(&fixture, true, Duration::from_millis(250), None).await;
    submit_event(&runtime, "event-1", "wake hello").await;
    wait_for(&fixture.submits, 1).await;
    wait_for(&fixture.sends, 1).await;

    // Replaying the same external event cannot cross the durable Agent/delivery fencing twice.
    // Draining the pipeline before counting is what makes this a real assertion: once every task
    // the replay could have spawned is terminal, a second submit or send can no longer appear.
    submit_event(&runtime, "event-1", "wake hello").await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;

    let runtime = start_runtime(&fixture, false, Duration::from_millis(250), None).await;
    wait_for_flow_revision(&runtime, 1).await;
    submit_event(&runtime, "event-2", "wake after restart").await;
    wait_for(&fixture.submits, 2).await;
    wait_for(&fixture.sends, 2).await;
    wait_for_flow_tasks(&runtime).await;
    // Routed business events are counted; a wired graph drops none. The
    // restarted runtime owns a fresh registry.
    let registry = runtime
        .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
        .unwrap();
    assert!(registry.ingress_stats().accepted_total() >= 1);
    assert_eq!(registry.ingress_stats().dropped_total(), 0);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchestrated_mention_path_attaches_context_then_presents_reply() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([]);
    let runtime = start_runtime_with_flow(
        &fixture,
        true,
        Duration::from_millis(250),
        None,
        orchestrated_test_flow(),
    )
    .await;
    wait_for_flow_revision(&runtime, 1).await;
    let active = runtime
        .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
        .unwrap()
        .active();
    assert_eq!(
        active.flow.flow_id, "qq.ai.orchestrated",
        "active flow: {active:?}"
    );
    submit_event_mentioned(&runtime, "event-orch-1", "wake hello").await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(
        fixture.submits.load(Ordering::SeqCst),
        1,
        "tasks: {:#?}",
        runtime.task_snapshots()
    );
    wait_for(&fixture.sends, 1).await;
    wait_for_flow_tasks(&runtime).await;
    let sent = fixture
        .last_send
        .lock()
        .unwrap()
        .clone()
        .expect("delivery send captured BotMessage");
    assert!(
        sent.reply_to.as_deref() == Some("msg-event-orch-1")
            || sent.segments.iter().any(|segment| matches!(
                segment,
                MessageSegment::Quote { .. } | MessageSegment::Reply { .. }
            )),
        "quoted reply missing: {sent:?}"
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchestrated_unmentioned_message_records_icl_without_agent_submit() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([]);
    let runtime = start_runtime_with_flow(
        &fixture,
        true,
        Duration::from_millis(250),
        None,
        orchestrated_test_flow(),
    )
    .await;
    wait_for_flow_revision(&runtime, 1).await;
    let active = runtime
        .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
        .unwrap()
        .active();
    assert_eq!(
        active.flow.flow_id, "qq.ai.orchestrated",
        "active flow: {active:?}"
    );
    submit_event(&runtime, "event-icl-only", "group chatter").await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
    let recorded = mutsuki_plugin_bot_conversation_context::ConversationContextStore::load_icl(
        fixture.conversation_context.as_ref(),
        "user:actor",
        20,
    )
    .unwrap();
    assert!(
        recorded.iter().any(|entry| entry.text == "group chatter"),
        "expected ICL record, got {recorded:?}"
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchestrated_empty_mention_does_not_submit_agent_without_waiter() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([]);
    let runtime = start_runtime_with_flow(
        &fixture,
        true,
        Duration::from_millis(250),
        None,
        orchestrated_test_flow(),
    )
    .await;
    wait_for_flow_revision(&runtime, 1).await;
    submit_event_mentioned(&runtime, "event-empty-at", "").await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(
        fixture.submits.load(Ordering::SeqCst),
        0,
        "bare @ must not fall through to Agent"
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchestrated_empty_mention_next_message_submits_agent() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([]);
    let runtime = start_runtime_with_flow(
        &fixture,
        true,
        Duration::from_millis(250),
        None,
        orchestrated_test_flow(),
    )
    .await;
    wait_for_flow_revision(&runtime, 1).await;
    submit_event_mentioned(&runtime, "event-empty-at", "").await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(
        fixture.submits.load(Ordering::SeqCst),
        0,
        "bare @ creates a waiter and must not submit"
    );
    submit_event(&runtime, "event-follow-up", "下一则普通话").await;
    wait_for(&fixture.submits, 1).await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_node_occupancy_is_not_resumed_before_submit() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([]);
    let runtime = start_runtime(&fixture, true, Duration::from_millis(10), None).await;
    submit_agent_node(&runtime, "event-reserved").await;
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.sends.load(Ordering::SeqCst),
        0,
        "Agent node Reserve occupies reply_id without sending"
    );
    runtime.shutdown().await;

    let runtime = start_runtime(&fixture, false, Duration::from_millis(10), None).await;
    wait_for_flow_tasks(&runtime).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        fixture.submits.load(Ordering::SeqCst),
        1,
        "direct Agent submit must not recover a second turn"
    );
    assert_eq!(
        fixture.sends.load(Ordering::SeqCst),
        0,
        "occupancy-only draft must not ResumeDue before presentation Submit"
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn orchestrated_presentation_failure_sends_error_notice() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([]);
    let mut flow = orchestrated_test_flow();
    let segment = flow
        .nodes
        .iter_mut()
        .find(|node| node.node_id == "segment")
        .expect("segment node");
    segment.config = json!({"pattern": "("});
    let runtime =
        start_runtime_with_flow(&fixture, true, Duration::from_secs(60), None, flow).await;
    wait_for_flow_revision(&runtime, 1).await;
    submit_event_mentioned(&runtime, "event-present-fail", "wake hello").await;
    wait_for(&fixture.sends, 1).await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 1);
    let sent = fixture
        .last_send
        .lock()
        .unwrap()
        .clone()
        .expect("error-edge notice captured");
    assert_eq!(
        sent.plain_text(),
        mutsuki_bot_service_host_integration::QQ_AI_PRESENTATION_FAILURE_TEXT
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transient_failure_recovers_and_duplicate_event_does_not_repeat_agent_or_send() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture =
        RuntimeFixture::new([TestSendOutcome::TransientFailure, TestSendOutcome::Success]);
    let runtime = start_runtime(&fixture, true, Duration::from_millis(5), None).await;

    submit_event(&runtime, "event-transient", "wake transient").await;
    wait_for(&fixture.sends, 1).await;
    assert_eq!(fixture.send_plan.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);

    submit_event(&runtime, "event-transient", "wake transient").await;
    wait_for_flow_tasks(&runtime).await;
    assert_eq!(fixture.send_plan.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permanent_cancel_and_timeout_are_queryable_and_only_manual_retry_resends() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    for (outcome, timeout_ms, status, event_id, retry) in [
        (
            TestSendOutcome::PermanentFailure,
            None,
            DeliveryStatus::PermanentlyFailed,
            "event-permanent",
            true,
        ),
        (
            TestSendOutcome::Cancelled,
            None,
            DeliveryStatus::ReconcileRequired,
            "event-cancelled",
            false,
        ),
        (
            TestSendOutcome::Timeout,
            Some(10),
            DeliveryStatus::ReconcileRequired,
            "event-timeout",
            false,
        ),
    ] {
        assert_terminal_send_outcome(outcome, timeout_ms, status, event_id, retry).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multipart_restart_recovers_only_unconfirmed_parts_and_keeps_receipts_for_all_shapes() {
    let _guard = RUNTIME_E2E_LOCK.lock().await;
    let fixture = RuntimeFixture::new([
        TestSendOutcome::Success,
        TestSendOutcome::TransientFailure,
        TestSendOutcome::Success,
        TestSendOutcome::Success,
        TestSendOutcome::Success,
        TestSendOutcome::Success,
    ]);
    let request = multipart_reply_request();

    let runtime = start_runtime(&fixture, true, Duration::from_secs(60), None).await;
    submit_reply_command(
        &runtime,
        BotReplyDeliveryCommand::Submit {
            request: Box::new(request.clone()),
            now_unix_ms: 100,
        },
    )
    .await;
    assert_eq!(fixture.send_plan.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;

    let runtime = start_runtime(&fixture, false, Duration::from_millis(5), None).await;
    wait_for(&fixture.sends, 5).await;
    assert_eq!(fixture.send_plan.attempts.load(Ordering::SeqCst), 6);
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 0);
    let receipt = wait_for_reply_succeeded(fixture.root.path(), &request.reply_id).await;
    assert_eq!(receipt.part_receipts.len(), 5);
    assert!(
        receipt
            .part_receipts
            .iter()
            .all(|part| part.status == DeliveryStatus::Succeeded),
        "{receipt:?}"
    );
    assert_eq!(receipt.part_receipts[0].attempt_count, 1);
    assert_eq!(receipt.part_receipts[1].attempt_count, 2);
    assert!(
        receipt.part_receipts[2..]
            .iter()
            .all(|part| part.attempt_count == 1)
    );
    runtime.shutdown().await;
}

async fn start_runtime(
    fixture: &RuntimeFixture,
    publish: bool,
    recovery_interval: Duration,
    send_timeout_ms: Option<u64>,
) -> ServiceRuntime {
    start_runtime_with_flow(
        fixture,
        publish,
        recovery_interval,
        send_timeout_ms,
        agent_flow(),
    )
    .await
}

async fn start_runtime_with_flow(
    fixture: &RuntimeFixture,
    publish: bool,
    recovery_interval: Duration,
    send_timeout_ms: Option<u64>,
    flow: BotFlowDocument,
) -> ServiceRuntime {
    let root = fixture.root.path();
    let submits = fixture.submits.clone();
    let sends = fixture.sends.clone();
    let last_send = fixture.last_send.clone();
    let agent_state = fixture.agent_state.clone();
    let send_plan = fixture.send_plan.clone();
    let data_dir = root.join("data");
    let state_dir = data_dir.join("bot");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::create_dir_all(root.join("logs")).unwrap();
    std::fs::create_dir_all(root.join("run")).unwrap();
    let repository = Arc::new(BotStateDbRepository::open(state_dir.join("state.sqlite3")).unwrap());
    let source_manifest = source_manifest();
    let mut send_descriptor =
        RunnerDescriptorBuilder::new(TEST_SEND_RUNNER_ID, TEST_SEND_PLUGIN_ID)
            .accepted_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
            .execution_class(ExecutionClass::Io)
            .invocation_mode(InvocationMode::AsyncReentrant)
            .concurrency(RunnerConcurrency::Reentrant {
                max_inflight_batches: 8,
                max_inflight_entries: 8,
            })
            .build();
    send_descriptor.batch.preferred_batch_size = 1;
    send_descriptor.batch.max_batch_entries = 1;
    send_descriptor.batch.max_entry_concurrency = 1;
    send_descriptor.batch.max_inflight_batches = 8;
    let send_manifest = PluginBuilder::new(TEST_SEND_PLUGIN_ID)
        .runner_descriptor(send_descriptor.clone())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_MESSAGE_SEND_PROTOCOL_ID)
                .input_schema(json!({"type": "object", "required": ["target", "segments"]}))
                .output_schema(json!({"type": "object", "required": ["message_id"]}))
                .error_schema(json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            TEST_SEND_RUNNER_ID,
            "test-send",
        )
        .extension(
            BotNodeCatalogFragment {
                nodes: vec![qq_send_catalog_node()],
            }
            .into_plugin_extension()
            .unwrap(),
        )
        .build()
        .manifest;
    let mut flow_manifest = flow_router_manifest();
    flow_manifest
        .provides
        .services
        .push(BOT_FLOW_REGISTRY_SERVICE_ID.into());
    flow_manifest.provides.capabilities.push("bot.flow".into());
    let catalog = BotNodeCatalog::from_manifests(&[
        source_manifest.clone(),
        send_manifest.clone(),
        flow_manifest.clone(),
        bot_agent_bridge_manifest(),
        bot_reply_delivery_manifest(),
        bot_conversation_context_manifest(),
        bot_reply_manifest(),
        bot_persona_manifest(),
        mutsuki_plugin_bot_command::bot_command_manifest(1),
        bot_interaction_manifest(),
        qq_forward_fold_catalog_manifest(),
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
                ConfigValue::from_json(&json!({ "flow": flow })),
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

    let media_descriptor = RunnerDescriptorBuilder::new(TEST_MEDIA_RUNNER_ID, TEST_MEDIA_PLUGIN_ID)
        .accepted_protocol(AGENT_TRANSCRIBE_PROTOCOL)
        .accepted_protocol(AGENT_SPEECH_SYNTHESIZE_PROTOCOL)
        .execution_class(ExecutionClass::Io)
        .build();
    let media_manifest = PluginBuilder::new(TEST_MEDIA_PLUGIN_ID)
        .runner_descriptor(media_descriptor.clone())
        .protocol_handler(
            test_protocol_descriptor(AGENT_TRANSCRIBE_PROTOCOL),
            TEST_MEDIA_RUNNER_ID,
            "test-agent-transcribe",
        )
        .protocol_handler(
            test_protocol_descriptor(AGENT_SPEECH_SYNTHESIZE_PROTOCOL),
            TEST_MEDIA_RUNNER_ID,
            "test-agent-speech-synthesize",
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
        mutsuki_plugin_bot_conversation_context::BOT_CONVERSATION_CONTEXT_PLUGIN_ID,
        mutsuki_plugin_bot_media::BOT_MEDIA_BRIDGE_PLUGIN_ID,
        mutsuki_plugin_bot_persona::BOT_PERSONA_PLUGIN_ID,
        mutsuki_plugin_bot_reply::BOT_REPLY_PLUGIN_ID,
        BOT_DELIVERY_PLUGIN_ID,
        BOT_REPLY_DELIVERY_PLUGIN_ID,
        BOT_INTERACTION_PLUGIN_ID,
        TEST_SEND_PLUGIN_ID,
        TEST_MEDIA_PLUGIN_ID,
        TEST_FORWARD_FOLD_PLUGIN_ID,
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
        .register_builtin_plugin(media_manifest)
        .register_builtin_plugin(qq_forward_fold_catalog_manifest())
        .register_builtin_loaded_plugin_factory(flow_manifest, move || {
            Ok::<mutsuki_runtime_sdk::LoadedPlugin, String>(mutsuki_runtime_sdk::LoadedPlugin {
                manifest: flow_loaded_manifest.clone(),
                runners: Vec::new(),
                async_handlers: Vec::new(),
                host_services: vec![mutsuki_runtime_sdk::RuntimeBootstrapperService::new(
                    BOT_FLOW_REGISTRY_SERVICE_ID,
                    service_registry.clone(),
                    "bot.flow",
                )],
                resource_providers: Vec::new(),
                async_resource_providers: Vec::new(),
                host_effects: Vec::new(),
            })
        })
        .register_builtin_async_handler(move || {
            Arc::new(MessageSendHandler {
                descriptor: send_descriptor.clone(),
                sends: sends.clone(),
                last_send: last_send.clone(),
                plan: send_plan.clone(),
            })
        })
        .register_dynamic_runner_limit(TEST_SEND_RUNNER_ID, move || (None, send_timeout_ms))
        .register_builtin_runner(move || {
            Box::new(UnusedMediaRunner {
                descriptor: media_descriptor.clone(),
            })
        })
        .register_builtin_runner(move || flow_ingress_runner(ingress_registry.clone()))
        .register_builtin_runner(|| Box::new(BotFlowMatchRunner::default()))
        .register_builtin_runner(|| Box::new(TestForwardFoldRunner::default()))
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
        repository.clone(),
        policy,
        Box::new(TestAgent::new(submits, agent_state)),
        Arc::new(TestMedia),
        Arc::new(TestDeliveryGateway),
        Arc::new(Allow),
        Arc::new(Allow),
        fixture.conversation_context.clone(),
        repository,
    );
    bundle
        .with_reply_delivery_recovery_interval(recovery_interval)
        .install(builder)
        .start()
        .await
        .unwrap()
}

async fn submit_agent_node(runtime: &ServiceRuntime, event_id: &str) {
    let event = bot_event(event_id, "wake reserved");
    let invocation = BotNodeInvocation {
        flow_id: "crash-window".into(),
        graph_revision: 1,
        execution_id: format!("execution-{event_id}"),
        node_id: "agent".into(),
        input_port_id: "input".into(),
        wiring: BotNodeWiring::default(),
        config: json!({}),
        input: BotFlowEventEnvelope {
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
        },
    };
    let handle = runtime
        .submit_task(Task::new(
            format!("agent-{event_id}-{}", fastrand::u64(..)),
            BOT_AGENT_SUBMIT_PROTOCOL_ID,
            serde_json::to_value(invocation).unwrap(),
        ))
        .unwrap();
    let outcome = wait_outcome(runtime, &handle).await;
    assert!(
        matches!(outcome, TaskOutcome::Completed { .. }),
        "{outcome:?}"
    );
}

async fn submit_reply_command(runtime: &ServiceRuntime, command: BotReplyDeliveryCommand) {
    let handle = runtime
        .submit_task(Task::new(
            format!("reply-command-{}", fastrand::u64(..)),
            BOT_REPLY_DELIVERY_PROTOCOL_ID,
            serde_json::to_value(command).unwrap(),
        ))
        .unwrap();
    let outcome = wait_outcome(runtime, &handle).await;
    assert!(
        matches!(outcome, TaskOutcome::Completed { .. }),
        "{outcome:?}"
    );
}

async fn assert_terminal_send_outcome(
    outcome: TestSendOutcome,
    timeout_ms: Option<u64>,
    expected_status: DeliveryStatus,
    event_id: &str,
    retry: bool,
) {
    let fixture = RuntimeFixture::new([outcome, TestSendOutcome::Success]);
    let runtime = start_runtime(&fixture, true, Duration::from_secs(60), timeout_ms).await;
    submit_event(&runtime, event_id, "wake terminal").await;
    wait_for(&fixture.send_plan.attempts, 1).await;
    let delivery_id = wait_for_delivery_status(fixture.root.path(), expected_status).await;
    assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);

    if retry {
        submit_reply_command(
            &runtime,
            BotReplyDeliveryCommand::RetryPart {
                delivery_id: delivery_id.clone(),
                now_unix_ms: 200,
            },
        )
        .await;
        wait_for(&fixture.sends, 1).await;
        assert_eq!(fixture.send_plan.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            wait_for_delivery_status(fixture.root.path(), DeliveryStatus::Succeeded).await,
            delivery_id
        );
        assert_eq!(fixture.submits.load(Ordering::SeqCst), 1);
    } else {
        // No retry was asked for, so the terminal delivery must stay terminal. Draining first is
        // what proves it: with every flow task finished, a second attempt can no longer be issued.
        wait_for_flow_tasks(&runtime).await;
        assert_eq!(fixture.send_plan.attempts.load(Ordering::SeqCst), 1);
    }
    runtime.shutdown().await;
}

async fn wait_for_delivery_status(root: &std::path::Path, status: DeliveryStatus) -> String {
    let repository = BotStateDbRepository::open(root.join("data/bot/state.sqlite3")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some((receipt, _)) = repository
                .delivery_page(None, 64)
                .unwrap()
                .items
                .into_iter()
                .find(|(receipt, _)| receipt.status == status)
            {
                return receipt.delivery_id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("delivery reaches expected status")
}

async fn wait_for_reply_succeeded(
    root: &std::path::Path,
    reply_id: &str,
) -> mutsuki_bot_protocol::BotReplyDeliveryReceipt {
    let repository = BotStateDbRepository::open(root.join("data/bot/state.sqlite3")).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let receipt = repository.reply_receipt(reply_id).await.unwrap();
            if receipt
                .part_receipts
                .iter()
                .all(|part| part.status == DeliveryStatus::Succeeded)
            {
                return receipt;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("all reply parts succeed")
}

fn multipart_reply_request() -> BotReplyDeliveryRequest {
    let conversation =
        mutsuki_bot_conversation::qq_conversation_from_event(&bot_event("multipart", "wake"))
            .unwrap();
    let audio = test_audio_resource("voice");
    let contents = vec![
        vec![MessageSegment::text("text-only")],
        vec![MessageSegment::Audio {
            resource: audio.clone(),
        }],
        vec![
            MessageSegment::text("text-and-voice"),
            MessageSegment::Audio { resource: audio },
        ],
        vec![MessageSegment::text("long-segment-one")],
        vec![MessageSegment::text("long-segment-two")],
    ];
    BotReplyDeliveryRequest {
        reply_id: "reply-multipart".into(),
        idempotency_key: "reply-multipart".into(),
        conversation,
        parts: contents
            .into_iter()
            .enumerate()
            .map(|(index, segments)| BotReplyDeliveryPart {
                part_id: format!("reply-multipart:part:{index}"),
                content: BotDeliveryContent {
                    segments,
                    summary: None,
                    reply_to: None,
                },
                not_before_unix_ms: None,
            })
            .collect(),
        policy: DeliveryPolicy {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 100,
            not_before_unix_ms: None,
            expires_at_unix_ms: None,
        },
        source_event_id: "multipart".into(),
        source_turn_id: "turn-multipart".into(),
        source_binding_key: None,
        occupancy_only: false,
    }
}

fn test_audio_resource(id: &str) -> ResourceRef {
    ResourceRef {
        resource_id: ResourceId {
            kind_id: "bot.media".into(),
            slot_id: id.into(),
            generation: 1,
            version: 1,
        },
        ref_id: format!("ref-{id}").into(),
        semantic: ResourceSemantic::VersionedSnapshot,
        provider_id: "test.media".into(),
        resource_kind: "bot.media".into(),
        schema: "audio/silk".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: Some(16),
        content_hash: Some(format!("sha256-{id}")),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

async fn submit_event(runtime: &ServiceRuntime, event_id: &str, text: &str) {
    submit_bot_event(runtime, bot_event(event_id, text)).await;
}

async fn submit_event_mentioned(runtime: &ServiceRuntime, event_id: &str, text: &str) {
    let mut event = bot_event(event_id, text);
    event.ext.insert("qqbot.mentioned_bot".into(), json!(true));
    if let Some(message) = event.message.as_mut() {
        message.message_id = Some(format!("msg-{event_id}"));
    }
    submit_bot_event(runtime, event).await;
}

async fn submit_bot_event(runtime: &ServiceRuntime, event: BotEvent) {
    let event_id = event.event_id.clone();
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

fn orchestrated_test_flow() -> BotFlowDocument {
    qq_ai_orchestrated_flow_with_source(
        TEST_SOURCE_NODE_TYPE,
        Some(BotFlowSourceSelector {
            protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
            event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
        }),
    )
}

fn agent_flow() -> BotFlowDocument {
    BotFlowDocument {
        flow_id: "qq-agent".into(),
        name: "QQ Agent".into(),
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
            flow_node("agent", BOT_AGENT_NODE_SUBMIT, json!({}), None),
            flow_node("delivery", "mutsuki.bot.delivery.reply", json!({}), None),
        ],
        edges: vec![
            flow_edge("source-agent", "source", "event", "agent", "input"),
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
                        title: "收到消息".into(),
                        direction: BotNodePortDirection::Output,
                        event_type: BotFlowTypeRef::new(
                            mutsuki_bot_protocol::BOT_FLOW_MESSAGE_EVENT_TYPE,
                            1,
                        ),
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

fn qq_send_catalog_node() -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: "mutsuki.bot.qq.send".into(),
        version: 1,
        title: "发送消息".into(),
        category: "QQ".into(),
        role: BotNodeRole::Sink,
        binding: Some(mutsuki_bot_protocol::BotNodeBinding {
            binding_id: format!("binding:{BOT_MESSAGE_SEND_PROTOCOL_ID}"),
            protocol_id: BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
            runner_hint: Some(TEST_SEND_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "input".into(),
                title: "消息".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                required: false,
            },
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "消息事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(
                    mutsuki_bot_protocol::BOT_FLOW_MESSAGE_EVENT_TYPE,
                    1,
                ),
                required: false,
            },
        ],
        config_schema: json!({"type": "object", "additionalProperties": false}),
    }
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

fn test_protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object"}))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({
            "type": "object",
            "required": ["code", "source", "route"]
        }))
        .build()
}

fn forward_fold_runner_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(
        "mutsuki.bot.adapter.qqbot.openapi",
        TEST_FORWARD_FOLD_PLUGIN_ID,
    )
    .accepted_protocol(BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID)
    .execution_class(ExecutionClass::Orchestration)
    .build()
}

fn qq_forward_fold_catalog_manifest() -> mutsuki_runtime_contracts::PluginManifest {
    let adapter = mutsuki_plugin_bot_adapter_qqbot::qqbot_adapter_manifest(1, false);
    let fold = adapter
        .provides
        .extensions
        .iter()
        .find_map(|extension| {
            BotNodeCatalogFragment::from_plugin_extension(extension)
                .ok()
                .flatten()
        })
        .into_iter()
        .flat_map(|fragment| fragment.nodes)
        .find(|node| node.node_type_id == "mutsuki.bot.qq.reply.forward_fold")
        .expect("adapter publishes forward_fold");
    PluginBuilder::new(TEST_FORWARD_FOLD_PLUGIN_ID)
        .runner_descriptor(forward_fold_runner_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID)
                .input_schema(json!({"type": "object"}))
                .output_schema(json!({
                    "type": "object",
                    "required": ["outputs", "metadata"]
                }))
                .error_schema(json!({
                    "type": "object",
                    "required": ["code", "source", "route"]
                }))
                .build(),
            "mutsuki.bot.adapter.qqbot.openapi",
            "qqbot-reply-forward-fold",
        )
        .extension(
            BotNodeCatalogFragment { nodes: vec![fold] }
                .into_plugin_extension()
                .expect("forward_fold catalog serializes"),
        )
        .build()
        .manifest
}

struct TestForwardFoldRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
}

impl Default for TestForwardFoldRunner {
    fn default() -> Self {
        Self {
            descriptor: forward_fold_runner_descriptor(),
        }
    }
}

impl Runner for TestForwardFoldRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let invocation =
                task.payload
                    .decode_shared::<BotNodeInvocation>()
                    .map_err(|error| {
                        mutsuki_runtime_contracts::RuntimeError::new(
                            mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                            "mutsuki.plugin.bot.adapter.qqbot",
                            error.to_string(),
                        )
                    })?;
            let mut request: BotReplyDeliveryRequest = serde_json::from_value(
                invocation.input.payload.value.clone(),
            )
            .map_err(|error| {
                mutsuki_runtime_contracts::RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                    "mutsuki.plugin.bot.adapter.qqbot",
                    error.to_string(),
                )
            })?;
            mutsuki_plugin_bot_adapter_qqbot::tasks::apply_forward_fold(
                &invocation.config,
                &mut request,
            )
            .map_err(|error| {
                mutsuki_runtime_contracts::RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                    "mutsuki.plugin.bot.adapter.qqbot",
                    error,
                )
            })?;
            let mut event = invocation.input.clone();
            event.payload.value = serde_json::to_value(&request).map_err(|error| {
                mutsuki_runtime_contracts::RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                    "mutsuki.plugin.bot.adapter.qqbot",
                    error.to_string(),
                )
            })?;
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(
                serde_json::to_value(BotNodeResult {
                    outputs: vec![BotNodeOutput {
                        port_id: "reply".into(),
                        event,
                    }],
                    metadata: Default::default(),
                })
                .map_err(|error| {
                    mutsuki_runtime_contracts::RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                        "mutsuki.plugin.bot.adapter.qqbot",
                        error.to_string(),
                    )
                })?,
            );
            Ok(result)
        })
    }
}

struct UnusedMediaRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
}

impl Runner for UnusedMediaRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_error(
            &batch,
            mutsuki_runtime_contracts::RuntimeError::new(
                "test.unexpected_media_call",
                TEST_MEDIA_PLUGIN_ID,
                "disabled_by_policy",
            ),
        ))
    }
}

#[derive(Clone, Copy, Debug)]
enum TestSendOutcome {
    Success,
    TransientFailure,
    PermanentFailure,
    Cancelled,
    Timeout,
}

struct TestSendPlan {
    outcomes: Mutex<VecDeque<TestSendOutcome>>,
    attempts: AtomicUsize,
}

impl TestSendPlan {
    fn new(outcomes: impl IntoIterator<Item = TestSendOutcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            attempts: AtomicUsize::new(0),
        }
    }

    fn next(&self) -> TestSendOutcome {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(TestSendOutcome::Success)
    }
}

struct MessageSendHandler {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    sends: Arc<AtomicUsize>,
    last_send: Arc<Mutex<Option<BotMessage>>>,
    plan: Arc<TestSendPlan>,
}

impl AsyncBatchHandler for MessageSendHandler {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, _ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        let outcome = self.plan.next();
        let sends = self.sends.clone();
        let last_send = self.last_send.clone();
        Box::pin(async move {
            if matches!(outcome, TestSendOutcome::Timeout) {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            map_work_batch_entries(&batch, |task| {
                let (message, as_node) = outbound_send(task);
                match outcome {
                    TestSendOutcome::Success | TestSendOutcome::Timeout => {
                        *last_send.lock().unwrap() = Some(message);
                        sends.fetch_add(1, Ordering::SeqCst);
                        let mut result = RunnerResult::completed(task.task_id.clone());
                        result.output = Some(if as_node {
                            serde_json::to_value(BotNodeResult {
                                outputs: Vec::new(),
                                metadata: BTreeMap::from([(
                                    "receipt".into(),
                                    json!({"id": "qq-message"}),
                                )]),
                            })
                            .unwrap()
                        } else {
                            json!({"id": "qq-message"})
                        });
                        Ok(result)
                    }
                    TestSendOutcome::TransientFailure | TestSendOutcome::PermanentFailure => {
                        let mut error = mutsuki_runtime_contracts::RuntimeError::new(
                            if matches!(outcome, TestSendOutcome::TransientFailure) {
                                "qq.transient"
                            } else {
                                "qq.permanent"
                            },
                            TEST_SEND_PLUGIN_ID,
                            "test.send",
                        );
                        error.evidence.insert(
                            "retryable".into(),
                            ScalarValue::Bool(matches!(outcome, TestSendOutcome::TransientFailure)),
                        );
                        if matches!(outcome, TestSendOutcome::TransientFailure) {
                            error
                                .evidence
                                .insert("retry_after_ms".into(), ScalarValue::Int(10));
                        }
                        Err(error)
                    }
                    TestSendOutcome::Cancelled => {
                        let mut result = RunnerResult::completed(task.task_id.clone());
                        result.status = RunnerStatus::Cancelled;
                        Ok(result)
                    }
                }
            })
        })
    }
}

fn outbound_send(task: &Task) -> (BotMessage, bool) {
    let value = task.payload.to_value();
    if let Ok(invocation) = serde_json::from_value::<BotNodeInvocation>(value.clone()) {
        (
            serde_json::from_value(invocation.input.payload.value).unwrap(),
            true,
        )
    } else {
        (serde_json::from_value(value).unwrap(), false)
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

impl DeliveryGateway for TestDeliveryGateway {
    fn send(
        &self,
        _target: &BotTarget,
        _content: &BotDeliveryContent,
    ) -> Result<DeliverySuccess, DeliveryFailure> {
        Ok(DeliverySuccess {
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
            ref_id: format!("ref-{id}").into(),
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
            cell_id: format!("cell-{id}").into(),
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
    tokio::time::timeout(Duration::from_secs(15), async {
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
    tokio::time::timeout(Duration::from_secs(30), async {
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

async fn wait_for_flow_revision(runtime: &ServiceRuntime, revision: u64) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime
                .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
                .unwrap()
                .active()
                .revision
                == revision
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("flow revision restores after start");
}

async fn wait_for_flow_tasks(runtime: &ServiceRuntime) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshots = runtime.task_snapshots().unwrap();
            if snapshots
                .iter()
                .filter(|snapshot| {
                    !snapshot
                        .task_id
                        .as_str()
                        .starts_with("bot-reply-delivery-recovery:")
                })
                .all(|snapshot| {
                    matches!(
                        snapshot.status,
                        TaskStatus::Completed
                            | TaskStatus::Failed
                            | TaskStatus::Cancelled
                            | TaskStatus::Expired
                            | TaskStatus::DeadLetter
                    )
                })
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Flow tasks did not finish: {:#?}", runtime.task_snapshots()));
}
