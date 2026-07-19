use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;
#[test]
fn task_pool_claims_ready_tasks_in_deterministic_order() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    let mut low = Task::new("b-low", "sim.work", json!({}));
    low.priority = 1;
    let mut high = Task::new("a-high", "sim.work", json!({}));
    high.priority = 10;
    let mut future = Task::new("future", "sim.work", json!({}));
    future.priority = 99;
    future.ready_at_step = Some(9);
    pool.enqueue(low).unwrap();
    pool.enqueue(high).unwrap();
    pool.enqueue(future).unwrap();

    let claimed = pool.claim_ready(&descriptor, 1, 0, 8);
    assert_eq!(
        claimed
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a-high", "b-low"]
    );
    assert_eq!(pool.running_count(), 2);
}

#[test]
fn task_pool_reports_earliest_indexed_future_step_without_scanning_records() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    let mut later = Task::new("later", "sim.work", json!({}));
    later.ready_at_step = Some(12);
    let mut earlier = Task::new("earlier", "sim.work", json!({}));
    earlier.ready_at_step = Some(7);
    pool.enqueue(later).unwrap();
    pool.enqueue(earlier).unwrap();

    assert_eq!(pool.next_required_step_after(0), Some(7));
    assert_eq!(pool.next_required_step_after(7), Some(12));

    let claimed = pool.claim_ready_for_executor(&descriptor, "executor", 7, 0, 1);
    pool.complete(&claimed[0].0, 7).unwrap();
    assert_eq!(pool.next_required_step_after(7), Some(12));
    pool.assert_indexes_consistent();
}

#[test]
fn task_pool_claims_single_task_with_executor_lease() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();
    pool.enqueue(Task::new("task-2", "sim.work", json!({})))
        .unwrap();

    let leased = pool.claim_ready_for_executor(&descriptor, "executor-1", 1, 0, 1);

    assert_eq!(leased.len(), 1);
    let (lease, task) = &leased[0];
    assert_eq!(lease.task_id, "task-1");
    assert_eq!(lease.runner_id, "worker");
    assert_eq!(lease.executor_id, "executor-1");
    assert_eq!(lease.expires_at_step, Some(2));
    assert_eq!(task.lease_id.as_deref(), Some(lease.lease_id.as_str()));
    assert_eq!(pool.running_count(), 1);
    assert_eq!(
        pool.get("task-1").unwrap().lease.as_ref().unwrap().lease_id,
        lease.lease_id
    );
}

#[test]
fn task_pool_rejects_duplicate_task_id_without_overwriting_record() {
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({"first": true})))
        .unwrap();

    let error = pool
        .enqueue(Task::new("task-1", "sim.work", json!({"second": true})))
        .unwrap_err();

    assert_eq!(error.error().code, ERR_TASK_DUPLICATE);
    assert_eq!(
        pool.get("task-1").unwrap().task.payload,
        json!({"first": true})
    );
}

#[test]
fn bounded_terminal_history_evicts_records_and_keeps_a_bounded_id_horizon() {
    let mut pool = TaskPool::default();
    pool.configure_history_retention(Some(TaskHistoryRetention::new(2, 3)));

    for index in 0..5 {
        let task_id = format!("task-{index}");
        pool.enqueue(Task::new(&task_id, "sim.work", json!({})))
            .unwrap();
        pool.cancel_by_core(&task_id, 0).unwrap();
    }

    assert_eq!(pool.records().len(), 2);
    assert_eq!(pool.retained_terminal_records(), 2);
    assert_eq!(pool.evicted_task_id_count(), 3);
    assert_eq!(pool.statistics().cancelled, 2);
    assert_eq!(pool.statistics().submitted_total, 5);
    assert_eq!(pool.statistics().terminal_records_evicted, 3);
    assert_eq!(
        pool.enqueue(Task::new("task-0", "sim.work", json!({})))
            .unwrap_err()
            .error()
            .code,
        ERR_TASK_DUPLICATE
    );

    pool.enqueue(Task::new("task-5", "sim.work", json!({})))
        .unwrap();
    pool.cancel_by_core("task-5", 0).unwrap();
    pool.enqueue(Task::new("task-0", "sim.work", json!({})))
        .unwrap();
}

