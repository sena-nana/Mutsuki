// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::assigning_clones,
    clippy::borrow_as_ptr,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_else,
    clippy::return_self_not_must_use,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unnecessary_literal_bound,
    clippy::useless_conversion
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::future::Future;
#[cfg(test)]
use std::io::Write;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
use mutsuki_runtime_contracts::TaskBatch;
use mutsuki_runtime_contracts::{
    CancelPolicy, DispatchLane, ERR_RUNTIME_HOST_FAILED, ExecutionClass, PluginDeploymentKind,
    PluginId, RunnerId, RuntimeError, RuntimeLoadPlan, RuntimeProfile, RuntimeProfileMode,
    SurfaceCompatibility, Task, TaskHandle, TaskId, TaskOutcome, TaskStatus,
};
#[cfg(test)]
use mutsuki_runtime_host::BinaryRunner;
use mutsuki_runtime_host::DeferredHostClient;
use mutsuki_runtime_host::{
    AttachedEffectHandle, ExecutionDomainConfig, HostRuntime, HostRuntimeCommand,
    HostRuntimeConfig, HostRuntimeReply, HostTaskSnapshot, HostTaskState, LaneExecutionPolicy,
    ProcessRunnerSpec, RunnerLimits, RuntimeBootstrapper, SpawnedBinaryRunner, TokioAsyncExecutor,
};
use mutsuki_runtime_plugin_loader::{
    BuiltinRegistry, ExternalRuntimeSpec, PluginCatalog, PluginInventory, PluginLoaderError,
    PluginRecord,
};
#[cfg(test)]
use mutsuki_runtime_sdk::RuntimeBootstrapperService;
#[cfg(test)]
use mutsuki_runtime_sdk::RuntimeResult;
use mutsuki_runtime_sdk::{AsyncBatchHandler, Runner, RuntimeFailure, RuntimeStopState};
use mutsuki_runtime_sdk::{
    HostEffect, HostEffectFuture, HostEffectKind, HostService, LoadedPlugin,
    ResourceRegistryGateway, RuntimeClientRef, TaskSubmitter,
};
use mutsuki_service_config::{
    ConfiguredPluginSelection, DispatchLaneName, ExecutionClassName, HostSecretStore,
    LanePolicySection, ServiceConfig, filtered_environment,
};
use mutsuki_service_control::{
    ControlChangeDomain, ControlChangeEvent, ControlChangeSubscription, ControlCommand,
    ControlError, ControlFuture, ControlHandler, ControlRequest, ControlResponse, ControlResult,
    CoreDrainResponse, CoreRuntimeMetrics, CoreStatus, ExecutionDomainMetrics,
    ExecutionLaneMetrics, HealthReport, HostMetrics, IdParam, LogTailEntry, LogTailParams,
    LogTailResponse, PluginCandidateStatus, PluginDeploymentClearParam, PluginDeploymentParam,
    PluginInventoryDiagnostic, PluginListResponse, PluginReloadChange, PluginReloadResponse,
    PluginStatus, RunnerStatus as ControlRunnerStatus, RuntimeStatisticsView, ServiceStatus,
    TaskEventPage, TaskEventsAfterParam, TaskFailureSummary as ControlTaskFailureSummary,
    TaskOutcomeView, TaskOutcomesBatchParam, TaskOutcomesBatchResponse, TaskPoolStatisticsView,
    TaskSnapshot as ControlTaskSnapshot, TaskSubmitBatchParam, TaskSubmitBatchResponse,
    TaskWaitParam, TaskWaitResponse,
};
use mutsuki_service_ipc::IpcServer;
use mutsuki_service_link::LinkControlServer;
use mutsuki_service_runner_supervisor::{
    ManagedRunnerSpec, RestartPolicy, RunnerProcessState, RunnerSnapshot, RunnerSupervisor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

/// Names this Host in structured errors raised before the runtime client is bound.
const SERVICE_RUNTIME_SOURCE: &str = "mutsuki.service.runtime";

mod abi_plugin;
mod application_services;
mod boot;
mod catalog;
mod components;
mod control_views;
mod event_source;
mod plugin_control;
mod process_metrics;
mod runner_control;
mod scoped_effects;
mod status_control;
mod sync_bridge;
mod task_control;

pub use application_services::{ApplicationServiceDirectory, RunnerFactoryContext};
use application_services::{ApplicationServiceTable, BUILTIN_COMPONENT};
#[allow(clippy::wildcard_imports)]
use boot::*;
#[allow(clippy::wildcard_imports)]
use catalog::*;
#[allow(clippy::wildcard_imports)]
use components::*;
#[allow(clippy::wildcard_imports)]
use control_views::*;
use event_source::EventSourceSupervisor;
pub use event_source::{
    HostEventSource, HostEventSourceConfig, HostEventSourceContext, HostEventSourceDescriptor,
    HostEventSourceError, HostEventSourceFuture, HostEventSourceHealth, HostEventSourceLogger,
    HostShutdownToken,
};
use process_metrics::current_rss_bytes;
pub use sync_bridge::block_on_host_future;

type NativeRunnerFactory = Arc<dyn Fn() -> Result<Box<dyn Runner>, String> + Send + Sync>;
type AsyncHandlerFactory =
    Arc<dyn Fn() -> Result<Arc<dyn AsyncBatchHandler>, String> + Send + Sync>;
type LoadedPluginFactory = Arc<dyn Fn() -> Result<LoadedPlugin, String> + Send + Sync>;
type HealthProbe = Arc<dyn Fn() -> Value + Send + Sync>;

/// Domain-neutral final LoadPlan lifecycle gate.
///
/// Owner packages may validate immutable domain state against the fully resolved plan and update
/// an already-validated snapshot after activation. ServiceHost never interprets owner data.
pub trait LoadPlanLifecycleHook: Send + Sync {
    fn validate(&self, plan: &RuntimeLoadPlan) -> Result<(), String>;
    fn activate(&self, plan: &RuntimeLoadPlan);
    fn deactivate(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Domain-neutral observer for state derived from the complete RuntimeLoadPlan.
///
/// Unlike an owner lifecycle hook, an observer is validated and committed for every successful
/// registry generation, even when its owning component was not replaced. This keeps global
/// projections such as a Flow node catalog aligned without recreating unrelated owner services.
pub trait LoadPlanObserver: Send + Sync {
    fn validate(&self, plan: &RuntimeLoadPlan) -> Result<(), String>;
    fn activate(&self, plan: &RuntimeLoadPlan);
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceRuntimeError {
    #[error(transparent)]
    Config(#[from] mutsuki_service_config::ConfigError),
    #[error(transparent)]
    Plugin(#[from] PluginLoaderError),
    #[error(transparent)]
    Core(#[from] RuntimeFailure),
    #[error(transparent)]
    Ipc(#[from] mutsuki_service_ipc::IpcError),
    #[error("link control bridge failed: {0}")]
    LinkControl(#[from] mutsuki_service_link::LinkControlServerError),
    #[error("external runner link {link} for plugin {plugin_id} is not supported")]
    UnsupportedRunnerLink { plugin_id: PluginId, link: String },
    #[error("external runner {runner_id} failed to start: {detail}")]
    ExternalRunnerSpawn { runner_id: RunnerId, detail: String },
    #[error("service runtime already started")]
    AlreadyStarted,
    #[error("service runtime is not available")]
    RuntimeUnavailable,
    #[error("event source registration failed: {0}")]
    EventSource(String),
    #[error("final LoadPlan validation failed: {0}")]
    LoadPlanHook(String),
    #[error("native runner factory failed: {0}")]
    NativeRunnerFactory(String),
    #[error("configured plugin id must not be empty")]
    EmptyConfiguredPluginId,
    #[error("configured plugin {0} is selected more than once")]
    DuplicateConfiguredPlugin(PluginId),
    #[error("configured plugin factory is not registered: {0}")]
    UnknownConfiguredPlugin(PluginId),
    #[error("configured plugin {plugin_id} contains raw credential field {field}")]
    RawConfiguredPluginSecret { plugin_id: PluginId, field: String },
    #[error("configured plugin {plugin_id} failed to install: {detail}")]
    ConfiguredPluginInstall { plugin_id: PluginId, detail: String },
    #[error("ABI plugin {plugin_id} failed to load: {detail}")]
    AbiPlugin { plugin_id: PluginId, detail: String },
    #[error("configured plugin {plugin_id} has no available artifact: {detail}")]
    PluginUnavailable { plugin_id: PluginId, detail: String },
    #[error("configured plugin {plugin_id} has multiple deployments and requires a Host selection")]
    PluginDeploymentAmbiguous { plugin_id: PluginId },
    #[error("configured plugin {plugin_id} deployment {deployment:?} is unavailable: {detail}")]
    PluginDeploymentUnavailable {
        plugin_id: PluginId,
        deployment: PluginDeploymentKind,
        detail: String,
    },
    #[error(
        "plugin {plugin_id} builtin and selected deployment expose different business surfaces"
    )]
    PluginBusinessSurfaceMismatch { plugin_id: PluginId },
    #[error("failed to read or write plugin deployment state {path}: {detail}")]
    PluginDeploymentState { path: String, detail: String },
    #[error(
        "application service {service_id} is provided by both {first_owner} and {second_owner}"
    )]
    DuplicateApplicationService {
        service_id: String,
        first_owner: String,
        second_owner: String,
    },
    #[error("component {component_id} requires unavailable application service {service_id}")]
    MissingApplicationService {
        component_id: String,
        service_id: String,
    },
}

pub type ServiceRuntimeResult<T> = Result<T, ServiceRuntimeError>;

pub struct ServiceRuntime {
    inner: Arc<ServiceRuntimeInner>,
    shutdown_rx: Option<oneshot::Receiver<String>>,
    ipc_server: Option<IpcServer>,
    link_control_server: Option<LinkControlServer>,
    _observe: mutsuki_service_observe::ObserveGuard,
}

#[derive(Clone)]
pub struct ServiceRuntimeHandle {
    inner: Arc<ServiceRuntimeInner>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginReconfigureGeneration {
    pub plugin_id: String,
    pub previous_generation: u64,
    pub generation: u64,
    pub rolled_back: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginReconfigureReport {
    pub components: Vec<PluginReconfigureGeneration>,
    pub registry_generation: u64,
    pub runner_errors: Vec<String>,
}

/// Product assembly boundary. All manifests, native runners and event sources are frozen at boot.
pub struct ServiceRuntimeBuilder {
    config: ServiceConfig,
    configured_plugins: ConfiguredPluginCatalog,
    components: PreparedComponentRegistry,
    runtime_client: Arc<DeferredHostClient>,
    application_services: ApplicationServiceDirectory,
    current_component: Option<String>,
}

/// Domain-neutral factory for a native product plugin selected by Host configuration.
///
/// Domain repositories own config decoding and installation. ServiceHost only resolves the
/// selection before the runtime profile and load plan are frozen.
pub trait ConfiguredPluginFactory: Send + Sync {
    fn plugin_id(&self) -> &str;

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String>;
}

#[derive(Clone, Default)]
pub struct ConfiguredPluginCatalog {
    factories: BTreeMap<String, Arc<dyn ConfiguredPluginFactory>>,
}

impl ConfiguredPluginCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, factory: F) -> ServiceRuntimeResult<()>
    where
        F: ConfiguredPluginFactory + 'static,
    {
        let plugin_id = factory.plugin_id().trim();
        if plugin_id.is_empty() {
            return Err(ServiceRuntimeError::EmptyConfiguredPluginId);
        }
        if self.factories.contains_key(plugin_id) {
            return Err(ServiceRuntimeError::DuplicateConfiguredPlugin(
                PluginId::from(plugin_id),
            ));
        }
        self.factories.insert(plugin_id.into(), Arc::new(factory));
        Ok(())
    }

    /// Atomically merges another owner catalog and rejects any duplicate plugin id.
    pub fn merge(&mut self, other: Self) -> ServiceRuntimeResult<()> {
        if let Some(plugin_id) = other
            .factories
            .keys()
            .find(|plugin_id| self.factories.contains_key(*plugin_id))
        {
            return Err(ServiceRuntimeError::DuplicateConfiguredPlugin(
                PluginId::from(plugin_id.as_str()),
            ));
        }
        self.factories.extend(other.factories);
        Ok(())
    }

    fn factory(&self, plugin_id: &str) -> Option<Arc<dyn ConfiguredPluginFactory>> {
        self.factories.get(plugin_id).cloned()
    }
}

struct ServiceRuntimeInner {
    config: ServiceConfig,
    configured_selections: Mutex<Vec<ConfiguredPluginSelection>>,
    started_at: Instant,
    catalog: Mutex<PluginCatalog>,
    host_runtime: Mutex<Option<HostRuntime>>,
    supervisor: RunnerSupervisor,
    sidecar_effects: Mutex<BTreeMap<String, (String, AttachedEffectHandle)>>,
    load_plan_hook_effects: Mutex<BTreeMap<String, Vec<AttachedEffectHandle>>>,
    event_sources: EventSourceSupervisor,
    event_source_effects: Mutex<BTreeMap<String, (String, AttachedEffectHandle)>>,
    configured_plugins: ConfiguredPluginCatalog,
    components: Mutex<PreparedComponentRegistry>,
    component_generations: Mutex<BTreeMap<String, u64>>,
    reload_lock: tokio::sync::Mutex<()>,
    runtime_client: Arc<DeferredHostClient>,
    application_services: ApplicationServiceDirectory,
    deployment_state: Mutex<PluginDeploymentState>,
    shutdown_tx: Mutex<Option<oneshot::Sender<String>>>,
    control_changes: Arc<ControlChangeHub>,
}

struct ControlChangeHub {
    revision: AtomicU64,
    sender: tokio::sync::broadcast::Sender<ControlChangeEvent>,
}

impl Default for ControlChangeHub {
    fn default() -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(128);
        Self {
            revision: AtomicU64::new(0),
            sender,
        }
    }
}

impl ControlChangeHub {
    fn publish(&self, domains: impl IntoIterator<Item = ControlChangeDomain>) {
        let domains = domains.into_iter().collect::<BTreeSet<_>>();
        if domains.is_empty() {
            return;
        }
        let event = ControlChangeEvent {
            revision: self
                .revision
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1),
            domains: domains.into_iter().collect(),
        };
        let _ = self.sender.send(event);
    }

    fn subscribe(&self) -> ControlChangeSubscription {
        ControlChangeSubscription::new(self.sender.subscribe())
    }
}

struct EventSourceScopeEffect {
    supervisor: EventSourceSupervisor,
    source_id: String,
    graceful: Duration,
}

struct RunnerSupervisorScopeEffect {
    supervisor: RunnerSupervisor,
    runner_id: String,
    graceful: Duration,
}

struct LoadPlanHookScopeEffect(Arc<dyn LoadPlanLifecycleHook>);

impl HostEffect for LoadPlanHookScopeEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        let hook = self.0.clone();
        Box::pin(async move {
            hook.deactivate().map_err(|detail| {
                RuntimeFailure::new(RuntimeError::new(
                    ERR_RUNTIME_HOST_FAILED,
                    "service.load_plan_hook",
                    detail,
                ))
            })
        })
    }
}

impl HostEffect for RunnerSupervisorScopeEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        Box::pin(async move {
            self.supervisor
                .remove(&self.runner_id, self.graceful)
                .await
                .map_err(|error| {
                    RuntimeFailure::new(RuntimeError::new(
                        ERR_RUNTIME_HOST_FAILED,
                        "service.runner_supervisor",
                        error.to_string(),
                    ))
                })
        })
    }
}

