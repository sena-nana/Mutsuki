//! The sandbox as a simulated platform adapter.
//!
//! Conversations the console simulates live on `BotPlatform::Sandbox`. The adapter declares
//! itself with a `BotAdapterDescriptor` like any real platform, so:
//!
//! - **compose**: a message typed in the console is published to `mutsuki.bot.flow/ingress@1`
//!   ([`SandboxFlowRuntime::ingest`]) and runs the same business graph a real platform would;
//! - **outbound**: replies reach the sandbox through the account-routed `message/send@2`, which
//!   the Flow outbound router forwards to [`SandboxOutboundRunner`]; the runner records the bot
//!   bubble through the sandbox application service;
//! - **live**: real mode is an observer of every adapter's Flow ingress
//!   ([`sandbox_live_observer`]) and sends as the bot through `message/send@2` on the real
//!   conversation's account ([`SandboxFlowRuntime::deliver`]).
//!
//! Nothing here wraps or depends on another adapter.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_flow::{BotFlowRegistry, BotIngressObserver};
use mutsuki_bot_protocol::{
    BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_MESSAGE_SEND_V2_PROTOCOL_ID, BotAccountRequest,
    BotAdapterDescriptor, BotAdapterEndpoint, BotAdapterOperations, BotConversationRef, BotEvent,
    BotExtMap, BotFlowEventEnvelope, BotMessage, BotPlatform, MessageSegment,
};
use mutsuki_bot_sandbox::{
    SANDBOX_SERVICE_ID, SandboxApi, SandboxError, SandboxRuntime, sandbox_conversation_from_target,
};
use mutsuki_bot_sdk::BotManifestSurface;
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, RunnerDescriptor, RunnerResult, RuntimeError,
    ScalarValue, Task, TaskOutcome, TaskPayload, WorkBatch,
};
use mutsuki_runtime_sdk::{
    PluginBuilder, Runner, RunnerContext, RunnerDescriptorBuilder, RuntimeClientRef, RuntimeResult,
    TaskHandleFuture, map_work_batch_entries,
};
use serde_json::{Value, json};

pub const SANDBOX_ADAPTER_PLUGIN_ID: &str = "mutsuki.plugin.bot.adapter-sandbox";
pub const SANDBOX_OUTBOUND_RUNNER_ID: &str = "mutsuki.bot.adapter.sandbox.outbound";
/// The sandbox's send endpoint behind `mutsuki.bot.message/send@2`; payload `BotAccountRequest`.
pub const SANDBOX_MESSAGE_SEND_PROTOCOL_ID: &str = "mutsuki.bot.sandbox.message/send@1";
/// Key of the live-mode observer on the Flow registry.
pub const SANDBOX_LIVE_OBSERVER_ID: &str = "mutsuki.bot.sandbox.live";

const LIVE_SEND_TIMEOUT: Duration = Duration::from_secs(15);

/// The adapter contract for the sandbox account.
#[must_use]
pub fn sandbox_adapter_descriptor(account_id: impl Into<String>) -> BotAdapterDescriptor {
    BotAdapterDescriptor {
        platform: BotPlatform::Sandbox,
        display_name: "沙盒".into(),
        accounts: vec![account_id.into()],
        operations: BotAdapterOperations {
            send: BotAdapterEndpoint::new(
                SANDBOX_MESSAGE_SEND_PROTOCOL_ID,
                SANDBOX_OUTBOUND_RUNNER_ID,
            ),
            recall: None,
            upload: None,
        },
        config_provider_id: None,
    }
}