#[test]
fn terminal_child_is_retained_until_waiters_are_consumed() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.configure_history_retention(Some(TaskHistoryRetention::new(0, 8)));
    let mut parent = Task::new("parent", "sim.work", json!({}));
    parent.trace_id = Some("trace".into());
    let mut child = Task::new("child", "sim.work", json!({}));
    child.trace_id = Some("trace".into());
    pool.enqueue(parent).unwrap();
    pool.enqueue(child).unwrap();
    let parent_lease = pool
        .claim_ready_for_executor(&descriptor, "executor", 1, 0, 1)
        .remove(0)
        .0;
    let child_handle = TaskHandle {
        task_id: "child".into(),
        protocol_id: "sim.work".into(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: Some("trace".into()),
        correlation_id: None,
    };
    pool.wait_on_task(
        &parent_lease,
        1,
        TaskAwait {
            parent_task_id: "parent".into(),
            child: child_handle,
            continuation: task_pool_test_continuation("continuation"),
            cancel_policy: CancelPolicy::Cascade,
        },
    )
    .unwrap();
    let child_lease = pool
        .claim_ready_for_executor(&descriptor, "executor", 1, 0, 1)
        .remove(0)
        .0;

    pool.complete(&child_lease, 1).unwrap();
    assert!(pool.get("child").is_some());
    assert_eq!(pool.take_waits_for_child("child").len(), 1);
    assert!(pool.get("child").is_none());
}

#[test]
fn task_pool_wait_block_and_wake_are_single_task_state_changes() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();
    let lease = pool.claim_ready_for_executor(&descriptor, "executor-1", 1, 0, 1)[0]
        .0
        .clone();

    pool.wait(&lease, 1, Some(8)).unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Waiting);
    assert_eq!(pool.get("task-1").unwrap().task.ready_at_step, Some(8));
    assert!(pool.get("task-1").unwrap().task.lease_id.is_none());

    pool.wake("task-1", 1).unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Ready);
    let lease = pool.claim_ready_for_executor(&descriptor, "executor-1", 8, 0, 1)[0]
        .0
        .clone();

    pool.block(&lease, 8).unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Blocked);
    pool.wake("task-1", 8).unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Ready);
}

#[test]
fn waiting_tasks_count_toward_runner_inflight_load() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();
    pool.enqueue(Task::new("task-2", "sim.work", json!({})))
        .unwrap();
    let lease = pool.claim_ready_for_executor(&descriptor, "executor-1", 1, 0, 1)[0]
        .0
        .clone();

    pool.wait(&lease, 1, None).unwrap();
    let load = pool.runner_load(&descriptor, 1, 0);

    assert_eq!(pool.waiting_count(), 1);
    assert_eq!(load.running_count, 0);
    assert_eq!(load.waiting_count, 1);
    assert_eq!(load.queued_count, 1);
    assert_eq!(load.pending_weight, 2);
}

#[test]
fn runner_load_uses_due_generation_and_selector_counts_without_claim_scan() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    let mut due = Task::new("due", "sim.work", json!({}));
    due.registry_generation = 7;
    let mut future = Task::new("future", "sim.work", json!({}));
    future.registry_generation = 7;
    future.ready_at_step = Some(9);
    let mut stale = Task::new("stale", "sim.work", json!({}));
    stale.registry_generation = 6;
    let mut other_hint = Task::new("other-hint", "sim.work", json!({}));
    other_hint.registry_generation = 7;
    other_hint.runner_hint = Some("other".into());
    for task in [due, future, stale, other_hint] {
        pool.enqueue(task).unwrap();
    }

    assert_eq!(pool.runner_load(&descriptor, 1, 7).queued_count, 1);
    assert_eq!(pool.runner_load(&descriptor, 9, 7).queued_count, 2);
    assert_eq!(pool.runner_load(&descriptor, 9, 6).queued_count, 1);

    pool.claim_ready_for_executor(&descriptor, "executor", 1, 7, 1);
    assert_eq!(pool.runner_load(&descriptor, 1, 7).queued_count, 0);
    pool.assert_indexes_consistent();
}

