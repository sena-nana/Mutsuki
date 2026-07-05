use super::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::{
    ArtifactType, BridgeDescriptor, CodecDescriptor, ContractSurface, ContractSurfaceKind,
    DomainEvent, HostExtensionDescriptor, HostExtensionKind, LifecyclePolicy, PermissionGrant,
    PluginArtifact, PluginBackendDescriptor, PluginDeploymentKind, RunnerBatchCapability,
    RunnerPayloadCapability, RuntimeCapabilityGraph, RuntimeEvent, RuntimeEventKind,
    RuntimeLoadPlan, RuntimeProfileMode, SchedulerPolicyDescriptor, WorkflowDescriptor,
};
use mutsuki_runtime_contracts::{ExecutionClass, RunnerPurity, RuntimeError};
use serde_json::json;

struct ChildWork;

impl SdkProtocol for ChildWork {
    const PROTOCOL_ID: &'static str = "child.work";
}

struct ParentWork;

impl SdkProtocol for ParentWork {
    const PROTOCOL_ID: &'static str = "parent.work";
}

#[derive(mutsuki_runtime_sdk::SdkProtocol)]
#[mutsuki(protocol_id = "macro.echo", version = "1.0.0")]
struct MacroEchoInput;

#[derive(mutsuki_runtime_sdk::ResourceKind)]
#[mutsuki(
    kind_id = "macro.text_buffer",
    semantic = "cow_versioned_state",
    schema = "macro.text_buffer.v1",
    provider_id = "macro.provider",
    operations("collect", "patch")
)]
struct MacroTextBuffer;

#[mutsuki_runtime_sdk::mutsuki_runner(
    runner_id = "macro.echo.runner",
    plugin_id = "macro.plugin",
    accepts(MacroEchoInput),
    purity = "pure",
    execution_class = "cpu"
)]
async fn macro_echo(ctx: AsyncRunnerContext, task: Task) -> RuntimeResult<RunnerResult> {
    ctx.call::<MacroEchoInput>(json!({"from": task.task_id}))
        .await?;
    Ok(RunnerResult::completed(task.task_id))
}

fn async_descriptor() -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: "async.runner".into(),
        plugin_id: "plugin-a".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec!["parent.work".into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Cpu,
        input_schema: json!({}),
        output_schema: json!({}),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec!["runner:async.runner".into()],
    }
}

struct ManualClient {
    outcomes: Mutex<HashMap<String, TaskOutcome>>,
}

impl RuntimeClient for ManualClient {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        Ok(batch
            .tasks
            .into_iter()
            .map(|task| TaskHandle {
                task_id: task.task_id,
                protocol_id: task.protocol_id,
                target_binding_id: task.target_binding_id,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id,
                correlation_id: task.correlation_id,
            })
            .collect())
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(self
            .outcomes
            .lock()
            .expect("outcomes mutex poisoned")
            .get(&handle.task_id)
            .cloned())
    }
}

#[test]
fn task_handle_future_polls_until_outcome() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let handle = TaskHandle {
        task_id: "task-1".into(),
        protocol_id: "child.work".into(),
        target_binding_id: None,
        cancel_policy: CancelPolicy::Cascade,
        trace_id: None,
        correlation_id: None,
    };
    let mut future = Box::pin(TaskHandleFuture::new(client.clone(), handle));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(future.as_mut().poll(&mut cx).is_pending());
    client
        .outcomes
        .lock()
        .expect("outcomes mutex poisoned")
        .insert(
            "task-1".into(),
            TaskOutcome::Completed {
                task_id: "task-1".into(),
                output_ref: Some("value:1".into()),
            },
        );

    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(_))));
}

