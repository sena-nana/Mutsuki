use std::collections::BTreeMap;
use std::io::Cursor;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{CoreRuntime, Runner, RunnerContext};
use serde_json::json;

use crate::{JsonlRunner, NativePluginHost, NativeRunner, runner_manifest};

fn descriptor(id: &str, kind: &str) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: id.into(),
        plugin_id: "plugin-a".into(),
        plugin_generation: 1,
        accepted_task_kinds: vec![kind.into()],
        purity: RunnerPurity::Pure,
        input_schema: json!({}),
        output_schema: json!({}),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{id}")],
    }
}

#[test]
fn native_plugin_host_boots_runtime_and_runs_runner_loop() {
    let runner_descriptor = descriptor("echo.runner", "raw.input");
    let mut host = NativePluginHost::new();
    host.register_manifest(runner_manifest("plugin-a", vec![runner_descriptor.clone()]));
    host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx: RunnerContext, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));
    let runtime_profile = RuntimeProfile {
        profile_id: "default".into(),
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    };
    let mut runtime: CoreRuntime = host.into_runtime(runtime_profile).unwrap();

    runtime.enqueue_task(Task::new("task-1", "raw.input", json!({"ok": true})));
    let report = runtime.run_until_idle(4).unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(
        runtime.tasks().get("task-1").unwrap().status,
        TaskStatus::Completed
    );
}

#[test]
fn jsonl_runner_uses_runner_step_method_surface() {
    let runner_descriptor = descriptor("jsonl.runner", "raw.input");
    let result = vec![RunnerResult::completed("task-1")];
    let response = format!("{}\n", json!({"id":"req-1","ok":true,"result": result}));
    let reader = Cursor::new(response.into_bytes());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = JsonlRunner::new(runner_descriptor, reader, writer);

    let results = runner
        .step(
            RunnerContext {
                registry_generation: 1,
                current_step: 1,
            },
            vec![Task::new("task-1", "raw.input", json!({}))],
        )
        .unwrap();
    let (_reader, writer) = runner.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert_eq!(results[0].task_id, "task-1");
    assert!(request.contains("\"method\":\"runner.step\""));
    assert!(request.contains("\"registry_generation\":1"));
}
