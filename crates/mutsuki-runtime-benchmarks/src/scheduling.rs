use std::collections::BTreeMap;
use std::hint::black_box;

use mutsuki_runtime_contracts::{RunnerDescriptor, Task};
use mutsuki_runtime_core::TaskPool;
use serde_json::json;

use crate::ALLOCATOR;
use crate::fixtures::runner_descriptor;
use crate::report::{BenchmarkMode, CaseResult};

#[derive(Clone, Copy, Debug)]
enum ProtocolDistribution {
    Single,
    Uniform,
    RunnerHint,
    OwnerContinuation,
}

impl ProtocolDistribution {
    fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single_protocol",
            Self::Uniform => "uniform_protocols",
            Self::RunnerHint => "runner_hint",
            Self::OwnerContinuation => "owner_continuation",
        }
    }
}

#[derive(Clone, Copy)]
struct SchedulingCase {
    axis: &'static str,
    tasks: usize,
    runners: usize,
    ready_percent: usize,
    batch_size: usize,
    distribution: ProtocolDistribution,
}

impl SchedulingCase {
    fn new(
        axis: &'static str,
        tasks: usize,
        runners: usize,
        ready_percent: usize,
        batch_size: usize,
        distribution: ProtocolDistribution,
    ) -> Self {
        Self {
            axis,
            tasks,
            runners,
            ready_percent,
            batch_size,
            distribution,
        }
    }
}

pub fn run(mode: BenchmarkMode) -> Result<Vec<CaseResult>, String> {
    let mut matrix = Vec::new();
    let scale = mode.select(
        vec![(1_000, 1), (10_000, 16), (100_000, 128)],
        [1_000, 10_000, 100_000]
            .into_iter()
            .flat_map(|tasks| [1, 16, 128].map(|runners| (tasks, runners)))
            .collect(),
    );
    matrix.extend(scale.into_iter().map(|(tasks, runners)| {
        SchedulingCase::new(
            "scale",
            tasks,
            runners,
            1,
            32,
            ProtocolDistribution::Uniform,
        )
    }));

    let tasks = mode.select(10_000, 100_000);
    for &ready_percent in mode.select(&[0, 100][..], &[0, 1, 50, 100][..]) {
        matrix.push(SchedulingCase::new(
            "ready_ratio",
            tasks,
            16,
            ready_percent,
            32,
            ProtocolDistribution::Uniform,
        ));
    }
    for &batch_size in mode.select(&[1, 256][..], &[1, 32, 256][..]) {
        matrix.push(SchedulingCase::new(
            "batch_size",
            tasks,
            16,
            100,
            batch_size,
            ProtocolDistribution::Uniform,
        ));
    }
    for distribution in [
        ProtocolDistribution::Single,
        ProtocolDistribution::Uniform,
        ProtocolDistribution::RunnerHint,
        ProtocolDistribution::OwnerContinuation,
    ] {
        matrix.push(SchedulingCase::new(
            "protocol_distribution",
            tasks,
            16,
            100,
            32,
            distribution,
        ));
    }
    matrix.into_iter().map(run_case).collect()
}

fn run_case(case: SchedulingCase) -> Result<CaseResult, String> {
    let runners = descriptors(case.runners, case.batch_size, case.distribution);
    let eligible = case.tasks * case.ready_percent / 100;
    let mut pool = TaskPool::default();
    for index in 0..case.tasks {
        let runner_index = index % case.runners;
        let protocol = match case.distribution {
            ProtocolDistribution::Uniform => format!("bench.protocol.{runner_index}"),
            _ => "bench.protocol.shared".to_string(),
        };
        let mut task = Task::new(
            format!("task-{index}"),
            protocol,
            json!({"index": index, "runner": runner_index}),
        );
        if index >= eligible {
            task.ready_at_step = Some(10);
        }
        if matches!(
            case.distribution,
            ProtocolDistribution::RunnerHint | ProtocolDistribution::OwnerContinuation
        ) {
            task.runner_hint = Some(format!("bench.runner.{runner_index}"));
        }
        pool.enqueue(task).map_err(|error| error.to_string())?;
    }
    if matches!(case.distribution, ProtocolDistribution::OwnerContinuation) {
        for runner in &runners {
            let leased = pool.claim_ready_for_executor(runner, "owner-setup", 0, 0, eligible);
            for (lease, _) in leased {
                pool.wait(&lease, 0, None)
                    .map_err(|error| error.to_string())?;
                pool.wake(&lease.task_id, 0)
                    .map_err(|error| error.to_string())?;
            }
        }
    }

    let measurement = ALLOCATOR.measurement();
    let queued = runners
        .iter()
        .map(|runner| black_box(pool.runner_load(runner, 1, 0).queued_count))
        .sum::<usize>();
    let mut claimed = 0;
    for (index, runner) in runners.iter().enumerate() {
        claimed += black_box(pool.claim_ready_for_executor(
            runner,
            format!("executor-{index}"),
            1,
            0,
            case.batch_size,
        ))
        .len();
    }
    let (elapsed_ns, allocations) = measurement.finish(&ALLOCATOR);
    let expected_claimed = expected_claimed(case, eligible);
    if claimed != expected_claimed {
        return Err(format!(
            "{} claimed {claimed}, expected {expected_claimed}",
            case.axis
        ));
    }

    Ok(CaseResult::measured(
        format!(
            "scheduling/{}/tasks-{}/runners-{}/ready-{}/batch-{}/{}",
            case.axis,
            case.tasks,
            case.runners,
            case.ready_percent,
            case.batch_size,
            case.distribution.as_str()
        ),
        "scheduling",
        BTreeMap::from([
            ("axis".into(), case.axis.into()),
            ("tasks".into(), case.tasks.to_string()),
            ("runners".into(), case.runners.to_string()),
            ("ready_percent".into(), case.ready_percent.to_string()),
            ("batch_size".into(), case.batch_size.to_string()),
            (
                "protocol_distribution".into(),
                case.distribution.as_str().into(),
            ),
        ]),
        1,
        claimed.max(case.runners) as u64,
        elapsed_ns,
        allocations,
        BTreeMap::from([
            ("eligible".into(), eligible as i128),
            ("queued_observations".into(), queued as i128),
            ("claimed".into(), claimed as i128),
            ("expected_claimed".into(), expected_claimed as i128),
            ("retained_records".into(), pool.records().len() as i128),
        ]),
    ))
}

fn descriptors(
    count: usize,
    batch_size: usize,
    distribution: ProtocolDistribution,
) -> Vec<RunnerDescriptor> {
    (0..count)
        .map(|index| {
            let protocol = match distribution {
                ProtocolDistribution::Uniform => format!("bench.protocol.{index}"),
                _ => "bench.protocol.shared".to_string(),
            };
            runner_descriptor(format!("bench.runner.{index}"), vec![protocol], batch_size)
        })
        .collect()
}

fn expected_claimed(case: SchedulingCase, eligible: usize) -> usize {
    match case.distribution {
        ProtocolDistribution::Single => eligible.min(case.runners * case.batch_size),
        _ => (0..case.runners)
            .map(|runner| {
                let assigned = if eligible <= runner {
                    0
                } else {
                    (eligible - 1 - runner) / case.runners + 1
                };
                assigned.min(case.batch_size)
            })
            .sum(),
    }
}
