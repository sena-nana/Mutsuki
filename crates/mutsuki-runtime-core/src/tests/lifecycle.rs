use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

fn execute_dispatch(mut dispatch: RunnerDispatch) -> RunnerCompletion {
    let batch_id = dispatch.batch.batch_id.clone();
    let expected_entries = dispatch.batch.entries.clone();
    let result = dispatch.runner.run_batch(dispatch.ctx, dispatch.batch);
    RunnerCompletion {
        runner: dispatch.runner,
        task_leases: dispatch.task_leases,
        batch_id,
        expected_entries,
        result,
    }
}

#[test]
fn same_step_retry_and_generation_switch_reject_the_old_attempt() {
    let descriptor = runner_descriptor("worker", "runtime.attempt", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue_at(Task::new("task-1", "runtime.attempt", json!({})), 7)
        .unwrap();

    let first = pool.claim_ready_for_executor_with_expiry(&descriptor, "executor-1", 7, 1, 1, None)
        [0]
    .0
    .clone();
    assert_eq!(
        pool.cancel_running_invocation("worker", &first.lease_id, 7),
        1
    );
    pool.get_mut_for_test("task-1").task.registry_generation = 2;
    let second =
        pool.claim_ready_for_executor_with_expiry(&descriptor, "executor-2", 7, 2, 1, None)[0]
            .0
            .clone();

    assert_ne!(first.lease_id, second.lease_id);
    assert_eq!(second.registry_generation, 2);
    assert_eq!(pool.get("task-1").unwrap().attempt_generation, 2);
    assert_eq!(
        pool.complete(&first, 7).unwrap_err().error().code,
        ERR_TASK_CLAIM_CONFLICT
    );
    pool.complete(&second, 7).unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Completed);
    assert_eq!(pool.statistics().ready, 0);
    assert_eq!(pool.statistics().running, 0);
    assert_eq!(pool.statistics().completed, 1);
}

#[test]
fn cancel_invalidates_the_active_attempt_and_rejects_its_completion() {
    let worker = runner_descriptor("worker", "runtime.cancel", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let handle = runtime
        .submit_task(Task::new("cancel-task", "runtime.cancel", json!({})))
        .unwrap();
    let (_, mut dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test", 1, "claim").clamp_to(1))
            },
            None,
        )
        .unwrap();

    runtime.cancel_task_handle(&handle).unwrap();
    assert_eq!(
        runtime.task_handle_status(&handle),
        Some(TaskStatus::Cancelled)
    );
    let report = runtime
        .complete_runner_dispatch(execute_dispatch(dispatches.pop().unwrap()))
        .unwrap();

    assert_eq!(report.completed_tasks, 0);
    assert_eq!(runtime.statistics().tasks.stale_results_rejected, 1);
    assert_eq!(runtime.statistics().tasks.running, 0);
    assert_eq!(runtime.statistics().tasks.cancelled, 1);
    assert_eq!(
        runtime.task_handle_status(&handle),
        Some(TaskStatus::Cancelled)
    );
}

#[test]
fn drain_rejects_new_submissions_but_finishes_already_accepted_work() {
    let worker = runner_descriptor("worker", "runtime.drain", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new("accepted", "runtime.drain", json!({})))
        .unwrap();

    assert_eq!(runtime.begin_drain().unwrap(), RuntimeStopState::Draining);
    let error = runtime
        .submit_task(Task::new("rejected", "runtime.drain", json!({})))
        .unwrap_err();
    assert_eq!(error.error().code, ERR_RUNTIME_NOT_ACCEPTING);

    runtime.run_until_idle(4).unwrap();
    assert_eq!(runtime.task_status("accepted"), Some(TaskStatus::Completed));
    assert!(runtime.is_drained());
}

