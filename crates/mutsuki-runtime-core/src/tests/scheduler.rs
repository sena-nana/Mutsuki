use std::collections::BTreeMap;

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

#[test]
fn claim_ready_dispatches_records_scheduler_decision_event_and_trace() {
    let mut worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    worker.batch.max_batch_entries = 4;
    worker.batch.preferred_batch_size = 4;
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new("schedule-1", "runtime.schedule.input", json!({})))
        .unwrap();

    let (_report, _dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test.scheduler", 4, "test.reason").clamp_to(2))
            },
            None,
        )
        .unwrap();

    let event = runtime
        .events()
        .iter()
        .find(|event| {
            event.name == "scheduler.decision"
                && event.attributes.get("runner_id") == Some(&ScalarValue::String("worker".into()))
        })
        .expect("scheduler decision event should be recorded");
    assert_eq!(
        event.attributes.get("scheduler_id"),
        Some(&ScalarValue::String("test.scheduler".into()))
    );
    assert_eq!(
        event.attributes.get("runner_id"),
        Some(&ScalarValue::String("worker".into()))
    );
    assert_eq!(
        event.attributes.get("requested_dispatch_limit"),
        Some(&ScalarValue::Int(4))
    );
    assert_eq!(
        event.attributes.get("effective_dispatch_limit"),
        Some(&ScalarValue::Int(2))
    );
    assert_eq!(
        event.attributes.get("reason"),
        Some(&ScalarValue::String("test.reason".into()))
    );
    assert!(runtime.trace_spans().iter().any(|span| {
        span.name == "scheduler.decision"
            && span.attributes.get("scheduler_id")
                == Some(&ScalarValue::String("test.scheduler".into()))
            && span.attributes.get("runner_id") == Some(&ScalarValue::String("worker".into()))
    }));
}

#[test]
fn claim_ready_dispatches_applies_lane_budget_before_batch_build() {
    let mut worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    worker.batch.max_batch_entries = 3;
    worker.batch.preferred_batch_size = 3;
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let mut interactive = Task::new("schedule-interactive", "runtime.schedule.input", json!({}));
    interactive.dispatch_lane = DispatchLane::Interactive;
    let mut first_bulk = Task::new("schedule-bulk-1", "runtime.schedule.input", json!({}));
    first_bulk.dispatch_lane = DispatchLane::Bulk;
    let mut second_bulk = Task::new("schedule-bulk-2", "runtime.schedule.input", json!({}));
    second_bulk.dispatch_lane = DispatchLane::Bulk;
    for task in [interactive, first_bulk, second_bulk] {
        runtime.submit_task(task).unwrap();
    }

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |descriptor, _load, _step, _generation| {
                if descriptor.runner_id != "worker" {
                    return Ok(ScheduleDecision::new("test.scheduler", 0, "test.skip"));
                }
                Ok(
                    ScheduleDecision::new("test.scheduler", 3, "test.lane").with_budget(
                        DispatchBudget {
                            max_entries: 3,
                            max_batches: 1,
                            max_bytes: usize::MAX,
                            lane_budget: BTreeMap::from([(
                                DispatchLane::Bulk,
                                LaneBudget { max_entries: 1 },
                            )]),
                        },
                    ),
                )
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 2);
    assert_eq!(dispatches.len(), 1);
    let task_ids: Vec<_> = dispatches[0]
        .batch
        .entries
        .iter()
        .map(|entry| entry.task_id.as_str())
        .collect();
    assert_eq!(task_ids, vec!["schedule-interactive", "schedule-bulk-1"]);
    assert_eq!(
        runtime.task_status("schedule-bulk-2"),
        Some(TaskStatus::Ready)
    );
}

#[test]
fn claim_ready_dispatches_clamps_to_runner_max_batch_entries() {
    let mut worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    worker.batch.max_batch_entries = 2;
    worker.batch.preferred_batch_size = 2;
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    for index in 1..=4 {
        runtime
            .submit_task(Task::new(
                format!("schedule-{index}"),
                "runtime.schedule.input",
                json!({}),
            ))
            .unwrap();
    }

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test.scheduler", 4, "test.max-batch"))
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 2);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].batch.entries.len(), 2);
    assert_eq!(runtime.task_status("schedule-3"), Some(TaskStatus::Ready));
}

