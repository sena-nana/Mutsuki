//! The Bilibili task runner: poll, link-resolve, notification-card and management dispatch.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_link_parser::{ResolvedLinkCard, preferred_event_url};
use mutsuki_bot_management::BilibiliManagementApi;
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_INGRESS_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BotCommandEvent, BotEvent, BotExtMap, BotFlowContext, BotFlowEventEnvelope, BotFlowPayload,
    BotFlowTypeRef, BotMessage, BotNodeInvocation, BotNodeOutput, BotNodeResult,
};
use mutsuki_protocol_browser::{
    BrowserSnapshot, BrowserSnapshotRequest, BrowserWaitMode, SNAPSHOT, SNAPSHOT_SCHEMA,
};
use mutsuki_runtime_contracts::{
    DomainEvent, ReadPlan, RunnerResult, RuntimeError, Task, TaskOutcome,
};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, ResourceRegistryGateway, Runner, RuntimeClientRef, RuntimeFailure,
    RuntimeResult, TaskAwaitRunnerAdapter,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{BilibiliCommands, require_admin};
use crate::error::{bili_error, bili_management_error, risk_control_failure};
use crate::manifest::runner_descriptor;
use crate::parse::{fresh_since, parse_dynamic_snapshot};
use crate::render::{
    CardMedia, CardRoute, PreparedCard, PreparedCards, layout_for_url, render_cards,
    render_qr_child, run_notification_card,
};
use crate::transport::ensure_bilibili_domain;
use crate::{
    BILIBILI_EVENT_TYPE, BilibiliError, BilibiliItem, BilibiliNotification, BilibiliPollKind,
    BilibiliRiskControlConfig, BilibiliTransport, LINK_RESOLVE, LinkResolveRequest,
    MANAGEMENT_COMMAND, NOTIFY_CARD, PLUGIN_ID, PollRequest, RISK_CONTROL_STATUS_EVENT,
    SharedBilibiliConfig, SqliteBilibiliRepository,
};

pub struct BilibiliRunner {
    /// Behind its own lock so a blocking upstream round trip never holds the
    /// runner mutex. It used to: every poll and every `/bili` command queued
    /// behind whichever request was currently in flight. Transport calls stay
    /// serialised among themselves because the trait takes `&mut self` and the
    /// session state it carries is shared.
    transport: Arc<Mutex<Box<dyn BilibiliTransport>>>,
    repository: Arc<SqliteBilibiliRepository>,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
    managed_config: Option<SharedBilibiliConfig>,
    management: Option<Arc<dyn BilibiliManagementApi>>,
    flow_registry: Option<Arc<BotFlowRegistry>>,
    /// Cursor keys skipped while the push chain was unwired. The next wired
    /// poll baselines them so the frozen window never replays as a backlog.
    frozen_polls: BTreeSet<String>,
}

impl BilibiliRunner {
    pub fn new(
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
    ) -> Self {
        Self {
            transport: Arc::new(Mutex::new(transport)),
            repository,
            resources,
            media_provider_id: media_provider_id.into(),
            managed_config: None,
            management: None,
            flow_registry: None,
            frozen_polls: BTreeSet::new(),
        }
    }

    /// Shares the Flow registry so the poll path can ask whether the push
    /// Source chain is wired into the active graph. Without a registry the
    /// runner keeps the historical always-poll behavior.
    pub fn with_flow_registry(mut self, registry: Arc<BotFlowRegistry>) -> Self {
        self.flow_registry = Some(registry);
        self
    }

    /// `None` when no registry is injected (wiring unknown); otherwise whether
    /// the `mutsuki.bot.bilibili.notification` Source chain has downstream.
    fn push_source_wired(&self) -> Option<bool> {
        self.flow_registry.as_ref().map(|registry| {
            registry.source_wired(BOT_EVENT_INGEST_PROTOCOL_ID, Some((BILIBILI_EVENT_TYPE, 1)))
        })
    }

    pub fn with_management(
        mut self,
        config: SharedBilibiliConfig,
        management: Arc<dyn BilibiliManagementApi>,
    ) -> Self {
        self.managed_config = Some(config);
        self.management = Some(management);
        self
    }

