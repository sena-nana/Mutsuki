use mutsuki_plugin_dev_mock::{
    ECHO_PROTOCOL, FAIL_PROTOCOL, PLUGIN_ID, PRODUCE_RESOURCE_PROTOCOL, RANDOM_FAIL_PROTOCOL,
    SLEEP_PROTOCOL,
};
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode, Task, TaskStatus};
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn dev_mock_plugin_loads_and_executes_batch_protocols() {
    let mut host = RuntimeBootstrapper::new();
    let mut loader =
        BuiltinPluginLoader::new().with_plugin(Box::new(mutsuki_plugin_dev_mock::plugin()));
    host.load_plugins(&mut loader).unwrap();
    let runtime = host.into_host_runtime(dev_mock_profile()).unwrap();

    for task in [
        Task::new("dev-echo", ECHO_PROTOCOL, json!({"value": 1})),
        Task::new("dev-sleep", SLEEP_PROTOCOL, json!({"duration_ms": 10})),
        Task::new("dev-fail", FAIL_PROTOCOL, json!({"reason": "expected"})),
        Task::new(
            "dev-random-fail",
            RANDOM_FAIL_PROTOCOL,
            json!({"fail_modulus": 1, "seed": "deterministic"}),
        ),
        Task::new(
            "dev-resource",
            PRODUCE_RESOURCE_PROTOCOL,
            json!({"ref_id": "dev-resource-1"}),
        ),
    ] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))
            .unwrap();
    }

    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    let HostRuntimeReply::Idle(_report) = reply else {
        panic!("expected idle reply");
    };

    assert_eq!(runtime.task_status("dev-echo"), Some(TaskStatus::Completed));
    assert_eq!(
        runtime.task_status("dev-sleep"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(runtime.task_status("dev-fail"), Some(TaskStatus::Failed));
    assert_eq!(
        runtime.task_status("dev-random-fail"),
        Some(TaskStatus::Failed)
    );
    assert_eq!(
        runtime.task_status("dev-resource"),
        Some(TaskStatus::Completed)
    );
    assert!(runtime.events_after(0).unwrap().iter().any(|event| {
        event.name == "resource.lineage" && event.subject_id.as_deref() == Some("dev-resource")
    }));
}

fn dev_mock_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "dev-mock".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
