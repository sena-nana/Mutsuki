use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_runtime_sdk::ResourceRegistryGateway;
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

use mutsuki_plugin_bot_adapter_qqbot::{
    QqAuthManager, QqBotClients, QqBotConfig, QqGatewayMapRunner, QqGatewayMediaHandler,
    QqIdSource, QqMediaProvider, QqOpenApiError, QqOpenApiRunner, ReqwestQqHttpClient,
    ResourceGatewayQqMediaProvider, SharedQqCredentials, qqbot_adapter_manifest,
};
use mutsuki_plugin_bot_qq_web::{
    LocalQqManagementProvider, QqBotManagementService, QqManagementAction, QqManagementError,
    QqManagementProvider, account_view_from_config,
};
use mutsuki_runtime_sdk::{LoadedPlugin, RuntimeBootstrapperService};
use serde_json::Value;

use crate::console_bridge::QQ_MANAGEMENT_SERVICE_ID;
use crate::event_source::{QqGatewayEventSource, QqGatewayHealthHandle};

type MediaFactory = Arc<
    dyn Fn(Arc<dyn ResourceRegistryGateway>) -> Result<Box<dyn QqMediaProvider>, String>
        + Send
        + Sync,
>;
type IdFactory = Arc<dyn Fn() -> Box<dyn QqIdSource> + Send + Sync>;

/// Complete product bundle for the `hosts/service` assembly.
///
/// The bundle owns only adapter configuration and shared account state. The
/// client secret is populated by `QqGatewayEventSource` through the Host secret
/// boundary when the ServiceRuntime starts.
pub struct QqBotPluginBundle {
    config: QqBotConfig,
    credentials: SharedQqCredentials,
    auth: QqAuthManager,
    health: QqGatewayHealthHandle,
    event_source: Option<QqGatewayEventSource>,
    media_factory: Option<MediaFactory>,
    media_provider_id: Option<String>,
    id_factory: IdFactory,
    management: Arc<QqBotManagementService>,
    local_management: Arc<LocalQqManagementProvider>,
}

impl QqBotPluginBundle {
    /// Builds the text/recall/account/Gateway bundle without declaring media upload.
    pub fn new(config: QqBotConfig) -> Result<Self, QqOpenApiError> {
        config
            .validate()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        let credentials = SharedQqCredentials::default();
        let auth = QqAuthManager::new();
        let event_source =
            QqGatewayEventSource::new(config.clone(), credentials.clone(), auth.clone());
        let health = event_source.health_handle();
        let local_management = Arc::new(LocalQqManagementProvider::new());
        local_management.upsert_account(account_view_from_config(
            &config.account_id,
            &config.client_secret_key,
            false,
            config.capability_matrix(),
            config.gateway_intents,
            config.shard,
            false,
            false,
            None,
            None,
        ));
        let provider = Arc::new(GatewayBackedQqManagementProvider {
            local: local_management.clone(),
            health: health.clone(),
            account_id: config.account_id.clone(),
            secret_key: config.client_secret_key.clone(),
            capability: config.capability_matrix(),
            intents: config.gateway_intents,
            shard: config.shard,
        });
        let management = Arc::new(QqBotManagementService::new(provider));
        Ok(Self {
            config,
            credentials,
            auth,
            health,
            event_source: Some(event_source),
            media_factory: None,
            media_provider_id: None,
            id_factory: Arc::new(|| Box::new(SystemQqIdSource::new())),
            management,
            local_management,
        })
    }

    /// Enables media upload only when a real resource provider is available.
    pub fn with_media_provider<F>(mut self, media_factory: F) -> Self
    where
        F: Fn() -> Box<dyn QqMediaProvider> + Send + Sync + 'static,
    {
        self.media_factory = Some(Arc::new(move |_resources| Ok(media_factory())));
        self
    }

    pub fn with_resource_media_provider(mut self, provider_id: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        self.media_provider_id = Some(provider_id.clone());
        self.media_factory = Some(Arc::new(move |resources| {
            ResourceGatewayQqMediaProvider::new(provider_id.clone(), resources)
                .map(|provider| Box::new(provider) as Box<dyn QqMediaProvider>)
                .map_err(|error| error.to_string())
        }));
        self
    }

    pub fn with_id_source_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Box<dyn QqIdSource> + Send + Sync + 'static,
    {
        self.id_factory = Arc::new(factory);
        self
    }

    pub fn health_handle(&self) -> QqGatewayHealthHandle {
        self.health.clone()
    }

    pub fn management_service(&self) -> Arc<QqBotManagementService> {
        self.management.clone()
    }

    pub fn local_management(&self) -> Arc<LocalQqManagementProvider> {
        self.local_management.clone()
    }

