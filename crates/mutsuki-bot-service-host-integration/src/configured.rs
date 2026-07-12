use mutsuki_bot_protocol::BotEventSubscription;
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig};
use mutsuki_plugin_bot_command::{BOT_COMMAND_PLUGIN_ID, BotCommandRunner, bot_command_manifest};
use mutsuki_plugin_bot_event_router::{
    BOT_EVENT_ROUTER_PLUGIN_ID, BotEventRouterRunner, bot_event_router_manifest,
};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use serde::Deserialize;
use serde_json::Value;

use crate::QqBotPluginBundle;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventRouterConfig {
    subscriptions: Vec<BotEventSubscription>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandConfig {
    prefixes: Vec<String>,
}

pub struct BotEventRouterConfiguredPlugin;

impl ConfiguredPluginFactory for BotEventRouterConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_EVENT_ROUTER_PLUGIN_ID
    }

    fn install(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: EventRouterConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        if config.subscriptions.is_empty() {
            return Err("subscriptions must not be empty".into());
        }
        let subscriptions = config.subscriptions;
        Ok(builder
            .register_builtin_plugin(bot_event_router_manifest(1))
            .register_builtin_runner(move || {
                Box::new(BotEventRouterRunner::new(1, subscriptions.clone()))
            }))
    }
}

pub struct BotCommandConfiguredPlugin;

impl ConfiguredPluginFactory for BotCommandConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_COMMAND_PLUGIN_ID
    }

    fn install(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: CommandConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        if config.prefixes.is_empty() || config.prefixes.iter().any(|prefix| prefix.is_empty()) {
            return Err("prefixes must contain non-empty values".into());
        }
        let prefixes = config.prefixes;
        Ok(builder
            .register_builtin_plugin(bot_command_manifest(1))
            .register_builtin_runner(move || Box::new(BotCommandRunner::new(1, prefixes.clone()))))
    }
}

pub struct QqBotConfiguredPlugin;

impl ConfiguredPluginFactory for QqBotConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        QQBOT_ADAPTER_PLUGIN_ID
    }

    fn install(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: QqBotConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        QqBotPluginBundle::new(config)
            .map_err(|error| error.redacted_message())?
            .install(builder)
            .map_err(|error| error.redacted_message())
    }
}

/// Catalog of production Bot plugins that can be selected by ServiceHost configuration.
/// Media upload is intentionally absent until a product registers an explicit provider-backed
/// QQ factory of its own.
pub fn configured_bot_plugin_catalog() -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(BotEventRouterConfiguredPlugin)?;
    catalog.register(BotCommandConfiguredPlugin)?;
    catalog.register(QqBotConfiguredPlugin)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn configured_qq_plugin_fails_preflight_without_host_secret() {
        let mut service = ServiceConfig::default();
        service.ipc.enabled = false;
        service.observe.console = false;
        service.plugins.dynamic_dirs.clear();
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
            .with_configured_plugin_catalog(configured_bot_plugin_catalog().unwrap())
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
