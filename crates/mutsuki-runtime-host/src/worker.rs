use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use mutsuki_runtime_contracts::{DispatchLane, ExecutionClass};
use mutsuki_runtime_core::{
    RunnerCompletion, RunnerDispatch, RunnerDispatchTarget, RuntimeFailure, RuntimeResult,
};
use serde::Serialize;

use crate::actor::{ActorSender, CoreActorMsg};
use crate::error::host_failure;
use crate::host::HostRuntimeConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneExecutionPolicy {
    pub weight: usize,
    pub reserved_entries: usize,
    pub max_share_percent: u8,
    pub queue_entry_limit: usize,
    pub max_inflight_bytes: usize,
    pub starvation_steps: u64,
    pub allow_idle_borrow: bool,
}

impl LaneExecutionPolicy {
    pub fn for_lane(lane: &DispatchLane) -> Self {
        match lane {
            DispatchLane::Control => Self {
                weight: 8,
                reserved_entries: 1,
                max_share_percent: 100,
                queue_entry_limit: 1_024,
                max_inflight_bytes: 16 * 1024 * 1024,
                starvation_steps: 1,
                allow_idle_borrow: true,
            },
            DispatchLane::Interactive => Self {
                weight: 8,
                reserved_entries: 2,
                max_share_percent: 100,
                queue_entry_limit: 4_096,
                max_inflight_bytes: 32 * 1024 * 1024,
                starvation_steps: 2,
                allow_idle_borrow: true,
            },
            DispatchLane::Normal => Self {
                weight: 4,
                reserved_entries: 0,
                max_share_percent: 100,
                queue_entry_limit: 8_192,
                max_inflight_bytes: 64 * 1024 * 1024,
                starvation_steps: 8,
                allow_idle_borrow: true,
            },
            DispatchLane::Background => Self {
                weight: 2,
                reserved_entries: 0,
                max_share_percent: 50,
                queue_entry_limit: 8_192,
                max_inflight_bytes: 64 * 1024 * 1024,
                starvation_steps: 16,
                allow_idle_borrow: true,
            },
            DispatchLane::Bulk => Self {
                weight: 1,
                reserved_entries: 0,
                max_share_percent: 25,
                queue_entry_limit: 4_096,
                max_inflight_bytes: 64 * 1024 * 1024,
                starvation_steps: 32,
                allow_idle_borrow: true,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionDomainConfig {
    pub domain_id: String,
    pub execution_classes: Vec<ExecutionClass>,
    pub threads: usize,
    pub queue_capacity: usize,
    pub max_inflight_bytes: usize,
    pub max_isolated_threads: usize,
    pub lane_policies: BTreeMap<DispatchLane, LaneExecutionPolicy>,
}

impl ExecutionDomainConfig {
    pub fn new(
        domain_id: impl Into<String>,
        execution_classes: Vec<ExecutionClass>,
        threads: usize,
    ) -> Self {
        Self {
            domain_id: domain_id.into(),
            execution_classes,
            threads,
            queue_capacity: 1_024,
            max_inflight_bytes: 64 * 1024 * 1024,
            max_isolated_threads: 2,
            lane_policies: DispatchLane::ALL
                .into_iter()
                .map(|lane| (lane.clone(), LaneExecutionPolicy::for_lane(&lane)))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LaneWorkerSnapshot {
    pub lane: DispatchLane,
    pub queued_batches: usize,
    pub queued_entries: usize,
    pub running_batches: usize,
    pub running_entries: usize,
    pub inflight_bytes: usize,
    pub queue_entry_limit: usize,
    pub max_inflight_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkerPoolSnapshot {
    pub domain_id: String,
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
    pub lanes: Vec<LaneWorkerSnapshot>,
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
    lane_entries: BTreeMap<DispatchLane, usize>,
    lane_bytes: BTreeMap<DispatchLane, usize>,
}

pub(crate) struct WorkerDispatchError {
    pub failure: RuntimeFailure,
    pub dispatch: Box<RunnerDispatch>,
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
    lanes: Mutex<BTreeMap<DispatchLane, LanePoolCounters>>,
    degraded: AtomicBool,
}

#[derive(Clone, Copy, Debug, Default)]
struct LanePoolCounters {
    queued_batches: usize,
    queued_entries: usize,
    running_batches: usize,
    running_entries: usize,
    inflight_bytes: usize,
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
            lanes: Mutex::new(BTreeMap::new()),
            degraded: AtomicBool::new(false),
        }
    }
}

pub(crate) struct WorkerPool {
    domain_id: String,
    execution_classes: Vec<ExecutionClass>,
    sender: Sender<QueuedDispatch>,
    receiver: Receiver<QueuedDispatch>,
    queue_capacity: usize,
    max_inflight_bytes: usize,
    max_isolated_threads: usize,
    configured_threads: usize,
    lane_policies: BTreeMap<DispatchLane, LaneExecutionPolicy>,
    actor_tx: ActorSender,
    next_worker_id: Arc<AtomicUsize>,
    state: Arc<WorkerPoolState>,
}

impl WorkerPool {
    fn new(
        domain_id: &str,
        execution_classes: Vec<ExecutionClass>,
        threads: usize,
        queue_capacity: usize,
        max_inflight_bytes: usize,
        max_isolated_threads: usize,
        lane_policies: BTreeMap<DispatchLane, LaneExecutionPolicy>,
        actor_tx: ActorSender,
    ) -> RuntimeResult<Self> {
        if threads == 0
            || queue_capacity == 0
            || max_inflight_bytes == 0
            || max_isolated_threads == 0
        {
            return Err(host_failure(
                "host.worker.config",
                format!(
                    "execution domain {domain_id} requires non-zero threads, queue capacity, byte budget and isolation capacity"
                ),
            ));
        }
        let (sender, receiver) = bounded(queue_capacity);
        let mut pool = Self {
            domain_id: domain_id.to_string(),
            execution_classes,
            sender,
            receiver,
            queue_capacity,
            max_inflight_bytes,
            max_isolated_threads: max_isolated_threads.min(threads),
            configured_threads: threads,
            lane_policies,
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
                    format!(
                        "execution domain {} has no dispatch capacity",
                        self.domain_id
                    ),
                ),
                dispatch: Box::new(dispatch),
                retryable: true,
            });
        }
        let entry_count = dispatch.batch.entries.len();
        let payload_bytes = match serde_json::to_vec(&dispatch.batch.payload) {
            Ok(payload) => payload.len(),
            Err(error) => {
                return Err(WorkerDispatchError {
                    failure: host_failure("host.worker.payload", error.to_string()),
                    dispatch: Box::new(dispatch),
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
                dispatch: Box::new(dispatch),
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
                dispatch: Box::new(dispatch),
                retryable: true,
            });
        }
        let lane_entries = dispatch_lane_entries(&dispatch);
        let lane_bytes = distribute_lane_bytes(&lane_entries, payload_bytes, entry_count);
        if let Err(failure) = self.reserve_lane_capacity(&lane_entries, &lane_bytes) {
            self.state
                .inflight_bytes
                .fetch_sub(payload_bytes, Ordering::AcqRel);
            return Err(WorkerDispatchError {
                failure,
                dispatch: Box::new(dispatch),
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
            lane_entries,
            lane_bytes,
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
                self.rollback_queued_lanes(&queued.lane_entries, &queued.lane_bytes);
                Err(WorkerDispatchError {
                    failure: host_failure("host.worker.dispatch", detail),
                    dispatch: Box::new(queued.dispatch),
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
            domain_id: self.domain_id.clone(),
            pool_id: self.domain_id.clone(),
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
            lanes: self.lane_snapshots(),
        }
    }

    fn reserve_lane_capacity(
        &self,
        lane_entries: &BTreeMap<DispatchLane, usize>,
        lane_bytes: &BTreeMap<DispatchLane, usize>,
    ) -> RuntimeResult<()> {
        let mut counters = self
            .state
            .lanes
            .lock()
            .expect("lane capacity lock poisoned");
        for (lane, entries) in lane_entries {
            let policy = self
                .lane_policies
                .get(lane)
                .expect("validated execution domain has every lane policy");
            let current = counters.get(lane).copied().unwrap_or_default();
            if current.queued_entries.saturating_add(*entries) > policy.queue_entry_limit {
                return Err(host_failure(
                    "host.worker.lane_queue_capacity",
                    format!("execution_domain.{}.lane.{lane:?}", self.domain_id),
                ));
            }
            if current
                .inflight_bytes
                .saturating_add(lane_bytes.get(lane).copied().unwrap_or_default())
                > policy.max_inflight_bytes
            {
                return Err(host_failure(
                    "host.worker.lane_byte_capacity",
                    format!("execution_domain.{}.lane.{lane:?}", self.domain_id),
                ));
            }
        }
        for (lane, entries) in lane_entries {
            let current = counters.entry(lane.clone()).or_default();
            current.queued_batches = current.queued_batches.saturating_add(1);
            current.queued_entries = current.queued_entries.saturating_add(*entries);
            current.inflight_bytes = current
                .inflight_bytes
                .saturating_add(lane_bytes.get(lane).copied().unwrap_or_default());
        }
        Ok(())
    }

    fn rollback_queued_lanes(
        &self,
        lane_entries: &BTreeMap<DispatchLane, usize>,
        lane_bytes: &BTreeMap<DispatchLane, usize>,
    ) {
        let mut counters = self
            .state
            .lanes
            .lock()
            .expect("lane capacity lock poisoned");
        for (lane, entries) in lane_entries {
            let current = counters.entry(lane.clone()).or_default();
            current.queued_batches = current.queued_batches.saturating_sub(1);
            current.queued_entries = current.queued_entries.saturating_sub(*entries);
            current.inflight_bytes = current
                .inflight_bytes
                .saturating_sub(lane_bytes.get(lane).copied().unwrap_or_default());
        }
    }

    fn lane_snapshots(&self) -> Vec<LaneWorkerSnapshot> {
        let counters = self
            .state
            .lanes
            .lock()
            .expect("lane capacity lock poisoned");
        DispatchLane::ALL
            .into_iter()
            .map(|lane| {
                let current = counters.get(&lane).copied().unwrap_or_default();
                let policy = self
                    .lane_policies
                    .get(&lane)
                    .expect("validated execution domain has every lane policy");
                LaneWorkerSnapshot {
                    lane,
                    queued_batches: current.queued_batches,
                    queued_entries: current.queued_entries,
                    running_batches: current.running_batches,
                    running_entries: current.running_entries,
                    inflight_bytes: current.inflight_bytes,
                    queue_entry_limit: policy.queue_entry_limit,
                    max_inflight_bytes: policy.max_inflight_bytes,
                }
            })
            .collect()
    }

    fn spawn_worker(&mut self) -> RuntimeResult<String> {
        let index = self.next_worker_id.fetch_add(1, Ordering::Relaxed);
        let worker_id = format!("{}-worker-{index}", self.domain_id);
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
    domains: BTreeMap<String, WorkerPool>,
    class_to_domain: HashMap<ExecutionClass, String>,
}

impl WorkerPools {
    pub(crate) fn get(&self, execution_class: &ExecutionClass) -> Option<&WorkerPool> {
        let domain_id = self.class_to_domain.get(execution_class)?;
        self.domains.get(domain_id)
    }

    pub(crate) fn get_mut(&mut self, execution_class: &ExecutionClass) -> Option<&mut WorkerPool> {
        let domain_id = self.class_to_domain.get(execution_class)?;
        self.domains.get_mut(domain_id)
    }

    pub(crate) fn snapshots(&self) -> Vec<WorkerPoolSnapshot> {
        self.domains.values().map(WorkerPool::snapshot).collect()
    }

    pub(crate) fn domain_id(&self, execution_class: &ExecutionClass) -> Option<&str> {
        self.class_to_domain
            .get(execution_class)
            .map(String::as_str)
    }

    pub(crate) fn lane_policies(
        &self,
        execution_class: &ExecutionClass,
    ) -> Option<&BTreeMap<DispatchLane, LaneExecutionPolicy>> {
        self.get(execution_class).map(|pool| &pool.lane_policies)
    }
}

pub(crate) fn worker_pools(
    config: &HostRuntimeConfig,
    actor_tx: ActorSender,
) -> RuntimeResult<WorkerPools> {
    let configured = if config.execution_domains.is_empty() {
        vec![
            ExecutionDomainConfig {
                domain_id: "compute".into(),
                execution_classes: vec![ExecutionClass::Orchestration, ExecutionClass::Cpu],
                threads: config.worker_threads,
                queue_capacity: config.pool_queue_limit,
                max_inflight_bytes: config.pool_max_inflight_bytes,
                max_isolated_threads: config.max_isolated_workers,
                lane_policies: ExecutionDomainConfig::new("compute", Vec::new(), 1).lane_policies,
            },
            ExecutionDomainConfig {
                domain_id: "blocking".into(),
                execution_classes: vec![
                    ExecutionClass::Io,
                    ExecutionClass::Blocking,
                    ExecutionClass::Script,
                ],
                threads: config.blocking_threads,
                queue_capacity: config.pool_queue_limit,
                max_inflight_bytes: config.pool_max_inflight_bytes,
                max_isolated_threads: config.max_isolated_workers,
                lane_policies: ExecutionDomainConfig::new("blocking", Vec::new(), 1).lane_policies,
            },
        ]
    } else {
        config.execution_domains.clone()
    };
    let mut domains = BTreeMap::new();
    let mut class_to_domain = HashMap::new();
    for domain in configured {
        validate_execution_domain(&domain, &domains, &class_to_domain)?;
        for execution_class in &domain.execution_classes {
            class_to_domain.insert(execution_class.clone(), domain.domain_id.clone());
        }
        let pool = WorkerPool::new(
            &domain.domain_id,
            domain.execution_classes,
            domain.threads,
            domain.queue_capacity,
            domain.max_inflight_bytes,
            domain.max_isolated_threads,
            domain.lane_policies,
            actor_tx.clone(),
        )?;
        domains.insert(domain.domain_id, pool);
    }
    for execution_class in [
        ExecutionClass::Orchestration,
        ExecutionClass::Io,
        ExecutionClass::Cpu,
        ExecutionClass::Blocking,
        ExecutionClass::Script,
    ] {
        if !class_to_domain.contains_key(&execution_class) {
            return Err(host_failure(
                "host.execution_domain.class_missing",
                format!("execution_class.{execution_class:?}"),
            ));
        }
    }
    Ok(WorkerPools {
        domains,
        class_to_domain,
    })
}

fn validate_execution_domain(
    domain: &ExecutionDomainConfig,
    domains: &BTreeMap<String, WorkerPool>,
    class_to_domain: &HashMap<ExecutionClass, String>,
) -> RuntimeResult<()> {
    if domain.domain_id.trim().is_empty()
        || domain.execution_classes.is_empty()
        || domains.contains_key(&domain.domain_id)
    {
        return Err(host_failure(
            "host.execution_domain.config",
            format!("execution_domain.{}", domain.domain_id),
        ));
    }
    for execution_class in &domain.execution_classes {
        if *execution_class == ExecutionClass::Control
            || class_to_domain.contains_key(execution_class)
        {
            return Err(host_failure(
                "host.execution_domain.class_conflict",
                format!("execution_class.{execution_class:?}"),
            ));
        }
    }
    for lane in DispatchLane::ALL {
        let Some(policy) = domain.lane_policies.get(&lane) else {
            return Err(host_failure(
                "host.execution_domain.lane_missing",
                format!("execution_domain.{}.lane.{lane:?}", domain.domain_id),
            ));
        };
        if policy.weight == 0
            || policy.max_share_percent == 0
            || policy.max_share_percent > 100
            || policy.queue_entry_limit == 0
            || policy.max_inflight_bytes == 0
        {
            return Err(host_failure(
                "host.execution_domain.lane_config",
                format!("execution_domain.{}.lane.{lane:?}", domain.domain_id),
            ));
        }
    }
    Ok(())
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
    actor_tx: ActorSender,
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
        move_lanes_to_running(&state, &queued.lane_entries);
        let started = worker_started(&worker_id, &queued.dispatch);
        if actor_tx.send(CoreActorMsg::WorkerStarted(started)).is_err() {
            finish_dispatch_counters(
                &state,
                queued.entry_count,
                queued.payload_bytes,
                &queued.lane_entries,
                &queued.lane_bytes,
            );
            break;
        }
        let entry_count = queued.entry_count;
        let payload_bytes = queued.payload_bytes;
        let lane_entries = queued.lane_entries;
        let lane_bytes = queued.lane_bytes;
        let completion = execute_dispatch(queued.dispatch);
        finish_dispatch_counters(
            &state,
            entry_count,
            payload_bytes,
            &lane_entries,
            &lane_bytes,
        );
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

fn finish_dispatch_counters(
    state: &WorkerPoolState,
    entry_count: usize,
    payload_bytes: usize,
    lane_entries: &BTreeMap<DispatchLane, usize>,
    lane_bytes: &BTreeMap<DispatchLane, usize>,
) {
    state.running_batches.fetch_sub(1, Ordering::AcqRel);
    state
        .running_entries
        .fetch_sub(entry_count, Ordering::AcqRel);
    state
        .inflight_bytes
        .fetch_sub(payload_bytes, Ordering::AcqRel);
    let mut counters = state.lanes.lock().expect("lane capacity lock poisoned");
    for (lane, entries) in lane_entries {
        let current = counters.entry(lane.clone()).or_default();
        current.running_batches = current.running_batches.saturating_sub(1);
        current.running_entries = current.running_entries.saturating_sub(*entries);
        current.inflight_bytes = current
            .inflight_bytes
            .saturating_sub(lane_bytes.get(lane).copied().unwrap_or_default());
    }
}

fn move_lanes_to_running(state: &WorkerPoolState, lane_entries: &BTreeMap<DispatchLane, usize>) {
    let mut counters = state.lanes.lock().expect("lane capacity lock poisoned");
    for (lane, entries) in lane_entries {
        let current = counters.entry(lane.clone()).or_default();
        current.queued_batches = current.queued_batches.saturating_sub(1);
        current.queued_entries = current.queued_entries.saturating_sub(*entries);
        current.running_batches = current.running_batches.saturating_add(1);
        current.running_entries = current.running_entries.saturating_add(*entries);
    }
}

fn dispatch_lane_entries(dispatch: &RunnerDispatch) -> BTreeMap<DispatchLane, usize> {
    let mut lanes = BTreeMap::new();
    for entry in &dispatch.batch.entries {
        *lanes.entry(entry.lane.clone()).or_default() += 1;
    }
    lanes
}

fn distribute_lane_bytes(
    lane_entries: &BTreeMap<DispatchLane, usize>,
    payload_bytes: usize,
    entry_count: usize,
) -> BTreeMap<DispatchLane, usize> {
    if entry_count == 0 {
        return BTreeMap::new();
    }
    let mut assigned = 0usize;
    let mut distributed = BTreeMap::new();
    for (index, (lane, entries)) in lane_entries.iter().enumerate() {
        let bytes = if index + 1 == lane_entries.len() {
            payload_bytes.saturating_sub(assigned)
        } else {
            payload_bytes.saturating_mul(*entries) / entry_count
        };
        assigned = assigned.saturating_add(bytes);
        distributed.insert(lane.clone(), bytes);
    }
    distributed
}

fn execute_dispatch(dispatch: RunnerDispatch) -> RunnerCompletion {
    let RunnerDispatch {
        target,
        ctx,
        task_leases,
        batch,
    } = dispatch;
    let batch_id = batch.batch_id.clone();
    let expected_entries = batch.entries.clone();
    match target {
        RunnerDispatchTarget::Sync(mut runner) => {
            let result = catch_unwind(AssertUnwindSafe(|| runner.run_batch(ctx, batch)))
                .unwrap_or_else(|_| {
                    Err(host_failure(
                        "host.worker.panic",
                        format!("runner {} panicked", runner.descriptor().runner_id),
                    ))
                });
            RunnerCompletion {
                runner: Some(runner),
                task_leases,
                batch_id,
                expected_entries,
                result,
            }
        }
        RunnerDispatchTarget::Async(_) => RunnerCompletion {
            runner: None,
            task_leases,
            batch_id,
            expected_entries,
            result: Err(host_failure(
                "host.worker.async_dispatch",
                "async handler was sent to a synchronous worker pool",
            )),
        },
    }
}

fn worker_started(worker_id: &str, dispatch: &RunnerDispatch) -> WorkerStarted {
    WorkerStarted {
        worker_id: worker_id.to_string(),
        execution_class: dispatch.target.descriptor().execution_class.clone(),
        runner_id: dispatch.target.descriptor().runner_id.clone(),
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
