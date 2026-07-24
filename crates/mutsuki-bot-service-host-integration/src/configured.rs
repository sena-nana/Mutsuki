use mutsuki_bot_protocol::BotEventSubscription;
use mutsuki_plugin_bot_adapter_qqbot::{QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig};
use mutsuki_plugin_bot_command::{
    BOT_COMMAND_PLUGIN_ID, BotCommandConfig, BotCommandRunner, bot_command_manifest,
};
use mutsuki_plugin_bot_event_router::{
    BOT_EVENT_ROUTER_PLUGIN_ID, BotEventRouterRunner, bot_event_router_manifest,
};
use mutsuki_service_config::{ConfiguredPluginStore, HostSecretStore};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    BilibiliConsoleBridge, BilibiliPollingCredentials, BilibiliPollingEventSource,
    QqBotPluginBundle,
};
use mutsuki_plugin_bot_bilibili::{
    BilibiliBackendConfig, BilibiliConfig, BilibiliConfigStore, BilibiliCredentialStore,
    BilibiliManagementService, BilibiliRunner, BilibiliSecretPresence, CredentialSecretState,
    PLUGIN_ID as BILIBILI_PLUGIN_ID, ReqwestBilibiliOpenPlatformTransport,
    ReqwestBilibiliTransport, SharedBilibiliConfig, SharedBilibiliCredential,
    SqliteBilibiliRepository,
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

struct HostBilibiliCredentialStore {
    host: HostSecretStore,
    shared: SharedBilibiliCredential,
}

impl BilibiliCredentialStore for HostBilibiliCredentialStore {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
        self.host
            .rotate(key, credential.clone())
            .map_err(|error| error.to_string())?;
        self.shared.set(credential);
        Ok(())
    }
}

struct HostBilibiliConfigStore(ConfiguredPluginStore);

impl BilibiliConfigStore for HostBilibiliConfigStore {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String> {
        let value = serde_json::to_value(config).map_err(|error| error.to_string())?;
        self.0
            .replace_config(BILIBILI_PLUGIN_ID, value)
            .map_err(|error| error.to_string())
    }
}

struct RejectBilibiliConfigStore;

impl BilibiliConfigStore for RejectBilibiliConfigStore {
    fn replace(&self, _config: &BilibiliConfig) -> Result<(), String> {
        Err("Host configured-plugin persistence is unavailable".into())
    }
}

struct HostSecretPresence(HostSecretStore);

