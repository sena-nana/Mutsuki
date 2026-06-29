use serde::de::DeserializeOwned;

use crate::*;

fn assert_missing_fields_fail<T: DeserializeOwned>(value: serde_json::Value) {
    assert!(serde_json::from_value::<T>(value).is_err());
}

fn resource_ref(ref_id: &str, kind_id: &str, semantic: ResourceSemantic) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version: 1,
        },
        semantic,
        provider_id: "resource.local".into(),
        resource_kind: kind_id.into(),
        schema: "bytes.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::MmapFile {
            path: "resource.bin".into(),
            offset: 0,
            len: 4,
            readonly: true,
        },
        size_hint: Some(4),
        content_hash: Some("hash:4".into()),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn resource_provider_compatibility() -> ResourceProviderCompatibility {
    ResourceProviderCompatibility {
        schema_version: "1.0.0".into(),
        required_operations: vec!["read".into(), "export".into()],
        preserves_resource_type_id: true,
        accepts_older_generations: true,
        lease_drain_required: true,
    }
}

#[test]
fn task_runner_resource_contracts_roundtrip_json() {
    let task = Task {
        task_id: "task-1".into(),
        protocol_id: "raw.input.chat_message".into(),
        priority: 10,
        ready_at_step: Some(2),
        payload: serde_json::json!({"actor_id": "actor-a"}),
        input_refs: vec!["value:raw-1".into()],
        output_ref: None,
        continuation_ref: None,
        target_binding_id: Some("binding:chat".into()),
        lease_id: Some("task-lease-1".into()),
        trace_id: Some("trace-1".into()),
        expected_versions: vec![VersionExpectation {
            ref_id: "state:actor-a".into(),
            expected_version: 7,
        }],
        correlation_id: Some("corr-1".into()),
        idempotency_key: Some("idem-1".into()),
        runner_hint: Some("orchestrator".into()),
        registry_generation: 3,
        required_surfaces: vec!["task_protocol:raw.input.chat_message".into()],
        created_sequence: 4,
    };
    assert_eq!(
        serde_json::from_str::<Task>(&serde_json::to_string(&task).unwrap()).unwrap(),
        task
    );

    let descriptor = RunnerDescriptor {
        runner_id: "runner-a".into(),
        plugin_id: "plugin-a".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec!["raw.input.chat_message".into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Cpu,
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: serde_json::json!({"type": "object"}),
        metadata: Default::default(),
        contract_surfaces: vec!["runner:runner-a".into()],
    };
    assert_eq!(
        serde_json::from_str::<RunnerDescriptor>(&serde_json::to_string(&descriptor).unwrap())
            .unwrap(),
        descriptor
    );

    let resource = resource_ref("resource:1", "blob", ResourceSemantic::FrozenValue);
    assert_eq!(
        serde_json::from_str::<ResourceRef>(&serde_json::to_string(&resource).unwrap()).unwrap(),
        resource
    );
}

#[test]
fn plugin_load_plan_roundtrips_and_keeps_surfaces() {
    let provides = PluginProvides {
        runners: Vec::new(),
        protocols: vec![ProtocolDescriptor {
            protocol_id: "im.message.received.v1".into(),
            version: "1.0.0".into(),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: serde_json::json!({"type": "object"}),
            error_schema: serde_json::json!({"type": "object"}),
            codec: "json".into(),
            compatibility: "semver".into(),
        }],
        handler_bindings: vec![HandlerBinding {
            binding_id: "message-handler".into(),
            plugin_id: "plugin-a".into(),
            protocol_id: "im.message.received.v1".into(),
            target_protocol_id: "cap.message.handle".into(),
            target_runner_hint: Some("message.runner".into()),
            pool_id: "default".into(),
            priority: 10,
            policy: "required".into(),
            metadata: Default::default(),
        }],
        resource_schemas: vec!["bytes.v1".into()],
        resource_providers: vec!["resource.local".into()],
        resource_types: vec![ResourceTypeDescriptor {
            kind_id: "blob".into(),
            semantic: ResourceSemantic::FrozenValue,
            schema: "bytes.v1".into(),
            provider_id: "resource.local".into(),
            operations: vec!["read".into(), "export".into()],
            reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
            compatibility: resource_provider_compatibility(),
        }],
        effects: vec!["effect.chat.send".into()],
        streams: vec!["chat.events".into()],
        subscriptions: vec!["chat.messages".into()],
        timers: vec!["heartbeat".into()],
        state_schemas: vec!["state.actor.v1".into()],
        host_backends: vec![HostBackendDescriptor {
            backend_id: "host.backend.builtin".into(),
            kind: HostExtensionKind::PluginBackend,
            supported_deployments: vec![PluginDeploymentKind::Builtin],
            reload_policy: "drain_and_swap".into(),
            drain_required: true,
        }],
        plugin_backends: vec![PluginBackendDescriptor {
            backend_id: "plugin.backend.builtin".into(),
            deployment_kind: PluginDeploymentKind::Builtin,
            task_client_protocol: "mutsuki.task.v1".into(),
            resource_client_protocol: "mutsuki.resource-plan.v1".into(),
            codec_id: Some("codec.json".into()),
            bridge_id: None,
        }],
        codecs: vec![CodecDescriptor {
            codec_id: "codec.json".into(),
            media_type: "application/json".into(),
            version: "1.0.0".into(),
            connection_scoped: true,
        }],
        bridges: vec![BridgeDescriptor {
            bridge_id: "bridge.abi.jsonl".into(),
            deployment_kind: PluginDeploymentKind::Abi,
            codec_ids: vec!["codec.json".into()],
            drain_policy: "connection_drain".into(),
        }],
        scheduler_policies: vec![SchedulerPolicyDescriptor {
            policy_id: "scheduler.fair".into(),
            version: "1.0.0".into(),
            decision_scope: "dispatch_budget".into(),
        }],
        workflows: vec![WorkflowDescriptor {
            workflow_id: "workflow.linear".into(),
            state_resource_kind: "workflow.instance".into(),
            runner_protocol_id: "workflow.linear.run".into(),
            reload_policy: "state_resource_handoff".into(),
        }],
    };
    let plan = RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: "default".into(),
        profile_hash: "sha256:profile".into(),
        registry_generation: 1,
        plugins: vec![PluginManifest {
            plugin_id: "plugin-a".into(),
            version: "0.1.0".into(),
            api_version: "mutsuki-plugin-v1".into(),
            artifact: PluginArtifact {
                artifact_type: ArtifactType::Native,
                path: "native".into(),
                sha256: "sha256:native".into(),
            },
            provides,
            requires: Vec::new(),
            permissions: PermissionGrant {
                effects: vec!["effect.chat.send".into()],
                resources: vec!["read".into()],
            },
            lifecycle: LifecyclePolicy {
                reload_policy: "drain_and_swap".into(),
                unload_timeout_ms: 5000,
                supports_cancel: true,
                supports_dispose: true,
                supports_snapshot: false,
            },
            metadata: Default::default(),
        }],
        load_order: vec!["plugin-a".into()],
        runner_bindings: Default::default(),
        plugin_deployments: [("plugin-a".into(), PluginDeploymentKind::Builtin)].into(),
        capability_graph: RuntimeCapabilityGraph::default(),
        contract_surfaces: vec![
            ContractSurface {
                surface_id: "runner:plugin-a/a".into(),
                kind: ContractSurfaceKind::Runner,
                owner_plugin_id: "plugin-a".into(),
                fingerprint: "sha256:a".into(),
                deprecated: false,
            },
            ContractSurface {
                surface_id: "protocol:im.message.received.v1".into(),
                kind: ContractSurfaceKind::Protocol,
                owner_plugin_id: "plugin-a".into(),
                fingerprint: "protocol:im.message.received.v1:1.0.0".into(),
                deprecated: false,
            },
            ContractSurface {
                surface_id: "handler_binding:message-handler".into(),
                kind: ContractSurfaceKind::HandlerBinding,
                owner_plugin_id: "plugin-a".into(),
                fingerprint: "handler_binding:message-handler".into(),
                deprecated: false,
            },
        ],
    };

    let decoded: RuntimeLoadPlan =
        serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
    assert_eq!(decoded, plan);
}

