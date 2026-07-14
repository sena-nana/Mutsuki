use crate::config::{MutsukiTauriConfig, PathsConfig};
use crate::error::{HostError, HostResult};
use crate::health::HostHealthState;
use crate::host::MutsukiTauriHost;
use crate::plugin_runner::{
    loaded_builtin_plugin_summary, loaded_builtin_runner_summary, scan_plugin_runners,
};
use mutsuki_runtime_contracts::{PluginDeploymentKind, RuntimeProfile, RuntimeProfileMode};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_host::{HostRuntimeConfig, RuntimeBootstrapper, runner_manifest};
use mutsuki_tauri_bridge::EventHub;
use mutsuki_tauri_resource::TauriResourceStore;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub struct MutsukiTauriHostBuilder {
    config: MutsukiTauriConfig,
    runtime_config: HostRuntimeConfig,
    runners: Vec<Box<dyn Runner>>,
}

impl MutsukiTauriHostBuilder {
    pub fn new() -> Self {
        Self {
            config: MutsukiTauriConfig::for_app("MutsukiTauriApp"),
            runtime_config: HostRuntimeConfig::default(),
            runners: Vec::new(),
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

    pub fn runtime_config(mut self, runtime_config: HostRuntimeConfig) -> Self {
        self.runtime_config = runtime_config;
        self
    }

    pub fn runner(mut self, runner: Box<dyn Runner>) -> Self {
        self.runners.push(runner);
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
        let mut loaded = scan_plugin_runners(&self.config, events.clone(), health.clone())?;
        let mut bootstrapper = RuntimeBootstrapper::new();
        let runners = self.runners;
        let descriptors = runners
            .iter()
            .map(|runner| runner.descriptor().clone())
            .collect::<Vec<_>>();
        let mut plugin_runners: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for descriptor in &descriptors {
            plugin_runners
                .entry(descriptor.plugin_id.clone())
                .or_default()
                .push(descriptor.clone());
        }
        let mut enabled_plugins = BTreeSet::new();
        let mut plugin_deployments = BTreeMap::new();
        let mut active_protocols = BTreeSet::new();

        for manifest in loaded.manifests {
            active_protocols.extend(
                manifest
                    .provides
                    .runners
                    .iter()
                    .flat_map(|runner| runner.accepted_protocol_ids.iter().cloned()),
            );
            enabled_plugins.insert(manifest.plugin_id.clone());
            plugin_deployments.insert(manifest.plugin_id.clone(), PluginDeploymentKind::Process);
            bootstrapper.register_manifest(manifest);
        }
        for runner in loaded.process_runners {
            bootstrapper.register_external_runner(PluginDeploymentKind::Process, runner);
        }

        let mut loaded_plugin_ids = enabled_plugins.clone();
        for (plugin_id, descriptors) in plugin_runners {
            if loaded_plugin_ids.contains(&plugin_id) {
                return Err(HostError::Config(format!(
                    "builtin runner plugin id conflicts with discovered plugin: {plugin_id}"
                )));
            }
            let manifest = runner_manifest(&plugin_id, descriptors.clone());
            loaded
                .plugins
                .push(loaded_builtin_plugin_summary(&manifest));
            for descriptor in &descriptors {
                active_protocols.extend(descriptor.accepted_protocol_ids.iter().cloned());
                loaded
                    .runners
                    .push(loaded_builtin_runner_summary(descriptor));
            }
            enabled_plugins.insert(plugin_id.clone());
            plugin_deployments.insert(plugin_id.clone(), PluginDeploymentKind::Builtin);
            loaded_plugin_ids.insert(plugin_id.clone());
            bootstrapper.register_manifest(manifest);
        }
        for runner in runners {
            bootstrapper.register_builtin_runner(runner);
        }
        loaded
            .plugins
            .sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
        loaded
            .runners
            .sort_by(|left, right| left.runner_id.cmp(&right.runner_id));
        let profile = RuntimeProfile {
            profile_id: self.config.profile_id.clone(),
            mode: RuntimeProfileMode::FullDev,
            enabled_plugins: enabled_plugins.iter().cloned().collect(),
            bindings: BTreeMap::new(),
            plugin_deployments,
            allow_dynamic_registration: false,
            allow_hot_reload: false,
        };
        let runtime = bootstrapper.into_host_runtime_with_config(
            profile,
            self.runtime_config.with_resource_provider(
                mutsuki_tauri_resource::PROVIDER_ID,
                resource_store.provider(),
            ),
        )?;
        Ok(MutsukiTauriHost::new(
            self.config,
            runtime,
            resource_store,
            events,
            health,
            loaded.plugins,
            loaded.runners,
            active_protocols,
        ))
    }
}

impl Default for MutsukiTauriHostBuilder {
    fn default() -> Self {
        Self::new()
    }
}
