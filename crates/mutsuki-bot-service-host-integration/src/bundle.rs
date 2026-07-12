use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_runtime_contracts::PluginManifest;
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

use mutsuki_plugin_bot_adapter_qqbot::{
    QqAuthManager, QqBotClients, QqBotConfig, QqGatewayMapRunner, QqIdSource, QqMediaProvider,
    QqOpenApiError, QqOpenApiRunner, ReqwestQqHttpClient, SharedQqCredentials,
    qqbot_adapter_manifest,
};

use crate::event_source::{QqGatewayEventSource, QqGatewayHealthHandle};

type MediaFactory = Arc<dyn Fn() -> Box<dyn QqMediaProvider> + Send + Sync>;
type IdFactory = Arc<dyn Fn() -> Box<dyn QqIdSource> + Send + Sync>;

/// Complete product bundle for `MutsukiServiceHost` assembly.
///
/// The bundle owns only adapter configuration and shared account state. The
/// client secret is populated by `QqGatewayEventSource` through the Host secret
/// boundary when the ServiceRuntime starts.
pub struct QqBotPluginBundle {
    manifest: PluginManifest,
    config: QqBotConfig,
    credentials: SharedQqCredentials,
    auth: QqAuthManager,
    health: QqGatewayHealthHandle,
    event_source: Option<QqGatewayEventSource>,
    media_factory: MediaFactory,
    id_factory: IdFactory,
}

impl QqBotPluginBundle {
    pub fn new<F>(config: QqBotConfig, media_factory: F) -> Result<Self, QqOpenApiError>
    where
        F: Fn() -> Box<dyn QqMediaProvider> + Send + Sync + 'static,
    {
        config
            .validate()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        let credentials = SharedQqCredentials::default();
        let auth = QqAuthManager::new();
        let event_source =
            QqGatewayEventSource::new(config.clone(), credentials.clone(), auth.clone());
        let health = event_source.health_handle();
        Ok(Self {
            manifest: qqbot_adapter_manifest(1),
            config,
            credentials,
            auth,
            health,
            event_source: Some(event_source),
            media_factory: Arc::new(media_factory),
            id_factory: Arc::new(|| Box::new(SystemQqIdSource::new())),
        })
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
        let id_factory = self.id_factory.clone();
        let health = self.health.clone();
        let health_component_id = format!("mutsuki.bot.qqbot.gateway:{}", self.config.account_id);
        let source = self
            .event_source
            .take()
            .ok_or_else(|| QqOpenApiError::InvalidPayload("event source already taken".into()))?;
        Ok(builder
            .register_builtin_plugin(self.manifest)
            .register_builtin_runner(move || {
                Box::new(QqGatewayMapRunner::new(
                    1,
                    gateway_config.account_id.clone(),
                ))
            })
            .register_fallible_builtin_runner(move || {
                let http = ReqwestQqHttpClient::new(&openapi_config)
                    .map_err(|error| error.redacted_message())?;
                Ok::<Box<dyn mutsuki_runtime_core::Runner>, String>(Box::new(
                    QqOpenApiRunner::new_with_auth(
                        1,
                        openapi_config.clone(),
                        QqBotClients::new(
                            Box::new(http),
                            media_factory(),
                            Arc::new(credentials.clone()),
                        ),
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
    next: u64,
}

impl SystemQqIdSource {
    fn new() -> Self {
        let next = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(1);
        Self { next }
    }
}

impl QqIdSource for SystemQqIdSource {
    fn next_msg_seq(&mut self) -> u64 {
        let current = self.next;
        self.next = self.next.saturating_add(1);
        current
    }
}