#[test]
fn surface_occupancy_handle_roundtrips_json() {
    let handle = SurfaceOccupancyHandle {
        handle_id: "subscription:1".into(),
        surface_id: "subscription:chat.messages".into(),
        owner_plugin_id: "plugin-a".into(),
        plugin_generation: 2,
        registry_generation: 7,
        kind: SurfaceOccupancyHandleKind::Subscription,
    };

    assert_eq!(
        serde_json::from_str::<SurfaceOccupancyHandle>(&serde_json::to_string(&handle).unwrap())
            .unwrap(),
        handle
    );
}

#[test]
fn system_extension_descriptors_roundtrip_json() {
    let host_backend = HostBackendDescriptor {
        backend_id: "host.backend.process".into(),
        kind: HostExtensionKind::Bridge,
        supported_deployments: vec![PluginDeploymentKind::Process],
        reload_policy: "drain_and_swap".into(),
        drain_required: true,
    };
    let plugin_backend = PluginBackendDescriptor {
        backend_id: "plugin.backend.process".into(),
        deployment_kind: PluginDeploymentKind::Process,
        task_client_protocol: "mutsuki.task.v1".into(),
        resource_client_protocol: "mutsuki.resource-plan.v1".into(),
        codec_id: Some("codec.json".into()),
        bridge_id: Some("bridge.process.jsonl".into()),
    };
    let codec = CodecDescriptor {
        codec_id: "codec.json".into(),
        media_type: "application/json".into(),
        version: "1.0.0".into(),
        connection_scoped: true,
    };
    let bridge = BridgeDescriptor {
        bridge_id: "bridge.process.jsonl".into(),
        deployment_kind: PluginDeploymentKind::Process,
        codec_ids: vec!["codec.json".into()],
        drain_policy: "connection_drain".into(),
    };
    let policy = SchedulerPolicyDescriptor {
        policy_id: "scheduler.priority".into(),
        version: "1.0.0".into(),
        decision_scope: "dispatch_budget".into(),
    };
    let workflow = WorkflowDescriptor {
        workflow_id: "workflow.retry".into(),
        state_resource_kind: "workflow.instance.retry".into(),
        runner_protocol_id: "workflow.retry.run".into(),
        reload_policy: "drain_or_handoff".into(),
    };

    assert_eq!(
        serde_json::from_str::<HostBackendDescriptor>(
            &serde_json::to_string(&host_backend).unwrap()
        )
        .unwrap(),
        host_backend
    );
    assert_eq!(
        serde_json::from_str::<PluginBackendDescriptor>(
            &serde_json::to_string(&plugin_backend).unwrap()
        )
        .unwrap(),
        plugin_backend
    );
    assert_eq!(
        serde_json::from_str::<CodecDescriptor>(&serde_json::to_string(&codec).unwrap()).unwrap(),
        codec
    );
    assert_eq!(
        serde_json::from_str::<BridgeDescriptor>(&serde_json::to_string(&bridge).unwrap()).unwrap(),
        bridge
    );
    assert_eq!(
        serde_json::from_str::<SchedulerPolicyDescriptor>(&serde_json::to_string(&policy).unwrap())
            .unwrap(),
        policy
    );
    assert_eq!(
        serde_json::from_str::<WorkflowDescriptor>(&serde_json::to_string(&workflow).unwrap())
            .unwrap(),
        workflow
    );
}