/// The adapter plugin: the outbound endpoint, its descriptor and the console's sandbox service.
///
/// # Errors
///
/// Fails when the manifest declares a surface wider than an effect drain may.
pub fn sandbox_adapter_manifest(account_id: impl Into<String>) -> Result<PluginManifest, String> {
    let mut manifest = PluginBuilder::new(SANDBOX_ADAPTER_PLUGIN_ID)
        .metadata("platform", ScalarValue::String("sandbox".into()))
        .metadata("adapter", ScalarValue::Bool(true))
        .runner_descriptor(outbound_descriptor())
        .protocol_handler(
            mutsuki_bot_sdk::manifest::object_protocol_descriptor(
                SANDBOX_MESSAGE_SEND_PROTOCOL_ID,
                &["account", "request"],
                &["id"],
            ),
            SANDBOX_OUTBOUND_RUNNER_ID,
            "sandbox-endpoint-send",
        )
        .extension(
            sandbox_adapter_descriptor(account_id)
                .into_plugin_extension()
                .map_err(|error| error.to_string())?,
        )
        .build()
        .manifest;
    manifest.provides.services.push(SANDBOX_SERVICE_ID.into());
    manifest.provides.capabilities.push("bot.sandbox".into());
    mutsuki_bot_sdk::manifest::checked(manifest, BotManifestSurface::EffectDrain)
}

#[must_use]
pub fn outbound_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(SANDBOX_OUTBOUND_RUNNER_ID, SANDBOX_ADAPTER_PLUGIN_ID)
        .accepted_protocol(SANDBOX_MESSAGE_SEND_PROTOCOL_ID)
        .execution_class(ExecutionClass::Io)
        .build()
}

/// Records replies addressed to simulated conversations as bot bubbles.
pub struct SandboxOutboundRunner {
    descriptor: RunnerDescriptor,
    sandbox: Arc<dyn SandboxApi>,
}

impl SandboxOutboundRunner {
    #[must_use]
    pub fn new(sandbox: Arc<dyn SandboxApi>) -> Self {
        Self {
            descriptor: outbound_descriptor(),
            sandbox,
        }
    }

    fn send(&self, task: &Task) -> Result<Value, RuntimeError> {
        let request: BotAccountRequest<BotMessage> =
            serde_json::from_value(task.payload.to_value())
                .map_err(|error| failure(task, "decode", error))?;
        if request.account.platform != BotPlatform::Sandbox {
            return Err(failure(
                task,
                "account",
                format!(
                    "bot.adapter.account_mismatch: {}:{} is not a sandbox account",
                    request.account.platform.as_str(),
                    request.account.account_id
                ),
            ));
        }
        let message = request.request;
        let conversation =
            sandbox_conversation_from_target(&request.account.account_id, &message.target)
                .ok_or_else(|| failure(task, "target", "unsupported sandbox target"))?;
        let recorded = self
            .sandbox
            .observe_outbound(
                &conversation,
                &message.segments,
                message.reply_to.as_deref(),
            )
            .ok_or_else(|| failure(task, "record", "sandbox did not record the message"))?;
        Ok(json!({ "id": recorded.message_id }))
    }
}

impl Runner for SandboxOutboundRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(self.send(task)?);
            Ok(result)
        })
    }
}

/// The sandbox's link to the running Host: compose into Flow ingress, send live through the
/// account-routed protocol.
pub struct SandboxFlowRuntime {
    client: RuntimeClientRef,
    registry: Arc<BotFlowRegistry>,
}

impl SandboxFlowRuntime {
    #[must_use]
    pub fn new(client: RuntimeClientRef, registry: Arc<BotFlowRegistry>) -> Self {
        Self { client, registry }
    }
}

#[async_trait]
impl SandboxRuntime for SandboxFlowRuntime {
    /// Live mode needs a real adapter in the active `LoadPlan`.
    fn live_available(&self) -> bool {
        self.registry
            .adapters()
            .entries()
            .iter()
            .any(|entry| entry.descriptor.platform != BotPlatform::Sandbox)
    }

