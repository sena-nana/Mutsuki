use mutsuki_plugin_dev_mock::{ECHO_PROTOCOL, PLUGIN_ID as DEV_PLUGIN_ID};
use mutsuki_plugin_workflow_broadcast::{
    BROADCAST_EMIT_PROTOCOL, PLUGIN_ID as BROADCAST_PLUGIN_ID,
};
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode, Task, TaskStatus};
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn workflow_broadcast_plugin_derives_and_executes_target_tasks() {
    let mut host = RuntimeBootstrapper::new();
    let mut loader = BuiltinPluginLoader::new()
        .with_plugin(Box::new(mutsuki_plugin_workflow_broadcast::plugin()))
        .with_plugin(Box::new(mutsuki_plugin_dev_mock::plugin()));
    host.load_plugins(&mut loader).unwrap();
    let runtime = host.into_host_runtime(broadcast_profile()).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "workflow-broadcast",
            BROADCAST_EMIT_PROTOCOL,
            json!({
                "mode": "fire_and_forget",
                "targets": [
                    {"task_id": "broadcast-target-1", "protocol_id": ECHO_PROTOCOL, "payload": {"value": 1}},
                    {"task_id": "broadcast-target-2", "protocol_id": ECHO_PROTOCOL, "payload": {"value": 2}}
                ]
            }),
        ))))
        .unwrap();

    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
        .unwrap();
    let HostRuntimeReply::Idle(_report) = reply else {
        panic!("expected idle reply");
    };

    assert_eq!(
        runtime.task_status("workflow-broadcast"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("broadcast-target-1"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("broadcast-target-2"),
        Some(TaskStatus::Completed)
    );
}

fn broadcast_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "workflow-broadcast".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![BROADCAST_PLUGIN_ID.into(), DEV_PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
