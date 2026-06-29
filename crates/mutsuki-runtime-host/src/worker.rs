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
    sender: mpsc::Sender<RunnerDispatch>,
    queued: Arc<AtomicUsize>,
    queue_limit: usize,
    _handles: Vec<thread::JoinHandle<()>>,
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