impl HostEffect for EventSourceScopeEffect {
    fn dispose(&mut self) -> HostEffectFuture<'_> {
        Box::pin(async move {
            self.supervisor
                .shutdown_source(&self.source_id, self.graceful)
                .await
                .map_err(|detail| {
                    RuntimeFailure::new(RuntimeError::new(
                        ERR_RUNTIME_HOST_FAILED,
                        "service.event_source",
                        detail,
                    ))
                })
        })
    }
}

impl ServiceRuntime {
    pub async fn start(config: ServiceConfig) -> ServiceRuntimeResult<Self> {
        ServiceRuntimeBuilder::new(config).start().await
    }

    pub fn handle(&self) -> ServiceRuntimeHandle {
        ServiceRuntimeHandle {
            inner: self.inner.clone(),
        }
    }

    pub async fn reconfigure_plugins(
        &self,
        candidates: &[ConfiguredPluginSelection],
    ) -> ServiceRuntimeResult<PluginReconfigureReport> {
        self.inner.reconfigure_plugins(candidates).await
    }
}

impl ServiceRuntimeHandle {
    pub async fn reconfigure_plugins(
        &self,
        candidates: &[ConfiguredPluginSelection],
    ) -> ServiceRuntimeResult<PluginReconfigureReport> {
        self.inner.reconfigure_plugins(candidates).await
    }

