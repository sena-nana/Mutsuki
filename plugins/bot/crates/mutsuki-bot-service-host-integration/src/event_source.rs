use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use mutsuki_service_runtime::{
    HostEventSource, HostEventSourceContext, HostEventSourceDescriptor, HostEventSourceFuture,
    HostEventSourceHealth,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use mutsuki_bot_management::{
    QQ_GATEWAY_ERROR_IDENTIFY_REJECTED, QQ_GATEWAY_ERROR_IDENTIFY_TIMEOUT,
    QQ_GATEWAY_ERROR_OPERATOR_RECONNECT, QQ_GATEWAY_ERROR_SERVER_RECONNECT,
    QQ_GATEWAY_ERROR_SESSION_EXPIRED, QQ_GATEWAY_ERROR_SESSION_INVALID,
};
use mutsuki_bot_protocol::{
    BOT_FLOW_INGRESS_PROTOCOL_ID, BotEvent, BotEventKind, BotTarget, BotUser, apply_bot_self_sent,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    GatewayAction, GatewayFrame, HttpMethod, QQBOT_ADAPTER_PLUGIN_ID, QqAuthManager, QqBotConfig,
    QqGatewayPump, QqOpenApiError, QqOpenApiTransport, ReqwestQqHttpClient, SharedQqCredentials,
    adapter::{
        qq_bot_disconnected_event, qq_gateway_frame_to_bot_event, qq_group_name_from_info,
        qq_self_user,
    },
    flow_envelope, qq_group_info_path, session_summary, validate_gateway_url,
};
use mutsuki_runtime_contracts::{Task, TaskPayload};

pub const QQBOT_GATEWAY_SOURCE_ID: &str = "mutsuki.bot.adapter.qqbot.gateway.source";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QqGatewayHealthSnapshot {
    pub connected: bool,
    pub identified: bool,
    pub last_heartbeat_unix_ms: Option<u128>,
    pub last_ack_unix_ms: Option<u128>,
    pub last_event_unix_ms: Option<u128>,
    pub reconnect_count: u64,
    pub last_error: Option<String>,
    pub last_error_code: Option<String>,
    pub started_at_unix_ms: Option<u128>,
    pub connected_since_unix_ms: Option<u128>,
    pub self_user: Option<BotUser>,
}

#[derive(Clone)]
pub struct QqGatewayHealthHandle {
    inner: Arc<Mutex<QqGatewayHealthSnapshot>>,
    /// Bumped on every snapshot mutation that console frontends render, so
    /// status consumers can react to transitions without polling.
    changes: Arc<watch::Sender<u64>>,
}

impl QqGatewayHealthHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(QqGatewayHealthSnapshot {
                started_at_unix_ms: unix_ms(),
                ..QqGatewayHealthSnapshot::default()
            })),
            changes: Arc::new(watch::Sender::new(0)),
        }
    }

    pub fn snapshot(&self) -> QqGatewayHealthSnapshot {
        self.inner.lock().expect("QQBot health mutex").clone()
    }

    /// Subscribes to gateway status transitions; each wakeup means the
    /// snapshot changed and consumers should re-read [`Self::snapshot`].
    pub fn status_changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub fn set_self_user(&self, user: BotUser) {
        self.update(|snapshot| {
            snapshot.self_user = Some(merge_self_user(snapshot.self_user.clone(), user));
        });
    }

    /// Applies one snapshot mutation and wakes status watchers.
    fn update(&self, apply: impl FnOnce(&mut QqGatewayHealthSnapshot)) {
        let mut snapshot = self.inner.lock().expect("QQBot health mutex");
        apply(&mut snapshot);
        drop(snapshot);
        self.changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}

#[derive(Clone, Default)]
pub struct QqGatewayControlHandle {
    reconnect: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

#[derive(Clone, Default)]
pub struct QqInboundObserveHandle {
    observer: Arc<Mutex<Option<Arc<dyn Fn(BotEvent) + Send + Sync>>>>,
    titles: Arc<Mutex<Option<Arc<dyn Fn(String, String) + Send + Sync>>>>,
}

impl QqInboundObserveHandle {
    pub fn set(&self, observer: Arc<dyn Fn(BotEvent) + Send + Sync>) {
        *self.observer.lock().expect("QQ inbound observer mutex") = Some(observer);
    }

    pub fn set_title_sink(&self, sink: Arc<dyn Fn(String, String) + Send + Sync>) {
        *self.titles.lock().expect("QQ inbound title mutex") = Some(sink);
    }

    fn notify(&self, event: BotEvent) {
        if let Some(observer) = self
            .observer
            .lock()
            .expect("QQ inbound observer mutex")
            .clone()
        {
            observer(event);
        }
    }

    fn notify_title(&self, group_id: &str, title: &str) {
        if let Some(sink) = self.titles.lock().expect("QQ inbound title mutex").clone() {
            sink(group_id.to_owned(), title.to_owned());
        }
    }
}

impl QqGatewayControlHandle {
    /// Requests a reconnect from the live QQ Gateway EventSource.
    pub fn reconnect(&self) -> Result<(), String> {
        let sender = self
            .reconnect
            .lock()
            .expect("QQBot reconnect mutex")
            .clone()
            .ok_or_else(|| "QQ Gateway 当前未运行".to_string())?;
        match sender.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err("QQ Gateway 生命周期任务已停止".into())
            }
        }
    }
}

