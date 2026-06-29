use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};
use std::thread;

use mutsuki_runtime_contracts::TaskStatus;
use mutsuki_runtime_core::{CoreRuntime, RuntimeResult};

use crate::actor::{CoreActorMsg, core_actor_loop};
use crate::capabilities::HostCapabilityRegistry;
use crate::commands::{HostRuntimeCommand, HostRuntimeReply};
use crate::error::host_failure;
use crate::scheduler::{DefaultScheduler, RunnerLimits, SchedulerPolicy};

#[derive(Clone, Debug)]
pub struct HostRuntimeConfig {
    pub worker_threads: usize,
    pub blocking_threads: usize,
    pub pool_queue_limit: usize,
    pub default_runner_limits: RunnerLimits,
    pub runner_limits: BTreeMap<String, RunnerLimits>,
    pub scheduler_policy: Arc<dyn SchedulerPolicy>,
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
        }
    }
}

pub struct HostRuntime {
    tx: mpsc::Sender<CoreActorMsg>,
    actor: Option<thread::JoinHandle<()>>,
    capabilities: Arc<HostCapabilityRegistry>,
}

impl HostRuntime {
    pub(crate) fn start(
        core: CoreRuntime,
        config: HostRuntimeConfig,
        capabilities: HostCapabilityRegistry,
    ) -> RuntimeResult<Self> {
        let (tx, rx) = mpsc::channel();
        let actor_tx = tx.clone();
        let actor = thread::Builder::new()
            .name("mutsuki-core-actor".into())
            .spawn(move || core_actor_loop(core, config, rx, actor_tx))
            .map_err(|error| host_failure("host.actor.spawn", error.to_string()))?;
        Ok(Self {
            tx,
            actor: Some(actor),
            capabilities: Arc::new(capabilities),
        })
    }

    pub fn capabilities(&self) -> &HostCapabilityRegistry {
        &self.capabilities
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