#[test]
fn max_batch_entries_one_still_dispatches_single_entry_work_batch() {
    let mut worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    worker.batch.max_batch_entries = 1;
    worker.batch.preferred_batch_size = 1;
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    for index in 1..=2 {
        runtime
            .submit_task(Task::new(
                format!("schedule-{index}"),
                "runtime.schedule.input",
                json!({}),
            ))
            .unwrap();
    }

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new(
                    "test.scheduler",
                    4,
                    "test.single-entry",
                ))
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 1);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].ctx.entry_count, 1);
    assert_eq!(dispatches[0].batch.entries.len(), 1);
    assert_eq!(dispatches[0].batch.task_leases.len(), 1);
    assert_eq!(runtime.task_status("schedule-2"), Some(TaskStatus::Ready));
}

#[test]
fn claim_ready_dispatches_applies_byte_budget_before_batch_build() {
    let mut worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    worker.batch.max_batch_entries = 3;
    worker.batch.preferred_batch_size = 3;
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    for index in 1..=3 {
        runtime
            .submit_task(Task::new(
                format!("schedule-{index}"),
                "runtime.schedule.input",
                json!("aaaa"),
            ))
            .unwrap();
    }
    assert_eq!(runtime.tasks().payload_wire_bytes_for_test("schedule-1"), 6);

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(
                    ScheduleDecision::new("test.scheduler", 3, "test.bytes").with_budget(
                        DispatchBudget {
                            max_entries: 3,
                            max_batches: 1,
                            max_bytes: 7,
                            lane_budget: BTreeMap::new(),
                        },
                    ),
                )
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 1);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].batch.entries.len(), 1);
    assert_eq!(runtime.task_status("schedule-2"), Some(TaskStatus::Ready));
}

#[test]
fn claim_ready_dispatches_splits_conflicting_writes_before_dispatch() {
    let mut worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    worker.batch.max_batch_entries = 4;
    worker.batch.preferred_batch_size = 4;
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let mut first = Task::new("schedule-1", "runtime.schedule.input", json!({}));
    first.resource_requirements = vec![ResourceRequirement {
        ref_id: "resource:shared".into(),
        mode: ResourceAccessMode::Write,
        expected_version: Some(1),
    }];
    let mut second = Task::new("schedule-2", "runtime.schedule.input", json!({}));
    second.resource_requirements = vec![ResourceRequirement {
        ref_id: "resource:shared".into(),
        mode: ResourceAccessMode::Write,
        expected_version: Some(1),
    }];
    runtime.submit_task(first).unwrap();
    runtime.submit_task(second).unwrap();

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test.scheduler", 4, "test.batch").clamp_to(4))
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 1);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].batch.entries.len(), 1);
    assert_eq!(dispatches[0].batch.task_leases.len(), 1);
    assert!(
        dispatches[0]
            .batch
            .resource_plan
            .conflict_entries
            .is_empty()
    );

    let mut completed = 0;
    for dispatch in dispatches {
        let RunnerDispatchTarget::Sync(mut runner) = dispatch.target else {
            panic!("test expected a synchronous runner dispatch");
        };
        let result = runner.run_batch(dispatch.ctx, dispatch.batch.clone());
        let report = runtime
            .complete_runner_dispatch(RunnerCompletion {
                runner: Some(runner),
                task_leases: dispatch.task_leases,
                batch_id: dispatch.batch.batch_id.clone(),
                expected_entries: dispatch.batch.entries.clone(),
                result,
            })
            .unwrap();
        completed += report.completed_tasks;
    }

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test.scheduler", 4, "test.batch").clamp_to(4))
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 1);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].batch.entries.len(), 1);
    assert!(
        dispatches[0]
            .batch
            .resource_plan
            .conflict_entries
            .is_empty()
    );

    for dispatch in dispatches {
        let RunnerDispatchTarget::Sync(mut runner) = dispatch.target else {
            panic!("test expected a synchronous runner dispatch");
        };
        let result = runner.run_batch(dispatch.ctx, dispatch.batch.clone());
        let report = runtime
            .complete_runner_dispatch(RunnerCompletion {
                runner: Some(runner),
                task_leases: dispatch.task_leases,
                batch_id: dispatch.batch.batch_id.clone(),
                expected_entries: dispatch.batch.entries.clone(),
                result,
            })
            .unwrap();
        completed += report.completed_tasks;
    }

    assert_eq!(completed, 2);
    assert_eq!(
        runtime.task_status("schedule-1"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("schedule-2"),
        Some(TaskStatus::Completed)
    );
}