    /// Returns the current owner selection used by generation-aware reconfiguration.
    #[must_use]
    pub fn configured_plugin_selection(
        &self,
        plugin_id: &str,
    ) -> Option<ConfiguredPluginSelection> {
        self.inner
            .configured_selections
            .lock()
            .expect("configured selections mutex")
            .iter()
            .find(|selection| selection.id == plugin_id)
            .cloned()
    }

    /// Live view of application services; every call observes the committed generation.
    pub fn application_services(&self) -> ApplicationServiceDirectory {
        self.inner.application_services.clone()
    }

    /// Returns the complete configured selection snapshot for product-owned diff compilation.
    #[must_use]
    pub fn configured_plugin_selections(&self) -> Vec<ConfiguredPluginSelection> {
        self.inner
            .configured_selections
            .lock()
            .expect("configured selections mutex")
            .clone()
    }

    pub fn host_service<T>(&self, service_id: &str) -> ServiceRuntimeResult<Arc<T>>
    where
        T: HostService,
    {
        let runtime = self.inner.host_runtime.lock().expect("host runtime mutex");
        let runtime = runtime
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?;
        Ok(runtime.host_context().services().require(service_id)?)
    }
}

impl Drop for ServiceRuntime {
    fn drop(&mut self) {
        if let Some(server) = self.ipc_server.take() {
            server.abort();
        }
        self.inner.event_sources.abort();
    }
}

impl ServiceRuntimeBuilder {
    pub fn new(config: ServiceConfig) -> Self {
        Self {
            config,
            configured_plugins: ConfiguredPluginCatalog::new(),
            components: PreparedComponentRegistry::default(),
            runtime_client: Arc::new(DeferredHostClient::new(SERVICE_RUNTIME_SOURCE)),
            application_services: ApplicationServiceDirectory::default(),
            current_component: None,
        }
    }

    fn owner_component(&mut self) -> &mut PreparedComponent {
        let component_id = self
            .current_component
            .clone()
            .unwrap_or_else(|| BUILTIN_COMPONENT.to_string());
        self.components.record(component_id)
    }

    /// Publishes an application-scoped service owned by the current component.
    ///
    /// The service lives until its owner component is reconfigured or removed; consumers in other
    /// components declare it with [`Self::require_application_service`] and resolve it lazily.
    pub fn register_application_service<T>(
        mut self,
        service_id: impl Into<String>,
        service: Arc<T>,
    ) -> Self
    where
        T: HostService,
    {
        self.owner_component().application_services.insert(
            service_id.into(),
            mutsuki_runtime_sdk::HostServiceValue::new(service),
        );
        self
    }

    /// Declares that the current component consumes an application service. Boot fails when it
    /// is missing, and reconfiguring its provider co-reloads this component.
    pub fn require_application_service(mut self, service_id: impl Into<String>) -> Self {
        self.owner_component()
            .required_application_services
            .insert(service_id.into());
        self
    }

    /// Context for owner objects (event sources, health probes) that resolve services at start.
    pub fn factory_context(&self) -> RunnerFactoryContext {
        RunnerFactoryContext::new(
            self.current_component
                .clone()
                .unwrap_or_else(|| BUILTIN_COMPONENT.to_string()),
            self.runtime_client.clone(),
            self.application_services.clone(),
        )
    }

    /// Registers a recreatable runner built from the scoped factory context.
    pub fn register_scoped_runner<F, E>(mut self, factory: F) -> Self
    where
        F: Fn(&RunnerFactoryContext) -> Result<Box<dyn Runner>, E> + Send + Sync + 'static,
        E: std::fmt::Display,
    {
        let context = self.factory_context();
        self.active_component("native-runner")
            .native_runner_factories
            .push(Arc::new(move || {
                factory(&context).map_err(|error| error.to_string())
            }));
        self
    }

    /// Registers a recreatable Host-driven async handler built from the scoped factory context.
    pub fn register_scoped_async_handler<F, E>(mut self, factory: F) -> Self
    where
        F: Fn(&RunnerFactoryContext) -> Result<Arc<dyn AsyncBatchHandler>, E>
            + Send
            + Sync
            + 'static,
        E: std::fmt::Display,
    {
        let context = self.factory_context();
        self.active_component("async-handler")
            .async_handler_factories
            .push(Arc::new(move || {
                factory(&context).map_err(|error| error.to_string())
            }));
        self
    }

    /// Registers a recreatable loaded plugin built from the scoped factory context.
    pub fn register_scoped_loaded_plugin_factory<F, E>(
        mut self,
        manifest: mutsuki_runtime_contracts::PluginManifest,
        factory: F,
    ) -> Self
    where
        F: Fn(&RunnerFactoryContext) -> Result<LoadedPlugin, E> + Send + Sync + 'static,
        E: std::fmt::Display,
    {
        let plugin_id = manifest.plugin_id.clone();
        let context = self.factory_context();
        let component = self.active_component("loaded-plugin");
        component.manifests.insert(plugin_id.clone(), manifest);
        component.loaded_plugin_factories.insert(
            plugin_id,
            Arc::new(move || factory(&context).map_err(|error| error.to_string())),
        );
        self
    }

