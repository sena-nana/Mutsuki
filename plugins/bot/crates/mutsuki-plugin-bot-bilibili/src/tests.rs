//! Shared fakes and fixtures for the crate-level runner and management tests.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_link_parser::ResolvedLinkCard;
use mutsuki_bot_management::{
    BilibiliCredentialSecretState, BilibiliManagementApi, BilibiliNotificationKind,
};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_INGRESS_PROTOCOL_ID, BotCommandEvent, BotExtMap,
    BotFlowEventEnvelope, BotFlowPayload, BotFlowTypeRef, BotMessage, BotNodeCatalogFragment,
    BotNodeInvocation, BotNodePortDirection, BotNodeResult, BotNodeRole, BotTarget, MessageSegment,
};
use mutsuki_protocol_browser::{BrowserSnapshot, SNAPSHOT, SNAPSHOT_SCHEMA};
use mutsuki_protocol_image::{
    CARD_RENDER, CardLayout, CardRenderRequest, ImageRenderResponse, QR_RENDER, QrRenderRequest,
    Rgba,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, ProtocolClass, RunnerContext, RunnerResult, SurfaceRequirement, Task,
    TaskOutcome, WorkBatch,
};
use mutsuki_runtime_sdk::{ResourceRegistryGateway, Runner, RuntimeResult};
use serde_json::json;

use super::*;

mod management;
mod runner;

use mutsuki_bot_management::BilibiliQrLoginStatus;
use mutsuki_bot_protocol::{
    BotAccountRef, BotEvent, BotEventKind, BotFlowDocument, BotFlowEdge, BotFlowEdgeKind,
    BotFlowNode, BotFlowNodePosition, BotFlowSourceSelector, BotNodeWiring, BotPlatform, BotUser,
};
use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CommandPlan, DispatchLane, ExportPlan, OrderingRequirement,
    PlanReceipt, ReadPlan, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
    ResourceSealState, ResourceSemantic, SnapshotDescriptor, StreamPlan, TaskBatch, TaskHandle,
    WorkResourcePlan, WritePlan,
};
use mutsuki_runtime_sdk::ResourcePlanGateway;

#[derive(Default)]
struct FakeTransportState {
    signature: String,
    qr_starts: u64,
}

struct FakeTransport(Arc<Mutex<FakeTransportState>>);

impl BilibiliTransport for FakeTransport {
    fn poll(
        &mut self,
        _kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError> {
        Ok(vec![BilibiliItem {
            id: "dynamic-1".into(),
            title: "latest".into(),
            url: format!("https://t.bilibili.com/{uid}"),
            image_url: None,
        }])
    }

    fn resolve(&mut self, _url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        unreachable!()
    }

    fn download(&mut self, _url: &str, _max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
        unreachable!()
    }

    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        self.0.lock().unwrap().qr_starts += 1;
        Ok(BilibiliQrCode {
            url: "https://passport.bilibili.com/qr".into(),
            key: "qr-key".into(),
        })
    }

    fn qr_poll(&mut self, _key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        Ok(BilibiliQrPoll {
            status: BilibiliQrStatus::Confirmed,
            credential: Some("SESSDATA=ROTATED".into()),
        })
    }

    fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError> {
        Ok(BilibiliProfile {
            name: format!("user-{uid}"),
            signature: self.0.lock().unwrap().signature.clone(),
        })
    }
}

#[derive(Default)]
struct RecordingCredentialStore(Mutex<Vec<(String, String)>>);

impl BilibiliCredentialStore for RecordingCredentialStore {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
        self.0.lock().unwrap().push((key.into(), credential));
        Ok(())
    }
}

#[derive(Default)]
struct RecordingConfigStore(Mutex<Vec<BilibiliConfig>>);

impl BilibiliConfigStore for RecordingConfigStore {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String> {
        self.0.lock().unwrap().push(config.clone());
        Ok(())
    }
}

struct UnusedResources;

