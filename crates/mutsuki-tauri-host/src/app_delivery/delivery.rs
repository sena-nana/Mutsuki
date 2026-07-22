use super::activator::TauriAppActivator;
use super::draft::{DeliveryDraft, DeliveryDraftStore};
use super::operation_history::{OperationHistory, OperationHistoryPolicy, OperationHistoryStats};
use super::transport::AppLinkTransport;
use super::types::{AppDeliveryError, AppDeliveryOptions, AppId, AppIdentity, DeliveryPhase};
use mutsuki_runtime_contracts::{CapabilityDescriptor, CapabilityRequestEnvelope, DeliveryReceipt};
use mutsuki_tauri_bridge::{DeliveryProgress, EventHub, MutsukiFrontendEvent};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

/// Unified wake-then-deliver orchestration for typed capability requests.
pub struct AppDeliveryService<A, T> {
    source: AppIdentity,
    activator: A,
    transport: T,
    drafts: DeliveryDraftStore,
    events: Option<Arc<EventHub>>,
    operations: Mutex<OperationHistory>,
    cancel: watch::Sender<bool>,
}

impl<A, T> AppDeliveryService<A, T>
where
    A: TauriAppActivator,
    T: AppLinkTransport,
{
    pub fn new(
        source: AppIdentity,
        activator: A,
        transport: T,
        drafts: DeliveryDraftStore,
    ) -> Self {
        Self::with_operation_policy(
            source,
            activator,
            transport,
            drafts,
            OperationHistoryPolicy::desktop_default(),
        )
    }

    pub fn with_operation_policy(
        source: AppIdentity,
        activator: A,
        transport: T,
        drafts: DeliveryDraftStore,
        operation_policy: OperationHistoryPolicy,
    ) -> Self {
        let (cancel, _) = watch::channel(false);
        Self {
            source,
            activator,
            transport,
            drafts,
            events: None,
            operations: Mutex::new(OperationHistory::new(operation_policy)),
            cancel,
        }
    }

    pub fn with_events(mut self, events: Arc<EventHub>) -> Self {
        self.events = Some(events);
        self
    }

    pub fn drafts(&self) -> &DeliveryDraftStore {
        &self.drafts
    }

    pub fn cancel_all(&self) {
        let _ = self.cancel.send(true);
    }

    pub async fn request_app(
        &self,
        target: AppId,
        capability: CapabilityDescriptor,
        payload: Value,
        options: AppDeliveryOptions,
    ) -> Result<DeliveryReceipt, AppDeliveryError> {
        let request_id = options
            .request_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let envelope = CapabilityRequestEnvelope::new(
            request_id,
            self.source.app_id.as_str(),
            target.as_str(),
            capability,
            payload,
        );
        self.run_delivery(envelope, options).await
    }

    async fn run_delivery(
        &self,
        envelope: CapabilityRequestEnvelope,
        options: AppDeliveryOptions,
    ) -> Result<DeliveryReceipt, AppDeliveryError> {
        let target = AppId::new(envelope.target.clone())?;
        self.emit_phase(
            &envelope.request_id,
            target.as_str(),
            DeliveryPhase::Connecting,
            None,
        );

        let connect_result = self.transport.try_connect(&target).await;
        let mut session = match connect_result {
            Ok(session) => session,
            Err(AppDeliveryError::EndpointUnavailable) if options.activate_if_offline => {
                self.emit_phase(
                    &envelope.request_id,
                    target.as_str(),
                    DeliveryPhase::TargetActivating,
                    None,
                );
                let descriptor = self.activator.resolve(&target).await?;
                let _activation = self.activator.activate(&descriptor).await?;
                self.wait_for_endpoint(&target, options.ready_timeout)
                    .await?
            }
            Err(error) => {
                return self.fail_with_optional_draft(envelope, options, error);
            }
        };

        if let Err(error) = session.ensure_protocol_compatible() {
            return self.fail_with_optional_draft(envelope, options, error);
        }

        self.emit_phase(
            &envelope.request_id,
            target.as_str(),
            DeliveryPhase::Negotiating,
            None,
        );
        if !session.capability_ready(&envelope.capability) {
            session = match self
                .transport
                .wait_capability_ready(&session, &envelope.capability, options.ready_timeout)
                .await
            {
                Ok(session) => session,
                Err(error) => {
                    return self.fail_with_optional_draft(envelope, options, error);
                }
            };
        }
        self.emit_phase(
            &envelope.request_id,
            target.as_str(),
            DeliveryPhase::TargetReady,
            None,
        );

        if let Err(error) = session.ensure_capability_ready(&envelope.capability) {
            return self.fail_with_optional_draft(envelope, options, error);
        }

        self.emit_phase(
            &envelope.request_id,
            target.as_str(),
            DeliveryPhase::Transmitting,
            None,
        );
        let receipt = match tokio::time::timeout(
            options.request_timeout,
            self.transport.transmit(&session, &envelope),
        )
        .await
        {
            Ok(Ok(receipt)) => receipt,
            Ok(Err(error)) => {
                return self.fail_with_optional_draft(envelope, options, error);
            }
            Err(_) => match self
                .transport
                .query_receipt(&session, &envelope.request_id)
                .await
            {
                Ok(Some(receipt)) => receipt,
                Ok(None) => {
                    return self.fail_with_optional_draft(
                        envelope,
                        options,
                        AppDeliveryError::ReceiptTimeout,
                    );
                }
                Err(error) => {
                    return self.fail_with_optional_draft(envelope, options, error);
                }
            },
        };

        let phase = match &receipt {
            DeliveryReceipt::Accepted { .. } | DeliveryReceipt::Duplicate { .. } => {
                DeliveryPhase::Accepted
            }
            DeliveryReceipt::Completed { .. } => DeliveryPhase::Completed,
            DeliveryReceipt::Rejected { .. } | DeliveryReceipt::Failed { .. } => {
                DeliveryPhase::DeliveryFailed
            }
        };
        self.emit_phase(&envelope.request_id, target.as_str(), phase, None);
        Ok(receipt)
    }

    async fn wait_for_endpoint(
        &self,
        target: &AppId,
        timeout: Duration,
    ) -> Result<super::transport::AppLinkSession, AppDeliveryError> {
        let started = tokio::time::Instant::now();
        let mut cancel = self.cancel.subscribe();
        loop {
            if *cancel.borrow() {
                return Err(AppDeliveryError::Cancelled);
            }
            match self.transport.try_connect(target).await {
                Ok(session) => return Ok(session),
                Err(AppDeliveryError::EndpointUnavailable) => {}
                Err(error) => return Err(error),
            }
            if started.elapsed() >= timeout {
                return Err(AppDeliveryError::ReadyTimeout);
            }
            tokio::select! {
                _ = cancel.changed() => {
                    if *cancel.borrow() {
                        return Err(AppDeliveryError::Cancelled);
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    }

    fn fail_with_optional_draft(
        &self,
        envelope: CapabilityRequestEnvelope,
        options: AppDeliveryOptions,
        error: AppDeliveryError,
    ) -> Result<DeliveryReceipt, AppDeliveryError> {
        if options.persist_on_failure {
            let draft = DeliveryDraft::from_envelope(&envelope, error.to_string());
            let _ = self.drafts.save(draft);
            self.emit_phase(
                &envelope.request_id,
                &envelope.target,
                DeliveryPhase::DraftSaved,
                Some(&error),
            );
        }
        self.emit_phase(
            &envelope.request_id,
            &envelope.target,
            DeliveryPhase::DeliveryFailed,
            Some(&error),
        );
        Err(error)
    }

    fn emit_phase(
        &self,
        request_id: &str,
        target_app: &str,
        phase: DeliveryPhase,
        error: Option<&AppDeliveryError>,
    ) {
        self.operations.lock().record(request_id, phase.clone());
        if let Some(events) = &self.events {
            let progress = DeliveryProgress {
                request_id: request_id.to_string(),
                target_app: target_app.to_string(),
                phase: phase.as_str().to_string(),
                error: error.map(ToString::to_string),
                error_kind: error.map(|value| value.kind_name().to_string()),
            };
            let _ = events.emit(MutsukiFrontendEvent::AppDelivery { progress });
        }
    }

    pub fn phase_for(&self, request_id: &str) -> Option<DeliveryPhase> {
        self.operations.lock().phase_for(request_id)
    }

    pub fn operation_stats(&self) -> OperationHistoryStats {
        self.operations.lock().stats()
    }
}
