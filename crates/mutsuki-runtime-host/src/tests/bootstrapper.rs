use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::CoreRuntime;
use mutsuki_runtime_sdk::{BuiltinPluginLoader, LoadedPlugin, PluginBuilder};
use serde_json::json;

use crate::{JsonlRunner, NativeRunner, RuntimeBootstrapper, runner_manifest_with_artifact};

use super::helpers::{
    abi_plugin_fixture, descriptor, host_with_echo_runner, host_with_portable_plugin_artifact,
    runtime_profile, runtime_profile_with_deployment,
};

#[test]
fn runtime_bootstrapper_boots_runtime_and_runs_runner_loop() {
    let mut runtime: CoreRuntime = host_with_echo_runner()
        .into_runtime(runtime_profile())
        .unwrap();

    runtime
        .submit_task(Task::new("task-1", "raw.input", json!({"ok": true})))
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
    let runtime = host_with_echo_runner()
        .into_host_runtime(runtime_profile())
        .unwrap();

    let submitted = runtime
        .dispatch(crate::HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "task-1",
            "raw.input",
            json!({"ok": true}),
        ))))
        .unwrap();
    let crate::HostRuntimeReply::TaskSubmitted(handle) = submitted else {
        panic!("expected task submitted reply");
    };
    assert_eq!(handle.task_id, "task-1");

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
fn same_plugin_artifact_runs_in_local_and_worker_adapter_hosts() {
    fn run_through_ordinary_host(task_id: &str) -> TaskStatus {
        let mut profile = runtime_profile_with_deployment(
            "plugin-portable-fixture",
            PluginDeploymentKind::Builtin,
        );
        profile.profile_id = format!("ordinary-host-{task_id}");
        let runtime = host_with_portable_plugin_artifact()
            .into_host_runtime(profile)
            .unwrap();

        runtime
            .dispatch(crate::HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                task_id,
                "portable.echo",
                json!({"input": "same-artifact"}),
            ))))
            .unwrap();
        runtime
            .dispatch(crate::HostRuntimeCommand::RunUntilIdle { max_ticks: 4 })
            .unwrap();
        runtime.task_status(task_id).unwrap()
    }

    let local_status = run_through_ordinary_host("local-task");
    let worker_adapter_status = run_through_ordinary_host("worker-adapter-task");

    assert_eq!(local_status, TaskStatus::Completed);
    assert_eq!(worker_adapter_status, TaskStatus::Completed);
}

#[test]
fn loaded_plugin_host_service_is_reachable_from_host_context_after_boot() {
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(host_service_plugin(
        "plugin-service",
        "service.echo",
        "ready",
    ));

    let runtime = host
        .into_host_runtime(host_service_profile("plugin-service"))
        .unwrap();

    let service = runtime
        .host_context()
        .services()
        .require::<String>("service.echo")
        .unwrap();
    assert_eq!(service.as_str(), "ready");
    assert!(runtime.host_context().services().is_frozen());
}

#[test]
fn plugin_loader_registers_sdk_built_plugin_services_for_host_boot() {
    let mut loader = BuiltinPluginLoader::new().with_plugin(Box::new(host_service_builder(
        "plugin-loader-service",
        "service.loader",
        "loaded",
    )));
    let mut host = RuntimeBootstrapper::new();
    host.load_plugins(&mut loader).unwrap();

    let runtime = host
        .into_host_runtime(host_service_profile("plugin-loader-service"))
        .unwrap();

    let service = runtime
        .host_context()
        .services()
        .require::<String>("service.loader")
        .unwrap();
    assert_eq!(service.as_str(), "loaded");
}

#[test]
fn host_runtime_reload_preserves_prepared_plugin_services_in_host_context() {
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(host_service_plugin(
        "plugin-service",
        "service.echo",
        "ready-v1",
    ));
    let mut runtime = host
        .into_host_runtime(host_service_profile("plugin-service"))
        .unwrap();

    let mut reload_host = RuntimeBootstrapper::new();
    reload_host.register_loaded_plugin(host_service_plugin(
        "plugin-service",
        "service.echo",
        "ready-v2",
    ));
    let prepared = reload_host
        .prepare_reload(host_service_profile("plugin-service"), 2)
        .unwrap();

    runtime.reload(prepared, Duration::from_secs(1)).unwrap();

    let service = runtime
        .host_context()
        .services()
        .require::<String>("service.echo")
        .unwrap();
    assert_eq!(service.as_str(), "ready-v2");
    assert_eq!(runtime.host_context().registry_generation(), 2);
    assert!(runtime.host_context().services().is_frozen());
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
fn loaded_abi_plugin_keeps_abi_runner_deployment() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(LoadedPlugin {
        manifest,
        runners: vec![Box::new(JsonlRunner::new(
            runner_descriptor,
            Cursor::new(Vec::<u8>::new()),
            Cursor::new(Vec::<u8>::new()),
        ))],
        async_handlers: Vec::new(),
        host_services: Vec::new(),
        resource_providers: Vec::new(),
        async_resource_providers: Vec::new(),
    });

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
        |_ctx, tasks| Ok(RunnerResult::completed(tasks.task_id)),
    )));

    let mismatched = mismatched_host.into_runtime(profile).err().unwrap();

    assert_eq!(mismatched.error().code, ERR_REGISTRY_UNAUTHORIZED);
}

#[test]
fn active_plugin_backend_requires_matching_bridge_deployment() {
    let (mut manifest, runner_descriptor) = abi_plugin_fixture();
    manifest.provides.bridges[0].deployment_kind = PluginDeploymentKind::Builtin;
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
        .expect("backend bridge deployment mismatch should fail");

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("capability"),
        Some(&ScalarValue::String(
            "plugin_backend:plugin.backend.plugin-abi.abi".into()
        ))
    );
}

#[test]
fn active_plugin_backend_requires_bridge_to_support_configured_codec() {
    let (mut manifest, runner_descriptor) = abi_plugin_fixture();
    manifest.provides.bridges[0].codec_ids.clear();
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
        .expect("backend codec not supported by bridge should fail");

    assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
    assert_eq!(
        error.error().evidence.get("capability"),
        Some(&ScalarValue::String(
            "plugin_backend:plugin.backend.plugin-abi.abi".into()
        ))
    );
}

fn host_service_profile(plugin_id: &str) -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "host-service".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![plugin_id.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        observability: ObservabilityProfile::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}

fn host_service_plugin(
    plugin_id: &str,
    service_id: &str,
    value: &str,
) -> mutsuki_runtime_sdk::LoadedPlugin {
    host_service_builder(plugin_id, service_id, value).build()
}

fn host_service_builder(plugin_id: &str, service_id: &str, value: &str) -> PluginBuilder {
    PluginBuilder::new(plugin_id).host_service(service_id, Arc::new(value.to_string()), None)
}
