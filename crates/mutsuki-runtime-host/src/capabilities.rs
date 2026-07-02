use std::collections::{BTreeMap, BTreeSet};

use mutsuki_runtime_contracts::{
    BridgeDescriptor, CodecDescriptor, HostExtensionDescriptor, PluginBackendDescriptor,
    PluginDeploymentKind, PluginManifest, RuntimeLoadPlan, SchedulerPolicyDescriptor,
    WorkflowDescriptor,
};
use mutsuki_runtime_core::RuntimeResult;
use mutsuki_runtime_sdk::CapabilityBroker;

use crate::error::{capability_provider_missing, capability_pruned};

#[derive(Clone, Debug, Default)]
pub struct HostCapabilityRegistry {
    host_extensions: BTreeMap<String, HostExtensionDescriptor>,
    plugin_backends: BTreeMap<String, PluginBackendDescriptor>,
    codecs: BTreeMap<String, CodecDescriptor>,
    bridges: BTreeMap<String, BridgeDescriptor>,
    scheduler_policies: BTreeMap<String, SchedulerPolicyDescriptor>,
    workflows: BTreeMap<String, WorkflowDescriptor>,
    provided: BTreeSet<String>,
}

impl HostCapabilityRegistry {
    pub fn from_load_plan(plan: &RuntimeLoadPlan) -> RuntimeResult<Self> {
        let graph = &plan.capability_graph;
        let registry = Self {
            host_extensions: active_descriptors(
                plan,
                &graph.active_host_extensions,
                |manifest| &manifest.provides.host_extensions,
                |descriptor| descriptor.extension_id.as_str(),
            ),
            plugin_backends: active_descriptors(
                plan,
                &graph.active_plugin_backends,
                |manifest| &manifest.provides.plugin_backends,
                |descriptor| descriptor.backend_id.as_str(),
            ),
            codecs: active_descriptors(
                plan,
                &graph.active_codecs,
                |manifest| &manifest.provides.codecs,
                |descriptor| descriptor.codec_id.as_str(),
            ),
            bridges: active_descriptors(
                plan,
                &graph.active_bridges,
                |manifest| &manifest.provides.bridges,
                |descriptor| descriptor.bridge_id.as_str(),
            ),
            scheduler_policies: active_descriptors(
                plan,
                &graph.active_scheduler_policies,
                |manifest| &manifest.provides.scheduler_policies,
                |descriptor| descriptor.policy_id.as_str(),
            ),
            workflows: active_descriptors(
                plan,
                &graph.active_workflows,
                |manifest| &manifest.provides.workflows,
                |descriptor| descriptor.workflow_id.as_str(),
            ),
            provided: graph.provided_capabilities.iter().cloned().collect(),
        };

        registry.ensure_active_ids_registered("host_extension", &graph.active_host_extensions)?;
        registry.ensure_active_ids_registered("plugin_backend", &graph.active_plugin_backends)?;
        registry.ensure_active_ids_registered("codec", &graph.active_codecs)?;
        registry.ensure_active_ids_registered("bridge", &graph.active_bridges)?;
        registry
            .ensure_active_ids_registered("scheduler_policy", &graph.active_scheduler_policies)?;
        registry.ensure_active_ids_registered("workflow", &graph.active_workflows)?;
        registry.ensure_active_backend_references()?;
        Ok(registry)
    }

    pub fn require_host_extension(
        &self,
        extension_id: &str,
    ) -> RuntimeResult<&HostExtensionDescriptor> {
        self.require("host_extension", extension_id, &self.host_extensions)
    }

    pub fn require_plugin_backend(
        &self,
        backend_id: &str,
    ) -> RuntimeResult<&PluginBackendDescriptor> {
        self.require("plugin_backend", backend_id, &self.plugin_backends)
    }

    pub(crate) fn active_plugin_backend_for_deployment(
        &self,
        deployment: &PluginDeploymentKind,
    ) -> Option<&PluginBackendDescriptor> {
        self.plugin_backends
            .values()
            .find(|backend| &backend.deployment_kind == deployment)
    }

    pub fn require_codec(&self, codec_id: &str) -> RuntimeResult<&CodecDescriptor> {
        self.require("codec", codec_id, &self.codecs)
    }

    pub fn require_bridge(&self, bridge_id: &str) -> RuntimeResult<&BridgeDescriptor> {
        self.require("bridge", bridge_id, &self.bridges)
    }

    pub fn require_scheduler_policy(
        &self,
        policy_id: &str,
    ) -> RuntimeResult<&SchedulerPolicyDescriptor> {
        self.require("scheduler_policy", policy_id, &self.scheduler_policies)
    }

