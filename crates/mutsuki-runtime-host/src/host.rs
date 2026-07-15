use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

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
use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::host_failure;
use crate::runtime_context::build_host_context;
use crate::scheduler::{
    DefaultScheduler, RunnerLimits, SchedulerPolicy, validate_single_instance_limits,
};

pub type HostResourceProviders = BTreeMap<String, Arc<dyn ResourceProviderGateway>>;

#[derive(Clone)]
pub struct HostRuntimeConfig {
    pub worker_threads: usize,
    pub blocking_threads: usize,
    pub pool_queue_limit: usize,
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
            .field("worker_threads", &self.worker_threads)
            .field("blocking_threads", &self.blocking_threads)
            .field("pool_queue_limit", &self.pool_queue_limit)
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
            worker_threads: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(2)
                .max(1),
            blocking_threads: 2,
            pool_queue_limit: 1024,
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
        if let Some(observability) = config.observability.clone() {
            core.configure_observability(observability);
        }
        core.configure_task_history_retention(config.task_history_retention);
        let (tx, rx) = mpsc::channel();
        let actor_tx = tx.clone();
        let actor = thread::Builder::new()
            .name("mutsuki-core-actor".into())
            .spawn(move || core_actor_loop(core, config, rx, actor_tx))
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
        })
    }

    pub fn capabilities(&self) -> &HostCapabilityRegistry {
        &self.capabilities
    }

    pub fn host_context(&self) -> &SdkHostContext {
        &self.context
    }

    pub fn dispatch(&self, command: HostRuntimeCommand) -> RuntimeResult<HostRuntimeReply> {
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

impl Drop for HostRuntime {
    fn drop(&mut self) {
        let _ = self.tx.send(CoreActorMsg::Shutdown);
        if let Some(actor) = self.actor.take() {
            let _ = actor.join();
        }
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
