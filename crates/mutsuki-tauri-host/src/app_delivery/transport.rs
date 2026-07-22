use super::types::{AppDeliveryError, AppId, HOST_PROTOCOL_VERSION};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityStatus {
    Ready,
    Unavailable,
    Incompatible,
}

impl AppLinkSession {
    pub fn capability_ready(&self, required: &CapabilityDescriptor) -> bool {
        matches!(self.capability_status(required), CapabilityStatus::Ready)
    }

    pub fn capability_status(&self, required: &CapabilityDescriptor) -> CapabilityStatus {
        if self
            .capabilities
            .iter()
            .any(|offered| required.is_compatible_with(offered))
        {
            return CapabilityStatus::Ready;
        }
        if self
            .capabilities
            .iter()
            .any(|offered| offered.name == required.name)
        {
            return CapabilityStatus::Incompatible;
        }
        CapabilityStatus::Unavailable
    }

    pub fn ensure_protocol_compatible(&self) -> Result<(), AppDeliveryError> {
        if self.host_protocol_version != HOST_PROTOCOL_VERSION {
            Err(AppDeliveryError::ProtocolIncompatible)
        } else {
            Ok(())
        }
    }

    /// Wait-loop probe: `Ok(true)` ready, `Ok(false)` keep waiting, `Err` terminal failure.
    pub fn readiness_for(&self, required: &CapabilityDescriptor) -> Result<bool, AppDeliveryError> {
        self.ensure_protocol_compatible()?;
        match self.capability_status(required) {
            CapabilityStatus::Ready => Ok(true),
            CapabilityStatus::Incompatible => Err(AppDeliveryError::ProtocolIncompatible),
            CapabilityStatus::Unavailable => Ok(false),
        }
    }

    pub fn ensure_capability_ready(
        &self,
        required: &CapabilityDescriptor,
    ) -> Result<(), AppDeliveryError> {
        self.ensure_protocol_compatible()?;
        match self.capability_status(required) {
            CapabilityStatus::Ready => Ok(()),
            CapabilityStatus::Incompatible => Err(AppDeliveryError::ProtocolIncompatible),
            CapabilityStatus::Unavailable => Err(AppDeliveryError::CapabilityUnavailable),
        }
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
                host_protocol_version: HOST_PROTOCOL_VERSION,
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
                host_protocol_version: HOST_PROTOCOL_VERSION,
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

    pub fn set_host_protocol_version(&self, app_id: &AppId, version: u32) {
        if let Some(peer) = self.peers.lock().get_mut(app_id.as_str()) {
            peer.host_protocol_version = version;
        }
    }

    pub fn set_capabilities(&self, app_id: &AppId, capabilities: Vec<CapabilityDescriptor>) {
        if let Some(peer) = self.peers.lock().get_mut(app_id.as_str()) {
            peer.capabilities = capabilities;
        }
    }

    pub fn mark_online(&self, app_id: &AppId) {
        if let Some(peer) = self.peers.lock().get_mut(app_id.as_str()) {
            peer.online = true;
        }
    }

    fn snapshot_session(
        &self,
        app_id: &AppId,
        readiness: MemoryReadinessView,
    ) -> Result<AppLinkSession, AppDeliveryError> {
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
        let capabilities = if readiness.capabilities_visible(peer) {
            peer.capabilities.clone()
        } else {
            Vec::new()
        };
        Ok(AppLinkSession {
            app_id: app_id.clone(),
            instance_id: format!("memory-{}", app_id.as_str()),
            host_protocol_version: peer.host_protocol_version,
            capabilities,
        })
    }
}

#[derive(Clone, Copy)]
enum MemoryReadinessView {
    Probe,
    Waiting { since: tokio::time::Instant },
    Authoritative,
}

impl MemoryReadinessView {
    fn capabilities_visible(self, peer: &MemoryPeer) -> bool {
        match (peer.ready_after, self) {
            (_, Self::Authoritative) | (None, _) => true,
            (Some(_), Self::Probe) => false,
            (Some(delay), Self::Waiting { since }) => since.elapsed() >= delay,
        }
    }
}

impl AppLinkTransport for InMemoryAppLinkTransport {
    async fn try_connect(&self, app_id: &AppId) -> Result<AppLinkSession, AppDeliveryError> {
        self.snapshot_session(app_id, MemoryReadinessView::Probe)
    }

    async fn wait_capability_ready(
        &self,
        session: &AppLinkSession,
        capability: &CapabilityDescriptor,
        timeout: Duration,
    ) -> Result<AppLinkSession, AppDeliveryError> {
        let started = tokio::time::Instant::now();
        loop {
            let current = self.snapshot_session(
                &session.app_id,
                MemoryReadinessView::Waiting { since: started },
            )?;
            if current.readiness_for(capability)? {
                return Ok(current);
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
        let current = self.snapshot_session(&session.app_id, MemoryReadinessView::Authoritative)?;
        current.ensure_capability_ready(&envelope.capability)?;
        let mut peers = self.peers.lock();
        let peer = peers
            .get_mut(session.app_id.as_str())
            .ok_or(AppDeliveryError::EndpointUnavailable)?;
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
    probe_timeout: Duration,
}

impl LinkLocalAppTransport {
    pub fn new(lease_dir: impl Into<PathBuf>) -> Self {
        Self {
            session: mutsuki_link_local::SessionIdentity::current(),
            lease_dir: lease_dir.into(),
            request_timeout: Duration::from_secs(30),
            probe_timeout: Duration::from_millis(200),
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
        crate::app_delivery::endpoint::connect_and_handshake(
            app_id,
            &self.session,
            self.probe_timeout,
        )
        .await
    }

    async fn wait_capability_ready(
        &self,
        session: &AppLinkSession,
        capability: &CapabilityDescriptor,
        timeout: Duration,
    ) -> Result<AppLinkSession, AppDeliveryError> {
        let started = tokio::time::Instant::now();
        loop {
            match self.try_connect(&session.app_id).await {
                Ok(current) => {
                    if current.readiness_for(capability)? {
                        return Ok(current);
                    }
                }
                Err(AppDeliveryError::EndpointUnavailable) => {}
                Err(error) => return Err(error),
            }
            if started.elapsed() >= timeout {
                return Err(AppDeliveryError::ReadyTimeout);
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    async fn transmit(
        &self,
        session: &AppLinkSession,
        envelope: &CapabilityRequestEnvelope,
    ) -> Result<DeliveryReceipt, AppDeliveryError> {
        session.ensure_capability_ready(&envelope.capability)?;
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
