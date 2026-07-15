use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use mutsuki_runtime_contracts::ExecutionClass;
use mutsuki_runtime_core::{RunnerCompletion, RunnerDispatch, RuntimeFailure, RuntimeResult};
use serde::Serialize;

use crate::actor::CoreActorMsg;
use crate::error::host_failure;
use crate::host::HostRuntimeConfig;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkerPoolSnapshot {
    pub pool_id: String,
    pub execution_classes: Vec<ExecutionClass>,
    pub configured_threads: usize,
    pub active_threads: usize,
    pub isolated_threads: usize,
    pub queued_batches: usize,
    pub queued_entries: usize,
    pub running_batches: usize,
    pub running_entries: usize,
    pub inflight_bytes: usize,
    pub max_inflight_bytes: usize,
    pub queue_capacity: usize,
    pub max_isolated_threads: usize,
    pub degraded: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PoolCapacitySnapshot {
    pub active_threads: usize,
    pub queued_batches: usize,
    pub queued_entries: usize,
    pub running_batches: usize,
    pub running_entries: usize,
    pub inflight_bytes: usize,
    pub max_inflight_bytes: usize,
}

pub(crate) struct WorkerStarted {
    pub worker_id: String,
    pub execution_class: ExecutionClass,
    pub runner_id: String,
    pub invocation_id: String,
    pub batch_id: String,
    pub task_ids: Vec<String>,
}

pub(crate) struct WorkerExited {
    pub worker_id: String,
    pub execution_class: ExecutionClass,
    pub isolated: bool,
}

struct QueuedDispatch {
    dispatch: RunnerDispatch,
    entry_count: usize,
    payload_bytes: usize,
}

pub(crate) struct WorkerDispatchError {
    pub failure: RuntimeFailure,
    pub dispatch: RunnerDispatch,
    pub retryable: bool,
}

struct WorkerPoolState {
    active_threads: AtomicUsize,
    queued_batches: AtomicUsize,
    queued_entries: AtomicUsize,
    running_batches: AtomicUsize,
    running_entries: AtomicUsize,
    inflight_bytes: AtomicUsize,
    isolated_workers: Mutex<BTreeSet<String>>,
    degraded: AtomicBool,
}

impl Default for WorkerPoolState {
    fn default() -> Self {
        Self {
            active_threads: AtomicUsize::new(0),
            queued_batches: AtomicUsize::new(0),
            queued_entries: AtomicUsize::new(0),
            running_batches: AtomicUsize::new(0),
            running_entries: AtomicUsize::new(0),
            inflight_bytes: AtomicUsize::new(0),
            isolated_workers: Mutex::new(BTreeSet::new()),
            degraded: AtomicBool::new(false),
        }
    }
}

pub(crate) struct WorkerPool {
    pool_id: String,
    execution_classes: Vec<ExecutionClass>,
    sender: Sender<QueuedDispatch>,
    receiver: Receiver<QueuedDispatch>,
    queue_capacity: usize,
    max_inflight_bytes: usize,
    max_isolated_threads: usize,
    configured_threads: usize,
    actor_tx: mpsc::Sender<CoreActorMsg>,
    next_worker_id: Arc<AtomicUsize>,
    state: Arc<WorkerPoolState>,
}

impl WorkerPool {
    fn new(
        pool_id: &str,
        execution_classes: Vec<ExecutionClass>,
        threads: usize,
        queue_capacity: usize,
        max_inflight_bytes: usize,
        max_isolated_threads: usize,
        actor_tx: mpsc::Sender<CoreActorMsg>,
    ) -> RuntimeResult<Self> {
        if threads == 0
            || queue_capacity == 0
            || max_inflight_bytes == 0
            || max_isolated_threads == 0
        {
            return Err(host_failure(
                "host.worker.config",
                format!(
                    "pool {pool_id} requires non-zero threads, queue capacity, byte budget and isolation capacity"
                ),
            ));
        }
        let (sender, receiver) = bounded(queue_capacity);
        let mut pool = Self {
            pool_id: pool_id.to_string(),
            execution_classes,
            sender,
            receiver,
            queue_capacity,
            max_inflight_bytes,
            max_isolated_threads: max_isolated_threads.min(threads),
            configured_threads: threads,
            actor_tx,
            next_worker_id: Arc::new(AtomicUsize::new(0)),
            state: Arc::new(WorkerPoolState::default()),
        };
        for _ in 0..threads {
            pool.spawn_worker()?;
        }
        Ok(pool)
    }

    pub(crate) fn available_slots(&self) -> usize {
        if self.state.degraded.load(Ordering::Acquire)
            || self.state.active_threads.load(Ordering::Acquire) == 0
        {
            return 0;
        }
        self.queue_capacity.saturating_sub(self.sender.len())
    }

    pub(crate) fn capacity(&self) -> PoolCapacitySnapshot {
        let isolated_threads = self
            .state
            .isolated_workers
            .lock()
            .expect("isolated worker lock poisoned")
            .len();
        PoolCapacitySnapshot {
            active_threads: self
                .state
                .active_threads
                .load(Ordering::Acquire)
                .saturating_sub(isolated_threads),
            queued_batches: self.state.queued_batches.load(Ordering::Acquire),
            queued_entries: self.state.queued_entries.load(Ordering::Acquire),
            running_batches: self.state.running_batches.load(Ordering::Acquire),
            running_entries: self.state.running_entries.load(Ordering::Acquire),
            inflight_bytes: self.state.inflight_bytes.load(Ordering::Acquire),
            max_inflight_bytes: self.max_inflight_bytes,
        }
    }

    pub(crate) fn send(&self, dispatch: RunnerDispatch) -> Result<(), WorkerDispatchError> {
        if self.available_slots() == 0 {
            return Err(WorkerDispatchError {
                failure: host_failure(
                    "host.worker.saturated",
                    format!("pool {} has no dispatch capacity", self.pool_id),
                ),
                dispatch,
                retryable: true,
            });
        }
        let entry_count = dispatch.batch.entries.len();
        let payload_bytes = match serde_json::to_vec(&dispatch.batch.payload) {
            Ok(payload) => payload.len(),
            Err(error) => {
                return Err(WorkerDispatchError {
                    failure: host_failure("host.worker.payload", error.to_string()),
                    dispatch,
                    retryable: false,
                });
            }
        };
        if payload_bytes > self.max_inflight_bytes {
            return Err(WorkerDispatchError {
                failure: host_failure(
                    "host.worker.byte_capacity",
                    format!(
                        "dispatch payload bytes {payload_bytes} exceed configured limit {}",
                        self.max_inflight_bytes
                    ),
                ),
                dispatch,
                retryable: false,
            });
        }
        if let Err(failure) = reserve_bytes(
            &self.state.inflight_bytes,
            payload_bytes,
            self.max_inflight_bytes,
        ) {
            return Err(WorkerDispatchError {
                failure,
                dispatch,
                retryable: true,
            });
        }
        self.state.queued_batches.fetch_add(1, Ordering::AcqRel);
        self.state
            .queued_entries
            .fetch_add(entry_count, Ordering::AcqRel);
        let queued = QueuedDispatch {
            dispatch,
            entry_count,
            payload_bytes,
        };
        match self.sender.try_send(queued) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.state.queued_batches.fetch_sub(1, Ordering::AcqRel);
                self.state
                    .queued_entries
                    .fetch_sub(entry_count, Ordering::AcqRel);
                self.state
                    .inflight_bytes
                    .fetch_sub(payload_bytes, Ordering::AcqRel);
                let (detail, queued) = match error {
                    TrySendError::Full(queued) => ("bounded queue is full".to_string(), queued),
                    TrySendError::Disconnected(queued) => {
                        ("worker queue is disconnected".to_string(), queued)
                    }
                };
                Err(WorkerDispatchError {
                    failure: host_failure("host.worker.dispatch", detail),
                    dispatch: queued.dispatch,
                    retryable: true,
                })
            }
        }
    }

    pub(crate) fn isolate(&self, worker_id: &str) -> bool {
        let mut isolated = self
            .state
            .isolated_workers
            .lock()
            .expect("isolated worker lock poisoned");
        if isolated.contains(worker_id) {
            return true;
        }
        if isolated.len() >= self.max_isolated_threads {
            self.state.degraded.store(true, Ordering::Release);
            return false;
        }
        isolated.insert(worker_id.to_string());
        if isolated.len() >= self.max_isolated_threads {
            self.state.degraded.store(true, Ordering::Release);
        }
        true
    }

    pub(crate) fn replace_exited_worker(&mut self, worker_id: &str) -> RuntimeResult<()> {
        let removed = self
            .state
            .isolated_workers
            .lock()
            .expect("isolated worker lock poisoned")
            .remove(worker_id);
        if !removed {
            return Ok(());
        }
        let isolated = self
            .state
            .isolated_workers
            .lock()
            .expect("isolated worker lock poisoned")
            .len();
        self.state
            .degraded
            .store(isolated >= self.max_isolated_threads, Ordering::Release);
        self.spawn_worker().map(|_| ())
    }

    pub(crate) fn snapshot(&self) -> WorkerPoolSnapshot {
        let isolated_threads = self
            .state
            .isolated_workers
            .lock()
            .expect("isolated worker lock poisoned")
            .len();
        WorkerPoolSnapshot {
            pool_id: self.pool_id.clone(),
            execution_classes: self.execution_classes.clone(),
            configured_threads: self.configured_threads,
            active_threads: self.state.active_threads.load(Ordering::Acquire),
            isolated_threads,
            queued_batches: self.state.queued_batches.load(Ordering::Acquire),
            queued_entries: self.state.queued_entries.load(Ordering::Acquire),
            running_batches: self.state.running_batches.load(Ordering::Acquire),
            running_entries: self.state.running_entries.load(Ordering::Acquire),
            inflight_bytes: self.state.inflight_bytes.load(Ordering::Acquire),
            max_inflight_bytes: self.max_inflight_bytes,
            queue_capacity: self.queue_capacity,
            max_isolated_threads: self.max_isolated_threads,
            degraded: self.state.degraded.load(Ordering::Acquire),
        }
    }

    fn spawn_worker(&mut self) -> RuntimeResult<String> {
        let index = self.next_worker_id.fetch_add(1, Ordering::Relaxed);
        let worker_id = format!("{}-worker-{index}", self.pool_id);
        let execution_class = self.execution_classes[0].clone();
        let receiver = self.receiver.clone();
        let actor_tx = self.actor_tx.clone();
        let state = self.state.clone();
        let thread_name = format!("mutsuki-{worker_id}");
        let worker_id_for_thread = worker_id.clone();
        state.active_threads.fetch_add(1, Ordering::AcqRel);
        thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                worker_loop(
                    worker_id_for_thread,
                    execution_class,
                    receiver,
                    actor_tx,
                    state,
                );
            })
            .map_err(|error| {
                self.state.active_threads.fetch_sub(1, Ordering::AcqRel);
                host_failure("host.worker.spawn", error.to_string())
            })?;
        Ok(worker_id)
    }
}

