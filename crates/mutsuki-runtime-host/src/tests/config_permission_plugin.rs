use mutsuki_plugin_config_permission::{
    CONFIG_DESCRIBE_PROTOCOL, PERMISSION_CHECK_PROTOCOL, PLUGIN_ID,
};
use mutsuki_runtime_contracts::{
    RuntimeEventKind, RuntimeProfile, RuntimeProfileMode, Task, TaskStatus,
};
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn config_permission_plugin_loads_and_checks_permissions() {
    let mut host = RuntimeBootstrapper::new();
    let mut loader = BuiltinPluginLoader::new()
        .with_plugin(Box::new(mutsuki_plugin_config_permission::plugin()));
    host.load_plugins(&mut loader).unwrap();
    let runtime = host.into_host_runtime(config_permission_profile()).unwrap();

    for task in [
        Task::new(
            "config-describe",
            CONFIG_DESCRIBE_PROTOCOL,
            json!({"scope": "test"}),
        ),
        Task::new(
            "permission-allowed",
            PERMISSION_CHECK_PROTOCOL,
            json!({
                "request": {"kind": "fs", "path": "C:/workspace/project/file.txt"},
                "grants": {"fs_paths": ["C:/workspace/project"]},
            }),
        ),
        Task::new(
            "permission-denied",
            PERMISSION_CHECK_PROTOCOL,
            json!({
                "request": {"kind": "http", "domain": "denied.example"},
                "grants": {"http_domains": ["allowed.example"]},
            }),
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

    assert_eq!(
        runtime.task_status("config-describe"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("permission-allowed"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("permission-denied"),
        Some(TaskStatus::Failed)
    );
    assert!(runtime.events_after(0).unwrap().iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == PERMISSION_CHECK_PROTOCOL
            && event
                .subject_id
                .as_deref()
                .is_some_and(|subject| subject.contains("permission-allowed"))
    }));
}

fn config_permission_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "config-permission".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
