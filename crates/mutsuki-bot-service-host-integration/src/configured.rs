use mutsuki_bot_protocol::BotEventSubscription;
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig};
use mutsuki_plugin_bot_command::{
    BOT_COMMAND_PLUGIN_ID, BotCommandConfig, BotCommandRunner, bot_command_manifest,
};
use mutsuki_plugin_bot_event_router::{
    BOT_EVENT_ROUTER_PLUGIN_ID, BotEventRouterRunner, bot_event_router_manifest,
};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{BilibiliPollingEventSource, QqBotPluginBundle};
use mutsuki_plugin_bot_bilibili::{
    BilibiliConfig, BilibiliRunner, PLUGIN_ID as BILIBILI_PLUGIN_ID, ReqwestBilibiliTransport,
    SharedBilibiliCredential, SqliteBilibiliRepository,
};
use mutsuki_plugin_bot_bilibili_workshop::{
    PLUGIN_ID as WORKSHOP_PLUGIN_ID, ReqwestWorkshopTransport, WorkshopRunner,
};
use mutsuki_plugin_bot_mihuashi::PLUGIN_ID as MIHUASHI_PLUGIN_ID;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventRouterConfig {
    subscriptions: Vec<BotEventSubscription>,
}

pub struct BotEventRouterConfiguredPlugin;

impl ConfiguredPluginFactory for BotEventRouterConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BOT_EVENT_ROUTER_PLUGIN_ID
    }

    fn prepare(
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

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: BotCommandConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate()?;
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

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: QqBotConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        let media_provider_id = config.media_provider_id.clone();
        let mut bundle =
            QqBotPluginBundle::new(config).map_err(|error| error.redacted_message())?;
        if let Some(provider_id) = media_provider_id {
            bundle = bundle.with_resource_media_provider(provider_id);
        }
        bundle
            .install(builder)
            .map_err(|error| error.redacted_message())
    }
}

pub struct BilibiliConfiguredPlugin;

impl ConfiguredPluginFactory for BilibiliConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        BILIBILI_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: BilibiliConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate()?;
        let repository = Arc::new(
            SqliteBilibiliRepository::open(builder.data_dir().join("bilibili/state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let credential = SharedBilibiliCredential::default();
        let runner_config = config.clone();
        let runner_repository = repository.clone();
        let runner_credential = credential.clone();
        let source = BilibiliPollingEventSource::new(config, credential);
        let mut manifest = mutsuki_plugin_bot_bilibili::manifest();
        manifest.requires.push(format!(
            "resource_strategy:{}",
            runner_config.media_provider_id
        ));
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                let transport = ReqwestBilibiliTransport::new(
                    runner_credential.clone(),
                    Duration::from_secs(15),
                );
                Ok::<
                    Box<dyn mutsuki_runtime_core::Runner>,
                    mutsuki_plugin_bot_bilibili::BilibiliError,
                >(Box::new(BilibiliRunner::new(
                    Box::new(transport),
                    runner_repository.clone(),
                    resources,
                    runner_config.media_provider_id.clone(),
                )))
            })
            .register_event_source(Box::new(source)))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkCardPluginConfig {
    media_provider_id: String,
}

impl LinkCardPluginConfig {
    fn validate(&self) -> Result<(), String> {
        if self.media_provider_id.trim().is_empty() {
            return Err("media_provider_id is required".into());
        }
        Ok(())
    }
}

pub struct WorkshopConfiguredPlugin;

impl ConfiguredPluginFactory for WorkshopConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        WORKSHOP_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: LinkCardPluginConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate()?;
        let mut manifest = mutsuki_plugin_bot_bilibili_workshop::manifest();
        manifest
            .requires
            .push(format!("resource_strategy:{}", config.media_provider_id));
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                let transport = ReqwestWorkshopTransport::new();
                Ok::<Box<dyn mutsuki_runtime_core::Runner>, String>(Box::new(WorkshopRunner::new(
                    Box::new(transport),
                    resources,
                    config.media_provider_id.clone(),
                )))
            }))
    }
}

pub struct MihuashiConfiguredPlugin;

impl ConfiguredPluginFactory for MihuashiConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        MIHUASHI_PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: LinkCardPluginConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        config.validate()?;
        let mut manifest = mutsuki_plugin_bot_mihuashi::manifest();
        manifest
            .requires
            .push(format!("resource_strategy:{}", config.media_provider_id));
        manifest
            .requires
            .push("task_protocol:mutsuki.browser.snapshot".into());
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_runtime_services_runner(move |client, resources| {
                mutsuki_plugin_bot_mihuashi::runner(
                    client,
                    resources,
                    config.media_provider_id.clone(),
                )
            }))
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
    catalog.register(BilibiliConfiguredPlugin)?;
    catalog.register(WorkshopConfiguredPlugin)?;
    catalog.register(MihuashiConfiguredPlugin)?;
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
