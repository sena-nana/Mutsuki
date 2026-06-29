use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::{
    BridgeDescriptor, CodecDescriptor, DomainEvent, HostBackendDescriptor, PluginBackendDescriptor,
    PluginManifest, RuntimeError, RuntimeEvent, RuntimeLoadPlan, ScalarValue,
    SchedulerPolicyDescriptor, Task, TaskHandle, TaskOutcome, WorkflowDescriptor,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use serde_json::Value;

use crate::{ResourceBackend, RuntimeClient, RuntimeClientRef};

pub trait TaskSubmitter: Send + Sync {
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle>;
    fn cancel_task(&self, task_id: &str) -> RuntimeResult<()>;
    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>>;
}

#[derive(Clone)]
pub struct TaskSubmitterRuntimeClient {
    submitter: Arc<dyn TaskSubmitter>,
}

impl TaskSubmitterRuntimeClient {
    pub fn new(submitter: Arc<dyn TaskSubmitter>) -> Self {
        Self { submitter }
    }

    pub fn into_runtime_client(self) -> RuntimeClientRef {
        Arc::new(self)
    }
}

impl RuntimeClient for TaskSubmitterRuntimeClient {
    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle> {
        self.submitter.submit_task(task)
    }

    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
        self.submitter.task_outcome(task_id)
    }
}

pub trait ConfigProvider: Send + Sync {
    fn get_config(&self, scope: &str, key: &str) -> RuntimeResult<Option<Value>>;
}

#[derive(Clone, Debug, Default)]
pub struct StaticConfigProvider {
    values: Arc<BTreeMap<(String, String), Value>>,
}

impl StaticConfigProvider {
    pub fn new(values: BTreeMap<(String, String), Value>) -> Self {
        Self {
            values: Arc::new(values),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }
}

impl ConfigProvider for StaticConfigProvider {
    fn get_config(&self, scope: &str, key: &str) -> RuntimeResult<Option<Value>> {
        Ok(self.values.get(&(scope.into(), key.into())).cloned())
    }
}

pub trait EventBridge: Send + Sync {
    fn publish_runtime_event(&self, event: RuntimeEvent) -> RuntimeResult<()>;
    fn publish_domain_event(&self, event: DomainEvent) -> RuntimeResult<()>;
    fn flush(&self) -> RuntimeResult<()>;
}

#[derive(Clone, Debug, Default)]
pub struct NoopEventBridge;

impl EventBridge for NoopEventBridge {
    fn publish_runtime_event(&self, _event: RuntimeEvent) -> RuntimeResult<()> {
        Ok(())
    }

    fn publish_domain_event(&self, _event: DomainEvent) -> RuntimeResult<()> {
        Ok(())
    }

    fn flush(&self) -> RuntimeResult<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct RecordingEventBridge {
    runtime_events: Arc<Mutex<Vec<RuntimeEvent>>>,
    domain_events: Arc<Mutex<Vec<DomainEvent>>>,
    flushes: Arc<Mutex<usize>>,
}

impl RecordingEventBridge {
    pub fn runtime_events(&self) -> Vec<RuntimeEvent> {
        self.runtime_events
            .lock()
            .expect("runtime event bridge mutex poisoned")
            .clone()
    }

    pub fn domain_events(&self) -> Vec<DomainEvent> {
        self.domain_events
            .lock()
            .expect("domain event bridge mutex poisoned")
            .clone()
    }

    pub fn flush_count(&self) -> usize {
        *self
            .flushes
            .lock()
            .expect("event bridge flush mutex poisoned")
    }
}

impl EventBridge for RecordingEventBridge {
    fn publish_runtime_event(&self, event: RuntimeEvent) -> RuntimeResult<()> {
        self.runtime_events
            .lock()
            .expect("runtime event bridge mutex poisoned")
            .push(event);
        Ok(())
    }

    fn publish_domain_event(&self, event: DomainEvent) -> RuntimeResult<()> {
        self.domain_events
            .lock()
            .expect("domain event bridge mutex poisoned")
            .push(event);
        Ok(())
    }

