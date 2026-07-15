use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ObservabilityOutletProfile, ObservabilityOverflowPolicy, ObservabilityProfile, Task, TaskBatch,
    TaskHandle,
};
use mutsuki_runtime_core::TaskHistoryRetention;
use mutsuki_runtime_host::{
    HostRuntime, HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply, RunnerLimits,
};
use serde_json::json;

use crate::ALLOCATOR;
use crate::fixtures::{BENCH_PROTOCOL_ID, echo_bootstrapper, runner_descriptor, runtime_profile};
use crate::report::{BenchmarkMode, CaseResult};

pub fn run(mode: BenchmarkMode) -> Result<Vec<CaseResult>, String> {
    let mut results = Vec::new();
    for entries in [1, 32, 256] {
        results.extend(batch_round_trip(entries)?);
    }
    results.push(actor_round_trip(mode)?);
    Ok(results)
}

fn batch_round_trip(entries: usize) -> Result<Vec<CaseResult>, String> {
    let runtime = host_runtime(entries)?;
    let tasks = (0..entries)
        .map(|index| {
            Task::new(
                format!("host-task-{entries}-{index}"),
                BENCH_PROTOCOL_ID,
                json!({"index": index}),
            )
        })
        .collect::<Vec<_>>();
    let measurement = ALLOCATOR.measurement();
    let reply = runtime
        .dispatch(HostRuntimeCommand::SubmitBatch(Box::new(TaskBatch {
            batch_id: format!("host-submit-{entries}"),
            tick_id: None,
            tasks,
            resource_plan: None,
        })))
        .map_err(|error| error.to_string())?;
    let (submit_elapsed_ns, submit_allocations) = measurement.finish(&ALLOCATOR);
    let HostRuntimeReply::TaskBatchSubmitted(handles) = reply else {
        return Err("host submit batch returned an unexpected reply".into());
    };
    if handles.len() != entries {
        return Err(format!(
            "host submitted {} handles, expected {entries}",
            handles.len()
        ));
    }
    runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 1_024 })
        .map_err(|error| error.to_string())?;
    wait_for_outcomes(&runtime, &handles)?;
    let dimensions = BTreeMap::from([("entries".into(), entries.to_string())]);
    let submit = CaseResult::measured(
        format!("host/submit-batch/entries-{entries}"),
        "host",
        dimensions.clone(),
        1,
        entries as u64,
        submit_elapsed_ns,
        submit_allocations,
        BTreeMap::from([("handles".into(), handles.len() as i128)]),
    );

    let outcome_measurement = ALLOCATOR.measurement();
    let mut completed = 0;
    for handle in &handles {
        match runtime
            .dispatch(HostRuntimeCommand::TaskOutcome(handle.clone()))
            .map_err(|error| error.to_string())?
        {
            HostRuntimeReply::TaskOutcome(Some(
                mutsuki_runtime_contracts::TaskOutcome::Completed { .. },
            )) => completed += 1,
            reply => return Err(format!("unexpected task outcome reply: {reply:?}")),
        }
    }
    let (outcome_elapsed_ns, outcome_allocations) = outcome_measurement.finish(&ALLOCATOR);
    let outcomes = CaseResult::measured(
        format!("host/task-outcome-batch/entries-{entries}"),
        "host",
        dimensions.clone(),
        1,
        entries as u64,
        outcome_elapsed_ns,
        outcome_allocations,
        BTreeMap::from([("completed".into(), completed)]),
    );

    let event_measurement = ALLOCATOR.measurement();
    let (event_items, event_pages, event_lost) = page_events(&runtime)?;
    let (event_elapsed_ns, event_allocations) = event_measurement.finish(&ALLOCATOR);
    let events = CaseResult::measured(
        format!("host/events-pagination/entries-{entries}"),
        "host",
        dimensions.clone(),
        event_pages,
        event_items.max(1),
        event_elapsed_ns,
        event_allocations,
        BTreeMap::from([
            ("items".into(), event_items as i128),
            ("pages".into(), event_pages as i128),
            ("lost".into(), event_lost as i128),
        ]),
    );

    let trace_measurement = ALLOCATOR.measurement();
    let (trace_items, trace_pages, trace_lost) = page_traces(&runtime)?;
    let (trace_elapsed_ns, trace_allocations) = trace_measurement.finish(&ALLOCATOR);
    let traces = CaseResult::measured(
        format!("host/traces-pagination/entries-{entries}"),
        "host",
        dimensions,
        trace_pages,
        trace_items.max(1),
        trace_elapsed_ns,
        trace_allocations,
        BTreeMap::from([
            ("items".into(), trace_items as i128),
            ("pages".into(), trace_pages as i128),
            ("lost".into(), trace_lost as i128),
        ]),
    );
    Ok(vec![submit, outcomes, events, traces])
}

