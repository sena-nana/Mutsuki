use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

#[test]
fn claim_ready_dispatches_records_scheduler_decision_event_and_trace() {
    let worker = runner_descriptor("worker", "runtime.schedule.input", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new("schedule-1", "runtime.schedule.input", json!({})))
        .unwrap();

    let (_report, _dispatches) = runtime
        .claim_ready_dispatches(
            |_descriptor, _load, _step, _generation| {
                ScheduleDecision::new("test.scheduler", 4, "test.reason").clamp_to(2)
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
