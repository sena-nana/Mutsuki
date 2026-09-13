use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mutsuki_bot_protocol::{
    BOT_ACTIVE_DELIVERY_PROTOCOL_ID, BOT_INTERACTION_SESSION_PROTOCOL_ID,
    BOT_REPLY_DELIVERY_PROTOCOL_ID, BotActiveDeliveryCommand, BotInteractionCommand,
    BotReplyDeliveryCommand,
};
use mutsuki_runtime_contracts::{Task, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_service_runtime::{
    HostEventSource, HostEventSourceContext, HostEventSourceDescriptor, HostEventSourceFuture,
    HostEventSourceHealth,
};
use tokio::sync::{oneshot, watch};

pub const BOT_REPLY_DELIVERY_RECOVERY_SOURCE_ID: &str =
    "mutsuki.bot.delivery.reply.recovery.source";
pub const BOT_ACTIVE_DELIVERY_RECOVERY_SOURCE_ID: &str = "mutsuki.bot.delivery.recovery.source";
pub const BOT_INTERACTION_RECOVERY_SOURCE_ID: &str = "mutsuki.bot.interaction.recovery.source";

const DEFAULT_RECOVERY_INTERVAL: Duration = Duration::from_millis(250);

type RecoveryPayload = Arc<dyn Fn(u64) -> serde_json::Value + Send + Sync>;

#[derive(Clone, Default)]
struct RecoveryHealth {
    running: bool,
    last_error: Option<String>,
}

/// Periodic Host EventSource that submits a durable recover/ResumeDue command.
pub struct BotTaskRecoveryEventSource {
    descriptor: HostEventSourceDescriptor,
    interval: Duration,
    protocol_id: &'static str,
    task_id_prefix: &'static str,
    payload: RecoveryPayload,
    health: Arc<Mutex<RecoveryHealth>>,
    stop: Arc<Mutex<Option<watch::Sender<bool>>>>,
    stopped: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

pub type BotReplyDeliveryRecoveryEventSource = BotTaskRecoveryEventSource;

impl BotTaskRecoveryEventSource {
    #[must_use]
    pub fn reply_delivery(interval: Duration, plugin_id: impl Into<String>) -> Self {
        Self::new_with(
            BOT_REPLY_DELIVERY_RECOVERY_SOURCE_ID,
            plugin_id,
            interval,
            BOT_REPLY_DELIVERY_PROTOCOL_ID,
            "bot-reply-delivery-recovery",
            Arc::new(|now_unix_ms| {
                serde_json::to_value(BotReplyDeliveryCommand::ResumeDue { now_unix_ms })
                    .expect("reply ResumeDue serializes")
            }),
        )
    }

    #[must_use]
    pub fn active_delivery(interval: Duration, plugin_id: impl Into<String>) -> Self {
        Self::new_with(
            BOT_ACTIVE_DELIVERY_RECOVERY_SOURCE_ID,
            plugin_id,
            interval,
            BOT_ACTIVE_DELIVERY_PROTOCOL_ID,
            "bot-active-delivery-recovery",
            Arc::new(|now_unix_ms| {
                serde_json::to_value(BotActiveDeliveryCommand::ResumeDue { now_unix_ms })
                    .expect("active ResumeDue serializes")
            }),
        )
    }

    #[must_use]
    pub fn interaction(interval: Duration, plugin_id: impl Into<String>) -> Self {
        Self::new_with(
            BOT_INTERACTION_RECOVERY_SOURCE_ID,
            plugin_id,
            interval,
            BOT_INTERACTION_SESSION_PROTOCOL_ID,
            "bot-interaction-recovery",
            Arc::new(|now_unix_ms| {
                serde_json::to_value(BotInteractionCommand::Recover { now_unix_ms })
                    .expect("interaction Recover serializes")
            }),
        )
    }

    #[must_use]
    pub fn default_interval() -> Duration {
        DEFAULT_RECOVERY_INTERVAL
    }

    fn new_with(
        source_id: &'static str,
        plugin_id: impl Into<String>,
        interval: Duration,
        protocol_id: &'static str,
        task_id_prefix: &'static str,
        payload: RecoveryPayload,
    ) -> Self {
        Self {
            descriptor: HostEventSourceDescriptor::new(source_id, plugin_id),
            interval,
            protocol_id,
            task_id_prefix,
            payload,
            health: Arc::new(Mutex::new(RecoveryHealth::default())),
            stop: Arc::new(Mutex::new(None)),
            stopped: Arc::new(Mutex::new(None)),
        }
    }
}

impl HostEventSource for BotTaskRecoveryEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        let interval = self.interval;
        let protocol_id = self.protocol_id;
        let task_id_prefix = self.task_id_prefix;
        let payload = self.payload.clone();
        let health = self.health.clone();
        let (stop_tx, stop_rx) = watch::channel(false);
        *self.stop.lock().expect("recovery stop mutex") = Some(stop_tx);
        let (stopped_tx, stopped_rx) = oneshot::channel();
        *self.stopped.lock().expect("recovery stopped mutex") = Some(stopped_rx);
        Box::pin(async move {
            let result = run_recovery(
                interval,
                protocol_id,
                task_id_prefix,
                payload,
                health.clone(),
                ctx,
                stop_rx,
            )
            .await;
            health.lock().expect("recovery health mutex").running = false;
            let _ = stopped_tx.send(());
            result
        })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        let stop = self.stop.lock().expect("recovery stop mutex").take();
        let stopped = self.stopped.lock().expect("recovery stopped mutex").take();
        Box::pin(async move {
            if let Some(stop) = stop {
                let _ = stop.send(true);
            }
            if let Some(stopped) = stopped {
                let _ = stopped.await;
            }
            Ok(())
        })
    }

    fn health(&self) -> HostEventSourceHealth {
        let health = self.health.lock().expect("recovery health mutex").clone();
        match (health.running, health.last_error) {
            (true, None) => HostEventSourceHealth::Healthy,
            (true, Some(error)) => HostEventSourceHealth::Degraded(error),
            (false, Some(error)) => HostEventSourceHealth::Unhealthy(error),
            (false, None) => HostEventSourceHealth::Unhealthy("recovery source is stopped".into()),
        }
    }
}