pub(crate) struct WorkerPools {
    compute: WorkerPool,
    blocking: WorkerPool,
}

impl WorkerPools {
    pub(crate) fn get(&self, execution_class: &ExecutionClass) -> Option<&WorkerPool> {
        match execution_class {
            ExecutionClass::Orchestration | ExecutionClass::Io | ExecutionClass::Cpu => {
                Some(&self.compute)
            }
            ExecutionClass::Blocking | ExecutionClass::Script => Some(&self.blocking),
            ExecutionClass::Control => None,
        }
    }

    pub(crate) fn get_mut(&mut self, execution_class: &ExecutionClass) -> Option<&mut WorkerPool> {
        match execution_class {
            ExecutionClass::Orchestration | ExecutionClass::Io | ExecutionClass::Cpu => {
                Some(&mut self.compute)
            }
            ExecutionClass::Blocking | ExecutionClass::Script => Some(&mut self.blocking),
            ExecutionClass::Control => None,
        }
    }

    pub(crate) fn snapshots(&self) -> Vec<WorkerPoolSnapshot> {
        vec![self.compute.snapshot(), self.blocking.snapshot()]
    }
}

pub(crate) fn worker_pools(
    config: &HostRuntimeConfig,
    actor_tx: mpsc::Sender<CoreActorMsg>,
) -> RuntimeResult<WorkerPools> {
    Ok(WorkerPools {
        compute: WorkerPool::new(
            "compute",
            vec![
                ExecutionClass::Orchestration,
                ExecutionClass::Io,
                ExecutionClass::Cpu,
            ],
            config.worker_threads,
            config.pool_queue_limit,
            config.pool_max_inflight_bytes,
            config.max_isolated_workers,
            actor_tx.clone(),
        )?,
        blocking: WorkerPool::new(
            "blocking",
            vec![ExecutionClass::Blocking, ExecutionClass::Script],
            config.blocking_threads,
            config.pool_queue_limit,
            config.pool_max_inflight_bytes,
            config.max_isolated_workers,
            actor_tx,
        )?,
    })
}

