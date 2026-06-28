use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use mutsuki_runtime_contracts::{
    CommandBatch, CommandPlan, ExecutionClass, ExportPlan, PlanReceipt, ResourceRef, SagaPlan,
    Task, TaskStatus,
};
use mutsuki_runtime_core::{
    CoreRuntime, RunnerCompletion, RunnerDispatch, RunnerLoopReport, RuntimeResult,
};

use crate::error::host_failure;
use crate::scheduler::{DefaultScheduler, RunnerLimits, SchedulerPolicy, decide_schedule};

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
}

impl HostRuntime {
    pub(crate) fn start(core: CoreRuntime, config: HostRuntimeConfig) -> RuntimeResult<Self> {
        let (tx, rx) = mpsc::channel();
        let actor_tx = tx.clone();
        let actor = thread::Builder::new()
            .name("mutsuki-core-actor".into())
            .spawn(move || core_actor_loop(core, config, rx, actor_tx))
            .map_err(|error| host_failure("host.actor.spawn", error.to_string()))?;
        Ok(Self {
            tx,
            actor: Some(actor),
        })
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

pub enum HostRuntimeCommand {
    SubmitTask(Box<Task>),
    TickOnce,
    RunUntilIdle { max_ticks: usize },
    CancelTask(String),
    CreateBlobResource { schema: String, bytes: Vec<u8> },
    CreateCapabilityResource { kind_id: String, schema: String },
    ExecuteExportPlan(Box<ExportPlan>),
    ExecuteCommandPlan(Box<CommandPlan>),
    ExecuteCommandBatch(Box<CommandBatch>),
    ExecuteSagaPlan(Box<SagaPlan>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostRuntimeReply {
    TaskSubmitted(String),
    Tick(RunnerLoopReport),
    Idle(RunnerLoopReport),
    TaskCancelled(String),
    ResourceCreated(ResourceRef),
    PlanReceipt(PlanReceipt),
    PlanReceipts(Vec<PlanReceipt>),
}

enum CoreActorMsg {
    Command(
        HostRuntimeCommand,
        mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
    ),
    TaskStatus(String, mpsc::Sender<Option<TaskStatus>>),
    WorkerCompleted(RunnerCompletion),
    Shutdown,
}

struct WorkerPool {
    sender: mpsc::Sender<RunnerDispatch>,
    queued: Arc<AtomicUsize>,
    queue_limit: usize,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl WorkerPool {
    fn new(
        name: &str,
        threads: usize,
        queue_limit: usize,
        actor_tx: mpsc::Sender<CoreActorMsg>,
    ) -> RuntimeResult<Self> {
        let (sender, receiver) = mpsc::channel::<RunnerDispatch>();
        let receiver = Arc::new(Mutex::new(receiver));
        let queued = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for index in 0..threads.max(1) {
            let receiver = receiver.clone();
            let queued = queued.clone();
            let actor_tx = actor_tx.clone();
            let thread_name = format!("mutsuki-{name}-worker-{index}");
            let handle = thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    loop {
                        let dispatch = {
                            let receiver = receiver.lock().expect("worker receiver mutex poisoned");
                            receiver.recv()
                        };
                        let Ok(dispatch) = dispatch else {
                            break;
                        };
                        queued.fetch_sub(1, Ordering::Relaxed);
                        let completion = execute_dispatch(dispatch);
                        if actor_tx
                            .send(CoreActorMsg::WorkerCompleted(completion))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(|error| host_failure("host.worker.spawn", error.to_string()))?;
            handles.push(handle);
        }
        Ok(Self {
            sender,
            queued,
            queue_limit,
            _handles: handles,
        })
    }

    fn available_slots(&self) -> usize {
        self.queue_limit
            .saturating_sub(self.queued.load(Ordering::Relaxed))
    }

    fn send(&self, dispatch: RunnerDispatch) -> RuntimeResult<()> {
        self.queued.fetch_add(1, Ordering::Relaxed);
        let result = self.sender.send(dispatch);
        if let Err(error) = result {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            return Err(host_failure("host.worker.dispatch", error.to_string()));
        }
        Ok(())
    }
}

fn execute_dispatch(dispatch: RunnerDispatch) -> RunnerCompletion {
    let RunnerDispatch {
        mut runner,
        ctx,
        task_leases,
        tasks,
    } = dispatch;
    let results = runner.step(ctx, tasks);
    RunnerCompletion {
        runner,
        task_leases,
        results,
    }
}

fn core_actor_loop(
    mut core: CoreRuntime,
    config: HostRuntimeConfig,
    rx: mpsc::Receiver<CoreActorMsg>,
    actor_tx: mpsc::Sender<CoreActorMsg>,
) {
    let mut pools = match worker_pools(&config, actor_tx) {
        Ok(pools) => pools,
        Err(_) => return,
    };
    let mut pending_cancels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            CoreActorMsg::Command(command, reply_tx) => {
                let shutdown = send_command_reply(
                    handle_command(
                        command,
                        &mut core,
                        &config,
                        &mut pools,
                        &rx,
                        &mut pending_cancels,
                    ),
                    reply_tx,
                );
                if shutdown {
                    break;
                }
            }
            CoreActorMsg::TaskStatus(task_id, reply_tx) => {
                let _ = reply_tx.send(core.task_status(&task_id));
            }
            CoreActorMsg::WorkerCompleted(mut completion) => {
                apply_pending_cancels(&mut completion, &mut pending_cancels);
                let _ = core.complete_runner_dispatch(completion);
                let _ = schedule_ready(&mut core, &config, &mut pools);
            }
            CoreActorMsg::Shutdown => break,
        }
    }
}

fn handle_command(
    command: HostRuntimeCommand,
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) -> RuntimeResult<(HostRuntimeReply, bool)> {
    match command {
        HostRuntimeCommand::SubmitTask(task) => {
            let task_id = core.submit_task(*task);
            Ok((HostRuntimeReply::TaskSubmitted(task_id), false))
        }
        HostRuntimeCommand::TickOnce => {
            let mut report = schedule_ready(core, config, pools)?;
            let shutdown =
                drain_worker_completions(core, config, pools, rx, pending_cancels, &mut report, 1);
            Ok((HostRuntimeReply::Tick(report), shutdown))
        }
        HostRuntimeCommand::RunUntilIdle { max_ticks } => {
            let mut shutdown = false;
            let mut aggregate = RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: 0,
            };
            for _ in 0..max_ticks {
                let report = schedule_ready(core, config, pools)?;
                aggregate.claimed_tasks += report.claimed_tasks;
                aggregate.completed_tasks += report.completed_tasks;
                shutdown = drain_worker_completions(
                    core,
                    config,
                    pools,
                    rx,
                    pending_cancels,
                    &mut aggregate,
                    8,
                );
                if core.tasks().ready_count() == 0 && core.tasks().running_count() == 0 {
                    break;
                }
                if shutdown {
                    break;
                }
            }
            Ok((HostRuntimeReply::Idle(aggregate), shutdown))
        }
        HostRuntimeCommand::CancelTask(task_id) => {
            let running_runner = running_runner_for_task(core, &task_id);
            core.cancel_task(&task_id)?;
            if let Some(runner_id) = running_runner {
                pending_cancels
                    .entry(runner_id)
                    .or_default()
                    .push(task_id.clone());
            }
            Ok((HostRuntimeReply::TaskCancelled(task_id), false))
        }
        HostRuntimeCommand::CreateBlobResource { schema, bytes } => Ok((
            HostRuntimeReply::ResourceCreated(core.create_blob_resource(&schema, bytes)),
            false,
        )),
        HostRuntimeCommand::CreateCapabilityResource { kind_id, schema } => Ok((
            HostRuntimeReply::ResourceCreated(core.create_capability_resource(&kind_id, &schema)),
            false,
        )),
        HostRuntimeCommand::ExecuteExportPlan(plan) => Ok((
            HostRuntimeReply::PlanReceipt(core.execute_export_plan(&plan)?),
            false,
        )),
        HostRuntimeCommand::ExecuteCommandPlan(plan) => Ok((
            HostRuntimeReply::PlanReceipt(core.execute_command_plan(&plan)?),
            false,
        )),
        HostRuntimeCommand::ExecuteCommandBatch(batch) => Ok((
            HostRuntimeReply::PlanReceipts(core.execute_command_batch(&batch)?),
            false,
        )),
        HostRuntimeCommand::ExecuteSagaPlan(saga) => Ok((
            HostRuntimeReply::PlanReceipts(core.execute_saga_plan(&saga)?),
            false,
        )),
    }
}

fn send_command_reply(
    outcome: RuntimeResult<(HostRuntimeReply, bool)>,
    reply_tx: mpsc::Sender<RuntimeResult<HostRuntimeReply>>,
) -> bool {
    let shutdown = outcome.as_ref().is_ok_and(|(_, shutdown)| *shutdown);
    let reply = outcome.map(|(reply, _)| reply);
    let _ = reply_tx.send(reply);
    shutdown
}

fn drain_worker_completions(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
    rx: &mpsc::Receiver<CoreActorMsg>,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
    aggregate: &mut RunnerLoopReport,
    max_messages: usize,
) -> bool {
    for _ in 0..max_messages {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(CoreActorMsg::WorkerCompleted(mut completion)) => {
                apply_pending_cancels(&mut completion, pending_cancels);
                if let Ok(report) = core.complete_runner_dispatch(completion) {
                    aggregate.completed_tasks += report.completed_tasks;
                }
                if let Ok(report) = schedule_ready(core, config, pools) {
                    aggregate.claimed_tasks += report.claimed_tasks;
                    aggregate.completed_tasks += report.completed_tasks;
                }
            }
            Ok(CoreActorMsg::TaskStatus(task_id, reply_tx)) => {
                let _ = reply_tx.send(core.task_status(&task_id));
            }
            Ok(CoreActorMsg::Command(command, reply_tx)) => {
                if send_command_reply(
                    handle_command(command, core, config, pools, rx, pending_cancels),
                    reply_tx,
                ) {
                    return true;
                }
            }
            Ok(CoreActorMsg::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => return false,
        }
    }
    false
}

fn running_runner_for_task(core: &CoreRuntime, task_id: &str) -> Option<String> {
    let record = core.tasks().get(task_id)?;
    if record.status != TaskStatus::Running {
        return None;
    }
    record.claimed_by.clone()
}

fn apply_pending_cancels(
    completion: &mut RunnerCompletion,
    pending_cancels: &mut BTreeMap<String, Vec<String>>,
) {
    let runner_id = completion.runner.descriptor().runner_id.clone();
    let Some(invocation_ids) = pending_cancels.remove(&runner_id) else {
        return;
    };
    for invocation_id in invocation_ids {
        let _ = completion.runner.cancel(&invocation_id);
    }
}

fn schedule_ready(
    core: &mut CoreRuntime,
    config: &HostRuntimeConfig,
    pools: &mut HashMap<ExecutionClass, WorkerPool>,
) -> RuntimeResult<RunnerLoopReport> {
    let (report, dispatches) = core.claim_ready_dispatches(
        |descriptor, load, current_step, registry_generation| {
            let limits = config
                .runner_limits
                .get(&descriptor.runner_id)
                .unwrap_or(&config.default_runner_limits);
            let pool_slots = pools
                .get(&descriptor.execution_class)
                .map(WorkerPool::available_slots)
                .unwrap_or(0);
            decide_schedule(
                descriptor,
                load,
                current_step,
                registry_generation,
                limits,
                pool_slots,
                config.scheduler_policy.as_ref(),
            )
        },
        None,
    )?;
    for dispatch in dispatches {
        let execution_class = dispatch.runner.descriptor().execution_class.clone();
        let Some(pool) = pools.get(&execution_class) else {
            return Err(host_failure(
                "host.worker.pool_missing",
                format!("execution_class.{execution_class:?}"),
            ));
        };
        pool.send(dispatch)?;
    }
    Ok(RunnerLoopReport {
        claimed_tasks: report.claimed_tasks,
        completed_tasks: report.completed_tasks,
    })
}

fn worker_pools(
    config: &HostRuntimeConfig,
    actor_tx: mpsc::Sender<CoreActorMsg>,
) -> RuntimeResult<HashMap<ExecutionClass, WorkerPool>> {
    let mut pools = HashMap::new();
    for execution_class in [
        ExecutionClass::Orchestration,
        ExecutionClass::Io,
        ExecutionClass::Cpu,
    ] {
        pools.insert(
            execution_class.clone(),
            WorkerPool::new(
                execution_class_name(&execution_class),
                config.worker_threads,
                config.pool_queue_limit,
                actor_tx.clone(),
            )?,
        );
    }
    for execution_class in [ExecutionClass::Blocking, ExecutionClass::Script] {
        pools.insert(
            execution_class.clone(),
            WorkerPool::new(
                execution_class_name(&execution_class),
                config.blocking_threads,
                config.pool_queue_limit,
                actor_tx.clone(),
            )?,
        );
    }
    Ok(pools)
}

fn execution_class_name(execution_class: &ExecutionClass) -> &'static str {
    match execution_class {
        ExecutionClass::Control => "control",
        ExecutionClass::Orchestration => "orchestration",
        ExecutionClass::Io => "io",
        ExecutionClass::Cpu => "cpu",
        ExecutionClass::Blocking => "blocking",
        ExecutionClass::Script => "script",
    }
}