async fn run_recovery(
    interval: Duration,
    protocol_id: &'static str,
    task_id_prefix: &'static str,
    payload: RecoveryPayload,
    health: Arc<Mutex<RecoveryHealth>>,
    ctx: HostEventSourceContext,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if interval.is_zero() {
        return Err("recovery interval must be greater than zero".into());
    }
    health.lock().expect("recovery health mutex").running = true;
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut inflight: Option<TaskHandle> = None;
    let mut sequence = 0_u64;
    let mut shutdown = ctx.shutdown.clone();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Some(handle) = inflight.take() {
                    match ctx.task_submitter.task_outcome(&handle) {
                        Ok(None) => {
                            inflight = Some(handle);
                            continue;
                        }
                        Ok(Some(TaskOutcome::Completed { .. })) => {
                            health.lock().expect("recovery health mutex").last_error = None;
                        }
                        Ok(Some(outcome)) => {
                            health.lock().expect("recovery health mutex").last_error =
                                Some(format!("recovery task failed: {outcome:?}"));
                        }
                        Err(error) => {
                            health.lock().expect("recovery health mutex").last_error =
                                Some(error.to_string());
                        }
                    }
                }
                sequence = sequence.wrapping_add(1);
                let task_id = format!("{task_id_prefix}:{sequence}");
                let task = Task::new(
                    task_id.clone(),
                    protocol_id,
                    payload(unix_ms()),
                );
                match ctx.task_submitter.submit_batch(TaskBatch::one(format!("batch:{task_id}"), task)) {
                    Ok(mut handles) if !handles.is_empty() => inflight = Some(handles.remove(0)),
                    Ok(_) => {
                        health.lock().expect("recovery health mutex").last_error =
                            Some("recovery returned no task handle".into());
                    }
                    Err(error) => {
                        health.lock().expect("recovery health mutex").last_error =
                            Some(error.to_string());
                    }
                }
            }
            _ = stop.changed() => break,
            _ = shutdown.cancelled() => break,
        }
    }
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
