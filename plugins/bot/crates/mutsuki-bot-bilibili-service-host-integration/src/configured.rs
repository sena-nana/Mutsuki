use std::sync::Arc;

use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_management::{BilibiliCredentialSecretState, BilibiliManagementApi};
use mutsuki_bot_sdk::BotSubmissionGate;
use mutsuki_bot_service_host_integration::{BILIBILI_MANAGEMENT_SERVICE_ID, ConfigProviderEffect};
use mutsuki_config_service::{
    ConfigApplyMode, ConfigApplyRequest, ConfigContext, ConfigService, ConfigValue,
    MemoryConfigProvider, capability,
};
use mutsuki_plugin_bot_bilibili::{
    BilibiliBackendConfig, BilibiliConfig, BilibiliConfigStore, BilibiliCredentialStore,
    BilibiliManagementService, BilibiliRunner, BilibiliSecretPresence, BilibiliTransport,
    HttpBilibiliTransport, PLUGIN_ID as BILIBILI_PLUGIN_ID, RuntimeBilibiliHttp,
    RuntimeBilibiliQrRenderer, SharedBilibiliConfig, SqliteBilibiliRepository,
    bilibili_config_descriptor, bilibili_config_value,
};
use mutsuki_runtime_contracts::{ContractSurfaceKind, SurfaceRequirement};
use mutsuki_runtime_sdk::{
    HostEffectKind, LoadedPlugin, RuntimeBootstrapperEffect, RuntimeBootstrapperService,
};
use mutsuki_service_config::HostSecretStore;
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use serde_json::Value;

use crate::{BilibiliPollingCredentials, BilibiliPollingEventSource};

pub struct BilibiliConfiguredPlugin {
    config_service: Option<Arc<ConfigService>>,
    flow_registry: Option<Arc<BotFlowRegistry>>,
    media_provider_id: String,
}

impl BilibiliConfiguredPlugin {
    #[must_use]
    pub fn new(config_service: Option<Arc<ConfigService>>, media_provider_id: String) -> Self {
        Self {
            config_service,
            flow_registry: None,
            media_provider_id,
        }
    }

    /// Shares the Flow registry so the push poll path and the management
    /// status observe the same active graph the router activates.
    #[must_use]
    pub fn with_flow_registry(mut self, registry: Arc<BotFlowRegistry>) -> Self {
        self.flow_registry = Some(registry);
        self
    }
}

struct HostBilibiliCredentialStore(HostSecretStore);

impl BilibiliCredentialStore for HostBilibiliCredentialStore {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
        self.0
            .rotate(key, credential)
            .map_err(|error| error.to_string())
    }
}

struct ConfigServiceBilibiliConfigStore(Arc<ConfigService>);