impl ResourcePlanGateway for UnusedResources {
    fn collect_read_plan(&self, _: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        unreachable!()
    }
    fn snapshot_read_plan(
        &self,
        _: &ReadPlan,
        _: &str,
        _: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        unreachable!()
    }
    fn open_stream_plan(&self, _: &ReadPlan) -> RuntimeResult<StreamPlan> {
        unreachable!()
    }
    fn execute_export_plan(&self, _: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        unreachable!()
    }
    fn commit_write_plan(&self, _: &WritePlan, _: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        unreachable!()
    }
    fn execute_command_plan(&self, _: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        unreachable!()
    }
    fn execute_command_batch(&self, _: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        unreachable!()
    }
    fn execute_saga_plan(&self, _: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        unreachable!()
    }
}

impl ResourceRegistryGateway for UnusedResources {
    fn open_resource_descriptor(&self, _: &str) -> RuntimeResult<ResourceRef> {
        unreachable!()
    }
    fn create_blob_resource(&self, _: &str, _: &str, _: Vec<u8>) -> RuntimeResult<ResourceRef> {
        unreachable!()
    }
    fn create_cow_state_resource(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        unreachable!()
    }
    fn create_capability_resource(&self, _: &str, _: &str, _: &str) -> RuntimeResult<ResourceRef> {
        unreachable!()
    }
}

struct SnapshotResources {
    bytes: Vec<u8>,
}

impl ResourcePlanGateway for SnapshotResources {
    fn collect_read_plan(&self, _: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Ok(self.bytes.clone())
    }
    fn snapshot_read_plan(
        &self,
        _: &ReadPlan,
        _: &str,
        _: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        unreachable!()
    }
    fn open_stream_plan(&self, _: &ReadPlan) -> RuntimeResult<StreamPlan> {
        unreachable!()
    }
    fn execute_export_plan(&self, _: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        unreachable!()
    }
    fn commit_write_plan(&self, _: &WritePlan, _: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        unreachable!()
    }
    fn execute_command_plan(&self, _: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        unreachable!()
    }
    fn execute_command_batch(&self, _: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        unreachable!()
    }
    fn execute_saga_plan(&self, _: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        unreachable!()
    }
}

impl ResourceRegistryGateway for SnapshotResources {
    fn open_resource_descriptor(&self, _: &str) -> RuntimeResult<ResourceRef> {
        Ok(snapshot_resource())
    }
    fn create_blob_resource(&self, _: &str, _: &str, _: Vec<u8>) -> RuntimeResult<ResourceRef> {
        unreachable!()
    }
    fn create_cow_state_resource(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Ok(snapshot_resource())
    }
    fn create_capability_resource(&self, _: &str, _: &str, _: &str) -> RuntimeResult<ResourceRef> {
        unreachable!()
    }
}