    fn active_component(&mut self, kind: &str) -> &mut PreparedComponent {
        if let Some(component_id) = self.current_component.clone() {
            self.components.record(component_id)
        } else {
            self.components.anonymous(kind)
        }
    }

    pub fn with_configured_plugin_catalog(mut self, catalog: ConfiguredPluginCatalog) -> Self {
        self.configured_plugins = catalog;
        self
    }

    /// Product-data root used by owner plugins for their private durable state.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.config.service.data_dir
    }

    /// Host-owned mutable secret boundary for integration crates that rotate credentials.
    pub fn host_secret_store(&self) -> HostSecretStore {
        self.config.host_secret_store()
    }

    /// Host-owned persistence boundary for owner-defined configured plugin data.
    /// Reads one owner-defined configured selection during boot-time catalog composition.
    /// Domain factories may use this only to combine declared plugin contributions before the
    /// load plan is frozen.
    pub fn configured_plugin_selection(
        &self,
        plugin_id: &str,
    ) -> Option<&ConfiguredPluginSelection> {
        self.config
            .plugins
            .configured
            .iter()
            .find(|selection| selection.id == plugin_id && selection.enabled)
    }

    /// Registers and enables a product-provided builtin manifest before the load plan is built.
    pub fn register_builtin_plugin(
        mut self,
        manifest: mutsuki_runtime_contracts::PluginManifest,
    ) -> Self {
        let plugin_id = manifest.plugin_id.clone();
        self.active_component("builtin-plugin")
            .manifests
            .insert(plugin_id, manifest);
        self
    }

    /// Registers a recreatable native runner factory for initial boot and every Core reload.
    pub fn register_builtin_runner<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Box<dyn Runner> + Send + Sync + 'static,
    {
        self.active_component("native-runner")
            .native_runner_factories
            .push(Arc::new(move || Ok(factory())));
        self
    }

    /// Registers a recreatable native runner whose external client or other
    /// fallible dependency is initialized at boot and every Core reload.
    pub fn register_fallible_builtin_runner<F, E>(mut self, factory: F) -> Self
    where
        F: Fn() -> Result<Box<dyn Runner>, E> + Send + Sync + 'static,
        E: std::fmt::Display,
    {
        self.active_component("native-runner")
            .native_runner_factories
            .push(Arc::new(move || {
                factory().map_err(|error| error.to_string())
            }));
        self
    }

    /// Registers a recreatable Host-driven async handler for boot and reload.
    pub fn register_builtin_async_handler<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Arc<dyn AsyncBatchHandler> + Send + Sync + 'static,
    {
        self.active_component("async-handler")
            .async_handler_factories
            .push(Arc::new(move || Ok(factory())));
        self
    }

    /// Registers a fallible Host-driven async handler factory for boot and reload.
    pub fn register_fallible_builtin_async_handler<F, E>(mut self, factory: F) -> Self
    where
        F: Fn() -> Result<Arc<dyn AsyncBatchHandler>, E> + Send + Sync + 'static,
        E: std::fmt::Display,
    {
        self.active_component("async-handler")
            .async_handler_factories
            .push(Arc::new(move || {
                factory().map_err(|error| error.to_string())
            }));
        self
    }

    /// Registers a Host-driven async handler that uses the booted runtime resource services.
    pub fn register_fallible_runtime_services_async_handler<F, E>(mut self, factory: F) -> Self
    where
        F: Fn(
                RuntimeClientRef,
                Arc<dyn ResourceRegistryGateway>,
            ) -> Result<Arc<dyn AsyncBatchHandler>, E>
            + Send
            + Sync
            + 'static,
        E: std::fmt::Display,
    {
        let services = self.runtime_client.clone();
        self.active_component("async-handler")
            .async_handler_factories
            .push(Arc::new(move || {
                factory(services.clone(), services.clone()).map_err(|error| error.to_string())
            }));
        self
    }

    /// Registers a recreatable runner that needs the booted runtime client for nested calls.
    pub fn register_runtime_client_runner<F>(mut self, factory: F) -> Self
    where
        F: Fn(RuntimeClientRef) -> Box<dyn Runner> + Send + Sync + 'static,
    {
        let client = self.runtime_client.clone();
        self.active_component("native-runner")
            .native_runner_factories
            .push(Arc::new(move || Ok(factory(client.clone()))));
        self
    }

    /// Registers a recreatable runner with both nested-task and host resource services.
    pub fn register_runtime_services_runner<F>(mut self, factory: F) -> Self
    where
        F: Fn(RuntimeClientRef, Arc<dyn ResourceRegistryGateway>) -> Box<dyn Runner>
            + Send
            + Sync
            + 'static,
    {
        let services = self.runtime_client.clone();
        self.active_component("native-runner")
            .native_runner_factories
            .push(Arc::new(move || {
                Ok(factory(services.clone(), services.clone()))
            }));
        self
    }

    /// Registers a recreatable runtime-services runner with fallible artifact or client setup.
    pub fn register_fallible_runtime_services_runner<F, E>(mut self, factory: F) -> Self
    where
        F: Fn(RuntimeClientRef, Arc<dyn ResourceRegistryGateway>) -> Result<Box<dyn Runner>, E>
            + Send
            + Sync
            + 'static,
        E: std::fmt::Display,
    {
        let services = self.runtime_client.clone();
        self.active_component("native-runner")
            .native_runner_factories
            .push(Arc::new(move || {
                factory(services.clone(), services.clone()).map_err(|error| error.to_string())
            }));
        self
    }

    /// Registers a builtin plugin that must be reconstructed at boot and reload.
    pub fn register_builtin_loaded_plugin_factory<F, E>(
        mut self,
        manifest: mutsuki_runtime_contracts::PluginManifest,
        factory: F,
    ) -> Self
    where
        F: Fn() -> Result<LoadedPlugin, E> + Send + Sync + 'static,
        E: std::fmt::Display,
    {
        let plugin_id = manifest.plugin_id.clone();
        let component = self.active_component("loaded-plugin");
        component.manifests.insert(plugin_id.clone(), manifest);
        component.loaded_plugin_factories.insert(
            plugin_id,
            Arc::new(move || factory().map_err(|error| error.to_string())),
        );
        self
    }

    /// Registers a recreatable loaded plugin whose Host services need the booted runtime client.
    ///
    /// The deferred client is stable across targeted component replacements and is bound before
    /// any registered Host service can be invoked. This keeps owner management services on the
    /// same runtime generation without exposing Core internals through product assembly code.
    pub fn register_runtime_client_loaded_plugin_factory<F, E>(
        mut self,
        manifest: mutsuki_runtime_contracts::PluginManifest,
        factory: F,
    ) -> Self
    where
        F: Fn(RuntimeClientRef) -> Result<LoadedPlugin, E> + Send + Sync + 'static,
        E: std::fmt::Display,
    {
        let plugin_id = manifest.plugin_id.clone();
        let client = self.runtime_client.clone();
        let component = self.active_component("loaded-plugin");
        component.manifests.insert(plugin_id.clone(), manifest);
        component.loaded_plugin_factories.insert(
            plugin_id,
            Arc::new(move || factory(client.clone()).map_err(|error| error.to_string())),
        );
        self
    }

    /// Registers a domain-neutral product component snapshot for `health`.
    pub fn register_health_probe<F>(mut self, component_id: impl Into<String>, probe: F) -> Self
    where
        F: Fn() -> Value + Send + Sync + 'static,
    {
        let component_id = component_id.into();
        let record_id = self
            .current_component
            .clone()
            .unwrap_or_else(|| component_id.clone());
        let component = self.components.record(record_id);
        component.health_probe = Some((component_id, Arc::new(probe)));
        self
    }

    pub fn register_event_source(mut self, source: Box<dyn HostEventSource>) -> Self {
        let component_id = self
            .current_component
            .clone()
            .unwrap_or_else(|| source.descriptor().source_id.clone());
        self.components
            .record(component_id)
            .event_sources
            .get_mut()
            .expect("prepared event source mutex")
            .push(source);
        self
    }

    /// Registers a final resolved-plan validation/activation observer.
    pub fn register_load_plan_hook(
        mut self,
        component_id: impl Into<String>,
        hook: Arc<dyn LoadPlanLifecycleHook>,
    ) -> Self {
        let component_id = self
            .current_component
            .clone()
            .unwrap_or_else(|| component_id.into());
        self.components
            .record(component_id)
            .load_plan_hooks
            .push(hook);
        self
    }

    /// Registers an observer whose projection depends on the complete resolved LoadPlan.
    pub fn register_load_plan_observer(
        mut self,
        component_id: impl Into<String>,
        observer: Arc<dyn LoadPlanObserver>,
    ) -> Self {
        let component_id = self
            .current_component
            .clone()
            .unwrap_or_else(|| component_id.into());
        self.components
            .record(component_id)
            .load_plan_observers
            .push(observer);
        self
    }

    /// Narrows Host-owned execution limits for a logical runner.
    ///
    /// Repeated registrations keep the strictest non-empty value, which lets multiple public
    /// handlers share a runner without weakening one another's safety boundary.
    pub fn configure_runner_limits(
        mut self,
        runner_id: impl Into<RunnerId>,
        max_running: Option<usize>,
        wall_clock_timeout_ms: Option<u64>,
    ) -> Self {
        let runner_id = runner_id.into();
        let component_id = self
            .current_component
            .clone()
            .unwrap_or_else(|| format!("runner:{runner_id}"));
        let limits = self
            .components
            .record(component_id)
            .runner_limits
            .entry(runner_id)
            .or_default();
        if let Some(max_running) = max_running {
            limits.max_running = limits.max_running.min(max_running.max(1));
        }
        if let Some(timeout) = wall_clock_timeout_ms.map(Duration::from_millis) {
            limits.wall_clock_deadline = Some(
                limits
                    .wall_clock_deadline
                    .map_or(timeout, |current| current.min(timeout)),
            );
        }
        self
    }

    /// Registers a runner limit whose values are resolved again for every ServiceHost reload.
    ///
    /// Product configuration owns the source of these values; ServiceHost remains responsible
    /// for applying the resulting scheduler boundary atomically with the runtime generation.
    pub fn register_dynamic_runner_limit<F>(
        mut self,
        runner_id: impl Into<RunnerId>,
        factory: F,
    ) -> Self
    where
        F: Fn() -> (Option<usize>, Option<u64>) + Send + Sync + 'static,
    {
        let runner_id = runner_id.into();
        let component_id = self
            .current_component
            .clone()
            .unwrap_or_else(|| format!("runner:{runner_id}"));
        self.components
            .record(component_id)
            .runner_limit_factories
            .push(Arc::new(move || {
                let (max_running, wall_clock_timeout_ms) = factory();
                RunnerLimitOverride {
                    runner_id: runner_id.clone(),
                    max_running,
                    wall_clock_timeout_ms,
                }
            }));
        self
    }

    pub async fn start(self) -> ServiceRuntimeResult<ServiceRuntime> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let self_ = self.install_configured_plugins()?;
        let ServiceRuntimeBuilder {
            config,
            configured_plugins,
            components,
            runtime_client,
            application_services,
            current_component: _,
        } = self_;
        application_services.replace(ApplicationServiceTable::from_components(&components)?);
        let builtin_registry = components.builtin_registry();
        let native_runner_factories = components.native_runner_factories();
        let async_handler_factories = components.async_handler_factories();
        let loaded_plugin_factories = components.loaded_plugin_factories();
        let configured_runner_limits = components.runner_limits();
        let runner_limit_factories = components.runner_limit_factories();
        let load_plan_hooks = components.load_plan_hooks();
        let owned_load_plan_hooks = components.owned_load_plan_hooks();
        let load_plan_observers = components.load_plan_observers();
        let event_sources = components.take_event_sources();
        let runner_limits =
            resolve_runner_limits(&configured_runner_limits, &runner_limit_factories);
        validate_event_sources(&event_sources, &config)?;
        let control_changes = Arc::new(ControlChangeHub::default());
        let log_changes = control_changes.clone();
        let observe = mutsuki_service_observe::init_observe_with_listener(
            &config,
            Arc::new(move || log_changes.publish([ControlChangeDomain::Logs])),
        );
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let runner_changes = control_changes.clone();
        let supervisor = RunnerSupervisor::with_change_listener(Arc::new(move || {
            runner_changes.publish([ControlChangeDomain::Runners]);
        }));
        let event_source_changes = control_changes.clone();
        let event_source_supervisor = EventSourceSupervisor::new(Arc::new(move || {
            event_source_changes.publish([ControlChangeDomain::EventSources]);
        }));
        let deployment_state = load_deployment_state(&config)?;
        let catalog = load_catalog_with_state(&config, &builtin_registry, &deployment_state)?;
        let (host_runtime, runtime_lock) = boot_core(
            &config,
            &catalog,
            runtime_client.clone(),
            CoreBootInputs {
                native_runner_factories: &native_runner_factories,
                async_handler_factories: &async_handler_factories,
                loaded_plugin_factories: &loaded_plugin_factories,
                runner_limits: &runner_limits,
                load_plan_hooks: &load_plan_hooks,
                load_plan_observers: &load_plan_observers,
            },
        )
        .await?;
        let task_change_subscription = host_runtime.subscribe_task_changes();
        for observer in &load_plan_observers {
            observer.activate(&runtime_lock);
        }
        runtime_client.bind(host_runtime.host_context())?;
        let task_submitter = host_runtime.host_context().task_submitter_ref();
        let sidecars = sidecar_specs(&config, &catalog);

        let inner = Arc::new(ServiceRuntimeInner {
            config: config.clone(),
            configured_selections: Mutex::new(config.plugins.configured.clone()),
            started_at: Instant::now(),
            catalog: Mutex::new(catalog),
            host_runtime: Mutex::new(Some(host_runtime)),
            supervisor,
            sidecar_effects: Mutex::new(BTreeMap::new()),
            load_plan_hook_effects: Mutex::new(BTreeMap::new()),
            event_sources: event_source_supervisor.clone(),
            event_source_effects: Mutex::new(BTreeMap::new()),
            configured_plugins,
            component_generations: Mutex::new(
                config
                    .plugins
                    .configured
                    .iter()
                    .filter(|selection| selection.enabled)
                    .map(|selection| (selection.id.clone(), 1))
                    .collect(),
            ),
            components: Mutex::new(components),
            reload_lock: tokio::sync::Mutex::new(()),
            runtime_client,
            application_services,
            deployment_state: Mutex::new(deployment_state),
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            control_changes,
        });
        start_task_change_forwarder(&inner, task_change_subscription);
        for (component_id, hook) in owned_load_plan_hooks {
            inner.attach_load_plan_hook_effect(&component_id, hook.clone())?;
            hook.activate(&runtime_lock);
        }
        start_supervised_sidecars(&sidecars, &inner).await;
        let ipc_server = mutsuki_service_ipc::start_server(
            &inner.config,
            Arc::new(RuntimeControl {
                inner: inner.clone(),
            }),
        )
        .await?;
        let control_handler: Arc<dyn ControlHandler> = Arc::new(RuntimeControl {
            inner: inner.clone(),
        });
        let link_control_server = if inner.config.ipc.enabled {
            Some(LinkControlServer::start(
                &inner.config.service.run_dir,
                &inner.config.service.instance_id,
                control_handler.clone(),
            )?)
        } else {
            None
        };
        let graceful = Duration::from_millis(config.runners.graceful_shutdown_ms);
        for source in event_sources {
            inner
                .start_scoped_event_source(source, task_submitter.clone(), &config, graceful)
                .await?;
        }
        Ok(ServiceRuntime {
            inner,
            shutdown_rx: Some(shutdown_rx),
            ipc_server,
            link_control_server,
            _observe: observe,
        })
    }

    fn install_configured_plugins(mut self) -> ServiceRuntimeResult<Self> {
        let selections = self
            .config
            .plugins
            .configured
            .iter()
            .filter(|selection| selection.enabled)
            .cloned()
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        let mut resolved = Vec::with_capacity(selections.len());
        for selection in selections {
            let plugin_id = selection.id.trim();
            if plugin_id.is_empty() {
                return Err(ServiceRuntimeError::EmptyConfiguredPluginId);
            }
            if !seen.insert(plugin_id.to_owned()) {
                return Err(ServiceRuntimeError::DuplicateConfiguredPlugin(
                    PluginId::from(plugin_id),
                ));
            }
            if let Some(field) = raw_credential_field(&selection.config, "") {
                return Err(ServiceRuntimeError::RawConfiguredPluginSecret {
                    plugin_id: PluginId::from(plugin_id),
                    field,
                });
            }
            if let Some(factory) = self.configured_plugins.factory(plugin_id) {
                resolved.push((selection, factory));
            }
        }
        for (selection, factory) in resolved {
            let plugin_id = selection.id.clone();
            self.current_component = Some(plugin_id.clone());
            self = factory.prepare(&selection.config, self).map_err(|detail| {
                ServiceRuntimeError::ConfiguredPluginInstall {
                    plugin_id: PluginId::from(plugin_id),
                    detail,
                }
            })?;
            self.current_component = None;
        }
        Ok(self)
    }
}