impl BilibiliConfigStore for ConfigServiceBilibiliConfigStore {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String> {
        let service = self.0.clone();
        let config = config.clone();
        mutsuki_service_runtime::block_on_host_future(async move {
            let snapshot = service
                .read(
                    BILIBILI_PLUGIN_ID,
                    ConfigContext::global(),
                    &[capability::VALUE_READ.into()],
                )
                .await?;
            let stored = snapshot.value.to_json();
            // The product-owned document wraps the runtime config with the
            // enable switch and projected UI fields; legacy documents hold the
            // bare runtime config.
            let candidate = match stored.get("enabled").and_then(Value::as_bool) {
                Some(enabled) => bilibili_config_value(enabled, &config),
                None => ConfigValue::from_json(
                    &serde_json::to_value(&config).expect("Bilibili config serializes"),
                ),
            };
            service
                .apply(
                    BILIBILI_PLUGIN_ID,
                    ConfigApplyRequest {
                        candidate,
                        expected_revision: snapshot.revision,
                        dry_run: false,
                    },
                    ConfigContext::global(),
                    &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
                )
                .await
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
    }
}

struct HostSecretPresence(HostSecretStore);

impl BilibiliSecretPresence for HostSecretPresence {
    fn inspect(&self, key: &str) -> BilibiliCredentialSecretState {
        match self.0.resolve(key) {
            None => BilibiliCredentialSecretState::Absent,
            Some(value) if value.trim().is_empty() => BilibiliCredentialSecretState::Invalid,
            Some(_) => BilibiliCredentialSecretState::Present,
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
        let mut config: BilibiliConfig =
            serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
        // The product assembly owns the media resource provider binding; the
        // owner document no longer carries a provider id.
        config.media_provider_id.clone_from(&self.media_provider_id);
        // The product pre-registers the owner config provider before plugins
        // install; only fall back to a plugin-owned provider when no product
        // owns this provider id yet.
        let config_provider = self.config_service.as_ref().and_then(|service| {
            service
                .registry()
                .get(BILIBILI_PLUGIN_ID)
                .is_err()
                .then(|| {
                    Arc::new(MemoryConfigProvider::new(
                        bilibili_config_descriptor(),
                        ConfigValue::from_json(
                            &serde_json::to_value(&config).expect("Bilibili config serializes"),
                        ),
                        ConfigApplyMode::HotReload,
                    ))
                })
                .map(|provider| (service.clone(), provider))
        });
        if let Some((service, provider)) = &config_provider {
            let service = service.clone();
            let provider = provider.clone();
            let seed = ConfigValue::from_json(
                &serde_json::to_value(&config).expect("Bilibili config serializes"),
            );
            let snapshot = mutsuki_service_runtime::block_on_host_future(async move {
                service
                    .prepare_provider_candidate(provider, Some(seed), ConfigContext::global())
                    .await
            })
            .map_err(|error| error.to_string())?;
            config = serde_json::from_value(snapshot.value.to_json())
                .map_err(|error| error.to_string())?;
        }
        config.validate()?;
        let host_secret_store = builder.host_secret_store();
        if config.management.enabled && !host_secret_store.rotation_available() {
            return Err("Bilibili management requires a Host security.secret_file".into());
        }
        let config_service = if config.management.enabled {
            Some(
                self.config_service
                    .clone()
                    .ok_or_else(|| "Bilibili management requires ConfigService".to_string())?,
            )
        } else {
            None
        };
        let repository = Arc::new(
            SqliteBilibiliRepository::open(builder.data_dir().join("bilibili/state.sqlite3"))
                .map_err(|error| error.to_string())?,
        );
        let shared_config = SharedBilibiliConfig::new(config);
        let runner_config = shared_config.clone();
        let runner_repository = repository.clone();
        // One transport for the runner and the Web Console so they share the WBI key cache.
        let transport: Arc<dyn BilibiliTransport> =
            Arc::new(HttpBilibiliTransport::new(shared_config.clone()));
        let runner_transport = transport.clone();
        let source_credentials = match &shared_config.snapshot().backend {
            BilibiliBackendConfig::WebCookie { cookie_secret_key } => {
                BilibiliPollingCredentials::WebCookie {
                    secret_key: cookie_secret_key.clone(),
                    required: !shared_config.snapshot().management.enabled,
                }
            }
        };
        let source = BilibiliPollingEventSource::new(shared_config.clone(), source_credentials);
        let manifest_config = runner_config.snapshot();
        let mut manifest = mutsuki_plugin_bot_bilibili::manifest_for_config(&manifest_config);
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            runner_config.snapshot().media_provider_id,
        ));
        BotSubmissionGate::ensure_manifest_business_surface(&manifest)
            .map_err(|error| error.to_string())?;

        let management_service = if let Some(config_service) = config_service {
            let mut service = BilibiliManagementService::new(
                runner_config.clone(),
                transport,
                repository.clone(),
                Arc::new(HostBilibiliCredentialStore(host_secret_store.clone())),
                Arc::new(ConfigServiceBilibiliConfigStore(config_service)),
                Arc::new(HostSecretPresence(host_secret_store.clone())),
            );
            if let Some(registry) = self.flow_registry.clone() {
                service = service.with_flow_registry(registry);
            }
            Some(Arc::new(service))
        } else {
            None
        };