#[test]
fn async_runner_adapter_suspends_and_resumes_call() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let descriptor = async_descriptor();
    let mut adapter = AsyncRunnerAdapter::new(
        descriptor,
        client.clone(),
        Box::new(|ctx, task| {
            Box::pin(async move {
                let outcome = ctx.call::<ChildWork>(json!({"from": task.task_id})).await?;
                match outcome {
                    TaskOutcome::Completed { .. } => Ok(RunnerResult::completed(task.task_id)),
                    TaskOutcome::Failed { error, .. } => {
                        Err(mutsuki_runtime_core::RuntimeFailure::new(error))
                    }
                    _ => Err(mutsuki_runtime_core::RuntimeFailure::new(
                        RuntimeError::new(
                            "task.await_unexpected_outcome",
                            "runtime.sdk",
                            "sdk.test",
                        ),
                    )),
                }
            })
        }),
    );

    let first = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("lease:test".into()),
                "invocation:test",
            ),
            Task::new("parent-1", "parent.work", json!({})),
        )
        .unwrap();

    assert_eq!(first.status, RunnerStatus::Waiting);
    assert_eq!(first.tasks[0].task_id, "parent-1:call:1");
    client
        .outcomes
        .lock()
        .expect("outcomes mutex poisoned")
        .insert(
            "parent-1:call:1".into(),
            TaskOutcome::Completed {
                task_id: "parent-1:call:1".into(),
                output_ref: None,
            },
        );

    let second = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                2,
                "executor:test",
                Some("lease:test-2".into()),
                "invocation:test-2",
            ),
            Task::new("parent-1", "parent.work", json!({})),
        )
        .unwrap();

    assert_eq!(second.status, RunnerStatus::Completed);
}

#[test]
fn async_runner_adapter_cancel_removes_invocation_by_invocation_id() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let descriptor = async_descriptor();
    let mut adapter = AsyncRunnerAdapter::new(
        descriptor,
        client.clone(),
        Box::new(|ctx, task| {
            Box::pin(async move {
                ctx.call::<ChildWork>(json!({"from": task.task_id})).await?;
                Ok(RunnerResult::completed(task.task_id))
            })
        }),
    );

    let first = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("lease:test".into()),
                "invocation:one",
            ),
            Task::new("parent-1", "parent.work", json!({})),
        )
        .unwrap();

    assert_eq!(first.status, RunnerStatus::Waiting);
    client
        .outcomes
        .lock()
        .expect("outcomes mutex poisoned")
        .insert(
            "parent-1:call:1".into(),
            TaskOutcome::Completed {
                task_id: "parent-1:call:1".into(),
                output_ref: None,
            },
        );

    adapter.cancel("invocation:one").unwrap();
    let second = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                2,
                "executor:test",
                Some("lease:test-2".into()),
                "invocation:two",
            ),
            Task::new("parent-1", "parent.work", json!({})),
        )
        .unwrap();

    assert_eq!(second.status, RunnerStatus::Waiting);
    assert_eq!(second.tasks[0].task_id, "parent-1:call:1");
}

#[test]
fn async_runner_adapter_emits_generic_child_task_with_trace_context() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let descriptor = async_descriptor();
    let mut adapter = AsyncRunnerAdapter::new(
        descriptor,
        client,
        Box::new(|ctx, task| {
            Box::pin(async move {
                ctx.call::<ChildWork>(json!({"from": task.task_id})).await?;
                Ok(RunnerResult::completed(task.task_id))
            })
        }),
    );
    let mut task = Task::new("parent-1", "parent.work", json!({}));
    task.trace_id = Some("trace-1".into());
    task.correlation_id = Some("corr-1".into());

    let first = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("lease:test".into()),
                "invocation:test",
            ),
            task,
        )
        .unwrap();

    assert_eq!(first.status, RunnerStatus::Waiting);
    assert_eq!(first.tasks[0].protocol_id, "child.work");
    assert_eq!(first.tasks[0].trace_id.as_deref(), Some("trace-1"));
    assert_eq!(first.tasks[0].correlation_id.as_deref(), Some("corr-1"));
    let task_await = first.task_await.as_ref().unwrap();
    assert_eq!(task_await.cancel_policy, CancelPolicy::Cascade);
    assert_eq!(task_await.child.trace_id.as_deref(), Some("trace-1"));
    assert_eq!(task_await.child.correlation_id.as_deref(), Some("corr-1"));
}

