//! Bounded actor mailboxes and control/data priority arbitration.

use futures_channel::oneshot;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{TaskHandle, TaskStatus};
use mutsuki_runtime_core::{RunnerCompletion, RuntimeResult};

use crate::async_executor::AsyncExecutorEvent;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply, HostTaskState};
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
        reply: mpsc::Sender<RuntimeResult<Vec<HostTaskState>>>,
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
}

/// Single actor-owned consumer. Every receive path must pass through its methods
/// so depth and age observations remain consistent.
pub(crate) struct ActorReceiver {
    rx: mpsc::Receiver<CoreActorMsg>,
    depth: Arc<AtomicUsize>,
    enqueued_at: Arc<Mutex<VecDeque<Instant>>>,
}

pub(crate) fn actor_channel(
    capacity: usize,
    wake: mpsc::Sender<()>,
) -> (ActorSender, ActorReceiver) {
    let (tx, rx) = mpsc::sync_channel(capacity);
    let depth = Arc::new(AtomicUsize::new(0));
    let enqueued_at = Arc::new(Mutex::new(VecDeque::new()));
    (
        ActorSender {
            tx,
            wake,
            depth: depth.clone(),
            enqueued_at: enqueued_at.clone(),
        },
        ActorReceiver {
            rx,
            depth,
            enqueued_at,
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
        self.rx.try_recv().map(|message| self.received(message))
    }

    pub(super) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<CoreActorMsg, mpsc::RecvTimeoutError> {
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