#[test]
fn resource_plan_contracts_roundtrip_json() {
    let resource = resource_ref(
        "resource:cow:1",
        "text_buffer",
        ResourceSemantic::CowVersionedState,
    );
    let read_plan = ReadPlan {
        plan_id: "read-plan:1".into(),
        resource: resource.clone(),
        operation: "collect".into(),
        args: serde_json::json!({"range": "all"}),
    };
    let patch = PatchDescriptor {
        patch_id: "patch:1".into(),
        target_ref: resource.clone(),
        base_version: 1,
        conflict_policy: "fail".into(),
        operations: serde_json::json!({"replace": "all"}),
    };
    let write_plan = WritePlan {
        plan_id: "write-plan:1".into(),
        resource: resource.clone(),
        base_version: 1,
        conflict_policy: "fail".into(),
        patch: patch.clone(),
        returning: Some(read_plan.clone()),
    };
    let command = CommandPlan {
        plan_id: "command:1".into(),
        capability: resource_ref(
            "resource:capability:1",
            "db_pool",
            ResourceSemantic::CapabilityResource,
        ),
        operation: "query".into(),
        args: serde_json::json!({"sql": "select 1"}),
        idempotency_key: None,
    };
    let snapshot = SnapshotDescriptor {
        snapshot_ref: resource_ref(
            "resource:snapshot:1",
            "ast_snapshot",
            ResourceSemantic::VersionedSnapshot,
        ),
        source_ref: resource.clone(),
        source_version: 1,
        snapshot_version: 1,
        is_stale: false,
        is_latest: true,
    };
    let receipt = PlanReceipt {
        plan_id: "write-plan:1".into(),
        status: "committed".into(),
        resource_ref: Some(resource),
        snapshot: Some(snapshot),
        new_version: Some(2),
        output: serde_json::Value::Null,
    };

    assert_eq!(
        serde_json::from_str::<ReadPlan>(&serde_json::to_string(&read_plan).unwrap()).unwrap(),
        read_plan
    );
    assert_eq!(
        serde_json::from_str::<PatchDescriptor>(&serde_json::to_string(&patch).unwrap()).unwrap(),
        patch
    );
    assert_eq!(
        serde_json::from_str::<WritePlan>(&serde_json::to_string(&write_plan).unwrap()).unwrap(),
        write_plan
    );
    assert_eq!(
        serde_json::from_str::<TransactionPlan>(
            &serde_json::to_string(&TransactionPlan {
                plan_id: "tx:1".into(),
                operations: vec![write_plan],
                strict: true,
            })
            .unwrap()
        )
        .unwrap()
        .strict,
        true
    );
    assert_eq!(
        serde_json::from_str::<CommandBatch>(
            &serde_json::to_string(&CommandBatch {
                batch_id: "batch:1".into(),
                commands: vec![command.clone()],
                rollback_guarantee: false,
            })
            .unwrap()
        )
        .unwrap()
        .commands,
        vec![command.clone()]
    );
    assert_eq!(
        serde_json::from_str::<SagaPlan>(
            &serde_json::to_string(&SagaPlan {
                saga_id: "saga:1".into(),
                steps: vec![command.clone()],
                compensations: vec![command],
            })
            .unwrap()
        )
        .unwrap()
        .steps
        .len(),
        1
    );
    assert_eq!(
        serde_json::from_str::<PlanReceipt>(&serde_json::to_string(&receipt).unwrap()).unwrap(),
        receipt
    );
}

