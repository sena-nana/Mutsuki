use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{Task, TaskBatch};
use serde_json::json;

use crate::ALLOCATOR;
use crate::fixtures::{BENCH_PROTOCOL_ID, echo_bootstrapper, runner_descriptor, runtime_profile};
use crate::report::{BenchmarkMode, CaseResult};

pub fn run(_mode: BenchmarkMode) -> Result<Vec<CaseResult>, String> {
    [1, 16, 256]
        .into_iter()
        .map(run_case)
        .collect::<Result<Vec<_>, _>>()
}

fn run_case(entries: usize) -> Result<CaseResult, String> {
    let descriptor = runner_descriptor(
        "bench.local.runner",
        vec![BENCH_PROTOCOL_ID.into()],
        entries,
    );
    let mut runtime = echo_bootstrapper(descriptor)
        .into_runtime(runtime_profile(Default::default()))
        .map_err(|error| error.to_string())?;
    runtime
        .submit_batch(TaskBatch {
            batch_id: format!("local-dispatch-{entries}"),
            tick_id: None,
            tasks: (0..entries)
                .map(|index| {
                    Task::new(
                        format!("local-task-{index}"),
                        BENCH_PROTOCOL_ID,
                        json!({"index": index, "message": "typed-local"}),
                    )
                })
                .collect(),
            resource_plan: None,
        })
        .map_err(|error| error.to_string())?;

    let measurement = ALLOCATOR.measurement();
    let report = runtime.tick_once().map_err(|error| error.to_string())?;
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    if report.claimed_tasks != entries || report.completed_tasks != entries {
        return Err(format!(
            "local dispatch expected {entries} claims/completions, got {}/{}",
            report.claimed_tasks, report.completed_tasks
        ));
    }

    Ok(CaseResult::measured(
        format!("local_dispatch/entries-{entries}"),
        "local_dispatch",
        BTreeMap::from([
            ("batch_size".into(), entries.to_string()),
            ("payload_representation".into(), "typed_local".into()),
            ("deployment".into(), "builtin".into()),
            ("rss_scope".into(), "benchmark_process".into()),
        ]),
        1,
        entries as u64,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("claimed".into(), report.claimed_tasks as i128),
            ("completed".into(), report.completed_tasks as i128),
        ]),
    ))
}