fn reserve_bytes(counter: &AtomicUsize, amount: usize, limit: usize) -> RuntimeResult<()> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(amount) else {
            return Err(host_failure(
                "host.worker.byte_capacity",
                "inflight byte counter overflow",
            ));
        };
        if next > limit {
            return Err(host_failure(
                "host.worker.byte_capacity",
                format!("inflight payload bytes {next} exceed configured limit {limit}"),
            ));
        }
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

fn worker_loop(
    worker_id: String,
    default_execution_class: ExecutionClass,
    receiver: Receiver<QueuedDispatch>,
    actor_tx: mpsc::Sender<CoreActorMsg>,
    state: Arc<WorkerPoolState>,
) {
    while let Ok(queued) = receiver.recv() {
        state.queued_batches.fetch_sub(1, Ordering::AcqRel);
        state
            .queued_entries
            .fetch_sub(queued.entry_count, Ordering::AcqRel);
        state.running_batches.fetch_add(1, Ordering::AcqRel);
        state
            .running_entries
            .fetch_add(queued.entry_count, Ordering::AcqRel);
        let started = worker_started(&worker_id, &queued.dispatch);
        if actor_tx.send(CoreActorMsg::WorkerStarted(started)).is_err() {
            finish_dispatch_counters(&state, queued.entry_count, queued.payload_bytes);
            break;
        }
        let entry_count = queued.entry_count;
        let payload_bytes = queued.payload_bytes;
        let completion = execute_dispatch(queued.dispatch);
        finish_dispatch_counters(&state, entry_count, payload_bytes);
        if actor_tx
            .send(CoreActorMsg::WorkerCompleted(completion))
            .is_err()
        {
            break;
        }
        if state
            .isolated_workers
            .lock()
            .expect("isolated worker lock poisoned")
            .contains(&worker_id)
        {
            state.active_threads.fetch_sub(1, Ordering::AcqRel);
            let _ = actor_tx.send(CoreActorMsg::WorkerExited(WorkerExited {
                worker_id,
                execution_class: default_execution_class,
                isolated: true,
            }));
            return;
        }
    }
    state.active_threads.fetch_sub(1, Ordering::AcqRel);
}