fn resolve_runner_limits(
    configured: &BTreeMap<RunnerId, RunnerLimits>,
    factories: &[RunnerLimitFactory],
) -> BTreeMap<RunnerId, RunnerLimits> {
    let mut limits = configured.clone();
    for factory in factories {
        let override_ = factory();
        let current = limits.entry(override_.runner_id).or_default();
        if let Some(max_running) = override_.max_running {
            current.max_running = current.max_running.min(max_running.max(1));
        }
        if let Some(timeout_ms) = override_.wall_clock_timeout_ms {
            let timeout = Duration::from_millis(timeout_ms.max(1));
            current.wall_clock_deadline = Some(
                current
                    .wall_clock_deadline
                    .map_or(timeout, |existing| existing.min(timeout)),
            );
        }
    }
    limits
}

fn raw_credential_field(value: &Value, path: &str) -> Option<String> {
    match value {
        Value::Object(object) => object.iter().find_map(|(key, value)| {
            let field = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            let normalized = key.to_ascii_lowercase().replace('-', "_");
            let is_reference = normalized.ends_with("_key") || normalized.ends_with("_ref");
            let is_credential = matches!(
                normalized.as_str(),
                "secret" | "client_secret" | "token" | "access_token" | "password" | "api_key"
            );
            if is_credential && !is_reference && !value.is_null() {
                Some(field)
            } else {
                raw_credential_field(value, &field)
            }
        }),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .find_map(|(index, value)| raw_credential_field(value, &format!("{path}[{index}]"))),
        _ => None,
    }
}

