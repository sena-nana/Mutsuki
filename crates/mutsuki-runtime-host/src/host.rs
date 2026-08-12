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
    AsyncResourceProviderGateway, HostContext as SdkHostContext, HostServiceRegistry,
    HostTaskSnapshot, ResourceProviderGateway,
};

use crate::actor::{ActorSender, CoreActorMsg, actor_channel, core_actor_loop};
use crate::async_executor::{AsyncEventSink, AsyncExecutor};
use crate::bootstrapper::PreparedRuntimeReload;
use crate::capabilities::HostCapabilityRegistry;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply, HostTaskState};
use crate::error::host_failure;
use crate::runtime_context::build_host_context;
use crate::scheduler::{DefaultScheduler, RunnerLimits, SchedulerPolicy, validate_runner_limits};
use crate::worker::worker_pools;

pub type HostResourceProviders = BTreeMap<String, Arc<dyn ResourceProviderGateway>>;
pub type HostAsyncResourceProviders = BTreeMap<String, Arc<dyn AsyncResourceProviderGateway>>;

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
        self.wait_until(revision, None)
    }

    /// Waits for a completion revision newer than `revision`, or until `timeout` elapses.
    ///
    /// Returns `Some(revision)` when notified, and `None` when the subscription is closed or the
    /// wait times out without a newer revision.
    pub fn wait_after_timeout(&self, revision: u64, timeout: Duration) -> Option<u64> {
        self.wait_until(revision, Some(Instant::now() + timeout))
    }

    fn wait_until(&self, revision: u64, deadline: Option<Instant>) -> Option<u64> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !state.closed && state.revision <= revision {
            state = match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return None;
                    }
                    let (guard, wait_result) = self
                        .inner
                        .changed
                        .wait_timeout(state, deadline.saturating_duration_since(now))
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if wait_result.timed_out() && guard.revision <= revision {
                        return None;
                    }
                    guard
                }
                None => self
                    .inner
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
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
    pub control_mailbox_depth: usize,
    pub data_mailbox_depth: usize,
    pub control_oldest_message_age: Duration,
    pub data_oldest_message_age: Duration,
    pub submit_to_dispatch_samples: u64,
    pub submit_to_dispatch_total_ns: u64,
    pub submit_to_dispatch_max_ns: u64,
    pub cancel_propagation_samples: u64,
    pub cancel_propagation_total_ns: u64,
    pub cancel_propagation_max_ns: u64,
    pub completion_route_samples: u64,
    pub completion_route_total_ns: u64,
    pub completion_route_max_ns: u64,
    pub scheduler_passes: u64,
    pub scheduler_total_ns: u64,
    pub scheduler_max_ns: u64,
    pub lane_starvation_events: u64,
    pub reserved_capacity_uses: u64,
}

#[derive(Debug, Default)]
#[doc(hidden)]
pub struct HostRuntimeMetrics {
    actor_commands: AtomicU64,
    task_status_queries: AtomicU64,
    task_state_batch_queries: AtomicU64,
    submit_to_dispatch_samples: AtomicU64,
    submit_to_dispatch_total_ns: AtomicU64,
    submit_to_dispatch_max_ns: AtomicU64,
    cancel_propagation_samples: AtomicU64,
    cancel_propagation_total_ns: AtomicU64,
    cancel_propagation_max_ns: AtomicU64,
    completion_route_samples: AtomicU64,
    completion_route_total_ns: AtomicU64,
    completion_route_max_ns: AtomicU64,
    scheduler_passes: AtomicU64,
    scheduler_total_ns: AtomicU64,
    scheduler_max_ns: AtomicU64,
    lane_starvation_events: AtomicU64,
    reserved_capacity_uses: AtomicU64,
}

impl HostRuntimeMetrics {
    pub(crate) fn record_submit_to_dispatch(&self, elapsed: Duration) {
        record_latency(
            elapsed,
            &self.submit_to_dispatch_samples,
            &self.submit_to_dispatch_total_ns,
            &self.submit_to_dispatch_max_ns,
        );
    }

