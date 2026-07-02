use std::io::Cursor;
use std::sync::Arc;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{CoreRuntime, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{PluginBuilder, ResourcePlanGateway, ResourceProviderGateway};
use serde_json::json;

use crate::{JsonlRunner, NativeRunner, RuntimeBootstrapper, runner_manifest_with_artifact};

use super::helpers::{
    abi_plugin_fixture, descriptor, host_with_echo_runner, runtime_profile,
    runtime_profile_with_deployment,
};

#[test]
fn runtime_bootstrapper_boots_runtime_and_runs_runner_loop() {
    let mut runtime: CoreRuntime = host_with_echo_runner()
        .into_runtime(runtime_profile())
        .unwrap();

    runtime
        .enqueue_task(Task::new("task-1", "raw.input", json!({"ok": true})))
        .unwrap();
    let report = runtime.run_until_idle(4).unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(
        runtime.tasks().get("task-1").unwrap().status,
        TaskStatus::Completed
    );
}

#[test]
fn runtime_bootstrapper_can_boot_host_runtime_control_plane() {
    let mut runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let submitted = runtime
        .dispatch(crate::HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-1",
            "raw.input",
            json!({"ok": true}),
        ))))
        .unwrap();
    assert_eq!(
        submitted,
        crate::HostRuntimeReply::TaskSubmitted("task-1".into())
    );

    let reply = runtime
        .dispatch(crate::HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
        .unwrap();

    let crate::HostRuntimeReply::Idle(report) = reply else {
        panic!("expected idle reply");
    };
    assert_eq!(report.completed_tasks, 1);
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Completed));
}

#[test]
fn abi_plugin_boots_through_registered_abi_runner_bridge() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(manifest);
    host.register_abi_runner(Box::new(JsonlRunner::new(
        runner_descriptor,
        reader,
        writer,
    )));

    let runtime = host.into_runtime(runtime_profile_with_deployment(
        "plugin-abi",
        PluginDeploymentKind::Abi,
    ));

    assert!(runtime.is_ok());
}

#[test]
fn abi_plugin_runner_requires_active_plugin_backend_descriptor() {
    let mut runner_descriptor = descriptor("abi.missing.backend", "abi.work");
    runner_descriptor.plugin_id = "plugin-abi".into();
    let manifest = runner_manifest_with_artifact(
        "plugin-abi",
        PluginArtifact {
            artifact_type: ArtifactType::Abi,
            path: "plugin-abi.so".into(),
            sha256: "sha256:abi".into(),
        },
        vec![runner_descriptor.clone()],
    );
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(manifest);
    host.register_abi_runner(Box::new(JsonlRunner::new(
        runner_descriptor,
        reader,
        writer,
    )));

    let error = host
        .into_runtime(runtime_profile_with_deployment(
            "plugin-abi",
            PluginDeploymentKind::Abi,
        ))
        .err()
        .expect("abi runner without active backend should fail");

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("capability"),
        Some(&ScalarValue::String("plugin_backend:Abi".into()))
    );
}

#[test]
fn enabled_plugin_runner_requires_matching_deployment_bridge() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let profile = runtime_profile_with_deployment("plugin-abi", PluginDeploymentKind::Abi);
    let mut missing_bridge_host = RuntimeBootstrapper::new();
    missing_bridge_host.register_manifest(manifest.clone());

    let missing_bridge = missing_bridge_host
        .into_runtime(profile.clone())
        .err()
        .unwrap();

    assert_eq!(missing_bridge.error().code, ERR_RUNNER_NOT_FOUND);

    let mut mismatched_host = RuntimeBootstrapper::new();
    mismatched_host.register_manifest(manifest);
    mismatched_host.register_runner(Box::new(NativeRunner::new(
        runner_descriptor,
        |_ctx, tasks| {
            Ok(tasks
                .into_iter()
                .map(|task| RunnerResult::completed(task.task_id))
                .collect())
        },
    )));

    let mismatched = mismatched_host.into_runtime(profile).err().unwrap();

    assert_eq!(mismatched.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn loaded_plugin_resource_provider_is_injected_into_host_runtime() {
    let provider_id = "mutsuki.host.boot-resource";
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(resource_provider_plugin(
        provider_id,
        Some(Arc::new(BootResourceProvider)),
    ));

    let mut runtime = host
        .into_host_runtime(resource_provider_profile())
        .expect("resource provider plugin should boot");
    let created = runtime
        .dispatch(crate::HostRuntimeCommand::CreateBlobResource {
            schema: "text.v1".into(),
            bytes: b"plugin".to_vec(),
        })
        .unwrap();

    let crate::HostRuntimeReply::ResourceCreated(resource) = created else {
        panic!("expected resource created reply");
    };
    assert_eq!(resource.provider_id, provider_id);

    let bytes = runtime
        .dispatch(crate::HostRuntimeCommand::CollectReadPlan(Box::new(
            ReadPlan {
                plan_id: "read:boot".into(),
                resource,
                operation: "collect".into(),
                args: serde_json::Value::Null,
            },
        )))
        .unwrap();
    assert_eq!(
        bytes,
        crate::HostRuntimeReply::ResourceBytes(b"plugin".to_vec())
    );
}

#[test]
fn active_resource_provider_requires_loaded_provider_instance() {
    let provider_id = "mutsuki.host.boot-resource";
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(resource_provider_plugin(provider_id, None));

    let error = host
        .into_host_runtime(resource_provider_profile())
        .err()
        .expect("active resource provider without instance should fail");

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("provider_id"),
        Some(&ScalarValue::String(provider_id.into()))
    );
}

