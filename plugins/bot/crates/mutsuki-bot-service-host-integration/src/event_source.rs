//! ServiceHost event source for the QQ Gateway.
//!
//! The gateway client itself lives in `mutsuki-plugin-bot-adapter-qqbot`; this wrapper owns
//! only what is ServiceHost-specific: the descriptor, resolving the app secret from the Host
//! secret store, bridging Host shutdown/logging, and mapping gateway health.

use std::sync::Arc;

use mutsuki_plugin_bot_adapter_qqbot::gateway::{
    QqGatewayClient, QqGatewayControlHandle, QqGatewayHealthHandle, QqGatewayHealthSnapshot,
    QqGatewayHost, QqInboundObserveHandle, safe_gateway_id,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    QQBOT_ADAPTER_PLUGIN_ID, QqAuthManager, QqBotConfig, SharedQqCredentials,
};
use mutsuki_service_runtime::{
    HostEventSource, HostEventSourceContext, HostEventSourceDescriptor, HostEventSourceFuture,
    HostEventSourceHealth,
};

pub const QQBOT_GATEWAY_SOURCE_ID: &str = "mutsuki.bot.adapter.qqbot.gateway.source";

pub struct QqGatewayEventSource {
    descriptor: HostEventSourceDescriptor,
    client: QqGatewayClient,
}

impl QqGatewayEventSource {
    pub fn new(config: QqBotConfig, credentials: SharedQqCredentials, auth: QqAuthManager) -> Self {
        let instance_id = format!("qqbot-gateway:{}", config.account_id);
        let source_id = format!(
            "{QQBOT_GATEWAY_SOURCE_ID}:{}",
            safe_gateway_id(&config.account_id)
        );
        Self {
            descriptor: HostEventSourceDescriptor::new(source_id, QQBOT_ADAPTER_PLUGIN_ID)
                .with_instance_id(instance_id)
                .require_secret(config.client_secret_key.clone()),
            client: QqGatewayClient::new(config, credentials, auth),
        }
    }

    pub fn health_handle(&self) -> QqGatewayHealthHandle {
        self.client.health_handle()
    }

    pub fn control_handle(&self) -> QqGatewayControlHandle {
        self.client.control_handle()
    }

    #[must_use]
    pub fn inbound_handle(&self) -> QqInboundObserveHandle {
        self.client.inbound_handle()
    }
}

impl HostEventSource for QqGatewayEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        let config = self.client.config();
        // Validation runs before the secret lookup, as it did when both lived here.
        if let Err(error) = config.validate() {
            return Box::pin(async move { Err(source_error(error)) });
        }
        let Some(secret) = ctx
            .config
            .secret(&config.client_secret_key)
            .filter(|secret| !secret.is_empty())
        else {
            let message = format!(
                "missing Host secret {} for QQBot account {}",
                config.client_secret_key, config.account_id
            );
            return Box::pin(async move { Err(source_error(message)) });
        };
        let events = ctx.events.clone();
        let host = QqGatewayHost {
            task_submitter: ctx.task_submitter.clone(),
            shutdown: ctx.shutdown.receiver(),
            log: Arc::new(move |level, message, correlation| {
                events.log(level, message, correlation);
            }),
        };
        self.client.start(host, secret)
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        self.client.shutdown()
    }

    fn health(&self) -> HostEventSourceHealth {
        host_health(self.client.health_handle().snapshot())
    }
}

fn host_health(health: QqGatewayHealthSnapshot) -> HostEventSourceHealth {
    if health.connected && health.identified {
        HostEventSourceHealth::Healthy
    } else if health.connected {
        HostEventSourceHealth::Degraded(
            health
                .last_error
                .unwrap_or_else(|| "QQBot Gateway is connected but not identified".into()),
        )
    } else {
        HostEventSourceHealth::Unhealthy(
            health
                .last_error
                .unwrap_or_else(|| "QQBot Gateway is disconnected".into()),
        )
    }
}

fn source_error(
    error: impl std::fmt::Display,
) -> Box<dyn std::error::Error + Send + Sync + 'static> {
    Box::new(std::io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_health_maps_connection_and_identify_state() {
        assert!(matches!(
            host_health(QqGatewayHealthSnapshot::default()),
            HostEventSourceHealth::Unhealthy(_)
        ));
        assert!(matches!(
            host_health(QqGatewayHealthSnapshot {
                connected: true,
                ..QqGatewayHealthSnapshot::default()
            }),
            HostEventSourceHealth::Degraded(_)
        ));
        assert_eq!(
            host_health(QqGatewayHealthSnapshot {
                connected: true,
                identified: true,
                ..QqGatewayHealthSnapshot::default()
            }),
            HostEventSourceHealth::Healthy
        );
        assert!(matches!(
            host_health(QqGatewayHealthSnapshot {
                last_error: Some("heartbeat ACK timed out".into()),
                ..QqGatewayHealthSnapshot::default()
            }),
            HostEventSourceHealth::Unhealthy(ref reason) if reason == "heartbeat ACK timed out"
        ));
    }
}
