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
        accepted_protocol_ids: vec![kind.into()],
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
                executor_id: "executor:test".into(),
                task_lease_id: Some("task-lease-test".into()),
            },
            vec![Task::new("task-1", "raw.input", json!({}))],
        )
        .unwrap();
    let (_reader, writer) = runner.into_inner();
    let request = String::from_utf8(writer.into_inner()).unwrap();

    assert_eq!(results[0].task_id, "task-1");
    assert!(request.contains("\"method\":\"runner.step\""));
    assert!(request.contains("\"registry_generation\":1"));
    assert!(request.contains("\"executor_id\":\"executor:test\""));
    assert!(request.contains("\"task_lease_id\":\"task-lease-test\""));
}

#[test]
fn resolver_emits_declared_runtime_surfaces() {
    let runner_descriptor = descriptor("echo.runner", "raw.input");
    let mut manifest = runner_manifest("plugin-a", vec![runner_descriptor]);
    manifest.provides.protocols = vec![ProtocolDescriptor {
        protocol_id: "im.message.received.v1".into(),
        version: "1.0.0".into(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        error_schema: json!({"type": "object"}),
        codec: "json".into(),
        compatibility: "semver".into(),
    }];
    manifest.provides.handler_bindings = vec![HandlerBinding {
        binding_id: "message-handler".into(),
        plugin_id: "plugin-a".into(),
        protocol_id: "im.message.received.v1".into(),
        target_protocol_id: "raw.input".into(),
        target_runner_hint: Some("echo.runner".into()),
        pool_id: "default".into(),
        priority: 1,
        policy: "required".into(),
        metadata: BTreeMap::new(),
    }];
    manifest.provides.resource_schemas = vec!["bytes.v1".into()];
    manifest.provides.resource_providers = vec!["resource.local".into()];
    manifest.provides.effects = vec!["effect.chat.send".into()];
    manifest.provides.streams = vec!["chat.events".into()];
    manifest.provides.subscriptions = vec!["chat.messages".into()];
    manifest.provides.timers = vec!["heartbeat".into()];
    manifest.provides.state_schemas = vec!["state.actor.v1".into()];
    let profile = RuntimeProfile {
        profile_id: "default".into(),
        enabled_plugins: vec!["plugin-a".into()],
        bindings: BTreeMap::new(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    };

    let plan = crate::resolve_load_plan(&[manifest], &profile);

    assert_surface(
        &plan,
        "protocol:im.message.received.v1",
        ContractSurfaceKind::Protocol,
    );
    assert_surface(
        &plan,
        "handler_binding:message-handler",
        ContractSurfaceKind::HandlerBinding,
    );
    assert_surface(
        &plan,
        "resource_schema:bytes.v1",
        ContractSurfaceKind::ResourceSchema,
    );
    assert_surface(
        &plan,
        "resource_provider:resource.local",
        ContractSurfaceKind::ResourceProvider,
    );
    assert_surface(
        &plan,
        "effect:effect.chat.send",
        ContractSurfaceKind::Effect,
    );
    assert_surface(&plan, "stream:chat.events", ContractSurfaceKind::Stream);
    assert_surface(
        &plan,
        "subscription:chat.messages",
        ContractSurfaceKind::Subscription,
    );
    assert_surface(&plan, "timer:heartbeat", ContractSurfaceKind::Timer);
    assert_surface(
        &plan,
        "state_schema:state.actor.v1",
        ContractSurfaceKind::StateSchema,
    );
}

fn assert_surface(plan: &RuntimeLoadPlan, surface_id: &str, kind: ContractSurfaceKind) {
    assert!(
        plan.contract_surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.kind == kind),
        "missing surface {surface_id}"
    );
}