fn validate_event_sources(
    sources: &[Box<dyn HostEventSource>],
    config: &ServiceConfig,
) -> ServiceRuntimeResult<()> {
    let mut ids = BTreeSet::new();
    let source_config = HostEventSourceConfig::from_service(config);
    for source in sources {
        let descriptor = source.descriptor();
        if descriptor.source_id.trim().is_empty()
            || descriptor.plugin_id.trim().is_empty()
            || descriptor.instance_id.trim().is_empty()
        {
            return Err(ServiceRuntimeError::EventSource(
                "source id, plugin id and instance id must not be empty".into(),
            ));
        }
        if !ids.insert(descriptor.source_id.clone()) {
            return Err(ServiceRuntimeError::EventSource(format!(
                "duplicate event source id {}",
                descriptor.source_id
            )));
        }
        let mut secret_keys = BTreeSet::new();
        for key in &descriptor.required_secrets {
            if key.trim().is_empty() || !secret_keys.insert(key) {
                return Err(ServiceRuntimeError::EventSource(format!(
                    "event source {} must declare non-empty unique required secret keys",
                    descriptor.source_id
                )));
            }
            if !source_config.contains_secret(key) {
                return Err(ServiceRuntimeError::EventSource(format!(
                    "event source {} requires missing Host secret {}",
                    descriptor.source_id, key
                )));
            }
        }
    }
    Ok(())
}

impl ServiceRuntime {
    /// Submits work through the same Host-owned TaskPool used by control-plane clients.
    pub fn submit_task(&self, task: Task) -> ServiceRuntimeResult<TaskHandle> {
        let submitter = self
            .inner
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .host_context()
            .task_submitter_ref();
        Ok(submitter.submit_task(task)?)
    }

    /// Reads the authoritative TaskPool outcome without exposing runtime internals.
    pub fn task_outcome(&self, handle: &TaskHandle) -> ServiceRuntimeResult<Option<TaskOutcome>> {
        let submitter = self
            .inner
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .host_context()
            .task_submitter_ref();
        Ok(submitter.task_outcome(handle)?)
    }

    /// Requests cancellation through the Host so cascade and runner cancellation stay intact.
    pub fn cancel_task(&self, handle: &TaskHandle) -> ServiceRuntimeResult<()> {
        let submitter = self
            .inner
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .host_context()
            .task_submitter_ref();
        Ok(submitter.cancel_task(handle)?)
    }

    /// Returns immutable task snapshots for embedded diagnostics and acceptance harnesses.
    pub fn task_snapshots(&self) -> ServiceRuntimeResult<Vec<HostTaskSnapshot>> {
        let runtime = self.inner.host_runtime.lock().expect("host runtime mutex");
        let runtime = runtime
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?;
        Ok(runtime.task_snapshots()?)
    }