#[test]
fn async_runner_adapter_emits_explicit_cancel_policy_descriptor() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let descriptor = async_descriptor();
    let mut adapter = AsyncRunnerAdapter::new(
        descriptor,
        client,
        Box::new(|ctx, task| {
            Box::pin(async move {
                ctx.call_with_cancel_policy::<ChildWork>(
                    json!({"from": task.task_id}),
                    CancelPolicy::Detach,
                )
                .await?;
                Ok(RunnerResult::completed(task.task_id))
            })
        }),
    );

    let first = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("lease:test".into()),
                "invocation:test",
            ),
            Task::new("parent-1", "parent.work", json!({})),
        )
        .unwrap();

    let task_await = first.task_await.as_ref().unwrap();
    assert_eq!(task_await.cancel_policy, CancelPolicy::Detach);
    assert_eq!(task_await.child.cancel_policy, CancelPolicy::Detach);
}

#[test]
fn async_runner_adapter_rejects_self_call_when_policy_disallows_it() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let descriptor = async_descriptor();
    let mut adapter = AsyncRunnerAdapter::new(
        descriptor,
        client,
        Box::new(|ctx, task| {
            Box::pin(async move {
                let task_id = task.task_id.clone();
                ctx.call_targeted::<ParentWork>(
                    "binding:self",
                    "async.runner",
                    json!({"from": task_id}),
                )
                .await?;
                Ok(RunnerResult::completed(task.task_id))
            })
        }),
    )
    .with_self_call_policy(false);

    let error = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("lease:test".into()),
                "invocation:test",
            ),
            Task::new("parent-1", "parent.work", json!({})),
        )
        .unwrap_err();

    assert_eq!(error.error().code, "task.self_call_blocked");
}

#[test]
fn async_runner_adapter_emits_targeted_child_task_descriptor() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let descriptor = async_descriptor();
    let mut adapter = AsyncRunnerAdapter::new(
        descriptor,
        client,
        Box::new(|ctx, task| {
            Box::pin(async move {
                ctx.call_targeted::<ChildWork>(
                    "binding:child",
                    "child.runner",
                    json!({"from": task.task_id}),
                )
                .await?;
                Ok(RunnerResult::completed(task.task_id))
            })
        }),
    );

    let first = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("lease:test".into()),
                "invocation:test",
            ),
            Task::new("parent-1", "parent.work", json!({})),
        )
        .unwrap();

    assert_eq!(first.status, RunnerStatus::Waiting);
    assert_eq!(
        first.tasks[0].target_binding_id.as_deref(),
        Some("binding:child")
    );
    assert_eq!(first.tasks[0].runner_hint.as_deref(), Some("child.runner"));
    assert_eq!(
        first
            .task_await
            .as_ref()
            .unwrap()
            .child
            .target_binding_id
            .as_deref(),
        Some("binding:child")
    );
}

#[test]
fn plugin_builder_loads_manifest_runners_and_host_services() {
    let descriptor = async_descriptor();
    let plugin = PluginBuilder::new("plugin-a")
        .version("1.2.3")
        .protocol::<MacroEchoInput>()
        .resource_type::<MacroTextBuffer>()
        .handler_binding(
            HandlerBindingBuilder::from_protocols::<MacroEchoInput, MacroEchoInput>(
                "binding:macro.echo",
                "plugin-a",
            )
            .target_runner_hint("macro.echo.runner")
            .build(),
        )
        .runner(Box::new(TestRunner {
            descriptor: descriptor.clone(),
        }))
        .host_service("service.echo", Arc::new(String::from("ready")), None)
        .build();

    assert_eq!(plugin.manifest.plugin_id, "plugin-a");
    assert_eq!(plugin.manifest.version, "1.2.3");
    assert_eq!(plugin.manifest.provides.runners, vec![descriptor]);
    assert_eq!(
        plugin.manifest.provides.protocols[0].protocol_id,
        "macro.echo"
    );
    assert_eq!(
        plugin.manifest.provides.resource_types[0].kind_id,
        "macro.text_buffer"
    );
    assert_eq!(
        plugin.manifest.provides.handler_bindings[0]
            .target_runner_hint
            .as_deref(),
        Some("macro.echo.runner")
    );
    assert_eq!(plugin.runners.len(), 1);
    assert_eq!(plugin.host_services[0].service_id, "service.echo");

    let mut loader =
        BuiltinPluginLoader::new().with_plugin(Box::new(PluginBuilder::new("plugin-b")));
    let loaded = loader.load_plugins().unwrap();
    assert_eq!(loaded[0].manifest.plugin_id, "plugin-b");
}

