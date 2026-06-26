use std::cell::RefCell;
use std::rc::Rc;

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;
#[test]
fn reload_allows_additive_and_blocks_breaking_surfaces() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![Box::new(CoreKernelRunner::new(1))];
    let mut runtime = CoreRuntime::boot(plan.clone(), runners).unwrap();

    let mut additive = plan.clone();
    additive.registry_generation = 2;
    additive.contract_surfaces.push(ContractSurface {
        surface_id: "runner:new".into(),
        kind: ContractSurfaceKind::Runner,
        owner_plugin_id: "plugin-a".into(),
        fingerprint: "sha256:new".into(),
        deprecated: false,
    });
    let decision = runtime.reload(additive).unwrap();
    assert!(
        decision
            .changes
            .iter()
            .any(|change| change.compatibility == SurfaceCompatibility::Additive)
    );

    let mut breaking = plan;
    breaking.contract_surfaces[0].fingerprint = "sha256:changed".into();
    let err = runtime.reload(breaking).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
}

#[test]
fn reload_with_runners_swaps_registry_generation_and_rebinds_ready_tasks() {
    let worker_v1 = runner_descriptor("worker", "raw.input", RunnerPurity::Pure);
    let plan_v1 = load_plan(vec![worker_v1.clone()], Vec::new());
    let runners_v1: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker_v1, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan_v1, runners_v1).unwrap();
    runtime.enqueue_task(Task::new("task-before-reload", "raw.input", json!({})));

    let mut worker_v2 = runner_descriptor("worker", "raw.input", RunnerPurity::Pure);
    worker_v2.plugin_generation = 2;
    let mut plan_v2 = load_plan(vec![worker_v2.clone()], Vec::new());
    plan_v2.registry_generation = 2;
    let runners_v2: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker_v2, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.events.push(DomainEvent {
                event_id: "handled-by-v2".into(),
                kind: "runner.v2.handled".into(),
                payload: json!({}),
            });
            result
        })),
        Box::new(CoreKernelRunner::new(2)),
    ];

    runtime.reload_with_runners(plan_v2, runners_v2).unwrap();
    runtime.run_until_idle(4).unwrap();

    assert_eq!(
        runtime
            .tasks()
            .get("task-before-reload")
            .unwrap()
            .task
            .registry_generation,
        2
    );
    assert!(runtime.events().iter().any(|event| {
        event.name == "runner.v2.handled" && event.subject_id.as_deref() == Some("handled-by-v2")
    }));
}

#[test]
fn reload_cancels_clean_running_invocation_and_retries_on_new_generation() {
    let worker_v1 = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan_v1 = load_plan(vec![worker_v1.clone()], Vec::new());
    let calls = Rc::new(RefCell::new(Vec::new()));
    let runners_v1: Vec<Box<dyn Runner>> = vec![
        Box::new(ContinuingRunner::new(worker_v1, calls.clone())),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan_v1, runners_v1).unwrap();
    runtime.enqueue_task(Task::new("running-clean", "sim.work", json!({})));
    runtime.tick_once().unwrap();

    assert_eq!(
        runtime.running_invocations()[0].pollution,
        InvocationPollution::Clean
    );

    let mut worker_v2 = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    worker_v2.plugin_generation = 2;
    let mut plan_v2 = load_plan(vec![worker_v2.clone()], Vec::new());
    plan_v2.registry_generation = 2;
    let runners_v2: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker_v2, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(2)),
    ];

    runtime.reload_with_runners(plan_v2, runners_v2).unwrap();

    assert_eq!(
        calls.borrow().as_slice(),
        &["cancel:running-clean", "dispose:worker"]
    );
    let record = runtime.tasks().get("running-clean").unwrap();
    assert_eq!(record.status, TaskStatus::Ready);
    assert_eq!(record.task.registry_generation, 2);
    assert_eq!(runtime.draining_generation_count(), 0);

    runtime.tick_once().unwrap();
    assert_eq!(
        runtime.tasks().get("running-clean").unwrap().status,
        TaskStatus::Completed
    );
}

#[test]
fn reload_keeps_polluted_running_invocation_in_draining_generation() {
    let effect_v1 = runner_descriptor("effect.chat", "effect.chat.send", RunnerPurity::Effectful);
    let plan_v1 = load_plan(vec![effect_v1.clone()], Vec::new());
    let calls = Rc::new(RefCell::new(Vec::new()));
    let runners_v1: Vec<Box<dyn Runner>> = vec![
        Box::new(ContinuingRunner::new(effect_v1, calls.clone())),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan_v1, runners_v1).unwrap();
    runtime.enqueue_task(Task::new("running-effect", "effect.chat.send", json!({})));
    runtime.tick_once().unwrap();

    assert_eq!(
        runtime.running_invocations()[0].pollution,
        InvocationPollution::Polluted
    );

    let mut effect_v2 =
        runner_descriptor("effect.chat", "effect.chat.send", RunnerPurity::Effectful);
    effect_v2.plugin_generation = 2;
    let mut plan_v2 = load_plan(vec![effect_v2.clone()], Vec::new());
    plan_v2.registry_generation = 2;
    let runners_v2: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(effect_v2, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(2)),
    ];

    runtime.reload_with_runners(plan_v2, runners_v2).unwrap();

    assert!(calls.borrow().is_empty());
    assert_eq!(runtime.draining_generation_count(), 1);
    assert_eq!(
        runtime.tasks().get("running-effect").unwrap().status,
        TaskStatus::Running
    );
    assert!(
        runtime.plugin_generation_states().iter().any(|state| {
            state.generation == 1 && state.phase == PluginGenerationPhase::Draining
        })
    );
}