pub struct QqGatewayEventSource {
    descriptor: HostEventSourceDescriptor,
    config: QqBotConfig,
    credentials: SharedQqCredentials,
    auth: QqAuthManager,
    health: QqGatewayHealthHandle,
    control: QqGatewayControlHandle,
    inbound: QqInboundObserveHandle,
    stop: Arc<Mutex<Option<watch::Sender<bool>>>>,
    stopped: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

impl QqGatewayEventSource {
    pub fn new(config: QqBotConfig, credentials: SharedQqCredentials, auth: QqAuthManager) -> Self {
        let instance_id = format!("qqbot-gateway:{}", config.account_id);
        let source_id = format!(
            "{QQBOT_GATEWAY_SOURCE_ID}:{}",
            safe_source_id(&config.account_id)
        );
        Self {
            descriptor: HostEventSourceDescriptor::new(source_id, QQBOT_ADAPTER_PLUGIN_ID)
                .with_instance_id(instance_id)
                .require_secret(config.client_secret_key.clone()),
            config,
            credentials,
            auth,
            health: QqGatewayHealthHandle::new(),
            control: QqGatewayControlHandle::default(),
            inbound: QqInboundObserveHandle::default(),
            stop: Arc::new(Mutex::new(None)),
            stopped: Arc::new(Mutex::new(None)),
            abort: Arc::new(Mutex::new(None)),
        }
    }

    pub fn health_handle(&self) -> QqGatewayHealthHandle {
        self.health.clone()
    }

    pub fn control_handle(&self) -> QqGatewayControlHandle {
        self.control.clone()
    }

    #[must_use]
    pub fn inbound_handle(&self) -> QqInboundObserveHandle {
        self.inbound.clone()
    }
}

impl HostEventSource for QqGatewayEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        let config = self.config.clone();
        let credentials = self.credentials.clone();
        let auth = self.auth.clone();
        let health = self.health.clone();
        let control = self.control.clone();
        let inbound = self.inbound.clone();
        let (stop_tx, stop_rx) = watch::channel(false);
        *self.stop.lock().expect("QQBot stop mutex") = Some(stop_tx);
        if let Err(error) = config.validate() {
            return Box::pin(async move { Err(source_error(error)) });
        }
        let Some(secret) = ctx
            .config
            .secret(&config.client_secret_key)
            .filter(|secret| !secret.is_empty())
        else {
            let message = format!(
                "missing Host secret {} for QQBot account {}",
                config.client_secret_key, config.account_id
            );
            return Box::pin(async move { Err(source_error(message)) });
        };
        let http = match ReqwestQqHttpClient::new(&config) {
            Ok(http) => http,
            Err(error) => return Box::pin(async move { Err(source_error(error)) }),
        };
        let (reconnect_tx, reconnect_rx) = mpsc::channel(1);
        *control.reconnect.lock().expect("QQBot reconnect mutex") = Some(reconnect_tx);
        credentials.set_client_secret(secret);
        let cleanup_credentials = credentials.clone();
        let cleanup_auth = auth.clone();
        let api = Arc::new(Mutex::new(QqOpenApiTransport::new_with_auth(
            config.clone(),
            Box::new(http),
            Arc::new(credentials.clone()),
            auth.clone(),
        )));
        let (stopped_tx, stopped_rx) = oneshot::channel();
        *self.stopped.lock().expect("QQBot stopped mutex") = Some(stopped_rx);
        let task = tokio::spawn(async move {
            let _stopped = NotifyStoppedOnDrop(Some(stopped_tx));

            {
                let _credentials = GatewayCredentialLease {
                    credentials: cleanup_credentials,
                    auth: cleanup_auth,
                };
                run_gateway(config, api, health, inbound, ctx, stop_rx, reconnect_rx).await
            }
        });
        *self.abort.lock().expect("QQBot abort mutex") = Some(task.abort_handle());
        Box::pin(async move {
            let outcome = task
                .await
                .map_err(|error| source_error(format!("QQBot Gateway task failed: {error}")))?;
            *control.reconnect.lock().expect("QQBot reconnect mutex") = None;
            outcome
        })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        let sender = self.stop.lock().expect("QQBot stop mutex").take();
        let stopped = self.stopped.lock().expect("QQBot stopped mutex").take();
        let abort = self.abort.lock().expect("QQBot abort mutex").take();
        let control = self.control.clone();
        Box::pin(async move {
            let mut abort = AbortHandleOnDrop(abort);
            if let Some(sender) = sender {
                let _ = sender.send(true);
            }
            if let Some(stopped) = stopped {
                let _ = stopped.await;
            }
            abort.0 = None;
            *control.reconnect.lock().expect("QQBot reconnect mutex") = None;
            Ok(())
        })
    }

    fn health(&self) -> HostEventSourceHealth {
        let health = self.health.snapshot();
        if health.connected && health.identified {
            HostEventSourceHealth::Healthy
        } else if health.connected {
            HostEventSourceHealth::Degraded(
                health
                    .last_error
                    .unwrap_or_else(|| "QQBot Gateway is connected but not identified".into()),
            )
        } else {
            HostEventSourceHealth::Unhealthy(
                health
                    .last_error
                    .unwrap_or_else(|| "QQBot Gateway is disconnected".into()),
            )
        }
    }
}