fn snapshot_resource() -> ResourceRef {
    ResourceRef {
        ref_id: "bilibili-risk-snapshot".into(),
        resource_id: ResourceId {
            kind_id: "browser.snapshot".into(),
            slot_id: "bilibili-risk-snapshot".into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::CowVersionedState,
        provider_id: "memory".into(),
        resource_kind: "browser.snapshot".into(),
        schema: SNAPSHOT_SCHEMA.into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::ProviderRpc {
            provider_id: "memory".into(),
            method: "memory".into(),
        },
        size_hint: Some(0),
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

struct CompletedChildClient;

impl mutsuki_runtime_sdk::TaskSubmitter for CompletedChildClient {
    fn submit_batch(&self, _: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        unreachable!()
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(Some(TaskOutcome::Completed {
            task_id: handle.task_id.clone(),
            output: None,
            output_ref: None,
        }))
    }

    // A fake without cancellation state: accepting the request is all it can do.
    fn cancel_task(&self, _handle: &TaskHandle) -> mutsuki_runtime_sdk::RuntimeResult<()> {
        Ok(())
    }
}

impl mutsuki_runtime_sdk::RuntimeClient for CompletedChildClient {}

struct RenderedChildClient;

impl mutsuki_runtime_sdk::TaskSubmitter for RenderedChildClient {
    fn submit_batch(&self, _: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        unreachable!()
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(Some(TaskOutcome::Completed {
            task_id: handle.task_id.clone(),
            output: Some(
                serde_json::to_value(ImageRenderResponse {
                    resource: rendered_image_resource(),
                    width: 1200,
                    height: 630,
                    byte_len: 128,
                })
                .unwrap(),
            ),
            output_ref: None,
        }))
    }

    // A fake without cancellation state: accepting the request is all it can do.
    fn cancel_task(&self, _handle: &TaskHandle) -> mutsuki_runtime_sdk::RuntimeResult<()> {
        Ok(())
    }
}

impl mutsuki_runtime_sdk::RuntimeClient for RenderedChildClient {}

fn rendered_image_resource() -> ResourceRef {
    let mut resource = snapshot_resource();
    resource.ref_id = "bilibili-rendered-card".into();
    resource.resource_id.kind_id = "blob".into();
    resource.resource_id.slot_id = "bilibili-rendered-card".into();
    resource.resource_kind = "blob".into();
    resource.schema = mutsuki_protocol_image::PNG_SCHEMA.into();
    resource.semantic = ResourceSemantic::FrozenValue;
    resource
}

struct LinkTransport;

impl BilibiliTransport for LinkTransport {
    fn poll(&mut self, _: &BilibiliPollKind, _: u64) -> Result<Vec<BilibiliItem>, BilibiliError> {
        unreachable!()
    }
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        Ok(ResolvedLinkCard {
            url: url.into(),
            title: "视频标题".into(),
            description: "视频简介".into(),
            image_url: None,
        })
    }
    fn download(&mut self, _: &str, _: usize) -> Result<Vec<u8>, BilibiliError> {
        unreachable!()
    }
    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        unreachable!()
    }
    fn qr_poll(&mut self, _: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        unreachable!()
    }
    fn profile(&mut self, _: u64) -> Result<BilibiliProfile, BilibiliError> {
        unreachable!()
    }
}

struct MultiPollTransport;

impl BilibiliTransport for MultiPollTransport {
    fn poll(&mut self, _: &BilibiliPollKind, _: u64) -> Result<Vec<BilibiliItem>, BilibiliError> {
        Ok(["3", "2", "1"]
            .into_iter()
            .map(|id| BilibiliItem {
                id: id.into(),
                title: format!("动态 {id}"),
                url: format!("https://t.bilibili.com/{id}"),
                image_url: None,
            })
            .collect())
    }
    fn resolve(&mut self, _: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        unreachable!()
    }
    fn download(&mut self, _: &str, _: usize) -> Result<Vec<u8>, BilibiliError> {
        unreachable!()
    }
    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        unreachable!()
    }
    fn qr_poll(&mut self, _: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        unreachable!()
    }
    fn profile(&mut self, _: u64) -> Result<BilibiliProfile, BilibiliError> {
        unreachable!()
    }
}

struct RiskControlledTransport;

impl BilibiliTransport for RiskControlledTransport {
    fn poll(&mut self, _: &BilibiliPollKind, _: u64) -> Result<Vec<BilibiliItem>, BilibiliError> {
        Err(BilibiliError::RiskControl352)
    }
    fn resolve(&mut self, _: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        unreachable!()
    }
    fn download(&mut self, _: &str, _: usize) -> Result<Vec<u8>, BilibiliError> {
        unreachable!()
    }
    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        unreachable!()
    }
    fn qr_poll(&mut self, _: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        unreachable!()
    }
    fn profile(&mut self, _: u64) -> Result<BilibiliProfile, BilibiliError> {
        unreachable!()
    }
}

struct AlwaysPresentSecrets;

impl BilibiliSecretPresence for AlwaysPresentSecrets {
    fn inspect(&self, _key: &str) -> BilibiliCredentialSecretState {
        BilibiliCredentialSecretState::Present
    }
}