    pub fn into_runtime_runner(
        self,
        client: RuntimeClientRef,
        risk_control: Option<BilibiliRiskControlConfig>,
    ) -> Box<dyn Runner> {
        let descriptor = runner_descriptor(risk_control.is_some());
        let state = Arc::new(Mutex::new(self));
        let factory = Box::new(move |ctx: AsyncRunnerContext, task: Task| {
            let state = state.clone();
            let risk_control = risk_control.clone();
            Box::pin(async move { run_task_async(ctx, task, state, risk_control).await })
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
                >
        });
        Box::new(
            TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false),
        )
    }

    pub(crate) fn finish_poll(
        &mut self,
        task: &Task,
        request: PollRequest,
        kind: BilibiliPollKind,
        items: Vec<BilibiliItem>,
        status: Option<DomainEvent>,
        force_baseline: bool,
    ) -> Result<RunnerResult, RuntimeError> {
        let key = poll_cursor_key(&kind, &request);
        let previous = if force_baseline {
            None
        } else {
            self.repository
                .cursor(&key)
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?
        };
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.events.extend(status);
        let Some(head) = items.first().map(|item| item.id.clone()) else {
            return Ok(result);
        };
        let Some(previous) = previous else {
            self.repository
                .set_cursor(&key, &head)
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
            return Ok(result);
        };
        for (index, item) in fresh_since(items, &previous).into_iter().enumerate() {
            result.tasks.push(notification_ingress_task(
                task,
                BilibiliNotification {
                    kind,
                    subscription_id: request.subscription_id.clone(),
                    uid: request.uid,
                    target: request.target.clone(),
                    item_id: item.id,
                    title: item.title,
                    url: item.url,
                    image_url: item.image_url,
                },
                index,
            ));
        }
        self.repository
            .set_cursor(&key, &head)
            .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
        Ok(result)
    }

    /// The command surface, cloned out from under the runner lock.
    pub(crate) fn commands(&self) -> BilibiliCommands {
        BilibiliCommands {
            management: self.management.clone(),
            managed_config: self.managed_config.clone(),
        }
    }

    /// Everything the card path needs, cloned out from under the runner lock so
    /// the cover download and resource write happen without it held.
    pub(crate) fn card_media(&self) -> CardMedia {
        CardMedia {
            transport: self.transport.clone(),
            resources: self.resources.clone(),
            media_provider_id: self.media_provider_id.clone(),
        }
    }
}

