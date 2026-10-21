//! Owner-provided component registry: configured factories, runner limits,
//! load-plan hooks and observers captured before boot and swapped on reconfigure.

#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Clone, Debug)]
pub(crate) struct RunnerLimitOverride {
    pub(crate) runner_id: RunnerId,
    pub(crate) max_running: Option<usize>,
    pub(crate) wall_clock_timeout_ms: Option<u64>,
}

pub(crate) type RunnerLimitFactory = Arc<dyn Fn() -> RunnerLimitOverride + Send + Sync>;

#[derive(Default)]
pub(crate) struct PreparedComponent {
    pub(crate) manifests: BTreeMap<PluginId, mutsuki_runtime_contracts::PluginManifest>,
    pub(crate) native_runner_factories: Vec<NativeRunnerFactory>,
    pub(crate) async_handler_factories: Vec<AsyncHandlerFactory>,
    pub(crate) loaded_plugin_factories: BTreeMap<PluginId, LoadedPluginFactory>,
    pub(crate) health_probe: Option<(String, HealthProbe)>,
    pub(crate) event_sources: Mutex<Vec<Box<dyn HostEventSource>>>,
    pub(crate) runner_limits: BTreeMap<RunnerId, RunnerLimits>,
    pub(crate) runner_limit_factories: Vec<RunnerLimitFactory>,
    pub(crate) load_plan_hooks: Vec<Arc<dyn LoadPlanLifecycleHook>>,
    pub(crate) load_plan_observers: Vec<Arc<dyn LoadPlanObserver>>,
}

impl Clone for PreparedComponent {
    fn clone(&self) -> Self {
        Self {
            manifests: self.manifests.clone(),
            native_runner_factories: self.native_runner_factories.clone(),
            async_handler_factories: self.async_handler_factories.clone(),
            loaded_plugin_factories: self.loaded_plugin_factories.clone(),
            health_probe: self.health_probe.clone(),
            event_sources: Mutex::new(Vec::new()),
            runner_limits: self.runner_limits.clone(),
            runner_limit_factories: self.runner_limit_factories.clone(),
            load_plan_hooks: self.load_plan_hooks.clone(),
            load_plan_observers: self.load_plan_observers.clone(),
        }
    }
}

/// Prepared ServiceHost assembly records. A component's manifest, executable factories,
/// health/event surfaces and limit policy now travel through one owner instead of parallel lists.
#[derive(Clone, Default)]
pub(crate) struct PreparedComponentRegistry {
    pub(crate) records: BTreeMap<String, PreparedComponent>,
    pub(crate) sequence: u64,
}

impl PreparedComponentRegistry {
    pub(crate) fn record(&mut self, component_id: impl Into<String>) -> &mut PreparedComponent {
        self.records.entry(component_id.into()).or_default()
    }

    pub(crate) fn anonymous(&mut self, kind: &str) -> &mut PreparedComponent {
        self.sequence = self.sequence.saturating_add(1);
        self.record(format!("{kind}:{}", self.sequence))
    }

    #[cfg(test)]
    pub(crate) fn register_manifest(
        &mut self,
        manifest: mutsuki_runtime_contracts::PluginManifest,
    ) {
        let plugin_id = manifest.plugin_id.clone();
        self.record(plugin_id.clone())
            .manifests
            .insert(plugin_id, manifest);
    }

    pub(crate) fn builtin_registry(&self) -> BuiltinRegistry {
        let mut registry = BuiltinRegistry::new();
        for manifest in self
            .records
            .values()
            .flat_map(|component| component.manifests.values().cloned())
        {
            registry.register_manifest(manifest);
        }
        registry
    }

    pub(crate) fn native_runner_factories(&self) -> Vec<NativeRunnerFactory> {
        self.records
            .values()
            .flat_map(|component| component.native_runner_factories.iter().cloned())
            .collect()
    }

    pub(crate) fn async_handler_factories(&self) -> Vec<AsyncHandlerFactory> {
        self.records
            .values()
            .flat_map(|component| component.async_handler_factories.iter().cloned())
            .collect()
    }

    pub(crate) fn loaded_plugin_factories(&self) -> BTreeMap<PluginId, LoadedPluginFactory> {
        self.records
            .values()
            .flat_map(|component| component.loaded_plugin_factories.clone())
            .collect()
    }

    pub(crate) fn health_probes(&self) -> BTreeMap<String, HealthProbe> {
        self.records
            .values()
            .filter_map(|component| component.health_probe.clone())
            .collect()
    }

    pub(crate) fn runner_limits(&self) -> BTreeMap<RunnerId, RunnerLimits> {
        let mut limits = BTreeMap::new();
        for (runner_id, registered) in self
            .records
            .values()
            .flat_map(|component| component.runner_limits.iter())
        {
            merge_runner_limits(limits.entry(runner_id.clone()).or_default(), registered);
        }
        limits
    }

    pub(crate) fn runner_limit_factories(&self) -> Vec<RunnerLimitFactory> {
        self.records
            .values()
            .flat_map(|component| component.runner_limit_factories.iter().cloned())
            .collect()
    }

    pub(crate) fn load_plan_hooks(&self) -> Vec<Arc<dyn LoadPlanLifecycleHook>> {
        self.records
            .values()
            .flat_map(|component| component.load_plan_hooks.iter().cloned())
            .collect()
    }

    pub(crate) fn owned_load_plan_hooks(&self) -> Vec<(String, Arc<dyn LoadPlanLifecycleHook>)> {
        self.records
            .iter()
            .flat_map(|(component_id, component)| {
                component
                    .load_plan_hooks
                    .iter()
                    .cloned()
                    .map(|hook| (component_id.clone(), hook))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn load_plan_observers(&self) -> Vec<Arc<dyn LoadPlanObserver>> {
        self.records
            .values()
            .flat_map(|component| component.load_plan_observers.iter().cloned())
            .collect()
    }

    pub(crate) fn take_event_sources(&self) -> Vec<Box<dyn HostEventSource>> {
        self.records
            .values()
            .flat_map(|component| {
                component
                    .event_sources
                    .lock()
                    .expect("prepared event source mutex")
                    .drain(..)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn replace_component(
        &mut self,
        component_id: &str,
        candidate: Option<PreparedComponent>,
    ) {
        match candidate {
            Some(candidate) => {
                self.records.insert(component_id.to_string(), candidate);
            }
            None => {
                self.records.remove(component_id);
            }
        }
    }
}

pub(crate) fn merge_runner_limits(current: &mut RunnerLimits, registered: &RunnerLimits) {
    current.max_running = current.max_running.min(registered.max_running.max(1));
    if let Some(timeout) = registered.wall_clock_deadline {
        current.wall_clock_deadline = Some(
            current
                .wall_clock_deadline
                .map_or(timeout, |existing| existing.min(timeout)),
        );
    }
}
