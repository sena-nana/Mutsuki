use mutsuki_bot_service_host_integration::{BOT_SANDBOX_PLUGIN_ID, bot_sandbox, bot_state_db};
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use serde_json::Value;

use crate::QqBotPluginBundle;

pub struct QqBotConfiguredPlugin {
    media_provider_id: String,
}

impl QqBotConfiguredPlugin {
    #[must_use]
    pub fn new(media_provider_id: impl Into<String>) -> Self {
        Self {
            media_provider_id: media_provider_id.into(),
        }
    }
}

impl ConfiguredPluginFactory for QqBotConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        QQBOT_ADAPTER_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let mut config: QqBotConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        // The product assembly owns the media resource provider binding; inbound
        // media validation reads it back from the adapter config.
        config.media_provider_id = Some(self.media_provider_id.clone());
        let (mut builder, repository) = bot_state_db(builder)?;
        let mut bundle =
            QqBotPluginBundle::new(config).map_err(|error| error.redacted_message())?;
        bundle = bundle
            .with_resource_media_provider(self.media_provider_id.clone())
            .with_state_repository(repository);
        if builder
            .configured_plugin_selection(BOT_SANDBOX_PLUGIN_ID)
            .is_some()
        {
            // Whichever of the sandbox plugin and this adapter prepares first creates the
            // simulator; selection order does not matter.
            let (with_sandbox, sandbox) = bot_sandbox(builder)?;
            builder = with_sandbox;
            bundle = bundle.with_workspace_sandbox(sandbox);
        }
        bundle
            .install(builder)
            .map_err(|error| error.redacted_message())
    }
}

/// The QQ adapter's configured plugin; merge it with the core Bot catalog.
pub fn configured_qq_plugin_catalog(
    media_provider_id: impl Into<String>,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(QqBotConfiguredPlugin::new(media_provider_id))?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn configured_qq_plugin_fails_preflight_without_host_secret() {
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service.ipc.enabled = false;
        service.observe.console = false;
        service.plugins.dynamic_dirs.clear();
        service.service.home_dir = root.path().into();
        service.service.data_dir = root.path().join("data");
        service.service.log_dir = root.path().join("logs");
        service.service.run_dir = root.path().join("run");
        std::fs::create_dir_all(&service.service.data_dir).unwrap();
        std::fs::create_dir_all(&service.service.log_dir).unwrap();
        std::fs::create_dir_all(&service.service.run_dir).unwrap();
        service.plugins.configured = vec![ConfiguredPluginSelection {
            id: QQBOT_ADAPTER_PLUGIN_ID.into(),
            enabled: true,
            config: json!({
                "account_id": "configured",
                "app_id": "APP_ID",
                "client_secret_key": "MISSING_CONFIGURED_QQ_SECRET"
            }),
        }];

        let error = match ServiceRuntimeBuilder::new(service)
            .with_configured_plugin_catalog(
                configured_qq_plugin_catalog(
                    mutsuki_bot_service_host_integration::DEFAULT_MEDIA_PROVIDER_ID,
                )
                .unwrap(),
            )
            .start()
            .await
        {
            Ok(runtime) => {
                runtime.shutdown().await;
                panic!("configured QQBot unexpectedly started")
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("MISSING_CONFIGURED_QQ_SECRET"));
    }
}
