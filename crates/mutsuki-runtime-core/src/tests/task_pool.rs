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

    pool.wake("task-1").unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Ready);
    let lease = pool.claim_ready_for_executor(&descriptor, "executor-1", 8, 0, 1)[0]
        .0
        .clone();

    pool.block(&lease, 8).unwrap();
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Blocked);
    pool.wake("task-1").unwrap();
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
    pool.wake("task-1").unwrap();

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

    assert_eq!(pool.reclaim_expired_leases(3), 0);
    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::Running);
    assert_eq!(pool.reclaim_expired_leases(4), 1);
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
    pool.reclaim_expired_leases(2);
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
    )
    .unwrap();

    let record = pool.get("task-1").unwrap();
    assert_eq!(record.status, TaskStatus::Expired);
    assert!(record.lease.is_none());
    assert!(record.claimed_by.is_none());
    assert!(record.owner_runner.is_none());
    assert!(record.task.lease_id.is_none());
    assert!(pool.complete(&lease, 1).is_err());
    assert!(pool.cancel_by_core("task-1").is_err());
}

#[test]
fn task_pool_dead_letter_is_terminal() {
    let mut pool = TaskPool::default();
    pool.enqueue(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    pool.dead_letter_by_core(
        "task-1",
        crate::runtime_error(ERR_TASK_DEAD_LETTER, "test", "task.dead_letter"),
    )
    .unwrap();

    assert_eq!(pool.get("task-1").unwrap().status, TaskStatus::DeadLetter);
    assert!(pool.wake("task-1").is_err());
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
fn task_pool_rejects_purity_and_generation_mismatched_claims() {
    let mut pool = TaskPool::default();
    let effectful = runner_descriptor("effect.chat", "sim.work", RunnerPurity::Effectful);
    let committer = runner_descriptor("commit", "sim.work", RunnerPurity::Committer);
    let pure = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);

    let mut work = Task::new("work-1", "sim.work", json!({}));
    work.registry_generation = 1;
    pool.enqueue(work).unwrap();

    assert!(pool.claim_ready(&effectful, 1, 1, 8).is_empty());
    assert!(pool.claim_ready(&committer, 1, 1, 8).is_empty());
    assert!(pool.claim_ready(&pure, 1, 2, 8).is_empty());

    assert_eq!(pool.rebind_ready_generation(1, 2), 1);
    let claimed = pool.claim_ready(&pure, 1, 2, 8);
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id, "work-1");
}

#[test]
fn task_pool_only_claims_ready_tasks() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    let ready = Task::new("ready", "sim.work", json!({}));
    let waiting = Task::new("waiting", "sim.work", json!({}));
    let blocked = Task::new("blocked", "sim.work", json!({}));
    pool.enqueue(ready).unwrap();
    pool.enqueue(waiting).unwrap();
    pool.enqueue(blocked).unwrap();
    pool.get_mut_for_test("waiting").status = TaskStatus::Waiting;
    pool.get_mut_for_test("blocked").status = TaskStatus::Blocked;

    let claimed = pool.claim_ready(&descriptor, 1, 0, 8);

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id, "ready");
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
