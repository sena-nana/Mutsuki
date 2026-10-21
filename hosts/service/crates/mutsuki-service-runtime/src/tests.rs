use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use mutsuki_runtime_contracts::{
    ArtifactType, CompletionBatch, ExecutionClass, LifecyclePolicy, PermissionGrant,
    PluginArtifact, PluginManifest, PluginProvides, RunnerBatchCapability, RunnerControlCapability,
    RunnerDescriptor, RunnerOrderingCapability, RunnerPayloadCapability, RunnerPurity,
    RunnerResourceCapability, Task, WorkBatch,
};
use mutsuki_runtime_sdk::map_work_batch_entries;
use mutsuki_service_control::{
    ControlErrorCode, HostMetrics, PluginReloadResponse, TaskOutcomeView, TaskSnapshot,
};
use serde_json::json;
use tempfile::tempdir;

use super::*;

#[test]
fn token_comparison_rejects_prefixes_and_length_variants() {
    assert!(tokens_match("local-abc", "local-abc"));
    assert!(!tokens_match("local-ab", "local-abc"));
    assert!(!tokens_match("local-abcd", "local-abc"));
    assert!(!tokens_match("", "local-abc"));
    assert!(!tokens_match("local-abc", ""));
    assert!(tokens_match("", ""));
}

#[tokio::test]
async fn control_change_hub_publishes_typed_invalidations() {
    let hub = ControlChangeHub::default();
    let mut changes = hub.subscribe();
    hub.publish([
        ControlChangeDomain::Tasks,
        ControlChangeDomain::Tasks,
        ControlChangeDomain::Runners,
    ]);
    let event = changes.changed().await.expect("control change");
    assert_eq!(event.revision, 1);
    assert_eq!(
        event.domains,
        vec![ControlChangeDomain::Tasks, ControlChangeDomain::Runners]
    );
}

macro_rules! control_result {
    ($response:expr, $variant:path) => {{
        match $response {
            ControlResponse::Ok($variant(value)) => value,
            other => panic!("unexpected control response: {other:?}"),
        }
    }};
}

#[test]
fn dynamic_runner_limits_recompute_from_the_boot_configuration() {
    let settings = Arc::new(Mutex::new((1_usize, 20_u64)));
    let factory_settings = settings.clone();
    let factories: Vec<RunnerLimitFactory> = vec![Arc::new(move || {
        let (max_running, timeout_ms) = *factory_settings.lock().unwrap();
        RunnerLimitOverride {
            runner_id: "test.dynamic.runner".into(),
            max_running: Some(max_running),
            wall_clock_timeout_ms: Some(timeout_ms),
        }
    })];
    let configured = BTreeMap::from([(
        "test.dynamic.runner".into(),
        RunnerLimits {
            max_running: 8,
            ..RunnerLimits::default()
        },
    )]);

    let initial = resolve_runner_limits(&configured, &factories);
    assert_eq!(initial["test.dynamic.runner"].max_running, 1);
    assert_eq!(
        initial["test.dynamic.runner"].wall_clock_deadline,
        Some(Duration::from_millis(20))
    );

    *settings.lock().unwrap() = (4, 250);
    let updated = resolve_runner_limits(&configured, &factories);
    assert_eq!(updated["test.dynamic.runner"].max_running, 4);
    assert_eq!(
        updated["test.dynamic.runner"].wall_clock_deadline,
        Some(Duration::from_millis(250))
    );
}

#[test]
fn service_execution_domains_map_to_core_topology_without_hidden_fallbacks() {
    let configured = mutsuki_service_config::ExecutionDomainSection {
        id: "interactive".into(),
        execution_classes: vec![ExecutionClassName::Orchestration, ExecutionClassName::Cpu],
        threads: 2,
        lanes: BTreeMap::from([(
            DispatchLaneName::Interactive,
            LanePolicySection {
                weight: Some(16),
                reserved_entries: Some(4),
                max_share_percent: Some(80),
                allow_idle_borrow: Some(false),
                ..LanePolicySection::default()
            },
        )]),
        ..mutsuki_service_config::ExecutionDomainSection::default()
    };

    let mapped = map_execution_domain(&configured);
    assert_eq!(mapped.domain_id, "interactive");
    assert_eq!(
        mapped.execution_classes,
        vec![ExecutionClass::Orchestration, ExecutionClass::Cpu,]
    );
    assert_eq!(mapped.threads, 2);
    let policy = &mapped.lane_policies[&DispatchLane::Interactive];
    assert_eq!(policy.weight, 16);
    assert_eq!(policy.reserved_entries, 4);
    assert_eq!(policy.max_share_percent, 80);
    assert!(!policy.allow_idle_borrow);
}

const TEST_PLUGIN_ID: &str = "test.control.facade";

fn test_builtin_registry() -> BuiltinRegistry {
    let mut registry = BuiltinRegistry::new();
    registry.register_manifest(minimal_manifest(TEST_PLUGIN_ID));
    registry
}

struct TestConfiguredFactory;

struct SecondConfiguredFactory;

#[derive(Debug)]
struct GenerationService(u64);

#[derive(Default)]
struct HookLifecycle {
    activations: AtomicUsize,
    deactivations: AtomicUsize,
}

struct TrackingLoadPlanHook(Arc<HookLifecycle>);

type HookLifecycles = Arc<Mutex<BTreeMap<u64, Arc<HookLifecycle>>>>;

impl LoadPlanLifecycleHook for TrackingLoadPlanHook {
    fn validate(&self, _plan: &RuntimeLoadPlan) -> Result<(), String> {
        Ok(())
    }

    fn activate(&self, _plan: &RuntimeLoadPlan) {
        self.0.activations.fetch_add(1, Ordering::SeqCst);
    }

    fn deactivate(&self) -> Result<(), String> {
        self.0.deactivations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct GenerationConfiguredFactory {
    plugin_id: &'static str,
    service_id: &'static str,
    hook_lifecycles: Option<HookLifecycles>,
}

struct CleanupDiagnosticConfiguredFactory {
    attempts: Arc<AtomicUsize>,
}

struct RetryCleanupEffect {
    attempts: Arc<AtomicUsize>,
    fail_once: bool,
    attempted: bool,
}

impl HostEffect for RetryCleanupEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let fail = self.fail_once && !self.attempted;
        self.attempted = true;
        Box::pin(async move {
            if fail {
                Err(RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_HOST_FAILED,
                    "test.cleanup_diagnostic",
                    "injected cleanup failure",
                )))
            } else {
                Ok(())
            }
        })
    }
}

impl ConfiguredPluginFactory for CleanupDiagnosticConfiguredFactory {
    fn plugin_id(&self) -> &str {
        "test.cleanup-diagnostic"
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let generation = config
            .get("generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| "generation is required".to_string())?;
        let manifest = minimal_manifest(self.plugin_id());
        let loaded_manifest = manifest.clone();
        let attempts = self.attempts.clone();
        Ok(
            builder.register_builtin_loaded_plugin_factory(manifest, move || {
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services: Vec::new(),
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: vec![mutsuki_runtime_sdk::RuntimeBootstrapperEffect {
                        kind: HostEffectKind::HostLocal,
                        effect: Box::new(RetryCleanupEffect {
                            attempts: attempts.clone(),
                            fail_once: generation == 1,
                            attempted: false,
                        }),
                    }],
                })
            }),
        )
    }
}

impl ConfiguredPluginFactory for GenerationConfiguredFactory {
    fn plugin_id(&self) -> &str {
        self.plugin_id
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let generation = config
            .get("generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| "generation is required".to_string())?;
        if config.get("fail").and_then(Value::as_bool) == Some(true) {
            return Err("candidate rejected".into());
        }
        let mut manifest = minimal_manifest(self.plugin_id);
        let service_id = self.service_id.to_string();
        let service_capability = format!("test.generation.service:{}", self.plugin_id);
        manifest.provides.services.push(service_id.clone());
        manifest
            .provides
            .capabilities
            .push(service_capability.clone());
        let loaded_manifest = manifest.clone();
        let service = Arc::new(GenerationService(generation));
        let builder = builder.register_builtin_loaded_plugin_factory(manifest, move || {
            Ok::<LoadedPlugin, String>(LoadedPlugin {
                manifest: loaded_manifest.clone(),
                runners: Vec::new(),
                async_handlers: Vec::new(),
                host_services: vec![RuntimeBootstrapperService::new(
                    service_id.clone(),
                    service.clone(),
                    service_capability.clone(),
                )],
                resource_providers: Vec::new(),
                async_resource_providers: Vec::new(),
                host_effects: Vec::new(),
            })
        });
        if let Some(lifecycles) = &self.hook_lifecycles {
            let lifecycle = Arc::new(HookLifecycle::default());
            lifecycles
                .lock()
                .expect("hook lifecycle mutex")
                .insert(generation, lifecycle.clone());
            Ok(builder
                .register_load_plan_hook(self.plugin_id, Arc::new(TrackingLoadPlanHook(lifecycle))))
        } else {
            Ok(builder)
        }
    }
}

struct DelayedAsyncHandler {
    descriptor: RunnerDescriptor,
}

impl AsyncBatchHandler for DelayedAsyncHandler {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &self,
        _ctx: mutsuki_runtime_contracts::RunnerContext,
        batch: WorkBatch,
    ) -> mutsuki_runtime_core::AsyncCompletionFuture {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            map_work_batch_entries(&batch, |task| {
                Ok(mutsuki_runtime_contracts::RunnerResult::completed(
                    task.task_id.clone(),
                ))
            })
        })
    }
}

