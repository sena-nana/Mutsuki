use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;
#[test]
fn deprecated_surface_blocks_new_task_occupancy() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![Box::new(CoreKernelRunner::new(1))];
    let mut runtime = CoreRuntime::boot(plan.clone(), runners).unwrap();

    let mut deprecated = plan;
    deprecated.registry_generation = 2;
    deprecated.contract_surfaces[0].deprecated = true;
    runtime.reload(deprecated).unwrap();

    let mut task = Task::new("deprecated-1", "sim.work", json!({}));
    task.required_surfaces = vec!["runner:orchestrator".into()];
    runtime.enqueue_task(task);

    let record = runtime.tasks().get("deprecated-1").unwrap();
    assert_eq!(record.status, TaskStatus::Failed);
    assert_eq!(record.failure.as_ref().unwrap().code, ERR_RELOAD_BLOCKED);
}

#[test]
fn removed_task_protocol_surface_uses_live_task_pool_occupancy() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![Box::new(CoreKernelRunner::new(1))];
    let mut runtime = CoreRuntime::boot(plan.clone(), runners).unwrap();
    runtime.enqueue_task(Task::new("ready-work", "sim.work", json!({})));

    let mut with_surface = plan.clone();
    with_surface.contract_surfaces.push(ContractSurface {
        surface_id: "task_protocol:sim.work".into(),
        kind: ContractSurfaceKind::TaskProtocol,
        owner_plugin_id: "plugin-a".into(),
        fingerprint: "task_protocol:sim.work".into(),
        deprecated: false,
    });
    runtime.reload(with_surface).unwrap();

    let mut removed = plan;
    removed.registry_generation = 2;
    let err = runtime.reload(removed).unwrap_err();

    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(
        runtime
            .surface_occupancy()
            .iter()
            .any(|item| { item.surface_id == "task_protocol:sim.work" && item.ready_tasks == 1 })
    );
}

#[test]
fn removed_effect_surface_uses_live_effect_inflight_occupancy() {
    let mut plan = load_plan(Vec::new(), Vec::new());
    plan.contract_surfaces.push(surface(
        "effect:effect.chat.send",
        ContractSurfaceKind::Effect,
    ));
    let mut runtime = boot_with_kernel(plan.clone());
    runtime.enqueue_task(Task::new("ready-effect", "effect.chat.send", json!({})));

    let removed = remove_surfaces(plan, &["effect:effect.chat.send"]);

    let err = runtime.reload(removed).unwrap_err();

    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(
        runtime.surface_occupancy().iter().any(|item| {
            item.surface_id == "effect:effect.chat.send" && item.effect_inflight == 1
        })
    );
}

#[test]
fn deprecated_effect_surface_rejects_new_effect_tasks() {
    let mut plan = load_plan(Vec::new(), Vec::new());
    plan.contract_surfaces.push(surface(
        "effect:effect.chat.send",
        ContractSurfaceKind::Effect,
    ));
    let mut runtime = boot_with_kernel(plan.clone());

    let mut deprecated = plan;
    deprecated.registry_generation = 2;
    deprecated
        .contract_surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == "effect:effect.chat.send")
        .unwrap()
        .deprecated = true;
    runtime.reload(deprecated).unwrap();

    runtime.enqueue_task(Task::new(
        "deprecated-effect",
        "effect.chat.send",
        json!({}),
    ));

    let record = runtime.tasks().get("deprecated-effect").unwrap();
    assert_eq!(record.status, TaskStatus::Failed);
    assert_eq!(record.failure.as_ref().unwrap().code, ERR_RELOAD_BLOCKED);
}

#[test]
fn removed_stream_surface_uses_live_stream_occupancy() {
    let mut plan = load_plan(Vec::new(), Vec::new());
    plan.contract_surfaces
        .push(surface("stream:chat.events", ContractSurfaceKind::Stream));
    let mut runtime = boot_with_kernel(plan.clone());
    let stream = runtime
        .open_stream(
            "chat.events",
            "bytes.v1",
            "resource.local",
            "stream://chat/events",
        )
        .unwrap();

    let removed = remove_surfaces(plan, &["stream:chat.events"]);

    let err = runtime.reload(removed.clone()).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(
        runtime
            .surface_occupancy()
            .iter()
            .any(|item| { item.surface_id == "stream:chat.events" && item.open_streams == 1 })
    );

    runtime.close_stream(&stream.ref_id).unwrap();
    runtime.reload(removed).unwrap();
}