    pub fn require_workflow(&self, workflow_id: &str) -> RuntimeResult<&WorkflowDescriptor> {
        self.require("workflow", workflow_id, &self.workflows)
    }

    fn require<'a, T>(
        &self,
        prefix: &str,
        id: &str,
        entries: &'a BTreeMap<String, T>,
    ) -> RuntimeResult<&'a T> {
        entries
            .get(id)
            .ok_or_else(|| self.unavailable_capability(prefix, id))
    }

    fn ensure_active_ids_registered(
        &self,
        prefix: &str,
        active_ids: &[String],
    ) -> RuntimeResult<()> {
        for id in active_ids {
            self.require_registered(prefix, id)?;
        }
        Ok(())
    }

    fn ensure_active_backend_references(&self) -> RuntimeResult<()> {
        for backend in self.plugin_backends.values() {
            if let Some(codec_id) = &backend.codec_id {
                self.require_codec(codec_id)?;
            }
            if let Some(bridge_id) = &backend.bridge_id {
                self.require_bridge(bridge_id)?;
            }
        }
        for bridge in self.bridges.values() {
            for codec_id in &bridge.codec_ids {
                self.require_codec(codec_id)?;
            }
        }
        Ok(())
    }

    fn require_registered(&self, prefix: &str, id: &str) -> RuntimeResult<()> {
        match prefix {
            "host_extension" => self.require_host_extension(id).map(|_| ()),
            "plugin_backend" => self.require_plugin_backend(id).map(|_| ()),
            "codec" => self.require_codec(id).map(|_| ()),
            "bridge" => self.require_bridge(id).map(|_| ()),
            "scheduler_policy" => self.require_scheduler_policy(id).map(|_| ()),
            "workflow" => self.require_workflow(id).map(|_| ()),
            _ => Err(capability_provider_missing(&format!("{prefix}:{id}"))),
        }
    }

    fn unavailable_capability(
        &self,
        prefix: &str,
        id: &str,
    ) -> mutsuki_runtime_core::RuntimeFailure {
        let capability = format!("{prefix}:{id}");
        if self.provided.contains(&capability) {
            capability_pruned(&capability)
        } else {
            capability_provider_missing(&capability)
        }
    }
}

impl CapabilityBroker for HostCapabilityRegistry {
    fn require_capability(&self, capability: &str) -> RuntimeResult<()> {
        if active_capability(self, capability) {
            Ok(())
        } else if self.provided.contains(capability) {
            Err(capability_pruned(capability))
        } else {
            Err(capability_provider_missing(capability))
        }
    }

    fn require_host_extension(&self, extension_id: &str) -> RuntimeResult<HostExtensionDescriptor> {
        HostCapabilityRegistry::require_host_extension(self, extension_id).cloned()
    }

    fn require_plugin_backend(&self, backend_id: &str) -> RuntimeResult<PluginBackendDescriptor> {
        HostCapabilityRegistry::require_plugin_backend(self, backend_id).cloned()
    }

    fn require_codec(&self, codec_id: &str) -> RuntimeResult<CodecDescriptor> {
        HostCapabilityRegistry::require_codec(self, codec_id).cloned()
    }

    fn require_bridge(&self, bridge_id: &str) -> RuntimeResult<BridgeDescriptor> {
        HostCapabilityRegistry::require_bridge(self, bridge_id).cloned()
    }

    fn require_scheduler_policy(
        &self,
        policy_id: &str,
    ) -> RuntimeResult<SchedulerPolicyDescriptor> {
        HostCapabilityRegistry::require_scheduler_policy(self, policy_id).cloned()
    }

    fn require_workflow(&self, workflow_id: &str) -> RuntimeResult<WorkflowDescriptor> {
        HostCapabilityRegistry::require_workflow(self, workflow_id).cloned()
    }
}

fn active_capability(registry: &HostCapabilityRegistry, capability: &str) -> bool {
    let Some((prefix, id)) = capability.split_once(':') else {
        return false;
    };
    match prefix {
        "host_extension" => registry.host_extensions.contains_key(id),
        "plugin_backend" => registry.plugin_backends.contains_key(id),
        "codec" => registry.codecs.contains_key(id),
        "bridge" => registry.bridges.contains_key(id),
        "scheduler_policy" => registry.scheduler_policies.contains_key(id),
        "workflow" => registry.workflows.contains_key(id),
        _ => false,
    }
}

fn active_descriptors<T, D, I>(
    plan: &RuntimeLoadPlan,
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
    for manifest in &plan.plugins {
        for descriptor in descriptors(manifest) {
            let descriptor_id = id(descriptor);
            if active_ids.contains(descriptor_id) {
                entries.insert(descriptor_id.to_string(), descriptor.clone());
            }
        }
    }
    entries
}
