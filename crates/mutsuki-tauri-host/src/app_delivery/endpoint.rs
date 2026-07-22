use super::types::{AppDeliveryError, AppId, HOST_PROTOCOL_VERSION};
use mutsuki_link_core::{
    ConnectContext, Connection, EndpointId, TransportBudget, TransportErrorKind,
};
use mutsuki_link_local::{
    self, EndpointLease, LocalConnection, LocalListener, SessionIdentity, endpoint_id_for_app,
    local_address_for_app,
};
use mutsuki_runtime_contracts::{
    CapabilityDescriptor, CapabilityRequestEnvelope, DeliveryReceipt, IdempotentReceiptStore,
    ReceiptRetentionPolicy, ReceiptStoreStats, RejectionReason,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

type CapabilityHandler =
    Arc<dyn Fn(CapabilityRequestEnvelope) -> DeliveryReceipt + Send + Sync + 'static>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointDescriptor {
    pub app_id: String,
    pub instance_id: String,
    pub host_protocol_version: u32,
    pub capabilities: Vec<CapabilityDescriptor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub(crate) enum LinkLocalClientFrame {
    DescribeEndpoint { host_protocol_version: u32 },
    CapabilityRequest(CapabilityRequestEnvelope),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub(crate) enum LinkLocalServerFrame {
    EndpointDescriptor(EndpointDescriptor),
    DeliveryReceipt(DeliveryReceipt),
    Rejected { reason: LinkLocalRejectReason },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LinkLocalRejectReason {
    ProtocolIncompatible,
    CapabilityUnavailable,
}

impl From<LinkLocalRejectReason> for AppDeliveryError {
    fn from(reason: LinkLocalRejectReason) -> Self {
        match reason {
            LinkLocalRejectReason::ProtocolIncompatible => Self::ProtocolIncompatible,
            LinkLocalRejectReason::CapabilityUnavailable => Self::CapabilityUnavailable,
        }
    }
}

/// Local endpoint owner that accepts typed capability requests over Link IPC.
pub struct AppCapabilityEndpoint {
    app_id: AppId,
    instance_id: String,
    lease: Mutex<Option<EndpointLease>>,
    handlers: Arc<Mutex<BTreeMap<String, (CapabilityDescriptor, CapabilityHandler)>>>,
    receipts: Arc<Mutex<IdempotentReceiptStore>>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
}

impl AppCapabilityEndpoint {
    pub fn open(
        app_id: AppId,
        instance_id: impl Into<String>,
        lease_dir: impl Into<PathBuf>,
    ) -> Result<Arc<Self>, AppDeliveryError> {
        Self::open_with_receipt_policy(
            app_id,
            instance_id,
            lease_dir,
            ReceiptRetentionPolicy::desktop_default(),
        )
    }

    pub fn open_with_receipt_policy(
        app_id: AppId,
        instance_id: impl Into<String>,
        lease_dir: impl Into<PathBuf>,
        receipt_policy: ReceiptRetentionPolicy,
    ) -> Result<Arc<Self>, AppDeliveryError> {
        let instance_id = instance_id.into();
        let session = SessionIdentity::current();
        let link_app = mutsuki_link_local::AppId::new(app_id.as_str())
            .map_err(|_| AppDeliveryError::AppNotInstalled)?;
        let address = local_address_for_app(&link_app, &session);
        let endpoint_id = endpoint_id_for_app(&link_app, &session);
        let lease_dir = lease_dir.into();
        let _ =
            mutsuki_link_local::reclaim_stale_lease(&lease_dir, &link_app, Duration::from_secs(0));
        let lease = EndpointLease::create(&lease_dir, &link_app, &instance_id).map_err(|_| {
            AppDeliveryError::ActivationFailed {
                message: "failed to create endpoint lease".into(),
            }
        })?;
        let endpoint = Arc::new(Self {
            app_id,
            instance_id,
            lease: Mutex::new(Some(lease)),
            handlers: Arc::new(Mutex::new(BTreeMap::new())),
            receipts: Arc::new(Mutex::new(IdempotentReceiptStore::with_policy(
                receipt_policy,
            ))),
            accept_task: Mutex::new(None),
        });
        endpoint.clone().spawn_accept_loop(address, endpoint_id)?;
        Ok(endpoint)
    }

    pub fn app_id(&self) -> &AppId {
        &self.app_id
    }

    pub fn register_handler<F>(&self, capability: CapabilityDescriptor, handler: F)
    where
        F: Fn(CapabilityRequestEnvelope) -> DeliveryReceipt + Send + Sync + 'static,
    {
        self.handlers.lock().insert(
            capability.name.clone(),
            (capability, Arc::new(handler) as CapabilityHandler),
        );
    }

    pub fn receipt_stats(&self) -> ReceiptStoreStats {
        self.receipts.lock().stats()
    }

    pub(crate) fn describe(&self) -> EndpointDescriptor {
        let handlers = self.handlers.lock();
        let mut capabilities: Vec<CapabilityDescriptor> = handlers
            .values()
            .map(|(capability, _)| capability.clone())
            .collect();
        capabilities.sort_by(|left, right| left.name.cmp(&right.name));
        EndpointDescriptor {
            app_id: self.app_id.as_str().to_string(),
            instance_id: self.instance_id.clone(),
            host_protocol_version: HOST_PROTOCOL_VERSION,
            capabilities,
        }
    }

    fn spawn_accept_loop(
        self: Arc<Self>,
        address: mutsuki_link_local::LocalAddress,
        endpoint_id: EndpointId,
    ) -> Result<(), AppDeliveryError> {
        let budget = TransportBudget {
            idle_timeout: None,
            ..TransportBudget::default()
        };
        let listener = LocalListener::bind(&address, endpoint_id, budget).map_err(|error| {
            AppDeliveryError::ActivationFailed {
                message: format!("bind local endpoint failed: {error}"),
            }
        })?;
        let endpoint = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok(mut connection) = listener.accept(EndpointId::from_bytes([1; 16])).await
                else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                };
                endpoint.serve_connection(&mut connection).await;
                let _ = connection.close_write();
            }
        });
        *self.accept_task.lock() = Some(handle);
        Ok(())
    }

    async fn serve_connection(&self, connection: &mut LocalConnection) {
        loop {
            let frame = match recv_json::<LinkLocalClientFrame>(connection).await {
                Ok(frame) => frame,
                Err(_) => return,
            };
            match frame {
                LinkLocalClientFrame::DescribeEndpoint {
                    host_protocol_version,
                } => {
                    if host_protocol_version != HOST_PROTOCOL_VERSION {
                        let _ = send_json(
                            connection,
                            &LinkLocalServerFrame::Rejected {
                                reason: LinkLocalRejectReason::ProtocolIncompatible,
                            },
                        )
                        .await;
                        return;
                    }
                    if send_json(
                        connection,
                        &LinkLocalServerFrame::EndpointDescriptor(self.describe()),
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                }
                LinkLocalClientFrame::CapabilityRequest(envelope) => {
                    let receipt = self.handle_envelope(envelope);
                    let _ = send_json(connection, &LinkLocalServerFrame::DeliveryReceipt(receipt))
                        .await;
                    // Let the framed writer flush before half-close.
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    return;
                }
            }
        }
    }

    fn handle_envelope(&self, envelope: CapabilityRequestEnvelope) -> DeliveryReceipt {
        let mut receipts = self.receipts.lock();
        if let Some(existing) = receipts.take_live(&envelope.request_id, Instant::now()) {
            return DeliveryReceipt::Duplicate {
                request_id: envelope.request_id.clone(),
                previous: Box::new(existing),
            };
        }
        drop(receipts);
        let handlers = self.handlers.lock();
        let Some((capability, handler)) = handlers.get(&envelope.capability.name) else {
            return DeliveryReceipt::Rejected {
                request_id: envelope.request_id,
                reason: RejectionReason::CapabilityUnavailable,
            };
        };
        if !envelope.capability.is_compatible_with(capability) {
            return DeliveryReceipt::Rejected {
                request_id: envelope.request_id,
                reason: RejectionReason::ProtocolIncompatible,
            };
        }
        let receipt = handler(envelope);
        drop(handlers);
        self.receipts
            .lock()
            .accept_or_duplicate(receipt.request_id().to_string(), receipt)
    }
}

