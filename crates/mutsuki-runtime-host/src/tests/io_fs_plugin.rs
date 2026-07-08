use std::fs;

use mutsuki_plugin_io_fs::{
    FS_EXISTS_PROTOCOL, FS_LIST_PROTOCOL, FS_READ_PROTOCOL, FS_STAT_PROTOCOL, FS_WRITE_PROTOCOL,
    PLUGIN_ID,
};
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode, Task, TaskStatus};
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn io_fs_plugin_executes_allowlisted_file_operations() {
    let root = std::env::temp_dir().join("mutsuki-io-fs-plugin-test");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let input = root.join("input.txt");
    let output = root.join("output.txt");
    fs::write(&input, "hello").unwrap();

    let mut host = RuntimeBootstrapper::new();
    let mut loader =
        BuiltinPluginLoader::new().with_plugin(Box::new(mutsuki_plugin_io_fs::plugin()));
    host.load_plugins(&mut loader).unwrap();
    let runtime = host.into_host_runtime(fs_profile()).unwrap();
    let allowlist = json!([root.to_string_lossy().to_string()]);

    for task in [
        Task::new(
            "fs-read",
            FS_READ_PROTOCOL,
            json!({"path": input, "allowlist": allowlist}),
        ),
        Task::new(
            "fs-write",
            FS_WRITE_PROTOCOL,
            json!({"path": output, "content": "written", "allowlist": allowlist}),
        ),
        Task::new(
            "fs-list",
            FS_LIST_PROTOCOL,
            json!({"path": root, "allowlist": allowlist}),
        ),
        Task::new(
            "fs-stat",
            FS_STAT_PROTOCOL,
            json!({"path": input, "allowlist": allowlist}),
        ),
        Task::new(
            "fs-exists-denied",
            FS_EXISTS_PROTOCOL,
            json!({"path": root.parent().unwrap().join("outside.txt"), "allowlist": allowlist}),
        ),
    ] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))
            .unwrap();
    }

    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 24 })
        .unwrap();
    let HostRuntimeReply::Idle(_report) = reply else {
        panic!("expected idle reply");
    };

    assert_eq!(runtime.task_status("fs-read"), Some(TaskStatus::Completed));
    assert_eq!(runtime.task_status("fs-write"), Some(TaskStatus::Completed));
    assert_eq!(runtime.task_status("fs-list"), Some(TaskStatus::Completed));
    assert_eq!(runtime.task_status("fs-stat"), Some(TaskStatus::Completed));
    assert_eq!(
        runtime.task_status("fs-read:effect"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("fs-write:effect"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("fs-exists-denied"),
        Some(TaskStatus::Failed)
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), "written");

    let _ = fs::remove_dir_all(&root);
}

fn fs_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "io-fs".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
