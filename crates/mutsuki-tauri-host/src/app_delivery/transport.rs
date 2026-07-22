use super::types::{AppDeliveryError, AppId};
use mutsuki_link_core::Connection;
use mutsuki_runtime_contracts::{
    CapabilityDescriptor, CapabilityRequestEnvelope, DeliveryReceipt, IdempotentReceiptStore,
};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Clone, Debug)]
pub struct AppLinkSession {
    pub app_id: AppId,
    pub instance_id: String,
    pub host_protocol_version: u32,
    pub capabilities: Vec<CapabilityDescriptor>,
}

impl AppLinkSession {
    pub fn capability_ready(&self, required: &CapabilityDescriptor) -> bool {
        self.capabilities
            .iter()
            .any(|offered| required.is_compatible_with(offered))
    }
}

pub trait AppLinkTransport: Send + Sync {
    fn try_connect(
        &self,
        app_id: &AppId,
    ) -> impl std::future::Future<Output = Result<AppLinkSession, AppDeliveryError>> + Send;

    fn wait_capability_ready(
        &self,
        session: &AppLinkSession,
        capability: &CapabilityDescriptor,
        timeout: Duration,
    ) -> impl std::future::Future<Output = Result<AppLinkSession, AppDeliveryError>> + Send;

    fn transmit(
        &self,
        session: &AppLinkSession,
        envelope: &CapabilityRequestEnvelope,
    ) -> impl std::future::Future<Output = Result<DeliveryReceipt, AppDeliveryError>> + Send;

    fn query_receipt(
        &self,
        session: &AppLinkSession,
        request_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<DeliveryReceipt>, AppDeliveryError>> + Send;
}

#[derive(Clone, Default)]
struct MemoryPeer {
    online: bool,
    host_protocol_version: u32,
    capabilities: Vec<CapabilityDescriptor>,
    ready_after: Option<Duration>,
    force_error: Option<AppDeliveryError>,
    receipts: IdempotentReceiptStore,
}

/// Injectable in-memory transport for pure Rust delivery state-machine tests.
#[derive(Clone, Default)]
pub struct InMemoryAppLinkTransport {
    peers: Arc<Mutex<BTreeMap<String, MemoryPeer>>>,
}

impl InMemoryAppLinkTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_online(&self, app_id: &AppId, capabilities: Vec<CapabilityDescriptor>) {
        self.peers.lock().insert(
            app_id.as_str().to_string(),
            MemoryPeer {
                online: true,
                host_protocol_version: 1,
                capabilities,
                ..MemoryPeer::default()
            },
        );
    }

    pub fn register_offline(
        &self,
        app_id: &AppId,
        capabilities: Vec<CapabilityDescriptor>,
        ready_after: Duration,
    ) {
        self.peers.lock().insert(
            app_id.as_str().to_string(),
            MemoryPeer {
                online: false,
                host_protocol_version: 1,
                capabilities,
                ready_after: Some(ready_after),
                ..MemoryPeer::default()
            },
        );
    }

    pub fn set_force_error(&self, app_id: &AppId, error: AppDeliveryError) {
        if let Some(peer) = self.peers.lock().get_mut(app_id.as_str()) {
            peer.force_error = Some(error);
        }
    }

    pub fn mark_online(&self, app_id: &AppId) {
        if let Some(peer) = self.peers.lock().get_mut(app_id.as_str()) {
            peer.online = true;
        }
    }
}

impl AppLinkTransport for InMemoryAppLinkTransport {
    async fn try_connect(&self, app_id: &AppId) -> Result<AppLinkSession, AppDeliveryError> {
        let peers = self.peers.lock();
        let Some(peer) = peers.get(app_id.as_str()) else {
            return Err(AppDeliveryError::AppNotInstalled);
        };
        if let Some(error) = peer.force_error.clone() {
            return Err(error);
        }
        if !peer.online {
            return Err(AppDeliveryError::EndpointUnavailable);
        }
        Ok(AppLinkSession {
            app_id: app_id.clone(),
            instance_id: format!("memory-{}", app_id.as_str()),
            host_protocol_version: peer.host_protocol_version,
            capabilities: if peer.ready_after.is_some_and(|delay| delay > Duration::ZERO) {
                Vec::new()
            } else {
                peer.capabilities.clone()
            },
        })
    }

    async fn wait_capability_ready(
        &self,
        session: &AppLinkSession,
        capability: &CapabilityDescriptor,
        timeout: Duration,
    ) -> Result<AppLinkSession, AppDeliveryError> {
        let started = tokio::time::Instant::now();
        loop {
            {
                let peers = self.peers.lock();
                let peer = peers
                    .get(session.app_id.as_str())
                    .ok_or(AppDeliveryError::EndpointUnavailable)?;
                if let Some(error) = peer.force_error.clone() {
                    return Err(error);
                }
                if peer
                    .capabilities
                    .iter()
                    .any(|offered| capability.is_compatible_with(offered))
                    && peer
                        .ready_after
                        .is_none_or(|delay| started.elapsed() >= delay)
                {
                    return Ok(AppLinkSession {
                        app_id: session.app_id.clone(),
                        instance_id: session.instance_id.clone(),
                        host_protocol_version: peer.host_protocol_version,
                        capabilities: peer.capabilities.clone(),
                    });
                }
            }
            if started.elapsed() >= timeout {
                return Err(AppDeliveryError::ReadyTimeout);
            }
            sleep(Duration::from_millis(5)).await;
        }
    }