async fn run_task_async(
    ctx: AsyncRunnerContext,
    mut task: Task,
    state: Arc<Mutex<BilibiliRunner>>,
    risk_control: Option<BilibiliRiskControlConfig>,
) -> RuntimeResult<RunnerResult> {
    if task.protocol_id == MANAGEMENT_COMMAND {
        let invocation = serde_json::from_value::<BotNodeInvocation>(task.payload.to_value()).ok();
        if let Some(invocation) = &invocation {
            task.payload = mutsuki_runtime_contracts::TaskPayload::from_local(
                invocation.input.payload.value.clone(),
            );
        }
        let result = run_management_task(ctx, task, state).await?;
        return if let Some(invocation) = invocation {
            wrap_management_node_result(result, invocation)
        } else {
            Ok(result)
        };
    }
    if task.protocol_id == LINK_RESOLVE {
        let payload: Value = task.payload.clone().into();
        let invocation = serde_json::from_value::<BotNodeInvocation>(payload).ok();
        let request = match &invocation {
            Some(invocation) => {
                link_resolve_request_from_invocation(invocation).map_err(|error| {
                    RuntimeFailure::new(bili_error(&task, BilibiliError::InvalidResponse(error)))
                })?
            }
            None => decode(&task).map_err(RuntimeFailure::new)?,
        };
        let prepared = {
            // The cooldown read needs the runner and is cheap; the resolve and the
            // cover download are blocking HTTP and must not hold its lock.
            let (media, repository) = {
                let runner = state.lock().expect("Bilibili runner mutex");
                (runner.card_media(), runner.repository.clone())
            };
            let cooldown_key = format!("{}:{}", request.account_id, request.url);
            if !repository
                .cooldown_ready(&cooldown_key, request.now_ms, request.cooldown_ms)
                .map_err(|error| {
                    RuntimeFailure::new(bili_error(
                        &task,
                        BilibiliError::Transport(error.to_string()),
                    ))
                })?
            {
                None
            } else {
                let card = media
                    .resolve(&request.url)
                    .map_err(|error| RuntimeFailure::new(bili_error(&task, error)))?;
                let (layout, kicker, live) = layout_for_url(&card.url);
                let card = media
                    .prepare_card_request(card, &task, layout, kicker, live)
                    .map_err(RuntimeFailure::new)?;
                Some(PreparedCards {
                    result: RunnerResult::completed(task.task_id.clone()),
                    cards: vec![PreparedCard {
                        target: request.target,
                        request: card,
                        route: CardRoute::Notify(request.outbound_binding),
                    }],
                    cursor_update: None,
                    cooldown_update: Some((repository, cooldown_key, request.now_ms)),
                })
            }
        };
        let result = match prepared {
            Some(prepared) => render_cards(&ctx, &task, prepared).await?,
            None => RunnerResult::completed(task.task_id.clone()),
        };
        return if let Some(invocation) = invocation {
            wrap_management_node_result(result, invocation)
        } else {
            Ok(result)
        };
    }
    if task.protocol_id == NOTIFY_CARD {
        let invocation = serde_json::from_value::<BotNodeInvocation>(task.payload.to_value())
            .map_err(|error| {
                RuntimeFailure::new(bili_error(
                    &task,
                    BilibiliError::InvalidResponse(format!(
                        "notify card node requires a flow invocation: {error}"
                    )),
                ))
            })?;
        return run_notification_card(&ctx, &task, state, invocation).await;
    }
    let request: PollRequest = decode(&task).map_err(RuntimeFailure::new)?;
    let kind = BilibiliPollKind::from_protocol_id(task.protocol_id.as_str()).ok_or_else(|| {
        RuntimeFailure::new(bili_error(
            &task,
            BilibiliError::InvalidResponse("unsupported poll protocol".into()),
        ))
    })?;
    // Flow is the only initiation surface: when the push Source chain is not
    // wired the business is frozen, so the upstream poll is skipped entirely.
    // The skipped cursor key baselines on the first wired poll instead of
    // replaying the frozen window as a notification backlog.
    let poll_plan = {
        let mut runner = state.lock().expect("Bilibili runner mutex");
        let key = poll_cursor_key(&kind, &request);
        match runner.push_source_wired() {
            Some(false) => {
                runner.frozen_polls.insert(key);
                None
            }
            Some(true) => Some(runner.frozen_polls.take(&key).is_some()),
            None => Some(false),
        }
    };
    let Some(force_baseline) = poll_plan else {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.output = Some(json!({ "push_wired": false, "poll_skipped": true }));
        return Ok(result);
    };
    // Clone the handle, then drop the runner guard before the blocking call:
    // holding it here serialised every other poll and every `/bili` command
    // behind whichever upstream request happened to be in flight.
    let transport = state
        .lock()
        .expect("Bilibili runner mutex")
        .transport
        .clone();
    let attempt = transport
        .lock()
        .expect("Bilibili transport mutex")
        .poll(&kind, request.uid);
    match attempt {
        Ok(items) => state
            .lock()
            .expect("Bilibili runner mutex")
            .finish_poll(&task, request, kind, items, None, force_baseline)
            .map_err(RuntimeFailure::new),
        Err(BilibiliError::RiskControl352)
            if kind == BilibiliPollKind::Dynamic && risk_control.is_some() =>
        {
            run_chromium_risk_control_fallback(
                ctx,
                task,
                request,
                state,
                risk_control.expect("checked"),
            )
            .await
        }
        Err(error) => Err(RuntimeFailure::new(bili_error(&task, error))),
    }
}

const BILIBILI_LINK_HOSTS: &[&str] = &["b23.tv", "bilibili.com", "hdslb.com"];

#[derive(Default, Deserialize)]
#[serde(default)]
struct BilibiliLinkFlowConfig {
    url: Option<String>,
    outbound_binding: Option<String>,
    cooldown_ms: Option<u64>,
}

fn link_resolve_request_from_invocation(
    invocation: &BotNodeInvocation,
) -> Result<LinkResolveRequest, String> {
    let flow: BilibiliLinkFlowConfig =
        serde::Deserialize::deserialize(&invocation.config).map_err(|error| error.to_string())?;
    let event: BotEvent = serde::Deserialize::deserialize(&invocation.input.payload.value)
        .map_err(|error| error.to_string())?;
    let url = flow
        .url
        .filter(|value| !value.is_empty())
        .or_else(|| preferred_event_url(&event, BILIBILI_LINK_HOSTS))
        .ok_or_else(|| "bilibili url is missing".to_string())?;
    let outbound_binding = flow
        .outbound_binding
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    let cooldown_ms = flow.cooldown_ms.unwrap_or(1_000);
    Ok(LinkResolveRequest {
        url,
        target: event.target,
        outbound_binding,
        account_id: event.bot.account_id,
        now_ms: event.time_ms.max(0).cast_unsigned(),
        cooldown_ms,
    })
}

