use std::io::Cursor;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::CoreRuntime;
use serde_json::json;

use crate::{JsonlRunner, NativePluginHost, NativeRunner};

use super::helpers::{
    abi_plugin_fixture, host_with_echo_runner, runtime_profile, runtime_profile_with_deployment,
};

#[test]
fn native_plugin_host_boots_runtime_and_runs_runner_loop() {
    let mut runtime: CoreRuntime = host_with_echo_runner()
        .into_runtime(runtime_profile())
        .unwrap();

    runtime.enqueue_task(Task::new("task-1", "raw.input", json!({"ok": true})));
    let report = runtime.run_until_idle(4).unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(
        runtime.tasks().get("task-1").unwrap().status,
        TaskStatus::Completed
    );
}

#[test]
fn native_plugin_host_can_boot_host_runtime_control_plane() {
    let mut runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let submitted = runtime
        .dispatch(crate::HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-1",
            "raw.input",
            json!({"ok": true}),
        ))))
        .unwrap();
    assert_eq!(
        submitted,
        crate::HostRuntimeReply::TaskSubmitted("task-1".into())
    );

    let reply = runtime
        .dispatch(crate::HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    let crate::HostRuntimeReply::Idle(report) = reply else {
        panic!("expected idle reply");
    };
    assert_eq!(report.completed_tasks, 1);
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Completed));
}

#[test]
fn abi_plugin_boots_through_registered_abi_runner_bridge() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut host = NativePluginHost::new();
    host.register_manifest(manifest);
    host.register_abi_runner(Box::new(JsonlRunner::new(
        runner_descriptor,
        reader,
        writer,
    )));

    let runtime = host.into_runtime(runtime_profile_with_deployment(
        "plugin-abi",
        PluginDeploymentKind::Abi,
    ));

    assert!(runtime.is_ok());
}

#[test]
fn enabled_plugin_runner_requires_matching_deployment_bridge() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let profile = runtime_profile_with_deployment("plugin-abi", PluginDeploymentKind::Abi);
    let mut missing_bridge_host = NativePluginHost::new();
    missing_bridge_host.register_manifest(manifest.clone());

    let missing_bridge = missing_bridge_host
        .into_runtime(profile.clone())
        .err()
        .unwrap();

    assert_eq!(missing_bridge.error().code, ERR_RUNNER_NOT_FOUND);

    let mut mismatched_host = NativePluginHost::new();
    mismatched_host.register_manifest(manifest);
    mismatched_host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));

    let mismatched = mismatched_host.into_runtime(profile).err().unwrap();

    assert_eq!(mismatched.error().code, ERR_REGISTRY_UNAUTHORIZED);
}