#[test]
fn descriptor_builders_create_sdk_authoring_surfaces() {
    let protocol = ProtocolDescriptorBuilder::new("builder.protocol")
        .version("2.0.0")
        .input_schema(json!({"type": "object"}))
        .build();
    let runner = RunnerDescriptorBuilder::new("builder.runner", "builder.plugin")
        .accepts::<MacroEchoInput>()
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Cpu)
        .batch_capability(RunnerBatchCapability {
            preferred_batch_size: 32,
            max_batch_entries: 128,
            ..Default::default()
        })
        .payload_capability(RunnerPayloadCapability {
            layouts: vec![PayloadLayout::Row, PayloadLayout::BinaryPacked],
            preferred_layout: PayloadLayout::BinaryPacked,
            zero_copy: true,
        })
        .build();
    let binding = HandlerBindingBuilder::from_protocols::<MacroEchoInput, MacroEchoInput>(
        "binding:builder",
        "builder.plugin",
    )
    .target_runner_hint("builder.runner")
    .pool_id("cpu")
    .priority(7)
    .build();
    let resource =
        ResourceTypeDescriptorBuilder::new("builder.resource", ResourceSemantic::CowVersionedState)
            .schema("builder.resource.v1")
            .provider_id("builder.provider")
            .operations(["collect", "patch"])
            .build();

    assert_eq!(protocol.version, "2.0.0");
    assert_eq!(runner.accepted_protocol_ids, vec!["macro.echo".to_string()]);
    assert_eq!(runner.contract_surfaces, vec!["runner:builder.runner"]);
    assert_eq!(runner.batch.preferred_batch_size, 32);
    assert_eq!(runner.batch.max_batch_entries, 128);
    assert_eq!(runner.payload.preferred_layout, PayloadLayout::BinaryPacked);
    assert_eq!(binding.pool_id, "cpu");
    assert_eq!(binding.priority, 7);
    assert_eq!(resource.provider_id, "builder.provider");
    assert_eq!(
        resource.compatibility.required_operations,
        vec!["collect".to_string(), "patch".to_string()]
    );
}

#[test]
fn batch_helpers_build_submit_and_payload_protocol_shapes() {
    let mut first = Task::new("batch-task-1", "macro.echo", json!({"n": 1}));
    first.resource_requirements = vec![TaskOptions::read("resource:shared", Some(7))];
    first.ordering = TaskOptions::strict_sequence("seq:macro");
    let second = Task::new("batch-task-2", "macro.echo", json!({"n": 2}));

    let batch = TaskBatchBuilder::new("batch:sdk")
        .tick_id("tick:42")
        .payload_layout(PayloadLayout::BinaryPacked)
        .task(first.clone())
        .task(second.clone())
        .build();

    assert_eq!(batch.batch_id, "batch:sdk");
    assert_eq!(batch.tick_id.as_deref(), Some("tick:42"));
    assert_eq!(batch.tasks.len(), 2);
    assert_eq!(batch.payload_layout, PayloadLayout::BinaryPacked);
    assert_eq!(
        batch.tasks[0].resource_requirements[0],
        ResourceRequirement {
            ref_id: "resource:shared".into(),
            mode: ResourceAccessMode::Read,
            expected_version: Some(7),
        }
    );

    let row = BatchPayloadBuilder::row_tasks(&[first, second]);
    assert!(matches!(row, BatchPayload::Row { entries } if entries.len() == 2));
    let packed = BatchPayloadBuilder::binary_packed("u32-le", vec![1, 0, 0, 0], 1);
    assert!(matches!(
        packed,
        BatchPayload::BinaryPacked { buffer }
            if buffer.encoding == "u32-le" && buffer.row_count == 1
    ));
}

