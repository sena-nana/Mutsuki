use super::types::{AppDeliveryError, AppId};
use mutsuki_link_core::{
    ConnectContext, Connection, EndpointId, TransportBudget, TransportErrorKind,
};
use mutsuki_link_local::{
    self, EndpointLease, LocalConnection, LocalListener, SessionIdentity, endpoint_id_for_app,
    local_address_for_app,
};
use mutsuki_runtime_contracts::{
    CapabilityDescriptor, CapabilityRequestEnvelope, DeliveryReceipt, IdempotentReceiptStore,
    RejectionReason,
};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

type CapabilityHandler =
    Arc<dyn Fn(CapabilityRequestEnvelope) -> DeliveryReceipt + Send + Sync + 'static>;

/// Local endpoint owner that accepts typed capability requests over Link IPC.
pub struct AppCapabilityEndpoint {
    app_id: AppId,
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
        let session = SessionIdentity::current();
        let link_app = mutsuki_link_local::AppId::new(app_id.as_str())
            .map_err(|_| AppDeliveryError::AppNotInstalled)?;
        let address = local_address_for_app(&link_app, &session);
        let endpoint_id = endpoint_id_for_app(&link_app, &session);
        let lease_dir = lease_dir.into();
        let _ =
            mutsuki_link_local::reclaim_stale_lease(&lease_dir, &link_app, Duration::from_secs(0));
        let lease = EndpointLease::create(&lease_dir, &link_app, instance_id).map_err(|_| {
            AppDeliveryError::ActivationFailed {
                message: "failed to create endpoint lease".into(),
            }
        })?;
        let endpoint = Arc::new(Self {
            app_id,
            lease: Mutex::new(Some(lease)),
            handlers: Arc::new(Mutex::new(BTreeMap::new())),
            receipts: Arc::new(Mutex::new(IdempotentReceiptStore::new())),
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
                if let Ok(envelope) = recv_json::<CapabilityRequestEnvelope>(&mut connection).await
                {
                    let receipt = endpoint.handle_envelope(envelope);
                    if send_json(&mut connection, &receipt).await.is_ok() {
                        // Let the framed writer flush before half-close.
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }
                let _ = connection.close_write();
            }
        });
        *self.accept_task.lock() = Some(handle);
        Ok(())
    }

    fn handle_envelope(&self, envelope: CapabilityRequestEnvelope) -> DeliveryReceipt {
        if let Some(existing) = self.receipts.lock().get(&envelope.request_id).cloned() {
            return DeliveryReceipt::Duplicate {
                request_id: envelope.request_id.clone(),
                previous: Box::new(existing),
            };
        }
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

pub(crate) async fn connect_and_transmit(
    target: &AppId,
    session: &SessionIdentity,
    envelope: &CapabilityRequestEnvelope,
    timeout: Duration,
) -> Result<DeliveryReceipt, AppDeliveryError> {
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
    let mut connection =
        mutsuki_link_local::connect(&address, local_endpoint, remote_endpoint, budget, &context)
            .await
            .map_err(|error| match error.kind {
                TransportErrorKind::Closed | TransportErrorKind::TimedOut => {
                    AppDeliveryError::EndpointUnavailable
                }
                _ => AppDeliveryError::DeliveryFailed {
                    message: error.to_string(),
                },
            })?;
    send_json(&mut connection, envelope)
        .await
        .map_err(|error| AppDeliveryError::DeliveryFailed {
            message: format!("send failed: {error}"),
        })?;
    let receipt = recv_json::<DeliveryReceipt>(&mut connection)
        .await
        .map_err(|error| AppDeliveryError::DeliveryFailed {
            message: format!("recv failed: {error}"),
        })?;
    let _ = connection.close_write();
    Ok(receipt)
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