impl ConfiguredPluginFactory for TestConfiguredFactory {
    fn plugin_id(&self) -> &str {
        "test.configured"
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        if config.get("mode").and_then(Value::as_str) != Some("enabled") {
            return Err("mode must be enabled".into());
        }
        Ok(builder.register_builtin_plugin(minimal_manifest(self.plugin_id())))
    }
}

impl ConfiguredPluginFactory for SecondConfiguredFactory {
    fn plugin_id(&self) -> &str {
        "test.configured.second"
    }

    fn prepare(
        &self,
        _config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        Ok(builder.register_builtin_plugin(minimal_manifest(self.plugin_id())))
    }
}

fn configured_selection(id: &str, config: Value) -> ConfiguredPluginSelection {
    ConfiguredPluginSelection {
        id: id.into(),
        enabled: true,
        config,
    }
}

#[test]
fn configured_plugin_catalog_installs_before_boot_and_keeps_secret_references() {
    let mut config = ServiceConfig::default();
    config.plugins.configured = vec![configured_selection(
        "test.configured",
        json!({"mode": "enabled", "client_secret_key": "TEST_SECRET"}),
    )];
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(TestConfiguredFactory).unwrap();

    let builder = ServiceRuntimeBuilder::new(config)
        .with_configured_plugin_catalog(catalog)
        .install_configured_plugins()
        .unwrap();

    assert_eq!(
        builder
            .components
            .builtin_registry()
            .load_all()
            .unwrap()
            .records
            .len(),
        1
    );
}

#[tokio::test]
async fn targeted_reconfigure_replaces_only_the_selected_owner_component() {
    let root = tempdir().expect("temp dir");
    let config_dir = root.path().join("config");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    let config_path = config_dir.join("local.toml");
    std::fs::write(
        &config_path,
        r#"
[[plugins.configured]]
id = "test.target"
enabled = true
[plugins.configured.config]
generation = 1

[[plugins.configured]]
id = "test.stable"
enabled = true
[plugins.configured.config]
generation = 7
"#,
    )
    .expect("config file");
    let mut config = ServiceConfig::load(mutsuki_service_config::ConfigOverrides {
        config_file: Some(config_path),
        home_dir: Some(root.path().to_path_buf()),
        ..Default::default()
    })
    .expect("loaded config");
    config.ipc.enabled = false;
    config.observe.console = false;
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = root.path().join("disabled");

    let target_hook_lifecycles = Arc::new(Mutex::new(BTreeMap::new()));
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog
        .register(GenerationConfiguredFactory {
            plugin_id: "test.target",
            service_id: "service.target",
            hook_lifecycles: Some(target_hook_lifecycles.clone()),
        })
        .unwrap();
    catalog
        .register(GenerationConfiguredFactory {
            plugin_id: "test.stable",
            service_id: "service.stable",
            hook_lifecycles: None,
        })
        .unwrap();
    let runtime = ServiceRuntimeBuilder::new(config.clone())
        .with_configured_plugin_catalog(catalog)
        .start()
        .await
        .expect("runtime starts");
    let target_before = runtime
        .host_service::<GenerationService>("service.target")
        .unwrap();
    let stable_before = runtime
        .host_service::<GenerationService>("service.stable")
        .unwrap();
    let target_hook_v1 = target_hook_lifecycles.lock().unwrap()[&1].clone();
    assert_eq!(target_hook_v1.activations.load(Ordering::SeqCst), 1);

    let report = runtime
        .reconfigure_plugins(&[ConfiguredPluginSelection {
            id: "test.target".into(),
            enabled: true,
            config: json!({"generation": 2}),
        }])
        .await
        .expect("target reconfigured");

    let target_after = runtime
        .host_service::<GenerationService>("service.target")
        .unwrap();
    let stable_after = runtime
        .host_service::<GenerationService>("service.stable")
        .unwrap();
    assert_eq!(target_before.0, 1);
    assert_eq!(target_after.0, 2);
    assert!(!Arc::ptr_eq(&target_before, &target_after));
    assert_eq!(stable_after.0, 7);
    assert!(Arc::ptr_eq(&stable_before, &stable_after));
    let target_hook_v2 = target_hook_lifecycles.lock().unwrap()[&2].clone();
    assert_eq!(target_hook_v1.deactivations.load(Ordering::SeqCst), 1);
    assert_eq!(target_hook_v2.activations.load(Ordering::SeqCst), 1);
    assert_eq!(
        report.components,
        vec![PluginReconfigureGeneration {
            plugin_id: "test.target".into(),
            previous_generation: 1,
            generation: 2,
            rolled_back: false,
        }]
    );

    // A later full reload must bootstrap from the reconfigured selections,
    // not the boot-time config snapshot (ABI plugins are re-initialized
    // from `configured_plugin_config`).
    assert_eq!(
        configured_plugin_config(&runtime.inner.current_config(), "test.target"),
        json!({"generation": 2})
    );
    assert!(runtime.inner.plugin_reload().await.is_ok());
    assert_eq!(
        runtime
            .host_service::<GenerationService>("service.target")
            .unwrap()
            .0,
        2
    );

    assert!(
        runtime
            .reconfigure_plugins(&[ConfiguredPluginSelection {
                id: "test.target".into(),
                enabled: true,
                config: json!({"generation": 3, "fail": true}),
            }])
            .await
            .is_err()
    );
    let target_after_rejection = runtime
        .host_service::<GenerationService>("service.target")
        .unwrap();
    assert!(Arc::ptr_eq(&target_after, &target_after_rejection));
    assert_eq!(
        runtime
            .inner
            .component_generations
            .lock()
            .unwrap()
            .get("test.target"),
        Some(&2)
    );
    runtime.shutdown().await;
    assert_eq!(target_hook_v1.deactivations.load(Ordering::SeqCst), 1);
    // The full reload above retires and re-activates every owned hook once.
    assert_eq!(target_hook_v2.activations.load(Ordering::SeqCst), 2);
    assert_eq!(target_hook_v2.deactivations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn targeted_reconfigure_reports_post_switch_scope_cleanup_failure() {
    let root = tempdir().expect("temp dir");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = root.path().to_path_buf();
    config.service.data_dir = root.path().join("data");
    config.service.log_dir = root.path().join("logs");
    config.service.run_dir = root.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = root.path().join("disabled");
    config.plugins.configured = vec![configured_selection(
        "test.cleanup-diagnostic",
        json!({"generation": 1}),
    )];

    let attempts = Arc::new(AtomicUsize::new(0));
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog
        .register(CleanupDiagnosticConfiguredFactory {
            attempts: attempts.clone(),
        })
        .unwrap();
    let runtime = ServiceRuntimeBuilder::new(config)
        .with_configured_plugin_catalog(catalog)
        .start()
        .await
        .expect("runtime starts");

    let report = runtime
        .reconfigure_plugins(&[ConfiguredPluginSelection {
            id: "test.cleanup-diagnostic".into(),
            enabled: true,
            config: json!({"generation": 2}),
        }])
        .await
        .expect("new generation remains authoritative");

    assert_eq!(report.registry_generation, 2);
    assert!(report.runner_errors.iter().any(|error| {
        error.contains("host.reload.post_switch_scope_cleanup_failed")
            && error.contains("injected cleanup failure")
    }));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    runtime.shutdown().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

type ShutdownTrace = Arc<Mutex<Vec<&'static str>>>;

struct TracingLoadPlanHook(ShutdownTrace);

impl LoadPlanLifecycleHook for TracingLoadPlanHook {
    fn validate(&self, _plan: &RuntimeLoadPlan) -> Result<(), String> {
        Ok(())
    }

    fn activate(&self, _plan: &RuntimeLoadPlan) {}

    fn deactivate(&self) -> Result<(), String> {
        self.0.lock().expect("trace mutex").push("core.deactivate");
        Ok(())
    }
}

struct TracingEventSource {
    descriptor: HostEventSourceDescriptor,
    trace: ShutdownTrace,
}

impl HostEventSource for TracingEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        Box::pin(async move {
            let mut shutdown = ctx.shutdown;
            shutdown.cancelled().await;
            Ok(())
        })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        self.trace
            .lock()
            .expect("trace mutex")
            .push("event_source.shutdown");
        Box::pin(async { Ok(()) })
    }

    fn health(&self) -> HostEventSourceHealth {
        HostEventSourceHealth::Healthy
    }
}

/// Core must outlive everything that holds a handle into it. An EventSource that is still
/// running when Core goes away submits into a dead runtime, and in-flight work is abandoned
/// rather than drained.
#[tokio::test]
async fn shutdown_stops_ingress_before_core_is_released() {
    let root = tempdir().expect("temp dir");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = root.path().to_path_buf();
    config.service.data_dir = root.path().join("data");
    config.service.log_dir = root.path().join("logs");
    config.service.run_dir = root.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = root.path().join("disabled");
    config.runners.graceful_shutdown_ms = 250;

    let trace: ShutdownTrace = Arc::new(Mutex::new(Vec::new()));
    let runtime = ServiceRuntimeBuilder::new(config)
        .register_builtin_plugin(minimal_manifest("test.shutdown-order"))
        .register_load_plan_hook(
            "test.shutdown-order",
            Arc::new(TracingLoadPlanHook(trace.clone())),
        )
        .register_event_source(Box::new(TracingEventSource {
            descriptor: HostEventSourceDescriptor::new("trace-source", "test.source"),
            trace: trace.clone(),
        }))
        .start()
        .await
        .expect("runtime starts");

    runtime.shutdown().await;

    assert_eq!(
        trace.lock().expect("trace mutex").as_slice(),
        &["event_source.shutdown", "core.deactivate"]
    );
}

#[tokio::test]
async fn full_reload_retires_and_reowns_load_plan_hooks() {
    let root = tempdir().expect("temp dir");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = root.path().to_path_buf();
    config.service.data_dir = root.path().join("data");
    config.service.log_dir = root.path().join("logs");
    config.service.run_dir = root.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = root.path().join("disabled");

    let lifecycle = Arc::new(HookLifecycle::default());
    let runtime = ServiceRuntimeBuilder::new(config)
        .register_builtin_plugin(minimal_manifest("test.load-plan-hook"))
        .register_load_plan_hook(
            "test.load-plan-hook",
            Arc::new(TrackingLoadPlanHook(lifecycle.clone())),
        )
        .start()
        .await
        .expect("runtime starts");
    assert_eq!(lifecycle.activations.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.deactivations.load(Ordering::SeqCst), 0);

    let response = runtime.inner.plugin_reload().await;
    let response: PluginReloadResponse = control_result!(response, ControlResult::PluginReload);
    assert!(response.runner_errors.is_empty());
    assert_eq!(lifecycle.activations.load(Ordering::SeqCst), 2);
    assert_eq!(lifecycle.deactivations.load(Ordering::SeqCst), 1);

    runtime.shutdown().await;
    assert_eq!(lifecycle.deactivations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn full_reload_waits_for_the_active_reload_transaction() {
    let root = tempdir().expect("temp dir");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = root.path().to_path_buf();
    config.service.data_dir = root.path().join("data");
    config.service.log_dir = root.path().join("logs");
    config.service.run_dir = root.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = root.path().join("disabled");

    let runtime = ServiceRuntimeBuilder::new(config)
        .register_builtin_plugin(minimal_manifest("test.serial-reload"))
        .start()
        .await
        .expect("runtime starts");
    let active_reload = runtime.inner.reload_lock.lock().await;
    let inner = runtime.inner.clone();
    let pending = tokio::spawn(async move { inner.plugin_reload().await });
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());

    drop(active_reload);
    let response = pending.await.expect("reload task joins");
    let response: PluginReloadResponse = control_result!(response, ControlResult::PluginReload);
    assert_eq!(response.previous_generation, 1);
    assert_eq!(response.registry_generation, 2);

    runtime.shutdown().await;
}

#[tokio::test]
async fn service_runtime_injects_executor_for_registered_async_handler() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = dir.path().to_path_buf();
    config.service.data_dir = dir.path().join("data");
    config.service.log_dir = dir.path().join("logs");
    config.service.run_dir = dir.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = dir.path().join("disabled");
    config.plugins.configured = vec![configured_selection("test.async", Value::Null)];

    let mut descriptor = chain_descriptor("test.async", "test.async.runner", "test.async.run");
    descriptor.execution_class = ExecutionClass::Io;
    descriptor.invocation_mode = mutsuki_runtime_contracts::InvocationMode::AsyncReentrant;
    descriptor.concurrency = mutsuki_runtime_contracts::RunnerConcurrency::Reentrant {
        max_inflight_batches: 4,
        max_inflight_entries: 4,
    };
    descriptor.batch.preferred_batch_size = 1;
    descriptor.batch.max_batch_entries = 1;
    descriptor.batch.max_entry_concurrency = 1;
    descriptor.batch.max_inflight_batches = 4;
    let factory_descriptor = descriptor.clone();
    let runtime = ServiceRuntimeBuilder::new(config)
        .register_builtin_plugin(runner_manifest("test.async", descriptor))
        .register_builtin_async_handler(move || {
            Arc::new(DelayedAsyncHandler {
                descriptor: factory_descriptor.clone(),
            })
        })
        .start()
        .await
        .expect("async runtime starts");

    runtime
        .inner
        .host_runtime
        .lock()
        .expect("runtime mutex")
        .as_ref()
        .expect("runtime")
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "async-task",
            "test.async.run",
            json!({}),
        ))))
        .expect("task submitted");

    for _ in 0..100 {
        let completed = runtime
            .inner
            .host_runtime
            .lock()
            .expect("runtime mutex")
            .as_ref()
            .expect("runtime")
            .task_snapshots()
            .expect("snapshots")
            .iter()
            .any(|task| task.task_id == "async-task" && task.status == TaskStatus::Completed);
        if completed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    {
        let host = runtime.inner.host_runtime.lock().expect("runtime mutex");
        let host = host.as_ref().expect("runtime");
        assert!(
            host.task_snapshots()
                .unwrap()
                .iter()
                .any(|task| task.task_id == "async-task" && task.status == TaskStatus::Completed)
        );
        assert!(host.async_executor().unwrap().is_some());
    }
    runtime.shutdown().await;
}