    fn flush(&self) -> RuntimeResult<()> {
        *self
            .flushes
            .lock()
            .expect("event bridge flush mutex poisoned") += 1;
        Ok(())
    }
}

pub trait ShutdownController: Send + Sync {
    fn is_shutdown_requested(&self) -> bool;
    fn request_shutdown(&self, reason: &str) -> RuntimeResult<()>;
}

#[derive(Clone, Debug, Default)]
pub struct ManualShutdownController {
    requested: Arc<AtomicBool>,
    reasons: Arc<Mutex<Vec<String>>>,
}

impl ManualShutdownController {
    pub fn reasons(&self) -> Vec<String> {
        self.reasons
            .lock()
            .expect("shutdown reasons mutex poisoned")
            .clone()
    }
}

impl ShutdownController for ManualShutdownController {
    fn is_shutdown_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    fn request_shutdown(&self, reason: &str) -> RuntimeResult<()> {
        self.requested.store(true, Ordering::SeqCst);
        self.reasons
            .lock()
            .expect("shutdown reasons mutex poisoned")
            .push(reason.into());
        Ok(())
    }
}

pub trait HostService: Any + Send + Sync {}

impl<T> HostService for T where T: Any + Send + Sync {}

#[derive(Default)]
pub struct HostServiceRegistry {
    services: Mutex<BTreeMap<String, Arc<dyn Any + Send + Sync>>>,
    frozen: AtomicBool,
}

impl HostServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&self, service_id: impl Into<String>, service: Arc<T>) -> RuntimeResult<()>
    where
        T: HostService,
    {
        let service_id = service_id.into();
        if self.frozen.load(Ordering::SeqCst) {
            return Err(sdk_error(
                mutsuki_runtime_contracts::ERR_REGISTRY_FROZEN,
                format!("host_service.{service_id}"),
            ));
        }
        let mut services = self
            .services
            .lock()
            .expect("host service registry mutex poisoned");
        if services.contains_key(&service_id) {
            return Err(sdk_error(
                mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
                format!("host_service.duplicate.{service_id}"),
            ));
        }
        services.insert(service_id, service);
        Ok(())
    }

    pub fn require<T>(&self, service_id: &str) -> RuntimeResult<Arc<T>>
    where
        T: HostService,
    {
        let services = self
            .services
            .lock()
            .expect("host service registry mutex poisoned");
        let Some(service) = services.get(service_id) else {
            return Err(sdk_error(
                mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
                format!("host_service.missing.{service_id}"),
            ));
        };
        service.clone().downcast::<T>().map_err(|_| {
            sdk_error(
                mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
                format!("host_service.type_mismatch.{service_id}"),
            )
        })
    }

    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen.load(Ordering::SeqCst)
    }
}

pub trait CapabilityBroker: Send + Sync {
    fn require_capability(&self, capability: &str) -> RuntimeResult<()>;
    fn require_host_backend(&self, backend_id: &str) -> RuntimeResult<HostBackendDescriptor>;
    fn require_plugin_backend(&self, backend_id: &str) -> RuntimeResult<PluginBackendDescriptor>;
    fn require_codec(&self, codec_id: &str) -> RuntimeResult<CodecDescriptor>;
    fn require_bridge(&self, bridge_id: &str) -> RuntimeResult<BridgeDescriptor>;
    fn require_scheduler_policy(&self, policy_id: &str)
    -> RuntimeResult<SchedulerPolicyDescriptor>;
    fn require_workflow(&self, workflow_id: &str) -> RuntimeResult<WorkflowDescriptor>;
}

#[derive(Clone, Debug, Default)]
pub struct StaticCapabilityBroker {
    active_capabilities: Arc<BTreeSet<String>>,
    provided_capabilities: Arc<BTreeSet<String>>,
    host_backends: Arc<BTreeMap<String, HostBackendDescriptor>>,
    plugin_backends: Arc<BTreeMap<String, PluginBackendDescriptor>>,
    codecs: Arc<BTreeMap<String, CodecDescriptor>>,
    bridges: Arc<BTreeMap<String, BridgeDescriptor>>,
    scheduler_policies: Arc<BTreeMap<String, SchedulerPolicyDescriptor>>,
    workflows: Arc<BTreeMap<String, WorkflowDescriptor>>,
}