#[test]
fn error_event_and_trace_contracts_roundtrip_json() {
    let mut error = RuntimeError::new("runtime.test_failed", "contracts.test", "test.route");
    error.evidence.insert("attempt".into(), ScalarValue::Int(1));
    let decoded_error: RuntimeError =
        serde_json::from_str(&serde_json::to_string(&error).unwrap()).unwrap();
    assert_eq!(decoded_error, error);

    let event = RuntimeEvent {
        sequence: 7,
        kind: RuntimeEventKind::Trace,
        name: "trace.closed".into(),
        subject_id: Some("trace-1".into()),
        attributes: [("ok".into(), ScalarValue::Bool(true))].into(),
        error: Some(error),
    };
    assert_eq!(
        serde_json::from_str::<RuntimeEvent>(&serde_json::to_string(&event).unwrap()).unwrap(),
        event
    );

    let span = TraceSpan {
        trace_id: "trace-1".into(),
        span_id: "span-1".into(),
        parent_span_id: None,
        name: "runner.step".into(),
        start: 1.0,
        end: Some(2.0),
        attributes: [("runner_id".into(), ScalarValue::String("worker".into()))].into(),
        status: SpanStatus::Ok,
    };
    assert_eq!(
        serde_json::from_str::<TraceSpan>(&serde_json::to_string(&span).unwrap()).unwrap(),
        span
    );
}

