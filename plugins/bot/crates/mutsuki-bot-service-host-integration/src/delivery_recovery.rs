use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mutsuki_bot_protocol::{BOT_REPLY_DELIVERY_PROTOCOL_ID, BotReplyDeliveryCommand};
use mutsuki_plugin_bot_delivery::BOT_REPLY_DELIVERY_PLUGIN_ID;
use mutsuki_runtime_contracts::{Task, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_service_runtime::{
    HostEventSource, HostEventSourceContext, HostEventSourceDescriptor, HostEventSourceFuture,
    HostEventSourceHealth,
};
use tokio::sync::{oneshot, watch};

pub const BOT_REPLY_DELIVERY_RECOVERY_SOURCE_ID: &str =
    "mutsuki.bot.delivery.reply.recovery.source";

#[derive(Clone, Default)]
struct RecoveryHealth {
    running: bool,
    last_error: Option<String>,
}

pub struct BotReplyDeliveryRecoveryEventSource {
    descriptor: HostEventSourceDescriptor,
    interval: Duration,
    health: Arc<Mutex<RecoveryHealth>>,
    stop: Arc<Mutex<Option<watch::Sender<bool>>>>,
    stopped: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl BotReplyDeliveryRecoveryEventSource {
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self::for_plugin(interval, BOT_REPLY_DELIVERY_PLUGIN_ID)
    }

    #[must_use]
    pub fn for_plugin(interval: Duration, plugin_id: impl Into<String>) -> Self {
        Self {
            descriptor: HostEventSourceDescriptor::new(
                BOT_REPLY_DELIVERY_RECOVERY_SOURCE_ID,
                plugin_id,
            ),
            interval,
            health: Arc::new(Mutex::new(RecoveryHealth::default())),
            stop: Arc::new(Mutex::new(None)),
            stopped: Arc::new(Mutex::new(None)),
        }
    }
}

impl HostEventSource for BotReplyDeliveryRecoveryEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        let interval = self.interval;
        let health = self.health.clone();
        let (stop_tx, stop_rx) = watch::channel(false);
        *self.stop.lock().expect("reply delivery stop mutex") = Some(stop_tx);
        let (stopped_tx, stopped_rx) = oneshot::channel();
        *self.stopped.lock().expect("reply delivery stopped mutex") = Some(stopped_rx);
        Box::pin(async move {
            let result = run_recovery(interval, health.clone(), ctx, stop_rx).await;
            health.lock().expect("reply delivery health mutex").running = false;
            let _ = stopped_tx.send(());
            result
        })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        let stop = self.stop.lock().expect("reply delivery stop mutex").take();
        let stopped = self
            .stopped
            .lock()
            .expect("reply delivery stopped mutex")
            .take();
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
        let health = self
            .health
            .lock()
            .expect("reply delivery health mutex")
            .clone();
        match (health.running, health.last_error) {
            (true, None) => HostEventSourceHealth::Healthy,
            (true, Some(error)) => HostEventSourceHealth::Degraded(error),
            (false, Some(error)) => HostEventSourceHealth::Unhealthy(error),
            (false, None) => {
                HostEventSourceHealth::Unhealthy("reply delivery recovery is stopped".into())
            }
        }
    }
}

async fn run_recovery(
    interval: Duration,
    health: Arc<Mutex<RecoveryHealth>>,
    ctx: HostEventSourceContext,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if interval.is_zero() {
        return Err("reply delivery recovery interval must be greater than zero".into());
    }
    health.lock().expect("reply delivery health mutex").running = true;
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
                            health.lock().expect("reply delivery health mutex").last_error = None;
                        }
                        Ok(Some(outcome)) => {
                            health.lock().expect("reply delivery health mutex").last_error =
                                Some(format!("reply delivery recovery task failed: {outcome:?}"));
                        }
                        Err(error) => {
                            health.lock().expect("reply delivery health mutex").last_error =
                                Some(error.to_string());
                        }
                    }
                }
                sequence = sequence.wrapping_add(1);
                let task_id = format!("bot-reply-delivery-recovery:{sequence}");
                let task = Task::new(
                    task_id.clone(),
                    BOT_REPLY_DELIVERY_PROTOCOL_ID,
                    serde_json::to_value(BotReplyDeliveryCommand::ResumeDue {
                        now_unix_ms: unix_ms(),
                    })?,
                );
                match ctx.task_submitter.submit_batch(TaskBatch::one(format!("batch:{task_id}"), task)) {
                    Ok(mut handles) if !handles.is_empty() => inflight = Some(handles.remove(0)),
                    Ok(_) => {
                        health.lock().expect("reply delivery health mutex").last_error =
                            Some("reply delivery recovery returned no task handle".into());
                    }
                    Err(error) => {
                        health.lock().expect("reply delivery health mutex").last_error =
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
