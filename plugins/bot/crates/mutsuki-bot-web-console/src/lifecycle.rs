//! ServiceHost control-backed config lifecycle (real plugin_reload).

use std::sync::Arc;

use mutsuki_config_service::{
    ConfigAction, ConfigError, ConfigLifecycle, ConfigValue, RestartPolicy,
};
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
            value
                .to_json()
                .get("runtime_plugins")
                .and_then(serde_json::Value::as_object)
                .into_iter()
                .flatten()
                .map(
                    |(id, value)| mutsuki_service_config::ConfiguredPluginSelection {
                        id: id.clone(),
                        enabled: value
                            .get("enabled")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true),
                        config: value
                            .get("config")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    },
                )
                .collect::<Vec<_>>()
        } else {
            let base = runtime.configured_plugin_selection(provider_id);
            vec![crate::configured_plugin_selection_from_value(
                provider_id,
                value,
                base.as_ref(),
            )?]
        };
        block_on_result(async move { runtime.reconfigure_plugins(&selections).await })
            .map(|_| ())
            .map_err(|error| ConfigError::ReloadFailed {
                reason: error.to_string(),
            })
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