    pub async fn run_foreground(self) -> ServiceRuntimeResult<()> {
        self.run_until_shutdown_signal(platform_shutdown_signal())
            .await
    }

    pub async fn wait_for_shutdown_request(&mut self) -> ServiceRuntimeResult<()> {
        self.wait_for_shutdown_request_with(platform_shutdown_signal())
            .await
    }

    pub async fn wait_for_shutdown_request_with<F>(
        &mut self,
        shutdown_signal: F,
    ) -> ServiceRuntimeResult<()>
    where
        F: Future<Output = String>,
    {
        let shutdown_rx = self
            .shutdown_rx
            .take()
            .ok_or(ServiceRuntimeError::AlreadyStarted)?;
        tokio::pin!(shutdown_signal);
        tokio::select! {
            reason = shutdown_rx => {
                tracing::info!(reason = ?reason, "service shutdown requested");
            }
            reason = &mut shutdown_signal => {
                tracing::info!(reason, "service shutdown signal received");
            }
        }
        Ok(())
    }

    pub async fn run_until_shutdown_signal<F>(
        mut self,
        shutdown_signal: F,
    ) -> ServiceRuntimeResult<()>
    where
        F: Future<Output = String>,
    {
        self.wait_for_shutdown_request_with(shutdown_signal).await?;
        self.shutdown().await;
        Ok(())
    }

    /// Stops the service in dependency order: stop producing work, let in-flight work finish,
    /// then tear down from the outside in, with Core last.
    ///
    /// The ordering matters because everything else in the service holds a handle into Core.
    /// Dropping Core first would abandon in-flight tasks, leave sidecars running against a
    /// runtime that no longer exists, and let EventSources submit into a closed channel.
    pub async fn shutdown(mut self) {
        let graceful = Duration::from_millis(self.inner.config.runners.graceful_shutdown_ms);

        // 1. Tell Core to stop accepting new work while the existing work is still allowed to
        //    finish. This has to happen before the ingress shutdowns so that anything they flush
        //    on their way out is rejected rather than half-executed.
        let drain_started = {
            let runtime = self.inner.host_runtime.lock().expect("host runtime mutex");
            match runtime.as_ref() {
                Some(runtime) => match runtime.begin_drain() {
                    Ok(state) => {
                        tracing::info!(state = ?state, "core drain started");
                        true
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "failed to begin core drain");
                        false
                    }
                },
                None => false,
            }
        };

        // 2. Wait for in-flight tasks, bounded by the same budget used for runner shutdown.
        if drain_started {
            self.await_core_drain(graceful);
        }

        // 3. Close ingress. No new control requests or upstream events can arrive after this.
        drop(self.link_control_server.take());
        if let Some(server) = self.ipc_server.take() {
            server.shutdown().await;
        }
        self.inner.event_sources.shutdown(graceful).await;

        // 4. Stop the Runner processes while Core is still alive, so their in-flight batches can
        //    still report completions instead of dying against a dropped runtime.
        self.inner.supervisor.shutdown(graceful).await;

        // 5. Core last.
        let host_runtime = self
            .inner
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .take();
        if let Some(host_runtime) = host_runtime {
            // Tokio runtimes perform a blocking shutdown when dropped. ServiceRuntime itself is
            // async, so release the Host-owned executor outside the caller's async context.
            let _ = tokio::task::spawn_blocking(move || drop(host_runtime)).await;
        }
    }

    /// Waits (event-driven on task completions) until no task is running or
    /// ready, or the budget expires.
    ///
    /// A timeout is not an error: the remaining work is abandoned deliberately, which is what
    /// the `graceful_shutdown_ms` budget means. It is logged so an operator can tell a clean
    /// stop from a truncated one.
    fn await_core_drain(&self, budget: Duration) {
        let outstanding = run_blocking_on_worker(|| {
            let runtime = self.inner.host_runtime.lock().expect("host runtime mutex");
            // Core gone or unreachable: there is nothing left to wait for.
            runtime
                .as_ref()
                .map_or(0, |runtime| runtime.wait_until_idle(budget).unwrap_or(0))
        });
        if outstanding > 0 {
            tracing::warn!(
                outstanding,
                budget_ms = budget.as_millis(),
                "core drain budget expired with tasks still outstanding"
            );
        }
    }

    /// Shared control-plane handler for in-process consumers (e.g. Web overview extension).
    pub fn control_handler(&self) -> Arc<dyn ControlHandler> {
        Arc::new(RuntimeControl {
            inner: self.inner.clone(),
        })
    }

    pub fn subscribe_control_changes(&self) -> ControlChangeSubscription {
        self.inner.control_changes.subscribe()
    }

    pub fn control_token(&self) -> &str {
        self.inner.config.control_token()
    }

    /// Resolves a Host service from this Runtime's frozen service registry.
    pub fn host_service<T>(&self, service_id: &str) -> ServiceRuntimeResult<Arc<T>>
    where
        T: HostService,
    {
        let runtime = self.inner.host_runtime.lock().expect("host runtime mutex");
        let runtime = runtime
            .as_ref()
            .ok_or(ServiceRuntimeError::AlreadyStarted)?;
        Ok(runtime.host_context().services().require(service_id)?)
    }
}

async fn ctrl_c_signal() -> String {
    match tokio::signal::ctrl_c().await {
        Ok(()) => "ctrl-c".to_string(),
        Err(error) => {
            tracing::warn!(error = %error, "failed to listen for ctrl-c");
            "ctrl-c-listener-error".to_string()
        }
    }
}

fn start_task_change_forwarder(
    runtime: &Arc<ServiceRuntimeInner>,
    subscription: mutsuki_runtime_host::TaskChangeSubscription,
) {
    let runtime = Arc::downgrade(runtime);
    let _ = std::thread::Builder::new()
        .name("mutsuki-control-task-changes".into())
        .spawn(move || {
            let mut revision = subscription.revision();
            while let Some(next) = subscription.wait_after(revision) {
                revision = next;
                let Some(runtime) = runtime.upgrade() else {
                    break;
                };
                runtime
                    .control_changes
                    .publish([ControlChangeDomain::Tasks]);
            }
        });
}

#[cfg(unix)]
async fn platform_shutdown_signal() -> String {
    use tokio::signal::unix::{SignalKind, signal};

    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        tracing::warn!("failed to listen for SIGTERM; falling back to ctrl-c");
        return ctrl_c_signal().await;
    };
    tokio::select! {
        reason = ctrl_c_signal() => reason,
        _ = terminate.recv() => "sigterm".to_string(),
    }
}

#[cfg(not(unix))]
async fn platform_shutdown_signal() -> String {
    ctrl_c_signal().await
}

struct RuntimeControl {
    inner: Arc<ServiceRuntimeInner>,
}

impl ControlHandler for RuntimeControl {
    fn handle(&self, request: ControlRequest) -> ControlFuture {
        let inner = self.inner.clone();
        Box::pin(async move { inner.handle_request(request).await })
    }
}