#[test]
fn claim_ready_dispatches_keeps_shared_reads_in_one_work_batch() {
    let mut worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    worker.batch.max_batch_entries = 4;
    worker.batch.preferred_batch_size = 4;
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let mut first = Task::new("schedule-1", "runtime.schedule.input", json!({}));
    first.resource_requirements = vec![ResourceRequirement {
        ref_id: "resource:shared".into(),
        mode: ResourceAccessMode::Read,
        expected_version: Some(1),
    }];
    let mut second = Task::new("schedule-2", "runtime.schedule.input", json!({}));
    second.resource_requirements = vec![ResourceRequirement {
        ref_id: "resource:shared".into(),
        mode: ResourceAccessMode::Read,
        expected_version: Some(1),
    }];
    runtime.submit_task(first).unwrap();
    runtime.submit_task(second).unwrap();

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(ScheduleDecision::new("test.scheduler", 4, "test.batch").clamp_to(4))
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 2);
    assert_eq!(dispatches.len(), 1);
    let dispatch = &dispatches[0];
    assert_eq!(dispatch.batch.entries.len(), 2);
    assert_eq!(dispatch.batch.resource_plan.read_views.len(), 1);
    assert_eq!(dispatch.batch.resource_plan.write_locks.len(), 0);
    assert_eq!(
        dispatch.batch.resource_plan.parallel_groups,
        vec![vec!["schedule-1".to_string(), "schedule-2".to_string()]]
    );
    assert!(dispatch.batch.resource_plan.serial_groups.is_empty());
    assert_eq!(dispatch.batch.resource_plan.parallelism_limit, 2);
    assert!(dispatch.batch.resource_plan.conflict_entries.is_empty());
}

#[test]
fn sharded_runner_requires_declared_instances_and_dispatches_one_batch_per_instance() {
    let mut worker = runner_descriptor("worker", "runtime.sharded.input", RunnerPurity::Pure);
    worker.concurrency = RunnerConcurrency::Sharded { instances: 2 };
    worker.batch.preferred_batch_size = 1;
    worker.batch.max_batch_entries = 1;
    worker.batch.max_entry_concurrency = 1;
    worker.batch.max_inflight_batches = 2;
    let plan = load_plan(vec![worker.clone()], Vec::new());

    let missing_instance: Vec<Box<dyn Runner>> =
        runners_with_kernel!(completed_runner!(worker.clone()));
    let error = CoreRuntime::boot(plan.clone(), missing_instance)
        .err()
        .expect("sharded runner should require every declared instance");
    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert!(error.error().route.contains("instances.1.expected.2"));

    let runners: Vec<Box<dyn Runner>> =
        runners_with_kernel!(completed_runner!(worker.clone()), completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new("shard-1", "runtime.sharded.input", json!({})))
        .unwrap();
    runtime
        .submit_task(Task::new("shard-2", "runtime.sharded.input", json!({})))
        .unwrap();

    let (report, dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                Ok(
                    ScheduleDecision::new("test.scheduler", 2, "test.sharded").with_budget(
                        DispatchBudget {
                            max_entries: 2,
                            max_batches: 2,
                            max_bytes: usize::MAX,
                            lane_budget: BTreeMap::new(),
                        },
                    ),
                )
            },
            None,
        )
        .unwrap();

    assert_eq!(report.claimed_tasks, 2);
    assert_eq!(dispatches.len(), 2);
    assert!(
        dispatches
            .iter()
            .all(|dispatch| matches!(dispatch.target, RunnerDispatchTarget::Sync(_)))
    );
}