fn actor_round_trip(mode: BenchmarkMode) -> Result<CaseResult, String> {
    let iterations = match mode {
        BenchmarkMode::Smoke => 1_000,
        BenchmarkMode::Full => 10_000,
    };
    let runtime = host_runtime(1)?;
    let measurement = ALLOCATOR.measurement();
    for _ in 0..iterations {
        match runtime
            .dispatch(HostRuntimeCommand::Statistics)
            .map_err(|error| error.to_string())?
        {
            HostRuntimeReply::Statistics(_) => {}
            _ => return Err("statistics command returned an unexpected reply".into()),
        }
    }
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    Ok(CaseResult::measured(
        "host/actor-command-round-trip/statistics",
        "host",
        BTreeMap::new(),
        iterations,
        iterations,
        elapsed_ns,
        allocations,
        BTreeMap::new(),
    ))
}

fn host_runtime(max_batch_entries: usize) -> Result<HostRuntime, String> {
    let descriptor = runner_descriptor(
        "bench.host.runner",
        vec![BENCH_PROTOCOL_ID.into()],
        max_batch_entries,
    );
    let observability = ObservabilityProfile {
        events: ObservabilityOutletProfile::new(4_096, ObservabilityOverflowPolicy::DropOldest),
        traces: ObservabilityOutletProfile::new(4_096, ObservabilityOverflowPolicy::DropOldest),
        detailed_scheduler_decisions: true,
        dispatch_spans: true,
    };
    let config = HostRuntimeConfig {
        worker_threads: 1,
        blocking_threads: 1,
        default_runner_limits: RunnerLimits {
            max_running: 1,
            max_waiting: 512,
            max_inflight: 512,
            deadline_ticks: None,
            wall_clock_deadline: None,
        },
        observability: Some(observability.clone()),
        task_history_retention: Some(TaskHistoryRetention::new(1_024, 2_048)),
        ..HostRuntimeConfig::default()
    };
    echo_bootstrapper(descriptor)
        .into_host_runtime_with_config(runtime_profile(observability), config)
        .map_err(|error| error.to_string())
}

fn wait_for_outcomes(runtime: &HostRuntime, handles: &[TaskHandle]) -> Result<(), String> {
    for _ in 0..1_024 {
        if handles.iter().all(|handle| {
            runtime.task_status(&handle.task_id).is_some_and(|status| {
                matches!(
                    status,
                    mutsuki_runtime_contracts::TaskStatus::Completed
                        | mutsuki_runtime_contracts::TaskStatus::Failed
                        | mutsuki_runtime_contracts::TaskStatus::Cancelled
                        | mutsuki_runtime_contracts::TaskStatus::Expired
                        | mutsuki_runtime_contracts::TaskStatus::DeadLetter
                )
            })
        }) {
            return Ok(());
        }
        runtime
            .dispatch(HostRuntimeCommand::TickOnce)
            .map_err(|error| error.to_string())?;
    }
    Err("host tasks did not reach terminal outcomes".into())
}

fn page_events(runtime: &HostRuntime) -> Result<(u64, u64, u64), String> {
    let mut cursor = 0;
    let mut items = 0;
    let mut pages = 0;
    let mut lost = 0;
    loop {
        let page = runtime
            .events_after(cursor, 32)
            .map_err(|error| error.to_string())?;
        items += page.items.len() as u64;
        pages += 1;
        lost += page.lost;
        cursor = page.next_sequence;
        if !page.truncated {
            return Ok((items, pages, lost));
        }
    }
}

fn page_traces(runtime: &HostRuntime) -> Result<(u64, u64, u64), String> {
    let mut cursor = 0;
    let mut items = 0;
    let mut pages = 0;
    let mut lost = 0;
    loop {
        let page = runtime
            .trace_spans_after(cursor, 32)
            .map_err(|error| error.to_string())?;
        items += page.items.len() as u64;
        pages += 1;
        lost += page.lost;
        cursor = page.next_sequence;
        if !page.truncated {
            return Ok((items, pages, lost));
        }
    }
}
