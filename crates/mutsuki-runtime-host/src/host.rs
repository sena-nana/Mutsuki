use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    ObservabilityPage, ObservabilityProfile, RuntimeEvent, TaskStatus, TraceSpan,
};
use mutsuki_runtime_core::{
    CoreRuntime, ReloadDecision, RuntimeResult, RuntimeStatistics, RuntimeStopState,
    TaskHistoryRetention,
};
use mutsuki_runtime_sdk::{
    HostContext as SdkHostContext, HostServiceRegistry, HostTaskSnapshot, ResourceProviderGateway,
};

use crate::actor::{CoreActorMsg, core_actor_loop};
use crate::bootstrapper::PreparedRuntimeReload;
use crate::capabilities::HostCapabilityRegistry;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply, HostTaskState};
use crate::error::host_failure;
use crate::runtime_context::build_host_context;
use crate::scheduler::{
    DefaultScheduler, RunnerLimits, SchedulerPolicy, validate_single_instance_limits,
};
use crate::worker::worker_pools;

pub type HostResourceProviders = BTreeMap<String, Arc<dyn ResourceProviderGateway>>;

#[derive(Debug, Default)]
struct TaskCompletionSubscriptionState {
    revision: u64,
    closed: bool,
}

#[derive(Debug, Default)]
struct TaskCompletionSubscriptionInner {
    state: Mutex<TaskCompletionSubscriptionState>,
    changed: Condvar,
}

#[derive(Clone, Debug)]
pub struct TaskCompletionSubscription {
    inner: Arc<TaskCompletionSubscriptionInner>,
}

impl TaskCompletionSubscription {
    pub fn revision(&self) -> u64 {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .revision
    }

    pub fn wait_after(&self, revision: u64) -> Option<u64> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !state.closed && state.revision <= revision {
            state = self
                .inner
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        (!state.closed).then_some(state.revision)
    }

    pub fn close(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        self.inner.changed.notify_all();
    }
}

#[derive(Debug)]
struct TaskCompletionNotifier {
    inner: Weak<TaskCompletionSubscriptionInner>,
}

