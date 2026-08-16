use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

fn state_delta(expected_version: u64, patch: serde_json::Value) -> StateDelta {
    StateDelta {
        target_ref: "state:actor".into(),
        expected_version,
        patch,
        conflict_policy: ConflictPolicy::Fail,
    }
}

fn commit_task(task_id: &str, delta: StateDelta) -> Task {
    Task::new(
        task_id,
        "core.commit",
        serde_json::to_value(delta).expect("StateDelta serializes"),
    )
}

fn rollback_task(task_id: &str, to_version: u64) -> Task {
    Task::new(
        task_id,
        "core.state.rollback",
        serde_json::to_value(StateRollback {
            target_ref: "state:actor".into(),
            to_version,
        })
        .expect("StateRollback serializes"),
    )
}

fn history_plan(capacity_per_ref: usize, retain_steps: u64) -> RuntimeLoadPlan {
    let mut plan = load_plan(Vec::new(), Vec::new());
    plan.observability.state_history = StateHistoryProfile::bounded(capacity_per_ref, retain_steps);
    plan
}

fn assert_failed(runtime: &CoreRuntime, task_id: &str, code: &str) {
    assert_eq!(runtime.task_status(task_id), Some(TaskStatus::Failed));
    let Some(TaskOutcome::Failed { error, .. }) = runtime.task_outcome(task_id).unwrap() else {
        panic!("{task_id} should have failed");
    };
    assert_eq!(error.code, code);
}

#[test]
fn kernel_commit_failures_fail_the_task_without_rewinding_state() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!();
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(commit_task(
            "commit-ok",
            state_delta(0, json!({"status": "kept"})),
        ))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(commit_task(
            "commit-stale",
            state_delta(0, json!({"status": "should-not-apply"})),
        ))
        .unwrap();
    runtime
        .enqueue_task(Task::new(
            "commit-bad",
            "core.commit",
            json!({"not": "a-state-delta"}),
        ))
        .unwrap();
    runtime.run_until_idle(4).unwrap();

    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(1, json!({"status": "kept"}))
    );
    assert_eq!(
        runtime.task_status("commit-ok"),
        Some(TaskStatus::Completed)
    );
    assert_failed(&runtime, "commit-stale", ERR_STATE_CONFLICT);
    assert_failed(&runtime, "commit-bad", "state.delta_decode_failed");
}

#[test]
fn sibling_commits_are_independent_kernel_tasks() {
    let worker = runner_descriptor("worker", "sim.unit", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.deltas.push(state_delta(0, json!({"step": "first"})));
        result.deltas.push(StateDelta {
            target_ref: "state:actor".into(),
            expected_version: 0,
            patch: json!({"step": "second"}),
            conflict_policy: ConflictPolicy::Merge,
        });
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("unit-1", "sim.unit", json!({})))
        .unwrap();
    runtime.run_until_idle(4).unwrap();

    assert_eq!(runtime.task_status("unit-1"), Some(TaskStatus::Completed));
    assert_eq!(
        runtime.task_status("unit-1:commit:0"),
        Some(TaskStatus::Completed)
    );
    assert_failed(&runtime, "unit-1:commit:1", ERR_STATE_CONFLICT);
    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(1, json!({"step": "first"}))
    );
}

#[test]
fn state_rollback_is_optional_and_expires_with_retain_steps() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!();
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(commit_task("seed", state_delta(0, json!({"value": "now"}))))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(rollback_task("rollback-off", 1))
        .unwrap();
    runtime.tick_once().unwrap();
    assert_failed(&runtime, "rollback-off", ERR_STATE_HISTORY_DISABLED);

    let mut runtime = CoreRuntime::boot(history_plan(8, 32), runners_with_kernel!()).unwrap();
    runtime
        .enqueue_task(commit_task("v1", state_delta(0, json!({"value": "old"}))))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(commit_task("v2", state_delta(1, json!({"value": "new"}))))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(rollback_task("rollback-on", 1))
        .unwrap();
    runtime.tick_once().unwrap();
    assert_eq!(
        runtime.task_status("rollback-on"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(1, json!({"value": "old"}))
    );

    let mut runtime = CoreRuntime::boot(history_plan(8, 1), runners_with_kernel!()).unwrap();
    runtime
        .enqueue_task(commit_task("v1", state_delta(0, json!({"value": "old"}))))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(commit_task("v2", state_delta(1, json!({"value": "new"}))))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(rollback_task("rollback-expired", 1))
        .unwrap();
    runtime.tick_once().unwrap();
    assert_failed(&runtime, "rollback-expired", ERR_STATE_HISTORY_UNAVAILABLE);
    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(2, json!({"value": "new"}))
    );
}

#[test]
fn plugin_reload_retains_state_and_can_roll_back_when_history_stays_enabled() {
    let mut runtime = CoreRuntime::boot(history_plan(8, 32), runners_with_kernel!()).unwrap();
    runtime
        .enqueue_task(commit_task("v1", state_delta(0, json!({"owned": "v1"}))))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(commit_task("v2", state_delta(1, json!({"owned": "v2"}))))
        .unwrap();
    runtime.tick_once().unwrap();

    let mut plan_v2 = history_plan(8, 32);
    plan_v2.registry_generation = 2;
    runtime
        .reload_with_runners(plan_v2, runners_with_kernel!(2;))
        .unwrap();
    runtime
        .enqueue_task(rollback_task("rollback-after-reload", 1))
        .unwrap();
    runtime.tick_once().unwrap();

    assert_eq!(
        runtime.task_status("rollback-after-reload"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(1, json!({"owned": "v1"}))
    );
}

#[test]
fn forged_commit_fails_and_unknown_protocol_stays_ready() {
    let worker = runner_descriptor("worker", "sim.derive", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result
            .tasks
            .push(Task::new("ghost-1", "unknown.protocol", json!({"x": 1})));
        result.tasks.push(Task::new(
            "forged-commit",
            "core.commit",
            json!({"not": "a-state-delta"}),
        ));
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "sim.derive", json!({})))
        .unwrap();
    runtime.run_until_idle(4).unwrap();

    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Completed));
    assert_eq!(runtime.task_status("ghost-1"), Some(TaskStatus::Ready));
    assert_failed(&runtime, "forged-commit", "state.delta_decode_failed");
    assert!(runtime.state_value("state:actor").is_none());
}
