use super::fixtures::*;
use mutsukicore_runtime_contracts::{ScalarValue, StrategyResultStatus};
use serde_json::json;

#[test]
fn jsonl_runtime_backend_smoke_drives_strategy_fixture_process_roundtrip_and_failure() {
    let (runtime, result) = tick_strategy_fixture_backend(r#"{"status":"wait_input"}"#);

    assert_eq!(result.status, StrategyResultStatus::WaitInput);
    assert!(result.error.is_none());
    let events = runtime.events();
    assert!(events.iter().any(|event| event.name == "agent.awake"));
    assert!(events.iter().any(|event| event.name == "runtime.publish"));
    assert!(events.iter().any(|event| event.name == "agent.input"));
    assert!(events.iter().any(|event| event.name == "agent.stop"));

    let failure_output = json!({
        "status": "failed",
        "error": {
            "code": mutsukicore_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED,
            "source": "test-strategy-plugin",
            "route": "test.strategy.exec",
            "lost_capability": null,
            "recovery": null,
            "cause": null,
            "evidence": {"exit_code": 7, "stderr": "boom"},
        },
    })
    .to_string();
    let (runtime, result) = tick_strategy_fixture_backend(&failure_output);

    assert_eq!(result.status, StrategyResultStatus::Failed);
    let error = result.error.as_ref().unwrap();
    assert_eq!(
        error.code,
        mutsukicore_runtime_contracts::ERR_RUNTIME_BACKEND_FAILED
    );
    assert_eq!(error.source, "test-strategy-plugin");
    assert_eq!(error.route, "test.strategy.exec");
    assert_eq!(error.evidence.get("exit_code"), Some(&ScalarValue::Int(7)));
    assert_eq!(
        error.evidence.get("stderr"),
        Some(&ScalarValue::String("boom".into()))
    );

    let event = runtime
        .events()
        .into_iter()
        .find(|event| event.name == "agent.input.error")
        .unwrap();
    assert_eq!(event.error.as_ref().unwrap(), error);
}