impl Drop for AppCapabilityEndpoint {
    fn drop(&mut self) {
        if let Some(handle) = self.accept_task.lock().take() {
            handle.abort();
        }
        if let Some(lease) = self.lease.lock().take() {
            let _ = lease.clear();
        }
    }
}

pub(crate) async fn connect_and_handshake(
    target: &AppId,
    session: &SessionIdentity,
    timeout: Duration,
) -> Result<super::transport::AppLinkSession, AppDeliveryError> {
    let mut connection = connect_local(target, session, timeout).await?;
    let result = perform_handshake(target, &mut connection).await;
    let _ = connection.close_write();
    result
}

pub(crate) async fn connect_and_transmit(
    target: &AppId,
    session: &SessionIdentity,
    envelope: &CapabilityRequestEnvelope,
    timeout: Duration,
) -> Result<DeliveryReceipt, AppDeliveryError> {
    let mut connection = connect_local(target, session, timeout).await?;
    let link_session = perform_handshake(target, &mut connection).await?;
    link_session.ensure_capability_ready(&envelope.capability)?;
    send_json(
        &mut connection,
        &LinkLocalClientFrame::CapabilityRequest(envelope.clone()),
    )
    .await
    .map_err(|error| AppDeliveryError::DeliveryFailed {
        message: format!("send failed: {error}"),
    })?;
    let receipt = match recv_json::<LinkLocalServerFrame>(&mut connection).await? {
        LinkLocalServerFrame::DeliveryReceipt(receipt) => receipt,
        other => return Err(map_unexpected_server_frame(other)),
    };
    let _ = connection.close_write();
    Ok(receipt)
}