struct FakeQrRenderer;

#[async_trait]
impl BilibiliQrRenderer for FakeQrRenderer {
    async fn render_qr(&self, content: &str) -> Result<Vec<u8>, BilibiliError> {
        assert_eq!(content, "https://passport.bilibili.com/qr");
        Ok(vec![1, 2, 3, 4])
    }
}

fn managed_config() -> BilibiliConfig {
    BilibiliConfig {
        backend: BilibiliBackendConfig::WebCookie {
            cookie_secret_key: "BILIBILI_COOKIE".into(),
        },
        live_interval_ms: 1_000,
        dynamic_interval_ms: 1_000,
        video_interval_ms: 1_000,
        retry: RetryConfig {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 100,
        },
        subscriptions: Vec::new(),
        link_resolver: LinkResolverConfig {
            enabled: false,
            cooldown_ms: 1_000,
            account_to_binding: BTreeMap::new(),
        },
        media_provider_id: "memory".into(),
        risk_control: None,
        management: BilibiliManagementConfig {
            enabled: true,
            allow_self_binding: true,
            admin_user_ids: vec!["admin".into()],
            self_binding_notifications: vec![BilibiliPollKind::Dynamic],
            self_binding_outbound_binding: "qq-main".into(),
        },
    }
}

fn login_runtime_runner(
    config: BilibiliConfig,
) -> (Arc<Mutex<FakeTransportState>>, Box<dyn Runner>) {
    let state = Arc::new(Mutex::new(FakeTransportState::default()));
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let shared = SharedBilibiliConfig::new(config);
    let management = Arc::new(BilibiliManagementService::new(
        shared.clone(),
        SharedBilibiliCredential::default(),
        Box::new(FakeTransport(state.clone())),
        repository.clone(),
        Arc::new(RecordingCredentialStore::default()),
        Arc::new(RecordingConfigStore::default()),
        Arc::new(AlwaysPresentSecrets),
    ));
    let runner = BilibiliRunner::new(
        Box::new(FakeTransport(state.clone())),
        repository,
        Arc::new(UnusedResources),
        "memory",
    )
    .with_management(shared, management)
    .into_runtime_runner(Arc::new(RenderedChildClient), None);
    (state, runner)
}

fn command_task(task_id: &str, actor_id: &str, args: &[&str]) -> Task {
    let target = BotTarget::Group {
        group_id: "group".into(),
    };
    Task::new(
        task_id,
        MANAGEMENT_COMMAND,
        serde_json::to_value(BotCommandEvent {
            source: BotEvent {
                event_id: format!("event-{task_id}"),
                platform: BotPlatform::QqBot,
                bot: BotAccountRef {
                    account_id: "bot".into(),
                    platform: BotPlatform::QqBot,
                },
                kind: BotEventKind::MessageCreated,
                time_ms: 1,
                target: target.clone(),
                actor: Some(BotUser {
                    user_id: actor_id.into(),
                    display_name: None,
                    avatar_url: None,
                }),
                message: Some(BotMessage::text(target, "/bili")),
                raw: None,
                ext: BotExtMap::new(),
            },
            name: "bili".into(),
            args: args.iter().map(|value| (*value).into()).collect(),
            command_path: vec!["bili".into()],
            typed_args: Default::default(),
            raw_text: format!("/bili {}", args.join(" ")),
        })
        .unwrap(),
    )
}

fn command_batch(tasks: Vec<Task>) -> WorkBatch {
    WorkBatch {
        batch_id: "batch".into(),
        tick_id: "tick".into(),
        batch_key: RUNNER_ID.into(),
        entries: tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: format!("entry-{index}").into(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: None,
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::PreserveSubmitOrder,
            })
            .collect(),
        payload: BatchPayload::from_tasks(&tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: Vec::new(),
    }
}

struct CountingTransport {
    polls: Arc<AtomicU64>,
}