    async fn ingest(&self, event: BotEvent) -> Result<(), SandboxError> {
        let event_id = event.event_id.clone();
        let envelope = BotFlowEventEnvelope::from_bot_event(event, None, Some(event_id.clone()))
            .map_err(|error| SandboxError::new("encode_failed", error.to_string()))?;
        let task = Task::new(
            format!("sandbox.ingress:{event_id}"),
            BOT_FLOW_INGRESS_PROTOCOL_ID,
            TaskPayload::from_local(envelope),
        );
        self.client
            .submit_one(task)
            .map_err(|error| SandboxError::new("runtime.submit", error.to_string()))?;
        Ok(())
    }

    async fn deliver(
        &self,
        operation_id: &str,
        conversation: &BotConversationRef,
        segments: &[MessageSegment],
        reply_to: Option<&str>,
    ) -> Result<Value, SandboxError> {
        let target = conversation
            .target()
            .ok_or_else(|| SandboxError::new("invalid_argument", "会话目标无效"))?;
        let request = BotAccountRequest {
            account: conversation.account(),
            request: BotMessage {
                message_id: None,
                target,
                sender: None,
                segments: segments.to_vec(),
                reply_to: reply_to.map(str::to_owned),
                time_ms: None,
                ext: BotExtMap::new(),
            },
        };
        let task = Task::new(
            format!("sandbox.live-send:{operation_id}:{}", unix_nanos()),
            BOT_MESSAGE_SEND_V2_PROTOCOL_ID,
            serde_json::to_value(request)
                .map_err(|error| SandboxError::new("encode_failed", error.to_string()))?,
        );
        let handle = self
            .client
            .submit_one(task)
            .map_err(|error| SandboxError::new("runtime.submit", error.to_string()))?;
        let outcome = tokio::time::timeout(
            LIVE_SEND_TIMEOUT,
            TaskHandleFuture::new(self.client.clone(), handle),
        )
        .await
        .map_err(|_| SandboxError::new("delivery.timeout", "真实消息发送超时"))?
        .map_err(|error| SandboxError::new("delivery.failed", error.to_string()))?;
        match outcome {
            TaskOutcome::Completed { output, .. } => Ok(output.unwrap_or(Value::Null)),
            TaskOutcome::Failed { error, .. } => Err(SandboxError::new(
                error.code.clone(),
                format!("真实消息发送失败（{}）", error.code),
            )),
            other => Err(SandboxError::new(
                "delivery.failed",
                format!("真实消息发送未完成（{other:?}）"),
            )),
        }
    }
}