    pub(crate) fn record_cancel_propagation(&self, elapsed: Duration) {
        record_latency(
            elapsed,
            &self.cancel_propagation_samples,
            &self.cancel_propagation_total_ns,
            &self.cancel_propagation_max_ns,
        );
    }

    pub(crate) fn record_completion_route(&self, elapsed: Duration) {
        record_latency(
            elapsed,
            &self.completion_route_samples,
            &self.completion_route_total_ns,
            &self.completion_route_max_ns,
        );
    }

    pub(crate) fn record_scheduler_pass(&self, elapsed: Duration) {
        record_latency(
            elapsed,
            &self.scheduler_passes,
            &self.scheduler_total_ns,
            &self.scheduler_max_ns,
        );
    }

    pub(crate) fn record_lane_starvation(&self, count: usize) {
        self.lane_starvation_events
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub(crate) fn record_reserved_capacity_use(&self, count: usize) {
        self.reserved_capacity_uses
            .fetch_add(count as u64, Ordering::Relaxed);
    }
}

fn record_latency(
    elapsed: Duration,
    samples: &AtomicU64,
    total_ns: &AtomicU64,
    max_ns: &AtomicU64,
) {
    let nanos = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
    samples.fetch_add(1, Ordering::Relaxed);
    total_ns.fetch_add(nanos, Ordering::Relaxed);
    max_ns.fetch_max(nanos, Ordering::Relaxed);
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
    pub management_threads: usize,
    pub management_queue_limit: usize,
    pub pool_queue_limit: usize,
    pub pool_max_inflight_bytes: usize,
    pub max_isolated_workers: usize,
    /// Configured physical execution domains. An empty list preserves the
    /// legacy compute/blocking topology.
    pub execution_domains: Vec<crate::ExecutionDomainConfig>,
    pub actor_control_queue_limit: usize,
    pub actor_data_queue_limit: usize,
    pub actor_control_quota: usize,
    pub default_runner_limits: RunnerLimits,
    pub runner_limits: BTreeMap<String, RunnerLimits>,
    pub scheduler_policy: Arc<dyn SchedulerPolicy>,
    /// Host-owned executor for native async handlers. `None` keeps the minimal
    /// synchronous Host surface and rejects async handlers structurally.
    pub async_executor: Option<Arc<dyn AsyncExecutor>>,
    #[doc(hidden)]
    pub async_event_sink: Option<AsyncEventSink>,
    #[doc(hidden)]
    pub async_resource_sequence: Arc<AtomicU64>,
    pub resource_providers: HostResourceProviders,
    pub async_resource_providers: HostAsyncResourceProviders,
    pub cancel_grace_period: Option<Duration>,
    pub worker_health_timeout: Option<Duration>,
    pub observability: Option<ObservabilityProfile>,
    pub task_history_retention: Option<TaskHistoryRetention>,
    #[doc(hidden)]
    pub actor_metrics: Arc<HostRuntimeMetrics>,
}

impl HostRuntimeConfig {
    pub fn with_async_executor(mut self, executor: Arc<dyn AsyncExecutor>) -> Self {
        self.async_executor = Some(executor);
        self
    }

    pub fn with_resource_provider(
        mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn ResourceProviderGateway>,
    ) -> Self {
        self.resource_providers.insert(provider_id.into(), provider);
        self
    }

