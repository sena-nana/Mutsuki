use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

fn runner_descriptor(id: &str, kind: &str, purity: RunnerPurity) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: id.into(),
        plugin_id: if id == "core.kernel" {
            "core"
        } else {
            "plugin-a"
        }
        .into(),
        plugin_generation: 1,
        accepted_task_kinds: vec![kind.into()],
        purity,
        input_schema: json!({}),
        output_schema: json!({}),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{id}")],
    }
}

fn manifest(
    runners: Vec<RunnerDescriptor>,
    handler_bindings: Vec<HandlerBinding>,
) -> PluginManifest {
    PluginManifest {
        plugin_id: "plugin-a".into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "native".into(),
            sha256: "sha256:native".into(),
        },
        provides: PluginProvides {
            runners,
            protocols: Vec::new(),
            handler_bindings,
            effects: vec!["effect.chat.send".into()],
            streams: vec!["chat.events".into()],
            subscriptions: vec!["chat.messages".into()],
            timers: vec!["heartbeat".into()],
            resource_schemas: vec!["bytes.v1".into()],
            resource_providers: vec!["resource.local".into()],
            state_schemas: vec!["state.actor.v1".into()],
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: vec!["effect.chat.send".into()],
            resources: vec!["read".into(), "write_own".into()],
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "drain_and_swap".into(),
            unload_timeout_ms: 5000,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: false,
        },
        metadata: BTreeMap::new(),
    }
}

fn load_plan(
    runners: Vec<RunnerDescriptor>,
    handler_bindings: Vec<HandlerBinding>,
) -> RuntimeLoadPlan {
    let mut all_runners = runners;
    all_runners.push(CoreKernelRunner::new(1).descriptor().clone());
    let mut plugins = vec![manifest(all_runners, handler_bindings)];
    plugins[0].provides.runners[0].plugin_id = "plugin-a".into();
    RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: "default".into(),
        profile_hash: "sha256:profile".into(),
        registry_generation: 1,
        plugins,
        load_order: vec!["plugin-a".into()],
        runner_bindings: BTreeMap::new(),
        contract_surfaces: vec![
            ContractSurface {
                surface_id: "runner:orchestrator".into(),
                kind: ContractSurfaceKind::Runner,
                owner_plugin_id: "plugin-a".into(),
                fingerprint: "sha256:orchestrator".into(),
                deprecated: false,
            },
            ContractSurface {
                surface_id: "runner:core.kernel".into(),
                kind: ContractSurfaceKind::Runner,
                owner_plugin_id: "core".into(),
                fingerprint: "sha256:core".into(),
                deprecated: false,
            },
        ],
    }
}

fn surface(id: &str, kind: ContractSurfaceKind) -> ContractSurface {
    ContractSurface {
        surface_id: id.into(),
        kind,
        owner_plugin_id: "plugin-a".into(),
        fingerprint: id.into(),
        deprecated: false,
    }
}

fn boot_with_kernel(plan: RuntimeLoadPlan) -> CoreRuntime {
    CoreRuntime::boot(plan, vec![Box::new(CoreKernelRunner::new(1))]).unwrap()
}

fn remove_surfaces(mut plan: RuntimeLoadPlan, surface_ids: &[&str]) -> RuntimeLoadPlan {
    plan.registry_generation = 2;
    plan.contract_surfaces
        .retain(|surface| !surface_ids.contains(&surface.surface_id.as_str()));
    plan
}

fn occupancy_handle(surface_id: &str, kind: SurfaceOccupancyHandleKind) -> SurfaceOccupancyHandle {
    SurfaceOccupancyHandle {
        handle_id: format!("{surface_id}:1"),
        surface_id: surface_id.into(),
        owner_plugin_id: "plugin-a".into(),
        plugin_generation: 1,
        registry_generation: 1,
        kind,
    }
}

struct StaticRunner {
    descriptor: RunnerDescriptor,
    result: Box<dyn Fn(&Task) -> RunnerResult>,
}

struct ContinuingRunner {
    descriptor: RunnerDescriptor,
    calls: Rc<RefCell<Vec<String>>>,
}

impl ContinuingRunner {
    fn new(descriptor: RunnerDescriptor, calls: Rc<RefCell<Vec<String>>>) -> Self {
        Self { descriptor, calls }
    }
}

