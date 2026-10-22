//! Bounded actor mailboxes and control/data priority arbitration.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use mutsuki_runtime_contracts::{TaskHandle, TaskStatus};
use mutsuki_runtime_core::{RunnerCompletion, RuntimeResult};

use super::lifecycle::TaskWaitReply;
use crate::async_executor::AsyncExecutorEvent;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::worker::{WorkerExited, WorkerStarted};

// Mailbox messages own structured Host commands; boxing would add allocation to every command.
#[allow(clippy::large_enum_variant)]
pub(crate) enum CoreActorMsg {
    Command(
        HostRuntimeCommand,
        mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
    ),
    TaskStatus(String, mpsc::Sender<Option<TaskStatus>>),
    WaitTaskStates {
        handles: Vec<TaskHandle>,
        deadline: Instant,
        reply: TaskWaitReply,
    },
    WorkerStarted(WorkerStarted),
    WorkerCompleted(RunnerCompletion),
    AsyncEvent(AsyncExecutorEvent),
    AsyncResourceCommand(
        HostRuntimeCommand,
        oneshot::Sender<RuntimeResult<HostRuntimeReply>>,
    ),
    WorkerExited(WorkerExited),
    ManagementFailed {
        runner_id: String,
        invocation_id: String,
    },
    Shutdown,
}

#[derive(Clone)]
/// Cloneable producer that keeps queue-depth and oldest-message metrics in sync
/// with the bounded channel.
pub(crate) struct ActorSender {
    tx: mpsc::SyncSender<CoreActorMsg>,
    wake: mpsc::Sender<()>,
    depth: Arc<AtomicUsize>,
    enqueued_at: Arc<Mutex<VecDeque<Instant>>>,
    spill: Arc<Mutex<VecDeque<CoreActorMsg>>>,
}

/// Single actor-owned consumer. Every receive path must pass through its methods
/// so depth and age observations remain consistent.
pub(crate) struct ActorReceiver {
    rx: mpsc::Receiver<CoreActorMsg>,
    depth: Arc<AtomicUsize>,
    enqueued_at: Arc<Mutex<VecDeque<Instant>>>,
    spill: Arc<Mutex<VecDeque<CoreActorMsg>>>,
}

pub(crate) fn actor_channel(
    capacity: usize,
    wake: mpsc::Sender<()>,
) -> (ActorSender, ActorReceiver) {
    let (tx, rx) = mpsc::sync_channel(capacity);
    let depth = Arc::new(AtomicUsize::new(0));
    let enqueued_at = Arc::new(Mutex::new(VecDeque::new()));
    let spill = Arc::new(Mutex::new(VecDeque::new()));
    (
        ActorSender {
            tx,
            wake,
            depth: depth.clone(),
            enqueued_at: enqueued_at.clone(),
            spill: spill.clone(),
        },
        ActorReceiver {
            rx,
            depth,
            enqueued_at,
            spill,
        },
    )
}

impl ActorSender {
    pub(crate) fn send(&self, message: CoreActorMsg) -> Result<(), ()> {
        self.depth.fetch_add(1, AtomicOrdering::AcqRel);
        self.enqueued_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(Instant::now());
        if self.tx.send(message).is_err() {
            self.depth.fetch_sub(1, AtomicOrdering::AcqRel);
            self.enqueued_at
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_back();
            return Err(());
        }
        self.wake.send(()).map_err(|_| ())
    }

    /// Async callers must never park a runtime worker on a full actor mailbox.
    pub(crate) fn try_send(&self, message: CoreActorMsg) -> Result<(), mpsc::TrySendError<()>> {
        let mut queued = self.enqueued_at.lock().unwrap_or_else(|p| p.into_inner());
        self.depth.fetch_add(1, AtomicOrdering::AcqRel);
        queued.push_back(Instant::now());
        if let Err(error) = self.tx.try_send(message) {
            self.depth.fetch_sub(1, AtomicOrdering::AcqRel);
            queued.pop_back();
            return Err(match error {
                mpsc::TrySendError::Full(_) => mpsc::TrySendError::Full(()),
                mpsc::TrySendError::Disconnected(_) => mpsc::TrySendError::Disconnected(()),
            });
        }
        let _ = self.wake.send(());
        Ok(())
    }

    /// Never parks the caller and never drops the message. Used for completion
    /// facts produced on async runtime workers: a full bounded mailbox spills
    /// into an ordered side queue that the actor drains after the channel.
    /// Spill growth is bounded by executor admission (each admitted invocation
    /// emits a fixed number of events), so this does not unbound the mailbox.
    /// Once anything is spilled, later messages from this path spill too, which
    /// keeps per-invocation event order intact.
    pub(crate) fn send_nonblocking(&self, message: CoreActorMsg) -> Result<(), ()> {
        let mut spill = self.spill.lock().unwrap_or_else(|p| p.into_inner());
        let message = if spill.is_empty() {
            let mut queued = self.enqueued_at.lock().unwrap_or_else(|p| p.into_inner());
            self.depth.fetch_add(1, AtomicOrdering::AcqRel);
            queued.push_back(Instant::now());
            match self.tx.try_send(message) {
                Ok(()) => {
                    drop(queued);
                    drop(spill);
                    let _ = self.wake.send(());
                    return Ok(());
                }
                Err(mpsc::TrySendError::Full(message)) => {
                    self.depth.fetch_sub(1, AtomicOrdering::AcqRel);
                    queued.pop_back();
                    message
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.depth.fetch_sub(1, AtomicOrdering::AcqRel);
                    queued.pop_back();
                    return Err(());
                }
            }
        } else {
            message
        };
        self.depth.fetch_add(1, AtomicOrdering::AcqRel);
        self.enqueued_at
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(Instant::now());
        spill.push_back(message);
        drop(spill);
        self.wake.send(()).map_err(|_| ())
    }