impl BilibiliSecretPresence for HostSecretPresence {
    fn inspect(&self, key: &str) -> CredentialSecretState {
        match self.0.resolve(key) {
            None => CredentialSecretState::Absent,
            Some(value) if value.trim().is_empty() => CredentialSecretState::Invalid,
            Some(_) => CredentialSecretState::Present,
        }
    }
}

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
        let host_secret_store = builder.host_secret_store();
        let configured_plugin_store = builder.configured_plugin_store();
        if matches!(config.backend, BilibiliBackendConfig::OpenPlatform { .. })
            && !host_secret_store.rotation_available()
        {
            return Err(
                "Bilibili Open Platform requires a Host security.secret_file for OAuth refresh"
                    .into(),
            );
        }
        if config.management.enabled && !host_secret_store.rotation_available() {
            return Err("Bilibili management requires a Host security.secret_file".into());
        }
        let configured_plugin_store = if config.management.enabled {
            Some(configured_plugin_store.ok_or_else(|| {
                "Bilibili management requires a loaded product config file".to_string()
            })?)
        } else {
            None
        };
        let repository = Arc::new(
            SqliteBilibiliRepository::open(builder.data_dir().join("bilibili/state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let web_credential = SharedBilibiliCredential::default();
        let app_secret = SharedBilibiliCredential::default();
        let oauth_credential = SharedBilibiliCredential::default();
        let shared_config = SharedBilibiliConfig::new(config);
        let runner_config = shared_config.clone();
        let runner_repository = repository.clone();
        let runner_web_credential = web_credential.clone();
        let runner_app_secret = app_secret.clone();
        let runner_oauth_credential = oauth_credential.clone();
        let source_credentials = match &shared_config.snapshot().backend {
            BilibiliBackendConfig::WebCookie { cookie_secret_key } => {
                BilibiliPollingCredentials::WebCookie {
                    secret_key: cookie_secret_key.clone(),
                    credential: web_credential.clone(),
                    required: !shared_config.snapshot().management.enabled,
                }
            }
            BilibiliBackendConfig::OpenPlatform {
                app_secret_key,
                oauth_credential_key,
                ..
            } => BilibiliPollingCredentials::OpenPlatform {
                app_secret_key: app_secret_key.clone(),
                app_secret: app_secret.clone(),
                oauth_credential_key: oauth_credential_key.clone(),
                oauth_credential: oauth_credential.clone(),
            },
        };
        let source = BilibiliPollingEventSource::new(shared_config.clone(), source_credentials);
        let manifest_config = runner_config.snapshot();
        let mut manifest = mutsuki_plugin_bot_bilibili::manifest_for_config(&manifest_config);
        manifest.requires.push(format!(
            "resource_strategy:{}",
            runner_config.snapshot().media_provider_id
        ));

        BilibiliConsoleBridge::clear();
        let management_service = match &runner_config.snapshot().backend {
            BilibiliBackendConfig::WebCookie { .. } => {
                let config_store: Arc<dyn BilibiliConfigStore> =
                    if let Some(store) = configured_plugin_store.clone() {
                        Arc::new(HostBilibiliConfigStore(store))
                    } else {
                        Arc::new(RejectBilibiliConfigStore)
                    };
                let service = Arc::new(BilibiliManagementService::new(
                    runner_config.clone(),
                    web_credential.clone(),
                    Box::new(ReqwestBilibiliTransport::new(
                        web_credential.clone(),
                        Duration::from_secs(15),
                    )),
                    repository.clone(),
                    Arc::new(HostBilibiliCredentialStore {
                        host: host_secret_store.clone(),
                        shared: web_credential.clone(),
                    }),
                    config_store,
                    Arc::new(HostSecretPresence(host_secret_store.clone())),
                ));
                BilibiliConsoleBridge::publish(service.clone());
                Some(service)
            }
            BilibiliBackendConfig::OpenPlatform {
                client_id,
                oauth_credential_key,
                authorized_uid,
                ..
            } => {
                let service = Arc::new(BilibiliManagementService::new(
                    runner_config.clone(),
                    oauth_credential.clone(),
                    Box::new(ReqwestBilibiliOpenPlatformTransport::new(
                        client_id,
                        *authorized_uid,
                        app_secret.clone(),
                        oauth_credential.clone(),
                        oauth_credential_key,
                        Arc::new(HostBilibiliCredentialStore {
                            host: host_secret_store.clone(),
                            shared: oauth_credential.clone(),
                        }),
                        Duration::from_secs(15),
                    )),
                    repository.clone(),
                    Arc::new(HostBilibiliCredentialStore {
                        host: host_secret_store.clone(),
                        shared: oauth_credential.clone(),
                    }),
                    Arc::new(RejectBilibiliConfigStore),
                    Arc::new(HostSecretPresence(host_secret_store.clone())),
                ));
                BilibiliConsoleBridge::publish(service.clone());
                Some(service)
            }
        };

        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |client, resources| {
                let snapshot = runner_config.snapshot();
                let transport: Box<dyn mutsuki_plugin_bot_bilibili::BilibiliTransport> =
                    match &snapshot.backend {
                        BilibiliBackendConfig::WebCookie { .. } => {
                            Box::new(ReqwestBilibiliTransport::new(
                                runner_web_credential.clone(),
                                Duration::from_secs(15),
                            ))
                        }
                        BilibiliBackendConfig::OpenPlatform {
                            client_id,
                            oauth_credential_key,
                            authorized_uid,
                            ..
                        } => Box::new(ReqwestBilibiliOpenPlatformTransport::new(
                            client_id,
                            *authorized_uid,
                            runner_app_secret.clone(),
                            runner_oauth_credential.clone(),
                            oauth_credential_key,
                            Arc::new(HostBilibiliCredentialStore {
                                host: host_secret_store.clone(),
                                shared: runner_oauth_credential.clone(),
                            }),
                            Duration::from_secs(15),
                        )),
                    };
                let mut runner = BilibiliRunner::new_for_backend(
                    transport,
                    runner_repository.clone(),
                    resources,
                    snapshot.media_provider_id.clone(),
                    snapshot.backend.kind(),
                );
                if snapshot.management.enabled {
                    let management = management_service.clone().ok_or_else(|| {
                        mutsuki_plugin_bot_bilibili::BilibiliError::ManagementUnavailable(
                            "Bilibili management service is unavailable".into(),
                        )
                    })?;
                    runner = runner.with_management(management);
                }
                Ok::<
                    Box<dyn mutsuki_runtime_core::Runner>,
                    mutsuki_plugin_bot_bilibili::BilibiliError,
                >(runner.into_runtime_runner(client, snapshot.risk_control.clone()))
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

    #[test]
    fn configured_bilibili_management_requires_host_persistence_boundaries() {
        let config = json!({
            "backend": {"type": "web_cookie", "cookie_secret_key": "BILIBILI_COOKIE"},
            "live_interval_ms": 1000,
            "dynamic_interval_ms": 1000,
            "video_interval_ms": 1000,
            "retry": {"max_attempts": 3, "initial_backoff_ms": 10, "max_backoff_ms": 100},
            "subscriptions": [],
            "link_resolver": {"enabled": false, "cooldown_ms": 1000, "account_to_binding": {}},
            "media_provider_id": "memory",
            "management": {
                "enabled": true,
                "allow_self_binding": true,
                "command": "bili",
                "admin_user_ids": ["admin"],
                "self_binding_notifications": ["dynamic"],
                "self_binding_outbound_binding": "qq-main"
            }
        });
        let error = match BilibiliConfiguredPlugin.prepare(
            &config,
            ServiceRuntimeBuilder::new(ServiceConfig::default()),
        ) {
            Ok(_) => panic!("Bilibili management unexpectedly accepted missing Host stores"),
            Err(error) => error,
        };
        assert!(error.contains("security.secret_file"));
    }

    #[test]
    fn configured_bilibili_open_platform_requires_rotatable_oauth_store() {
        let config = json!({
            "backend": {
                "type": "open_platform",
                "client_id": "client",
                "app_secret_key": "BILIBILI_OPEN_APP_SECRET",
                "oauth_credential_key": "BILIBILI_OPEN_OAUTH",
                "authorized_uid": 42
            },
            "live_interval_ms": 1000,
            "dynamic_interval_ms": 1000,
            "video_interval_ms": 1000,
            "retry": {"max_attempts": 3, "initial_backoff_ms": 10, "max_backoff_ms": 100},
            "subscriptions": [],
            "link_resolver": {"enabled": false, "cooldown_ms": 1000, "account_to_binding": {}},
            "media_provider_id": "memory",
            "management": {
                "enabled": false,
                "allow_self_binding": false,
                "command": "bili",
                "admin_user_ids": [],
                "self_binding_notifications": ["live", "video"],
                "self_binding_outbound_binding": ""
            }
        });
        let error = BilibiliConfiguredPlugin
            .prepare(
                &config,
                ServiceRuntimeBuilder::new(ServiceConfig::default()),
            )
            .err()
            .expect("Open Platform unexpectedly accepted a non-rotatable secret store");
        assert!(error.contains("OAuth refresh"));
    }
}
