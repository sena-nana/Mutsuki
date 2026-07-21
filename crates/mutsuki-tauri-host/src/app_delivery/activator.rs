use super::types::{ActivationError, ActivationReceipt, AppDescriptor, AppId};
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Mutex;

pub trait TauriAppActivator: Send + Sync {
    fn resolve(
        &self,
        app_id: &AppId,
    ) -> impl std::future::Future<Output = Result<AppDescriptor, ActivationError>> + Send;

    fn activate(
        &self,
        app: &AppDescriptor,
    ) -> impl std::future::Future<Output = Result<ActivationReceipt, ActivationError>> + Send;
}

#[derive(Clone, Debug, Default)]
pub struct NullAppActivator;

impl TauriAppActivator for NullAppActivator {
    async fn resolve(&self, app_id: &AppId) -> Result<AppDescriptor, ActivationError> {
        Err(ActivationError::AppNotInstalled(app_id.to_string()))
    }

    async fn activate(&self, app: &AppDescriptor) -> Result<ActivationReceipt, ActivationError> {
        Err(ActivationError::AppNotInstalled(app.app_id.to_string()))
    }
}

/// Registry-backed activator that can launch installed executables.
#[derive(Clone, Default)]
pub struct ProcessAppActivator {
    apps: Arc<Mutex<BTreeMap<String, AppDescriptor>>>,
    running: Arc<Mutex<BTreeMap<String, String>>>,
}

impl ProcessAppActivator {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, descriptor: AppDescriptor) {
        self.apps
            .lock()
            .await
            .insert(descriptor.app_id.as_str().to_string(), descriptor);
    }

    pub async fn mark_running(&self, app_id: &AppId, instance_id: impl Into<String>) {
        self.running
            .lock()
            .await
            .insert(app_id.as_str().to_string(), instance_id.into());
    }
}

impl TauriAppActivator for ProcessAppActivator {
    async fn resolve(&self, app_id: &AppId) -> Result<AppDescriptor, ActivationError> {
        self.apps
            .lock()
            .await
            .get(app_id.as_str())
            .cloned()
            .ok_or_else(|| ActivationError::AppNotInstalled(app_id.to_string()))
    }

    async fn activate(&self, app: &AppDescriptor) -> Result<ActivationReceipt, ActivationError> {
        if let Some(instance_id) = self.running.lock().await.get(app.app_id.as_str()).cloned() {
            return Ok(ActivationReceipt {
                app_id: app.app_id.clone(),
                instance_id,
                already_running: true,
            });
        }
        let executable = app.executable.as_ref().ok_or_else(|| {
            ActivationError::ActivationFailed(format!(
                "no executable registered for {}",
                app.app_id
            ))
        })?;
        let mut command = Command::new(executable);
        command.args(&app.launch_args);
        command
            .spawn()
            .map_err(|error| ActivationError::ActivationFailed(error.to_string()))?;
        let instance_id = uuid::Uuid::new_v4().to_string();
        self.running
            .lock()
            .await
            .insert(app.app_id.as_str().to_string(), instance_id.clone());
        Ok(ActivationReceipt {
            app_id: app.app_id.clone(),
            instance_id,
            already_running: false,
        })
    }
}