#[test]
fn derive_macros_generate_protocol_resource_and_runner_descriptors() {
    let protocol = MacroEchoInput::descriptor();
    let resource = MacroTextBuffer::descriptor();
    let runner = macro_echo_descriptor();

    assert_eq!(protocol.protocol_id, "macro.echo");
    assert_eq!(protocol.version, "1.0.0");
    assert_eq!(resource.kind_id, "macro.text_buffer");
    assert_eq!(resource.semantic, ResourceSemantic::CowVersionedState);
    assert_eq!(resource.operations, vec!["collect", "patch"]);
    assert_eq!(runner.runner_id, "macro.echo.runner");
    assert_eq!(runner.plugin_id, "macro.plugin");
    assert_eq!(runner.accepted_protocol_ids, vec!["macro.echo".to_string()]);
}

#[test]
fn macro_generated_runner_adapter_suspends_with_child_task_and_await() {
    let client = Arc::new(ManualClient {
        outcomes: Mutex::new(HashMap::new()),
    });
    let mut adapter = macro_echo_adapter(client);

    let first = adapter
        .run_one_for_test(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some("lease:test".into()),
                "invocation:test",
            ),
            Task::new("macro-parent", "macro.echo", json!({})),
        )
        .unwrap();

    assert_eq!(first.status, RunnerStatus::Waiting);
    assert_eq!(first.tasks[0].task_id, "macro-parent:call:1");
    assert_eq!(first.tasks[0].protocol_id, "macro.echo");
    let task_await = first.task_await.as_ref().unwrap();
    assert_eq!(task_await.parent_task_id, "macro-parent");
    assert_eq!(task_await.child.task_id, "macro-parent:call:1");
}

#[test]
fn host_service_registry_freezes_and_rejects_invalid_access() {
    let registry = HostServiceRegistry::new();
    registry
        .register("service.echo", Arc::new(String::from("ready")))
        .unwrap();
    assert_eq!(
        registry.require::<String>("service.echo").unwrap().as_str(),
        "ready"
    );
    assert!(registry.register("service.echo", Arc::new(1usize)).is_err());
    assert!(registry.require::<usize>("service.echo").is_err());
    assert!(registry.require::<String>("service.missing").is_err());

    registry.freeze();
    assert!(registry.is_frozen());
    assert!(registry.register("service.late", Arc::new(1usize)).is_err());
}

#[test]
fn static_capability_broker_serves_only_active_load_plan_surfaces() {
    let plan = capability_plan();
    let broker = StaticCapabilityBroker::from_load_plan(&plan);

    assert!(
        broker
            .require_capability("host_extension:host.extension.builtin")
            .is_ok()
    );
    assert_eq!(
        broker
            .require_host_extension("host.extension.builtin")
            .unwrap()
            .extension_id,
        "host.extension.builtin"
    );
    assert_eq!(
        broker
            .require_plugin_backend("plugin.backend.builtin")
            .unwrap()
            .backend_id,
        "plugin.backend.builtin"
    );
    assert_eq!(
        broker.require_codec("codec.json").unwrap().codec_id,
        "codec.json"
    );
    assert_eq!(
        broker.require_bridge("bridge.abi.jsonl").unwrap().bridge_id,
        "bridge.abi.jsonl"
    );
    assert_eq!(
        broker
            .require_scheduler_policy("scheduler.fair")
            .unwrap()
            .policy_id,
        "scheduler.fair"
    );
    assert_eq!(
        broker
            .require_workflow("workflow.linear")
            .unwrap()
            .workflow_id,
        "workflow.linear"
    );
    assert!(
        broker
            .require_plugin_backend("plugin.backend.pruned")
            .is_err()
    );
}