fn finish_dispatch_counters(state: &WorkerPoolState, entry_count: usize, payload_bytes: usize) {
    state.running_batches.fetch_sub(1, Ordering::AcqRel);
    state
        .running_entries
        .fetch_sub(entry_count, Ordering::AcqRel);
    state
        .inflight_bytes
        .fetch_sub(payload_bytes, Ordering::AcqRel);
}

fn execute_dispatch(dispatch: RunnerDispatch) -> RunnerCompletion {
    let RunnerDispatch {
        mut runner,
        ctx,
        task_leases,
        batch,
    } = dispatch;
    let batch_id = batch.batch_id.clone();
    let expected_entries = batch.entries.clone();
    let result =
        catch_unwind(AssertUnwindSafe(|| runner.run_batch(ctx, batch))).unwrap_or_else(|_| {
            Err(host_failure(
                "host.worker.panic",
                format!("runner {} panicked", runner.descriptor().runner_id),
            ))
        });
    RunnerCompletion {
        runner,
        task_leases,
        batch_id,
        expected_entries,
        result,
    }
}

fn worker_started(worker_id: &str, dispatch: &RunnerDispatch) -> WorkerStarted {
    WorkerStarted {
        worker_id: worker_id.to_string(),
        execution_class: dispatch.runner.descriptor().execution_class.clone(),
        runner_id: dispatch.runner.descriptor().runner_id.clone(),
        invocation_id: dispatch.ctx.invocation_id.clone(),
        batch_id: dispatch.batch.batch_id.clone(),
        task_ids: dispatch
            .batch
            .entries
            .iter()
            .map(|entry| entry.task_id.clone())
            .collect(),
    }
}