#[test]
fn configured_plugin_catalog_rejects_duplicate_and_raw_credentials() {
    let cases = [
        (
            vec![
                configured_selection("test.configured", json!({"mode": "enabled"})),
                configured_selection("test.configured", json!({"mode": "enabled"})),
            ],
            "selected more than once",
        ),
        (
            vec![configured_selection(
                "test.configured",
                json!({"mode": "enabled", "client_secret": "raw"}),
            )],
            "contains raw credential field client_secret",
        ),
    ];
    for (configured, expected) in cases {
        let mut config = ServiceConfig::default();
        config.plugins.configured = configured;
        let mut catalog = ConfiguredPluginCatalog::new();
        catalog.register(TestConfiguredFactory).unwrap();
        let error = ServiceRuntimeBuilder::new(config)
            .with_configured_plugin_catalog(catalog)
            .install_configured_plugins()
            .err()
            .expect("configuration must fail");
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn configured_plugin_catalog_merge_is_atomic_and_rejects_duplicates() {
    let mut first = ConfiguredPluginCatalog::new();
    first.register(TestConfiguredFactory).unwrap();
    let mut second = ConfiguredPluginCatalog::new();
    second.register(SecondConfiguredFactory).unwrap();
    first.merge(second).unwrap();
    assert!(first.factory("test.configured").is_some());
    assert!(first.factory("test.configured.second").is_some());

    let mut duplicate = ConfiguredPluginCatalog::new();
    duplicate.register(TestConfiguredFactory).unwrap();
    let error = first.merge(duplicate).unwrap_err();
    assert!(matches!(
        error,
        ServiceRuntimeError::DuplicateConfiguredPlugin(plugin_id)
            if plugin_id == "test.configured"
    ));
    assert!(first.factory("test.configured.second").is_some());
}

#[test]
fn deployment_resolver_defaults_to_builtin_and_honors_host_preference() {
    let mut config = ServiceConfig::default();
    config.plugins.configured = vec![configured_selection("test.choice", Value::Null)];
    let builtin_manifest = minimal_manifest("test.choice");
    let mut abi_manifest = builtin_manifest.clone();
    abi_manifest.artifact.artifact_type = ArtifactType::Abi;
    abi_manifest.artifact.path = "test_choice.dll".into();
    abi_manifest.artifact.sha256 = format!("sha256:{}", "1".repeat(64));
    let inventory = PluginInventory {
        records: vec![
            PluginRecord {
                manifest_path: "<builtin>".into(),
                manifest: builtin_manifest,
                runtime: None,
                resolved_artifact: None,
            },
            PluginRecord {
                manifest_path: "plugin.toml".into(),
                manifest: abi_manifest,
                runtime: None,
                resolved_artifact: Some("test_choice.dll".into()),
            },
        ],
        diagnostics: Vec::new(),
    };
    let default = resolve_catalog(
        &config,
        inventory.clone(),
        &PluginDeploymentState::default(),
    )
    .unwrap();
    assert_eq!(
        deployment_for(&default.records[0]),
        PluginDeploymentKind::Builtin
    );

    let preferred = resolve_catalog(
        &config,
        inventory,
        &PluginDeploymentState {
            version: deployment_state_version(),
            plugins: [("test.choice".into(), PluginDeploymentKind::Abi)].into(),
        },
    )
    .unwrap();
    assert_eq!(
        deployment_for(&preferred.records[0]),
        PluginDeploymentKind::Abi
    );
}

#[test]
fn log_tail_reads_recent_lines_and_advances_cursor() {
    let dir = tempdir().expect("temp dir");
    let log_path = dir.path().join("service.log");
    std::fs::write(&log_path, "one\ntwo\nthree\n").expect("write log");

    let response = read_log_tail(
        &log_path,
        LogTailParams {
            cursor: None,
            lines: Some(2),
            filters: Default::default(),
        },
    )
    .expect("tail succeeds");

    assert_eq!(response.entries.len(), 2);
    assert_eq!(response.entries[0].line, "two");
    assert_eq!(response.entries[1].line, "three");

    std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("open log")
        .write_all(b"four\n")
        .expect("append log");
    let next = read_log_tail(
        &log_path,
        LogTailParams {
            cursor: Some(response.cursor),
            lines: Some(10),
            filters: Default::default(),
        },
    )
    .expect("incremental tail succeeds");

    assert_eq!(next.entries.len(), 1);
    assert_eq!(next.entries[0].line, "four");
    assert!(next.cursor > response.cursor);
}

#[test]
fn log_tail_resets_cursor_after_truncation() {
    let dir = tempdir().expect("temp dir");
    let log_path = dir.path().join("service.log");
    std::fs::write(&log_path, "fresh\n").expect("write log");

    let response = read_log_tail(
        &log_path,
        LogTailParams {
            cursor: Some(10_000),
            lines: Some(10),
            filters: Default::default(),
        },
    )
    .expect("tail succeeds");

    assert_eq!(response.entries.len(), 1);
    assert_eq!(response.entries[0].line, "fresh");
}

#[test]
fn log_tail_rejects_filters() {
    let dir = tempdir().expect("temp dir");
    let log_path = dir.path().join("service.log");
    std::fs::write(&log_path, "line\n").expect("write log");
    let mut filters = BTreeMap::new();
    filters.insert("level".into(), "info".into());

    let error = read_log_tail(
        &log_path,
        LogTailParams {
            cursor: None,
            lines: None,
            filters,
        },
    )
    .expect_err("filters rejected");

    assert!(matches!(error, ControlError::BadRequest(_)));
}

#[tokio::test]
async fn task_list_returns_live_runtime_snapshots() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    {
        let mut guard = inner.host_runtime.lock().expect("host runtime mutex");
        let runtime = guard.as_mut().expect("runtime started");
        let mut task = Task::new("control-task-1", "control.input", json!({ "hidden": true }));
        task.priority = 3;
        task.trace_id = Some("trace-control".into());
        task.required_surfaces = vec!["surface:control".into()];
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))
            .expect("submit task");
    }

    let response = inner
        .handle_request(ControlRequest::new("token", ControlCommand::TaskList))
        .await;

    let snapshots: Vec<TaskSnapshot> = control_result!(response, ControlResult::TaskList);
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].task_id, "control-task-1");
    assert_eq!(snapshots[0].protocol_id, "control.input");
    assert_eq!(snapshots[0].status, "ready");
    assert_eq!(snapshots[0].priority, 3);
    assert_eq!(
        snapshots[0].trace_id.as_ref().map(|id| id.as_str()),
        Some("trace-control")
    );
    assert_eq!(
        snapshots[0].required_surfaces,
        vec![mutsuki_runtime_contracts::SurfaceId::from(
            "surface:control"
        )]
    );
    assert!(snapshots[0].lease_id.is_none());
    assert!(snapshots[0].failure.is_none());
}