    pub fn with_async_resource_provider(
        mut self,
        provider_id: impl Into<String>,
        provider: Arc<dyn AsyncResourceProviderGateway>,
    ) -> Self {
        self.async_resource_providers
            .insert(provider_id.into(), provider);
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
            .field("management_threads", &self.management_threads)
            .field("management_queue_limit", &self.management_queue_limit)
            .field("pool_queue_limit", &self.pool_queue_limit)
            .field("pool_max_inflight_bytes", &self.pool_max_inflight_bytes)
            .field("max_isolated_workers", &self.max_isolated_workers)
            .field("execution_domains", &self.execution_domains)
            .field("actor_control_queue_limit", &self.actor_control_queue_limit)
            .field("actor_data_queue_limit", &self.actor_data_queue_limit)
            .field("actor_control_quota", &self.actor_control_quota)
            .field("default_runner_limits", &self.default_runner_limits)
            .field("runner_limits", &self.runner_limits)
            .field("scheduler_policy", &self.scheduler_policy)
            .field(
                "async_executor",
                &self
                    .async_executor
                    .as_ref()
                    .map(|executor| executor.snapshot()),
            )
            .field(
                "resource_providers",
                &self.resource_providers.keys().collect::<Vec<_>>(),
            )
            .field(
                "async_resource_providers",
                &self.async_resource_providers.keys().collect::<Vec<_>>(),
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
            management_threads: 1,
            management_queue_limit: 256,
            pool_queue_limit: 1024,
            pool_max_inflight_bytes: 64 * 1024 * 1024,
            max_isolated_workers: 2,
            execution_domains: Vec::new(),
            actor_control_queue_limit: 1_024,
            actor_data_queue_limit: 4_096,
            actor_control_quota: 32,
            default_runner_limits: RunnerLimits::default(),
            runner_limits: BTreeMap::new(),
            scheduler_policy: Arc::new(DefaultScheduler),
            async_executor: None,
            async_event_sink: None,
            async_resource_sequence: Arc::new(AtomicU64::new(0)),
            resource_providers: BTreeMap::new(),
            async_resource_providers: BTreeMap::new(),
            cancel_grace_period: Some(Duration::from_secs(30)),
            worker_health_timeout: None,
            observability: None,
            task_history_retention: None,
            actor_metrics: Arc::new(HostRuntimeMetrics::default()),
        }
    }
}

pub struct HostRuntime {
    tx: ActorSender,
    data_tx: ActorSender,
    actor: Option<thread::JoinHandle<()>>,
    capabilities: Arc<HostCapabilityRegistry>,
    context: SdkHostContext,
    completion_hub: Arc<TaskCompletionHub>,
    metrics: Arc<HostRuntimeMetrics>,
}

impl HostRuntime {
    pub(crate) fn start(
        mut core: CoreRuntime,
        mut config: HostRuntimeConfig,
        capabilities: HostCapabilityRegistry,
        services: Arc<HostServiceRegistry>,
        profile_id: String,
        registry_generation: u64,
    ) -> RuntimeResult<Self> {
        validate_runner_limits(&config.default_runner_limits, &config.runner_limits)?;
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
        if config.actor_control_queue_limit == 0
            || config.actor_data_queue_limit == 0
            || config.actor_control_quota == 0
        {
            return Err(host_failure(
                "host.actor.config",
                "actor queue limits and control quota must be positive",
            ));
        }
        let metrics = Arc::new(HostRuntimeMetrics::default());
        config.actor_metrics = metrics.clone();
        let (wake_tx, wake_rx) = mpsc::channel();
        let (tx, control_rx) = actor_channel(config.actor_control_queue_limit, wake_tx.clone());
        let (data_tx, data_rx) = actor_channel(config.actor_data_queue_limit, wake_tx);
        let actor_tx = data_tx.clone();
        let async_event_tx = data_tx.clone();
        config.async_event_sink = Some(Arc::new(move |event| {
            let _ = async_event_tx.send(CoreActorMsg::AsyncEvent(event));
        }));
        let pools = worker_pools(&config, actor_tx)?;
        let management = crate::management::ManagementExecutor::new(
            config.management_threads,
            config.management_queue_limit,
            tx.clone(),
        )?;
        let completion_hub = Arc::new(TaskCompletionHub::default());
        let actor_completion_hub = completion_hub.clone();
        let actor = thread::Builder::new()
            .name("mutsuki-core-actor".into())
            .spawn(move || {
                core_actor_loop(
                    core,
                    config,
                    control_rx,
                    data_rx,
                    wake_rx,
                    pools,
                    management,
                    actor_completion_hub,
                )
            })
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
            data_tx,
            actor: Some(actor),
            capabilities,
            context,
            completion_hub,
            metrics,
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
            .map_err(|_| host_failure("host.actor.command", "actor mailbox closed"))?;
        reply_rx
            .recv()
            .map_err(|error| host_failure("host.actor.reply", error.to_string()))?
    }