#[test]
fn removed_subscription_and_timer_surfaces_use_registered_occupancy() {
    let mut plan = load_plan(Vec::new(), Vec::new());
    plan.contract_surfaces.push(surface(
        "subscription:chat.messages",
        ContractSurfaceKind::Subscription,
    ));
    plan.contract_surfaces
        .push(surface("timer:heartbeat", ContractSurfaceKind::Timer));
    let mut runtime = boot_with_kernel(plan.clone());
    runtime
        .register_surface_occupancy(occupancy_handle(
            "subscription:chat.messages",
            SurfaceOccupancyHandleKind::Subscription,
        ))
        .unwrap();
    runtime
        .register_surface_occupancy(occupancy_handle(
            "timer:heartbeat",
            SurfaceOccupancyHandleKind::Timer,
        ))
        .unwrap();

    let removed = remove_surfaces(plan, &["subscription:chat.messages", "timer:heartbeat"]);

    let err = runtime.reload(removed.clone()).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(runtime.surface_occupancy().iter().any(|item| {
        item.surface_id == "subscription:chat.messages" && item.subscriptions == 1
    }));
    assert!(
        runtime
            .surface_occupancy()
            .iter()
            .any(|item| { item.surface_id == "timer:heartbeat" && item.timers == 1 })
    );

    runtime
        .release_surface_occupancy("subscription:chat.messages:1")
        .unwrap();
    runtime
        .release_surface_occupancy("timer:heartbeat:1")
        .unwrap();
    runtime.reload(removed).unwrap();
}

#[test]
fn deprecated_stream_subscription_and_timer_surfaces_reject_new_occupancy() {
    let mut plan = load_plan(Vec::new(), Vec::new());
    plan.contract_surfaces
        .push(surface("stream:chat.events", ContractSurfaceKind::Stream));
    plan.contract_surfaces.push(surface(
        "subscription:chat.messages",
        ContractSurfaceKind::Subscription,
    ));
    plan.contract_surfaces
        .push(surface("timer:heartbeat", ContractSurfaceKind::Timer));
    let mut runtime = boot_with_kernel(plan.clone());

    let mut deprecated = plan;
    deprecated.registry_generation = 2;
    for surface in &mut deprecated.contract_surfaces {
        if matches!(
            surface.kind,
            ContractSurfaceKind::Stream
                | ContractSurfaceKind::Subscription
                | ContractSurfaceKind::Timer
        ) {
            surface.deprecated = true;
        }
    }
    runtime.reload(deprecated).unwrap();

    let stream_err = runtime
        .open_stream(
            "chat.events",
            "bytes.v1",
            "resource.local",
            "stream://chat/events",
        )
        .unwrap_err();
    assert_eq!(stream_err.error().code, ERR_RELOAD_BLOCKED);

    let subscription_err = runtime
        .register_surface_occupancy(occupancy_handle(
            "subscription:chat.messages",
            SurfaceOccupancyHandleKind::Subscription,
        ))
        .unwrap_err();
    assert_eq!(subscription_err.error().code, ERR_RELOAD_BLOCKED);

    let timer_err = runtime
        .register_surface_occupancy(occupancy_handle(
            "timer:heartbeat",
            SurfaceOccupancyHandleKind::Timer,
        ))
        .unwrap_err();
    assert_eq!(timer_err.error().code, ERR_RELOAD_BLOCKED);
}

#[test]
fn removed_surface_requires_zero_occupancy() {
    let old = vec![ContractSurface {
        surface_id: "runner:old".into(),
        kind: ContractSurfaceKind::Runner,
        owner_plugin_id: "plugin-a".into(),
        fingerprint: "sha256:old".into(),
        deprecated: false,
    }];
    let occupancy = vec![SurfaceOccupancy {
        surface_id: "runner:old".into(),
        ready_tasks: 1,
        running_invocations: 0,
        resource_refs: 0,
        state_refs: 0,
        active_leases: 0,
        open_streams: 0,
        subscriptions: 0,
        timers: 0,
        effect_inflight: 0,
    }];

    let err = crate::registry::compare_surfaces(&old, &[], &occupancy).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
}
