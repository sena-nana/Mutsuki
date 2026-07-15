use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;
#[test]
fn deprecated_surface_blocks_new_task_occupancy() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!();
    let mut runtime = CoreRuntime::boot(plan.clone(), runners).unwrap();

    let mut deprecated = plan;
    deprecated.registry_generation = 2;
    deprecated.contract_surfaces[0].deprecated = true;
    runtime.reload_load_plan_only(deprecated).unwrap();

    let mut task = Task::new("deprecated-1", "sim.work", json!({}));
    task.required_surfaces = vec!["runner:orchestrator".into()];
    runtime.enqueue_task(task).unwrap();

    let record = runtime.tasks().get("deprecated-1").unwrap();
    assert_eq!(record.status, TaskStatus::Failed);
    assert_eq!(record.failure.as_ref().unwrap().code, ERR_RELOAD_BLOCKED);
}

#[test]
fn removed_task_protocol_surface_uses_live_task_pool_occupancy() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!();
    let mut runtime = CoreRuntime::boot(plan.clone(), runners).unwrap();
    runtime
        .enqueue_task(Task::new("ready-work", "sim.work", json!({})))
        .unwrap();

    let mut with_surface = plan.clone();
    with_surface.contract_surfaces.push(ContractSurface {
        surface_id: "task_protocol:sim.work".into(),
        kind: ContractSurfaceKind::TaskProtocol,
        owner_plugin_id: "plugin-a".into(),
        fingerprint: "task_protocol:sim.work".into(),
        deprecated: false,
    });
    runtime.reload_load_plan_only(with_surface).unwrap();

    let mut removed = plan;
    removed.registry_generation = 2;
    let err = runtime.reload_load_plan_only(removed).unwrap_err();

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
    runtime
        .enqueue_task(Task::new("ready-effect", "effect.chat.send", json!({})))
        .unwrap();

    let removed = remove_surfaces(plan, &["effect:effect.chat.send"]);

    let err = runtime.reload_load_plan_only(removed).unwrap_err();

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
    runtime.reload_load_plan_only(deprecated).unwrap();

    runtime
        .enqueue_task(Task::new(
            "deprecated-effect",
            "effect.chat.send",
            json!({}),
        ))
        .unwrap();

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
            "mutsuki.std.resource.memory",
            "stream://chat/events",
        )
        .unwrap();

    let removed = remove_surfaces(plan, &["stream:chat.events"]);

    let err = runtime.reload_load_plan_only(removed.clone()).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(
        runtime
            .surface_occupancy()
            .iter()
            .any(|item| { item.surface_id == "stream:chat.events" && item.open_streams == 1 })
    );

    runtime.close_stream(&stream.ref_id).unwrap();
    runtime.reload_load_plan_only(removed).unwrap();
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

    let err = runtime.reload_load_plan_only(removed.clone()).unwrap_err();
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
    runtime.reload_load_plan_only(removed).unwrap();
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
    runtime.reload_load_plan_only(deprecated).unwrap();

    let stream_err = runtime
        .open_stream(
            "chat.events",
            "bytes.v1",
            "mutsuki.std.resource.memory",
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
fn deprecated_resource_surfaces_reject_new_resource_creation() {
    let schema_plan = plan_with_surfaces(&[
        (
            "resource_schema:bytes.v1",
            ContractSurfaceKind::ResourceSchema,
        ),
        (
            "resource_provider:mutsuki.std.resource.memory",
            ContractSurfaceKind::ResourceProvider,
        ),
    ]);
    let mut runtime = boot_with_kernel(schema_plan.clone());

    runtime
        .reload_load_plan_only(deprecated_plan(schema_plan, "resource_schema:bytes.v1"))
        .unwrap();

    let schema_err = runtime
        .register_resource_descriptor(external_resource_ref(
            "resource:bytes",
            "bytes",
            "bytes.v1",
            "mutsuki.std.resource.memory",
        ))
        .unwrap_err();
    assert_eq!(schema_err.error().code, ERR_RELOAD_BLOCKED);

    let provider_plan = plan_with_surfaces(&[
        (
            "resource_schema:text.v1",
            ContractSurfaceKind::ResourceSchema,
        ),
        (
            "resource_provider:mutsuki.std.resource.memory",
            ContractSurfaceKind::ResourceProvider,
        ),
    ]);
    let mut runtime = boot_with_kernel(provider_plan.clone());

    runtime
        .reload_load_plan_only(deprecated_plan(
            provider_plan,
            "resource_provider:mutsuki.std.resource.memory",
        ))
        .unwrap();

    let provider_err = runtime
        .register_resource_descriptor(external_resource_ref(
            "resource:text",
            "text",
            "text.v1",
            "mutsuki.std.resource.memory",
        ))
        .unwrap_err();
    assert_eq!(provider_err.error().code, ERR_RELOAD_BLOCKED);
}

#[test]
fn removed_resource_surfaces_use_live_resource_and_write_lease_occupancy() {
    let plan = plan_with_surfaces(&[
        (
            "resource_schema:bytes.v1",
            ContractSurfaceKind::ResourceSchema,
        ),
        (
            "resource_provider:mutsuki.std.resource.memory",
            ContractSurfaceKind::ResourceProvider,
        ),
    ]);
    let mut runtime = boot_with_kernel(plan.clone());
    let resource = runtime
        .register_resource_descriptor(external_resource_ref(
            "resource:bytes",
            "bytes",
            "bytes.v1",
            "mutsuki.std.resource.memory",
        ))
        .unwrap();
    let _lease = runtime
        .lock_resource(&resource.ref_id, "writer-task", None)
        .unwrap();

    let removed = remove_surfaces(
        plan,
        &[
            "resource_schema:bytes.v1",
            "resource_provider:mutsuki.std.resource.memory",
        ],
    );

    let err = runtime.reload_load_plan_only(removed).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(runtime.surface_occupancy().iter().any(|item| {
        item.surface_id == "resource_schema:bytes.v1"
            && item.resource_refs == 1
            && item.active_leases == 1
    }));
    assert!(runtime.surface_occupancy().iter().any(|item| {
        item.surface_id == "resource_provider:mutsuki.std.resource.memory"
            && item.resource_refs == 1
            && item.active_leases == 1
    }));
}

#[test]
fn removed_resource_schema_surface_uses_resource_cell_lease_occupancy() {
    let plan = plan_with_surfaces(&[(
        "resource_schema:http.connection_pool.v1",
        ContractSurfaceKind::ResourceSchema,
    )]);
    let mut runtime = boot_with_kernel(plan.clone());
    let cell = runtime
        .create_resource_cell(
            "cell:http",
            "http.connection_pool",
            "plugin-http",
            "http.connection_pool.v1",
            "drain",
        )
        .unwrap();
    let lease = runtime
        .acquire_resource_lease(
            &cell.cell_id,
            "task-http",
            "executor-http",
            "exclusive",
            None,
        )
        .unwrap();

    let removed = remove_surfaces(plan, &["resource_schema:http.connection_pool.v1"]);

    let err = runtime.reload_load_plan_only(removed.clone()).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(runtime.surface_occupancy().iter().any(|item| {
        item.surface_id == "resource_schema:http.connection_pool.v1" && item.active_leases == 1
    }));

    runtime.release_resource_lease(&lease).unwrap();
    runtime.reload_load_plan_only(removed).unwrap();
}

#[test]
fn waiting_task_blocks_removed_task_protocol_surface() {
    let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
    let mut plan = load_plan(vec![parent.clone()], Vec::new());
    plan.contract_surfaces.push(surface(
        "task_protocol:parent.work",
        ContractSurfaceKind::TaskProtocol,
    ));
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(parent, |task| {
        await_child_result(task, Task::new("child-1", "child.work", json!({})))
    }));
    let mut runtime = CoreRuntime::boot(plan.clone(), runners).unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();

    let removed = remove_surfaces(plan, &["task_protocol:parent.work"]);

    let err = runtime.reload_load_plan_only(removed).unwrap_err();
    assert_eq!(err.error().code, ERR_RELOAD_BLOCKED);
    assert!(runtime.surface_occupancy().iter().any(|item| {
        item.surface_id == "task_protocol:parent.work" && item.running_invocations == 1
    }));
    assert!(runtime.running_invocations().is_empty());
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

fn plan_with_surfaces(surfaces: &[(&str, ContractSurfaceKind)]) -> RuntimeLoadPlan {
    let mut plan = load_plan(Vec::new(), Vec::new());
    plan.contract_surfaces
        .extend(surfaces.iter().map(|(id, kind)| surface(id, kind.clone())));
    plan
}

fn deprecated_plan(mut plan: RuntimeLoadPlan, surface_id: &str) -> RuntimeLoadPlan {
    plan.registry_generation = 2;
    plan.contract_surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == surface_id)
        .unwrap()
        .deprecated = true;
    plan
}

fn await_child_result(parent: &Task, child: Task) -> RunnerResult {
    let child_handle = TaskHandle {
        task_id: child.task_id.clone(),
        protocol_id: child.protocol_id.clone(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: parent.trace_id.clone(),
        correlation_id: parent.correlation_id.clone(),
    };
    RunnerResult {
        task_id: parent.task_id.clone(),
        output: None,
        deltas: Vec::new(),
        events: Vec::new(),
        tasks: vec![child],
        effects: Vec::new(),
        values: Vec::new(),
        resources: Vec::new(),
        task_await: Some(TaskAwait {
            parent_task_id: parent.task_id.clone(),
            child: child_handle,
            continuation: test_continuation("continuation:parent"),
            cancel_policy: CancelPolicy::Cascade,
        }),
        status: RunnerStatus::Waiting,
    }
}

fn test_continuation(ref_id: &str) -> TaskStepContinuation {
    TaskStepContinuation {
        continuation: ResourceRef {
            ref_id: ref_id.into(),
            resource_id: ResourceId {
                kind_id: "continuation".into(),
                slot_id: ref_id.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "continuation".into(),
            schema: "continuation.v1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        },
        wake: None,
        reason: Some("await child".into()),
    }
}