        if management_service.is_some() {
            manifest
                .provides
                .services
                .push(BILIBILI_MANAGEMENT_SERVICE_ID.into());
            manifest
                .provides
                .capabilities
                .push("bot.bilibili.management".into());
        }
        let loaded_manifest = manifest.clone();
        let management_api = management_service
            .clone()
            .map(|service| service as Arc<dyn BilibiliManagementApi>);
        let runner_flow_registry = self.flow_registry.clone();
        let builder = builder.register_builtin_loaded_plugin_factory(manifest, move || {
            let host_services = management_api
                .as_ref()
                .map(|management_api| {
                    RuntimeBootstrapperService::new(
                        BILIBILI_MANAGEMENT_SERVICE_ID,
                        Arc::new(management_api.clone()),
                        "bot.bilibili.management",
                    )
                })
                .into_iter()
                .collect();
            let host_effects = config_provider
                .as_ref()
                .map(|(service, provider)| {
                    service
                        .register_provider_staged(provider.clone())
                        .map(|registration| RuntimeBootstrapperEffect {
                            kind: HostEffectKind::HostLocal,
                            effect: Box::new(ConfigProviderEffect(registration)),
                        })
                        .map_err(|error| error.to_string())
                })
                .transpose()?
                .into_iter()
                .collect();
            Ok::<LoadedPlugin, String>(LoadedPlugin {
                manifest: loaded_manifest.clone(),
                runners: Vec::new(),
                async_handlers: Vec::new(),
                host_services,
                resource_providers: Vec::new(),
                async_resource_providers: Vec::new(),
                host_effects,
            })
        });

        Ok(
            builder
                .register_fallible_runtime_services_runner(move |client, resources| {
                    let snapshot = runner_config.snapshot();
                    let mut runner = BilibiliRunner::new(
                        runner_transport.clone(),
                        runner_repository.clone(),
                        resources.clone(),
                        snapshot.media_provider_id.clone(),
                    );
                    if snapshot.management.enabled {
                        let management = management_service.clone().ok_or_else(|| {
                            mutsuki_plugin_bot_bilibili::BilibiliError::ManagementUnavailable(
                                "Bilibili management service is unavailable".into(),
                            )
                        })?;
                        management.bind_qr_renderer(Arc::new(RuntimeBilibiliQrRenderer::new(
                            client.clone(),
                            resources,
                        )));
                        management.bind_http(Arc::new(RuntimeBilibiliHttp::new(client.clone())));
                        runner = runner.with_management(runner_config.clone(), management);
                    }
                    if let Some(registry) = runner_flow_registry.clone() {
                        runner = runner.with_flow_registry(registry);
                    }
                    Ok::<
                        Box<dyn mutsuki_runtime_sdk::Runner>,
                        mutsuki_plugin_bot_bilibili::BilibiliError,
                    >(
                        runner.into_runtime_runner(client, snapshot.risk_control.clone())
                    )
                })
                .register_event_source(Box::new(source)),
        )
    }
}

/// The Bilibili owner's configured plugin; merge it with the core Bot catalog. `config` enables
/// the management console and `flow_registry` lets the push poll observe the active graph.
pub fn configured_bilibili_plugin_catalog(
    config: Option<Arc<ConfigService>>,
    flow_registry: Option<Arc<BotFlowRegistry>>,
    media_provider_id: impl Into<String>,
) -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut plugin = BilibiliConfiguredPlugin::new(config, media_provider_id.into());
    if let Some(registry) = flow_registry {
        plugin = plugin.with_flow_registry(registry);
    }
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(plugin)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use mutsuki_service_config::ServiceConfig;
    use serde_json::json;

    use super::*;

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
                "admin_user_ids": ["admin"],
                "self_binding_notifications": ["dynamic"],
                "self_binding_outbound_binding": "qq-main"
            }
        });
        let error = match BilibiliConfiguredPlugin::new(
            None,
            mutsuki_bot_service_host_integration::DEFAULT_MEDIA_PROVIDER_ID.to_string(),
        )
        .prepare(
            &config,
            ServiceRuntimeBuilder::new(ServiceConfig::default()),
        ) {
            Ok(_) => panic!("Bilibili management unexpectedly accepted missing Host stores"),
            Err(error) => error,
        };
        assert!(error.contains("security.secret_file"));
    }
}
