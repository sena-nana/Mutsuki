use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_runtime_sdk::ResourceRegistryGateway;
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

use mutsuki_plugin_bot_adapter_qqbot::{
    QqAuthManager, QqBotClients, QqBotConfig, QqGatewayMapRunner, QqIdSource, QqMediaProvider,
    QqOpenApiError, QqOpenApiRunner, ReqwestQqHttpClient, ResourceGatewayQqMediaProvider,
    SharedQqCredentials, qqbot_adapter_manifest,
};

use crate::event_source::{QqGatewayEventSource, QqGatewayHealthHandle};

type MediaFactory = Arc<
    dyn Fn(Arc<dyn ResourceRegistryGateway>) -> Result<Box<dyn QqMediaProvider>, String>
        + Send
        + Sync,
>;
type IdFactory = Arc<dyn Fn() -> Box<dyn QqIdSource> + Send + Sync>;

/// Complete product bundle for `MutsukiServiceHost` assembly.
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
        Ok(Self {
            config,
            credentials,
            auth,
            health,
            event_source: Some(event_source),
            media_factory: None,
            media_provider_id: None,
            id_factory: Arc::new(|| Box::new(SystemQqIdSource::new())),
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
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_builtin_runner(move || {
                Box::new(QqGatewayMapRunner::new(
                    1,
                    gateway_config.account_id.clone(),
                ))
            })
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
                })
            })
            .register_event_source(Box::new(source)))
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
