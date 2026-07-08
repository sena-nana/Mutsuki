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

#[derive(Debug)]
struct CapacityAssertingScheduler;

impl SchedulerPolicy for CapacityAssertingScheduler {
    fn decide(&self, input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision> {
        if input.runner.runner_id != "batch.runner" {
            return Ok(ScheduleDecision::new(
                "test.capacity",
                0,
                "test.capacity.skip",
            ));
        }
        assert_eq!(input.host_capacity.preferred_batch_size, 16);
        if input.host_capacity.queued_batches == 0 {
            return Ok(ScheduleDecision::new(
                "test.capacity",
                0,
                "test.capacity.no-ready",
            ));
        }
        assert_eq!(input.host_capacity.running_batches, 0);
        assert_eq!(input.host_capacity.running_entries, 0);
        assert_eq!(
            input.host_capacity.queued_entries,
            input.host_capacity.queued_batches
        );
        assert!(input.host_capacity.saturation > 0.0);
        assert_eq!(input.host_capacity.max_entry_concurrency, 1);
        Ok(ScheduleDecision::new(
            "test.capacity",
            input.host_capacity.preferred_batch_size,
            "test.capacity",
        ))
    }
}

#[derive(Debug)]
struct FailingScheduler;

impl SchedulerPolicy for FailingScheduler {
    fn decide(&self, _input: &ScheduleInput<'_>) -> RuntimeResult<ScheduleDecision> {
        Err(mutsuki_runtime_core::RuntimeFailure::new(
            RuntimeError::new("scheduler.failed", "test.scheduler", "test.scheduler.fail"),
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
            if tasks.task_id == "slow-1" {
                release_rx.recv().unwrap();
            }
            Ok(RunnerResult::completed(tasks.task_id))
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
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Running));
    assert_eq!(runtime.task_status("slow-2"), Some(TaskStatus::Ready));
    release_tx.send(()).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    assert_eq!(runtime.task_status("slow-1"), Some(TaskStatus::Completed));
}

#[test]
fn scheduler_policy_receives_host_capacity_feedback() {
    let mut runner_descriptor = descriptor("batch.runner", "batch.work");
    runner_descriptor.batch.preferred_batch_size = 16;
    runner_descriptor.batch.max_batch_entries = 64;
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, task| Ok(RunnerResult::completed(task.task_id)),
    )));
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(CapacityAssertingScheduler);
    let runtime = host
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "batch-capacity-1",
            "batch.work",
            json!({}),
        ))))
        .unwrap();
    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    let HostRuntimeReply::Idle(report) = reply else {
        panic!("expected idle reply");
    };
    assert_eq!(report.claimed_tasks, 1);
}

#[test]
fn custom_scheduler_failure_is_not_defaulted() {
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FailingScheduler);
    let runtime = host_with_echo_runner()
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-fail-schedule",
            "raw.input",
            json!({}),
        ))))
        .unwrap();

    let err = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 1 })
        .unwrap_err();

    assert_eq!(err.error().code, "scheduler.failed");
    assert_eq!(
        runtime.task_status("task-fail-schedule"),
        Some(TaskStatus::Ready)
    );
}

#[test]
fn host_runtime_rejects_non_kernel_control_runner_before_scheduling() {
    let runner_descriptor =
        descriptor_with_class("control.runner", "control.work", ExecutionClass::Control);
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));
    let mut config = crate::HostRuntimeConfig::default();
    config.scheduler_policy = Arc::new(FixedScheduler { limit: 99 });
    let err = match host.into_host_runtime_with_config(runtime_profile(), config) {
        Ok(_) => panic!("host runtime boot should reject non-kernel control runner"),
        Err(error) => error,
    };

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}
