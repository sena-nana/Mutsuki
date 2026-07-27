use std::sync::Arc;
use std::time::Duration;

use mutsuki_runtime_contracts::{
    CrossDomainTaskRequest, RuntimeDomainId, Task, TaskOutcome, TaskStatus,
};
use mutsuki_runtime_sdk::HostServiceRegistry;
use serde_json::json;

use crate::{
    ExecutionDomainConfig, HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply,
    RuntimeGroupHost,
};

use super::helpers::{host_with_echo_runner, runtime_profile};

fn domain_id(value: &str) -> RuntimeDomainId {
    RuntimeDomainId::new(value).unwrap()
}

fn runtime(shared: Arc<HostServiceRegistry>) -> crate::HostRuntime {
    let mut bootstrapper = host_with_echo_runner();
    bootstrapper.use_shared_services(shared).unwrap();
    bootstrapper
        .into_host_runtime_with_config(
            runtime_profile(),
            HostRuntimeConfig {
                event_driven: true,
                ..HostRuntimeConfig::default()
            },
        )
        .unwrap()
}

#[test]
fn runtime_group_routes_idempotently_and_isolates_domain_abort() {
    let shared = Arc::new(HostServiceRegistry::new());
    shared.freeze();
    let mut group = RuntimeGroupHost::with_defaults(shared.clone());
    let bot = domain_id("bot-domain");
    let agent = domain_id("agent-domain");
    group
        .insert_domain(bot.clone(), runtime(shared.clone()))
        .unwrap();
    group
        .insert_domain(agent.clone(), runtime(shared.clone()))
        .unwrap();

    let request = CrossDomainTaskRequest {
        request_id: "bot-to-agent-1".into(),
        source_domain: bot.clone(),
        target_domain: agent.clone(),
        task: Task::new("agent-turn-1", "raw.input", json!({"turn": 1})),
        timeout_ms: 1_000,
        idempotency_key: "conversation-1-turn-1".into(),
        max_attempts: 2,
    };
    let first = group.submit_cross_domain(request.clone()).unwrap();
    let duplicate = group.submit_cross_domain(request).unwrap();
    assert_eq!(first, duplicate);
    assert!(matches!(
        group.wait_outcome(&first, Duration::from_secs(1)).unwrap(),
        Some(TaskOutcome::Completed { .. })
    ));
    assert_eq!(
        group
            .domain(&agent)
            .unwrap()
            .statistics()
            .unwrap()
            .tasks
            .submitted_total,
        1
    );

    group.abort_domain(&agent, "test.agent.abort").unwrap();
    let bot_runtime = group.domain(&bot).unwrap();
    let reply = bot_runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "bot-control-after-agent-abort",
            "raw.input",
            json!({}),
        ))))
        .unwrap();
    let HostRuntimeReply::TaskSubmitted(handle) = reply else {
        panic!("expected task handle");
    };
    let states = bot_runtime
        .wait_task_states(vec![handle], Duration::from_secs(1))
        .unwrap();
    assert_eq!(states[0].status, Some(TaskStatus::Completed));
}

#[test]
fn execution_domain_registry_maps_every_execution_class_once() {
    let shared = Arc::new(HostServiceRegistry::new());
    shared.freeze();
    let mut bootstrapper = host_with_echo_runner();
    bootstrapper.use_shared_services(shared).unwrap();
    let config = HostRuntimeConfig {
        execution_domains: vec![
            ExecutionDomainConfig::new(
                "interactive",
                vec![
                    mutsuki_runtime_contracts::ExecutionClass::Orchestration,
                    mutsuki_runtime_contracts::ExecutionClass::Cpu,
                ],
                2,
            ),
            ExecutionDomainConfig::new(
                "io",
                vec![
                    mutsuki_runtime_contracts::ExecutionClass::Io,
                    mutsuki_runtime_contracts::ExecutionClass::Blocking,
                ],
                2,
            ),
            ExecutionDomainConfig::new(
                "script",
                vec![mutsuki_runtime_contracts::ExecutionClass::Script],
                1,
            ),
        ],
        ..HostRuntimeConfig::default()
    };
    let runtime = bootstrapper
        .into_host_runtime_with_config(runtime_profile(), config)
        .unwrap();
    let snapshots = runtime.worker_pools().unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.domain_id.as_str())
            .collect::<Vec<_>>(),
        vec!["interactive", "io", "script"]
    );
    assert!(snapshots.iter().all(|snapshot| snapshot.lanes.len() == 5));
}