async fn connect_local(
    target: &AppId,
    session: &SessionIdentity,
    timeout: Duration,
) -> Result<LocalConnection, AppDeliveryError> {
    let link_app = mutsuki_link_local::AppId::new(target.as_str())
        .map_err(|_| AppDeliveryError::AppNotInstalled)?;
    let address = local_address_for_app(&link_app, session);
    let local_endpoint = EndpointId::from_bytes([1; 16]);
    let remote_endpoint = endpoint_id_for_app(&link_app, session);
    let budget = TransportBudget {
        idle_timeout: None,
        ..TransportBudget::default()
    };
    let context = ConnectContext {
        deadline: Some(Instant::now() + timeout),
        ..ConnectContext::default()
    };
    mutsuki_link_local::connect(&address, local_endpoint, remote_endpoint, budget, &context)
        .await
        .map_err(|error| match error.kind {
            TransportErrorKind::Closed | TransportErrorKind::TimedOut => {
                AppDeliveryError::EndpointUnavailable
            }
            _ => AppDeliveryError::DeliveryFailed {
                message: error.to_string(),
            },
        })
}

async fn perform_handshake(
    target: &AppId,
    connection: &mut LocalConnection,
) -> Result<super::transport::AppLinkSession, AppDeliveryError> {
    send_json(
        connection,
        &LinkLocalClientFrame::DescribeEndpoint {
            host_protocol_version: HOST_PROTOCOL_VERSION,
        },
    )
    .await
    .map_err(map_handshake_failure)?;
    let frame = recv_json::<LinkLocalServerFrame>(connection)
        .await
        .map_err(map_handshake_failure)?;
    match frame {
        LinkLocalServerFrame::EndpointDescriptor(descriptor) => {
            if descriptor.app_id != target.as_str()
                || descriptor.host_protocol_version != HOST_PROTOCOL_VERSION
            {
                return Err(AppDeliveryError::ProtocolIncompatible);
            }
            Ok(super::transport::AppLinkSession {
                app_id: target.clone(),
                instance_id: descriptor.instance_id,
                host_protocol_version: descriptor.host_protocol_version,
                capabilities: descriptor.capabilities,
            })
        }
        other => Err(map_unexpected_server_frame(other)),
    }
}

fn map_handshake_failure(error: AppDeliveryError) -> AppDeliveryError {
    match error {
        AppDeliveryError::EndpointUnavailable => AppDeliveryError::EndpointUnavailable,
        AppDeliveryError::DeliveryFailed { .. } | AppDeliveryError::ReceiptTimeout => {
            AppDeliveryError::ProtocolIncompatible
        }
        other => other,
    }
}

fn map_unexpected_server_frame(frame: LinkLocalServerFrame) -> AppDeliveryError {
    match frame {
        LinkLocalServerFrame::Rejected { reason } => reason.into(),
        _ => AppDeliveryError::ProtocolIncompatible,
    }
}

async fn send_json<T: serde::Serialize>(
    connection: &mut LocalConnection,
    value: &T,
) -> Result<(), AppDeliveryError> {
    let bytes = serde_json::to_vec(value).map_err(|error| AppDeliveryError::DeliveryFailed {
        message: error.to_string(),
    })?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match connection.try_send(&bytes) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind == TransportErrorKind::WouldBlock => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(AppDeliveryError::DeliveryFailed {
                        message: "send timed out".into(),
                    });
                }
                tokio::task::yield_now().await;
            }
            Err(error) => {
                return Err(AppDeliveryError::DeliveryFailed {
                    message: error.to_string(),
                });
            }
        }
    }
}

async fn recv_json<T: serde::de::DeserializeOwned>(
    connection: &mut LocalConnection,
) -> Result<T, AppDeliveryError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match connection.try_receive() {
            Ok(Some(bytes)) => {
                return serde_json::from_slice(&bytes).map_err(|error| {
                    AppDeliveryError::DeliveryFailed {
                        message: error.to_string(),
                    }
                });
            }
            Ok(None) => {
                return Err(AppDeliveryError::DeliveryFailed {
                    message: "connection closed before receipt".into(),
                });
            }
            Err(error) if error.kind == TransportErrorKind::WouldBlock => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(AppDeliveryError::ReceiptTimeout);
                }
                tokio::task::yield_now().await;
            }
            Err(error) => {
                return Err(AppDeliveryError::DeliveryFailed {
                    message: error.to_string(),
                });
            }
        }
    }
}
