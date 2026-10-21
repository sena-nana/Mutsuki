//! Core boot: execution domains, runtime lock files and the runtime
//! bootstrapper assembled from the plugin catalog.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) struct CoreBootInputs<'a> {
    pub(crate) native_runner_factories: &'a [NativeRunnerFactory],
    pub(crate) async_handler_factories: &'a [AsyncHandlerFactory],
    pub(crate) loaded_plugin_factories: &'a BTreeMap<PluginId, LoadedPluginFactory>,
    pub(crate) runner_limits: &'a BTreeMap<RunnerId, RunnerLimits>,
    pub(crate) load_plan_hooks: &'a [Arc<dyn LoadPlanLifecycleHook>],
    pub(crate) load_plan_observers: &'a [Arc<dyn LoadPlanObserver>],
}

pub(crate) async fn boot_core(
    config: &ServiceConfig,
    catalog: &PluginCatalog,
    runtime_client: Arc<DeferredRuntimeClient>,
    inputs: CoreBootInputs<'_>,
) -> ServiceRuntimeResult<(HostRuntime, RuntimeLoadPlan)> {
    let (bootstrapper, profile) = runtime_bootstrapper(
        config,
        catalog,
        inputs.native_runner_factories,
        inputs.async_handler_factories,
        inputs.loaded_plugin_factories,
        runtime_client,
    )
    .await?;
    let worker_settings = config.core.worker_pool_settings();
    tracing::info!(
        worker_profile = ?config.core.worker_profile,
        compute_threads = worker_settings.compute_threads,
        blocking_threads = worker_settings.blocking_threads,
        total_worker_threads = worker_settings.compute_threads + worker_settings.blocking_threads,
        queue_capacity = worker_settings.queue_capacity,
        max_inflight_bytes = worker_settings.max_inflight_bytes,
        max_isolated_workers = worker_settings.max_isolated_workers,
        runner_wall_clock_timeout_ms = ?config.core.runner_wall_clock_timeout_ms,
        cancel_grace_period_ms = ?config.core.cancel_grace_period_ms,
        worker_health_timeout_ms = ?config.core.worker_health_timeout_ms,
        native_isolation = "cooperative_cancel",
        process_isolation = "hard_terminate_and_recover",
        "resolved Host worker topology"
    );
    let host_config = HostRuntimeConfig {
        event_driven: true,
        worker_threads: worker_settings.compute_threads,
        blocking_threads: worker_settings.blocking_threads,
        pool_queue_limit: worker_settings.queue_capacity,
        pool_max_inflight_bytes: worker_settings.max_inflight_bytes,
        max_isolated_workers: worker_settings.max_isolated_workers,
        default_runner_limits: RunnerLimits {
            wall_clock_deadline: config
                .core
                .runner_wall_clock_timeout_ms
                .map(Duration::from_millis),
            ..RunnerLimits::default()
        },
        cancel_grace_period: config
            .core
            .cancel_grace_period_ms
            .map(Duration::from_millis),
        worker_health_timeout: config
            .core
            .worker_health_timeout_ms
            .map(Duration::from_millis),
        execution_domains: config
            .core
            .execution_domains
            .iter()
            .map(map_execution_domain)
            .collect(),
        actor_control_queue_limit: config
            .core
            .actor_control_queue_limit
            .unwrap_or_else(|| HostRuntimeConfig::default().actor_control_queue_limit),
        actor_data_queue_limit: config
            .core
            .actor_data_queue_limit
            .unwrap_or_else(|| HostRuntimeConfig::default().actor_data_queue_limit),
        actor_control_quota: config
            .core
            .actor_control_quota
            .unwrap_or_else(|| HostRuntimeConfig::default().actor_control_quota),
        runner_limits: inputs.runner_limits.clone(),
        async_executor: Some(Arc::new(TokioAsyncExecutor::new(
            worker_settings.compute_threads.clamp(1, 4),
            worker_settings.queue_capacity,
            worker_settings.queue_capacity,
            worker_settings.max_inflight_bytes,
        )?)),
        ..HostRuntimeConfig::default()
    };
    let prepared = bootstrapper.prepare_host_runtime_with_config(profile, host_config)?;
    let lock = prepared.load_plan().clone();
    for hook in inputs.load_plan_hooks {
        hook.validate(&lock)
            .map_err(ServiceRuntimeError::LoadPlanHook)?;
    }
    for observer in inputs.load_plan_observers {
        observer
            .validate(&lock)
            .map_err(ServiceRuntimeError::LoadPlanHook)?;
    }
    let runtime = prepared.start()?;
    write_runtime_lock(config, &lock)?;
    Ok((runtime, lock))
}

