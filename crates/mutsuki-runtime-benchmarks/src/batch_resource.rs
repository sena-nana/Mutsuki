use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    OrderingRequirement, ResourceAccessMode, ResourceRequirement, Task, TaskBatch,
};
use mutsuki_runtime_core::{RunnerCompletion, RunnerDispatch, ScheduleDecision};
use serde_json::json;

use crate::ALLOCATOR;
use crate::fixtures::{BENCH_PROTOCOL_ID, echo_bootstrapper, runner_descriptor, runtime_profile};
use crate::report::{BenchmarkMode, CaseResult};

#[derive(Clone, Copy, Debug)]
enum ResourcePattern {
    None,
    SharedRead,
    WriteConflict,
    StrictOrder,
}

impl ResourcePattern {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "no_resources",
            Self::SharedRead => "shared_read",
            Self::WriteConflict => "write_conflict",
            Self::StrictOrder => "strict_order",
        }
    }
}

pub fn run(_mode: BenchmarkMode) -> Result<Vec<CaseResult>, String> {
    let mut results = Vec::new();
    for entries in [1, 32, 256] {
        for pattern in [
            ResourcePattern::None,
            ResourcePattern::SharedRead,
            ResourcePattern::WriteConflict,
            ResourcePattern::StrictOrder,
        ] {
            results.extend(run_case(entries, pattern)?);
        }
    }
    Ok(results)
}

fn run_case(entries: usize, pattern: ResourcePattern) -> Result<Vec<CaseResult>, String> {
    let mut descriptor = runner_descriptor(
        "bench.batch.runner",
        vec![BENCH_PROTOCOL_ID.into()],
        entries,
    );
    descriptor.resources.batch_read = true;
    descriptor.resources.batch_write = true;
    let mut runtime = echo_bootstrapper(descriptor)
        .into_runtime(runtime_profile(Default::default()))
        .map_err(|error| error.to_string())?;
    let tasks = (0..entries)
        .map(|index| task_for_pattern(index, pattern))
        .collect::<Vec<_>>();
    runtime
        .submit_batch(TaskBatch {
            batch_id: format!("batch-{}-{}", pattern.as_str(), entries),
            tick_id: None,
            tasks,
            resource_plan: None,
        })
        .map_err(|error| error.to_string())?;

    let plan_measurement = ALLOCATOR.measurement();
    let (claim_report, mut dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, load, _step, _generation| {
                Ok(ScheduleDecision::new(
                    "benchmark",
                    load.queued_count.min(entries),
                    "batch-resource",
                ))
            },
            None,
        )
        .map_err(|error| error.to_string())?;
    let (plan_elapsed_ns, plan_allocations) = plan_measurement.finish(&ALLOCATOR);
    let dispatched_entries = match pattern {
        ResourcePattern::WriteConflict => 1,
        _ => entries,
    };
    if claim_report.claimed_tasks != dispatched_entries || dispatches.len() != 1 {
        return Err(format!(
            "{} entries expected one dispatch with {dispatched_entries} claims, got {} dispatches and {} claims",
            pattern.as_str(),
            dispatches.len(),
            claim_report.claimed_tasks
        ));
    }
    let dispatch = dispatches.remove(0);
    validate_plan(&dispatch, entries, pattern)?;
    let plan = &dispatch.batch.resource_plan;
    let dimensions = BTreeMap::from([
        ("entries".into(), entries.to_string()),
        ("resource_pattern".into(), pattern.as_str().into()),
        ("payload_layout".into(), "row".into()),
    ]);
    let plan_case = CaseResult::measured(
        format!("batch_resource/plan/entries-{entries}/{}", pattern.as_str()),
        "batch_resource",
        dimensions.clone(),
        1,
        entries as u64,
        plan_elapsed_ns,
        plan_allocations,
        BTreeMap::from([
            ("requested_entries".into(), entries as i128),
            ("dispatched_entries".into(), dispatched_entries as i128),
            (
                "deferred_entries".into(),
                entries.saturating_sub(dispatched_entries) as i128,
            ),
            ("read_views".into(), plan.read_views.len() as i128),
            ("write_locks".into(), plan.write_locks.len() as i128),
            ("parallel_groups".into(), plan.parallel_groups.len() as i128),
            ("serial_groups".into(), plan.serial_groups.len() as i128),
            (
                "conflict_entries".into(),
                plan.conflict_entries.len() as i128,
            ),
            ("parallelism_limit".into(), plan.parallelism_limit as i128),
        ]),
    );

    let RunnerDispatch {
        mut runner,
        ctx,
        task_leases,
        batch,
    } = dispatch;
    let batch_id = batch.batch_id.clone();
    let expected_entries = batch.entries.clone();
    let completion_measurement = ALLOCATOR.measurement();
    let result = runner.run_batch(ctx, batch);
    let completed = runtime
        .complete_runner_dispatch(RunnerCompletion {
            runner,
            task_leases,
            batch_id,
            expected_entries,
            result,
        })
        .map_err(|error| error.to_string())?;
    let (completion_elapsed_ns, completion_allocations) = completion_measurement.finish(&ALLOCATOR);
    if completed.completed_tasks != dispatched_entries {
        return Err(format!(
            "{} completion routed {}, expected {dispatched_entries}",
            pattern.as_str(),
            completed.completed_tasks
        ));
    }
    let completion_case = CaseResult::measured(
        format!(
            "batch_resource/completion/entries-{entries}/{}",
            pattern.as_str()
        ),
        "batch_resource",
        dimensions,
        1,
        dispatched_entries as u64,
        completion_elapsed_ns,
        completion_allocations,
        BTreeMap::from([
            ("completed".into(), completed.completed_tasks as i128),
            (
                "retained_terminal".into(),
                runtime.tasks().retained_terminal_records() as i128,
            ),
        ]),
    );
    Ok(vec![plan_case, completion_case])
}