/// Live mode: every `BotEvent` any adapter submits to Flow ingress is projected into the sandbox.
///
/// The ingress runner must not wait on the sandbox's history writes, so the observer only queues
/// the event; one worker thread applies events in arrival order and exits when the observer is
/// removed from the registry.
///
/// # Panics
///
/// Panics when the worker thread cannot be spawned.
#[must_use]
pub fn sandbox_live_observer(sandbox: Arc<dyn SandboxApi>) -> BotIngressObserver {
    let (sender, receiver) = std::sync::mpsc::channel::<BotEvent>();
    std::thread::Builder::new()
        .name("mutsuki-sandbox-live".into())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                sandbox.observe_event(event);
            }
        })
        .expect("sandbox live observer thread starts");
    let sender = std::sync::Mutex::new(sender);
    Arc::new(move |event: &BotEvent| {
        let _ = sender
            .lock()
            .expect("sandbox live observer mutex")
            .send(event.clone());
    })
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn failure(task: &Task, route: &str, error: impl std::fmt::Display) -> RuntimeError {
    mutsuki_bot_sdk::failure::runtime_error_with_message(
        SANDBOX_ADAPTER_PLUGIN_ID,
        format!("sandbox.outbound.{route}.{}", task.task_id),
        error,
    )
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BotAccountRef, BotAdapterDirectory, BotOutboundOperation, BotTarget,
    };
    use mutsuki_bot_sandbox::{
        DEFAULT_SANDBOX_ACCOUNT_ID, SANDBOX_GROUP_ID, SandboxService, SandboxSpeakerRole,
    };
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchKey, BatchPayload, DispatchLane, OrderingRequirement, WorkResourcePlan,
    };

    use super::*;

    fn account(platform: BotPlatform) -> BotAccountRef {
        BotAccountRef {
            account_id: DEFAULT_SANDBOX_ACCOUNT_ID.into(),
            platform,
        }
    }

    fn send_task(task_id: &str, platform: BotPlatform, text: &str) -> Task {
        Task::new(
            task_id,
            SANDBOX_MESSAGE_SEND_PROTOCOL_ID,
            serde_json::to_value(BotAccountRequest {
                account: account(platform),
                request: BotMessage::text(
                    BotTarget::Group {
                        group_id: SANDBOX_GROUP_ID.into(),
                    },
                    text,
                ),
            })
            .unwrap(),
        )
    }

    fn work_batch(tasks: &[Task]) -> WorkBatch {
        WorkBatch {
            batch_id: "batch".into(),
            tick_id: "tick".into(),
            batch_key: BatchKey::from(SANDBOX_OUTBOUND_RUNNER_ID),
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
            payload: BatchPayload::from_tasks(tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    #[test]
    fn manifest_declares_the_sandbox_adapter_with_a_served_endpoint() {
        let manifest = sandbox_adapter_manifest(DEFAULT_SANDBOX_ACCOUNT_ID).unwrap();
        let directory = BotAdapterDirectory::from_manifests([&manifest]).unwrap();
        let route = directory
            .route(BotOutboundOperation::Send, &account(BotPlatform::Sandbox))
            .unwrap();
        assert_eq!(route.plugin_id, SANDBOX_ADAPTER_PLUGIN_ID);
        assert!(manifest.provides.runners.iter().any(|runner| {
            runner.runner_id == route.endpoint.runner_id
                && runner
                    .accepted_protocol_ids
                    .iter()
                    .any(|protocol| protocol == &route.endpoint.protocol_id)
        }));
        assert!(
            manifest
                .provides
                .handler_bindings
                .iter()
                .any(|binding| binding.binding_id
                    == format!("binding:{SANDBOX_MESSAGE_SEND_PROTOCOL_ID}"))
        );
        // The sandbox serves no account-routed protocol itself: the Flow router owns those.
        assert!(manifest.provides.runners.iter().all(|runner| {
            runner
                .accepted_protocol_ids
                .iter()
                .all(|protocol| protocol != BOT_MESSAGE_SEND_V2_PROTOCOL_ID)
        }));
    }

    #[test]
    fn outbound_records_a_bot_bubble_and_refuses_foreign_accounts() {
        let sandbox = Arc::new(SandboxService::new());
        let mut runner = SandboxOutboundRunner::new(sandbox.clone());
        let tasks = vec![
            send_task("own", BotPlatform::Sandbox, "pong"),
            send_task("foreign", BotPlatform::QqBot, "lost"),
        ];
        let completion = runner
            .run_batch(
                RunnerContext::new(
                    1,
                    1,
                    "executor",
                    None::<mutsuki_runtime_contracts::TaskLeaseId>,
                    "inv",
                ),
                work_batch(&tasks),
            )
            .unwrap();
        let own = completion.results[0].result.as_ref().unwrap();
        let message_id = own.output.as_ref().unwrap()["id"].as_str().unwrap();
        assert!(completion.results[1].error.is_some());

        let conversation = sandbox_conversation_from_target(
            DEFAULT_SANDBOX_ACCOUNT_ID,
            &BotTarget::Group {
                group_id: SANDBOX_GROUP_ID.into(),
            },
        )
        .unwrap();
        let messages =
            futures_executor::block_on(sandbox.messages(&conversation.origin_key())).unwrap();
        let bubble = messages
            .iter()
            .find(|item| item.message_id == message_id)
            .expect("recorded bubble");
        assert_eq!(bubble.role, SandboxSpeakerRole::Bot);
        assert_eq!(bubble.text, "pong");
        assert!(messages.iter().all(|item| item.text != "lost"));
    }
}