pub(crate) fn map_execution_domain(
    configured: &mutsuki_service_config::ExecutionDomainSection,
) -> ExecutionDomainConfig {
    let mut domain = ExecutionDomainConfig::new(
        configured.id.clone(),
        configured
            .execution_classes
            .iter()
            .copied()
            .map(map_execution_class)
            .collect(),
        configured.threads,
    );
    domain.queue_capacity = configured.queue_capacity;
    domain.max_inflight_bytes = configured.max_inflight_bytes;
    domain.max_isolated_threads = configured.max_isolated_threads;
    for (lane, configured_policy) in &configured.lanes {
        let lane = map_dispatch_lane(*lane);
        let policy = domain
            .lane_policies
            .entry(lane.clone())
            .or_insert_with(|| LaneExecutionPolicy::for_lane(&lane));
        apply_lane_policy(policy, configured_policy);
    }
    domain
}

pub(crate) fn apply_lane_policy(policy: &mut LaneExecutionPolicy, configured: &LanePolicySection) {
    if let Some(value) = configured.weight {
        policy.weight = value;
    }
    if let Some(value) = configured.reserved_entries {
        policy.reserved_entries = value;
    }
    if let Some(value) = configured.max_share_percent {
        policy.max_share_percent = value;
    }
    if let Some(value) = configured.queue_entry_limit {
        policy.queue_entry_limit = value;
    }
    if let Some(value) = configured.max_inflight_bytes {
        policy.max_inflight_bytes = value;
    }
    if let Some(value) = configured.starvation_steps {
        policy.starvation_steps = value;
    }
    if let Some(value) = configured.allow_idle_borrow {
        policy.allow_idle_borrow = value;
    }
}

pub(crate) fn map_execution_class(value: ExecutionClassName) -> ExecutionClass {
    match value {
        ExecutionClassName::Orchestration => ExecutionClass::Orchestration,
        ExecutionClassName::Io => ExecutionClass::Io,
        ExecutionClassName::Cpu => ExecutionClass::Cpu,
        ExecutionClassName::Blocking => ExecutionClass::Blocking,
        ExecutionClassName::Script => ExecutionClass::Script,
    }
}

pub(crate) fn map_dispatch_lane(value: DispatchLaneName) -> DispatchLane {
    match value {
        DispatchLaneName::Control => DispatchLane::Control,
        DispatchLaneName::Interactive => DispatchLane::Interactive,
        DispatchLaneName::Normal => DispatchLane::Normal,
        DispatchLaneName::Background => DispatchLane::Background,
        DispatchLaneName::Bulk => DispatchLane::Bulk,
    }
}

