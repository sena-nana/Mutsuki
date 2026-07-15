use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    ObservabilityOutletProfile, ObservabilityOverflowPolicy, ScalarValue, SpanStatus, TraceSpan,
};
use mutsuki_runtime_core::TraceLog;

const ITERATIONS: u64 = 2_000_000;

fn elapsed_per_iteration(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1_000_000_000.0 / ITERATIONS as f64
}

fn baseline_aggregate_counter() -> Duration {
    let started = Instant::now();
    let mut sequence = 0_u64;
    let mut dropped = 0_u64;
    for _ in 0..ITERATIONS {
        sequence = sequence.saturating_add(1);
        dropped = dropped.saturating_add(1);
        black_box((sequence, dropped));
    }
    started.elapsed()
}

fn disabled_trace_path() -> Duration {
    let mut traces = TraceLog::with_capacity(0);
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        let span = traces.record_with(|_| panic!("disabled trace path built span attributes"));
        black_box(span);
    }
    assert_eq!(traces.allocated_capacity(), 0);
    started.elapsed()
}

fn enabled_trace_path() -> Duration {
    let mut traces = TraceLog::with_profile(ObservabilityOutletProfile::new(
        64,
        ObservabilityOverflowPolicy::DropOldest,
    ));
    let started = Instant::now();
    for decision in 0..ITERATIONS {
        let span = traces.record_with(|sequence| TraceSpan {
            sequence,
            trace_id: format!("trace-scheduler-{}", decision % 8),
            span_id: format!("span-{sequence}"),
            parent_span_id: None,
            name: "scheduler.decision".into(),
            start: sequence as f64,
            end: Some(sequence as f64),
            attributes: BTreeMap::from([
                (
                    "runner_id".into(),
                    ScalarValue::String(format!("runner-{}", decision % 8)),
                ),
                ("current_step".into(), ScalarValue::Int(decision as i64)),
            ]),
            status: SpanStatus::Ok,
        });
        black_box(span);
    }
    assert_eq!(traces.retained(), 64);
    started.elapsed()
}

fn main() {
    let baseline_ns = elapsed_per_iteration(baseline_aggregate_counter());
    let disabled_ns = elapsed_per_iteration(disabled_trace_path());
    let enabled_ns = elapsed_per_iteration(enabled_trace_path());
    let allowed_disabled_ns = (baseline_ns * 5.0).max(baseline_ns + 25.0);
    let disabled_within_budget = disabled_ns <= allowed_disabled_ns;
    println!(
        "{{\"iterations\":{ITERATIONS},\"baseline_ns_per_decision\":{baseline_ns:.3},\"disabled_ns_per_decision\":{disabled_ns:.3},\"enabled_ns_per_decision\":{enabled_ns:.3},\"allowed_disabled_ns_per_decision\":{allowed_disabled_ns:.3},\"disabled_within_budget\":{disabled_within_budget}}}"
    );
    assert!(
        disabled_within_budget,
        "disabled observability path exceeded the explicit hot-path budget"
    );
}