    pub(crate) fn depth(&self) -> usize {
        self.depth.load(AtomicOrdering::Acquire)
    }

    pub(crate) fn oldest_age(&self) -> Duration {
        self.enqueued_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .front()
            .map(Instant::elapsed)
            .unwrap_or_default()
    }
}

impl ActorReceiver {
    fn received(&self, message: CoreActorMsg) -> CoreActorMsg {
        self.depth.fetch_sub(1, AtomicOrdering::AcqRel);
        self.enqueued_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front();
        message
    }

    fn try_recv(&self) -> Result<CoreActorMsg, mpsc::TryRecvError> {
        match self.rx.try_recv() {
            Ok(message) => Ok(self.received(message)),
            Err(error) => {
                // Spilled messages are newer than anything already in the
                // channel from the same producer path, so drain them last.
                let spilled = self
                    .spill
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .pop_front();
                spilled.map(|message| self.received(message)).ok_or(error)
            }
        }
    }

    pub(super) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<CoreActorMsg, mpsc::RecvTimeoutError> {
        match self.try_recv() {
            Ok(message) => return Ok(message),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        self.rx
            .recv_timeout(timeout)
            .map(|message| self.received(message))
    }
}

/// Selects the next message while bounding control bursts so data-plane
/// completions cannot starve behind control traffic.
pub(super) fn receive_actor_message(
    control_rx: &ActorReceiver,
    data_rx: &ActorReceiver,
    wake_rx: &mpsc::Receiver<()>,
    timeout: Option<Duration>,
    control_quota: usize,
    control_burst: &mut usize,
) -> Result<CoreActorMsg, mpsc::RecvTimeoutError> {
    let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
    loop {
        if *control_burst >= control_quota.max(1)
            && let Ok(message) = data_rx.try_recv()
        {
            *control_burst = 0;
            return Ok(message);
        }
        match control_rx.try_recv() {
            Ok(message) => {
                *control_burst = control_burst.saturating_add(1);
                return Ok(message);
            }
            Err(mpsc::TryRecvError::Disconnected)
                if data_rx.depth.load(AtomicOrdering::Acquire) == 0 =>
            {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            Err(_) => {}
        }
        match data_rx.try_recv() {
            Ok(message) => {
                *control_burst = 0;
                return Ok(message);
            }
            Err(mpsc::TryRecvError::Disconnected)
                if control_rx.depth.load(AtomicOrdering::Acquire) == 0 =>
            {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            Err(_) => {}
        }
        match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(mpsc::RecvTimeoutError::Timeout);
                }
                match wake_rx.recv_timeout(remaining) {
                    Ok(()) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        return Err(mpsc::RecvTimeoutError::Timeout);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(mpsc::RecvTimeoutError::Disconnected);
                    }
                }
            }
            None => {
                if wake_rx.recv().is_err() {
                    return Err(mpsc::RecvTimeoutError::Disconnected);
                }
            }
        }
    }
}

/// Groups the two priority lanes and their shared wake channel for actor startup.
pub(super) struct ActorMailboxes {
    pub(super) control: ActorReceiver,
    pub(super) data: ActorReceiver,
    pub(super) wake: mpsc::Receiver<()>,
}

#[cfg(test)]
mod async_admission_tests {
    use super::*;

    #[test]
    fn saturated_async_mailbox_rejects_without_changing_depth() {
        let (wake, _) = mpsc::channel();
        let (sender, receiver) = actor_channel(1, wake);
        sender.try_send(CoreActorMsg::Shutdown).ok().unwrap();
        assert!(matches!(
            sender.try_send(CoreActorMsg::Shutdown),
            Err(mpsc::TrySendError::Full(_))
        ));
        assert_eq!(sender.depth(), 1);
        drop(receiver);
        assert!(matches!(
            sender.try_send(CoreActorMsg::Shutdown),
            Err(mpsc::TrySendError::Disconnected(_))
        ));
        assert_eq!(sender.depth(), 1);
    }

    #[test]
    fn nonblocking_send_spills_in_order_when_mailbox_is_full() {
        let (wake, _wake_rx) = mpsc::channel();
        let (sender, receiver) = actor_channel(1, wake);
        let failed = |invocation_id: &str| CoreActorMsg::ManagementFailed {
            runner_id: "runner".into(),
            invocation_id: invocation_id.into(),
        };
        sender.send_nonblocking(failed("first")).unwrap();
        sender.send_nonblocking(failed("second")).unwrap();
        sender.send_nonblocking(failed("third")).unwrap();
        assert_eq!(sender.depth(), 3);
        let order = (0..3)
            .map(|_| match receiver.try_recv() {
                Ok(CoreActorMsg::ManagementFailed { invocation_id, .. }) => invocation_id,
                _ => panic!("expected spilled message"),
            })
            .collect::<Vec<_>>();
        assert_eq!(order, ["first", "second", "third"]);
        assert_eq!(sender.depth(), 0);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }
}
