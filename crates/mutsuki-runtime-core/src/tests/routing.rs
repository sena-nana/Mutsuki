use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;
#[test]
fn pure_runner_explicitly_enqueues_derived_tasks() {
    let orchestrator = runner_descriptor("orchestrator", "raw.input.chat", RunnerPurity::Pure);
    let plan = load_plan(vec![orchestrator.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(orchestrator, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            let mut derived = Task::new(
                format!("{}:memory", task.task_id),
                "sim.memory.retrieve",
                json!({"mode": "fast"}),
            );
            derived.priority = 5;
            derived.input_refs = task.input_refs.clone();
            derived.runner_hint = Some("memory.runner".into());
            derived.correlation_id = task.correlation_id.clone();
            result.tasks.push(derived);
            result
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime.publish_raw_input("raw-1", "raw.input.chat", json!({"text": "hello"}));
    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert!(runtime.tasks().get("raw-1:memory").is_some());
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "runner.step")
    );
}

#[test]
fn pure_runner_outputs_are_routed_to_commit_and_effect_tasks() {
    let worker = runner_descriptor("worker", "sim.behavior.evaluate", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.deltas.push(StateDelta {
                target_ref: "state:actor".into(),
                expected_version: 0,
                patch: json!({"intent": "reply"}),
                conflict_policy: ConflictPolicy::Fail,
            });
            result.effects.push(EffectRequest {
                effect_id: "send-1".into(),
                kind: "effect.chat.send".into(),
                payload: json!({"text": "ok"}),
                preconditions: vec![EffectPrecondition {
                    ref_id: "state:actor".into(),
                    expected_version: 0,
                }],
                idempotency_key: Some("send-1".into()),
            });
            result
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.enqueue_task(Task::new("task-1", "sim.behavior.evaluate", json!({})));

    runtime.tick_once().unwrap();

    assert!(runtime.tasks().get("task-1:commit").is_some());
    assert!(runtime.tasks().get("task-1:effect:send-1").is_some());
}

#[test]
fn runner_result_value_and_resource_refs_are_recorded_as_lineage() {
    let worker = runner_descriptor("worker", "sim.resource.produce", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.values.push(ValueRef {
                ref_id: "value:1".into(),
                provider_id: "resource.local".into(),
                schema: "value.small.v1".into(),
                version: 1,
                generation: 1,
                size_hint: Some(12),
                content_hash: Some("hash:value".into()),
                lifetime: ResourceLifetime::Persistent,
                storage: ValueStorage::LocalValueStore,
            });
            result.resources.push(ResourceRef {
                ref_id: "resource:1".into(),
                provider_id: "resource.local".into(),
                resource_kind: "bytes".into(),
                schema: "bytes.v1".into(),
                version: 1,
                generation: 1,
                access: ResourceAccess::MmapFile {
                    path: "resource.bin".into(),
                    offset: 0,
                    len: 3,
                    readonly: true,
                },
                size_hint: Some(3),
                content_hash: Some("hash:resource".into()),
                lifetime: ResourceLifetime::Persistent,
                lease: None,
                seal_state: ResourceSealState::Sealed,
            });
            result
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.enqueue_task(Task::new("task-refs", "sim.resource.produce", json!({})));

    runtime.tick_once().unwrap();

    assert!(runtime.events().iter().any(|event| {
        event.kind == RuntimeEventKind::Resource && event.name == "value.lineage"
    }));
    assert!(runtime.events().iter().any(|event| {
        event.kind == RuntimeEventKind::Resource && event.name == "resource.lineage"
    }));
}

#[test]
fn committer_task_is_the_only_state_store_mutation_path() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![Box::new(CoreKernelRunner::new(1))];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let delta = StateDelta {
        target_ref: "state:actor".into(),
        expected_version: 0,
        patch: json!({"ok": true}),
        conflict_policy: ConflictPolicy::Fail,
    };
    runtime.enqueue_task(Task::new(
        "commit-1",
        "core.commit",
        serde_json::to_value(delta).unwrap(),
    ));

    runtime.tick_once().unwrap();

    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(1, json!({"ok": true}))
    );
}

#[test]
fn runner_trace_records_plugin_generation_and_contract_facts() {
    let worker = runner_descriptor("worker", "sim.trace", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.enqueue_task(Task::new("trace-task", "sim.trace", json!({})));

    runtime.tick_once().unwrap();

    let span = runtime
        .trace_spans()
        .iter()
        .find(|span| {
            span.attributes.get("runner_id") == Some(&ScalarValue::String("worker".into()))
        })
        .unwrap();
    assert_eq!(
        span.attributes.get("plugin_id"),
        Some(&ScalarValue::String("plugin-a".into()))
    );
    assert_eq!(
        span.attributes.get("plugin_generation"),
        Some(&ScalarValue::Int(1))
    );
    assert!(span.attributes.contains_key("artifact_hash"));
    assert!(span.attributes.contains_key("descriptor_hash"));
    assert!(span.attributes.contains_key("contract_fingerprint"));
}