async fn run_gateway(
    config: QqBotConfig,
    api: Arc<Mutex<QqOpenApiTransport>>,
    health: QqGatewayHealthHandle,
    inbound: QqInboundObserveHandle,
    ctx: HostEventSourceContext,
    mut local_stop: watch::Receiver<bool>,
    mut reconnect: mpsc::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut host_stop = ctx.shutdown.clone();
    let mut pump = QqGatewayPump::with_account(&config.account_id, config.gateway_dedup_window);
    let mut reconnect_attempt = 0_u32;
    let group_names = GroupNameCache::default();
    loop {
        if host_stop.is_cancelled() || *local_stop.borrow() {
            mark_stopped(&health);
            return Ok(());
        }
        let outcome = match run_connection(
            &config,
            api.clone(),
            &mut pump,
            GatewayConnectionContext {
                health: &health,
                inbound: &inbound,
                group_names: &group_names,
                ctx: &ctx,
                reconnect_attempt: &mut reconnect_attempt,
                host_stop: &mut host_stop,
                local_stop: &mut local_stop,
                reconnect: &mut reconnect,
            },
        )
        .await
        {
            Err(GatewayFailure::Recoverable(detail)) => {
                Ok(ConnectionEnd::Reconnect(ReconnectReason::new(detail)))
            }
            outcome => outcome,
        };
        match outcome {
            Ok(ConnectionEnd::Shutdown) => {
                mark_stopped(&health);
                return Ok(());
            }
            Ok(ConnectionEnd::Reconnect(reason)) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                mark_reconnect(&health, &reason);
                ctx.events
                    .log("warn", &format!("QQBot Gateway reconnect: {reason}"), None);
                let delay = reconnect_delay(&config, reconnect_attempt);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = host_stop.cancelled() => {
                        mark_stopped(&health);
                        return Ok(());
                    }
                    _ = local_stop.changed() => {
                        mark_stopped(&health);
                        return Ok(());
                    }
                    signal = reconnect.recv() => {
                        if signal.is_none() {
                            mark_stopped(&health);
                            return Ok(());
                        }
                    }
                }
            }
            Err(GatewayFailure::Recoverable(_)) => unreachable!("mapped to reconnect"),
            Err(GatewayFailure::RateLimited(reason)) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                mark_reconnect(&health, &ReconnectReason::new(reason.clone()));
                ctx.events.log(
                    "warn",
                    &format!("QQBot Gateway rate limited: {reason}"),
                    None,
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(config.gateway_rate_limit_delay_ms)) => {}
                    _ = host_stop.cancelled() => {
                        mark_stopped(&health);
                        return Ok(());
                    }
                    _ = local_stop.changed() => {
                        mark_stopped(&health);
                        return Ok(());
                    }
                    signal = reconnect.recv() => {
                        if signal.is_none() {
                            mark_stopped(&health);
                            return Ok(());
                        }
                    }
                }
            }
            Err(GatewayFailure::Fatal(reason)) => {
                mark_error(&health, &reason);
                return Err(source_error(reason));
            }
        }
    }
}

struct GatewayConnectionContext<'a> {
    health: &'a QqGatewayHealthHandle,
    inbound: &'a QqInboundObserveHandle,
    group_names: &'a GroupNameCache,
    ctx: &'a HostEventSourceContext,
    reconnect_attempt: &'a mut u32,
    host_stop: &'a mut mutsuki_service_runtime::HostShutdownToken,
    local_stop: &'a mut watch::Receiver<bool>,
    reconnect: &'a mut mpsc::Receiver<()>,
}