fn wrap_management_node_result(
    mut result: RunnerResult,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    let outputs = result
        .tasks
        .drain(..)
        .map(|task| {
            if task.protocol_id != BOT_MESSAGE_SEND_PROTOCOL_ID {
                return Err(RuntimeFailure::new(RuntimeError::new(
                    "bot.bilibili.node.unexpected_task",
                    PLUGIN_ID,
                    task.protocol_id,
                )));
            }
            let message: BotMessage =
                serde_json::from_value(task.payload.to_value()).map_err(|error| {
                    RuntimeFailure::new(RuntimeError::new(
                        "bot.bilibili.node.output_invalid",
                        PLUGIN_ID,
                        error.to_string(),
                    ))
                })?;
            Ok(BotNodeOutput {
                port_id: "message".into(),
                event: BotFlowEventEnvelope::outbound_message(
                    &invocation.input,
                    format!("{}:message", invocation.input.event_id),
                    serde_json::to_value(message).expect("BotMessage serializes"),
                ),
            })
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs,
            metadata: Default::default(),
        })
        .expect("BotNodeResult serializes"),
    );
    Ok(result)
}

async fn run_management_task(
    ctx: AsyncRunnerContext,
    task: Task,
    state: Arc<Mutex<BilibiliRunner>>,
) -> RuntimeResult<RunnerResult> {
    let command: BotCommandEvent = decode(&task).map_err(RuntimeFailure::new)?;
    let action = command.args.first().map(String::as_str).unwrap_or("help");
    if action != "login" && action != "preview" {
        let commands = state.lock().expect("Bilibili runner mutex").commands();
        return commands.run_command(&task).map_err(RuntimeFailure::new);
    }
    let (management, config, actor_id, is_admin) = {
        let runner = state.lock().expect("Bilibili runner mutex");
        let Some(management) = runner.management.clone() else {
            return Ok(RunnerResult::completed(task.task_id));
        };
        let config = runner
            .managed_config
            .as_ref()
            .expect("management config is installed with the API")
            .snapshot();
        // Only `login` and `preview` reach this block; QR login is the sole
        // credential path and stays available with management switched off.
        if action == "preview" && !config.management.enabled {
            return Ok(RunnerResult::completed(task.task_id));
        }
        let actor_id = command
            .source
            .actor
            .as_ref()
            .map(|actor| actor.user_id.clone())
            .ok_or_else(|| RuntimeFailure::new(bili_error(&task, BilibiliError::Forbidden)))?;
        let is_admin = config
            .management
            .admin_user_ids
            .iter()
            .any(|candidate| candidate == &actor_id);
        (management, config, actor_id, is_admin)
    };
    if action == "login" {
        require_admin(is_admin).map_err(|error| RuntimeFailure::new(bili_error(&task, error)))?;
        let session = management
            .login_start_session(&actor_id)
            .map_err(|error| RuntimeFailure::new(bili_management_error(&task, error)))?;
        let rendered = render_qr_child(&ctx, &task, session.url).await?;
        return Ok(state
            .lock()
            .expect("Bilibili runner mutex")
            .commands()
            .command_reply(
                &task,
                &command,
                "请使用 Bilibili App 扫码确认，然后发送 /bili login-status；二维码不会把 Cookie 写入聊天或 Task payload。",
                Some(rendered.resource),
            ));
    }
    match management.preview(&actor_id, is_admin, command.args.get(1).map(String::as_str)) {
        Ok(card) => {
            let (layout, kicker, live) = layout_for_url(&card.url);
            let media = state.lock().expect("Bilibili runner mutex").card_media();
            let request = media
                .prepare_card_request(
                    ResolvedLinkCard {
                        url: card.url,
                        title: card.title,
                        description: card.description,
                        image_url: card.image_url,
                    },
                    &task,
                    layout,
                    kicker,
                    live,
                )
                .map_err(RuntimeFailure::new)?;
            render_cards(
                &ctx,
                &task,
                PreparedCards {
                    result: RunnerResult::completed(task.task_id.clone()),
                    cards: vec![PreparedCard {
                        target: command.source.target,
                        request,
                        route: CardRoute::Command(Some(
                            config.management.self_binding_outbound_binding,
                        )),
                    }],
                    cursor_update: None,
                    cooldown_update: None,
                },
            )
            .await
        }
        Err(error) if error.message.contains("暂无可预览") => Ok(state
            .lock()
            .expect("Bilibili runner mutex")
            .commands()
            .command_reply(&task, &command, error.message, None)),
        Err(error) => Err(RuntimeFailure::new(bili_management_error(&task, error))),
    }
}