    pub fn reload(
        &mut self,
        mut prepared: PreparedRuntimeReload,
        drain_timeout: Duration,
    ) -> RuntimeResult<ReloadDecision> {
        if let Some(affected_plugins) = &prepared.affected_plugins {
            prepared.services = mutsuki_runtime_sdk::HostServiceRegistry::merge_for_plugins(
                self.context.services(),
                &prepared.services,
                affected_plugins,
            )?;
        }
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

    /// Blocks until every handle reaches a terminal outcome, the runtime stops, or `timeout`
    /// elapses. The actor owns the wait so completion returns the committed terminal states without
    /// a follow-up status query.
    pub fn wait_task_states(
        &self,
        handles: Vec<mutsuki_runtime_contracts::TaskHandle>,
        timeout: Duration,
    ) -> RuntimeResult<Vec<HostTaskState>> {
        if handles.is_empty() {
            return Ok(Vec::new());
        }
        let deadline = Instant::now() + timeout;
        self.metrics.actor_commands.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .task_state_batch_queries
            .fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(CoreActorMsg::WaitTaskStates {
                handles,
                deadline,
                reply: reply_tx,
            })
            .map_err(|_| host_failure("host.actor.command", "actor mailbox closed"))?;
        reply_rx
            .recv()
            .map_err(|error| host_failure("host.actor.reply", error.to_string()))?
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
            control_mailbox_depth: self.tx.depth(),
            data_mailbox_depth: self.data_tx.depth(),
            control_oldest_message_age: self.tx.oldest_age(),
            data_oldest_message_age: self.data_tx.oldest_age(),
            submit_to_dispatch_samples: self
                .metrics
                .submit_to_dispatch_samples
                .load(Ordering::Relaxed),
            submit_to_dispatch_total_ns: self
                .metrics
                .submit_to_dispatch_total_ns
                .load(Ordering::Relaxed),
            submit_to_dispatch_max_ns: self
                .metrics
                .submit_to_dispatch_max_ns
                .load(Ordering::Relaxed),
            cancel_propagation_samples: self
                .metrics
                .cancel_propagation_samples
                .load(Ordering::Relaxed),
            cancel_propagation_total_ns: self
                .metrics
                .cancel_propagation_total_ns
                .load(Ordering::Relaxed),
            cancel_propagation_max_ns: self
                .metrics
                .cancel_propagation_max_ns
                .load(Ordering::Relaxed),
            completion_route_samples: self
                .metrics
                .completion_route_samples
                .load(Ordering::Relaxed),
            completion_route_total_ns: self
                .metrics
                .completion_route_total_ns
                .load(Ordering::Relaxed),
            completion_route_max_ns: self.metrics.completion_route_max_ns.load(Ordering::Relaxed),
            scheduler_passes: self.metrics.scheduler_passes.load(Ordering::Relaxed),
            scheduler_total_ns: self.metrics.scheduler_total_ns.load(Ordering::Relaxed),
            scheduler_max_ns: self.metrics.scheduler_max_ns.load(Ordering::Relaxed),
            lane_starvation_events: self.metrics.lane_starvation_events.load(Ordering::Relaxed),
            reserved_capacity_uses: self.metrics.reserved_capacity_uses.load(Ordering::Relaxed),
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

    pub fn async_executor(&self) -> RuntimeResult<Option<crate::AsyncExecutorSnapshot>> {
        match self.dispatch(HostRuntimeCommand::AsyncExecutor)? {
            HostRuntimeReply::AsyncExecutor(snapshot) => Ok(snapshot),
            reply => Err(host_failure(
                "host.async_executor",
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