#[test]
fn woken_continuation_can_only_be_reclaimed_by_owner_runner() {
    let owner = runner_descriptor("owner.runner", "sim.work", RunnerPurity::Pure);
    let alternate = runner_descriptor("alternate.runner", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();
    let lease = pool.claim_ready_for_executor(&owner, "executor-1", 1, 0, 1)[0]
        .0
        .clone();
    pool.wait(&lease, 1, None).unwrap();
    pool.wake("task-1", 2).unwrap();

    assert!(
        pool.claim_ready_for_executor(&alternate, "executor-2", 2, 0, 1)
            .is_empty()
    );
    let claimed = pool.claim_ready_for_executor(&owner, "executor-1", 2, 0, 1);

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].1.task_id, "task-1");
}

#[test]
fn task_pool_wakes_due_waiting_tasks_and_clears_wait_links() {
    let owner = runner_descriptor("owner.runner", "sim.work", RunnerPurity::Pure);
    let alternate = runner_descriptor("alternate.runner", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("parent-1", "sim.work", json!({})))
        .unwrap();
    pool.enqueue(Task::new("child-1", "child.work", json!({})))
        .unwrap();
    let lease = pool.claim_ready_for_executor(&owner, "executor-1", 1, 0, 1)[0]
        .0
        .clone();
    let child = TaskHandle {
        task_id: "child-1".into(),
        protocol_id: "child.work".into(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: None,
        correlation_id: None,
    };
    let mut continuation = task_pool_test_continuation("continuation:parent");
    continuation.wake = Some(WakeCondition::Timer { ready_at_step: 5 });
    pool.wait_on_task(
        &lease,
        1,
        TaskAwait {
            parent_task_id: "parent-1".into(),
            child,
            continuation,
            cancel_policy: CancelPolicy::Cascade,
        },
    )
    .unwrap();

    assert!(pool.wake_due_tasks(4).is_empty());
    assert_eq!(pool.get("parent-1").unwrap().status, TaskStatus::Waiting);

    assert_eq!(pool.wake_due_tasks(5), vec![("parent-1".into(), 5)]);
    assert_eq!(pool.get("parent-1").unwrap().status, TaskStatus::Ready);
    assert!(pool.awaits_for_parent("parent-1").is_empty());
    assert!(pool.take_waits_for_child("child-1").is_empty());
    assert!(
        pool.claim_ready_for_executor(&alternate, "executor-2", 5, 0, 1)
            .is_empty()
    );
    let claimed = pool.claim_ready_for_executor(&owner, "executor-1", 5, 0, 1);
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].1.task_id, "parent-1");
}

#[test]
fn task_pool_reclaims_expired_running_leases_to_ready() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();
    let lease = pool.claim_ready_for_executor(&descriptor, "executor-1", 3, 0, 1)[0]
        .0
        .clone();

    assert_eq!(pool.reclaim_expired_task_leases(3).len(), 0);
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Running);
    assert_eq!(pool.reclaim_expired_task_leases(4).len(), 1);
    let record = pool.get("task-1").unwrap();
    assert_eq!(record.status, TaskStatus::Ready);
    assert!(record.lease.is_none());
    assert!(record.task.lease_id.is_none());

    let error = pool.complete(&lease, 4).unwrap_err();
    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);
}

#[test]
fn task_pool_rejects_stale_or_mismatched_lease_commits() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();
    let stale_lease = pool.claim_ready_for_executor(&descriptor, "executor-1", 1, 1, 1)[0]
        .0
        .clone();
    pool.reclaim_expired_task_leases(2);
    let fresh_lease = pool.claim_ready_for_executor(&descriptor, "executor-2", 2, 1, 1)[0]
        .0
        .clone();

    let error = pool.complete(&stale_lease, 2).unwrap_err();
    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);

    let mut mismatched_executor = fresh_lease.clone();
    mismatched_executor.executor_id = "executor-other".into();
    let error = pool.complete(&mismatched_executor, 2).unwrap_err();
    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);

    let mut mismatched_generation = fresh_lease.clone();
    mismatched_generation.registry_generation = 99;
    let error = pool.complete(&mismatched_generation, 2).unwrap_err();
    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);

    pool.complete(&fresh_lease, 2).unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Completed);
}

