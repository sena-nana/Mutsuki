use crate::config::{MutsukiTauriConfig, PathsConfig};
use crate::error::{HostError, HostResult};
use crate::health::HostHealthState;
use crate::host::{HostComponents, MutsukiTauriHost};
use crate::plugin_abi::DeferredPluginHost;
use crate::plugin_runner::{
    BuiltinAsyncHandlerFactory, BuiltinRunnerFactory, RuntimeClientRunnerFactory,
    declared_permission_grant_manifest, register_builtin_runtime, register_discovered_plugins,
    register_permission_grants, scan_plugin_runners,
};
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode};
use mutsuki_runtime_core::{AsyncBatchHandler, Runner};
use mutsuki_runtime_host::{HostRuntimeConfig, RuntimeBootstrapper, TokioAsyncExecutor};
use mutsuki_runtime_sdk::{RuntimeClientRef, TaskSubmitterRuntimeClient};
use mutsuki_tauri_bridge::EventHub;
use mutsuki_tauri_resource::TauriResourceStore;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct MutsukiTauriHostBuilder {
    config: MutsukiTauriConfig,
    runtime_config: HostRuntimeConfig,
    runners: Vec<Box<dyn Runner>>,
    runner_factories: Vec<BuiltinRunnerFactory>,
    runtime_client_runner_factories: Vec<RuntimeClientRunnerFactory>,
    async_handler_factories: Vec<BuiltinAsyncHandlerFactory>,
}

impl MutsukiTauriHostBuilder {
    pub fn new() -> Self {
        Self {
            config: MutsukiTauriConfig::for_app("MutsukiTauriApp"),
            runtime_config: HostRuntimeConfig {
                event_driven: true,
                ..HostRuntimeConfig::default()
            },
            runners: Vec::new(),
            runner_factories: Vec::new(),
            runtime_client_runner_factories: Vec::new(),
            async_handler_factories: Vec::new(),
        }
    }

    pub fn app_name(mut self, app_name: impl Into<String>) -> Self {
        let app_name = app_name.into();
        self.config.app_name = app_name.clone();
        self.config.paths = PathsConfig::for_app(&app_name);
        self
    }

    pub fn config(mut self, config: MutsukiTauriConfig) -> Self {
        self.config = config;
        self
    }

    pub fn runtime_config(mut self, mut runtime_config: HostRuntimeConfig) -> Self {
        runtime_config.event_driven = true;
        self.runtime_config = runtime_config;
        self
    }

    /// 注册仅用于首个 generation 的 runner；需要插件 reload 时使用 `runner_factory`。
    pub fn runner(mut self, runner: Box<dyn Runner>) -> Self {
        self.runners.push(runner);
        self
    }

    /// 注册可为每个 runtime generation 创建新实例的内建 runner。
    pub fn runner_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Box<dyn Runner> + Send + Sync + 'static,
    {
        self.runner_factories.push(Arc::new(factory));
        self
    }

    /// 注册依赖 Host runtime client、且可为每个 generation 创建新实例的内建 runner。
    pub fn runtime_client_runner_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(RuntimeClientRef) -> Box<dyn Runner> + Send + Sync + 'static,
    {
        self.runtime_client_runner_factories.push(Arc::new(factory));
        self
    }

    /// 注册可为每个 runtime generation 创建新实例的异步内建 handler。
    pub fn async_handler_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Arc<dyn AsyncBatchHandler> + Send + Sync + 'static,
    {
        self.async_handler_factories.push(Arc::new(factory));
        self
    }

    pub fn build(self) -> HostResult<MutsukiTauriHost> {
        std::fs::create_dir_all(&self.config.paths.resources_dir)
            .map_err(|error| HostError::Config(error.to_string()))?;
        std::fs::create_dir_all(&self.config.paths.logs_dir)
            .map_err(|error| HostError::Config(error.to_string()))?;
        std::fs::create_dir_all(&self.config.paths.plugins_dir)
            .map_err(|error| HostError::Config(error.to_string()))?;
        std::fs::create_dir_all(&self.config.paths.runners_dir)
            .map_err(|error| HostError::Config(error.to_string()))?;

        let resource_store = Arc::new(TauriResourceStore::new(&self.config.paths.resources_dir));
        let event_buffer = self.config.event_buffer;
        let events = Arc::new(EventHub::new(event_buffer));
        let health = Arc::new(HostHealthState::default());
        let abi_host = Arc::new(DeferredPluginHost::default());
        let runtime_client =
            TaskSubmitterRuntimeClient::new(abi_host.clone()).into_runtime_client();
        let mut loaded = scan_plugin_runners(
            &self.config,
            events.clone(),
            health.clone(),
            abi_host.clone(),
        )?;
        let mut bootstrapper = RuntimeBootstrapper::new();
        let reload_blocked_by_builtin_runners = !self.runners.is_empty();
        let mut runtime_config = self.runtime_config;
        let mut runners = self.runners;
        runners.extend(self.runner_factories.iter().map(|factory| factory()));
        runners.extend(
            self.runtime_client_runner_factories
                .iter()
                .map(|factory| factory(runtime_client.clone())),
        );
        let async_handlers = self
            .async_handler_factories
            .iter()
            .map(|factory| factory())
            .collect::<Vec<_>>();
        if !async_handlers.is_empty() && runtime_config.async_executor.is_none() {
            runtime_config.async_executor = Some(Arc::new(TokioAsyncExecutor::new(
                runtime_config.worker_threads.clamp(1, 4),
                runtime_config.pool_queue_limit,
                runtime_config.pool_queue_limit,
                runtime_config.pool_max_inflight_bytes,
            )?));
        }
        let permission_grants = declared_permission_grant_manifest(&loaded);
        let mut discovered = register_discovered_plugins(&mut loaded, &mut bootstrapper);
        register_permission_grants(permission_grants, &mut bootstrapper, &mut discovered);
        register_builtin_runtime(
            &mut loaded,
            &mut bootstrapper,
            runners,
            async_handlers,
            &mut discovered,
        )?;
        let observability = runtime_config.observability.clone().unwrap_or_default();
        let profile = RuntimeProfile {
            profile_id: self.config.profile_id.clone(),
            mode: RuntimeProfileMode::FullDev,
            enabled_plugins: discovered.enabled_plugins.iter().cloned().collect(),
            bindings: BTreeMap::new(),
            surface_bindings: BTreeMap::new(),
            supported_extensions: Vec::new(),
            plugin_deployments: discovered.plugin_deployments,
            observability: observability.clone(),
            allow_dynamic_registration: false,
            allow_hot_reload: true,
        };
        let runtime = bootstrapper.into_host_runtime_with_config(
            profile,
            runtime_config.with_resource_provider(
                mutsuki_tauri_resource::PROVIDER_ID,
                resource_store.provider(),
            ),
        )?;
        abi_host.bind(&runtime).map_err(HostError::Config)?;
        Ok(MutsukiTauriHost::new(
            self.config,
            HostComponents {
                runtime,
                resources: resource_store,
                events,
                health,
                plugins: loaded.plugins,
                runners: loaded.runners,
                packages: loaded.packages,
                active_protocols: discovered.active_protocols,
                reload_blocked_by_builtin_runners,
                builtin_runner_factories: self.runner_factories,
                runtime_client_runner_factories: self.runtime_client_runner_factories,
                async_handler_factories: self.async_handler_factories,
                runtime_client,
                observability,
                abi_host,
            },
        ))
    }
}

impl Default for MutsukiTauriHostBuilder {
    fn default() -> Self {
        Self::new()
    }
}