async fn run_connection(
    config: &QqBotConfig,
    api: Arc<Mutex<QqOpenApiTransport>>,
    pump: &mut QqGatewayPump,
    lifecycle: GatewayConnectionContext<'_>,
) -> Result<ConnectionEnd, GatewayFailure> {
    let GatewayConnectionContext {
        health,
        inbound,
        group_names,
        ctx,
        reconnect_attempt,
        host_stop,
        local_stop,
        reconnect,
    } = lifecycle;
    let (gateway_url, access_token, self_user) = gateway_credentials(config, api.clone()).await?;
    if let Some(user) = self_user {
        health.set_self_user(user);
    }
    let selected_url = pump.resume_url().unwrap_or(&gateway_url);
    validate_gateway_url(selected_url, config.allow_insecure_transport).map_err(fatal_failure)?;
    let connect = tokio_tungstenite::connect_async(selected_url);
    let (mut websocket, _) = tokio::select! {
        result = tokio::time::timeout(Duration::from_millis(config.connect_timeout_ms), connect) => {
            result
                .map_err(|_| GatewayFailure::Recoverable("Gateway connect timed out".into()))?
                .map_err(recoverable_failure)?
        }
        _ = host_stop.cancelled() => return Ok(ConnectionEnd::Shutdown),
        _ = local_stop.changed() => return Ok(ConnectionEnd::Shutdown),
        signal = reconnect.recv() => return Ok(connection_end_from_reconnect_signal(signal)),
    };
    mark_connected(health);

    let hello = tokio::select! {
        result = tokio::time::timeout(
            Duration::from_millis(config.gateway_hello_timeout_ms),
            websocket.next(),
        ) => result
            .map_err(|_| GatewayFailure::Recoverable("Gateway HELLO timed out".into()))?
            .ok_or_else(|| GatewayFailure::Recoverable("Gateway closed before HELLO".into()))?
            .map_err(recoverable_failure)?,
        _ = host_stop.cancelled() => {
            let _ = websocket.close(None).await;
            return Ok(ConnectionEnd::Shutdown);
        }
        _ = local_stop.changed() => {
            let _ = websocket.close(None).await;
            return Ok(ConnectionEnd::Shutdown);
        }
        signal = reconnect.recv() => {
            let _ = websocket.close(None).await;
            return Ok(connection_end_from_reconnect_signal(signal));
        }
    };
    let hello = message_json(hello)?;
    let hello_frame: GatewayFrame = serde_json::from_value(hello)
        .map_err(|error| GatewayFailure::Fatal(format!("invalid HELLO: {error}")))?;
    if hello_frame.op != 10 {
        return Err(GatewayFailure::Fatal(format!(
            "expected HELLO opcode 10, received {}",
            hello_frame.op
        )));
    }
    let heartbeat_ms = hello_frame
        .d
        .get("heartbeat_interval")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| GatewayFailure::Fatal("HELLO missing heartbeat_interval".into()))?;
    pump.handle_frame(hello_frame, 0)
        .map_err(GatewayFailure::Fatal)?;
    send_auth_action(&mut websocket, config, pump, &access_token).await?;
    let identify_timeout = Duration::from_millis(config.gateway_hello_timeout_ms);
    let mut awaiting_ready_since = Some(Instant::now());

    let (mut sink, mut stream) = websocket.split();
    let (incoming_tx, mut incoming_rx) = mpsc::channel(config.gateway_queue_capacity);
    let _reader = AbortOnDrop(tokio::spawn(async move {
        while let Some(message) = stream.next().await {
            if incoming_tx
                .send(message.map_err(|error| error.to_string()))
                .await
                .is_err()
            {
                break;
            }
        }
    }));
    let mut heartbeat = tokio::time::interval(Duration::from_millis(heartbeat_ms));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let ack_timeout = Duration::from_millis(config.gateway_ack_timeout_ms);
    let mut awaiting_ack_since: Option<Instant> = None;
    let mut cached_heartbeat_seq: Option<Option<u64>> = None;
    let mut cached_heartbeat_text = String::new();

    let end = loop {
        tokio::select! {
            _ = host_stop.cancelled() => break ConnectionEnd::Shutdown,
            _ = local_stop.changed() => break ConnectionEnd::Shutdown,
            signal = reconnect.recv() => break connection_end_from_reconnect_signal(signal),
            _ = wait_deadline(awaiting_ready_since, identify_timeout) => {
                if awaiting_ready_since.is_some() {
                    break ConnectionEnd::Reconnect(ReconnectReason::classified(
                        QQ_GATEWAY_ERROR_IDENTIFY_TIMEOUT,
                        "Identify timed out waiting for READY",
                    ));
                }
            }
            _ = wait_deadline(awaiting_ack_since, ack_timeout) => {
                if awaiting_ack_since.is_some() {
                    break ConnectionEnd::Reconnect(ReconnectReason::new(
                        "heartbeat ACK timed out",
                    ));
                }
            }
            _ = heartbeat.tick() => {
                if awaiting_ack_since.is_some() {
                    continue;
                }
                let sequence = pump.last_sequence();
                if cached_heartbeat_seq != Some(sequence) {
                    cached_heartbeat_text = pump.heartbeat_text();
                    cached_heartbeat_seq = Some(sequence);
                }
                sink.send(Message::Text(cached_heartbeat_text.clone().into()))
                    .await
                    .map_err(recoverable_failure)?;
                awaiting_ack_since = Some(Instant::now());
                health.inner.lock().expect("QQBot health mutex").last_heartbeat_unix_ms = unix_ms();
            }
            incoming = incoming_rx.recv() => {
                let Some(incoming) = incoming else {
                    break ConnectionEnd::Reconnect(ReconnectReason::new(
                        "Gateway receive stream ended",
                    ));
                };
                let message = incoming.map_err(GatewayFailure::Recoverable)?;
                match message {
                    Message::Ping(payload) => {
                        sink.send(Message::Pong(payload)).await
                            .map_err(recoverable_failure)?;
                    }
                    Message::Close(frame) => {
                        let _ = sink.send(Message::Close(frame.clone())).await;
                        let reason = frame
                            .as_ref()
                            .map(|frame| format!("Gateway close {} {}", u16::from(frame.code), frame.reason))
                            .unwrap_or_else(|| "Gateway closed".into());
                        let Some(code) = frame.as_ref().map(|frame| u16::from(frame.code)) else {
                            break ConnectionEnd::Reconnect(ReconnectReason::new(reason));
                        };
                        match classify_gateway_close(code) {
                            GatewayCloseDisposition::Permanent => {
                                return Err(GatewayFailure::Fatal(reason));
                            }
                            GatewayCloseDisposition::RefreshToken => {
                                api.lock().expect("QQBot API mutex").invalidate_token();
                                break ConnectionEnd::Reconnect(ReconnectReason::new(reason));
                            }
                            GatewayCloseDisposition::Reidentify => {
                                pump.clear_session();
                                api.lock().expect("QQBot API mutex").invalidate_token();
                                break ConnectionEnd::Reconnect(reconnect_reason_from_close(
                                    code, reason,
                                ));
                            }
                            GatewayCloseDisposition::RateLimited => {
                                return Err(GatewayFailure::RateLimited(reason));
                            }
                            GatewayCloseDisposition::Resume => {
                                break ConnectionEnd::Reconnect(reconnect_reason_from_close(
                                    code, reason,
                                ));
                            }
                        }
                    }
                    Message::Text(ref text)
                        if gateway_opcode(text.as_ref()) == Some(11) =>
                    {
                        // Heartbeat ACK is the idle hot path: skip full GatewayFrame decode
                        // and the pump action queue.
                        awaiting_ack_since = None;
                        health.inner.lock().expect("QQBot health mutex").last_ack_unix_ms =
                            unix_ms();
                    }
                    Message::Text(_) | Message::Binary(_) => {
                        let raw = message_json(message)?;
                        let frame: GatewayFrame = serde_json::from_value(raw)
                            .map_err(|error| GatewayFailure::Recoverable(format!("invalid Gateway frame: {error}")))?;
                        let event_type = frame.t.clone().unwrap_or_else(|| "none".into());
                        let sequence = frame.s;
                        let task = pump.handle_frame(frame.clone(), 0)
                            .map_err(GatewayFailure::Recoverable)?;
                        if matches!(frame.t.as_deref(), Some("READY" | "RESUMED")) {
                            health.update(|snapshot| {
                                snapshot.identified = true;
                                snapshot.last_error = None;
                                snapshot.last_error_code = None;
                            });
                            awaiting_ready_since = None;
                            *reconnect_attempt = 0;
                        }
                        if frame.op == 9
                            && !(frame.d.as_bool().unwrap_or(false) && pump.session_id().is_some())
                        {
                            while pump.pop_action().is_some() {}
                            break ConnectionEnd::Reconnect(ReconnectReason::classified(
                                QQ_GATEWAY_ERROR_IDENTIFY_REJECTED,
                                "Identify or Resume rejected",
                            ));
                        }
                        while let Some(action) = pump.pop_action() {
                            match action {
                                GatewayAction::Identify => {
                                    sink.send(Message::Text(QqGatewayPump::identify_frame(config, &access_token).to_string().into()))
                                        .await.map_err(recoverable_failure)?;
                                    awaiting_ready_since = Some(Instant::now());
                                }
                                GatewayAction::Resume => {
                                    let frame = pump.resume_frame(&access_token).map_err(GatewayFailure::Recoverable)?;
                                    sink.send(Message::Text(frame.to_string().into())).await
                                        .map_err(recoverable_failure)?;
                                    awaiting_ready_since = Some(Instant::now());
                                }
                                GatewayAction::Heartbeat(_) => {
                                    let sequence = pump.last_sequence();
                                    if cached_heartbeat_seq != Some(sequence) {
                                        cached_heartbeat_text = pump.heartbeat_text();
                                        cached_heartbeat_seq = Some(sequence);
                                    }
                                    sink.send(Message::Text(cached_heartbeat_text.clone().into())).await
                                        .map_err(recoverable_failure)?;
                                }
                                GatewayAction::Reconnect => break,
                                GatewayAction::AckHeartbeat => {
                                    awaiting_ack_since = None;
                                    health.inner.lock().expect("QQBot health mutex").last_ack_unix_ms = unix_ms();
                                }
                                GatewayAction::UnknownOpcode(opcode) => ctx.events.log(
                                    "warn",
                                    &format!("unknown QQBot Gateway opcode {opcode}"),
                                    frame.id.as_deref(),
                                ),
                                GatewayAction::UnknownEvent(kind) => ctx.events.log(
                                    "warn",
                                    &format!("unknown QQBot Gateway event type {kind}"),
                                    frame.id.as_deref(),
                                ),
                                GatewayAction::DispatchTask(_) => {}
                            }
                        }
                        if frame.op == 7 {
                            break ConnectionEnd::Reconnect(ReconnectReason::classified(
                                QQ_GATEWAY_ERROR_SERVER_RECONNECT,
                                "server requested reconnect",
                            ));
                        }
                        if let Some(task) = task {
                            let correlation_id = task.correlation_id.clone();
                            let self_id = health
                                .inner
                                .lock()
                                .expect("QQBot health mutex")
                                .self_user
                                .as_ref()
                                .map(|user| user.user_id.clone());
                            let mut mapped = qq_gateway_frame_to_bot_event(
                                &config.account_id,
                                &config.app_id,
                                frame.clone(),
                            )
                            .ok();
                            let skip_ingress = mapped.as_mut().is_some_and(|event| {
                                apply_bot_self_sent(event, self_id.as_deref())
                            });
                            if !skip_ingress
                                && let Err(error) = ctx.task_submitter.submit_one(task)
                            {
                                pump.forget_dispatch(&frame);
                                return Err(recoverable_failure(error));
                            }
                            if let Some(event) = mapped {
                                if event.kind == BotEventKind::BotConnected
                                    && let Some(actor) = event.actor.clone()
                                {
                                    health.set_self_user(actor);
                                }
                                notify_group_event(
                                    inbound,
                                    api.clone(),
                                    group_names,
                                    event,
                                );
                            }
                            let mut snapshot = health.inner.lock().expect("QQBot health mutex");
                            snapshot.last_event_unix_ms = unix_ms();
                            tracing::info!(
                                account_id = %config.account_id,
                                session = %session_summary(pump.session_id()),
                                event_type,
                                sequence = sequence.unwrap_or_default(),
                                correlation_id = correlation_id.as_deref().unwrap_or(""),
                                "QQBot Gateway frame submitted"
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    let _ = sink.send(Message::Close(None)).await;
    mark_disconnected(health);
    submit_bot_disconnected_ingress(ctx, config);
    Ok(end)
}

fn submit_bot_disconnected_ingress(ctx: &HostEventSourceContext, config: &QqBotConfig) {
    let Ok(envelope) = flow_envelope(qq_bot_disconnected_event(&config.account_id), None, None)
    else {
        return;
    };
    let task = Task::new(
        format!(
            "mutsuki.bot.flow.ingress:disconnected:{}",
            config.account_id
        ),
        BOT_FLOW_INGRESS_PROTOCOL_ID,
        TaskPayload::from_local(envelope),
    );
    if let Err(error) = ctx.task_submitter.submit_one(task) {
        ctx.events.log(
            "warn",
            &format!("failed to submit QQBot disconnected ingress: {error}"),
            None,
        );
    }
}

async fn send_auth_action<S>(
    websocket: &mut tokio_tungstenite::WebSocketStream<S>,
    config: &QqBotConfig,
    pump: &mut QqGatewayPump,
    access_token: &str,
) -> Result<(), GatewayFailure>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let action = pump.pop_action().unwrap_or(GatewayAction::Identify);
    let frame = match action {
        GatewayAction::Resume => pump
            .resume_frame(access_token)
            .unwrap_or_else(|_| QqGatewayPump::identify_frame(config, access_token)),
        _ => QqGatewayPump::identify_frame(config, access_token),
    };
    websocket
        .send(Message::Text(frame.to_string().into()))
        .await
        .map_err(recoverable_failure)
}

async fn gateway_credentials(
    config: &QqBotConfig,
    api: Arc<Mutex<QqOpenApiTransport>>,
) -> Result<(String, String, Option<BotUser>), GatewayFailure> {
    let app_id = config.app_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut api = api.lock().expect("QQBot API mutex");
        let account = api.execute_json(HttpMethod::Get, "/users/@me".into(), Value::Null)?;
        account
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| QqOpenApiError::InvalidResponse("account.id".into()))?;
        let self_user = qq_self_user(&account, &app_id);
        let gateway = api.execute_json(HttpMethod::Get, "/gateway/bot".into(), Value::Null)?;
        let url = gateway
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| QqOpenApiError::InvalidResponse("gateway.url".into()))?
            .to_owned();
        let token = api.access_token()?;
        Ok::<_, QqOpenApiError>((url, token, self_user))
    })
    .await
    .map_err(recoverable_failure)?;
    result.map_err(|error| classify_api_error(config, error))
}

#[derive(Clone, Default)]
struct GroupNameCache {
    inner: Arc<Mutex<HashMap<String, GroupNameEntry>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GroupNameEntry {
    Name(String),
    Denied,
    Pending,
}

impl GroupNameCache {
    fn cached_name(&self, group_id: &str) -> Option<String> {
        match self
            .inner
            .lock()
            .expect("QQ group name cache")
            .get(group_id)
        {
            Some(GroupNameEntry::Name(name)) => Some(name.clone()),
            _ => None,
        }
    }

    fn start_fetch(&self, group_id: &str) -> bool {
        let mut cache = self.inner.lock().expect("QQ group name cache");
        match cache.get(group_id) {
            Some(GroupNameEntry::Name(_) | GroupNameEntry::Denied | GroupNameEntry::Pending) => {
                false
            }
            None => {
                cache.insert(group_id.to_owned(), GroupNameEntry::Pending);
                true
            }
        }
    }

    fn store_name(&self, group_id: &str, name: String) {
        self.inner
            .lock()
            .expect("QQ group name cache")
            .insert(group_id.to_owned(), GroupNameEntry::Name(name));
    }

    fn store_denied(&self, group_id: &str) {
        self.inner
            .lock()
            .expect("QQ group name cache")
            .insert(group_id.to_owned(), GroupNameEntry::Denied);
    }

    fn clear_pending(&self, group_id: &str) {
        let mut cache = self.inner.lock().expect("QQ group name cache");
        if matches!(cache.get(group_id), Some(GroupNameEntry::Pending)) {
            cache.remove(group_id);
        }
    }
}

fn notify_group_event(
    inbound: &QqInboundObserveHandle,
    api: Arc<Mutex<QqOpenApiTransport>>,
    cache: &GroupNameCache,
    mut event: BotEvent,
) {
    let Some(group_id) = group_openid_from_event(&event).map(str::to_owned) else {
        inbound.notify(event);
        return;
    };
    if let Some(name) = event
        .ext
        .get("qqbot.group_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    {
        cache.store_name(&group_id, name);
        inbound.notify(event);
        return;
    }
    if let Some(name) = cache.cached_name(&group_id) {
        event.ext.insert("qqbot.group_name".into(), json!(name));
        inbound.notify(event);
        return;
    }
    inbound.notify(event);
    if cache.start_fetch(&group_id) {
        schedule_group_name_fetch(api, cache.clone(), inbound.clone(), group_id);
    }
}

fn group_openid_from_event(event: &BotEvent) -> Option<&str> {
    match &event.target {
        BotTarget::Group { group_id } if !group_id.is_empty() && group_id != "unknown_group" => {
            Some(group_id)
        }
        _ => None,
    }
}

fn schedule_group_name_fetch(
    api: Arc<Mutex<QqOpenApiTransport>>,
    cache: GroupNameCache,
    inbound: QqInboundObserveHandle,
    group_id: String,
) {
    tokio::spawn(async move {
        let path = qq_group_info_path(&group_id);
        let fetched = tokio::task::spawn_blocking(move || {
            let mut api = api.lock().expect("QQBot API mutex");
            api.execute_json(HttpMethod::Get, path, Value::Null)
        })
        .await;
        match fetched {
            Ok(Ok(body)) => {
                if let Some(name) = qq_group_name_from_info(&body) {
                    cache.store_name(&group_id, name.clone());
                    inbound.notify_title(&group_id, &name);
                } else {
                    cache.store_denied(&group_id);
                }
            }
            Ok(Err(error)) if error.retryable() => cache.clear_pending(&group_id),
            Ok(Err(_)) => cache.store_denied(&group_id),
            Err(_) => cache.clear_pending(&group_id),
        }
    });
}

fn classify_api_error(_config: &QqBotConfig, error: QqOpenApiError) -> GatewayFailure {
    match error {
        QqOpenApiError::CredentialsUnavailable
        | QqOpenApiError::InvalidPayload(_)
        | QqOpenApiError::InvalidResponse(_)
        | QqOpenApiError::ResponseTooLarge { .. }
        | QqOpenApiError::HttpStatus {
            status: 400 | 401 | 403 | 404,
            ..
        } => GatewayFailure::Fatal(error.redacted_message()),
        _ => GatewayFailure::Recoverable(error.redacted_message()),
    }
}

fn message_json(message: Message) -> Result<Value, GatewayFailure> {
    match message {
        Message::Text(text) => serde_json::from_str(text.as_ref()),
        Message::Binary(bytes) => serde_json::from_slice(bytes.as_ref()),
        other => {
            return Err(GatewayFailure::Recoverable(format!(
                "expected JSON Gateway frame, received {other:?}"
            )));
        }
    }
    .map_err(recoverable_failure)
}

fn gateway_opcode(text: &str) -> Option<u64> {
    #[derive(serde::Deserialize)]
    struct OpOnly {
        op: u64,
    }
    serde_json::from_str::<OpOnly>(text)
        .ok()
        .map(|frame| frame.op)
}

async fn wait_deadline(since: Option<Instant>, timeout: Duration) {
    match since {
        Some(sent) => {
            let elapsed = sent.elapsed();
            if elapsed < timeout {
                tokio::time::sleep(timeout - elapsed).await;
            }
        }
        None => std::future::pending::<()>().await,
    }
}

fn reconnect_delay(config: &QqBotConfig, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(20);
    let base = config
        .reconnect_initial_delay_ms
        .saturating_mul(1_u64 << exponent)
        .min(config.reconnect_max_delay_ms);
    let jitter = if config.reconnect_jitter_ms == 0 {
        0
    } else {
        fastrand::u64(0..=config.reconnect_jitter_ms)
    };
    Duration::from_millis(
        base.saturating_add(jitter)
            .min(config.reconnect_max_delay_ms),
    )
}

fn merge_self_user(existing: Option<BotUser>, incoming: BotUser) -> BotUser {
    let Some(current) = existing else {
        return incoming;
    };
    BotUser {
        user_id: if current.user_id.is_empty() {
            incoming.user_id
        } else {
            current.user_id
        },
        display_name: current.display_name.or(incoming.display_name),
        avatar_url: current.avatar_url.or(incoming.avatar_url),
    }
}

fn mark_connected(health: &QqGatewayHealthHandle) {
    health.update(|snapshot| {
        let now = unix_ms();
        if snapshot.started_at_unix_ms.is_none() {
            snapshot.started_at_unix_ms = now;
        }
        snapshot.connected_since_unix_ms = now;
        snapshot.connected = true;
        snapshot.identified = false;
        snapshot.last_error = None;
        snapshot.last_error_code = None;
    });
}

fn mark_disconnected(health: &QqGatewayHealthHandle) {
    health.update(|snapshot| {
        snapshot.connected = false;
        snapshot.identified = false;
        snapshot.connected_since_unix_ms = None;
    });
}

fn mark_reconnect(health: &QqGatewayHealthHandle, reason: &ReconnectReason) {
    health.update(|snapshot| {
        snapshot.connected = false;
        snapshot.identified = false;
        snapshot.connected_since_unix_ms = None;
        snapshot.reconnect_count = snapshot.reconnect_count.saturating_add(1);
        snapshot.last_error = Some(reason.detail.clone());
        snapshot.last_error_code = reason.code.map(str::to_owned);
    });
}

fn mark_error(health: &QqGatewayHealthHandle, error: &str) {
    health.update(|snapshot| {
        snapshot.connected = false;
        snapshot.identified = false;
        snapshot.connected_since_unix_ms = None;
        snapshot.last_error = Some(error.into());
        snapshot.last_error_code = None;
    });
}

fn mark_stopped(health: &QqGatewayHealthHandle) {
    health.update(|snapshot| {
        snapshot.connected = false;
        snapshot.identified = false;
    });
}

fn unix_ms() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GatewayCloseDisposition {
    Permanent,
    RefreshToken,
    Reidentify,
    RateLimited,
    Resume,
}

fn classify_gateway_close(code: u16) -> GatewayCloseDisposition {
    // Official QQ Gateway close semantics: 4006/4007 and 4900-4913 discard the
    // session and Identify; 4009 and ordinary disconnects Resume; 4008 is rate
    // limiting; 4001/4002/4010-4014 cannot retry; 4914/4915 are permanent
    // account state. 4004 is a defensive auth-refresh path.
    match code {
        4001 | 4002 | 4010..=4014 | 4914 | 4915 => GatewayCloseDisposition::Permanent,
        4004 => GatewayCloseDisposition::RefreshToken,
        4006 | 4007 | 4900..=4913 => GatewayCloseDisposition::Reidentify,
        4008 => GatewayCloseDisposition::RateLimited,
        4009 => GatewayCloseDisposition::Resume,
        _ => GatewayCloseDisposition::Resume,
    }
}

fn safe_source_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(48)
        .collect()
}

fn reconnect_reason_from_close(code: u16, detail: String) -> ReconnectReason {
    match code {
        4006 | 4007 => ReconnectReason::classified(QQ_GATEWAY_ERROR_SESSION_INVALID, detail),
        4009 => ReconnectReason::classified(QQ_GATEWAY_ERROR_SESSION_EXPIRED, detail),
        _ => ReconnectReason::new(detail),
    }
}

fn connection_end_from_reconnect_signal(signal: Option<()>) -> ConnectionEnd {
    match signal {
        Some(()) => ConnectionEnd::Reconnect(ReconnectReason::classified(
            QQ_GATEWAY_ERROR_OPERATOR_RECONNECT,
            "operator requested reconnect",
        )),
        None => ConnectionEnd::Shutdown,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReconnectReason {
    code: Option<&'static str>,
    detail: String,
}

impl std::fmt::Display for ReconnectReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(code) => write!(formatter, "{code} ({})", self.detail),
            None => formatter.write_str(&self.detail),
        }
    }
}