#[test]
fn task_pool_core_terminal_states_release_lease_and_owner() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();
    let lease = pool.claim_ready_for_executor(&descriptor, "executor-1", 1, 0, 1)[0]
        .0
        .clone();

    pool.expire_by_core(
        "task-1",
        crate::runtime_error(ERR_TASK_EXPIRED, "test", "task.expire"),
        1,
    )
    .unwrap();

    let record = pool.get("task-1").unwrap();
    assert_eq!(record.status, TaskStatus::Expired);
    assert!(record.lease.is_none());
    assert!(record.claimed_by.is_none());
    assert!(record.owner_runner.is_none());
    assert!(record.task.lease_id.is_none());
    assert!(pool.complete(&lease, 1).is_err());
    assert!(pool.cancel_by_core("task-1", 1).is_err());
}

#[test]
fn task_pool_dead_letter_is_terminal() {
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    pool.dead_letter_by_core(
        "task-1",
        crate::runtime_error(ERR_TASK_DEAD_LETTER, "test", "task.dead_letter"),
        0,
    )
    .unwrap();

    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::DeadLetter);
    assert!(pool.wake("task-1", 0).is_err());
}

#[test]
fn core_task_facade_returns_result_snapshot_and_task_events() {
    let plan = super::fixtures::load_plan(Vec::new(), Vec::new());
    let mut runtime = super::fixtures::boot_with_kernel(plan);

    runtime
        .submit_task(Task::new("task-1", "unhandled.protocol", json!({})))
        .unwrap();
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Ready));
    assert_eq!(
        runtime.task_result("task-1").unwrap().status,
        TaskStatus::Ready
    );
    assert!(
        runtime
            .task_events("task-1")
            .iter()
            .any(|event| event.name == "task.enqueue")
    );
    runtime.cancel_task_by_id("task-1").unwrap();
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Cancelled));
}

#[test]
fn task_pool_rejects_generation_mismatched_claims() {
    let mut pool = TaskPool::default();
    let pure = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);

    let mut work = Task::new("work-1", "sim.work", json!({}));
    work.registry_generation = 1;
    pool.enqueue(work).unwrap();

    assert!(pool.claim_ready(&pure, 1, 2, 8).is_empty());

    assert_eq!(pool.rebind_ready_generation(1, 2), 1);
    let claimed = pool.claim_ready(&pure, 1, 2, 8);
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id, "work-1");
}

#[test]
fn task_pool_only_claims_ready_tasks() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let waiting_runner = runner_descriptor("waiting", "sim.wait", RunnerPurity::Pure);
    let blocked_runner = runner_descriptor("blocked", "sim.block", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("ready", "sim.work", json!({})))
        .unwrap();
    pool.enqueue(Task::new("waiting", "sim.wait", json!({})))
        .unwrap();
    pool.enqueue(Task::new("blocked", "sim.block", json!({})))
        .unwrap();
    let waiting = pool.claim_ready_for_executor(&waiting_runner, "executor", 1, 0, 1)[0]
        .0
        .clone();
    pool.wait(&waiting, 1, None).unwrap();
    let blocked = pool.claim_ready_for_executor(&blocked_runner, "executor", 1, 0, 1)[0]
        .0
        .clone();
    pool.block(&blocked, 1).unwrap();

    let claimed = pool.claim_ready(&descriptor, 1, 0, 8);

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id, "ready");
}

#[test]
fn ready_indexes_merge_protocol_queues_in_global_stable_order() {
    let mut descriptor = runner_descriptor("worker", "sim.alpha", RunnerPurity::Pure);
    descriptor.accepted_protocol_ids.push("sim.beta".into());
    let mut pool = TaskPool::default();

    let mut later = Task::new("later", "sim.alpha", json!({}));
    later.ready_at_step = Some(2);
    later.priority = 100;
    let mut beta_high = Task::new("beta-high", "sim.beta", json!({}));
    beta_high.priority = 10;
    let mut alpha_high = Task::new("alpha-high", "sim.alpha", json!({}));
    alpha_high.priority = 10;
    let alpha_low = Task::new("alpha-low", "sim.alpha", json!({}));
    pool.enqueue(later).unwrap();
    pool.enqueue(beta_high).unwrap();
    pool.enqueue(alpha_high).unwrap();
    pool.enqueue(alpha_low).unwrap();

    let claimed = pool.claim_ready(&descriptor, 2, 0, 8);

    assert_eq!(
        claimed
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["beta-high", "alpha-high", "alpha-low", "later"]
    );
    pool.assert_indexes_consistent();
}