impl BilibiliTransport for CountingTransport {
    fn poll(
        &mut self,
        _kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError> {
        let sequence = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(vec![BilibiliItem {
            id: format!("item-{sequence}"),
            title: format!("item {sequence}"),
            url: format!("https://t.bilibili.com/{uid}/{sequence}"),
            image_url: None,
        }])
    }

    fn resolve(&mut self, _url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        unreachable!()
    }

    fn download(&mut self, _url: &str, _max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
        unreachable!()
    }

    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        unreachable!()
    }

    fn qr_poll(&mut self, _key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        unreachable!()
    }

    fn profile(&mut self, _uid: u64) -> Result<BilibiliProfile, BilibiliError> {
        unreachable!()
    }
}

fn push_flow_document(wired: bool) -> BotFlowDocument {
    let mut flow = BotFlowDocument {
        flow_id: "push".into(),
        name: "push".into(),
        nodes: vec![
            BotFlowNode {
                node_id: "push".into(),
                node_type_id: BILIBILI_NOTIFICATION_NODE_TYPE.into(),
                node_type_version: 1,
                config: json!({}),
                source: Some(BotFlowSourceSelector {
                    protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                    event_type: Some(BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1)),
                }),
                position: BotFlowNodePosition::default(),
            },
            BotFlowNode {
                node_id: "card".into(),
                node_type_id: BILIBILI_CARD_NODE_TYPE.into(),
                node_type_version: 1,
                config: json!({}),
                source: None,
                position: BotFlowNodePosition::default(),
            },
        ],
        edges: Vec::new(),
    };
    if wired {
        flow.edges.push(BotFlowEdge {
            edge_id: "push-card".into(),
            from_node_id: "push".into(),
            from_port_id: "event".into(),
            to_node_id: "card".into(),
            to_port_id: "event".into(),
            kind: BotFlowEdgeKind::Event,
        });
    }
    flow
}

fn push_registry(wired: bool) -> Arc<BotFlowRegistry> {
    use mutsuki_bot_flow::BotNodeCatalog;
    use mutsuki_bot_protocol::BotFlowSnapshot;

    let manifest = manifest_with_surfaces(false, false);
    Arc::new(
        BotFlowRegistry::with_snapshot(
            BotNodeCatalog::from_manifests(&[manifest]).unwrap(),
            BotFlowSnapshot {
                revision: 1,
                flow: push_flow_document(wired),
            },
        )
        .unwrap(),
    )
}

fn run_poll_task(runner: &mut Box<dyn Runner>, task_id: &str) -> RunnerResult {
    let task = Task::new(
        task_id,
        POLL_LIVE,
        serde_json::to_value(PollRequest {
            subscription_id: "sub".into(),
            uid: 7,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            outbound_binding: "qq-main".into(),
        })
        .unwrap(),
    );
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
    let batch = command_batch(vec![task]);
    let completed = runner.run_batch(context, batch).unwrap();
    completed.results[0].result.as_ref().unwrap().clone()
}

fn count_runner(polls: Arc<AtomicU64>, registry: Option<Arc<BotFlowRegistry>>) -> Box<dyn Runner> {
    let mut runner = BilibiliRunner::new(
        Box::new(CountingTransport {
            polls: polls.clone(),
        }),
        Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
        Arc::new(UnusedResources),
        "memory",
    );
    if let Some(registry) = registry {
        runner = runner.with_flow_registry(registry);
    }
    runner.into_runtime_runner(Arc::new(CompletedChildClient), None)
}

fn managed_status_service(state: Arc<Mutex<FakeTransportState>>) -> BilibiliManagementService {
    BilibiliManagementService::new(
        SharedBilibiliConfig::new(managed_config()),
        SharedBilibiliCredential::default(),
        Box::new(FakeTransport(state)),
        Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
        Arc::new(RecordingCredentialStore::default()),
        Arc::new(RecordingConfigStore::default()),
        Arc::new(AlwaysPresentSecrets),
    )
}
