use std::fmt;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, RunnerResult, Task, TaskStatus, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RunnerManagementHandle, RuntimeResult};
use serde_json::json;

use crate::{
    HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, RuntimeBootstrapper, runner_manifest,
};

use super::helpers::{descriptor_with_class, runtime_profile};

struct BlockingManagedRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    started: mpsc::Sender<()>,
    work_release: mpsc::Receiver<()>,
    management: Arc<BlockingManagement>,
}

struct BlockingManagement {
    observed: mpsc::Sender<String>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl fmt::Debug for BlockingManagement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BlockingManagement")
    }
}

impl RunnerManagementHandle for BlockingManagement {
    fn cancel(&self, invocation_id: &str) -> RuntimeResult<()> {
        self.observed.send(invocation_id.into()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(())
    }

    fn dispose(&self) -> RuntimeResult<()> {
        Ok(())
    }
}

impl Runner for BlockingManagedRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        self.started.send(()).unwrap();
        self.work_release.recv().unwrap();
        let results = batch
            .entries
            .iter()
            .map(|entry| EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: Some(RunnerResult::completed(entry.task_id.clone())),
                error: None,
            })
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        Some(self.management.clone())
    }
}

#[test]
fn management_cancel_runs_while_batch_and_management_response_are_blocked() {
    let runner_descriptor = descriptor_with_class(
        "managed.blocking.runner",
        "managed.blocking",
        ExecutionClass::Blocking,
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (work_release_tx, work_release_rx) = mpsc::channel();
    let (cancel_observed_tx, cancel_observed_rx) = mpsc::channel();
    let (management_release_tx, management_release_rx) = mpsc::channel();
    let management = Arc::new(BlockingManagement {
        observed: cancel_observed_tx,
        release: Mutex::new(management_release_rx),
    });
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    bootstrapper.register_runner(Box::new(BlockingManagedRunner {
        descriptor: runner_descriptor,
        started: started_tx,
        work_release: work_release_rx,
        management,
    }));
    let runtime = bootstrapper
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap();

    let handle = match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "managed-blocking-1",
            "managed.blocking",
            json!({}),
        ))))
        .unwrap()
    {
        HostRuntimeReply::TaskSubmitted(handle) => handle,
        reply => panic!("unexpected submit reply: {reply:?}"),
    };
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let cancelled_at = Instant::now();
    runtime
        .dispatch(HostRuntimeCommand::CancelTask(handle))
        .unwrap();
    assert!(cancelled_at.elapsed() < Duration::from_millis(50));
    let invocation_id = cancel_observed_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert_eq!(invocation_id, "batch-1-managed.blocking.runner-1");

    let actor_probe = Instant::now();
    runtime.drive_state().unwrap();
    assert!(actor_probe.elapsed() < Duration::from_millis(50));
    assert_eq!(
        runtime.task_status("managed-blocking-1"),
        Some(TaskStatus::Cancelled)
    );

    management_release_tx.send(()).unwrap();
    work_release_tx.send(()).unwrap();
}