#[test]
fn config_and_event_bridge_do_not_invent_missing_values_or_subscriptions() {
    let config = StaticConfigProvider::new(BTreeMap::from([(
        ("plugin-a".into(), "limit".into()),
        json!(3),
    )]));
    assert_eq!(
        config.get_config("plugin-a", "limit").unwrap(),
        Some(json!(3))
    );
    assert_eq!(config.get_config("plugin-a", "missing").unwrap(), None);

    let bridge = RecordingEventBridge::default();
    bridge
        .publish_runtime_event(RuntimeEvent {
            sequence: 1,
            kind: RuntimeEventKind::Host,
            name: "host.ready".into(),
            subject_id: Some("host".into()),
            attributes: BTreeMap::new(),
            error: None,
        })
        .unwrap();
    bridge
        .publish_domain_event(DomainEvent {
            event_id: "domain-1".into(),
            kind: "test.domain".into(),
            payload: json!({"ok": true}),
        })
        .unwrap();
    bridge.flush().unwrap();

    assert_eq!(bridge.runtime_events().len(), 1);
    assert_eq!(bridge.domain_events().len(), 1);
    assert_eq!(bridge.flush_count(), 1);
}

#[test]
fn task_submitter_adapter_preserves_task_handle_and_outcome_contract() {
    let submitter = Arc::new(ManualSubmitter {
        outcomes: Mutex::new(HashMap::from([(
            "task-1".into(),
            TaskOutcome::Completed {
                task_id: "task-1".into(),
                output_ref: Some("value:1".into()),
            },
        )])),
        cancelled: Mutex::new(Vec::new()),
    });
    let client = TaskSubmitterRuntimeClient::new(submitter.clone());
    let handle = client
        .submit_task(Task::new("task-1", "raw.input", json!({})))
        .unwrap();

    assert_eq!(handle.task_id, "task-1");
    assert!(matches!(
        client.task_outcome(&handle).unwrap(),
        Some(TaskOutcome::Completed { output_ref, .. }) if output_ref.as_deref() == Some("value:1")
    ));
    submitter.cancel_task(&handle).unwrap();
    assert_eq!(
        *submitter.cancelled.lock().expect("cancel mutex poisoned"),
        vec!["task-1".to_string()]
    );
}

struct TestRunner {
    descriptor: RunnerDescriptor,
}

impl Runner for TestRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let results = batch
            .entries
            .iter()
            .map(|entry| EntryCompletion {
                entry_id: entry.entry_id.clone(),
                task_id: entry.task_id.clone(),
                result: Some(RunnerResult::completed(entry.task_id.clone())),
                error: None,
            })
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }
}

struct ManualSubmitter {
    outcomes: Mutex<HashMap<String, TaskOutcome>>,
    cancelled: Mutex<Vec<String>>,
}

impl TaskSubmitter for ManualSubmitter {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        Ok(batch
            .tasks
            .into_iter()
            .map(|task| TaskHandle {
                task_id: task.task_id,
                protocol_id: task.protocol_id,
                target_binding_id: task.target_binding_id,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id,
                correlation_id: task.correlation_id,
            })
            .collect())
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.cancelled
            .lock()
            .expect("cancel mutex poisoned")
            .push(handle.task_id.clone());
        Ok(())
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        Ok(self
            .outcomes
            .lock()
            .expect("outcome mutex poisoned")
            .get(&handle.task_id)
            .cloned())
    }
}