impl ServiceRuntimeInner {
    async fn handle_request(&self, request: ControlRequest) -> ControlResponse {
        if !tokens_match(&request.token, self.config.control_token()) {
            return ControlResponse::err(ControlError::Unauthorized);
        }
        let change_domain = match &request.command {
            ControlCommand::PluginReload
            | ControlCommand::PluginDeploymentSet(_)
            | ControlCommand::PluginDeploymentClear(_) => Some(ControlChangeDomain::Plugins),
            ControlCommand::RunnerRestart(_) | ControlCommand::RunnerStop(_) => {
                Some(ControlChangeDomain::Runners)
            }
            ControlCommand::EventSourceRestart(_) => Some(ControlChangeDomain::EventSources),
            ControlCommand::TaskSubmitBatch(_) | ControlCommand::TaskCancel(_) => {
                Some(ControlChangeDomain::Tasks)
            }
            _ => None,
        };
        let response = match request.command {
            ControlCommand::ServiceStatus => self.service_status().await,
            ControlCommand::ServiceShutdown => self.service_shutdown(),
            ControlCommand::CoreStatus => self.core_status(),
            ControlCommand::PluginList => self.plugin_list(),
            ControlCommand::PluginReload => self.plugin_reload().await,
            ControlCommand::PluginDeploymentSet(param) => self.plugin_deployment_set(param).await,
            ControlCommand::PluginDeploymentClear(param) => {
                self.plugin_deployment_clear(param).await
            }
            ControlCommand::RunnerList => self.runner_list().await,
            ControlCommand::RunnerRestart(param) => self.runner_restart(param).await,
            ControlCommand::RunnerStop(param) => self.runner_stop(param).await,
            ControlCommand::EventSourceList => self.event_source_list(),
            ControlCommand::EventSourceRestart(param) => self.event_source_restart(param).await,
            ControlCommand::CoreBeginDrain => self.core_begin_drain(),
            ControlCommand::TaskSubmitBatch(param) => self.task_submit_batch(param),
            ControlCommand::TaskList => self.task_list(),
            ControlCommand::TaskCancel(param) => self.task_cancel(param),
            ControlCommand::TaskOutcome(param) => self.task_outcome(param),
            ControlCommand::TaskOutcomesBatch(param) => self.task_outcomes_batch(param),
            ControlCommand::TaskWait(param) => self.task_wait(param).await,
            ControlCommand::TaskEventsAfter(param) => self.task_events_after(param),
            ControlCommand::HealthCheck => self.health_check().await,
            ControlCommand::LogTail(param) => self.log_tail(param),
            ControlCommand::RuntimeStatistics => self.runtime_statistics(),
            ControlCommand::HostMetrics => self.host_metrics(),
        };
        if matches!(response, ControlResponse::Ok(_))
            && let Some(domain) = change_domain
        {
            self.control_changes.publish([domain]);
        }
        response
    }
}

fn read_log_tail(
    path: impl AsRef<std::path::Path>,
    params: LogTailParams,
) -> Result<LogTailResponse, ControlError> {
    if !params.filters.is_empty() {
        return Err(ControlError::BadRequest(
            "log_tail filters are not supported by this runtime".into(),
        ));
    }

    let path = path.as_ref();
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(LogTailResponse {
            cursor: 0,
            entries: Vec::new(),
        });
    };
    let len = metadata.len();
    let start = params.cursor.filter(|cursor| *cursor <= len).unwrap_or(0);
    let file =
        std::fs::File::open(path).map_err(|error| ControlError::Failed(error.to_string()))?;
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(start))
        .map_err(|error| ControlError::Failed(error.to_string()))?;

    let max_lines = params.lines.unwrap_or(100);
    let mut entries = Vec::new();
    let mut cursor = start;
    loop {
        let offset = cursor;
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|error| ControlError::Failed(error.to_string()))?;
        if bytes == 0 {
            break;
        }
        cursor += bytes as u64;
        entries.push(LogTailEntry {
            offset,
            line: line.trim_end_matches(['\r', '\n']).to_string(),
        });
    }
    if entries.len() > max_lines {
        entries.drain(0..entries.len() - max_lines);
    }

    Ok(LogTailResponse { cursor, entries })
}

/// Compares a presented token against the control token without leaking the shared prefix length
/// through timing. The control socket lets an unauthenticated local caller retry indefinitely, so
/// the byte-by-byte early exit of `==` is an oracle for recovering the token one byte at a time.
fn tokens_match(presented: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

fn sidecar_specs(config: &ServiceConfig, catalog: &PluginCatalog) -> Vec<ManagedRunnerSpec> {
    catalog
        .external_records()
        .filter_map(|record| {
            let runtime = record.resolved_runtime()?;
            if runtime.runner_link == "binary-stdio" && !record.manifest.provides.runners.is_empty()
            {
                return None;
            }
            let runner_id = record
                .manifest
                .provides
                .runners
                .first()
                .map(|runner| runner.runner_id.to_string())
                .unwrap_or_else(|| format!("sidecar:{}", record.manifest.plugin_id));
            Some(ManagedRunnerSpec {
                runner_id,
                plugin_id: record.manifest.plugin_id.to_string(),
                runtime,
                env_allowlist: config.runners.env_allowlist.clone(),
                service_home: config.service.home_dir.clone(),
                restart: RestartPolicy::from_config(&config.runners),
            })
        })
        .collect()
}

fn reload_drain_timeout(config: &ServiceConfig, catalog: &PluginCatalog) -> Duration {
    let max_plugin_timeout = catalog
        .records
        .iter()
        .map(|record| record.manifest.lifecycle.unload_timeout_ms)
        .max()
        .unwrap_or(0);
    Duration::from_millis(config.runners.graceful_shutdown_ms.max(max_plugin_timeout))
}

fn duration_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn current_cpu_time_ms() -> Option<u64> {
    process_metrics::process_cpu_time_ms()
}

fn drain_blocking_stderr(runner_id: String, stderr: std::process::ChildStderr) {
    let reader = BufReader::new(stderr);
    for line in reader.lines().map_while(Result::ok) {
        tracing::warn!(runner_id, stream = "stderr", line);
    }
}

/// Client settings for the control endpoint of a locally configured service.
/// Lives with the server-side config owner: the public IPC client only takes
/// transport, endpoint and token and never sees `ServiceConfig`.
pub fn local_control_client_config(
    config: &ServiceConfig,
) -> mutsuki_service_ipc::ControlClientConfig {
    mutsuki_service_ipc::ControlClientConfig::new(
        config.ipc.transport.clone(),
        config.ipc_endpoint(),
        config.control_token(),
    )
    .with_profile(mutsuki_service_ipc::ControlIpcProfile::from_config(config))
}

/// Runs a synchronous, potentially long Host call (reload drain, bounded
/// waits) without parking an async runtime worker. On a multi-thread runtime
/// the worker hands its other tasks off for the duration; elsewhere (tests on a
/// current-thread runtime, or no runtime) the call simply runs inline.
fn run_blocking_on_worker<T>(call: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(call)
        }
        _ => call(),
    }
}

/// One-line operator summary of a structured Host lifecycle diagnostic.
fn runtime_error_summary(error: &mutsuki_runtime_contracts::RuntimeError) -> String {
    match error.evidence.get("detail") {
        Some(mutsuki_runtime_contracts::ScalarValue::String(detail)) => {
            format!("{}: {detail}", error.route)
        }
        _ => format!("{} ({})", error.route, error.code),
    }
}

#[cfg(test)]
mod tests;