impl StaticCapabilityBroker {
    pub fn from_load_plan(plan: &RuntimeLoadPlan) -> Self {
        let active = &plan.capability_graph;
        Self {
            active_capabilities: Arc::new(active.active_capabilities.iter().cloned().collect()),
            provided_capabilities: Arc::new(active.provided_capabilities.iter().cloned().collect()),
            host_backends: Arc::new(active_descriptors(
                &plan.plugins,
                &active.active_host_backends,
                |manifest| &manifest.provides.host_backends,
                |descriptor| descriptor.backend_id.as_str(),
            )),
            plugin_backends: Arc::new(active_descriptors(
                &plan.plugins,
                &active.active_plugin_backends,
                |manifest| &manifest.provides.plugin_backends,
                |descriptor| descriptor.backend_id.as_str(),
            )),
            codecs: Arc::new(active_descriptors(
                &plan.plugins,
                &active.active_codecs,
                |manifest| &manifest.provides.codecs,
                |descriptor| descriptor.codec_id.as_str(),
            )),
            bridges: Arc::new(active_descriptors(
                &plan.plugins,
                &active.active_bridges,
                |manifest| &manifest.provides.bridges,
                |descriptor| descriptor.bridge_id.as_str(),
            )),
            scheduler_policies: Arc::new(active_descriptors(
                &plan.plugins,
                &active.active_scheduler_policies,
                |manifest| &manifest.provides.scheduler_policies,
                |descriptor| descriptor.policy_id.as_str(),
            )),
            workflows: Arc::new(active_descriptors(
                &plan.plugins,
                &active.active_workflows,
                |manifest| &manifest.provides.workflows,
                |descriptor| descriptor.workflow_id.as_str(),
            )),
        }
    }

    pub fn with_active_capability(mut self, capability: impl Into<String>) -> Self {
        let mut active = (*self.active_capabilities).clone();
        active.insert(capability.into());
        self.active_capabilities = Arc::new(active);
        self
    }

    fn require_descriptor<T: Clone>(
        &self,
        prefix: &str,
        id: &str,
        entries: &BTreeMap<String, T>,
    ) -> RuntimeResult<T> {
        entries
            .get(id)
            .cloned()
            .ok_or_else(|| self.capability_error(&format!("{prefix}:{id}")))
    }

    fn capability_error(&self, capability: &str) -> RuntimeFailure {
        let mut error = RuntimeError::new(
            mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
            "runtime.sdk",
            format!("capability.{capability}"),
        );
        error
            .evidence
            .insert("capability".into(), ScalarValue::String(capability.into()));
        if self.provided_capabilities.contains(capability) {
            error.evidence.insert(
                "detail".into(),
                ScalarValue::String("inactive_load_plan".into()),
            );
        } else {
            error.evidence.insert(
                "detail".into(),
                ScalarValue::String("provider_missing".into()),
            );
        }
        RuntimeFailure::new(error)
    }
}

impl CapabilityBroker for StaticCapabilityBroker {
    fn require_capability(&self, capability: &str) -> RuntimeResult<()> {
        if self.active_capabilities.contains(capability) {
            Ok(())
        } else {
            Err(self.capability_error(capability))
        }
    }

    fn require_host_backend(&self, backend_id: &str) -> RuntimeResult<HostBackendDescriptor> {
        self.require_descriptor("host_backend", backend_id, &self.host_backends)
    }

    fn require_plugin_backend(&self, backend_id: &str) -> RuntimeResult<PluginBackendDescriptor> {
        self.require_descriptor("plugin_backend", backend_id, &self.plugin_backends)
    }

    fn require_codec(&self, codec_id: &str) -> RuntimeResult<CodecDescriptor> {
        self.require_descriptor("codec", codec_id, &self.codecs)
    }

    fn require_bridge(&self, bridge_id: &str) -> RuntimeResult<BridgeDescriptor> {
        self.require_descriptor("bridge", bridge_id, &self.bridges)
    }