#[test]
fn abort_invalidates_running_work_and_prevents_later_execution() {
    let worker = runner_descriptor("worker", "runtime.abort", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new("abort-task", "runtime.abort", json!({})))
        .unwrap();
    let (_, mut dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test", 1, "claim").clamp_to(1))
            },
            None,
        )
        .unwrap();

    assert_eq!(runtime.abort("test abort").unwrap(), 1);
    assert_eq!(runtime.stop_state(), RuntimeStopState::Aborted);
    assert_eq!(
        runtime.task_status("abort-task"),
        Some(TaskStatus::Cancelled)
    );
    let report = runtime
        .complete_runner_dispatch(execute_dispatch(dispatches.pop().unwrap()))
        .unwrap();

    assert_eq!(report.completed_tasks, 0);
    assert_eq!(runtime.statistics().tasks.stale_results_rejected, 1);
    assert_eq!(runtime.statistics().tasks.running, 0);
    assert_eq!(runtime.statistics().tasks.cancelled, 1);
    assert_eq!(
        runtime
            .submit_task(Task::new("after-abort", "runtime.abort", json!({})))
            .unwrap_err()
            .error()
            .code,
        ERR_RUNTIME_ABORTED
    );
    assert_eq!(
        runtime.tick_once().unwrap_err().error().code,
        ERR_RUNTIME_ABORTED
    );
}

#[test]
fn bounded_observation_drops_events_without_blocking_task_execution() {
    let worker = runner_descriptor("worker", "runtime.observe", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.configure_event_capacity(1);
    runtime
        .submit_task(Task::new("observed", "runtime.observe", json!({})))
        .unwrap();

    runtime.run_until_idle(4).unwrap();

    let statistics = runtime.statistics();
    assert_eq!(runtime.task_status("observed"), Some(TaskStatus::Completed));
    assert_eq!(statistics.retained_events, 1);
    assert!(statistics.dropped_events > 0);
    assert_eq!(statistics.tasks.ready, 0);
    assert_eq!(statistics.tasks.running, 0);
    assert_eq!(statistics.tasks.completed, 1);
    assert_eq!(statistics.tasks.attempts_started, 1);
    assert!(statistics.tasks.cumulative_queue_steps >= 1);
    assert!(statistics.tasks.cumulative_execution_steps >= 1);
}

#[test]
fn disabled_event_outlet_keeps_task_execution_independent_from_observation() {
    let worker = runner_descriptor("worker", "runtime.no-events", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.configure_event_capacity(0);
    runtime
        .submit_task(Task::new("no-events", "runtime.no-events", json!({})))
        .unwrap();

    runtime.run_until_idle(4).unwrap();

    assert_eq!(
        runtime.task_status("no-events"),
        Some(TaskStatus::Completed)
    );
    let statistics = runtime.statistics();
    assert_eq!(statistics.retained_events, 0);
    assert!(statistics.dropped_events > 0);
    assert_eq!(statistics.tasks.completed, 1);
}

#[test]
fn constant_cost_statistics_track_failed_tasks_without_sampling() {
    let worker = runner_descriptor("worker", "runtime.fail", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.status = RunnerStatus::Failed;
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new("failed", "runtime.fail", json!({})))
        .unwrap();

    runtime.run_until_idle(4).unwrap();

    let statistics = runtime.statistics().tasks;
    assert_eq!(statistics.ready, 0);
    assert_eq!(statistics.running, 0);
    assert_eq!(statistics.completed, 0);
    assert_eq!(statistics.failed, 1);
    assert!(
        runtime
            .events()
            .iter()
            .any(|event| event.name == "task.failed"
                && event.subject_id.as_deref() == Some("failed"))
    );
}

#[test]
fn task_events_cover_submitted_started_progress_and_terminal_states() {
    let worker = runner_descriptor("worker", "runtime.progress", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.status = RunnerStatus::Waiting;
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let handle = runtime
        .submit_task(Task::new("progress-task", "runtime.progress", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();
    assert_eq!(runtime.statistics().tasks.waiting, 1);
    runtime.cancel_task_handle(&handle).unwrap();
    assert_eq!(runtime.statistics().tasks.waiting, 0);
    assert_eq!(runtime.statistics().tasks.cancelled, 1);

    let names: Vec<_> = runtime
        .task_handle_events(&handle)
        .into_iter()
        .map(|event| event.name.as_str())
        .collect();
    assert!(names.contains(&"task.submitted"));
    assert!(names.contains(&"task.started"));
    assert!(names.contains(&"task.progress"));
    assert!(names.contains(&"task.cancelled"));
}
