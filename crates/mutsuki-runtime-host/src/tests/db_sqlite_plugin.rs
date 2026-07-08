use std::fs;

use mutsuki_plugin_db_sqlite::{DB_EXECUTE_PROTOCOL, DB_QUERY_PROTOCOL, PLUGIN_ID};
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode, Task, TaskStatus};
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn db_sqlite_plugin_executes_allowlisted_sql() {
    let root = std::env::temp_dir().join("mutsuki-db-sqlite-plugin-test");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let db_path = root.join("test.sqlite");

    let mut host = RuntimeBootstrapper::new();
    let mut loader =
        BuiltinPluginLoader::new().with_plugin(Box::new(mutsuki_plugin_db_sqlite::plugin()));
    host.load_plugins(&mut loader).unwrap();
    let runtime = host.into_host_runtime(sqlite_profile()).unwrap();
    let allowlist = json!([root.to_string_lossy().to_string()]);

    for task in [
        Task::new(
            "db-create",
            DB_EXECUTE_PROTOCOL,
            json!({
                "path": db_path,
                "db_path_allowlist": allowlist,
                "sql": "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL)"
            }),
        ),
        Task::new(
            "db-insert",
            DB_EXECUTE_PROTOCOL,
            json!({
                "path": db_path,
                "db_path_allowlist": allowlist,
                "sql": "INSERT INTO items (name) VALUES (?)",
                "params": ["mutsuki"]
            }),
        ),
        Task::new(
            "db-query",
            DB_QUERY_PROTOCOL,
            json!({
                "path": db_path,
                "db_path_allowlist": allowlist,
                "sql": "SELECT name FROM items WHERE id = ?",
                "params": [1]
            }),
        ),
    ] {
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))
            .unwrap();
    }

    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 16 })
        .unwrap();
    let HostRuntimeReply::Idle(_report) = reply else {
        panic!("expected idle reply");
    };

    assert_eq!(
        runtime.task_status("db-create"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("db-create:effect"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("db-insert"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("db-insert:effect"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(runtime.task_status("db-query"), Some(TaskStatus::Completed));
    assert_eq!(
        runtime.task_status("db-query:effect"),
        Some(TaskStatus::Completed)
    );

    let _ = fs::remove_dir_all(&root);
}

fn sqlite_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "db-sqlite".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