#[tokio::test]
async fn task_wait_rejects_timeouts_above_the_control_cap() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    let response = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::TaskWait(TaskWaitParam {
                ids: vec!["any".into()],
                timeout_ms: mutsuki_service_control::MAX_TASK_WAIT_TIMEOUT_MS + 1,
            }),
        ))
        .await;
    assert!(matches!(
        response,
        ControlResponse::Error(ref body) if body.code == ControlErrorCode::BadRequest
    ));
}

#[tokio::test]
async fn host_metrics_returns_process_snapshot() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    let response = inner
        .handle_request(ControlRequest::new("token", ControlCommand::HostMetrics))
        .await;
    let metrics: HostMetrics = control_result!(response, ControlResult::HostMetrics);
    assert_eq!(metrics.pid, std::process::id());
    assert!(metrics.cpu_time_ms.is_some());
    assert!(metrics.core.is_some());
    assert!(!metrics.execution_domains.is_empty());
    assert!(
        metrics
            .execution_domains
            .iter()
            .all(|domain| !domain.lanes.is_empty())
    );
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    assert!(metrics.rss_bytes.unwrap_or(0) > 0);
    // uptime_ms may be 0 when the runtime was just started.
}

#[tokio::test]
async fn configured_execution_domains_boot_through_the_service_runtime_path() {
    let dir = tempdir().expect("temp dir");
    let core = mutsuki_service_config::CoreSection {
        execution_domains: vec![
            mutsuki_service_config::ExecutionDomainSection {
                id: "interactive".into(),
                execution_classes: vec![ExecutionClassName::Orchestration, ExecutionClassName::Cpu],
                threads: 1,
                ..mutsuki_service_config::ExecutionDomainSection::default()
            },
            mutsuki_service_config::ExecutionDomainSection {
                id: "background".into(),
                execution_classes: vec![
                    ExecutionClassName::Io,
                    ExecutionClassName::Blocking,
                    ExecutionClassName::Script,
                ],
                threads: 1,
                ..mutsuki_service_config::ExecutionDomainSection::default()
            },
        ],
        ..mutsuki_service_config::CoreSection::default()
    };
    let inner = test_started_runtime_inner_with_core("token", dir.path(), core).await;

    let metrics: HostMetrics = control_result!(inner.host_metrics(), ControlResult::HostMetrics);
    assert_eq!(metrics.execution_domains.len(), 2);
    assert!(
        metrics
            .execution_domains
            .iter()
            .any(|domain| domain.domain_id == "interactive")
    );
    assert!(
        metrics
            .execution_domains
            .iter()
            .any(|domain| domain.domain_id == "background")
    );
    assert!(
        metrics
            .execution_domains
            .iter()
            .all(|domain| domain.active_threads == 1)
    );
}

#[tokio::test]
async fn runtime_statistics_returns_task_pool_counts() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    {
        let mut guard = inner.host_runtime.lock().expect("host runtime mutex");
        let runtime = guard.as_mut().expect("runtime started");
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                "stats-task-1",
                "control.input",
                json!({}),
            ))))
            .expect("submit task");
    }

    let response = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::RuntimeStatistics,
        ))
        .await;

    let stats: RuntimeStatisticsView = control_result!(response, ControlResult::RuntimeStatistics);
    assert_eq!(stats.tasks.ready, 1);
    assert_eq!(stats.tasks.submitted_total, 1);

    {
        let mut guard = inner.host_runtime.lock().expect("host runtime mutex");
        *guard = None;
    }
    let missing = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::RuntimeStatistics,
        ))
        .await;
    assert_eq!(
        missing.error().expect("failed").code,
        ControlErrorCode::CoreUnavailable
    );
}

#[tokio::test]
async fn task_cancel_and_outcome_use_task_handle() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    {
        let mut guard = inner.host_runtime.lock().expect("host runtime mutex");
        let runtime = guard.as_mut().expect("runtime started");
        runtime
            .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
                "cancel-task-1",
                "control.input",
                json!({}),
            ))))
            .expect("submit task");
    }

    let cancel = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::TaskCancel(IdParam {
                id: "cancel-task-1".into(),
            }),
        ))
        .await;
    assert!(matches!(
        cancel,
        ControlResponse::Ok(ControlResult::TaskCancel)
    ));

    let outcome = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::TaskOutcome(IdParam {
                id: "cancel-task-1".into(),
            }),
        ))
        .await;
    let view: TaskOutcomeView = control_result!(outcome, ControlResult::TaskOutcome);
    assert_eq!(view.task_id, "cancel-task-1");
    assert_eq!(view.status, "cancelled");
}