fn notification_ingress_task(
    parent: &Task,
    notification: BilibiliNotification,
    index: usize,
) -> Task {
    let context = BotFlowContext {
        bot: None,
        target: Some(notification.target.clone()),
        actor: None,
        ext: BotExtMap::new(),
    };
    let envelope = BotFlowEventEnvelope {
        event_id: format!("{}:notify:{index}", parent.task_id),
        protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
        payload: BotFlowPayload {
            event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
            value: serde_json::to_value(notification).expect("BilibiliNotification serializes"),
        },
        context,
        trace_id: parent.trace_id.clone().map(Into::into),
        correlation_id: parent
            .correlation_id
            .clone()
            .or_else(|| Some(parent.task_id.to_string())),
    };
    let mut child = Task::new(
        format!("mutsuki.bot.flow.ingress:{}:notify:{index}", parent.task_id),
        BOT_FLOW_INGRESS_PROTOCOL_ID,
        mutsuki_runtime_contracts::TaskPayload::from_local(envelope),
    );
    child.registry_generation = parent.registry_generation;
    child
}

async fn run_chromium_risk_control_fallback(
    ctx: AsyncRunnerContext,
    task: Task,
    request: PollRequest,
    state: Arc<Mutex<BilibiliRunner>>,
    risk_control: BilibiliRiskControlConfig,
) -> RuntimeResult<RunnerResult> {
    let (resources, media_provider_id) = {
        let runner = state.lock().expect("Bilibili runner mutex");
        (runner.resources.clone(), runner.media_provider_id.clone())
    };
    let output = resources
        .create_cow_state_resource(
            &media_provider_id,
            "mutsuki.browser.snapshot.output",
            SNAPSHOT_SCHEMA,
            Vec::new(),
        )
        .map_err(|error| risk_control_failure(&task, "resource.create", error.to_string()))?;
    let snapshot_request = BrowserSnapshotRequest {
        url: format!("https://space.bilibili.com/{}/dynamic", request.uid),
        output_resource: output.clone(),
        wait_mode: BrowserWaitMode::Selector,
        selector: Some("body".into()),
        timeout_ms: risk_control.timeout_ms,
    };
    let outcome = ctx
        .call_raw(
            SNAPSHOT,
            serde_json::to_value(snapshot_request)
                .map_err(|error| risk_control_failure(&task, "request.encode", error))?,
        )
        .await
        .map_err(|error| risk_control_failure(&task, "snapshot.task", error.to_string()))?;
    if !matches!(outcome.into_outcome(), TaskOutcome::Completed { .. }) {
        return Err(risk_control_failure(
            &task,
            "snapshot.outcome",
            "Chromium snapshot task did not complete",
        ));
    }
    let latest = resources
        .open_resource_descriptor(output.ref_id.as_str())
        .map_err(|error| risk_control_failure(&task, "resource.open", error.to_string()))?;
    let bytes = resources
        .collect_read_plan(&ReadPlan {
            plan_id: format!("bilibili.risk-control.read.{}", task.task_id),
            resource: latest,
            operation: "collect".into(),
            args: Value::Null,
        })
        .map_err(|error| risk_control_failure(&task, "resource.read", error.to_string()))?;
    if bytes.len() > risk_control.max_response_bytes {
        return Err(risk_control_failure(
            &task,
            "response.oversized",
            format!(
                "Chromium response is {} bytes; maximum is {}",
                bytes.len(),
                risk_control.max_response_bytes
            ),
        ));
    }
    let snapshot: BrowserSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| risk_control_failure(&task, "response.decode", error))?;
    ensure_bilibili_domain(&snapshot.final_url)
        .map_err(|error| risk_control_failure(&task, "redirect.denied", error))?;
    let items = parse_dynamic_snapshot(&snapshot.html)
        .map_err(|error| risk_control_failure(&task, "dom.parse", error))?;
    let status = DomainEvent {
        event_id: format!("{}:risk-control", task.task_id),
        kind: RISK_CONTROL_STATUS_EVENT.into(),
        payload: json!({
            "task_id": task.task_id,
            "uid": request.uid,
            "risk_control_code": 352,
            "backend": "chromium",
            "status": "degraded",
            "fallback": "succeeded"
        }),
    };
    state
        .lock()
        .expect("Bilibili runner mutex")
        .finish_poll(
            &task,
            request,
            BilibiliPollKind::Dynamic,
            items,
            Some(status),
            false,
        )
        .map_err(RuntimeFailure::new)
}

fn poll_cursor_key(kind: &BilibiliPollKind, request: &PollRequest) -> String {
    format!("{kind:?}:{}:{}", request.uid, request.subscription_id)
}

pub(crate) fn decode<T: for<'de> Deserialize<'de>>(task: &Task) -> Result<T, RuntimeError> {
    serde_json::from_value(task.payload.clone().into())
        .map_err(|error| bili_error(task, BilibiliError::InvalidResponse(error.to_string())))
}