#[test]
fn scheduling_indexes_exclude_unrelated_tasks_and_empty_tick_worksets() {
    let descriptor = runner_descriptor("worker", "sim.target", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    for index in 0..10_000 {
        pool.enqueue(Task::new(
            format!("noise-{index}"),
            format!("sim.noise.{}", index % 32),
            json!({"index": index}),
        ))
        .unwrap();
    }
    pool.enqueue(Task::new("target", "sim.target", json!({})))
        .unwrap();

    assert_eq!(pool.ready_dispatch_candidate_count_for_test(&descriptor), 1);
    let target_lease = pool.claim_ready_for_executor(&descriptor, "executor", 1, 0, 1)[0]
        .0
        .clone();
    assert_eq!(target_lease.task_id, "target");
    pool.complete(&target_lease, 1).unwrap();

    let task_ids = pool
        .records()
        .into_iter()
        .filter(|record| record.status == TaskStatus::Ready)
        .map(|record| record.task.task_id.clone())
        .collect::<Vec<_>>();
    for task_id in task_ids {
        pool.cancel_by_core(&task_id, 1).unwrap();
    }
    assert_eq!(pool.ready_count(), 0);
    assert_eq!(pool.pending_tick_index_entries_for_test(), 0);
    assert!(pool.wake_due_tasks(2).is_empty());
    assert!(pool.runner_load(&descriptor, 2, 0).queued_count == 0);
    pool.assert_indexes_consistent();
}

#[test]
fn randomized_task_state_transitions_match_rebuilt_indexes() {
    let runners = [
        runner_descriptor("runner-0", "sim.0", RunnerPurity::Pure),
        runner_descriptor("runner-1", "sim.1", RunnerPurity::Pure),
    ];
    let mut pool = TaskPool::default();
    let mut seed = 0x26_u64;
    let mut next_task = 0_u64;
    let mut current_step = 1_u64;

    for _ in 0..48 {
        enqueue_random_task(&mut pool, &mut seed, &mut next_task, current_step);
    }
    pool.assert_indexes_consistent();

    for iteration in 0..800 {
        let operation = next_random(&mut seed) % 9;
        match operation {
            0 => enqueue_random_task(&mut pool, &mut seed, &mut next_task, current_step),
            1 => {
                let runner = &runners[(next_random(&mut seed) as usize) % runners.len()];
                let _ = pool.claim_ready_for_executor_with_expiry(
                    runner,
                    format!("executor-{}", runner.runner_id),
                    current_step,
                    0,
                    3,
                    Some(current_step.saturating_add(2)),
                );
            }
            2 => transition_random_running_task(&mut pool, &mut seed, current_step),
            3 => {
                if let Some(task_id) = random_task_with_status(
                    &pool,
                    &mut seed,
                    &[TaskStatus::Waiting, TaskStatus::Blocked],
                ) {
                    pool.wake(&task_id, current_step).unwrap();
                }
            }
            4 => {
                current_step = current_step.saturating_add(1);
                pool.wake_due_tasks(current_step);
            }
            5 => {
                current_step = current_step.saturating_add(1);
                pool.reclaim_expired_task_leases(current_step);
            }
            6 => {
                if let Some(task_id) = random_non_terminal_task(&pool, &mut seed) {
                    pool.cancel_by_core(&task_id, current_step).unwrap();
                }
            }
            7 => {
                let (old_generation, new_generation) =
                    if iteration % 2 == 0 { (0, 1) } else { (1, 0) };
                pool.rebind_ready_generation(old_generation, new_generation);
            }
            _ => {
                if let Some(task_id) =
                    random_task_with_status(&pool, &mut seed, &[TaskStatus::Ready])
                {
                    pool.reject_ready(
                        &task_id,
                        crate::runtime_error(ERR_STATE_CONFLICT, "test", "task.random.reject"),
                    )
                    .unwrap();
                }
            }
        }
        pool.assert_indexes_consistent();
        if iteration % 97 == 0 {
            let mut aborted = pool.clone();
            aborted.abort_all(
                current_step,
                crate::runtime_error(ERR_RUNTIME_ABORTED, "test", "runtime.random.abort"),
            );
            aborted.assert_indexes_consistent();
        }
    }
}

fn enqueue_random_task(
    pool: &mut TaskPool,
    seed: &mut u64,
    next_task: &mut u64,
    current_step: u64,
) {
    let protocol = next_random(seed) % 2;
    let mut task = Task::new(
        format!("random-{next_task}"),
        format!("sim.{protocol}"),
        json!({"value": next_random(seed) % 10_000}),
    );
    *next_task = next_task.saturating_add(1);
    task.priority = (next_random(seed) % 17) as i64 - 8;
    if next_random(seed).is_multiple_of(4) {
        task.ready_at_step = Some(current_step.saturating_add(next_random(seed) % 4));
    }
    if next_random(seed).is_multiple_of(3) {
        task.runner_hint = Some(format!("runner-{}", next_random(seed) % 2));
    }
    if next_random(seed).is_multiple_of(5) {
        task.expected_versions.push(VersionExpectation {
            ref_id: format!("state-{}", next_random(seed) % 4),
            expected_version: next_random(seed) % 3,
        });
    }
    pool.enqueue_at(task, current_step).unwrap();
}

fn transition_random_running_task(pool: &mut TaskPool, seed: &mut u64, current_step: u64) {
    pool.reclaim_expired_task_leases(current_step);
    let Some(task_id) = random_task_with_status(pool, seed, &[TaskStatus::Running]) else {
        return;
    };
    let lease = pool.get(&task_id).unwrap().lease.clone().unwrap();
    match next_random(seed) % 6 {
        0 => pool.complete(&lease, current_step).unwrap(),
        1 => pool
            .fail(
                &lease,
                current_step,
                crate::runtime_error(ERR_RUNTIME_HOST_FAILED, "test", "task.random.fail"),
            )
            .unwrap(),
        2 => pool
            .wait(
                &lease,
                current_step,
                Some(current_step.saturating_add(1 + next_random(seed) % 3)),
            )
            .unwrap(),
        3 => pool.block(&lease, current_step).unwrap(),
        4 => pool.cancel_task(&lease, current_step).unwrap(),
        _ => {
            assert_eq!(
                pool.cancel_running_invocation(&lease.runner_id, &lease.lease_id, current_step),
                1
            );
        }
    }
}

fn random_non_terminal_task(pool: &TaskPool, seed: &mut u64) -> Option<String> {
    let candidates = pool
        .records()
        .into_iter()
        .filter(|record| {
            !matches!(
                record.status,
                TaskStatus::Completed
                    | TaskStatus::Failed
                    | TaskStatus::Cancelled
                    | TaskStatus::Expired
                    | TaskStatus::DeadLetter
            )
        })
        .map(|record| record.task.task_id.clone())
        .collect::<Vec<_>>();
    choose_random(candidates, seed)
}

fn random_task_with_status(
    pool: &TaskPool,
    seed: &mut u64,
    statuses: &[TaskStatus],
) -> Option<String> {
    let candidates = pool
        .records()
        .into_iter()
        .filter(|record| statuses.contains(&record.status))
        .map(|record| record.task.task_id.clone())
        .collect::<Vec<_>>();
    choose_random(candidates, seed)
}

fn choose_random(candidates: Vec<String>, seed: &mut u64) -> Option<String> {
    if candidates.is_empty() {
        None
    } else {
        let index = (next_random(seed) as usize) % candidates.len();
        candidates.into_iter().nth(index)
    }
}

fn next_random(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed
}

fn task_pool_test_continuation(ref_id: &str) -> TaskStepContinuation {
    TaskStepContinuation {
        continuation: ResourceRef {
            ref_id: ref_id.into(),
            resource_id: ResourceId {
                kind_id: "continuation".into(),
                slot_id: ref_id.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "continuation".into(),
            schema: "continuation.v1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        },
        wake: None,
        reason: Some("await child".into()),
    }
}