#[tokio::test]
async fn distributed_neutral_control_submits_observes_and_drains_core() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    let submit = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::TaskSubmitBatch(TaskSubmitBatchParam {
                batch: TaskBatch::one(
                    "control-batch-1",
                    Task::new("submitted-task-1", "control.input", json!({ "value": 1 })),
                ),
            }),
        ))
        .await;
    let submitted: TaskSubmitBatchResponse =
        control_result!(submit, ControlResult::TaskSubmitBatch);
    assert_eq!(submitted.handles.len(), 1);
    assert_eq!(submitted.handles[0].task_id, "submitted-task-1");

    let events = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::TaskEventsAfter(TaskEventsAfterParam {
                sequence: 0,
                limit: 16,
            }),
        ))
        .await;
    let page: TaskEventPage = control_result!(events, ControlResult::TaskEventsAfter);
    assert!(!page.events.is_empty());
    assert!(page.next_sequence > 0);
    assert_eq!(page.lost, 0);
    assert_eq!(page.dropped, 0);
    assert!(page.latest_sequence >= page.next_sequence);
    assert!(page.earliest_available_sequence.is_some());
    assert!(page.events.iter().any(|event| {
        event.kind == mutsuki_runtime_contracts::RuntimeEventKind::Task
            && event.subject_id.as_deref() == Some("submitted-task-1")
    }));

    let invalid_events = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::TaskEventsAfter(TaskEventsAfterParam {
                sequence: 0,
                limit: 0,
            }),
        ))
        .await;
    assert_eq!(
        invalid_events.error().expect("invalid event error").code,
        ControlErrorCode::BadRequest
    );

    let drain = inner
        .handle_request(ControlRequest::new("token", ControlCommand::CoreBeginDrain))
        .await;
    let drain: CoreDrainResponse = control_result!(drain, ControlResult::CoreBeginDrain);
    assert_eq!(drain.state, "draining");

    let rejected = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::TaskSubmitBatch(TaskSubmitBatchParam {
                batch: TaskBatch::one(
                    "control-batch-2",
                    Task::new("submitted-task-2", "control.input", json!({})),
                ),
            }),
        ))
        .await;
    assert_eq!(
        rejected.error().expect("drain rejection").code,
        ControlErrorCode::Failed
    );
}

#[test]
fn task_outcome_preserves_structured_failure_evidence() {
    let mut error = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "plugin.test",
        "test.route",
    );
    error.evidence.insert(
        "message".into(),
        mutsuki_runtime_contracts::ScalarValue::String("redacted detail".into()),
    );

    let view = to_control_task_outcome(
        &TaskId::from("failed-task"),
        Some(TaskOutcome::Failed {
            task_id: "failed-task".into(),
            error,
        }),
    );

    assert_eq!(view.status, "failed");
    assert_eq!(view.reason.as_deref(), Some("test.route"));
    assert_eq!(view.evidence["message"], json!("redacted detail"));
}

#[test]
fn task_outcome_exposes_inline_business_output() {
    let view = to_control_task_outcome(
        &TaskId::from("completed-task"),
        Some(TaskOutcome::Completed {
            task_id: "completed-task".into(),
            output: Some(json!({"answer": 42})),
            output_ref: None,
        }),
    );

    assert_eq!(view.status, "completed");
    assert_eq!(view.output, Some(json!({"answer": 42})));
}

#[test]
fn service_host_uses_binary_run_batch_not_step() {
    use std::io::Cursor;

    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, CompletionBatch, DispatchLane, EntryCompletion,
        OrderingRequirement, RunnerContext, RunnerResult, TaskLease, WorkBatch, WorkResourcePlan,
    };
    use mutsuki_runtime_wire::{
        BINARY_CODEC_ID, DEFAULT_WIRE_LIMITS, Opcode, ProtocolHello, ProtocolHelloAck,
        RunBatchRequest, decode_binary_request, encode_binary_response,
    };

    let descriptor = RunnerDescriptor {
        runner_id: "binary.test".into(),
        plugin_id: "plugin.test".into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec!["raw.input".into()],
        purity: mutsuki_runtime_contracts::RunnerPurity::Pure,
        execution_class: mutsuki_runtime_contracts::ExecutionClass::Cpu,
        invocation_mode: mutsuki_runtime_contracts::InvocationMode::SyncExclusive,
        concurrency: mutsuki_runtime_contracts::RunnerConcurrency::Exclusive,
        input_schema: json!({}),
        output_schema: json!({}),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec!["runner:binary.test".into()],
    };
    let mut task = Task::new("task-1", "raw.input", json!({}));
    task.lease_id = Some("lease-1".into());
    let batch = WorkBatch {
        batch_id: "batch-1".into(),
        tick_id: "tick-1".into(),
        batch_key: "binary.test".into(),
        entries: vec![BatchEntry {
            entry_id: "task-1".into(),
            task_id: "task-1".into(),
            trace_id: None,
            parent_id: None,
            payload_index: 0,
            resource_requirement_indices: Vec::new(),
            cancel_index: Some(0),
            deadline_tick: None,
            priority: 0,
            lane: DispatchLane::Normal,
            ordering: OrderingRequirement::None,
        }],
        payload: BatchPayload::from_tasks(&[task.clone()]),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: vec![TaskLease {
            lease_id: "lease-1".into(),
            task_id: "task-1".into(),
            runner_id: "binary.test".into(),
            executor_id: "executor:test".into(),
            registry_generation: 1,
            attempt_generation: 0,
            acquired_at_step: 1,
            expires_at_step: None,
        }],
    };
    let completion = CompletionBatch {
        batch_id: "batch-1".into(),
        tick_id: "tick-1".into(),
        results: vec![EntryCompletion {
            entry_id: "task-1".into(),
            task_id: "task-1".into(),
            result: Some(RunnerResult::completed("task-1")),
            error: None,
        }],
        metadata: Vec::new(),
    };
    let hello = ProtocolHello::binary();
    let ack: ProtocolHelloAck = hello.accept(BINARY_CODEC_ID, None).unwrap();
    let mut responses =
        encode_binary_response(1, Opcode::PluginInitialize, Ok(&ack), DEFAULT_WIRE_LIMITS)
            .expect("encode handshake response");
    responses.extend(
        encode_binary_response(
            2,
            Opcode::RunnerRunBatch,
            Ok(&completion),
            DEFAULT_WIRE_LIMITS,
        )
        .expect("encode run response"),
    );
    let reader = Cursor::new(responses);
    let writer = Cursor::new(Vec::<u8>::new());
    let mut runner = BinaryRunner::new(descriptor, reader, writer);
    let result = runner
        .run_batch(
            RunnerContext::new(
                1,
                1,
                "executor:test",
                Some::<mutsuki_runtime_contracts::TaskLeaseId>("lease-1".into()),
                "invocation:test",
            ),
            batch,
        )
        .expect("run_batch");
    let (_reader, writer) = runner.into_inner();
    let written = writer.into_inner();
    let request_offset = 4 + u32::from_be_bytes(written[..4].try_into().unwrap()) as usize;
    let (_, request) =
        decode_binary_request::<RunBatchRequest>(&written[request_offset..], DEFAULT_WIRE_LIMITS)
            .unwrap();
    assert_eq!(result.batch_id, "batch-1");
    assert_eq!(request.runner_id, "binary.test");
    assert_eq!(request.batch.batch_id, "batch-1");
}

#[tokio::test]
async fn plugin_reload_requires_auth_and_swaps_generation() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;

    let unauthorized = inner
        .handle_request(ControlRequest::new("wrong", ControlCommand::PluginReload))
        .await;
    assert_eq!(
        unauthorized.error().expect("error").code,
        ControlErrorCode::Unauthorized
    );

    let response = inner
        .handle_request(ControlRequest::new("token", ControlCommand::PluginReload))
        .await;
    let reload: PluginReloadResponse = control_result!(response, ControlResult::PluginReload);
    assert_eq!(reload.previous_generation, 1);
    assert_eq!(reload.registry_generation, 2);
    assert_eq!(reload.plugin_count, 1);

    let status: CoreStatus = control_result!(inner.core_status(), ControlResult::CoreStatus);
    assert_eq!(status.registry_generation, Some(2));
    let guard = inner.host_runtime.lock().expect("host runtime mutex");
    assert_eq!(
        guard
            .as_ref()
            .expect("runtime")
            .host_context()
            .registry_generation(),
        2
    );
}