fn resource_provider_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "resource-provider".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec!["plugin-resource".into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}

fn resource_provider_plugin(
    provider_id: &str,
    provider: Option<Arc<dyn ResourceProviderGateway>>,
) -> mutsuki_runtime_sdk::LoadedPlugin {
    let mut builder = PluginBuilder::new("plugin-resource")
        .resource_provider(provider_id)
        .resource_type_descriptor(ResourceTypeDescriptor {
            kind_id: "mutsuki.host.boot-resource.blob".into(),
            semantic: ResourceSemantic::FrozenValue,
            schema: "mutsuki.host.boot-resource.blob.v1".into(),
            provider_id: provider_id.into(),
            operations: vec!["collect".into()],
            reload_policy: ResourceProviderReloadPolicy::NoLiveResources,
            compatibility: ResourceProviderCompatibility {
                schema_version: "1.0.0".into(),
                required_operations: vec!["collect".into()],
                preserves_resource_type_id: true,
                accepts_older_generations: false,
                lease_drain_required: false,
            },
        });
    if let Some(provider) = provider {
        builder = builder.resource_provider_gateway(provider_id, provider);
    }
    builder.build()
}

struct BootResourceProvider;

impl ResourcePlanGateway for BootResourceProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        Ok(plan.args["bytes"]
            .as_array()
            .map(|bytes| {
                bytes
                    .iter()
                    .filter_map(|byte| byte.as_u64().map(|byte| byte as u8))
                    .collect()
            })
            .unwrap_or_else(|| b"plugin".to_vec()))
    }

    fn snapshot_read_plan(
        &self,
        _plan: &ReadPlan,
        _kind_id: &str,
        _schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        Err(unused_provider_method("snapshot"))
    }

    fn open_stream_plan(&self, _plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unused_provider_method("stream"))
    }

    fn execute_export_plan(&self, _plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        Err(unused_provider_method("export"))
    }

    fn commit_write_plan(&self, _plan: &WritePlan, _bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        Err(unused_provider_method("write"))
    }

    fn execute_command_plan(&self, _plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        Err(unused_provider_method("command"))
    }

    fn execute_command_batch(&self, _batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unused_provider_method("batch"))
    }

    fn execute_saga_plan(&self, _saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        Err(unused_provider_method("saga"))
    }
}

impl ResourceProviderGateway for BootResourceProvider {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        Ok(boot_resource_ref(
            "mutsuki.host.boot-resource.blob",
            ResourceSemantic::FrozenValue,
            schema,
            bytes,
        ))
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        Ok(boot_resource_ref(
            kind_id,
            ResourceSemantic::CowVersionedState,
            schema,
            bytes,
        ))
    }

    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        Ok(boot_resource_ref(
            kind_id,
            ResourceSemantic::CapabilityResource,
            schema,
            Vec::new(),
        ))
    }
}

fn boot_resource_ref(
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    bytes: Vec<u8>,
) -> ResourceRef {
    ResourceRef {
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: "boot-resource".into(),
            generation: 1,
            version: 1,
        },
        ref_id: "boot-resource".into(),
        semantic,
        provider_id: "mutsuki.host.boot-resource".into(),
        resource_kind: kind_id.into(),
        schema: schema.into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: Some(bytes.len() as u64),
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn unused_provider_method(method: &str) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        "test.unused_provider_method",
        "runtime.host.test",
        method,
    ))
}
