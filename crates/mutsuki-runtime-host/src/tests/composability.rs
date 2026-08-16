use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_sdk::HostRuntime as SdkHostRuntime;
use serde_json::json;

use crate::{HostRuntime, HostRuntimeCommand, NativeRunner, RuntimeBootstrapper, runner_manifest};

use super::helpers::{descriptor, runtime_profile};

fn wait_for_task_status(runtime: &HostRuntime, task_id: &str, expected: TaskStatus) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if runtime.task_status(task_id) == Some(expected.clone()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "task {task_id} did not reach {expected:?}; current status: {:?}",
        runtime.task_status(task_id)
    );
}

fn state_writing_host(expected_version: u64, patch: serde_json::Value) -> RuntimeBootstrapper {
    let runner_descriptor = descriptor("state.runner", "sim.state");
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        move |_ctx, task| {
            let mut result = RunnerResult::completed(task.task_id);
            result.deltas.push(StateDelta {
                target_ref: "state:actor".into(),
                expected_version,
                patch: patch.clone(),
                conflict_policy: ConflictPolicy::Fail,
            });
            Ok(result)
        },
    )));
    host
}

#[test]
fn host_reload_applies_kernel_commits_and_retains_them() {
    let mut runtime = state_writing_host(0, json!({"owned": "data"}))
        .into_host_runtime(runtime_profile())
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "before-reload",
            "sim.state",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
        .unwrap();
    wait_for_task_status(&runtime, "before-reload:commit:0", TaskStatus::Completed);

    let prepared = state_writing_host(1, json!({"owned": "data", "by": "v2"}))
        .prepare_reload(runtime_profile(), 2)
        .unwrap();
    SdkHostRuntime::reload(&mut runtime, prepared, Duration::from_secs(1)).unwrap();
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "after-reload",
            "sim.state",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
        .unwrap();
    wait_for_task_status(&runtime, "after-reload:commit:0", TaskStatus::Completed);
    assert_eq!(runtime.host_context().registry_generation(), 2);
}

#[test]
fn host_optional_state_history_can_roll_back_within_retain_steps() {
    let mut profile = runtime_profile();
    profile.observability.state_history = StateHistoryProfile::bounded(8, 32);
    let runtime = state_writing_host(0, json!({"owned": "v1"}))
        .into_host_runtime(profile)
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "write-v1",
            "sim.state",
            json!({}),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
        .unwrap();
    wait_for_task_status(&runtime, "write-v1:commit:0", TaskStatus::Completed);

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "write-v2",
            "core.commit",
            serde_json::to_value(state_delta(1, json!({"owned": "v2"}))).unwrap(),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
        .unwrap();
    wait_for_task_status(&runtime, "write-v2", TaskStatus::Completed);

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "rollback-1",
            "core.state.rollback",
            serde_json::to_value(StateRollback {
                target_ref: "state:actor".into(),
                to_version: 1,
            })
            .unwrap(),
        ))))
        .unwrap();
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
        .unwrap();
    wait_for_task_status(&runtime, "rollback-1", TaskStatus::Completed);
}

fn state_delta(expected_version: u64, patch: serde_json::Value) -> StateDelta {
    StateDelta {
        target_ref: "state:actor".into(),
        expected_version,
        patch,
        conflict_policy: ConflictPolicy::Fail,
    }
}
