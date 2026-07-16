use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, RunnerContext, Task, TaskLease,
    WorkBatch, WorkResourcePlan,
};
use mutsuki_runtime_wire::{DisposeRunnerRequest, RunBatchRequest};
use serde_json::json;

pub fn dispose_request() -> DisposeRunnerRequest {
    DisposeRunnerRequest {
        runner_id: "benchmark.runner".into(),
    }
}

pub fn run_batch_request(entries: usize, payload_bytes: usize) -> RunBatchRequest {
    let payload = "x".repeat(payload_bytes);
    let mut tasks = Vec::with_capacity(entries);
    let mut batch_entries = Vec::with_capacity(entries);
    let mut leases = Vec::with_capacity(entries);
    for index in 0..entries {
        let task_id = format!("wire-task-{index}");
        let lease_id = format!("wire-lease-{index}");
        let mut task = Task::new(
            task_id.clone(),
            "mutsuki.benchmark.echo",
            json!({"payload": payload}),
        );
        task.lease_id = Some(lease_id.clone());
        task.registry_generation = 1;
        batch_entries.push(BatchEntry {
            entry_id: format!("wire-entry-{index}"),
            task_id: task_id.clone(),
            trace_id: None,
            parent_id: None,
            payload_index: index,
            resource_requirement_indices: Vec::new(),
            cancel_index: Some(index),
            deadline_tick: None,
            priority: 0,
            lane: DispatchLane::Normal,
            ordering: OrderingRequirement::None,
        });
        leases.push(TaskLease {
            lease_id,
            task_id,
            runner_id: "benchmark.runner".into(),
            executor_id: "benchmark.executor".into(),
            registry_generation: 1,
            acquired_at_step: 1,
            expires_at_step: Some(2),
        });
        tasks.push(task);
    }
    let batch = WorkBatch {
        batch_id: "wire-batch".into(),
        tick_id: "wire-tick".into(),
        batch_key: "benchmark.runner".into(),
        entries: batch_entries,
        payload: BatchPayload::from_tasks(&tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: leases,
    };
    let mut ctx = RunnerContext::new(1, 1, "benchmark.executor", None, "wire-invocation")
        .with_batch(batch.batch_id.clone(), entries);
    ctx.task_lease_ids = batch
        .task_leases
        .iter()
        .map(|lease| lease.lease_id.clone())
        .collect();
    RunBatchRequest {
        runner_id: "benchmark.runner".into(),
        ctx,
        batch,
    }
}