fn task_for_pattern(index: usize, pattern: ResourcePattern) -> Task {
    let mut task = Task::new(
        format!("batch-task-{index}"),
        BENCH_PROTOCOL_ID,
        json!({"index": index}),
    );
    match pattern {
        ResourcePattern::None => {}
        ResourcePattern::SharedRead => task.resource_requirements.push(ResourceRequirement {
            ref_id: "resource:shared".into(),
            mode: ResourceAccessMode::Read,
            expected_version: Some(1),
        }),
        ResourcePattern::WriteConflict => {
            task.resource_requirements.push(ResourceRequirement {
                ref_id: "resource:shared".into(),
                mode: ResourceAccessMode::Write,
                expected_version: Some(1),
            });
        }
        ResourcePattern::StrictOrder => {
            task.ordering = OrderingRequirement::StrictSequence {
                sequence_id: "benchmark-sequence".into(),
            };
        }
    }
    task
}

fn validate_plan(
    dispatch: &RunnerDispatch,
    entries: usize,
    pattern: ResourcePattern,
) -> Result<(), String> {
    let plan = &dispatch.batch.resource_plan;
    let valid = match pattern {
        ResourcePattern::None => {
            plan.read_views.is_empty()
                && plan.write_locks.is_empty()
                && plan.conflict_entries.is_empty()
                && plan.parallelism_limit == entries
        }
        ResourcePattern::SharedRead => {
            plan.read_views.len() == 1
                && plan.write_locks.is_empty()
                && plan.conflict_entries.is_empty()
                && plan.parallelism_limit == entries
        }
        ResourcePattern::WriteConflict => {
            plan.write_locks.len() == 1
                && plan.conflict_entries.is_empty()
                && plan.parallelism_limit == 1
        }
        ResourcePattern::StrictOrder => {
            plan.serial_groups.len() == entries && plan.parallelism_limit == 1
        }
    };
    valid.then_some(()).ok_or_else(|| {
        format!(
            "invalid resource plan for {} with {entries} entries: {plan:?}",
            pattern.as_str()
        )
    })
}