    async fn transmit(
        &self,
        session: &AppLinkSession,
        envelope: &CapabilityRequestEnvelope,
    ) -> Result<DeliveryReceipt, AppDeliveryError> {
        let mut peers = self.peers.lock();
        let peer = peers
            .get_mut(session.app_id.as_str())
            .ok_or(AppDeliveryError::EndpointUnavailable)?;
        if let Some(error) = peer.force_error.clone() {
            return Err(error);
        }
        if !peer
            .capabilities
            .iter()
            .any(|offered| envelope.capability.is_compatible_with(offered))
        {
            return Err(AppDeliveryError::CapabilityUnavailable);
        }
        let receipt = DeliveryReceipt::Accepted {
            request_id: envelope.request_id.clone(),
            remote_task_id: Some(format!("task-{}", envelope.request_id)),
        };
        Ok(peer
            .receipts
            .accept_or_duplicate(envelope.request_id.clone(), receipt))
    }

    async fn query_receipt(
        &self,
        session: &AppLinkSession,
        request_id: &str,
    ) -> Result<Option<DeliveryReceipt>, AppDeliveryError> {
        let peers = self.peers.lock();
        let peer = peers
            .get(session.app_id.as_str())
            .ok_or(AppDeliveryError::EndpointUnavailable)?;
        Ok(peer.receipts.get(request_id).cloned())
    }
}

/// Local IPC transport backed by MutsukiLink Named Pipe / UDS addresses.
#[derive(Clone)]
pub struct LinkLocalAppTransport {
    session: mutsuki_link_local::SessionIdentity,
    lease_dir: PathBuf,
    request_timeout: Duration,
}

impl LinkLocalAppTransport {
    pub fn new(lease_dir: impl Into<PathBuf>) -> Self {
        Self {
            session: mutsuki_link_local::SessionIdentity::current(),
            lease_dir: lease_dir.into(),
            request_timeout: Duration::from_secs(30),
        }
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

impl AppLinkTransport for LinkLocalAppTransport {
    async fn try_connect(&self, app_id: &AppId) -> Result<AppLinkSession, AppDeliveryError> {
        let link_app = mutsuki_link_local::AppId::new(app_id.as_str())
            .map_err(|_| AppDeliveryError::AppNotInstalled)?;
        let _ = mutsuki_link_local::reclaim_stale_lease(
            &self.lease_dir,
            &link_app,
            Duration::from_secs(0),
        );
        let address = mutsuki_link_local::local_address_for_app(&link_app, &self.session);
        let budget = mutsuki_link_core::TransportBudget {
            idle_timeout: None,
            ..mutsuki_link_core::TransportBudget::default()
        };
        let context = mutsuki_link_core::ConnectContext {
            deadline: Some(std::time::Instant::now() + Duration::from_millis(200)),
            ..mutsuki_link_core::ConnectContext::default()
        };
        match mutsuki_link_local::connect(
            &address,
            mutsuki_link_core::EndpointId::from_bytes([1; 16]),
            mutsuki_link_local::endpoint_id_for_app(&link_app, &self.session),
            budget,
            &context,
        )
        .await
        {
            Ok(mut connection) => {
                let _ = connection.close_write();
                Ok(AppLinkSession {
                    app_id: app_id.clone(),
                    instance_id: format!("probed-{}", app_id.as_str()),
                    host_protocol_version: super::types::HOST_PROTOCOL_VERSION,
                    // Capability readiness is confirmed by wait/transmit, not a local catalog.
                    capabilities: Vec::new(),
                })
            }
            Err(_) => Err(AppDeliveryError::EndpointUnavailable),
        }
    }

    async fn wait_capability_ready(
        &self,
        session: &AppLinkSession,
        capability: &CapabilityDescriptor,
        timeout: Duration,
    ) -> Result<AppLinkSession, AppDeliveryError> {
        let started = tokio::time::Instant::now();
        loop {
            if self.try_connect(&session.app_id).await.is_ok() {
                // Peer endpoint is listening; typed negotiate happens on transmit/receipt.
                return Ok(AppLinkSession {
                    app_id: session.app_id.clone(),
                    instance_id: session.instance_id.clone(),
                    host_protocol_version: session.host_protocol_version,
                    capabilities: vec![capability.clone()],
                });
            }
            if started.elapsed() >= timeout {
                return Err(AppDeliveryError::ReadyTimeout);
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    async fn transmit(
        &self,
        _session: &AppLinkSession,
        envelope: &CapabilityRequestEnvelope,
    ) -> Result<DeliveryReceipt, AppDeliveryError> {
        let target = AppId::new(envelope.target.clone())?;
        crate::app_delivery::endpoint::connect_and_transmit(
            &target,
            &self.session,
            envelope,
            self.request_timeout,
        )
        .await
    }

    async fn query_receipt(
        &self,
        _session: &AppLinkSession,
        _request_id: &str,
    ) -> Result<Option<DeliveryReceipt>, AppDeliveryError> {
        Ok(None)
    }
}