pub(crate) fn write_runtime_lock(
    config: &ServiceConfig,
    lock: &mutsuki_runtime_contracts::RuntimeLock,
) -> ServiceRuntimeResult<()> {
    let path = config.service.run_dir.join("runtime.lock.json");
    fs::create_dir_all(&config.service.run_dir).map_err(|error| {
        ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    let bytes = serde_json::to_vec_pretty(lock).map_err(|error| {
        ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    mutsuki_service_config::write_managed_file_atomically(&path, &bytes).map_err(|error| {
        ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: error.to_string(),
        }
    })
}

pub(crate) fn restore_runtime_lock(
    path: &std::path::Path,
    previous: Option<Vec<u8>>,
) -> std::io::Result<()> {
    match previous {
        Some(bytes) => mutsuki_service_config::write_managed_file_atomically(path, &bytes)
            .map_err(std::io::Error::other),
        None if path.exists() => fs::remove_file(path),
        None => Ok(()),
    }
}

pub(crate) async fn runtime_bootstrapper(
    config: &ServiceConfig,
    catalog: &PluginCatalog,
    native_runner_factories: &[NativeRunnerFactory],
    async_handler_factories: &[AsyncHandlerFactory],
    loaded_plugin_factories: &BTreeMap<PluginId, LoadedPluginFactory>,
    runtime_client: Arc<DeferredRuntimeClient>,
) -> ServiceRuntimeResult<(RuntimeBootstrapper, RuntimeProfile)> {
    let mut bootstrapper = RuntimeBootstrapper::new();
    let mut enabled_plugins = Vec::new();
    let mut deployments = BTreeMap::new();

    for record in &catalog.records {
        if !is_bootable_record(record) {
            continue;
        }

        let deployment =
            PluginDeploymentKind::default_for_artifact(&record.manifest.artifact.artifact_type);
        deployments.insert(record.manifest.plugin_id.clone(), deployment.clone());
        enabled_plugins.push(record.manifest.plugin_id.clone());
        if matches!(
            record.manifest.artifact.artifact_type,
            mutsuki_runtime_contracts::ArtifactType::Abi
        ) {
            bootstrapper.register_loaded_plugin(
                abi_plugin::load_abi_plugin(
                    record.clone(),
                    config.clone(),
                    runtime_client.clone(),
                    configured_plugin_config(config, record.manifest.plugin_id.as_str()),
                )
                .await?,
            );
        } else if let Some(factory) = loaded_plugin_factories.get(&record.manifest.plugin_id) {
            let plugin = factory().map_err(ServiceRuntimeError::NativeRunnerFactory)?;
            if plugin.manifest.business_surface() != record.manifest.business_surface() {
                return Err(ServiceRuntimeError::PluginBusinessSurfaceMismatch {
                    plugin_id: record.manifest.plugin_id.clone(),
                });
            }
            bootstrapper.register_loaded_plugin(plugin);
        } else {
            bootstrapper.register_manifest(record.manifest.clone());
        }

        if let Some(runtime) = &record.runtime {
            register_stdio_runners(config, &mut bootstrapper, record, runtime, deployment)?;
        }
    }

    for factory in native_runner_factories {
        let runner = factory().map_err(ServiceRuntimeError::NativeRunnerFactory)?;
        let plugin_id = &runner.descriptor().plugin_id;
        if deployments.get(plugin_id) == Some(&PluginDeploymentKind::Builtin) {
            bootstrapper.register_builtin_runner(runner);
        }
    }
    for factory in async_handler_factories {
        let handler = factory().map_err(ServiceRuntimeError::NativeRunnerFactory)?;
        let plugin_id = &handler.descriptor().plugin_id;
        if deployments.get(plugin_id) == Some(&PluginDeploymentKind::Builtin) {
            bootstrapper.register_async_handler(handler);
        }
    }
    Ok((
        bootstrapper,
        RuntimeProfile {
            profile_id: config.service.profile.clone(),
            mode: RuntimeProfileMode::ExtensibleRuntime,
            enabled_plugins,
            bindings: BTreeMap::new(),
            surface_bindings: BTreeMap::new(),
            supported_extensions: Vec::new(),
            plugin_deployments: deployments,
            observability: Default::default(),
            allow_dynamic_registration: false,
            allow_hot_reload: true,
        },
    ))
}

pub(crate) fn configured_plugin_config(config: &ServiceConfig, plugin_id: &str) -> Value {
    config
        .plugins
        .configured
        .iter()
        .find(|selection| selection.enabled && selection.id.trim() == plugin_id)
        .map(|selection| selection.config.clone())
        .unwrap_or(Value::Null)
}

pub(crate) fn is_bootable_record(record: &PluginRecord) -> bool {
    record.runtime.is_none()
        || record
            .runtime
            .as_ref()
            .map(|runtime| runtime.runner_link == "binary-stdio")
            .unwrap_or(false)
}

pub(crate) fn register_stdio_runners(
    config: &ServiceConfig,
    bootstrapper: &mut RuntimeBootstrapper,
    record: &PluginRecord,
    runtime: &ExternalRuntimeSpec,
    deployment: PluginDeploymentKind,
) -> ServiceRuntimeResult<()> {
    if runtime.runner_link != "binary-stdio" {
        return Err(ServiceRuntimeError::UnsupportedRunnerLink {
            plugin_id: record.manifest.plugin_id.clone(),
            link: runtime.runner_link.clone(),
        });
    }
    for descriptor in &record.manifest.provides.runners {
        let mut extra_env = runtime.env.clone();
        extra_env.insert(
            "MUTSUKI_HOME".into(),
            config.service.home_dir.to_string_lossy().into_owned(),
        );
        extra_env.insert("MUTSUKI_RUNNER_ID".into(), descriptor.runner_id.to_string());
        extra_env.insert("MUTSUKI_PLUGIN_ID".into(), descriptor.plugin_id.to_string());
        let spec = ProcessRunnerSpec {
            command: runtime.command.clone().into(),
            args: runtime.args.clone(),
            cwd: runtime.cwd.clone(),
            env: filtered_environment(&config.runners.env_allowlist, extra_env),
        };
        let mut runner =
            SpawnedBinaryRunner::spawn(descriptor.clone(), &spec).map_err(|source| {
                ServiceRuntimeError::ExternalRunnerSpawn {
                    runner_id: descriptor.runner_id.clone(),
                    detail: source.to_string(),
                }
            })?;
        if let Some(stderr) = runner.take_stderr() {
            let runner_id = descriptor.runner_id.clone();
            std::thread::spawn(move || drain_blocking_stderr(runner_id.to_string(), stderr));
        }
        bootstrapper.register_external_runner(deployment.clone(), Box::new(runner));
    }
    Ok(())
}

pub(crate) async fn start_supervised_sidecars(
    specs: &[ManagedRunnerSpec],
    inner: &Arc<ServiceRuntimeInner>,
) {
    let graceful = Duration::from_millis(inner.config.runners.graceful_shutdown_ms);
    for spec in specs {
        if let Err(error) = inner.supervisor.start(spec.clone()).await {
            tracing::error!(error = %error, "failed to start supervised runner");
            continue;
        }
        if let Err(error) = inner.attach_sidecar_effect(spec, graceful) {
            let _ = inner.supervisor.remove(&spec.runner_id, graceful).await;
            tracing::error!(error = %error, "failed to bind supervised runner to plugin scope");
        }
    }
}

pub(crate) async fn reconcile_supervised_sidecars(
    desired: Vec<ManagedRunnerSpec>,
    supervisor: &RunnerSupervisor,
    graceful: Duration,
) -> Vec<String> {
    supervisor
        .reconcile(desired, graceful)
        .await
        .into_iter()
        .map(|error| error.to_string())
        .collect()
}
