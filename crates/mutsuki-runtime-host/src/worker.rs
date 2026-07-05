use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use mutsuki_runtime_contracts::ExecutionClass;
use mutsuki_runtime_core::{RunnerCompletion, RunnerDispatch, RuntimeResult};

use crate::actor::CoreActorMsg;
use crate::error::host_failure;
use crate::host::HostRuntimeConfig;

pub(crate) struct WorkerPool {
    name: String,
    sender: mpsc::Sender<RunnerDispatch>,
    receiver: Arc<Mutex<mpsc::Receiver<RunnerDispatch>>>,
    queued: Arc<AtomicUsize>,
    queue_limit: usize,
    actor_tx: mpsc::Sender<CoreActorMsg>,
    next_worker_id: Arc<AtomicUsize>,
    handles: Vec<thread::JoinHandle<()>>,
}

pub(crate) struct WorkerStarted {
    pub worker_id: String,
    pub execution_class: ExecutionClass,
    pub runner_id: String,
    pub invocation_id: String,
    pub batch_id: String,
    pub task_ids: Vec<String>,
}

impl WorkerPool {
    pub(crate) fn new(
        name: &str,
        threads: usize,
        queue_limit: usize,
        actor_tx: mpsc::Sender<CoreActorMsg>,
    ) -> RuntimeResult<Self> {
        let (sender, receiver) = mpsc::channel::<RunnerDispatch>();
        let receiver = Arc::new(Mutex::new(receiver));
        let queued = Arc::new(AtomicUsize::new(0));
        let mut pool = Self {
            name: name.to_string(),
            sender,
            receiver,
            queued,
            queue_limit,
            actor_tx,
            next_worker_id: Arc::new(AtomicUsize::new(0)),
            handles: Vec::new(),
        };
        for _ in 0..threads.max(1) {
            pool.spawn_worker()?;
        }
        Ok(pool)
    }

    pub(crate) fn available_slots(&self) -> usize {
        self.queue_limit
            .saturating_sub(self.queued.load(Ordering::Relaxed))
    }

    pub(crate) fn send(&self, dispatch: RunnerDispatch) -> RuntimeResult<()> {
        self.queued.fetch_add(1, Ordering::Relaxed);
        let result = self.sender.send(dispatch);
        if let Err(error) = result {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            return Err(host_failure("host.worker.dispatch", error.to_string()));
        }
        Ok(())
    }

    pub(crate) fn replace_isolated_worker(&mut self) -> RuntimeResult<()> {
        self.spawn_worker().map(|_| ())
    }

    fn spawn_worker(&mut self) -> RuntimeResult<String> {
        let index = self.next_worker_id.fetch_add(1, Ordering::Relaxed);
        let worker_id = format!("{}-worker-{index}", self.name);
        let receiver = self.receiver.clone();
        let queued = self.queued.clone();
        let actor_tx = self.actor_tx.clone();
        let thread_name = format!("mutsuki-{worker_id}");
        let worker_id_for_thread = worker_id.clone();
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
                    let started = worker_started(&worker_id_for_thread, &dispatch);
                    if actor_tx.send(CoreActorMsg::WorkerStarted(started)).is_err() {
                        break;
                    }
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
        self.handles.push(handle);
        Ok(worker_id)
    }
}

fn execute_dispatch(dispatch: RunnerDispatch) -> RunnerCompletion {
    let RunnerDispatch {
        mut runner,
        ctx,
        task_leases,
        batch,
    } = dispatch;
    let batch_id = batch.batch_id.clone();
    let result = runner.run_batch(ctx, batch);
    RunnerCompletion {
        runner,
        task_leases,
        batch_id,
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

pub(crate) fn worker_pools(
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
