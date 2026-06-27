use serde::de::DeserializeOwned;

use crate::*;

fn assert_missing_fields_fail<T: DeserializeOwned>(value: serde_json::Value) {
    assert!(serde_json::from_value::<T>(value).is_err());
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

    let resource = ResourceRef {
        ref_id: "resource:1".into(),
        provider_id: "resource.local".into(),
        resource_kind: "blob".into(),
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
    };
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
        effects: vec!["effect.chat.send".into()],
        streams: vec!["chat.events".into()],
        subscriptions: vec!["chat.messages".into()],
        timers: vec!["heartbeat".into()],
        state_schemas: vec!["state.actor.v1".into()],
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
    assert_missing_fields_fail::<SurfaceOccupancyHandle>(serde_json::json!({
        "handle_id": "timer:1"
    }));
    assert_missing_fields_fail::<ResourceRef>(serde_json::json!({
        "ref_id": "resource:1"
    }));
}