#[tokio::test]
async fn deployment_management_persists_only_available_configured_choice() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    let unavailable = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::PluginDeploymentSet(PluginDeploymentParam {
                plugin_id: TEST_PLUGIN_ID.into(),
                deployment: PluginDeploymentKind::Abi,
            }),
        ))
        .await;
    assert!(unavailable.error().is_some());
    assert!(!deployment_state_path(&inner.config).exists());

    let selected = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::PluginDeploymentSet(PluginDeploymentParam {
                plugin_id: TEST_PLUGIN_ID.into(),
                deployment: PluginDeploymentKind::Builtin,
            }),
        ))
        .await;
    assert!(matches!(
        selected,
        ControlResponse::Ok(ControlResult::PluginDeploymentSet(_))
    ));
    let state = load_deployment_state(&inner.config).unwrap();
    assert_eq!(
        state.plugins.get(TEST_PLUGIN_ID),
        Some(&PluginDeploymentKind::Builtin)
    );

    let cleared = inner
        .handle_request(ControlRequest::new(
            "token",
            ControlCommand::PluginDeploymentClear(PluginDeploymentClearParam {
                plugin_id: TEST_PLUGIN_ID.into(),
            }),
        ))
        .await;
    assert!(matches!(
        cleared,
        ControlResponse::Ok(ControlResult::PluginDeploymentClear(_))
    ));
    assert!(
        load_deployment_state(&inner.config)
            .unwrap()
            .plugins
            .is_empty()
    );
}

#[tokio::test]
async fn plugin_reload_keeps_unselected_invalid_artifact_as_diagnostic() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    std::fs::create_dir_all(dir.path().join("installed").join("bad")).expect("plugin dir");
    std::fs::write(
        dir.path().join("installed").join("bad").join("plugin.toml"),
        "not valid toml",
    )
    .expect("write invalid manifest");

    let response = inner
        .handle_request(ControlRequest::new("token", ControlCommand::PluginReload))
        .await;
    assert!(matches!(
        response,
        ControlResponse::Ok(ControlResult::PluginReload(_))
    ));

    let status: CoreStatus = control_result!(inner.core_status(), ControlResult::CoreStatus);
    assert_eq!(status.registry_generation, Some(2));
    let plugins: PluginListResponse =
        control_result!(inner.plugin_list(), ControlResult::PluginList);
    assert_eq!(plugins.plugins.len(), 1);
    assert_eq!(plugins.plugins[0].plugin_id, TEST_PLUGIN_ID);
    assert_eq!(plugins.diagnostics.len(), 1);
}

#[tokio::test]
async fn plugin_list_reflects_catalog_after_successful_reload() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    let plugin_dir = dir.path().join("installed").join("dynamic");
    std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
    let mut manifest = minimal_manifest("mutsuki.dynamic.test");
    manifest.artifact.artifact_type = ArtifactType::Process;
    manifest.artifact.path = "dynamic-test".into();
    manifest.artifact.sha256 = "sha256:dynamic-test".into();
    let plugin = mutsuki_service_plugin_loader::PluginToml {
        manifest,
        runtime: None,
    };
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        toml::to_string(&plugin).expect("manifest toml"),
    )
    .expect("write manifest");

    let response = inner
        .handle_request(ControlRequest::new("token", ControlCommand::PluginReload))
        .await;
    let reload: PluginReloadResponse = control_result!(response, ControlResult::PluginReload);
    assert_eq!(reload.plugin_count, 1);

    let plugins: PluginListResponse =
        control_result!(inner.plugin_list(), ControlResult::PluginList);
    assert!(
        plugins
            .plugins
            .iter()
            .any(|plugin| plugin.plugin_id == "mutsuki.dynamic.test")
    );
}

#[tokio::test]
async fn plugin_list_includes_loaded_records_missing_from_configured_and_candidates() {
    let dir = tempdir().expect("temp dir");
    let inner = test_started_runtime_inner("token", dir.path()).await;
    {
        let mut catalog = inner.catalog.lock().expect("catalog mutex");
        catalog.records.push(PluginRecord {
            manifest_path: "<builtin>".into(),
            manifest: minimal_manifest("mutsuki.bot.mihuashi"),
            runtime: None,
            resolved_artifact: None,
        });
    }

    let plugins: PluginListResponse =
        control_result!(inner.plugin_list(), ControlResult::PluginList);
    let loaded = plugins
        .plugins
        .iter()
        .find(|plugin| plugin.plugin_id == "mutsuki.bot.mihuashi")
        .expect("records-only plugin");
    assert!(!loaded.configured);
    assert_eq!(loaded.active_deployment.as_deref(), Some("builtin"));
    assert!(loaded.candidates.is_empty());
}

#[test]
fn event_source_required_secret_is_validated_before_runtime_start() {
    let secret_key = format!("MISSING_EVENT_SOURCE_SECRET_{}", std::process::id());
    let sources: Vec<Box<dyn HostEventSource>> = vec![Box::new(FailingEventSource {
        descriptor: HostEventSourceDescriptor::new("secret-source", "test.source")
            .require_secret(secret_key.clone()),
    })];

    let error = validate_event_sources(&sources, &ServiceConfig::default())
        .expect_err("missing required Host secret must fail preflight");

    assert!(matches!(
        error,
        ServiceRuntimeError::EventSource(message)
            if message.contains("secret-source") && message.contains(&secret_key)
    ));
}

#[tokio::test]
async fn fallible_native_runner_factory_fails_without_panicking() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.log_dir = dir.path().join("logs");
    config.plugins.dynamic_dirs.clear();

    let result = ServiceRuntimeBuilder::new(config)
        .register_fallible_builtin_runner(|| -> Result<Box<dyn Runner>, &'static str> {
            Err("client init rejected")
        })
        .start()
        .await;

    assert!(matches!(
        result,
        Err(ServiceRuntimeError::NativeRunnerFactory(message))
            if message == "client init rejected"
    ));
}

#[tokio::test]
async fn product_health_probe_is_exposed_without_domain_coupling() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = dir.path().to_path_buf();
    config.service.data_dir = dir.path().join("data");
    config.service.log_dir = dir.path().join("logs");
    config.service.run_dir = dir.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = dir.path().join("disabled");

    let runtime = ServiceRuntimeBuilder::new(config)
        .register_health_probe(
            "test.component",
            || serde_json::json!({"status": "ok", "ready": true}),
        )
        .start()
        .await
        .expect("runtime starts");
    let report: HealthReport = control_result!(
        runtime.inner.health_check().await,
        ControlResult::HealthCheck
    );
    assert_eq!(report.components["test.component"]["status"], "ok");
    assert_eq!(report.components["test.component"]["ready"], true);
    runtime.shutdown().await;
}

#[tokio::test]
async fn health_reports_degraded_when_native_isolation_capacity_is_exhausted() {
    struct BlockingGateRunner {
        descriptor: RunnerDescriptor,
        gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }

    impl Runner for BlockingGateRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.descriptor
        }

        fn run_batch(
            &mut self,
            _ctx: mutsuki_runtime_contracts::RunnerContext,
            batch: WorkBatch,
        ) -> RuntimeResult<CompletionBatch> {
            let (released, wake) = &*self.gate;
            let mut released = released.lock().expect("worker gate poisoned");
            while !*released {
                released = wake.wait(released).expect("worker gate poisoned");
            }
            map_work_batch_entries(&batch, |task| {
                Ok(mutsuki_runtime_contracts::RunnerResult::completed(
                    task.task_id.clone(),
                ))
            })
        }
    }

    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = dir.path().to_path_buf();
    config.service.data_dir = dir.path().join("data");
    config.service.log_dir = dir.path().join("logs");
    config.service.run_dir = dir.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = dir.path().join("disabled");
    config.plugins.configured = vec![configured_selection("test.worker.health", Value::Null)];
    config.core.worker_profile = mutsuki_service_config::WorkerProfile::LowResource;
    config.core.runner_wall_clock_timeout_ms = Some(30);
    config.core.max_isolated_workers = Some(1);

    let mut descriptor = chain_descriptor(
        "test.worker.health",
        "test.worker.health.runner",
        "test.worker.health.run",
    );
    descriptor.execution_class = ExecutionClass::Blocking;
    let factory_descriptor = descriptor.clone();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let factory_gate = gate.clone();
    let runtime = ServiceRuntimeBuilder::new(config)
        .register_builtin_plugin(runner_manifest("test.worker.health", descriptor))
        .register_builtin_runner(move || {
            Box::new(BlockingGateRunner {
                descriptor: factory_descriptor.clone(),
                gate: factory_gate.clone(),
            })
        })
        .start()
        .await
        .expect("runtime starts");
    runtime
        .inner
        .host_runtime
        .lock()
        .expect("runtime mutex")
        .as_ref()
        .expect("runtime")
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "worker-health-hang",
            "test.worker.health.run",
            json!({}),
        ))))
        .expect("task submitted");
    let report = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let report: HealthReport = control_result!(
                runtime.inner.health_check().await,
                ControlResult::HealthCheck
            );
            if report.core == "degraded" {
                break report;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("worker isolation reaches degraded health");
    assert_eq!(report.core, "degraded");
    assert!(
        report
            .recent_errors
            .iter()
            .any(|error| error == "worker_pool:isolation_capacity_exhausted")
    );
    assert!(
        report.components["worker_pools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pool| pool["pool_id"] == "blocking" && pool["degraded"] == true)
    );

    let (released, wake) = &*gate;
    *released.lock().expect("worker gate poisoned") = true;
    wake.notify_all();
    runtime.shutdown().await;
}

#[tokio::test]
async fn idle_service_runtime_does_not_tick_and_health_and_shutdown_remain_responsive() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = dir.path().to_path_buf();
    config.service.data_dir = dir.path().join("data");
    config.service.log_dir = dir.path().join("logs");
    config.service.run_dir = dir.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = dir.path().join("disabled");

    let runtime = ServiceRuntimeBuilder::new(config)
        .start()
        .await
        .expect("idle runtime starts");
    let before = runtime
        .inner
        .host_runtime
        .lock()
        .expect("runtime mutex")
        .as_ref()
        .expect("runtime")
        .drive_state()
        .expect("drive state");

    tokio::time::sleep(Duration::from_millis(50)).await;
    let health = tokio::time::timeout(Duration::from_secs(1), runtime.inner.health_check())
        .await
        .expect("health remains responsive while Core actor sleeps");
    let report: HealthReport = control_result!(health, ControlResult::HealthCheck);
    assert_eq!(report.core, "ok");
    let after = runtime
        .inner
        .host_runtime
        .lock()
        .expect("runtime mutex")
        .as_ref()
        .expect("runtime")
        .drive_state()
        .expect("drive state");
    assert_eq!(after.current_step, before.current_step);
    assert_eq!(after.timed_wakeups, before.timed_wakeups);
    assert_eq!(after.next_wake_deadline, None);

    tokio::time::timeout(Duration::from_secs(3), runtime.shutdown())
        .await
        .expect("shutdown interrupts idle actor sleep");
}