impl TaskCompletionNotifier {
    fn notify(&self, revision: u64) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let mut state = inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return false;
        }
        state.revision = state.revision.max(revision);
        inner.changed.notify_all();
        true
    }

    fn close(&self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.closed = true;
            inner.changed.notify_all();
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TaskCompletionHub {
    state: Mutex<TaskCompletionHubState>,
    notifications: AtomicU64,
}

#[derive(Debug, Default)]
struct TaskCompletionHubState {
    revision: u64,
    closed: bool,
    subscribers: Vec<TaskCompletionNotifier>,
}

impl TaskCompletionHub {
    fn subscribe(&self) -> TaskCompletionSubscription {
        let inner = Arc::new(TaskCompletionSubscriptionInner::default());
        let notifier = TaskCompletionNotifier {
            inner: Arc::downgrade(&inner),
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            notifier.close();
        } else {
            let _ = notifier.notify(state.revision);
            state.subscribers.push(notifier);
        }
        TaskCompletionSubscription { inner }
    }

    pub(crate) fn publish(&self, revision: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed || revision <= state.revision {
            return;
        }
        state.revision = revision;
        state
            .subscribers
            .retain(|subscriber| subscriber.notify(revision));
        self.notifications.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        for subscriber in &state.subscribers {
            subscriber.close();
        }
        state.subscribers.clear();
    }

    fn notifications(&self) -> u64 {
        self.notifications.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostRuntimeMetricsSnapshot {
    pub actor_commands: u64,
    pub task_status_queries: u64,
    pub task_state_batch_queries: u64,
    pub completion_notifications: u64,
}

#[derive(Debug, Default)]
struct HostRuntimeMetrics {
    actor_commands: AtomicU64,
    task_status_queries: AtomicU64,
    task_state_batch_queries: AtomicU64,
}

#[derive(Clone)]
pub struct HostRuntimeConfig {
    /// Enables mailbox- and deadline-driven scheduling. Disabled preserves the explicit-tick
    /// embedding mode used by deterministic tests and replay hosts.
    pub event_driven: bool,
    /// Wall-clock duration represented by one logical Core step when a deadline requires time
    /// to advance. An idle runtime does not arm this timer.
    pub tick_interval: Duration,
    pub worker_threads: usize,
    pub blocking_threads: usize,
    pub pool_queue_limit: usize,
    pub pool_max_inflight_bytes: usize,
    pub max_isolated_workers: usize,
    pub default_runner_limits: RunnerLimits,
    pub runner_limits: BTreeMap<String, RunnerLimits>,
    pub scheduler_policy: Arc<dyn SchedulerPolicy>,
    pub resource_providers: HostResourceProviders,
    pub cancel_grace_period: Option<Duration>,
    pub worker_health_timeout: Option<Duration>,
    pub observability: Option<ObservabilityProfile>,
    pub task_history_retention: Option<TaskHistoryRetention>,
}

impl HostRuntimeConfig {
    pub fn with_resource_provider(
        mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn ResourceProviderGateway>,
    ) -> Self {
        self.resource_providers.insert(provider_id.into(), provider);
        self
    }
}

impl fmt::Debug for HostRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostRuntimeConfig")
            .field("event_driven", &self.event_driven)
            .field("tick_interval", &self.tick_interval)
            .field("worker_threads", &self.worker_threads)
            .field("blocking_threads", &self.blocking_threads)
            .field("pool_queue_limit", &self.pool_queue_limit)
            .field("pool_max_inflight_bytes", &self.pool_max_inflight_bytes)
            .field("max_isolated_workers", &self.max_isolated_workers)
            .field("default_runner_limits", &self.default_runner_limits)
            .field("runner_limits", &self.runner_limits)
            .field("scheduler_policy", &self.scheduler_policy)
            .field(
                "resource_providers",
                &self.resource_providers.keys().collect::<Vec<_>>(),
            )
            .field("cancel_grace_period", &self.cancel_grace_period)
            .field("worker_health_timeout", &self.worker_health_timeout)
            .field("observability", &self.observability)
            .field("task_history_retention", &self.task_history_retention)
            .finish()
    }
}

impl Default for HostRuntimeConfig {
    fn default() -> Self {
        Self {
            event_driven: false,
            tick_interval: Duration::from_millis(10),
            worker_threads: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(2)
                .max(1),
            blocking_threads: 2,
            pool_queue_limit: 1024,
            pool_max_inflight_bytes: 64 * 1024 * 1024,
            max_isolated_workers: 2,
            default_runner_limits: RunnerLimits::default(),
            runner_limits: BTreeMap::new(),
            scheduler_policy: Arc::new(DefaultScheduler),
            resource_providers: BTreeMap::new(),
            cancel_grace_period: Some(Duration::from_secs(30)),
            worker_health_timeout: None,
            observability: None,
            task_history_retention: None,
        }
    }
}

pub struct HostRuntime {
    tx: mpsc::Sender<CoreActorMsg>,
    actor: Option<thread::JoinHandle<()>>,
    capabilities: Arc<HostCapabilityRegistry>,
    context: SdkHostContext,
    completion_hub: Arc<TaskCompletionHub>,
    metrics: Arc<HostRuntimeMetrics>,
}

impl HostRuntime {
    pub(crate) fn start(
        mut core: CoreRuntime,
        config: HostRuntimeConfig,
        capabilities: HostCapabilityRegistry,
        services: Arc<HostServiceRegistry>,
        profile_id: String,
        registry_generation: u64,
    ) -> RuntimeResult<Self> {
        validate_single_instance_limits(&config.default_runner_limits, &config.runner_limits)?;
        if config.tick_interval.is_zero() {
            return Err(host_failure(
                "host.driver.tick_interval",
                "tick_interval must be greater than zero",
            ));
        }
        if let Some(observability) = config.observability.clone() {
            core.configure_observability(observability);
        }
        core.configure_task_history_retention(config.task_history_retention);
        let (tx, rx) = mpsc::channel();
        let actor_tx = tx.clone();
        let pools = worker_pools(&config, actor_tx)?;
        let completion_hub = Arc::new(TaskCompletionHub::default());
        let actor_completion_hub = completion_hub.clone();
        let actor = thread::Builder::new()
            .name("mutsuki-core-actor".into())
            .spawn(move || core_actor_loop(core, config, rx, pools, actor_completion_hub))
            .map_err(|error| host_failure("host.actor.spawn", error.to_string()))?;
        let capabilities = Arc::new(capabilities);
        let context = build_host_context(
            tx.clone(),
            capabilities.clone(),
            services,
            profile_id,
            registry_generation,
        );
        Ok(Self {
            tx,
            actor: Some(actor),
            capabilities,
            context,
            completion_hub,
            metrics: Arc::new(HostRuntimeMetrics::default()),
        })
    }

    pub fn capabilities(&self) -> &HostCapabilityRegistry {
        &self.capabilities
    }

    pub fn host_context(&self) -> &SdkHostContext {
        &self.context
    }

    pub fn dispatch(&self, command: HostRuntimeCommand) -> RuntimeResult<HostRuntimeReply> {
        self.metrics.actor_commands.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(CoreActorMsg::Command(command, reply_tx))
            .map_err(|error| host_failure("host.actor.command", error.to_string()))?;
        reply_rx
            .recv()
            .map_err(|error| host_failure("host.actor.reply", error.to_string()))?
    }

    pub fn reload(
        &mut self,
        prepared: PreparedRuntimeReload,
        drain_timeout: Duration,
    ) -> RuntimeResult<ReloadDecision> {
        let capabilities = prepared.capabilities.clone();
        let services = prepared.services.clone();
        let profile_id = prepared.profile_id.clone();
        let registry_generation = prepared.registry_generation;
        match self.dispatch(HostRuntimeCommand::Reload {
            prepared,
            drain_timeout,
        })? {
            HostRuntimeReply::Reloaded(decision) => {
                self.capabilities = Arc::new(capabilities);
                self.context = build_host_context(
                    self.tx.clone(),
                    self.capabilities.clone(),
                    services,
                    profile_id,
                    registry_generation,
                );
                Ok(decision)
            }
            reply => Err(host_failure(
                "host.reload",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn task_status(&self, task_id: &str) -> Option<TaskStatus> {
        self.metrics.actor_commands.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .task_status_queries
            .fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(CoreActorMsg::TaskStatus(task_id.to_string(), reply_tx))
            .ok()?;
        reply_rx.recv().ok().flatten()
    }

    pub fn task_snapshots(&self) -> RuntimeResult<Vec<HostTaskSnapshot>> {
        match self.dispatch(HostRuntimeCommand::TaskSnapshots)? {
            HostRuntimeReply::TaskSnapshots(snapshots) => Ok(snapshots),
            reply => Err(host_failure(
                "host.task_snapshots",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn task_states(
        &self,
        handles: Vec<mutsuki_runtime_contracts::TaskHandle>,
    ) -> RuntimeResult<Vec<HostTaskState>> {
        self.metrics
            .task_state_batch_queries
            .fetch_add(1, Ordering::Relaxed);
        match self.dispatch(HostRuntimeCommand::TaskStatesBatch(handles))? {
            HostRuntimeReply::TaskStatesBatch(states) => Ok(states),
            reply => Err(host_failure(
                "host.task_states",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn subscribe_task_completions(&self) -> TaskCompletionSubscription {
        self.completion_hub.subscribe()
    }

    pub fn metrics(&self) -> HostRuntimeMetricsSnapshot {
        HostRuntimeMetricsSnapshot {
            actor_commands: self.metrics.actor_commands.load(Ordering::Relaxed),
            task_status_queries: self.metrics.task_status_queries.load(Ordering::Relaxed),
            task_state_batch_queries: self
                .metrics
                .task_state_batch_queries
                .load(Ordering::Relaxed),
            completion_notifications: self.completion_hub.notifications(),
        }
    }

    pub fn begin_drain(&self) -> RuntimeResult<RuntimeStopState> {
        match self.dispatch(HostRuntimeCommand::BeginDrain)? {
            HostRuntimeReply::DrainStarted(state) => Ok(state),
            reply => Err(host_failure(
                "host.begin_drain",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn abort(&self, reason: impl Into<String>) -> RuntimeResult<usize> {
        match self.dispatch(HostRuntimeCommand::Abort {
            reason: reason.into(),
        })? {
            HostRuntimeReply::RuntimeAborted { cancelled_tasks } => Ok(cancelled_tasks),
            reply => Err(host_failure(
                "host.abort",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn stop_state(&self) -> RuntimeResult<RuntimeStopState> {
        match self.dispatch(HostRuntimeCommand::StopState)? {
            HostRuntimeReply::StopState(state) => Ok(state),
            reply => Err(host_failure(
                "host.stop_state",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn statistics(&self) -> RuntimeResult<RuntimeStatistics> {
        match self.dispatch(HostRuntimeCommand::Statistics)? {
            HostRuntimeReply::Statistics(statistics) => Ok(statistics),
            reply => Err(host_failure(
                "host.statistics",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn drive_state(&self) -> RuntimeResult<HostRuntimeDriveState> {
        match self.dispatch(HostRuntimeCommand::DriveState)? {
            HostRuntimeReply::DriveState(state) => Ok(state),
            reply => Err(host_failure(
                "host.drive_state",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn worker_pools(&self) -> RuntimeResult<Vec<crate::WorkerPoolSnapshot>> {
        match self.dispatch(HostRuntimeCommand::WorkerPools)? {
            HostRuntimeReply::WorkerPools(pools) => Ok(pools),
            reply => Err(host_failure(
                "host.worker_pools",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn events_after(
        &self,
        sequence: u64,
        limit: usize,
    ) -> RuntimeResult<ObservabilityPage<RuntimeEvent>> {
        match self.dispatch(HostRuntimeCommand::EventsAfter { sequence, limit })? {
            HostRuntimeReply::Events(page) => Ok(page),
            reply => Err(host_failure(
                "host.events_after",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }

    pub fn trace_spans_after(
        &self,
        sequence: u64,
        limit: usize,
    ) -> RuntimeResult<ObservabilityPage<TraceSpan>> {
        match self.dispatch(HostRuntimeCommand::TraceSpansAfter { sequence, limit })? {
            HostRuntimeReply::TraceSpans(page) => Ok(page),
            reply => Err(host_failure(
                "host.trace_spans_after",
                format!("unexpected reply: {reply:?}"),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRuntimeDriveState {
    pub current_step: u64,
    pub next_required_tick: Option<u64>,
    pub next_wake_deadline: Option<Instant>,
    pub timed_wakeups: u64,
}

impl Drop for HostRuntime {
    fn drop(&mut self) {
        let _ = self.tx.send(CoreActorMsg::Shutdown);
        if let Some(actor) = self.actor.take() {
            let _ = actor.join();
        }
        self.completion_hub.close();
    }
}

impl mutsuki_runtime_sdk::HostRuntime for HostRuntime {
    type PreparedReload = PreparedRuntimeReload;

    fn host_context(&self) -> &SdkHostContext {
        &self.context
    }

    fn reload(
        &mut self,
        prepared: Self::PreparedReload,
        drain_timeout: Duration,
    ) -> RuntimeResult<ReloadDecision> {
        HostRuntime::reload(self, prepared, drain_timeout)
    }

    fn begin_drain(&self) -> RuntimeResult<RuntimeStopState> {
        HostRuntime::begin_drain(self)
    }

    fn abort(&self, reason: &str) -> RuntimeResult<usize> {
        HostRuntime::abort(self, reason)
    }

    fn stop_state(&self) -> RuntimeResult<RuntimeStopState> {
        HostRuntime::stop_state(self)
    }

    fn statistics(&self) -> RuntimeResult<RuntimeStatistics> {
        HostRuntime::statistics(self)
    }

    fn task_snapshots(&self) -> RuntimeResult<Vec<HostTaskSnapshot>> {
        HostRuntime::task_snapshots(self)
    }

    fn events_after(
        &self,
        sequence: u64,
        limit: usize,
    ) -> RuntimeResult<ObservabilityPage<RuntimeEvent>> {
        HostRuntime::events_after(self, sequence, limit)
    }

    fn trace_spans_after(
        &self,
        sequence: u64,
        limit: usize,
    ) -> RuntimeResult<ObservabilityPage<TraceSpan>> {
        HostRuntime::trace_spans_after(self, sequence, limit)
    }
}
