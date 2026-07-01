use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use mutsuki_runtime_contracts::TaskStatus;
use mutsuki_runtime_core::{CoreRuntime, ReloadDecision, RuntimeResult};
use mutsuki_runtime_sdk::{HostContext as SdkHostContext, ResourceProviderGateway};

use crate::actor::{CoreActorMsg, core_actor_loop};
use crate::bootstrapper::PreparedRuntimeReload;
use crate::capabilities::HostCapabilityRegistry;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::host_failure;
use crate::runtime_context::build_host_context;
use crate::scheduler::{DefaultScheduler, RunnerLimits, SchedulerPolicy};

#[derive(Clone)]
pub struct HostRuntimeConfig {
    pub worker_threads: usize,
    pub blocking_threads: usize,
    pub pool_queue_limit: usize,
    pub default_runner_limits: RunnerLimits,
    pub runner_limits: BTreeMap<String, RunnerLimits>,
    pub scheduler_policy: Arc<dyn SchedulerPolicy>,
    pub resource_provider: Option<Arc<dyn ResourceProviderGateway>>,
    pub cancel_grace_period: Option<Duration>,
    pub worker_health_timeout: Option<Duration>,
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
            .field("resource_provider", &self.resource_provider.is_some())
            .field("cancel_grace_period", &self.cancel_grace_period)
            .field("worker_health_timeout", &self.worker_health_timeout)
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
            resource_provider: None,
            cancel_grace_period: Some(Duration::from_secs(30)),
            worker_health_timeout: None,
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
        core: CoreRuntime,
        config: HostRuntimeConfig,
        capabilities: HostCapabilityRegistry,
        profile_id: String,
        registry_generation: u64,
    ) -> RuntimeResult<Self> {
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

    pub fn dispatch(&mut self, command: HostRuntimeCommand) -> RuntimeResult<HostRuntimeReply> {
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
    fn host_context(&self) -> &SdkHostContext {
        &self.context
    }
}