fn capability_plan() -> RuntimeLoadPlan {
    let host_extension = HostExtensionDescriptor {
        extension_id: "host.extension.builtin".into(),
        kind: HostExtensionKind::PluginBackend,
        supported_deployments: vec![PluginDeploymentKind::Builtin],
        reload_policy: "static".into(),
        drain_required: false,
    };
    let plugin_backend = PluginBackendDescriptor {
        backend_id: "plugin.backend.builtin".into(),
        deployment_kind: PluginDeploymentKind::Builtin,
        task_client_protocol: "mutsuki.task.v1".into(),
        resource_client_protocol: "mutsuki.resource-plan.v1".into(),
        codec_id: Some("codec.json".into()),
        bridge_id: Some("bridge.abi.jsonl".into()),
    };
    let codec = CodecDescriptor {
        codec_id: "codec.json".into(),
        media_type: "application/json".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    };
    let bridge = BridgeDescriptor {
        bridge_id: "bridge.abi.jsonl".into(),
        deployment_kind: PluginDeploymentKind::Abi,
        codec_ids: vec!["codec.json".into()],
        drain_policy: "connection_drain".into(),
    };
    let scheduler = SchedulerPolicyDescriptor {
        policy_id: "scheduler.fair".into(),
        version: "1.0.0".into(),
        decision_scope: "dispatch_budget".into(),
    };
    let workflow = WorkflowDescriptor {
        workflow_id: "workflow.linear".into(),
        state_resource_kind: "workflow.instance".into(),
        runner_protocol_id: "workflow.linear.run".into(),
        reload_policy: "state_resource_handoff".into(),
    };
    RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: "test".into(),
        profile_hash: "sha256:test".into(),
        registry_generation: 1,
        plugins: vec![mutsuki_runtime_contracts::PluginManifest {
            plugin_id: "plugin-a".into(),
            version: "0.1.0".into(),
            api_version: "mutsuki-plugin-v1".into(),
            artifact: PluginArtifact {
                artifact_type: ArtifactType::Native,
                path: "native".into(),
                sha256: "sha256:native".into(),
            },
            provides: mutsuki_runtime_contracts::PluginProvides {
                host_extensions: vec![host_extension],
                plugin_backends: vec![plugin_backend],
                codecs: vec![codec],
                bridges: vec![bridge],
                scheduler_policies: vec![scheduler],
                workflows: vec![workflow],
                ..mutsuki_runtime_contracts::PluginProvides::default()
            },
            requires: Vec::new(),
            permissions: PermissionGrant {
                effects: Vec::new(),
                resources: Vec::new(),
            },
            lifecycle: LifecyclePolicy {
                reload_policy: "drain_and_swap".into(),
                unload_timeout_ms: 5000,
                supports_cancel: true,
                supports_dispose: true,
                supports_snapshot: false,
            },
            metadata: BTreeMap::new(),
        }],
        load_order: vec!["plugin-a".into()],
        runner_bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        capability_graph: RuntimeCapabilityGraph {
            profile_mode: RuntimeProfileMode::LockedBuiltin,
            provided_capabilities: vec![
                "host_extension:host.extension.builtin".into(),
                "plugin_backend:plugin.backend.builtin".into(),
                "plugin_backend:plugin.backend.pruned".into(),
                "codec:codec.json".into(),
                "bridge:bridge.abi.jsonl".into(),
                "scheduler_policy:scheduler.fair".into(),
                "workflow:workflow.linear".into(),
            ],
            required_capabilities: Vec::new(),
            active_capabilities: vec![
                "host_extension:host.extension.builtin".into(),
                "plugin_backend:plugin.backend.builtin".into(),
                "codec:codec.json".into(),
                "bridge:bridge.abi.jsonl".into(),
                "scheduler_policy:scheduler.fair".into(),
                "workflow:workflow.linear".into(),
            ],
            active_capability_providers: Vec::new(),
            active_resource_providers: Vec::new(),
            active_host_extensions: vec!["host.extension.builtin".into()],
            active_plugin_backends: vec!["plugin.backend.builtin".into()],
            active_codecs: vec!["codec.json".into()],
            active_bridges: vec!["bridge.abi.jsonl".into()],
            active_scheduler_policies: vec!["scheduler.fair".into()],
            active_workflows: vec!["workflow.linear".into()],
            permission_audit: Vec::new(),
        },
        contract_surfaces: vec![ContractSurface {
            surface_id: "host_extension:host.extension.builtin".into(),
            kind: ContractSurfaceKind::HostExtension,
            owner_plugin_id: "plugin-a".into(),
            fingerprint: "sha256:host.extension.builtin".into(),
            deprecated: false,
        }],
    }
}