impl Runner for ContinuingRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, _ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        Ok(tasks
            .into_iter()
            .map(|task| RunnerResult {
                task_id: task.task_id,
                deltas: Vec::new(),
                events: Vec::new(),
                tasks: Vec::new(),
                effects: Vec::new(),
                values: Vec::new(),
                resources: Vec::new(),
                status: RunnerStatus::Continue,
            })
            .collect())
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.calls
            .borrow_mut()
            .push(format!("cancel:{invocation_id}"));
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.calls
            .borrow_mut()
            .push(format!("dispose:{}", self.descriptor.runner_id));
        Ok(())
    }
}

impl StaticRunner {
    fn new(descriptor: RunnerDescriptor, result: impl Fn(&Task) -> RunnerResult + 'static) -> Self {
        Self {
            descriptor,
            result: Box::new(result),
        }
    }
}

impl Runner for StaticRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, _ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        Ok(tasks.iter().map(|task| (self.result)(task)).collect())
    }
}

#[test]
fn task_pool_claims_ready_tasks_in_deterministic_order() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    let mut low = Task::new("b-low", "sim.work", json!({}));
    low.priority = 1;
    let mut high = Task::new("a-high", "sim.work", json!({}));
    high.priority = 10;
    let mut future = Task::new("future", "sim.work", json!({}));
    future.priority = 99;
    future.ready_at_step = Some(9);
    pool.enqueue(low);
    pool.enqueue(high);
    pool.enqueue(future);

    let claimed = pool.claim_ready(&descriptor, 1, 0, 8);
    assert_eq!(
        claimed
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a-high", "b-low"]
    );
    assert_eq!(pool.running_count(), 2);
}

#[test]
fn task_pool_rejects_purity_and_generation_mismatched_claims() {
    let mut pool = TaskPool::default();
    let effectful = runner_descriptor("effect.chat", "sim.work", RunnerPurity::Effectful);
    let committer = runner_descriptor("commit", "sim.work", RunnerPurity::Committer);
    let pure = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);

    let mut work = Task::new("work-1", "sim.work", json!({}));
    work.registry_generation = 1;
    pool.enqueue(work);

    assert!(pool.claim_ready(&effectful, 1, 1, 8).is_empty());
    assert!(pool.claim_ready(&committer, 1, 1, 8).is_empty());
    assert!(pool.claim_ready(&pure, 1, 2, 8).is_empty());

    assert_eq!(pool.rebind_ready_generation(1, 2), 1);
    let claimed = pool.claim_ready(&pure, 1, 2, 8);
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id, "work-1");
}

#[test]
fn pure_runner_explicitly_enqueues_derived_tasks() {
    let orchestrator = runner_descriptor("orchestrator", "raw.input.chat", RunnerPurity::Pure);
    let plan = load_plan(vec![orchestrator.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(orchestrator, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            let mut derived = Task::new(
                format!("{}:memory", task.task_id),
                "sim.memory.retrieve",
                json!({"mode": "fast"}),
            );
            derived.priority = 5;
            derived.input_refs = task.input_refs.clone();
            derived.runner_hint = Some("memory.runner".into());
            derived.correlation_id = task.correlation_id.clone();
            result.tasks.push(derived);
            result
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime.publish_raw_input("raw-1", "raw.input.chat", json!({"text": "hello"}));
    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert!(runtime.tasks().get("raw-1:memory").is_some());
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "runner.step")
    );
}

#[test]
fn task_pool_only_claims_ready_tasks() {
    let descriptor = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let mut pool = TaskPool::default();
    let ready = Task::new("ready", "sim.work", json!({}));
    let waiting = Task::new("waiting", "sim.work", json!({}));
    let blocked = Task::new("blocked", "sim.work", json!({}));
    pool.enqueue(ready);
    pool.enqueue(waiting);
    pool.enqueue(blocked);
    pool.get_mut_for_test("waiting").status = TaskStatus::Waiting;
    pool.get_mut_for_test("blocked").status = TaskStatus::Blocked;

    let claimed = pool.claim_ready(&descriptor, 1, 0, 8);

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id, "ready");
}

fn handler_binding(binding_id: &str, protocol_id: &str, target_task_kind: &str) -> HandlerBinding {
    HandlerBinding {
        binding_id: binding_id.into(),
        plugin_id: "plugin-a".into(),
        protocol_id: protocol_id.into(),
        target_task_kind: target_task_kind.into(),
        target_runner_hint: None,
        pool_id: "default".into(),
        priority: 0,
        policy: "required".into(),
        metadata: BTreeMap::new(),
    }
}