#[tokio::test]
async fn waiting_for_shutdown_request_does_not_release_the_runtime() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.observe.console = false;
    config.service.home_dir = dir.path().to_path_buf();
    config.service.data_dir = dir.path().join("data");
    config.service.log_dir = dir.path().join("logs");
    config.service.run_dir = dir.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = dir.path().join("disabled");

    let mut runtime = ServiceRuntimeBuilder::new(config)
        .start()
        .await
        .expect("runtime starts");
    runtime
        .wait_for_shutdown_request_with(async { "test".to_string() })
        .await
        .expect("shutdown request observed");
    let report = runtime.inner.health_check().await;
    let report: HealthReport = control_result!(report, ControlResult::HealthCheck);
    assert_eq!(report.core, "ok");
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn product_builder_event_source_runs_real_three_stage_pipeline() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    std::fs::create_dir_all(dir.path().join("run")).expect("run");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.ipc.token = Some("test-token".into());
    config.observe.console = false;
    config.service.home_dir = dir.path().to_path_buf();
    config.service.log_dir = dir.path().join("logs");
    config.service.run_dir = dir.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = dir.path().join("disabled");
    config.plugins.configured = [
        "test.stage.first",
        "test.stage.second",
        "test.stage.terminal",
    ]
    .into_iter()
    .map(|id| configured_selection(id, Value::Null))
    .collect();
    config.runners.graceful_shutdown_ms = 250;

    let terminal_count = Arc::new(AtomicUsize::new(0));
    let source_starts = Arc::new(AtomicUsize::new(0));
    let source_stops = Arc::new(AtomicUsize::new(0));
    let client_factory_count = Arc::new(AtomicUsize::new(0));
    let client_checks = Arc::new(AtomicUsize::new(0));
    let source = MockEventSource {
        descriptor: HostEventSourceDescriptor::new("mock-source", "test.source"),
        starts: source_starts.clone(),
        stops: source_stops.clone(),
    };
    let first_descriptor =
        chain_descriptor("test.stage.first", "test.stage.first.runner", "test.input");
    let second_descriptor = chain_descriptor(
        "test.stage.second",
        "test.stage.second.runner",
        "test.intermediate",
    );
    let terminal_descriptor = chain_descriptor(
        "test.stage.terminal",
        "test.stage.terminal.runner",
        "test.output",
    );

    let first_factory_descriptor = first_descriptor.clone();
    let second_factory_descriptor = second_descriptor.clone();
    let second_factory_count = client_factory_count.clone();
    let second_client_checks = client_checks.clone();
    let terminal_factory_descriptor = terminal_descriptor.clone();
    let terminal_factory_count = terminal_count.clone();
    let runtime = ServiceRuntimeBuilder::new(config)
        .register_builtin_plugin(runner_manifest("test.stage.first", first_descriptor))
        .register_builtin_plugin(runner_manifest("test.stage.second", second_descriptor))
        .register_builtin_plugin(runner_manifest("test.stage.terminal", terminal_descriptor))
        .register_builtin_runner(move || {
            Box::new(ChainRunner::next(
                first_factory_descriptor.clone(),
                "test.intermediate",
            ))
        })
        .register_runtime_client_runner(move |client| {
            second_factory_count.fetch_add(1, Ordering::SeqCst);
            Box::new(ChainRunner::next_with_client(
                second_factory_descriptor.clone(),
                "test.output",
                client,
                second_client_checks.clone(),
            ))
        })
        .register_builtin_runner(move || {
            Box::new(ChainRunner::terminal(
                terminal_factory_descriptor.clone(),
                terminal_factory_count.clone(),
            ))
        })
        .register_event_source(Box::new(source))
        .start()
        .await
        .expect("real service runtime starts");

    wait_for_count(&terminal_count, 1).await;
    assert_eq!(client_factory_count.load(Ordering::SeqCst), 1);
    assert_eq!(client_checks.load(Ordering::SeqCst), 1);
    let snapshots = runtime
        .inner
        .host_runtime
        .lock()
        .expect("runtime mutex")
        .as_ref()
        .expect("runtime")
        .task_snapshots()
        .expect("task snapshots");
    assert_eq!(snapshots.len(), 3);
    assert!(
        snapshots
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
    );
    assert!(
        snapshots
            .iter()
            .all(|task| task.correlation_id.as_deref() == Some("corr-mock-1"))
    );

    let unauthorized_sources = runtime
        .inner
        .handle_request(ControlRequest::new(
            "wrong",
            ControlCommand::EventSourceList,
        ))
        .await;
    assert_eq!(
        unauthorized_sources.error().expect("unauthorized").code,
        ControlErrorCode::Unauthorized
    );
    let sources = runtime
        .inner
        .handle_request(ControlRequest::new(
            "test-token",
            ControlCommand::EventSourceList,
        ))
        .await;
    let sources: Vec<mutsuki_service_control::EventSourceStatus> =
        control_result!(sources, ControlResult::EventSourceList);
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].state, "running");
    assert_eq!(sources[0].health, "healthy");
    assert!(sources[0].last_event_unix_ms.is_some());
    assert!(sources[0].started_at_unix_ms.is_some());

    let health: HealthReport = control_result!(
        runtime.inner.health_check().await,
        ControlResult::HealthCheck
    );
    assert_eq!(health.event_sources, "ok");
    assert_eq!(health.event_source_details[0].source_id, "mock-source");

    let reload = runtime.inner.plugin_reload().await;
    let reload: PluginReloadResponse = control_result!(reload, ControlResult::PluginReload);
    assert_eq!(reload.event_sources, "kept");
    assert_eq!(source_starts.load(Ordering::SeqCst), 1);
    assert_eq!(client_factory_count.load(Ordering::SeqCst), 2);

    let restart = runtime
        .inner
        .handle_request(ControlRequest::new(
            "test-token",
            ControlCommand::EventSourceRestart(IdParam {
                id: "mock-source".into(),
            }),
        ))
        .await;
    assert!(matches!(
        restart,
        ControlResponse::Ok(ControlResult::EventSourceRestart)
    ));
    wait_for_count(&terminal_count, 2).await;
    assert_eq!(client_checks.load(Ordering::SeqCst), 2);
    let sources = runtime.inner.event_sources.list();
    assert_eq!(sources[0].reconnects, 1);
    assert_eq!(sources[0].state, "running");

    runtime.shutdown().await;
    assert_eq!(source_starts.load(Ordering::SeqCst), 2);
    assert_eq!(source_stops.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn health_reports_event_source_runtime_failure_without_stopping_service() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("logs")).expect("logs");
    let mut config = ServiceConfig::default();
    config.ipc.enabled = false;
    config.ipc.token = Some("test-token".into());
    config.observe.console = false;
    config.service.home_dir = dir.path().to_path_buf();
    config.service.data_dir = dir.path().join("data");
    config.service.log_dir = dir.path().join("logs");
    config.service.run_dir = dir.path().join("run");
    config.plugins.dynamic_dirs.clear();
    config.plugins.disabled_dir = dir.path().join("disabled");
    config.runners.graceful_shutdown_ms = 50;
    let runtime = ServiceRuntimeBuilder::new(config)
        .register_event_source(Box::new(FailingEventSource {
            descriptor: HostEventSourceDescriptor::new("failed-source", "test.source"),
        }))
        .start()
        .await
        .expect("service stays available");
    tokio::time::timeout(Duration::from_secs(1), async {
        while runtime.inner.event_sources.list()[0].state != "failed" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("source fails");

    let health: HealthReport = control_result!(
        runtime.inner.health_check().await,
        ControlResult::HealthCheck
    );
    assert_eq!(health.service, "ok");
    assert_eq!(health.core, "ok");
    assert_eq!(health.event_sources, "degraded");
    assert!(health.recent_errors[0].contains("failed-source"));
    runtime.shutdown().await;
}

async fn wait_for_count(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while counter.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime completed chain");
}

struct MockEventSource {
    descriptor: HostEventSourceDescriptor,
    starts: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
}

struct FailingEventSource {
    descriptor: HostEventSourceDescriptor,
}

impl HostEventSource for FailingEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, _ctx: HostEventSourceContext) -> HostEventSourceFuture {
        Box::pin(async { Err(std::io::Error::other("upstream disconnected").into()) })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        Box::pin(async { Ok(()) })
    }

    fn health(&self) -> HostEventSourceHealth {
        HostEventSourceHealth::Unhealthy("upstream disconnected".into())
    }
}

