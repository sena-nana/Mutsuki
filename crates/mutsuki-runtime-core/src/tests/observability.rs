use mutsuki_runtime_contracts::*;

use crate::*;

use super::fixtures::*;

fn aggregate_only_runtime() -> CoreRuntime {
    let worker = runner_descriptor("idle.worker", "idle.work", RunnerPurity::Pure);
    let mut plan = load_plan(vec![worker.clone()], Vec::new());
    plan.plugins[0]
        .provides
        .runners
        .retain(|runner| runner.runner_id != "core.kernel");
    plan.observability = ObservabilityProfile {
        events: ObservabilityOutletProfile::new(0, ObservabilityOverflowPolicy::DropNew),
        traces: ObservabilityOutletProfile::new(64, ObservabilityOverflowPolicy::DropOldest),
        detailed_scheduler_decisions: false,
        dispatch_spans: false,
    };
    CoreRuntime::boot(plan, vec![completed_runner!(worker)]).unwrap()
}

#[test]
fn idle_day_and_one_million_scheduler_decisions_keep_trace_memory_bounded() {
    const IDLE_TICKS_IN_24_HOURS_AT_ONE_HZ: usize = 24 * 60 * 60;
    const TOTAL_DECISIONS: usize = 1_000_000;
    let mut runtime = aggregate_only_runtime();

    for _ in 0..IDLE_TICKS_IN_24_HOURS_AT_ONE_HZ {
        runtime
            .claim_ready_dispatches(
                |_descriptor, _load, _step, _generation| {
                    Ok(ScheduleDecision::new("idle.aggregate", 0, "idle"))
                },
                None,
            )
            .unwrap();
    }
    let after_day = runtime.statistics();
    assert_eq!(after_day.retained_traces, 0);
    assert_eq!(
        after_day.scheduler_decisions,
        IDLE_TICKS_IN_24_HOURS_AT_ONE_HZ as u64
    );

    for _ in IDLE_TICKS_IN_24_HOURS_AT_ONE_HZ..TOTAL_DECISIONS {
        runtime
            .claim_ready_dispatches(
                |_descriptor, _load, _step, _generation| {
                    Ok(ScheduleDecision::new("idle.aggregate", 0, "idle"))
                },
                None,
            )
            .unwrap();
    }
    let statistics = runtime.statistics();
    assert_eq!(statistics.scheduler_decisions, TOTAL_DECISIONS as u64);
    assert!(statistics.retained_traces <= 64);
    assert_eq!(statistics.retained_traces, 0);
}

#[test]
fn dispatch_spans_require_explicit_profile_opt_in() {
    let worker = runner_descriptor("worker", "trace.work", RunnerPurity::Pure);
    let mut plan = load_plan(vec![worker.clone()], Vec::new());
    plan.observability.dispatch_spans = false;
    plan.observability.detailed_scheduler_decisions = false;
    let mut runtime =
        CoreRuntime::boot(plan, runners_with_kernel!(completed_runner!(worker))).unwrap();
    runtime
        .submit_task(Task::new("trace-off", "trace.work", serde_json::json!({})))
        .unwrap();
    runtime.run_until_idle(4).unwrap();

    assert_eq!(runtime.statistics().retained_traces, 0);
    assert!(runtime.trace_spans().is_empty());
}