fn boot_error(plan: RuntimeLoadPlan, runners: Vec<Box<dyn Runner>>) -> RuntimeFailure {
    match CoreRuntime::boot(plan, runners) {
        Ok(_) => panic!("runtime boot should fail"),
        Err(error) => error,
    }
}

#[test]
fn handler_bindings_are_queryable_but_do_not_fan_out_tasks() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("message.runner".into());
    binding.priority = 10;
    let plan = load_plan(vec![runner.clone()], vec![binding.clone()]);
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(runner, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    assert_eq!(
        runtime.handler_bindings("im.message.received.v1"),
        vec![&binding]
    );

    runtime.publish_raw_input("raw-1", "im.message.received.v1", json!({"text": "hello"}));
    let report = runtime.tick_once().unwrap();

    assert_eq!(report.claimed_tasks, 0);
    assert!(runtime.tasks().get("raw-1").is_some());
    assert_eq!(runtime.tasks().get("raw-1:message-handler"), None);
}

#[test]
fn handler_binding_load_plan_validation_rejects_missing_task_kind() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let plan = load_plan(
        vec![runner.clone()],
        vec![handler_binding(
            "message-handler",
            "im.message.received.v1",
            "cap.message.missing",
        )],
    );
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(runner, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn handler_binding_load_plan_validation_rejects_bad_runner_hint() {
    let runner = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("missing.runner".into());
    let plan = load_plan(vec![runner.clone()], vec![binding]);
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(runner, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn handler_binding_load_plan_validation_rejects_hint_that_cannot_handle_kind() {
    let hinted = runner_descriptor("hinted.runner", "cap.other", RunnerPurity::Pure);
    let target = runner_descriptor("message.runner", "cap.message.handle", RunnerPurity::Pure);
    let mut binding = handler_binding(
        "message-handler",
        "im.message.received.v1",
        "cap.message.handle",
    );
    binding.target_runner_hint = Some("hinted.runner".into());
    let plan = load_plan(vec![hinted.clone(), target.clone()], vec![binding]);
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(hinted, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(StaticRunner::new(target, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];

    let err = boot_error(plan, runners);

    assert_eq!(err.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

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
fn pure_runner_outputs_are_routed_to_commit_and_effect_tasks() {
    let worker = runner_descriptor("worker", "sim.behavior.evaluate", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.deltas.push(StateDelta {
                target_ref: "state:actor".into(),
                expected_version: 0,
                patch: json!({"intent": "reply"}),
                conflict_policy: ConflictPolicy::Fail,
            });
            result.effects.push(EffectRequest {
                effect_id: "send-1".into(),
                kind: "effect.chat.send".into(),
                payload: json!({"text": "ok"}),
                preconditions: vec![EffectPrecondition {
                    ref_id: "state:actor".into(),
                    expected_version: 0,
                }],
                idempotency_key: Some("send-1".into()),
            });
            result
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.enqueue_task(Task::new("task-1", "sim.behavior.evaluate", json!({})));

    runtime.tick_once().unwrap();

    assert!(runtime.tasks().get("task-1:commit").is_some());
    assert!(runtime.tasks().get("task-1:effect:send-1").is_some());
}

#[test]
fn runner_result_value_and_resource_refs_are_recorded_as_lineage() {
    let worker = runner_descriptor("worker", "sim.resource.produce", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.values.push(ValueRef {
                ref_id: "value:1".into(),
                provider_id: "resource.local".into(),
                schema: "value.small.v1".into(),
                version: 1,
                generation: 1,
                size_hint: Some(12),
                content_hash: Some("hash:value".into()),
                lifetime: ResourceLifetime::Persistent,
                storage: ValueStorage::LocalValueStore,
            });
            result.resources.push(ResourceRef {
                ref_id: "resource:1".into(),
                provider_id: "resource.local".into(),
                resource_kind: "bytes".into(),
                schema: "bytes.v1".into(),
                version: 1,
                generation: 1,
                access: ResourceAccess::MmapFile {
                    path: "resource.bin".into(),
                    offset: 0,
                    len: 3,
                    readonly: true,
                },
                size_hint: Some(3),
                content_hash: Some("hash:resource".into()),
                lifetime: ResourceLifetime::Persistent,
                lease: None,
                seal_state: ResourceSealState::Sealed,
            });
            result
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.enqueue_task(Task::new("task-refs", "sim.resource.produce", json!({})));

    runtime.tick_once().unwrap();

    assert!(runtime.events().iter().any(|event| {
        event.kind == RuntimeEventKind::Resource && event.name == "value.lineage"
    }));
    assert!(runtime.events().iter().any(|event| {
        event.kind == RuntimeEventKind::Resource && event.name == "resource.lineage"
    }));
}

#[test]
fn committer_task_is_the_only_state_store_mutation_path() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![Box::new(CoreKernelRunner::new(1))];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let delta = StateDelta {
        target_ref: "state:actor".into(),
        expected_version: 0,
        patch: json!({"ok": true}),
        conflict_policy: ConflictPolicy::Fail,
    };
    runtime.enqueue_task(Task::new(
        "commit-1",
        "core.commit",
        serde_json::to_value(delta).unwrap(),
    ));

    runtime.tick_once().unwrap();

    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(1, json!({"ok": true}))
    );
}

#[test]
fn resource_manager_supports_value_ref_mmap_cow_and_exclusive_write_lease() {
    let mut resources = ResourceManager::new();
    let small = resources.pack_value("small.v1", json!({"a": 1})).unwrap();
    assert!(matches!(small, PackedValue::Inline(_)));
    let big = resources
        .pack_value("big.v1", json!({"blob": "x".repeat(5000)}))
        .unwrap();
    let value_ref = match big {
        PackedValue::Value(value_ref) => value_ref,
        _ => panic!("large value should be stored by ref"),
    };
    assert_eq!(
        resources.get_value(&value_ref).unwrap()["blob"]
            .as_str()
            .unwrap()
            .len(),
        5000
    );

    let resource = resources
        .create_mmap_resource("bytes.v1", b"abc".to_vec())
        .unwrap();
    assert_eq!(resources.read_resource(&resource).unwrap(), b"abc");
    let blob = resources.create_blob_resource("blob.v1", b"blob-data".to_vec());
    assert!(matches!(blob.access, ResourceAccess::Blob { .. }));
    assert_eq!(resources.read_resource(&blob).unwrap(), b"blob-data");
    let cow = resources.copy_on_write(&resource, b"xyz".to_vec()).unwrap();
    assert_ne!(cow.ref_id, resource.ref_id);
    let lease = resources
        .acquire_write_lease(&resource.ref_id, "runner-a", Some(5))
        .unwrap();
    let updated = resources
        .write_with_lease(&lease, b"def".to_vec(), 2)
        .unwrap();
    assert_eq!(updated.generation, resource.generation + 1);
    assert_eq!(resources.read_resource(&updated).unwrap(), b"def");
}

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

#[test]
fn removed_task_kind_surface_uses_live_task_pool_occupancy() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![Box::new(CoreKernelRunner::new(1))];
    let mut runtime = CoreRuntime::boot(plan.clone(), runners).unwrap();
    runtime.enqueue_task(Task::new("ready-work", "sim.work", json!({})));

    let mut with_surface = plan.clone();
    with_surface.contract_surfaces.push(ContractSurface {
        surface_id: "task_kind:sim.work".into(),
        kind: ContractSurfaceKind::TaskKind,
        owner_plugin_id: "plugin-a".into(),
        fingerprint: "task_kind:sim.work".into(),
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
            .any(|item| { item.surface_id == "task_kind:sim.work" && item.ready_tasks == 1 })
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
fn runner_trace_records_plugin_generation_and_contract_facts() {
    let worker = runner_descriptor("worker", "sim.trace", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(StaticRunner::new(worker, |task| {
            RunnerResult::completed(task.task_id.clone())
        })),
        Box::new(CoreKernelRunner::new(1)),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime.enqueue_task(Task::new("trace-task", "sim.trace", json!({})));

    runtime.tick_once().unwrap();

    let span = runtime
        .trace_spans()
        .iter()
        .find(|span| {
            span.attributes.get("runner_id") == Some(&ScalarValue::String("worker".into()))
        })
        .unwrap();
    assert_eq!(
        span.attributes.get("plugin_id"),
        Some(&ScalarValue::String("plugin-a".into()))
    );
    assert_eq!(
        span.attributes.get("plugin_generation"),
        Some(&ScalarValue::Int(1))
    );
    assert!(span.attributes.contains_key("artifact_hash"));
    assert!(span.attributes.contains_key("descriptor_hash"));
    assert!(span.attributes.contains_key("contract_fingerprint"));
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
