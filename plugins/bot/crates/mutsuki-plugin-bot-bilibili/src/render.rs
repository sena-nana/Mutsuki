//! Card and QR rendering through the image renderer protocols, and the cover media they need.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_link_parser::ResolvedLinkCard;
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotExtMap, BotFlowEventEnvelope, BotMessage, BotNodeInvocation,
    BotNodeOutput, BotNodeResult, BotTarget, MessageSegment,
};
use mutsuki_protocol_image::{
    CARD_RENDER, CardGradient, CardLayout, CardRenderRequest, ImageRenderResponse, QR_RENDER,
    QrRenderRequest, Rgba,
};
use mutsuki_runtime_contracts::{
    ReadPlan, RunnerResult, RuntimeError, Task, TaskBatch, TaskOutcome,
};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, ResourceRegistryGateway, RuntimeClientRef, RuntimeFailure, RuntimeResult,
    TaskHandleFuture,
};
use serde_json::Value;
use url::Url;

use crate::commands::command_outbound_result;
use crate::error::bili_error;
use crate::runner::BilibiliRunner;
use crate::{
    BilibiliError, BilibiliNotification, BilibiliPollKind, BilibiliQrRenderer, BilibiliTransport,
    MAX_MEDIA_BYTES, SqliteBilibiliRepository,
};

pub struct RuntimeBilibiliQrRenderer {
    client: RuntimeClientRef,
    resources: Arc<dyn ResourceRegistryGateway>,
    next_task: AtomicU64,
}