impl HostEventSource for MockEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        let sequence = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            let mut task = Task::new(
                format!("mock-source-{sequence}"),
                "test.input",
                json!({ "value": "pipeline input" }),
            );
            task.correlation_id = Some(format!("corr-mock-{sequence}"));
            ctx.task_submitter.submit_task(task)?;
            let mut shutdown = ctx.shutdown;
            shutdown.cancelled().await;
            Ok(())
        })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn health(&self) -> HostEventSourceHealth {
        HostEventSourceHealth::Healthy
    }
}

struct ChainRunner {
    descriptor: RunnerDescriptor,
    next_protocol: Option<&'static str>,
    terminal_count: Option<Arc<AtomicUsize>>,
    runtime_client: Option<RuntimeClientRef>,
    client_checks: Option<Arc<AtomicUsize>>,
}

impl ChainRunner {
    fn next(descriptor: RunnerDescriptor, next_protocol: &'static str) -> Self {
        Self {
            descriptor,
            next_protocol: Some(next_protocol),
            terminal_count: None,
            runtime_client: None,
            client_checks: None,
        }
    }

    fn next_with_client(
        descriptor: RunnerDescriptor,
        next_protocol: &'static str,
        runtime_client: RuntimeClientRef,
        client_checks: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            descriptor,
            next_protocol: Some(next_protocol),
            terminal_count: None,
            runtime_client: Some(runtime_client),
            client_checks: Some(client_checks),
        }
    }

    fn terminal(descriptor: RunnerDescriptor, count: Arc<AtomicUsize>) -> Self {
        Self {
            descriptor,
            next_protocol: None,
            terminal_count: Some(count),
            runtime_client: None,
            client_checks: None,
        }
    }
}

impl Runner for ChainRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: mutsuki_runtime_contracts::RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let next_protocol = self.next_protocol;
        let terminal_count = self.terminal_count.clone();
        let runtime_client = self.runtime_client.clone();
        let client_checks = self.client_checks.clone();
        map_work_batch_entries(&batch, |task| {
            if let Some(client) = &runtime_client {
                let handle = TaskHandle {
                    task_id: task.task_id.clone(),
                    protocol_id: task.protocol_id.clone(),
                    target_binding_id: task.target_binding_id.clone(),
                    cancel_policy: CancelPolicy::Cascade,
                    trace_id: task.trace_id.clone(),
                    correlation_id: task.correlation_id.clone(),
                };
                let _ = client.task_outcome(&handle).map_err(|error| {
                    mutsuki_runtime_contracts::RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                        "test.stage.second",
                        format!("runtime_client.outcome:{error}"),
                    )
                })?;
                if let Some(checks) = &client_checks {
                    checks.fetch_add(1, Ordering::SeqCst);
                }
            }
            let mut result =
                mutsuki_runtime_contracts::RunnerResult::completed(task.task_id.clone());
            if let Some(protocol) = next_protocol {
                let mut next = Task::new(
                    format!("{}:{protocol}", task.task_id),
                    protocol,
                    task.payload.clone(),
                );
                next.registry_generation = ctx.registry_generation;
                next.correlation_id = task.correlation_id.clone();
                result.tasks.push(next);
            } else if let Some(count) = &terminal_count {
                count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(result)
        })
    }
}

fn chain_descriptor(plugin_id: &str, runner_id: &str, protocol: &str) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: runner_id.into(),
        plugin_id: plugin_id.into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![protocol.into()],
        purity: RunnerPurity::Pure,
        execution_class: ExecutionClass::Orchestration,
        invocation_mode: mutsuki_runtime_contracts::InvocationMode::SyncExclusive,
        concurrency: mutsuki_runtime_contracts::RunnerConcurrency::Exclusive,
        input_schema: json!({}),
        output_schema: json!({}),
        batch: RunnerBatchCapability::default(),
        payload: RunnerPayloadCapability::default(),
        resources: RunnerResourceCapability::default(),
        ordering: RunnerOrderingCapability::default(),
        control: RunnerControlCapability::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![
            format!("runner:{runner_id}").into(),
            format!("task_protocol:{protocol}").into(),
        ],
    }
}

fn runner_manifest(plugin_id: &str, descriptor: RunnerDescriptor) -> PluginManifest {
    mutsuki_runtime_host::runner_manifest(plugin_id, vec![descriptor])
}

async fn test_started_runtime_inner(token: &str, root: &Path) -> ServiceRuntimeInner {
    test_started_runtime_inner_with_core(token, root, Default::default()).await
}

async fn test_started_runtime_inner_with_core(
    token: &str,
    root: &Path,
    core: mutsuki_service_config::CoreSection,
) -> ServiceRuntimeInner {
    let mut config = ServiceConfig::default();
    config.core = core;
    config.ipc.token = Some(token.into());
    config.service.home_dir = root.to_path_buf();
    config.service.data_dir = root.join("data");
    config.service.log_dir = root.join("logs");
    config.service.run_dir = root.join("run");
    config.plugins.configured = vec![configured_selection(TEST_PLUGIN_ID, Value::Null)];
    config.plugins.dynamic_dirs = vec![root.join("installed")];
    config.plugins.disabled_dir = root.join("disabled");
    let registry = test_builtin_registry();
    let catalog = load_catalog(&config, &registry).expect("catalog");
    let mut components = PreparedComponentRegistry::default();
    for record in registry.load_all().expect("builtin components").records {
        components.register_manifest(record.manifest);
    }
    let runtime_client = Arc::new(DeferredRuntimeClient::default());
    let (host_runtime, _) = boot_core(
        &config,
        &catalog,
        runtime_client.clone(),
        CoreBootInputs {
            native_runner_factories: &[],
            async_handler_factories: &[],
            loaded_plugin_factories: &BTreeMap::new(),
            runner_limits: &BTreeMap::new(),
            load_plan_hooks: &[],
            load_plan_observers: &[],
        },
    )
    .await
    .expect("core");
    let configured_selections = config.plugins.configured.clone();
    ServiceRuntimeInner {
        config,
        configured_selections: Mutex::new(configured_selections),
        started_at: Instant::now(),
        catalog: Mutex::new(catalog),
        host_runtime: Mutex::new(Some(host_runtime)),
        supervisor: RunnerSupervisor::new(),
        sidecar_effects: Mutex::new(BTreeMap::new()),
        load_plan_hook_effects: Mutex::new(BTreeMap::new()),
        event_sources: EventSourceSupervisor::default(),
        event_source_effects: Mutex::new(BTreeMap::new()),
        configured_plugins: ConfiguredPluginCatalog::default(),
        components: Mutex::new(components),
        component_generations: Mutex::new(BTreeMap::new()),
        reload_lock: tokio::sync::Mutex::new(()),
        runtime_client,
        deployment_state: Mutex::new(PluginDeploymentState {
            version: deployment_state_version(),
            plugins: BTreeMap::new(),
        }),
        shutdown_tx: Mutex::new(None),
        control_changes: Arc::new(ControlChangeHub::default()),
    }
}

fn minimal_manifest(plugin_id: &str) -> PluginManifest {
    PluginManifest {
        plugin_id: plugin_id.into(),
        version: "0.1.0".into(),
        api_version: "mutsuki-plugin-v1".into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "native".into(),
            sha256: "sha256:native".into(),
            companion_artifacts: Vec::new(),
        },
        provides: PluginProvides::default(),
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: Vec::new(),
            resources: Vec::new(),
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "drain_and_swap".into(),
            unload_timeout_ms: 100,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: false,
        },
        metadata: BTreeMap::new(),
    }
}
