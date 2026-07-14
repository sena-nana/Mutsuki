use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mutsuki_plugin_bot_bilibili::{
    BilibiliConfig, BilibiliPollKind, PLUGIN_ID, PollRequest, SharedBilibiliCredential,
};
use mutsuki_runtime_contracts::{Task, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_service_runtime::{
    HostEventSource, HostEventSourceContext, HostEventSourceDescriptor, HostEventSourceFuture,
    HostEventSourceHealth,
};
use tokio::sync::{oneshot, watch};

#[derive(Clone, Debug, Default)]
struct PollingHealth {
    running: bool,
    last_success: Option<Instant>,
    last_error: Option<String>,
}

pub struct BilibiliPollingEventSource {
    descriptor: HostEventSourceDescriptor,
    config: BilibiliConfig,
    credential: SharedBilibiliCredential,
    health: Arc<Mutex<PollingHealth>>,
    stop: Arc<Mutex<Option<watch::Sender<bool>>>>,
    stopped: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl BilibiliPollingEventSource {
    pub fn new(config: BilibiliConfig, credential: SharedBilibiliCredential) -> Self {
        Self {
            descriptor: HostEventSourceDescriptor::new(
                "mutsuki.bot.bilibili.polling.source",
                PLUGIN_ID,
            )
            .require_secret(config.cookie_secret_key.clone()),
            config,
            credential,
            health: Arc::new(Mutex::new(PollingHealth::default())),
            stop: Arc::new(Mutex::new(None)),
            stopped: Arc::new(Mutex::new(None)),
        }
    }
}

impl HostEventSource for BilibiliPollingEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        let Some(cookie) = ctx.config.secret(&self.config.cookie_secret_key) else {
            let key = self.config.cookie_secret_key.clone();
            return Box::pin(async move { Err(format!("missing Bilibili secret {key}").into()) });
        };
        self.credential.set(cookie);
        let config = self.config.clone();
        let health = self.health.clone();
        let credential = self.credential.clone();
        let (stop_tx, stop_rx) = watch::channel(false);
        *self.stop.lock().expect("Bilibili stop mutex") = Some(stop_tx);
        let (stopped_tx, stopped_rx) = oneshot::channel();
        *self.stopped.lock().expect("Bilibili stopped mutex") = Some(stopped_rx);
        Box::pin(async move {
            let result = run_polling(config, health.clone(), ctx, stop_rx).await;
            credential.clear();
            let _ = stopped_tx.send(());
            result
        })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        let stop = self.stop.lock().expect("Bilibili stop mutex").take();
        let stopped = self.stopped.lock().expect("Bilibili stopped mutex").take();
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
        let health = self.health.lock().expect("Bilibili health mutex").clone();
        if health.running && health.last_error.is_none() {
            HostEventSourceHealth::Healthy
        } else if health.running {
            HostEventSourceHealth::Degraded(health.last_error.unwrap_or_default())
        } else {
            HostEventSourceHealth::Unhealthy(
                health
                    .last_error
                    .unwrap_or_else(|| "polling stopped".into()),
            )
        }
    }
}

async fn run_polling(
    config: BilibiliConfig,
    health: Arc<Mutex<PollingHealth>>,
    ctx: HostEventSourceContext,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    health.lock().expect("Bilibili health mutex").running = true;
    let mut inflight: BTreeMap<(u64, String, String), TaskHandle> = BTreeMap::new();
    let mut next_due: BTreeMap<(u64, String, String), Instant> = BTreeMap::new();
    let mut failures: BTreeMap<(u64, String, String), u32> = BTreeMap::new();
    let mut task_sequence = 0_u64;
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    let mut host_shutdown = ctx.shutdown.clone();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                for subscription in &config.subscriptions {
                    for kind in &subscription.notifications {
                        let key = (
                            subscription.uid,
                            kind.protocol_id().to_owned(),
                            subscription.outbound_binding.clone(),
                        );
                        if let Some(handle) = inflight.get(&key).cloned() {
                            match ctx.task_submitter.task_outcome(&handle) {
                                Ok(None) => continue,
                                Ok(Some(TaskOutcome::Completed { .. })) => {
                                    inflight.remove(&key);
                                    let mut health = health.lock().expect("Bilibili health mutex");
                                    health.last_success = Some(Instant::now());
                                    health.last_error = None;
                                    failures.remove(&key);
                                }
                                Ok(Some(outcome)) => {
                                    inflight.remove(&key);
                                    health.lock().expect("Bilibili health mutex").last_error = Some(format!("poll task failed: {outcome:?}"));
                                    let attempts = failures.entry(key.clone()).or_default();
                                    *attempts = attempts.saturating_add(1).min(config.retry.max_attempts);
                                    next_due.insert(key.clone(), Instant::now() + retry_delay(&config, *attempts));
                                    continue;
                                }
                                Err(error) => {
                                    health.lock().expect("Bilibili health mutex").last_error = Some(error.to_string());
                                    let attempts = failures.entry(key.clone()).or_default();
                                    *attempts = attempts.saturating_add(1).min(config.retry.max_attempts);
                                    next_due.insert(key.clone(), Instant::now() + retry_delay(&config, *attempts));
                                    continue;
                                }
                            }
                        }
                        let now = Instant::now();
                        if next_due.get(&key).is_some_and(|due| *due > now) { continue; }
                        let request = PollRequest { uid: subscription.uid, target: subscription.target.clone(), outbound_binding: subscription.outbound_binding.clone() };
                        task_sequence = task_sequence.wrapping_add(1);
                        let task_id = format!("bilibili:{:?}:{}:{task_sequence}", kind, subscription.uid);
                        let task = Task::new(task_id.clone(), kind.protocol_id(), serde_json::to_value(request)?);
                        match ctx.task_submitter.submit_batch(TaskBatch::one(format!("batch:{task_id}"), task)) {
                            Ok(mut handles) if !handles.is_empty() => { inflight.insert(key.clone(), handles.remove(0)); }
                            Ok(_) => { health.lock().expect("Bilibili health mutex").last_error = Some("poll submit returned no handle".into()); }
                            Err(error) => { health.lock().expect("Bilibili health mutex").last_error = Some(error.to_string()); }
                        }
                        next_due.insert(key, now + interval_for(&config, kind));
                    }
                }
            }
            _ = stop.changed() => break,
            _ = host_shutdown.cancelled() => break,
        }
    }
    health.lock().expect("Bilibili health mutex").running = false;
    Ok(())
}

fn interval_for(config: &BilibiliConfig, kind: &BilibiliPollKind) -> Duration {
    Duration::from_millis(match kind {
        BilibiliPollKind::Live => config.live_interval_ms,
        BilibiliPollKind::Dynamic => config.dynamic_interval_ms,
        BilibiliPollKind::Video => config.video_interval_ms,
    })
}

fn retry_delay(config: &BilibiliConfig, attempts: u32) -> Duration {
    let multiplier = 1_u64
        .checked_shl(attempts.saturating_sub(1))
        .unwrap_or(u64::MAX);
    Duration::from_millis(
        config
            .retry
            .initial_backoff_ms
            .saturating_mul(multiplier)
            .min(config.retry.max_backoff_ms),
    )
}