#[test]
fn task_handle_outcome_and_await_contracts_roundtrip_json() {
    let handle = TaskHandle {
        task_id: "child-1".into(),
        protocol_id: "child.work".into(),
        target_binding_id: Some("binding:child".into()),
        cancel_policy: CancelPolicy::Cascade,
        trace_id: Some("trace-1".into()),
        correlation_id: Some("corr-1".into()),
    };
    assert_eq!(
        serde_json::from_str::<TaskHandle>(&serde_json::to_string(&handle).unwrap()).unwrap(),
        handle
    );

    let outcome = TaskOutcome::Completed {
        task_id: "child-1".into(),
        output_ref: Some("value:child".into()),
    };
    assert_eq!(
        serde_json::from_str::<TaskOutcome>(&serde_json::to_string(&outcome).unwrap()).unwrap(),
        outcome
    );

    let task_await = TaskAwait {
        parent_task_id: "parent-1".into(),
        child: handle,
        continuation: TaskStepContinuation {
            continuation: ResourceRef {
                ref_id: "continuation:parent-1".into(),
                resource_id: ResourceId {
                    kind_id: "continuation".into(),
                    slot_id: "continuation:parent-1".into(),
                    generation: 1,
                    version: 1,
                },
                semantic: ResourceSemantic::FrozenValue,
                provider_id: "mutsuki.sdk".into(),
                resource_kind: "continuation".into(),
                schema: "mutsuki.continuation.v1".into(),
                version: 1,
                generation: 1,
                access: ResourceAccess::Inline,
                size_hint: None,
                content_hash: None,
                lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
                lease: None,
                seal_state: ResourceSealState::Sealed,
            },
            wake: Some(WakeCondition::ManualWake),
            reason: Some("sdk.await".into()),
        },
        cancel_policy: CancelPolicy::Cascade,
    };
    assert_eq!(
        serde_json::from_str::<TaskAwait>(&serde_json::to_string(&task_await).unwrap()).unwrap(),
        task_await
    );

    let mut result = RunnerResult::completed("parent-1");
    result.task_await = Some(task_await);
    result.status = RunnerStatus::Waiting;
    assert_eq!(
        serde_json::from_str::<RunnerResult>(&serde_json::to_string(&result).unwrap()).unwrap(),
        result
    );
}

#[test]
fn missing_new_contract_fields_fail_deserialization() {
    assert_missing_fields_fail::<Task>(serde_json::json!({
        "task_id": "task-1",
        "protocol_id": "raw.input"
    }));
    assert_missing_fields_fail::<RunnerDescriptor>(serde_json::json!({
        "runner_id": "runner-a"
    }));
    assert_missing_fields_fail::<RuntimeLoadPlan>(serde_json::json!({
        "lock_version": 1
    }));
    assert_missing_fields_fail::<PluginProvides>(serde_json::json!({
        "runners": [],
        "protocols": [],
        "handler_bindings": [],
        "resource_schemas": [],
        "resource_providers": [],
        "effects": []
    }));
    assert_missing_fields_fail::<ResourceTypeDescriptor>(serde_json::json!({
        "kind_id": "blob",
        "semantic": "frozen_value",
        "schema": "bytes.v1",
        "provider_id": "resource.local",
        "operations": []
    }));
    assert_missing_fields_fail::<SurfaceOccupancyHandle>(serde_json::json!({
        "handle_id": "timer:1"
    }));
    assert_missing_fields_fail::<ResourceRef>(serde_json::json!({
        "ref_id": "resource:1"
    }));
}
