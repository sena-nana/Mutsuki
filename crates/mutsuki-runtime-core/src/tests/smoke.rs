use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

#[test]
fn core_runtime_smoke_routes_runner_outputs_through_kernel_and_effect_runner() {
    let worker = runner_descriptor("worker", "runtime.smoke.input", RunnerPurity::Pure);
    let effect_runner =
        runner_descriptor("effect.smoke", "effect.chat.send", RunnerPurity::Effectful);
    let plan = load_plan(vec![worker.clone(), effect_runner.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(
        boxed_runner!(worker, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.deltas.push(StateDelta {
                target_ref: "state:smoke".into(),
                expected_version: 0,
                patch: json!({"status": "committed"}),
                conflict_policy: ConflictPolicy::Fail,
            });
            result.events.push(DomainEvent {
                event_id: "domain-1".into(),
                kind: "domain.smoke".into(),
                payload: json!({"task_id": task.task_id}),
            });
            result.effects.push(EffectRequest {
                effect_id: "effect-1".into(),
                kind: "effect.chat.send".into(),
                payload: json!({"status": "sent"}),
                preconditions: Vec::new(),
                idempotency_key: Some("effect-1".into()),
            });
            result
        }),
        boxed_runner!(effect_runner, |task| {
            assert_eq!(task.protocol_id, "effect.chat.send");
            RunnerResult::completed(task.task_id.clone())
        })
    );
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime
        .submit_task(Task::new(
            "smoke-1",
            "runtime.smoke.input",
            json!({"input": true}),
        ))
        .unwrap();
    runtime.run_until_idle(8).unwrap();

    assert_eq!(runtime.task_status("smoke-1"), Some(TaskStatus::Completed));
    assert_eq!(
        runtime.task_status("smoke-1:commit"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("smoke-1:event:domain-1"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("smoke-1:effect:effect-1"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.state_value("state:smoke").unwrap(),
        &(1, json!({"status": "committed"}))
    );
    assert!(
        runtime
            .events()
            .iter()
            .any(|event| { event.kind == RuntimeEventKind::Task && event.name == "domain.smoke" })
    );
    assert!(runtime.trace_spans().iter().any(|span| {
        span.name == "runner.step"
            && span.attributes.get("runner_id") == Some(&ScalarValue::String("worker".into()))
    }));
}
