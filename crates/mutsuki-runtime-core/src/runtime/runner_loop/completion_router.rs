use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    BatchEntry, CompletionBatch, EntryCompletion, RunnerDescriptor, RuntimeError, TaskLease,
};

use crate::RuntimeResult;

use super::{CoreRuntime, RunnerCompletion, RunnerLoopReport};

pub(super) fn complete_runner_dispatch(
    runtime: &mut CoreRuntime,
    completion: RunnerCompletion,
) -> RuntimeResult<RunnerLoopReport> {
    let descriptor = completion.runner.descriptor().clone();
    let result = completion.result;
    runtime.registry.put_runner(completion.runner);
    let result = match result {
        Ok(result) => result,
        Err(failure) => {
            let completed = super::failure_reporting::fail_runner_dispatches(
                runtime,
                &completion.task_leases,
                failure.error().clone(),
            )?;
            return Ok(RunnerLoopReport {
                claimed_tasks: 0,
                completed_tasks: completed,
            });
        }
    };
    if result.batch_id != completion.batch_id {
        let failure = batch_claim_conflict(format!("batch.result.{}", result.batch_id));
        let completed = super::failure_reporting::fail_runner_dispatches(
            runtime,
            &completion.task_leases,
            failure,
        )?;
        return Ok(RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: completed,
        });
    }
    let completed = route_completion_batch(
        runtime,
        &descriptor,
        &completion.task_leases,
        &completion.expected_entries,
        result,
    )?;
    Ok(RunnerLoopReport {
        claimed_tasks: 0,
        completed_tasks: completed,
    })
}

fn route_completion_batch(
    runtime: &mut CoreRuntime,
    descriptor: &RunnerDescriptor,
    leases: &[TaskLease],
    expected_entries: &[BatchEntry],
    batch: CompletionBatch,
) -> RuntimeResult<usize> {
    let mut leases_by_task = BTreeMap::new();
    for lease in leases {
        leases_by_task.insert(lease.task_id.as_str(), lease);
    }
    let mut expected_entries_by_id = BTreeMap::new();
    for entry in expected_entries {
        if expected_entries_by_id
            .insert(entry.entry_id.as_str(), entry)
            .is_some()
            || !leases_by_task.contains_key(entry.task_id.as_str())
        {
            return super::failure_reporting::fail_runner_dispatches(
                runtime,
                leases,
                batch_claim_conflict(format!("batch.entry.expected.{}", entry.entry_id)),
            );
        }
    }
    let mut completions_by_entry = BTreeMap::new();
    for completion in &batch.results {
        let Some(entry) = expected_entries_by_id.get(completion.entry_id.as_str()) else {
            return super::failure_reporting::fail_runner_dispatches(
                runtime,
                leases,
                batch_claim_conflict(format!("batch.entry.{}", completion.entry_id)),
            );
        };
        if entry.task_id != completion.task_id
            || !leases_by_task.contains_key(completion.task_id.as_str())
        {
            return super::failure_reporting::fail_runner_dispatches(
                runtime,
                leases,
                batch_claim_conflict(format!("batch.entry.{}", completion.entry_id)),
            );
        }
        if completions_by_entry
            .insert(completion.entry_id.as_str(), completion)
            .is_some()
        {
            return super::failure_reporting::fail_runner_dispatches(
                runtime,
                leases,
                batch_claim_conflict(format!("batch.entry.{}", completion.entry_id)),
            );
        }
    }
    let mut completed = 0;
    for entry in expected_entries {
        let Some(lease) = leases_by_task.get(entry.task_id.as_str()) else {
            return super::failure_reporting::fail_runner_dispatches(
                runtime,
                leases,
                batch_claim_conflict(format!("batch.entry.expected.{}", entry.entry_id)),
            );
        };
        let Some(completion) = completions_by_entry.get(entry.entry_id.as_str()) else {
            completed += super::failure_reporting::fail_runner_dispatch(
                runtime,
                lease,
                batch_claim_conflict(format!("batch.missing.{}", entry.entry_id)),
            )?;
            continue;
        };
        completed += route_entry_completion(runtime, descriptor, lease, (*completion).clone())?;
    }
    Ok(completed)
}

fn route_entry_completion(
    runtime: &mut CoreRuntime,
    descriptor: &RunnerDescriptor,
    lease: &TaskLease,
    completion: EntryCompletion,
) -> RuntimeResult<usize> {
    if let Some(error) = completion.error {
        return super::failure_reporting::fail_runner_dispatch(runtime, lease, error);
    }
    let Some(result) = completion.result else {
        return super::failure_reporting::fail_runner_dispatch(
            runtime,
            lease,
            batch_claim_conflict(format!("batch.entry.empty.{}", completion.entry_id)),
        );
    };
    if result.task_id != lease.task_id || completion.task_id != lease.task_id {
        return super::failure_reporting::fail_runner_dispatch(
            runtime,
            lease,
            batch_claim_conflict(format!("task.result.{}", result.task_id)),
        );
    }
    match runtime.route_result(descriptor, lease, result) {
        Ok(count) => Ok(count),
        Err(failure) if is_stale_completion_conflict(failure.error()) => {
            super::failure_reporting::record_rejected_runner_result(
                runtime,
                lease.task_id.clone(),
                failure.error().clone(),
            );
            Ok(0)
        }
        Err(failure) => Err(failure),
    }
}

fn batch_claim_conflict(route: String) -> RuntimeError {
    crate::runtime_error(
        mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT,
        "runtime.runner_loop",
        route,
    )
}

fn is_stale_completion_conflict(error: &RuntimeError) -> bool {
    error.code == mutsuki_runtime_contracts::ERR_TASK_CLAIM_CONFLICT
        && error.route.starts_with("task.route.")
}
