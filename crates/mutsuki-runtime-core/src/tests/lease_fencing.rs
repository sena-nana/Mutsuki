use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

#[test]
fn stale_runner_completion_is_rejected_after_lease_reclaim() {
    let worker = runner_descriptor("worker", "runtime.lease.input", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new("task-1", "runtime.lease.input", json!({})))
        .unwrap();

    let (_report, mut dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test.scheduler", 1, "test.claim").clamp_to(1))
            },
            Some(2),
        )
        .unwrap();
    assert_eq!(dispatches.len(), 1);
    let stale_lease_id = dispatches[0].task_leases[0].lease_id.clone();
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Running));

    runtime.tick_once().unwrap();
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Ready));
    assert!(runtime.task_events("task-1").iter().any(|event| {
        event.name == "task.lease.expired"
            && event.attributes.get("lease_id")
                == Some(&ScalarValue::String(stale_lease_id.clone()))
    }));

    let dispatch = dispatches.pop().unwrap();
    let mut stale_result = RunnerResult::completed("task-1");
    stale_result
        .tasks
        .push(Task::new("child-1", "runtime.lease.child", json!({})));
    let stale_completion = CompletionBatch {
        batch_id: dispatch.batch.batch_id.clone(),
        tick_id: dispatch.batch.tick_id.clone(),
        results: vec![EntryCompletion {
            entry_id: dispatch.batch.entries[0].entry_id.clone(),
            task_id: "task-1".into(),
            result: Some(stale_result),
            error: None,
        }],
        metadata: Vec::new(),
    };
    let report = runtime
        .complete_runner_dispatch(RunnerCompletion {
            runner: dispatch.runner,
            task_leases: dispatch.task_leases,
            batch_id: dispatch.batch.batch_id.clone(),
            expected_entries: dispatch.batch.entries.clone(),
            result: Ok(stale_completion),
        })
        .unwrap();

    assert_eq!(report.completed_tasks, 0);
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Ready));
    assert_eq!(runtime.task_status("child-1"), None);
    let rejected = runtime
        .task_events("task-1")
        .into_iter()
        .find(|event| event.name == "task.result.rejected")
        .expect("stale result rejection should be recorded");
    assert_eq!(
        rejected.error.as_ref().map(|error| error.code.as_str()),
        Some(ERR_TASK_CLAIM_CONFLICT)
    );
}
