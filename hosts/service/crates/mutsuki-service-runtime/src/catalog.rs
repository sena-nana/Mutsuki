//! Plugin catalog resolution and the persisted per-plugin deployment state.

#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PluginDeploymentState {
    #[serde(default = "deployment_state_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) plugins: BTreeMap<PluginId, PluginDeploymentKind>,
}

impl Default for PluginDeploymentState {
    fn default() -> Self {
        Self {
            version: deployment_state_version(),
            plugins: BTreeMap::new(),
        }
    }
}

pub(crate) fn deployment_state_version() -> u32 {
    1
}

#[cfg(test)]
pub(crate) fn load_catalog(
    config: &ServiceConfig,
    builtin_registry: &BuiltinRegistry,
) -> ServiceRuntimeResult<PluginCatalog> {
    let preferences = load_deployment_state(config)?;
    load_catalog_with_state(config, builtin_registry, &preferences)
}

pub(crate) fn load_catalog_with_state(
    config: &ServiceConfig,
    builtin_registry: &BuiltinRegistry,
    preferences: &PluginDeploymentState,
) -> ServiceRuntimeResult<PluginCatalog> {
    let builtin = builtin_registry.load_all()?;
    let inventory = PluginInventory::scan(
        &config.plugins.dynamic_dirs,
        &config.plugins.disabled_dir,
        builtin,
    )?;
    resolve_catalog(config, inventory, preferences)
}

pub(crate) fn resolve_catalog(
    config: &ServiceConfig,
    inventory: PluginInventory,
    preferences: &PluginDeploymentState,
) -> ServiceRuntimeResult<PluginCatalog> {
    let mut selected = Vec::new();
    for selection in config
        .plugins
        .configured
        .iter()
        .filter(|selection| selection.enabled)
    {
        let plugin_id = selection.id.trim();
        let candidates = inventory
            .records
            .iter()
            .filter(|record| record.manifest.plugin_id == plugin_id)
            .collect::<Vec<_>>();
        let diagnostic = || {
            let details = inventory
                .diagnostics
                .iter()
                .filter(|item| item.plugin_id.as_ref().map(|id| id.as_str()) == Some(plugin_id))
                .map(|item| item.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            if details.is_empty() {
                "plugin is not linked or installed".into()
            } else {
                details
            }
        };
        if candidates.is_empty() {
            return Err(ServiceRuntimeError::PluginUnavailable {
                plugin_id: PluginId::from(plugin_id),
                detail: diagnostic(),
            });
        }
        let chosen = if let Some(preferred) = preferences.plugins.get(plugin_id) {
            candidates
                .iter()
                .copied()
                .find(|record| deployment_for(record) == *preferred)
                .ok_or_else(|| ServiceRuntimeError::PluginDeploymentUnavailable {
                    plugin_id: PluginId::from(plugin_id),
                    deployment: preferred.clone(),
                    detail: diagnostic(),
                })?
        } else if candidates.len() == 1 {
            candidates[0]
        } else {
            let builtin = candidates
                .iter()
                .copied()
                .find(|record| deployment_for(record) == PluginDeploymentKind::Builtin);
            let builtin_abi_only = candidates.iter().all(|record| {
                matches!(
                    deployment_for(record),
                    PluginDeploymentKind::Builtin | PluginDeploymentKind::Abi
                )
            });
            if builtin_abi_only {
                builtin.ok_or_else(|| ServiceRuntimeError::PluginDeploymentAmbiguous {
                    plugin_id: PluginId::from(plugin_id),
                })?
            } else {
                return Err(ServiceRuntimeError::PluginDeploymentAmbiguous {
                    plugin_id: PluginId::from(plugin_id),
                });
            }
        };
        if deployment_for(chosen) != PluginDeploymentKind::Builtin
            && let Some(builtin) = candidates
                .iter()
                .find(|record| deployment_for(record) == PluginDeploymentKind::Builtin)
            && builtin.manifest.business_surface() != chosen.manifest.business_surface()
        {
            return Err(ServiceRuntimeError::PluginBusinessSurfaceMismatch {
                plugin_id: PluginId::from(plugin_id),
            });
        }
        selected.push(chosen.clone());
    }
    Ok(PluginCatalog::resolved(inventory, selected))
}

pub(crate) fn deployment_for(record: &PluginRecord) -> PluginDeploymentKind {
    PluginDeploymentKind::default_for_artifact(&record.manifest.artifact.artifact_type)
}

pub(crate) fn deployment_name(deployment: &PluginDeploymentKind) -> String {
    format!("{deployment:?}").to_ascii_lowercase()
}

pub(crate) fn deployment_state_path(config: &ServiceConfig) -> std::path::PathBuf {
    config.service.data_dir.join("plugin-deployments.json")
}

pub(crate) fn load_deployment_state(
    config: &ServiceConfig,
) -> ServiceRuntimeResult<PluginDeploymentState> {
    let path = deployment_state_path(config);
    if !path.is_file() {
        return Ok(PluginDeploymentState {
            version: deployment_state_version(),
            plugins: BTreeMap::new(),
        });
    }
    let bytes = fs::read(&path).map_err(|error| ServiceRuntimeError::PluginDeploymentState {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    let state: PluginDeploymentState = serde_json::from_slice(&bytes).map_err(|error| {
        ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    if state.version != deployment_state_version() {
        return Err(ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: format!("unsupported state version {}", state.version),
        });
    }
    Ok(state)
}

pub(crate) fn save_deployment_state(
    config: &ServiceConfig,
    state: &PluginDeploymentState,
) -> ServiceRuntimeResult<()> {
    let path = deployment_state_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
    }
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| {
        ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    // Rename over the old file: deleting it first would lose the deployment
    // state if the process died between the two steps.
    mutsuki_service_config::write_managed_file_atomically(&path, &bytes).map_err(|error| {
        ServiceRuntimeError::PluginDeploymentState {
            path: path.display().to_string(),
            detail: error.to_string(),
        }
    })
}