    fn require_scheduler_policy(
        &self,
        policy_id: &str,
    ) -> RuntimeResult<SchedulerPolicyDescriptor> {
        self.require_descriptor("scheduler_policy", policy_id, &self.scheduler_policies)
    }

    fn require_workflow(&self, workflow_id: &str) -> RuntimeResult<WorkflowDescriptor> {
        self.require_descriptor("workflow", workflow_id, &self.workflows)
    }
}

#[derive(Clone)]
pub struct HostContext {
    runtime_id: String,
    profile_id: String,
    registry_generation: u64,
    capability_broker: Arc<dyn CapabilityBroker>,
    services: Arc<HostServiceRegistry>,
    config: Arc<dyn ConfigProvider>,
    events: Arc<dyn EventBridge>,
    task_submitter: Arc<dyn TaskSubmitter>,
    resource_backend: Arc<dyn ResourceBackend>,
    shutdown: Arc<dyn ShutdownController>,
}

impl HostContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runtime_id: impl Into<String>,
        profile_id: impl Into<String>,
        registry_generation: u64,
        capability_broker: Arc<dyn CapabilityBroker>,
        services: Arc<HostServiceRegistry>,
        config: Arc<dyn ConfigProvider>,
        events: Arc<dyn EventBridge>,
        task_submitter: Arc<dyn TaskSubmitter>,
        resource_backend: Arc<dyn ResourceBackend>,
        shutdown: Arc<dyn ShutdownController>,
    ) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            profile_id: profile_id.into(),
            registry_generation,
            capability_broker,
            services,
            config,
            events,
            task_submitter,
            resource_backend,
            shutdown,
        }
    }

    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn registry_generation(&self) -> u64 {
        self.registry_generation
    }

    pub fn capability_broker(&self) -> &dyn CapabilityBroker {
        self.capability_broker.as_ref()
    }

    pub fn services(&self) -> &HostServiceRegistry {
        &self.services
    }

    pub fn config(&self) -> &dyn ConfigProvider {
        self.config.as_ref()
    }

    pub fn events(&self) -> &dyn EventBridge {
        self.events.as_ref()
    }

    pub fn task_submitter(&self) -> &dyn TaskSubmitter {
        self.task_submitter.as_ref()
    }

    pub fn task_submitter_ref(&self) -> Arc<dyn TaskSubmitter> {
        self.task_submitter.clone()
    }

    pub fn resource_backend(&self) -> &dyn ResourceBackend {
        self.resource_backend.as_ref()
    }

    pub fn shutdown(&self) -> &dyn ShutdownController {
        self.shutdown.as_ref()
    }
}

pub trait HostRuntime {
    fn host_context(&self) -> &HostContext;

    fn submit_task(&self, task: Task) -> RuntimeResult<TaskHandle> {
        self.host_context().task_submitter().submit_task(task)
    }

    fn cancel_task(&self, task_id: &str) -> RuntimeResult<()> {
        self.host_context().task_submitter().cancel_task(task_id)
    }

    fn task_outcome(&self, task_id: &str) -> RuntimeResult<Option<TaskOutcome>> {
        self.host_context().task_submitter().task_outcome(task_id)
    }

    fn request_shutdown(&self, reason: &str) -> RuntimeResult<()> {
        self.host_context().shutdown().request_shutdown(reason)
    }
}

fn active_descriptors<T, D, I>(
    manifests: &[PluginManifest],
    active_ids: &[String],
    descriptors: D,
    id: I,
) -> BTreeMap<String, T>
where
    T: Clone,
    D: for<'a> Fn(&'a PluginManifest) -> &'a [T],
    I: Fn(&T) -> &str,
{
    let active_ids: BTreeSet<_> = active_ids.iter().map(String::as_str).collect();
    let mut entries = BTreeMap::new();
    for manifest in manifests {
        for descriptor in descriptors(manifest) {
            let descriptor_id = id(descriptor);
            if active_ids.contains(descriptor_id) {
                entries.insert(descriptor_id.to_string(), descriptor.clone());
            }
        }
    }
    entries
}

fn sdk_error(code: &'static str, route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(code, "runtime.sdk", route))
}
