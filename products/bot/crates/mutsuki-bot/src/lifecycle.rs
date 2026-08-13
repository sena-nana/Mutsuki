//! ServiceHost control-backed config lifecycle (real plugin_reload).

use std::sync::Arc;

use mutsuki_agent_service_host_integration::LOCAL_AGENT_PLUGIN_ID;
use mutsuki_config_service::{
    ConfigAction, ConfigError, ConfigLifecycle, ConfigValue, RestartPolicy,
};
use mutsuki_plugin_bot_agent::BOT_AGENT_BRIDGE_PLUGIN_ID;
use mutsuki_service_control::{
    ControlCommand, ControlHandler, ControlRequest, ControlResponse, ControlResult,
};
use mutsuki_service_runtime::ServiceRuntimeHandle;

pub struct ControlPluginReloadLifecycle {
    control: Arc<dyn ControlHandler>,
    token: String,
}

pub struct TargetedPluginReloadLifecycle {
    runtime: ServiceRuntimeHandle,
}

impl TargetedPluginReloadLifecycle {
    pub fn new(runtime: ServiceRuntimeHandle) -> Self {
        Self { runtime }
    }

    fn reconfigure(&self, provider_id: &str, value: &ConfigValue) -> Result<(), ConfigError> {
        let runtime = self.runtime.clone();
        let selections = if provider_id == crate::PRODUCT_CONFIG_PROVIDER_ID {
            product_reconfigure_candidates(
                runtime.configured_plugin_selections(),
                &value.to_json(),
            )?
        } else {
            let base = runtime.configured_plugin_selection(provider_id);
            let mut selections = vec![crate::configured_plugin_selection_from_value(
                provider_id,
                value,
                base.as_ref(),
            )?];
            if provider_id == LOCAL_AGENT_PLUGIN_ID {
                if let Some(bridge) =
                    runtime.configured_plugin_selection(BOT_AGENT_BRIDGE_PLUGIN_ID)
                {
                    selections.push(bridge);
                }
            }
            selections
        };
        if selections.is_empty() {
            return Ok(());
        }
        block_on_result(async move { runtime.reconfigure_plugins(&selections).await })
            .map(|_| ())
            .map_err(|error| ConfigError::ReloadFailed {
                reason: error.to_string(),
            })
    }
}

fn product_reconfigure_candidates(
    current: Vec<mutsuki_service_config::ConfiguredPluginSelection>,
    product: &serde_json::Value,
) -> Result<Vec<mutsuki_service_config::ConfiguredPluginSelection>, ConfigError> {
    let desired = crate::runtime_plugin_selections(product)?;
    let desired = desired
        .into_iter()
        .map(|selection| (selection.id.clone(), selection))
        .collect::<std::collections::BTreeMap<_, _>>();
    let current = current
        .into_iter()
        .filter(|selection| !crate::is_product_owner_plugin(&selection.id))
        .map(|selection| (selection.id.clone(), selection))
        .collect::<std::collections::BTreeMap<_, _>>();
    let ids = desired
        .keys()
        .chain(current.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut candidates = Vec::new();
    for id in ids {
        match (current.get(&id), desired.get(&id)) {
            (Some(current), Some(desired))
                if current.enabled == desired.enabled && current.config == desired.config => {}
            (_, Some(desired)) => candidates.push(desired.clone()),
            (Some(current), None) if current.enabled => {
                let mut disabled = current.clone();
                disabled.enabled = false;
                candidates.push(disabled);
            }
            _ => {}
        }
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::product_reconfigure_candidates;
    use mutsuki_service_config::ConfiguredPluginSelection;

    #[test]
    fn removed_generic_plugin_becomes_an_explicit_disabled_candidate() {
        let current = vec![ConfiguredPluginSelection {
            id: "example.generic".into(),
            enabled: true,
            config: serde_json::json!({"value": 1}),
        }];
        let candidates =
            product_reconfigure_candidates(current, &serde_json::json!({"runtime_plugins": {}}))
                .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "example.generic");
        assert!(!candidates[0].enabled);
        assert_eq!(candidates[0].config, serde_json::json!({"value": 1}));
    }
}

impl ConfigLifecycle for TargetedPluginReloadLifecycle {
    fn execute(
        &self,
        provider_id: &str,
        _current: &ConfigValue,
        candidate: &ConfigValue,
        policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError> {
        let needs_reload = matches!(policy, RestartPolicy::PluginReload)
            || pending
                .iter()
                .any(|action| matches!(action, ConfigAction::PluginReloaded));
        if !needs_reload {
            if provider_id == crate::PRODUCT_CONFIG_PROVIDER_ID {
                crate::runtime_plugin_selections(&candidate.to_json())?;
            }
            return Ok(Vec::new());
        }
        self.reconfigure(provider_id, candidate)?;
        Ok(vec![ConfigAction::PluginReloaded])
    }

    fn rollback(
        &self,
        provider_id: &str,
        current: &ConfigValue,
        _policy: RestartPolicy,
        completed: &[ConfigAction],
    ) -> Result<(), ConfigError> {
        if completed
            .iter()
            .any(|action| matches!(action, ConfigAction::PluginReloaded))
        {
            self.reconfigure(provider_id, current)
        } else {
            Ok(())
        }
    }
}

impl ControlPluginReloadLifecycle {
    pub fn new(control: Arc<dyn ControlHandler>, token: impl Into<String>) -> Self {
        Self {
            control,
            token: token.into(),
        }
    }
}

impl ConfigLifecycle for ControlPluginReloadLifecycle {
    fn execute(
        &self,
        _provider_id: &str,
        _current: &ConfigValue,
        _candidate: &ConfigValue,
        policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError> {
        let needs_reload = matches!(policy, RestartPolicy::PluginReload)
            || pending
                .iter()
                .any(|action| matches!(action, ConfigAction::PluginReloaded));
        if !needs_reload {
            return Ok(Vec::new());
        }
        let control = self.control.clone();
        let token = self.token.clone();
        let response = block_on_control(async move {
            control
                .handle(ControlRequest::new(token, ControlCommand::PluginReload))
                .await
        });
        match response {
            ControlResponse::Ok(ControlResult::PluginReload(_)) => {
                Ok(vec![ConfigAction::PluginReloaded])
            }
            ControlResponse::Error(error) => Err(ConfigError::ReloadFailed {
                reason: format!("{}: {}", error.code, error.message),
            }),
            ControlResponse::Ok(result) => Err(ConfigError::ReloadFailed {
                reason: format!(
                    "plugin_reload returned {:?} control response",
                    result.method()
                ),
            }),
        }
    }

    fn rollback(
        &self,
        provider_id: &str,
        current: &ConfigValue,
        policy: RestartPolicy,
        completed: &[ConfigAction],
    ) -> Result<(), ConfigError> {
        self.execute(provider_id, current, current, policy, completed)
            .map(|_| ())
    }
}

fn block_on_control<F>(future: F) -> ControlResponse
where
    F: std::future::Future<Output = ControlResponse> + Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            _ => std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("config lifecycle runtime")
                    .block_on(future)
            })
            .join()
            .expect("config lifecycle thread"),
        },
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("config lifecycle runtime");
            runtime.block_on(future)
        }
    }
}

fn block_on_result<F, T, E>(future: F) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            _ => std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("config lifecycle runtime")
                    .block_on(future)
            })
            .join()
            .expect("config lifecycle thread"),
        },
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("config lifecycle runtime")
            .block_on(future),
    }
}
