use mutsuki_plugin_observe_log::{LOG_EMIT_PROTOCOL, PLUGIN_ID};
use mutsuki_runtime_contracts::{RuntimeEventKind, RuntimeProfile, RuntimeProfileMode, Task};
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn observe_log_plugin_loads_and_appends_domain_event() {
    let mut host = RuntimeBootstrapper::new();
    let mut loader =
        BuiltinPluginLoader::new().with_plugin(Box::new(mutsuki_plugin_observe_log::plugin()));
    host.load_plugins(&mut loader).unwrap();
    let runtime = host.into_host_runtime(observe_profile()).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "log-task",
            LOG_EMIT_PROTOCOL,
            json!({"message": "hello"}),
        ))))
        .unwrap();
    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();
    let HostRuntimeReply::Idle(_report) = reply else {
        panic!("expected idle reply");
    };

    assert!(runtime.events_after(0).unwrap().iter().any(|event| {
        event.kind == RuntimeEventKind::Task
            && event.name == LOG_EMIT_PROTOCOL
            && event
                .subject_id
                .as_deref()
                .is_some_and(|subject| subject.contains("log-task"))
    }));
}

fn observe_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "observe-log".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