impl RuntimeBilibiliQrRenderer {
    #[must_use]
    pub fn new(client: RuntimeClientRef, resources: Arc<dyn ResourceRegistryGateway>) -> Self {
        Self {
            client,
            resources,
            next_task: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl BilibiliQrRenderer for RuntimeBilibiliQrRenderer {
    async fn render_qr(&self, content: &str) -> Result<Vec<u8>, BilibiliError> {
        let sequence = self.next_task.fetch_add(1, Ordering::Relaxed) + 1;
        let epoch_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let task_id = format!("bilibili.web.qr.{epoch_nanos}.{sequence}");
        let task = Task::new(
            task_id.clone(),
            QR_RENDER,
            serde_json::to_value(QrRenderRequest {
                content: content.into(),
                min_dimensions: 256,
            })
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?,
        );
        let handle = self
            .client
            .submit_batch(TaskBatch::one(format!("{task_id}.batch"), task))
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| BilibiliError::Transport("QR render returned no task handle".into()))?;
        let outcome = TaskHandleFuture::new(self.client.clone(), handle)
            .await
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        let response: ImageRenderResponse = match outcome {
            TaskOutcome::Completed {
                output: Some(output),
                ..
            } => serde_json::from_value(output)
                .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?,
            TaskOutcome::Completed { output: None, .. } => {
                return Err(BilibiliError::InvalidResponse(
                    "QR renderer completed without output".into(),
                ));
            }
            outcome => {
                return Err(BilibiliError::Transport(format!(
                    "QR renderer did not complete: {outcome:?}"
                )));
            }
        };
        self.resources
            .collect_read_plan(&ReadPlan {
                plan_id: format!("{task_id}.read"),
                resource: response.resource,
                operation: "collect".into(),
                args: Value::Null,
            })
            .map_err(|error| BilibiliError::Transport(error.to_string()))
    }
}

pub(crate) struct PreparedCards {
    pub(crate) result: RunnerResult,
    pub(crate) cards: Vec<PreparedCard>,
    pub(crate) cursor_update: Option<(Arc<SqliteBilibiliRepository>, String, String)>,
    pub(crate) cooldown_update: Option<(Arc<SqliteBilibiliRepository>, String, u64)>,
}

pub(crate) struct PreparedCard {
    pub(crate) target: BotTarget,
    pub(crate) request: CardRenderRequest,
    pub(crate) route: CardRoute,
}

pub(crate) enum CardRoute {
    Notify(String),
    Command(Option<String>),
}

pub(crate) struct CardMedia {
    pub(crate) transport: Arc<Mutex<Box<dyn BilibiliTransport>>>,
    pub(crate) resources: Arc<dyn ResourceRegistryGateway>,
    pub(crate) media_provider_id: String,
}

impl CardMedia {
    pub(crate) fn resolve(&self, url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        self.transport
            .lock()
            .expect("Bilibili transport mutex")
            .resolve(url)
    }

    pub(crate) fn prepare_card_request(
        &self,
        card: ResolvedLinkCard,
        task: &Task,
        layout: CardLayout,
        kicker: &str,
        live: bool,
    ) -> Result<CardRenderRequest, RuntimeError> {
        let cover = if let Some(image_url) = card.image_url {
            let bytes = self
                .transport
                .lock()
                .expect("Bilibili transport mutex")
                .download(&image_url, MAX_MEDIA_BYTES)
                .map_err(|error| bili_error(task, error))?;
            let resource = self
                .resources
                .create_blob_resource(
                    &self.media_provider_id,
                    "mutsuki.bot.image.original.v1",
                    bytes,
                )
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
            Some(resource)
        } else {
            None
        };
        Ok(CardRenderRequest {
            brand: "哔哩哔哩".into(),
            title: card.title,
            description: card.description,
            url: card.url,
            cover,
            fallback_gradient: CardGradient {
                start: Rgba {
                    red: 251,
                    green: 114,
                    blue: 153,
                    alpha: 255,
                },
                end: Rgba {
                    red: 0,
                    green: 174,
                    blue: 236,
                    alpha: 255,
                },
            },
            layout,
            kicker: kicker.into(),
            live,
            ..CardRenderRequest::default()
        })
    }
}

fn layout_for_poll(kind: BilibiliPollKind) -> (CardLayout, &'static str, bool) {
    match kind {
        BilibiliPollKind::Live => (CardLayout::Row, "直播", true),
        BilibiliPollKind::Dynamic => (CardLayout::Feed, "动态", false),
        BilibiliPollKind::Video => (CardLayout::Media, "投稿", false),
    }
}

fn poll_description(kind: BilibiliPollKind) -> &'static str {
    match kind {
        BilibiliPollKind::Live => "直播状态更新",
        BilibiliPollKind::Dynamic => "发布了新动态",
        BilibiliPollKind::Video => "发布了新投稿",
    }
}

pub(crate) fn layout_for_url(url: &str) -> (CardLayout, &'static str, bool) {
    let parsed = Url::parse(url).ok();
    let host = parsed
        .as_ref()
        .and_then(|value| value.host_str())
        .unwrap_or_default();
    let path = parsed.as_ref().map_or("", Url::path);
    if host.contains("live.bilibili") {
        (CardLayout::Hero, "直播", true)
    } else if host == "t.bilibili.com" || path.contains("/opus") || path.contains("/dynamic") {
        (CardLayout::Feed, "动态", false)
    } else {
        (CardLayout::Media, "投稿", false)
    }
}

pub(crate) async fn render_qr_child(
    ctx: &AsyncRunnerContext,
    task: &Task,
    content: String,
) -> RuntimeResult<ImageRenderResponse> {
    let outcome = ctx
        .call_raw(
            QR_RENDER,
            serde_json::to_value(QrRenderRequest {
                content,
                min_dimensions: 256,
            })
            .map_err(|error| {
                RuntimeFailure::new(bili_error(
                    task,
                    BilibiliError::InvalidResponse(error.to_string()),
                ))
            })?,
        )
        .await?;
    decode_render_outcome(task, outcome, "QR")
}

pub(crate) async fn render_cards(
    ctx: &AsyncRunnerContext,
    task: &Task,
    mut prepared: PreparedCards,
) -> RuntimeResult<RunnerResult> {
    for (index, card) in prepared.cards.into_iter().enumerate() {
        let url = card.request.url.clone();
        let outcome = ctx
            .call_raw(
                CARD_RENDER,
                serde_json::to_value(card.request).map_err(|error| {
                    RuntimeFailure::new(bili_error(
                        task,
                        BilibiliError::InvalidResponse(error.to_string()),
                    ))
                })?,
            )
            .await?;
        let rendered = decode_render_outcome(task, outcome, "card")?;
        let message = BotMessage {
            message_id: None,
            target: card.target,
            sender: None,
            segments: vec![
                MessageSegment::Image {
                    resource: rendered.resource,
                },
                MessageSegment::Text { text: url },
            ],
            reply_to: None,
            time_ms: None,
            ext: BotExtMap::new(),
        };
        match card.route {
            CardRoute::Notify(binding) => {
                prepared
                    .result
                    .tasks
                    .push(outbound_task(task, message, &binding, index));
            }
            CardRoute::Command(binding) => {
                let binding = binding.as_deref().filter(|value| !value.is_empty());
                prepared
                    .result
                    .tasks
                    .extend(command_outbound_result(task, message, binding).tasks);
            }
        }
    }
    if let Some((repository, key, head)) = prepared.cursor_update {
        repository.set_cursor(&key, &head).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::Transport(error.to_string()),
            ))
        })?;
    }
    if let Some((repository, key, now_ms)) = prepared.cooldown_update {
        repository.record_cooldown(&key, now_ms).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::Transport(error.to_string()),
            ))
        })?;
    }
    Ok(prepared.result)
}