impl ReconnectReason {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            code: None,
            detail: detail.into(),
        }
    }

    fn classified(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            detail: detail.into(),
        }
    }
}

enum ConnectionEnd {
    Shutdown,
    Reconnect(ReconnectReason),
}

enum GatewayFailure {
    Recoverable(String),
    RateLimited(String),
    Fatal(String),
}

fn recoverable_failure(error: impl std::fmt::Display) -> GatewayFailure {
    GatewayFailure::Recoverable(mutsuki_plugin_bot_adapter_qqbot::adapter::redact_urls(
        &error.to_string(),
    ))
}

fn fatal_failure(error: impl std::fmt::Display) -> GatewayFailure {
    GatewayFailure::Fatal(mutsuki_plugin_bot_adapter_qqbot::adapter::redact_urls(
        &error.to_string(),
    ))
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct AbortHandleOnDrop(Option<tokio::task::AbortHandle>);

impl Drop for AbortHandleOnDrop {
    fn drop(&mut self) {
        if let Some(abort) = self.0.take() {
            abort.abort();
        }
    }
}

struct GatewayCredentialLease {
    credentials: SharedQqCredentials,
    auth: QqAuthManager,
}

struct NotifyStoppedOnDrop(Option<oneshot::Sender<()>>);

impl Drop for NotifyStoppedOnDrop {
    fn drop(&mut self) {
        if let Some(stopped) = self.0.take() {
            let _ = stopped.send(());
        }
    }
}

impl Drop for GatewayCredentialLease {
    fn drop(&mut self) {
        self.auth.invalidate();
        self.credentials.clear();
    }
}

fn source_error(
    error: impl std::fmt::Display,
) -> Box<dyn std::error::Error + Send + Sync + 'static> {
    Box::new(std::io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use mutsuki_plugin_bot_adapter_qqbot::QqCredentialProvider;

    use super::*;

    #[test]
    fn gateway_close_codes_distinguish_qq_recovery_and_permanent_rejection() {
        assert_eq!(
            classify_gateway_close(1000),
            GatewayCloseDisposition::Resume
        );
        assert_eq!(
            classify_gateway_close(4004),
            GatewayCloseDisposition::RefreshToken
        );
        assert_eq!(
            classify_gateway_close(4009),
            GatewayCloseDisposition::Resume
        );
        for code in [4006, 4007, 4900, 4913] {
            assert_eq!(
                classify_gateway_close(code),
                GatewayCloseDisposition::Reidentify
            );
        }
        assert_eq!(
            classify_gateway_close(4008),
            GatewayCloseDisposition::RateLimited
        );
        for code in [4001, 4002, 4010, 4011, 4012, 4013, 4014, 4914, 4915] {
            assert_eq!(
                classify_gateway_close(code),
                GatewayCloseDisposition::Permanent
            );
        }
        for code in [1001, 1006, 4000] {
            assert_eq!(
                classify_gateway_close(code),
                GatewayCloseDisposition::Resume
            );
        }
    }

    #[test]
    fn gateway_health_reports_disconnected_degraded_connected_and_reconnect_state() {
        let source = QqGatewayEventSource::new(
            QqBotConfig::new("main", "APP_ID"),
            SharedQqCredentials::default(),
            QqAuthManager::new(),
        );
        assert!(matches!(
            source.health(),
            HostEventSourceHealth::Unhealthy(_)
        ));

        mark_connected(&source.health);
        assert!(matches!(
            source.health(),
            HostEventSourceHealth::Degraded(_)
        ));

        source
            .health
            .inner
            .lock()
            .expect("QQBot health mutex")
            .identified = true;
        assert_eq!(source.health(), HostEventSourceHealth::Healthy);

        mark_reconnect(
            &source.health,
            &ReconnectReason::new("heartbeat ACK timed out"),
        );
        assert!(matches!(
            source.health(),
            HostEventSourceHealth::Unhealthy(ref reason)
                if reason == "heartbeat ACK timed out"
        ));
        assert_eq!(source.health.snapshot().reconnect_count, 1);
    }

    #[tokio::test]
    async fn gateway_health_transitions_wakeup_status_watchers() {
        let source = QqGatewayEventSource::new(
            QqBotConfig::new("main", "APP_ID"),
            SharedQqCredentials::default(),
            QqAuthManager::new(),
        );
        let mut status = source.health.status_changes();

        mark_connected(&source.health);
        status.changed().await.expect("connect notifies watchers");
        assert_eq!(*status.borrow(), 1);

        mark_reconnect(
            &source.health,
            &ReconnectReason::new("heartbeat ACK timed out"),
        );
        status.changed().await.expect("reconnect notifies watchers");
        assert_eq!(*status.borrow(), 2);

        source.health.set_self_user(BotUser {
            user_id: "BOT_OPENID".into(),
            display_name: Some("mutsuki".into()),
            avatar_url: None,
        });
        status.changed().await.expect("self user notifies watchers");
        assert_eq!(*status.borrow(), 3);
    }

    #[test]
    fn reconnect_signal_treats_channel_close_as_shutdown() {
        assert!(matches!(
            connection_end_from_reconnect_signal(None),
            ConnectionEnd::Shutdown
        ));
        match connection_end_from_reconnect_signal(Some(())) {
            ConnectionEnd::Reconnect(reason) => {
                assert_eq!(reason.code, Some(QQ_GATEWAY_ERROR_OPERATOR_RECONNECT));
            }
            ConnectionEnd::Shutdown => panic!("operator reconnect must not shut down"),
        }
    }

    #[test]
    fn close_codes_map_session_invalid_without_labeling_4009_as_duplicate_login() {
        assert_eq!(
            reconnect_reason_from_close(4006, "Gateway close 4006".into()).code,
            Some(QQ_GATEWAY_ERROR_SESSION_INVALID)
        );
        assert_eq!(
            reconnect_reason_from_close(4007, "Gateway close 4007".into()).code,
            Some(QQ_GATEWAY_ERROR_SESSION_INVALID)
        );
        assert_eq!(
            reconnect_reason_from_close(4009, "Gateway close 4009".into()).code,
            Some(QQ_GATEWAY_ERROR_SESSION_EXPIRED)
        );
        assert_eq!(
            reconnect_reason_from_close(1001, "Gateway close 1001".into()).code,
            None
        );
    }

    #[tokio::test]
    async fn shutdown_abort_guard_cancels_the_owned_gateway_task() {
        let task = tokio::spawn(std::future::pending::<()>());
        let guard = AbortHandleOnDrop(Some(task.abort_handle()));

        drop(guard);

        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn gateway_credential_lease_clears_secret_when_dropped() {
        let credentials = SharedQqCredentials::default();
        credentials.set_client_secret("TEST_ONLY_SECRET".into());
        let lease = GatewayCredentialLease {
            credentials: credentials.clone(),
            auth: QqAuthManager::new(),
        };
        assert_eq!(credentials.client_secret().unwrap(), "TEST_ONLY_SECRET");

        drop(lease);

        assert!(matches!(
            credentials.client_secret(),
            Err(QqOpenApiError::CredentialsUnavailable)
        ));
    }

    #[test]
    fn self_user_keeps_name_and_avatar_when_later_update_is_empty() {
        let source = QqGatewayEventSource::new(
            QqBotConfig::new("main", "APP_ID"),
            SharedQqCredentials::default(),
            QqAuthManager::new(),
        );
        source.health.set_self_user(BotUser {
            user_id: "BOT_OPENID".into(),
            display_name: Some("mutsuki".into()),
            avatar_url: Some("https://example.test/bot.png".into()),
        });
        source.health.set_self_user(BotUser {
            user_id: "OTHER".into(),
            display_name: None,
            avatar_url: None,
        });
        let cached = source.health.snapshot().self_user.unwrap();
        assert_eq!(cached.user_id, "BOT_OPENID");
        assert_eq!(cached.display_name.as_deref(), Some("mutsuki"));
        assert_eq!(
            cached.avatar_url.as_deref(),
            Some("https://example.test/bot.png")
        );
    }

    #[test]
    fn group_name_cache_fetches_once_and_keeps_resolved_or_denied() {
        let cache = GroupNameCache::default();
        assert!(cache.start_fetch("group-1"));
        assert!(!cache.start_fetch("group-1"));
        cache.store_name("group-1", "读书分享会".into());
        assert_eq!(cache.cached_name("group-1").as_deref(), Some("读书分享会"));
        assert!(!cache.start_fetch("group-1"));

        assert!(cache.start_fetch("group-2"));
        cache.clear_pending("group-2");
        assert!(cache.start_fetch("group-2"));
        cache.store_denied("group-2");
        assert!(!cache.start_fetch("group-2"));
        assert_eq!(cache.cached_name("group-2"), None);
    }
}