    pub fn install(
        mut self,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, QqOpenApiError> {
        let gateway_config = self.config.clone();
        let openapi_config = self.config.clone();
        let credentials = self.credentials.clone();
        let auth = self.auth.clone();
        let media_factory = self.media_factory.clone();
        let media_provider_id = self.media_provider_id.clone();
        let media_enabled = media_factory.is_some();
        let id_factory = self.id_factory.clone();
        let health = self.health.clone();
        let management = self.management.clone();
        let health_component_id = format!("mutsuki.bot.qqbot.gateway:{}", self.config.account_id);
        let source = self
            .event_source
            .take()
            .ok_or_else(|| QqOpenApiError::InvalidPayload("event source already taken".into()))?;
        let mut manifest = qqbot_adapter_manifest(1, media_enabled);
        if let Some(provider_id) = media_provider_id {
            manifest
                .requires
                .push(format!("resource_strategy:{provider_id}"));
        }
        let loaded_manifest = manifest.clone();
        let builder = builder.register_builtin_loaded_plugin_factory(manifest, move || {
            Ok::<LoadedPlugin, String>(LoadedPlugin {
                manifest: loaded_manifest.clone(),
                runners: Vec::new(),
                async_handlers: Vec::new(),
                host_services: vec![RuntimeBootstrapperService {
                    service_id: QQ_MANAGEMENT_SERVICE_ID.into(),
                    capability: None,
                    service: management.clone(),
                }],
                resource_providers: Vec::new(),
                async_resource_providers: Vec::new(),
            })
        });
        let builder = if media_enabled {
            builder.register_fallible_runtime_services_async_handler(move |_runtime, resources| {
                QqGatewayMediaHandler::new(gateway_config.clone(), resources).map(|handler| {
                    Arc::new(handler) as Arc<dyn mutsuki_runtime_core::AsyncBatchHandler>
                })
            })
        } else {
            builder.register_builtin_runner(move || {
                Box::new(QqGatewayMapRunner::new(
                    1,
                    gateway_config.account_id.clone(),
                ))
            })
        };
        Ok(builder
            .register_fallible_runtime_services_runner(move |_runtime, resources| {
                let http = ReqwestQqHttpClient::new(&openapi_config)
                    .map_err(|error| error.redacted_message())?;
                Ok::<Box<dyn mutsuki_runtime_core::Runner>, String>(Box::new(
                    QqOpenApiRunner::new_with_auth(
                        1,
                        openapi_config.clone(),
                        {
                            let clients =
                                QqBotClients::new(Box::new(http), Arc::new(credentials.clone()));
                            match &media_factory {
                                Some(factory) => clients.with_media_provider(factory(resources)?),
                                None => clients,
                            }
                        },
                        id_factory(),
                        auth.clone(),
                    ),
                ))
            })
            .register_health_probe(health_component_id, move || {
                let snapshot = health.snapshot();
                json!({
                    "status": if snapshot.connected && snapshot.identified {
                        "ok"
                    } else if snapshot.connected {
                        "degraded"
                    } else {
                        "unhealthy"
                    },
                    "connected": snapshot.connected,
                    "identified": snapshot.identified,
                    "last_heartbeat_unix_ms": snapshot.last_heartbeat_unix_ms,
                    "last_ack_unix_ms": snapshot.last_ack_unix_ms,
                    "last_event_unix_ms": snapshot.last_event_unix_ms,
                    "reconnect_count": snapshot.reconnect_count,
                    "last_error": snapshot.last_error,
                    "started_at_unix_ms": snapshot.started_at_unix_ms,
                    "connected_since_unix_ms": snapshot.connected_since_unix_ms,
                })
            })
            .register_event_source(Box::new(source)))
    }
}

struct GatewayBackedQqManagementProvider {
    local: Arc<LocalQqManagementProvider>,
    health: QqGatewayHealthHandle,
    account_id: String,
    secret_key: String,
    capability: mutsuki_bot_protocol::QqBotCapabilityMatrix,
    intents: u64,
    shard: [u64; 2],
}

impl GatewayBackedQqManagementProvider {
    fn refresh_account(&self) {
        let snapshot = self.health.snapshot();
        self.local.upsert_account(account_view_from_config(
            &self.account_id,
            &self.secret_key,
            true,
            self.capability.clone(),
            self.intents,
            self.shard,
            snapshot.connected,
            snapshot.identified,
            snapshot
                .last_heartbeat_unix_ms
                .map(|value| value.min(u128::from(u64::MAX)) as u64),
            snapshot.last_error.as_deref(),
        ));
    }
}

impl QqManagementProvider for GatewayBackedQqManagementProvider {
    fn load_snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<mutsuki_plugin_bot_qq_web::QqBotManagementSnapshot, QqManagementError> {
        self.refresh_account();
        self.local.load_snapshot(query, include_secret_status)
    }

    fn apply(
        &self,
        actor_id: &str,
        action: &QqManagementAction,
    ) -> Result<Value, QqManagementError> {
        self.refresh_account();
        self.local.apply(actor_id, action)
    }
}

struct SystemQqIdSource {
    next: u16,
}

impl SystemQqIdSource {
    fn new() -> Self {
        let next = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u16)
            .unwrap_or(1);
        Self { next }
    }

    #[cfg(test)]
    fn from_seed(next: u16) -> Self {
        Self { next }
    }
}

impl QqIdSource for SystemQqIdSource {
    fn next_msg_seq(&mut self) -> u64 {
        let current = u64::from(self.next);
        self.next = self.next.wrapping_add(1);
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_message_sequence_stays_within_qq_unsigned_16_bit_range() {
        let mut source = SystemQqIdSource::from_seed(u16::MAX);

        assert_eq!(source.next_msg_seq(), u64::from(u16::MAX));
        assert_eq!(source.next_msg_seq(), 0);
        assert_eq!(source.next_msg_seq(), 1);
    }
}
