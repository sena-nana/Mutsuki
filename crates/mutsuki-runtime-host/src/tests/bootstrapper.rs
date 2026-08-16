use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mutsuki_runtime_contracts::*;
use mutsuki_runtime_core::{CoreRuntime, Runner, RunnerManagementHandle, RuntimeResult};
use mutsuki_runtime_sdk::{
    BuiltinPluginLoader, HostEffect, HostEffectFuture, HostEffectKind, HostServiceRegistry,
    LoadedPlugin, PluginBuilder,
};
use serde_json::json;

use crate::{
    BinaryRunner, NativeRunner, RuntimeBootstrapper, runner_manifest, runner_manifest_with_artifact,
};

use super::helpers::{
    abi_plugin_fixture, descriptor, host_with_echo_runner, host_with_portable_plugin_artifact,
    runtime_profile, runtime_profile_with_deployment,
};

struct CountingEffect(Arc<AtomicUsize>);

impl HostEffect for CountingEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        let disposed = self.0.clone();
        Box::pin(async move {
            disposed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[derive(Debug)]
struct CountingManagement(Arc<AtomicUsize>);

impl RunnerManagementHandle for CountingManagement {
    fn cancel(&self, _invocation_id: &str) -> RuntimeResult<()> {
        Ok(())
    }

    fn dispose(&self) -> RuntimeResult<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ManagedDeploymentRunner {
    descriptor: RunnerDescriptor,
    management: Arc<CountingManagement>,
    direct_disposes: Arc<AtomicUsize>,
}

impl Runner for ManagedDeploymentRunner {
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

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.direct_disposes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn management_handle(&self) -> Option<Arc<dyn RunnerManagementHandle>> {
        Some(self.management.clone())
    }
}

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
fn prepared_host_runtime_exposes_the_exact_external_plan_used_for_start() {
    let prepared = host_with_echo_runner()
        .prepare_host_runtime_with_config(runtime_profile(), crate::HostRuntimeConfig::default())
        .unwrap();
    let plan = prepared.load_plan().clone();
    assert!(plan.plugins.iter().all(|plugin| plugin.plugin_id != "core"));

    let runtime = prepared.start().unwrap();
    assert_eq!(
        runtime.host_context().registry_generation(),
        plan.registry_generation
    );
    assert_eq!(runtime.host_context().profile_id(), plan.profile_id);
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
    let scoped_service = runtime
        .host_context()
        .plugin_scope("plugin-service")
        .unwrap()
        .require_service::<String>("service.echo")
        .unwrap();
    assert_eq!(scoped_service.as_str(), "ready");
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
fn rebindable_host_service_reaches_scope_composition_with_provider_binding() {
    let plugin = PluginBuilder::new("plugin-rebindable")
        .rebindable_host_service(
            "service.rebindable",
            Arc::new(String::from("ready")),
            "test.host.lifecycle",
        )
        .build();
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(plugin);

    let runtime = host
        .into_host_runtime(host_service_profile("plugin-rebindable"))
        .unwrap();
    let entries = runtime.host_context().services().owned_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "service.rebindable");
    assert!(entries[0].3);
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
    let staged_scopes = prepared.scopes.as_ref().unwrap();
    let staged_scope = staged_scopes.plugin_scope("plugin-service").unwrap();
    assert_eq!(
        staged_scopes
            .manager()
            .snapshot(staged_scope)
            .unwrap()
            .state,
        crate::ScopeState::Activating
    );
    assert_eq!(prepared.load_plan().registry_generation, 2);
    assert!(
        prepared
            .load_plan()
            .plugins
            .iter()
            .all(|plugin| plugin.plugin_id != "core")
    );

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
fn plugin_effect_is_disposed_by_its_scope_on_runtime_shutdown() {
    let disposed = Arc::new(AtomicUsize::new(0));
    let plugin = PluginBuilder::new("plugin-effect")
        .host_effect(
            HostEffectKind::HostLocal,
            Box::new(CountingEffect(disposed.clone())),
        )
        .build();
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(plugin);
    let runtime = host
        .into_host_runtime(host_service_profile("plugin-effect"))
        .unwrap();

    assert_eq!(disposed.load(Ordering::SeqCst), 0);
    drop(runtime);
    assert_eq!(disposed.load(Ordering::SeqCst), 1);
}

#[test]
fn runner_deployments_share_one_scope_owned_backend_lifecycle() {
    let deployments = [
        (ArtifactType::Native, PluginDeploymentKind::Builtin),
        (ArtifactType::Abi, PluginDeploymentKind::Abi),
        (ArtifactType::Process, PluginDeploymentKind::Process),
        (ArtifactType::Python, PluginDeploymentKind::Python),
    ];

    for (index, (artifact_type, deployment)) in deployments.into_iter().enumerate() {
        let plugin_id = format!("plugin-managed-{index}");
        let mut runner_descriptor = descriptor(
            &format!("managed.runner.{index}"),
            &format!("managed.work.{index}"),
        );
        runner_descriptor.plugin_id = plugin_id.as_str().into();
        let manifest = managed_deployment_manifest(
            &plugin_id,
            artifact_type,
            deployment.clone(),
            runner_descriptor.clone(),
        );
        let management_disposes = Arc::new(AtomicUsize::new(0));
        let direct_disposes = Arc::new(AtomicUsize::new(0));
        let runner = Box::new(ManagedDeploymentRunner {
            descriptor: runner_descriptor,
            management: Arc::new(CountingManagement(management_disposes.clone())),
            direct_disposes: direct_disposes.clone(),
        });
        let mut host = RuntimeBootstrapper::new();
        host.register_manifest(manifest);
        match deployment.clone() {
            PluginDeploymentKind::Builtin => host.register_builtin_runner(runner),
            PluginDeploymentKind::Abi => host.register_abi_runner(runner),
            deployment => host.register_external_runner(deployment, runner),
        }

        let runtime = host
            .into_host_runtime(runtime_profile_with_deployment(&plugin_id, deployment))
            .unwrap();
        assert_eq!(management_disposes.load(Ordering::SeqCst), 0);
        assert_eq!(direct_disposes.load(Ordering::SeqCst), 0);

        drop(runtime);

        assert_eq!(management_disposes.load(Ordering::SeqCst), 1);
        assert_eq!(direct_disposes.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn core_boot_failure_rolls_back_activated_plugin_effects() {
    let disposed = Arc::new(AtomicUsize::new(0));
    let runner = descriptor("duplicate.runner", "duplicate.work");
    let mut plugin = PluginBuilder::new("plugin-a")
        .host_effect(
            HostEffectKind::HostLocal,
            Box::new(CountingEffect(disposed.clone())),
        )
        .build();
    plugin.manifest = runner_manifest("plugin-a", vec![runner.clone()]);
    plugin.runners = vec![
        Box::new(NativeRunner::new(runner.clone(), |_ctx, tasks| {
            Ok(RunnerResult::completed(tasks.task_id))
        })),
        Box::new(NativeRunner::new(runner, |_ctx, tasks| {
            Ok(RunnerResult::completed(tasks.task_id))
        })),
    ];
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(plugin);

    let error = host.into_host_runtime(runtime_profile()).err().unwrap();
    assert_eq!(disposed.load(Ordering::SeqCst), 1, "{error}");
}

#[test]
fn targeted_reload_keeps_unaffected_scope_and_discards_unused_candidate() {
    fn plugin(id: &str, disposed: Arc<AtomicUsize>) -> LoadedPlugin {
        PluginBuilder::new(id)
            .host_effect(
                HostEffectKind::HostLocal,
                Box::new(CountingEffect(disposed)),
            )
            .build()
    }
    fn profile() -> RuntimeProfile {
        let mut profile = host_service_profile("plugin-a");
        profile.enabled_plugins.push("plugin-b".into());
        profile
    }

    let old_a = Arc::new(AtomicUsize::new(0));
    let old_b = Arc::new(AtomicUsize::new(0));
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(plugin("plugin-a", old_a.clone()));
    host.register_loaded_plugin(plugin("plugin-b", old_b.clone()));
    let mut runtime = host.into_host_runtime(profile()).unwrap();

    let new_a = Arc::new(AtomicUsize::new(0));
    let unused_b = Arc::new(AtomicUsize::new(0));
    let mut candidate = RuntimeBootstrapper::new();
    candidate.register_loaded_plugin(plugin("plugin-a", new_a.clone()));
    candidate.register_loaded_plugin(plugin("plugin-b", unused_b.clone()));
    let prepared = candidate
        .prepare_targeted_reload_with_runner_limits(
            profile(),
            2,
            Default::default(),
            ["plugin-a".to_string()].into_iter().collect(),
        )
        .unwrap();
    runtime.reload(prepared, Duration::from_secs(1)).unwrap();

    assert_eq!(old_a.load(Ordering::SeqCst), 1);
    assert_eq!(old_b.load(Ordering::SeqCst), 0);
    assert_eq!(new_a.load(Ordering::SeqCst), 0);
    assert_eq!(unused_b.load(Ordering::SeqCst), 1);
    assert!(runtime.scope_reached_state("plugin-a", crate::ScopeState::Draining));

    let newest_a = Arc::new(AtomicUsize::new(0));
    let discarded_b = Arc::new(AtomicUsize::new(0));
    let mut next_candidate = RuntimeBootstrapper::new();
    next_candidate.register_loaded_plugin(plugin("plugin-a", newest_a.clone()));
    next_candidate.register_loaded_plugin(plugin("plugin-b", discarded_b.clone()));
    let prepared = next_candidate
        .prepare_targeted_reload_with_runner_limits(
            profile(),
            3,
            Default::default(),
            ["plugin-a".to_string()].into_iter().collect(),
        )
        .unwrap();
    runtime.reload(prepared, Duration::from_secs(1)).unwrap();

    assert_eq!(runtime.scope_set_count(), 2);
    assert_eq!(new_a.load(Ordering::SeqCst), 1);
    assert_eq!(discarded_b.load(Ordering::SeqCst), 1);
    drop(runtime);
    assert_eq!(old_b.load(Ordering::SeqCst), 1);
    assert_eq!(newest_a.load(Ordering::SeqCst), 1);
}

#[test]
fn targeted_reload_disposes_only_dynamic_effects_in_the_reloaded_plugin_scope() {
    fn profile() -> RuntimeProfile {
        let mut profile = host_service_profile("plugin-a");
        profile.enabled_plugins.push("plugin-b".into());
        profile
    }

    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(PluginBuilder::new("plugin-a").build());
    host.register_loaded_plugin(PluginBuilder::new("plugin-b").build());
    let mut runtime = host.into_host_runtime(profile()).unwrap();
    let disposed_a = Arc::new(AtomicUsize::new(0));
    let disposed_b = Arc::new(AtomicUsize::new(0));
    runtime
        .attach_plugin_effect(
            "plugin-a",
            HostEffectKind::HostLocal,
            Box::new(CountingEffect(disposed_a.clone())),
        )
        .unwrap();
    runtime
        .attach_plugin_effect(
            "plugin-b",
            HostEffectKind::HostLocal,
            Box::new(CountingEffect(disposed_b.clone())),
        )
        .unwrap();

    let mut candidate = RuntimeBootstrapper::new();
    candidate.register_loaded_plugin(PluginBuilder::new("plugin-a").build());
    candidate.register_loaded_plugin(PluginBuilder::new("plugin-b").build());
    let prepared = candidate
        .prepare_targeted_reload_with_runner_limits(
            profile(),
            2,
            Default::default(),
            ["plugin-a".to_string()].into_iter().collect(),
        )
        .unwrap();
    runtime.reload(prepared, Duration::from_secs(1)).unwrap();

    assert_eq!(disposed_a.load(Ordering::SeqCst), 1);
    assert_eq!(disposed_b.load(Ordering::SeqCst), 0);
    drop(runtime);
    assert_eq!(disposed_b.load(Ordering::SeqCst), 1);
}

#[test]
fn repeated_clean_reloads_keep_scope_sets_bounded() {
    let disposed = Arc::new(AtomicUsize::new(0));
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(
        PluginBuilder::new("plugin-bounded")
            .host_effect(
                HostEffectKind::HostLocal,
                Box::new(CountingEffect(disposed.clone())),
            )
            .build(),
    );
    let mut runtime = host
        .into_host_runtime(host_service_profile("plugin-bounded"))
        .unwrap();

    for generation in 2..=10_001 {
        let mut candidate = RuntimeBootstrapper::new();
        candidate.register_loaded_plugin(
            PluginBuilder::new("plugin-bounded")
                .host_effect(
                    HostEffectKind::HostLocal,
                    Box::new(CountingEffect(disposed.clone())),
                )
                .build(),
        );
        let prepared = candidate
            .prepare_reload(host_service_profile("plugin-bounded"), generation)
            .unwrap();
        runtime.reload(prepared, Duration::from_secs(1)).unwrap();
        assert_eq!(runtime.scope_set_count(), 1);
    }

    assert_eq!(disposed.load(Ordering::SeqCst), 10_000);
    drop(runtime);
    assert_eq!(disposed.load(Ordering::SeqCst), 10_001);
}

#[test]
fn targeted_reload_expands_the_connected_service_domain() {
    fn consumer() -> LoadedPlugin {
        PluginBuilder::new("plugin-consumer")
            .requires(SurfaceRequirement::service("service.shared"))
            .build()
    }

    fn profile() -> RuntimeProfile {
        let mut profile = host_service_profile("plugin-provider");
        profile.enabled_plugins.push("plugin-consumer".into());
        profile
    }

    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(host_service_plugin(
        "plugin-provider",
        "service.shared",
        "provider-v1",
    ));
    host.register_loaded_plugin(consumer());
    let mut runtime = host.into_host_runtime(profile()).unwrap();

    let mut candidate = RuntimeBootstrapper::new();
    candidate.register_loaded_plugin(host_service_plugin(
        "plugin-provider",
        "service.shared",
        "provider-v2",
    ));
    candidate.register_loaded_plugin(consumer());
    let prepared = candidate
        .prepare_targeted_reload_with_runner_limits(
            profile(),
            2,
            Default::default(),
            ["plugin-consumer".to_string()].into_iter().collect(),
        )
        .unwrap();

    assert_eq!(
        prepared.affected_plugins.as_ref().unwrap(),
        &["plugin-consumer".into(), "plugin-provider".into()]
            .into_iter()
            .collect()
    );
    let scopes = prepared.scopes.as_ref().unwrap();
    let consumer_scope = scopes.plugin_scope("plugin-consumer").unwrap();
    let resolved = scopes
        .manager()
        .resolve_service::<String>(consumer_scope, crate::ServiceKey::new("service.shared"))
        .unwrap()
        .unwrap();
    assert_eq!(resolved.plugin_generation, 2);
    assert_eq!(resolved.service.as_str(), "provider-v2");

    runtime.reload(prepared, Duration::from_secs(1)).unwrap();
    let service = runtime
        .host_context()
        .plugin_scope("plugin-consumer")
        .unwrap()
        .require_service::<String>("service.shared")
        .unwrap();
    assert_eq!(service.as_str(), "provider-v2");
    assert_eq!(
        runtime
            .host_context()
            .plugin_scope("plugin-provider")
            .unwrap()
            .handle()
            .plugin_generation,
        2
    );
    assert_eq!(
        runtime
            .host_context()
            .plugin_scope("plugin-consumer")
            .unwrap()
            .handle()
            .plugin_generation,
        2
    );
}

#[test]
fn targeted_reload_rejects_a_domain_split_from_the_active_contract() {
    fn consumer(requires_service: bool) -> LoadedPlugin {
        let builder = PluginBuilder::new("plugin-consumer");
        if requires_service {
            builder
                .requires(SurfaceRequirement::service("service.shared"))
                .build()
        } else {
            builder.build()
        }
    }

    fn profile() -> RuntimeProfile {
        let mut profile = host_service_profile("plugin-provider");
        profile.enabled_plugins.push("plugin-consumer".into());
        profile
    }

    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(host_service_plugin(
        "plugin-provider",
        "service.shared",
        "provider-v1",
    ));
    host.register_loaded_plugin(consumer(true));
    let mut runtime = host.into_host_runtime(profile()).unwrap();

    let mut candidate = RuntimeBootstrapper::new();
    candidate.register_loaded_plugin(host_service_plugin(
        "plugin-provider",
        "service.shared",
        "provider-v2",
    ));
    candidate.register_loaded_plugin(consumer(false));
    let prepared = candidate
        .prepare_targeted_reload_with_runner_limits(
            profile(),
            2,
            Default::default(),
            ["plugin-provider".to_string()].into_iter().collect(),
        )
        .unwrap();

    let error = runtime
        .reload(prepared, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.error().route, "host.scope.reload_domain_incomplete");
    assert_eq!(runtime.host_context().registry_generation(), 1);
    let service = runtime
        .host_context()
        .plugin_scope("plugin-consumer")
        .unwrap()
        .require_service::<String>("service.shared")
        .unwrap();
    assert_eq!(service.as_str(), "provider-v1");
}

#[test]
fn targeted_reload_does_not_treat_host_services_as_plugin_domain_edges() {
    fn provider_contract() -> LoadedPlugin {
        let mut plugin = PluginBuilder::new("host-provider").build();
        plugin
            .manifest
            .provides
            .services
            .push("service.host".into());
        plugin
    }

    fn consumer() -> LoadedPlugin {
        PluginBuilder::new("plugin-consumer")
            .requires(SurfaceRequirement::service("service.host"))
            .build()
    }

    fn profile() -> RuntimeProfile {
        let mut profile = host_service_profile("host-provider");
        profile.enabled_plugins.push("plugin-consumer".into());
        profile
    }

    let services = Arc::new(HostServiceRegistry::new());
    services
        .register("service.host", Arc::new(String::from("host-value")))
        .unwrap();
    services.freeze();

    let mut host = RuntimeBootstrapper::new();
    host.use_shared_services(services.clone()).unwrap();
    host.register_loaded_plugin(provider_contract());
    host.register_loaded_plugin(consumer());
    let mut runtime = host.into_host_runtime(profile()).unwrap();

    let mut candidate = RuntimeBootstrapper::new();
    candidate.use_shared_services(services).unwrap();
    candidate.register_loaded_plugin(provider_contract());
    candidate.register_loaded_plugin(consumer());
    let prepared = candidate
        .prepare_targeted_reload_with_runner_limits(
            profile(),
            2,
            Default::default(),
            ["plugin-consumer".to_string()].into_iter().collect(),
        )
        .unwrap();

    runtime.reload(prepared, Duration::from_secs(1)).unwrap();
    let service = runtime
        .host_context()
        .plugin_scope("plugin-consumer")
        .unwrap()
        .require_service::<String>("service.host")
        .unwrap();
    assert_eq!(service.as_str(), "host-value");
}

#[test]
fn restart_required_scope_rejects_reload_before_generation_switch() {
    fn plugin(effect: Option<Box<dyn HostEffect>>) -> LoadedPlugin {
        let mut builder = PluginBuilder::new("plugin-restart").lifecycle(LifecyclePolicy {
            reload_policy: "restart_required".into(),
            unload_timeout_ms: 1_000,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: false,
        });
        if let Some(effect) = effect {
            builder = builder.host_effect(HostEffectKind::HostLocal, effect);
        }
        builder.build()
    }

    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(plugin(None));
    let mut runtime = host
        .into_host_runtime(host_service_profile("plugin-restart"))
        .unwrap();
    let candidate_disposed = Arc::new(AtomicUsize::new(0));
    let mut candidate = RuntimeBootstrapper::new();
    candidate.register_loaded_plugin(plugin(Some(Box::new(CountingEffect(
        candidate_disposed.clone(),
    )))));
    let prepared = candidate
        .prepare_reload(host_service_profile("plugin-restart"), 2)
        .unwrap();

    assert!(runtime.reload(prepared, Duration::from_secs(1)).is_err());
    assert_eq!(runtime.host_context().registry_generation(), 1);
    assert_eq!(candidate_disposed.load(Ordering::SeqCst), 1);
}

#[test]
fn post_switch_cleanup_failure_retains_dirty_old_scope() {
    struct FailingEffect(Arc<AtomicUsize>);

    impl Drop for FailingEffect {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl HostEffect for FailingEffect {
        fn dispose(&mut self) -> HostEffectFuture<'_> {
            Box::pin(async { Err(crate::error::host_failure("test.scope.cleanup", "injected")) })
        }
    }

    let dropped = Arc::new(AtomicUsize::new(0));
    let mut host = RuntimeBootstrapper::new();
    host.register_loaded_plugin(
        PluginBuilder::new("plugin-dirty")
            .host_effect(
                HostEffectKind::BackendInstance,
                Box::new(FailingEffect(dropped.clone())),
            )
            .build(),
    );
    let mut runtime = host
        .into_host_runtime(host_service_profile("plugin-dirty"))
        .unwrap();

    let mut candidate = RuntimeBootstrapper::new();
    candidate.register_loaded_plugin(PluginBuilder::new("plugin-dirty").build());
    let prepared = candidate
        .prepare_reload(host_service_profile("plugin-dirty"), 2)
        .unwrap();

    runtime.reload(prepared, Duration::from_secs(1)).unwrap();
    assert!(
        runtime
            .take_reload_lifecycle_errors()
            .iter()
            .any(|error| { error.starts_with("host.reload.post_switch_scope_cleanup_failed:") })
    );
    assert_eq!(runtime.host_context().registry_generation(), 2);
    assert_eq!(runtime.retained_dirty_scope_sets(), 1);
    assert!(runtime.scope_reached_state("plugin-dirty", crate::ScopeState::Draining));
    assert!(runtime.scope_reached_state("plugin-dirty", crate::ScopeState::FailedDirty));
    drop(runtime);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
}

#[test]
fn abi_plugin_boots_through_registered_abi_runner_bridge() {
    let (manifest, runner_descriptor) = abi_plugin_fixture();
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(manifest);
    host.register_abi_runner(Box::new(BinaryRunner::new(
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
        runners: vec![Box::new(BinaryRunner::new(
            runner_descriptor,
            Cursor::new(Vec::<u8>::new()),
            Cursor::new(Vec::<u8>::new()),
        ))],
        async_handlers: Vec::new(),
        host_services: Vec::new(),
        resource_providers: Vec::new(),
        async_resource_providers: Vec::new(),
        host_effects: Vec::new(),
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
            companion_artifacts: Vec::new(),
        },
        vec![runner_descriptor.clone()],
    );
    let reader = Cursor::new(Vec::<u8>::new());
    let writer = Cursor::new(Vec::<u8>::new());
    let mut host = RuntimeBootstrapper::new();
    host.register_manifest(manifest);
    host.register_abi_runner(Box::new(BinaryRunner::new(
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
    host.register_abi_runner(Box::new(BinaryRunner::new(
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
    host.register_abi_runner(Box::new(BinaryRunner::new(
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
        surface_bindings: Default::default(),
        supported_extensions: Vec::new(),
        plugin_deployments: Default::default(),
        observability: ObservabilityProfile::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}

fn managed_deployment_manifest(
    plugin_id: &str,
    artifact_type: ArtifactType,
    deployment: PluginDeploymentKind,
    runner: RunnerDescriptor,
) -> PluginManifest {
    let mut manifest = runner_manifest_with_artifact(
        plugin_id,
        PluginArtifact {
            artifact_type,
            path: format!("fixture-{plugin_id}"),
            sha256: format!("sha256:{plugin_id}"),
            companion_artifacts: Vec::new(),
        },
        vec![runner],
    );
    if deployment != PluginDeploymentKind::Builtin {
        manifest
            .provides
            .host_extensions
            .push(HostExtensionDescriptor {
                extension_id: format!("host.extension.{plugin_id}.backend"),
                kind: HostExtensionKind::PluginBackend,
                supported_deployments: vec![deployment.clone()],
                reload_policy: "drain_and_swap".into(),
                drain_required: true,
            });
        manifest
            .provides
            .plugin_backends
            .push(PluginBackendDescriptor {
                backend_id: format!("plugin.backend.{plugin_id}"),
                deployment_kind: deployment,
                task_client_protocol: "mutsuki.task.v1".into(),
                resource_client_protocol: "mutsuki.resource-plan.v1".into(),
                codec_id: None,
                bridge_id: None,
            });
    }
    manifest
}

fn host_service_plugin(
    plugin_id: &str,
    service_id: &str,
    value: &str,
) -> mutsuki_runtime_sdk::LoadedPlugin {
    host_service_builder(plugin_id, service_id, value).build()
}

fn host_service_builder(plugin_id: &str, service_id: &str, value: &str) -> PluginBuilder {
    PluginBuilder::new(plugin_id).host_service(
        service_id,
        Arc::new(value.to_string()),
        "test.host.lifecycle",
    )
}
