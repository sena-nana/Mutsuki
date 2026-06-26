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
    pool.enqueue(low);
    pool.enqueue(high);
    pool.enqueue(future);

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
fn task_pool_rejects_purity_and_generation_mismatched_claims() {
    let mut pool = TaskPool::default();
    let effectful = runner_descriptor("effect.chat", "sim.work", RunnerPurity::Effectful);
    let committer = runner_descriptor("commit", "sim.work", RunnerPurity::Committer);
    let pure = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);

    let mut work = Task::new("work-1", "sim.work", json!({}));
    work.registry_generation = 1;
    pool.enqueue(work);

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
    pool.enqueue(ready);
    pool.enqueue(waiting);
    pool.enqueue(blocked);
    pool.get_mut_for_test("waiting").status = TaskStatus::Waiting;
    pool.get_mut_for_test("blocked").status = TaskStatus::Blocked;

    let claimed = pool.claim_ready(&descriptor, 1, 0, 8);

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id, "ready");
}