pub(crate) async fn run_notification_card(
    ctx: &AsyncRunnerContext,
    task: &Task,
    state: Arc<Mutex<BilibiliRunner>>,
    invocation: BotNodeInvocation,
) -> RuntimeResult<RunnerResult> {
    let notification: BilibiliNotification =
        serde::Deserialize::deserialize(&invocation.input.payload.value).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::InvalidResponse(error.to_string()),
            ))
        })?;
    let request = {
        let media = state.lock().expect("Bilibili runner mutex").card_media();
        let (layout, kicker, live) = layout_for_poll(notification.kind);
        media
            .prepare_card_request(
                ResolvedLinkCard {
                    url: notification.url.clone(),
                    title: notification.title.clone(),
                    description: poll_description(notification.kind).into(),
                    image_url: notification.image_url.clone(),
                },
                task,
                layout,
                kicker,
                live,
            )
            .map_err(RuntimeFailure::new)?
    };
    let outcome = ctx
        .call_raw(
            CARD_RENDER,
            serde_json::to_value(request).map_err(|error| {
                RuntimeFailure::new(bili_error(
                    task,
                    BilibiliError::InvalidResponse(error.to_string()),
                ))
            })?,
        )
        .await?;
    let rendered = decode_render_outcome(task, outcome, "card")?;
    let message = BotMessage {
        message_id: None,
        target: notification.target,
        sender: None,
        segments: vec![
            MessageSegment::Image {
                resource: rendered.resource,
            },
            MessageSegment::Text {
                text: notification.url,
            },
        ],
        reply_to: None,
        time_ms: None,
        ext: BotExtMap::new(),
    };
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![BotNodeOutput {
                port_id: "message".into(),
                event: BotFlowEventEnvelope::outbound_message(
                    &invocation.input,
                    format!("{}:message", invocation.input.event_id),
                    serde_json::to_value(message).expect("BotMessage serializes"),
                ),
            }],
            metadata: Default::default(),
        })
        .expect("BotNodeResult serializes"),
    );
    Ok(result)
}

fn decode_render_outcome(
    task: &Task,
    outcome: impl Into<TaskOutcome>,
    kind: &str,
) -> RuntimeResult<ImageRenderResponse> {
    match outcome.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output).map_err(|error| {
            RuntimeFailure::new(bili_error(
                task,
                BilibiliError::InvalidResponse(error.to_string()),
            ))
        }),
        TaskOutcome::Completed { output: None, .. } => Err(RuntimeFailure::new(bili_error(
            task,
            BilibiliError::InvalidResponse(format!("{kind} renderer completed without output")),
        ))),
        _ => Err(RuntimeFailure::new(bili_error(
            task,
            BilibiliError::Transport(format!("{kind} renderer child task failed")),
        ))),
    }
}

fn outbound_task(parent: &Task, message: BotMessage, binding: &str, index: usize) -> Task {
    let mut task = Task::new(
        format!("{}:notify:{index}", parent.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).expect("BotMessage serializes"),
    );
    task.target_binding_id = Some(binding.into());
    task.trace_id = parent.trace_id.clone();
    task.correlation_id = parent
        .correlation_id
        .clone()
        .or_else(|| Some(parent.task_id.to_string()));
    task
}
