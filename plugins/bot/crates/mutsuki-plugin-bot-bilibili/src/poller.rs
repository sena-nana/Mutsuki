//! The Bilibili subscription poll loop, independent of any Host.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mutsuki_bot_sdk::BotSubmissionGate;
use mutsuki_runtime_contracts::{Task, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_runtime_sdk::TaskSubmitter;
use tokio::sync::watch;

use crate::{BilibiliConfig, BilibiliPollKind, PollRequest, SharedBilibiliConfig};

/// What a Host reports about the poll loop.
#[derive(Clone, Debug, Default)]
pub struct BilibiliPollerHealth {
    pub running: bool,
    pub last_success: Option<Instant>,
    pub last_error: Option<String>,
}

/// Resolves once the flag is set; a flag that is already set resolves immediately.
async fn cancelled(flag: &mut watch::Receiver<bool>) {
    if !*flag.borrow() {
        let _ = flag.changed().await;
    }
}

/// Schedules poll tasks for every live subscription until `stop` or `host_shutdown` is set.
///
/// Each `(subscription, kind)` has at most one poll in flight; a failed or erroring poll backs
/// off exponentially per [`BilibiliConfig::retry`], and state for removed subscriptions is
/// dropped. Submissions go through [`BotSubmissionGate`], so only poll protocols and ingress can
/// be initiated here.
pub async fn run_bilibili_polling(
    config: SharedBilibiliConfig,
    health: Arc<Mutex<BilibiliPollerHealth>>,
    task_submitter: Arc<dyn TaskSubmitter>,
    mut stop: watch::Receiver<bool>,
    mut host_shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    health.lock().expect("Bilibili health mutex").running = true;
    // The polling source is a business initiation surface: it may submit its own
    // poll protocols (and ingress), never platform business protocols. Flow owns
    // rendering and delivery, so an unwired graph freezes pushes.
    let submitter: Arc<dyn TaskSubmitter> = Arc::new(BotSubmissionGate::new(task_submitter));
    let mut inflight: BTreeMap<(String, String), TaskHandle> = BTreeMap::new();
    let mut next_due: BTreeMap<(String, String), Instant> = BTreeMap::new();
    let mut failures: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut task_sequence = 0_u64;
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let snapshot = config.current();
                // Drop scheduling state of subscriptions/kinds that no longer
                // exist so removed subscriptions do not leak map entries.
                let live = |key: &(String, String)| {
                    snapshot.subscriptions.iter().any(|subscription| {
                        subscription.subscription_id == key.0
                            && subscription
                                .notifications
                                .iter()
                                .any(|kind| kind.protocol_id() == key.1)
                    })
                };
                inflight.retain(|key, _| live(key));
                next_due.retain(|key, _| live(key));
                failures.retain(|key, _| live(key));
                for subscription in &snapshot.subscriptions {
                    if subscription.paused { continue; }
                    for kind in &subscription.notifications {
                        let key = (
                            subscription.subscription_id.clone(),
                            kind.protocol_id().to_owned(),
                        );
                        if let Some(handle) = inflight.get(&key).cloned() {
                            match submitter.task_outcome(&handle) {
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
                                    *attempts = attempts.saturating_add(1).min(snapshot.retry.max_attempts);
                                    next_due.insert(key.clone(), Instant::now() + retry_delay(&snapshot, *attempts));
                                    continue;
                                }
                                Err(error) => {
                                    health.lock().expect("Bilibili health mutex").last_error = Some(error.to_string());
                                    let attempts = failures.entry(key.clone()).or_default();
                                    *attempts = attempts.saturating_add(1).min(snapshot.retry.max_attempts);
                                    next_due.insert(key.clone(), Instant::now() + retry_delay(&snapshot, *attempts));
                                    continue;
                                }
                            }
                        }
                        let now = Instant::now();
                        if next_due.get(&key).is_some_and(|due| *due > now) { continue; }
                        let request = PollRequest { subscription_id: subscription.subscription_id.clone(), uid: subscription.uid, target: subscription.target.clone(), outbound_binding: subscription.outbound_binding.clone() };
                        task_sequence = task_sequence.wrapping_add(1);
                        let task_id = format!("bilibili:{:?}:{}:{task_sequence}", kind, subscription.uid);
                        let task = Task::new(task_id.clone(), kind.protocol_id(), serde_json::to_value(request)?);
                        match submitter.submit_batch(TaskBatch::one(format!("batch:{task_id}"), task)) {
                            Ok(mut handles) if !handles.is_empty() => { inflight.insert(key.clone(), handles.remove(0)); }
                            Ok(_) => { health.lock().expect("Bilibili health mutex").last_error = Some("poll submit returned no handle".into()); }
                            Err(error) => { health.lock().expect("Bilibili health mutex").last_error = Some(error.to_string()); }
                        }
                        next_due.insert(key, now + interval_for(&snapshot, kind));
                    }
                }
            }
            _ = stop.changed() => break,
            () = cancelled(&mut host_shutdown) => break,
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
