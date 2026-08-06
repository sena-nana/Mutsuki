//! ServiceHost control-backed config lifecycle (real plugin_reload).

use std::sync::Arc;

use mutsuki_bot_config::{ConfigAction, ConfigError, ConfigLifecycle, RestartPolicy};
use mutsuki_service_control::{
    ControlCommand, ControlHandler, ControlRequest, ControlResponse, ControlResult,
};

pub struct ControlPluginReloadLifecycle {
    control: Arc<dyn ControlHandler>,
    token: String,
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
        policy: RestartPolicy,
        completed: &[ConfigAction],
    ) -> Result<(), ConfigError> {
        self.execute(provider_id, policy, completed).map(|_| ())
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
