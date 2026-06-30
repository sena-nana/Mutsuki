use crate::config::{MutsukiTauriConfig, PathsConfig};
use crate::echo::EchoRunner;
use crate::error::{HostError, HostResult};
use crate::host::MutsukiTauriHost;
use mutsuki_runtime_contracts::{PluginDeploymentKind, RuntimeProfile, RuntimeProfileMode};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_host::{HostRuntimeConfig, RuntimeBootstrapper, runner_manifest};
use mutsuki_tauri_bridge::EventHub;
use mutsuki_tauri_resource::TauriResourceStore;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct MutsukiTauriHostBuilder {
    config: MutsukiTauriConfig,
    runners: Vec<Box<dyn Runner>>,
}

impl MutsukiTauriHostBuilder {
    pub fn new() -> Self {
        Self {
            config: MutsukiTauriConfig::for_app("MutsukiTauriApp"),
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

    pub fn runner(mut self, runner: Box<dyn Runner>) -> Self {
        self.runners.push(runner);
        self
    }

    pub fn build(self) -> HostResult<MutsukiTauriHost> {
        std::fs::create_dir_all(&self.config.paths.resources_dir)
            .map_err(|error| HostError::Config(error.to_string()))?;
        std::fs::create_dir_all(&self.config.paths.logs_dir)
            .map_err(|error| HostError::Config(error.to_string()))?;

        let resource_store = Arc::new(TauriResourceStore::new(&self.config.paths.resources_dir));
        let mut bootstrapper = RuntimeBootstrapper::new();
        let mut runners = self.runners;
        if runners.is_empty() {
            runners.push(Box::new(EchoRunner::new()));
        }
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
        for (plugin_id, descriptors) in plugin_runners {
            bootstrapper.register_manifest(runner_manifest(&plugin_id, descriptors));
        }
        for runner in runners {
            bootstrapper.register_builtin_runner(runner);
        }
        let enabled_plugins = descriptors
            .iter()
            .map(|descriptor| descriptor.plugin_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let profile = RuntimeProfile {
            profile_id: self.config.profile_id.clone(),
            mode: RuntimeProfileMode::FullDev,
            enabled_plugins: enabled_plugins.iter().cloned().collect(),
            bindings: BTreeMap::new(),
            plugin_deployments: enabled_plugins
                .iter()
                .map(|plugin_id| (plugin_id.clone(), PluginDeploymentKind::Builtin))
                .collect(),
            allow_dynamic_registration: false,
            allow_hot_reload: false,
        };
        let runtime = bootstrapper.into_host_runtime_with_config(
            profile,
            HostRuntimeConfig {
                resource_provider: Some(resource_store.clone()),
                ..HostRuntimeConfig::default()
            },
        )?;
        let event_buffer = self.config.event_buffer;
        Ok(MutsukiTauriHost::new(
            self.config,
            runtime,
            resource_store,
            Arc::new(EventHub::new(event_buffer)),
            descriptors,
        ))
    }
}

impl Default for MutsukiTauriHostBuilder {
    fn default() -> Self {
        Self::new()
    }
}
