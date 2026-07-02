use std::sync::{Arc, mpsc};

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{RuntimeResult, ScheduleDecision};
use serde_json::json;

use crate::{
    HostRuntimeCommand, HostRuntimeReply, NativeRunner, RuntimeBootstrapper, ScheduleInput,
};
use crate::{SchedulerPolicy, runner_manifest};

use super::helpers::{descriptor, descriptor_with_class, host_with_echo_runner, runtime_profile};

#[derive(Debug)]
struct FixedScheduler {
    limit: usize,
}

impl SchedulerPolicy for FixedScheduler {
    fn decide(&self, _input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision> {
        Ok(ScheduleDecision::new(
            "test.fixed",
            self.limit,
            "test.fixed",
        ))
    }
}

#[test]
fn custom_scheduler_can_leave_ready_task_undispatched() {
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FixedScheduler { limit: 0 });
    let runtime = host_with_echo_runner()
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-held",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 2 })
        .unwrap();

    let HostRuntimeReply::Idle(report) = reply else {
        panic!("expected idle reply");
    };
    assert_eq!(report.claimed_tasks, 0);
    assert_eq!(runtime.task_status("task-held"), Some(TaskStatus::Ready));
}

#[test]
fn custom_scheduler_limit_is_clamped_to_runner_capacity() {
    let runner_descriptor = descriptor("slow.runner", "slow.work");
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        move |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| {
                    if task.task_id == "slow-1" {
                        release_rx.recv().unwrap();
                    }
                    RunnerResult::completed(task.task_id)
                })
                .collect())
        },
    )));
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FixedScheduler { limit: 99 });
    let runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    for task_id in ["slow-1", "slow-2"] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                task_id,
                "slow.work",
                json!({}),
            ))))
            .unwrap();
    }
    runtime.dispatch(HostRuntimeCommand::TickOnce).unwrap();

    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Running));
    assert_eq!(runtime.task_status("slow-2"), Some(TaskStatus::Ready));
    release_tx.send(()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Completed));
}

#[test]
fn host_runtime_rejects_non_kernel_control_runner_before_scheduling() {
    let runner_descriptor =
        descriptor_with_class("control.runner", "control.work", ExecutionClass::Control);
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FixedScheduler { limit: 99 });
    let err = match host.into_host_runtime_with_config(runtime_profile(), config) {
        Ok(_) => panic!("host runtime boot should reject non-kernel control runner"),
        Err(error) => error,
    };

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}
