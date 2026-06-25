use serde::de::DeserializeOwned;

use crate::*;

fn assert_missing_fields_fail<T: DeserializeOwned>(value: serde_json::Value) {
    assert!(serde_json::from_value::<T>(value).is_err());
}

#[test]
fn task_runner_resource_contracts_roundtrip_json() {
    let task = Task {
        task_id: "task-1".into(),
        kind: "raw.input.chat_message".into(),
        priority: 10,
        ready_at_step: Some(2),
        payload: serde_json::json!({"actor_id": "actor-a"}),
        input_refs: vec!["value:raw-1".into()],
        expected_versions: vec![VersionExpectation {
            ref_id: "state:actor-a".into(),
            expected_version: 7,
        }],
        correlation_id: Some("corr-1".into()),
        idempotency_key: Some("idem-1".into()),
        runner_hint: Some("orchestrator".into()),
        registry_generation: 3,
        required_surfaces: vec!["task_kind:raw.input.chat_message".into()],
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
        accepted_task_kinds: vec!["raw.input.chat_message".into()],
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
    let plan = RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: "default".into(),
        profile_hash: "sha256:profile".into(),
        registry_generation: 1,
        plugins: Vec::new(),
        load_order: vec!["plugin-a".into()],
        runner_bindings: Default::default(),
        contract_surfaces: vec![ContractSurface {
            surface_id: "runner:plugin-a/a".into(),
            kind: ContractSurfaceKind::Runner,
            owner_plugin_id: "plugin-a".into(),
            fingerprint: "sha256:a".into(),
            deprecated: false,
        }],
    };

    let decoded: RuntimeLoadPlan =
        serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
    assert_eq!(decoded, plan);
}

#[test]
fn missing_new_contract_fields_fail_deserialization() {
    assert_missing_fields_fail::<Task>(serde_json::json!({
        "task_id": "task-1",
        "kind": "raw.input"
    }));
    assert_missing_fields_fail::<RunnerDescriptor>(serde_json::json!({
        "runner_id": "runner-a"
    }));
    assert_missing_fields_fail::<RuntimeLoadPlan>(serde_json::json!({
        "lock_version": 1
    }));
    assert_missing_fields_fail::<ResourceRef>(serde_json::json!({
        "ref_id": "resource:1"
    }));
}
