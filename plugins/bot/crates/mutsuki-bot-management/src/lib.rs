//! Headless Bot management contracts: QQ and Bilibili console APIs.

mod bilibili;

pub use bilibili::*;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_protocol::{
    BotDeliveryAttempt, BotDeliveryReceipt, BotInteractionSession, DeliveryStatus,
    InteractionStatus, QqBotCapabilityMatrix, QqConversationRef,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QqBotManagementSnapshot {
    pub revision: u64,
    pub accounts: Vec<QqAccountView>,
    pub deliveries: Vec<QqDeliveryView>,
    pub interactions: Vec<BotInteractionSession>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqAccountView {
    pub account_id: String,
    pub app_id: String,
    pub enabled: bool,
    pub health: String,
    pub connection_state: QqGatewayConnectionState,
    pub last_heartbeat_unix_ms: Option<u64>,
    pub last_error: Option<String>,
    pub reconnect_count: u64,
    pub intents: u64,
    pub shard: [u64; 2],
    pub credential_reference: String,
    pub credential_status: String,
    pub rate_limit_status: String,
    pub capability: QqBotCapabilityMatrix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqGatewayConnectionState {
    Disconnected,
    Connected,
    Identified,
    Resumable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqDeliveryView {
    pub receipt: BotDeliveryReceipt,
    pub attempts: Vec<BotDeliveryAttempt>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QqManagementPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum QqManagementAction {
    AccountSetEnabled {
        account_id: String,
        enabled: bool,
    },
    AccountHealthCheck {
        account_id: String,
    },
    AccountReconnect {
        account_id: String,
    },
    AccountSendTest {
        account_id: String,
        conversation: QqConversationRef,
        text: String,
    },
    DeliveryRetry {
        delivery_id: String,
    },
    DeliveryCancel {
        delivery_id: String,
    },
    DeliveryPreview {
        delivery_id: String,
    },
    InteractionCancel {
        session_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QqManagementWriteRequest {
    #[serde(default)]
    pub operation_id: String,
    pub expected_revision: u64,
    pub action: QqManagementAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QqManagementWriteResult {
    pub revision: u64,
    pub audit_id: String,
    pub result: Value,
}

#[async_trait]
pub trait QqBotManagementApi: Send + Sync {
    async fn snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError>;

    async fn write(
        &self,
        actor_id: &str,
        request: QqManagementWriteRequest,
    ) -> Result<QqManagementWriteResult, QqManagementError>;

    async fn delivery_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<QqManagementPage<QqDeliveryView>, QqManagementError> {
        let snapshot = self.snapshot(query, false).await?;
        Ok(stable_page(snapshot.deliveries, after, limit, |item| {
            item.receipt.delivery_id.clone()
        }))
    }

    async fn interaction_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<QqManagementPage<BotInteractionSession>, QqManagementError> {
        let snapshot = self.snapshot(query, false).await?;
        Ok(stable_page(snapshot.interactions, after, limit, |item| {
            item.session_id.clone()
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QqManagementError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for QqManagementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for QqManagementError {}

/// Live snapshot/action backend injected into [`QqBotManagementService`].
#[async_trait]
pub trait QqManagementProvider: Send + Sync {
    /// Loads the current management projection.
    ///
    /// # Errors
    ///
    /// Returns a typed management error when the owner cannot materialize the snapshot.
    fn load_snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError>;

    /// Applies one management action against the live owner.
    ///
    /// # Errors
    ///
    /// Returns a typed management error when the action is rejected or the owner fails.
    async fn apply(
        &self,
        operation_id: &str,
        replaying: bool,
        actor_id: &str,
        action: &QqManagementAction,
    ) -> Result<Value, QqManagementError>;

    /// # Errors
    ///
    /// Returns a typed management error when the owner cannot list deliveries.
    fn delivery_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<crate::QqManagementPage<QqDeliveryView>, QqManagementError> {
        let snapshot = self.load_snapshot(query, false)?;
        Ok(stable_page(snapshot.deliveries, after, limit, |item| {
            item.receipt.delivery_id.clone()
        }))
    }

    /// # Errors
    ///
    /// Returns a typed management error when the owner cannot list interactions.
    fn interaction_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<crate::QqManagementPage<BotInteractionSession>, QqManagementError> {
        let snapshot = self.load_snapshot(query, false)?;
        Ok(stable_page(snapshot.interactions, after, limit, |item| {
            item.session_id.clone()
        }))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QqManagementAuditEntry {
    pub audit_id: String,
    pub actor_id: String,
    pub action: String,
    pub revision: u64,
    pub result: Value,
    pub created_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QqManagementOperationReservation {
    Reserved,
    Resume,
    Completed(QqManagementAuditEntry),
}

pub trait QqManagementStateStore: Send + Sync {
    /// # Errors
    ///
    /// Returns a typed storage error when the revision cannot be read.
    fn revision(&self) -> Result<u64, QqManagementError>;
    /// # Errors
    ///
    /// Returns a conflict or storage error when the operation cannot be reserved.
    fn begin_operation(
        &self,
        operation_id: &str,
        expected_revision: u64,
        actor_id: &str,
        action: &str,
        created_at_unix_ms: u64,
    ) -> Result<QqManagementOperationReservation, QqManagementError>;
    /// # Errors
    ///
    /// Returns a conflict or storage error when completion cannot be committed.
    fn complete_operation(
        &self,
        operation_id: &str,
        action: &str,
        result: Value,
        created_at_unix_ms: u64,
    ) -> Result<QqManagementAuditEntry, QqManagementError>;
    /// # Errors
    ///
    /// Returns a typed storage error when audit entries cannot be read.
    fn audits(&self) -> Result<Vec<QqManagementAuditEntry>, QqManagementError>;
}

#[derive(Default)]
struct MemoryQqManagementStateStore {
    state: Mutex<MemoryManagementState>,
}

#[derive(Default)]
struct MemoryManagementState {
    revision: u64,
    audits: Vec<QqManagementAuditEntry>,
    operations: HashMap<String, MemoryManagementOperation>,
}

struct MemoryManagementOperation {
    expected_revision: u64,
    actor_id: String,
    action: String,
    completed: Option<QqManagementAuditEntry>,
}

impl QqManagementStateStore for MemoryQqManagementStateStore {
    fn revision(&self) -> Result<u64, QqManagementError> {
        Ok(self
            .state
            .lock()
            .expect("QQ management state mutex")
            .revision)
    }

    fn begin_operation(
        &self,
        operation_id: &str,
        expected_revision: u64,
        actor_id: &str,
        action: &str,
        _created_at_unix_ms: u64,
    ) -> Result<QqManagementOperationReservation, QqManagementError> {
        let mut state = self.state.lock().expect("QQ management state mutex");
        if let Some(operation) = state.operations.get(operation_id) {
            if operation.expected_revision != expected_revision
                || operation.actor_id != actor_id
                || operation.action != action
            {
                return Err(operation_conflict(operation_id));
            }
            return Ok(operation.completed.clone().map_or(
                QqManagementOperationReservation::Resume,
                QqManagementOperationReservation::Completed,
            ));
        }
        if state.revision != expected_revision {
            return Err(revision_conflict(expected_revision, state.revision));
        }
        if state.operations.values().any(|operation| {
            operation.expected_revision == expected_revision && operation.completed.is_none()
        }) {
            return Err(QqManagementError {
                code: "operation.in_progress".into(),
                message: "another management operation is still in progress".into(),
            });
        }
        state.operations.insert(
            operation_id.into(),
            MemoryManagementOperation {
                expected_revision,
                actor_id: actor_id.into(),
                action: action.into(),
                completed: None,
            },
        );
        Ok(QqManagementOperationReservation::Reserved)
    }

    fn complete_operation(
        &self,
        operation_id: &str,
        action: &str,
        result: Value,
        created_at_unix_ms: u64,
    ) -> Result<QqManagementAuditEntry, QqManagementError> {
        let mut state = self.state.lock().expect("QQ management state mutex");
        let operation = state
            .operations
            .get(operation_id)
            .ok_or_else(|| operation_conflict(operation_id))?;
        if let Some(completed) = &operation.completed {
            return Ok(completed.clone());
        }
        if state.revision != operation.expected_revision {
            return Err(revision_conflict(
                operation.expected_revision,
                state.revision,
            ));
        }
        let actor_id = operation.actor_id.clone();
        let action = action.to_owned();
        state.revision = state.revision.saturating_add(1);
        let audit = QqManagementAuditEntry {
            audit_id: format!("audit-{}", state.revision),
            actor_id,
            action,
            revision: state.revision,
            result,
            created_at_unix_ms,
        };
        state.audits.push(audit.clone());
        state
            .operations
            .get_mut(operation_id)
            .expect("operation reserved above")
            .completed = Some(audit.clone());
        Ok(audit)
    }

    fn audits(&self) -> Result<Vec<QqManagementAuditEntry>, QqManagementError> {
        Ok(self
            .state
            .lock()
            .expect("QQ management state mutex")
            .audits
            .clone())
    }
}

/// Production management service: revision fence + audit around an injected provider.
pub struct QqBotManagementService {
    write_lock: tokio::sync::Mutex<()>,
    provider: Arc<dyn QqManagementProvider>,
    state: Arc<dyn QqManagementStateStore>,
}

impl QqBotManagementService {
    #[must_use]
    pub fn new(provider: Arc<dyn QqManagementProvider>) -> Self {
        Self {
            write_lock: tokio::sync::Mutex::new(()),
            provider,
            state: Arc::new(MemoryQqManagementStateStore::default()),
        }
    }

    #[must_use]
    pub fn with_state_store(
        provider: Arc<dyn QqManagementProvider>,
        state: Arc<dyn QqManagementStateStore>,
    ) -> Self {
        Self {
            write_lock: tokio::sync::Mutex::new(()),
            provider,
            state,
        }
    }

    /// Builds a deterministic in-memory service for Web and contract tests.
    #[must_use]
    pub fn local(provider: LocalQqManagementProvider) -> Self {
        Self::new(Arc::new(provider))
    }

    #[must_use]
    pub fn audits(&self) -> Vec<(String, String, String, u64)> {
        self.state
            .audits()
            .unwrap_or_default()
            .iter()
            .map(|entry| {
                (
                    entry.audit_id.clone(),
                    entry.actor_id.clone(),
                    entry.action.clone(),
                    entry.revision,
                )
            })
            .collect()
    }
}

#[async_trait]
impl QqBotManagementApi for QqBotManagementService {
    async fn snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError> {
        let mut snapshot = self.provider.load_snapshot(query, include_secret_status)?;
        snapshot.revision = self.state.revision()?;
        Ok(snapshot)
    }

    async fn write(
        &self,
        actor_id: &str,
        request: QqManagementWriteRequest,
    ) -> Result<QqManagementWriteResult, QqManagementError> {
        if request.operation_id.is_empty()
            || request.operation_id.len() > 128
            || !request
                .operation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(QqManagementError {
                code: "invalid_argument".into(),
                message: "operation_id is invalid".into(),
            });
        }
        let _write = self.write_lock.lock().await;
        let action = action_name(&request.action);
        let operation_fingerprint =
            serde_json::to_string(&request.action).map_err(|error| QqManagementError {
                code: "invalid_argument".into(),
                message: error.to_string(),
            })?;
        let replaying = match self.state.begin_operation(
            &request.operation_id,
            request.expected_revision,
            actor_id,
            &operation_fingerprint,
            unix_ms(),
        )? {
            QqManagementOperationReservation::Completed(audit) => {
                return Ok(QqManagementWriteResult {
                    revision: audit.revision,
                    audit_id: audit.audit_id,
                    result: audit.result,
                });
            }
            QqManagementOperationReservation::Reserved => false,
            QqManagementOperationReservation::Resume => true,
        };
        let result = self
            .provider
            .apply(&request.operation_id, replaying, actor_id, &request.action)
            .await?;
        let audit = self.state.complete_operation(
            &request.operation_id,
            action,
            result.clone(),
            unix_ms(),
        )?;
        Ok(QqManagementWriteResult {
            revision: audit.revision,
            audit_id: audit.audit_id,
            result,
        })
    }

    async fn delivery_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<crate::QqManagementPage<QqDeliveryView>, QqManagementError> {
        self.provider.delivery_page(query, after, limit)
    }

    async fn interaction_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<crate::QqManagementPage<BotInteractionSession>, QqManagementError> {
        self.provider.interaction_page(query, after, limit)
    }
}

/// Deterministic in-memory provider used by Web and contract tests.
#[derive(Default)]
pub struct LocalQqManagementProvider {
    state: Mutex<LocalState>,
}

#[derive(Clone, Default)]
struct LocalState {
    accounts: Vec<QqAccountView>,
    deliveries: Vec<QqDeliveryView>,
    interactions: Vec<BotInteractionSession>,
}

impl LocalQqManagementProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock_state(&self) -> MutexGuard<'_, LocalState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn upsert_account(&self, account: QqAccountView) {
        let mut state = self.lock_state();
        if let Some(existing) = state
            .accounts
            .iter_mut()
            .find(|item| item.account_id == account.account_id)
        {
            *existing = account;
        } else {
            state.accounts.push(account);
        }
    }

    pub fn upsert_delivery(&self, delivery: QqDeliveryView) {
        let mut state = self.lock_state();
        if let Some(existing) = state
            .deliveries
            .iter_mut()
            .find(|item| item.receipt.delivery_id == delivery.receipt.delivery_id)
        {
            *existing = delivery;
        } else {
            state.deliveries.push(delivery);
        }
    }

    pub fn upsert_interaction(&self, session: BotInteractionSession) {
        let mut state = self.lock_state();
        if let Some(existing) = state
            .interactions
            .iter_mut()
            .find(|item| item.session_id == session.session_id)
        {
            *existing = session;
        } else {
            state.interactions.push(session);
        }
    }
}

#[async_trait]
impl QqManagementProvider for LocalQqManagementProvider {
    fn load_snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError> {
        let state = self.lock_state().clone();
        let query = query.trim().to_ascii_lowercase();
        let accounts = state
            .accounts
            .into_iter()
            .filter(|account| {
                query.is_empty() || account.account_id.to_ascii_lowercase().contains(&query)
            })
            .map(|mut account| {
                if !include_secret_status {
                    account.credential_reference.clear();
                    account.credential_status = "restricted".into();
                }
                account
            })
            .collect();
        let deliveries = state
            .deliveries
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item
                        .receipt
                        .delivery_id
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .collect();
        Ok(QqBotManagementSnapshot {
            revision: 0,
            accounts,
            deliveries,
            interactions: state.interactions,
        })
    }

    async fn apply(
        &self,
        _operation_id: &str,
        _replaying: bool,
        _actor_id: &str,
        action: &QqManagementAction,
    ) -> Result<Value, QqManagementError> {
        let mut state = self.lock_state();
        match action {
            QqManagementAction::AccountSetEnabled {
                account_id,
                enabled,
            } => set_account_enabled(&mut state, account_id, *enabled),
            QqManagementAction::AccountHealthCheck { account_id } => {
                check_account_health(&mut state, account_id)
            }
            QqManagementAction::AccountReconnect { account_id } => {
                reconnect_account(&mut state, account_id)
            }
            QqManagementAction::AccountSendTest {
                account_id,
                conversation,
                text,
            } => send_test(&mut state, account_id, conversation, text),
            QqManagementAction::DeliveryRetry { delivery_id } => {
                retry_delivery(&mut state, delivery_id)
            }
            QqManagementAction::DeliveryCancel { delivery_id } => {
                cancel_delivery(&mut state, delivery_id)
            }
            QqManagementAction::DeliveryPreview { delivery_id } => {
                preview_delivery(&state, delivery_id)
            }
            QqManagementAction::InteractionCancel { session_id } => {
                cancel_interaction(&mut state, session_id)
            }
        }
    }
}

fn set_account_enabled(
    state: &mut LocalState,
    account_id: &str,
    enabled: bool,
) -> Result<Value, QqManagementError> {
    account_mut(state, account_id)?.enabled = enabled;
    Ok(json!({ "account_id": account_id, "enabled": enabled }))
}

fn check_account_health(
    state: &mut LocalState,
    account_id: &str,
) -> Result<Value, QqManagementError> {
    let account = account_mut(state, account_id)?;
    account.health = match account.connection_state {
        QqGatewayConnectionState::Identified | QqGatewayConnectionState::Resumable => "ok".into(),
        QqGatewayConnectionState::Connected => "degraded".into(),
        QqGatewayConnectionState::Disconnected => "unhealthy".into(),
    };
    Ok(json!({
        "account_id": account_id,
        "health": account.health,
        "connection_state": account.connection_state,
    }))
}

fn reconnect_account(state: &mut LocalState, account_id: &str) -> Result<Value, QqManagementError> {
    let account = account_mut(state, account_id)?;
    account.connection_state = QqGatewayConnectionState::Connected;
    account.health = "reconnecting".into();
    account.rate_limit_status = "ready".into();
    Ok(json!({ "account_id": account_id, "reconnected": true }))
}

fn send_test(
    state: &mut LocalState,
    account_id: &str,
    conversation: &QqConversationRef,
    text: &str,
) -> Result<Value, QqManagementError> {
    let _ = account_mut(state, account_id)?;
    if text.trim().is_empty() {
        return Err(QqManagementError {
            code: "invalid_argument".into(),
            message: "test text must not be empty".into(),
        });
    }
    Ok(json!({
        "account_id": account_id,
        "conversation": conversation,
        "sent": true,
        "preview": text,
    }))
}

fn retry_delivery(state: &mut LocalState, delivery_id: &str) -> Result<Value, QqManagementError> {
    let delivery = delivery_mut(state, delivery_id)?;
    if !matches!(
        delivery.receipt.status,
        DeliveryStatus::RetryScheduled
            | DeliveryStatus::PermanentlyFailed
            | DeliveryStatus::ReconcileRequired
    ) {
        return Err(QqManagementError {
            code: "invalid_state".into(),
            message: format!(
                "delivery `{delivery_id}` status {:?} cannot retry",
                delivery.receipt.status
            ),
        });
    }
    let attempt = delivery.receipt.attempt_count.saturating_add(1);
    delivery.receipt.attempt_count = attempt;
    delivery.receipt.status = DeliveryStatus::Pending;
    delivery.receipt.error_code = None;
    delivery.attempts.push(BotDeliveryAttempt {
        delivery_id: delivery_id.into(),
        attempt,
        status: DeliveryStatus::Pending,
        started_at_unix_ms: 0,
        retry_at_unix_ms: None,
        error_code: None,
    });
    Ok(json!({ "delivery_id": delivery_id, "attempt_count": attempt }))
}

fn cancel_delivery(state: &mut LocalState, delivery_id: &str) -> Result<Value, QqManagementError> {
    delivery_mut(state, delivery_id)?.receipt.status = DeliveryStatus::Cancelled;
    Ok(json!({ "delivery_id": delivery_id, "status": "cancelled" }))
}

fn preview_delivery(state: &LocalState, delivery_id: &str) -> Result<Value, QqManagementError> {
    let delivery = state
        .deliveries
        .iter()
        .find(|item| item.receipt.delivery_id == delivery_id)
        .ok_or_else(|| QqManagementError {
            code: "not_found".into(),
            message: format!("delivery `{delivery_id}` was not found"),
        })?;
    Ok(json!({
        "delivery_id": delivery_id,
        "status": "previewed",
        "attempt_count": delivery.receipt.attempt_count,
    }))
}

fn cancel_interaction(
    state: &mut LocalState,
    session_id: &str,
) -> Result<Value, QqManagementError> {
    let session = state
        .interactions
        .iter_mut()
        .find(|item| item.session_id == session_id)
        .ok_or_else(|| QqManagementError {
            code: "not_found".into(),
            message: format!("interaction `{session_id}` was not found"),
        })?;
    if session.status != InteractionStatus::Waiting {
        return Err(QqManagementError {
            code: "invalid_state".into(),
            message: format!("interaction `{session_id}` is not waiting"),
        });
    }
    session.status = InteractionStatus::Cancelled;
    session.version = session.version.saturating_add(1);
    Ok(json!({
        "session_id": session_id,
        "status": "cancelled",
        "version": session.version,
    }))
}

/// Adapter config facts and live gateway status used to build one account view.
pub struct QqAccountViewInput {
    pub account_id: String,
    pub app_id: String,
    pub credential_reference: String,
    pub credential_present: bool,
    pub capability: QqBotCapabilityMatrix,
    pub intents: u64,
    pub shard: [u64; 2],
    pub connected: bool,
    pub identified: bool,
    pub last_heartbeat_unix_ms: Option<u64>,
    pub last_error: Option<String>,
    pub reconnect_count: u64,
}

/// Builds an account view from adapter config facts plus live gateway health.
#[must_use]
pub fn account_view_from_config(input: QqAccountViewInput) -> QqAccountView {
    let connection_state = if input.identified {
        QqGatewayConnectionState::Identified
    } else if input.connected {
        QqGatewayConnectionState::Connected
    } else if input.last_error.is_some() {
        QqGatewayConnectionState::Resumable
    } else {
        QqGatewayConnectionState::Disconnected
    };
    let health = if input.identified && input.connected {
        "ok"
    } else if input.connected {
        "degraded"
    } else if input.last_error.is_some() {
        "unhealthy"
    } else {
        "stopped"
    };
    QqAccountView {
        account_id: input.account_id,
        app_id: input.app_id,
        enabled: true,
        health: health.into(),
        connection_state,
        last_heartbeat_unix_ms: input.last_heartbeat_unix_ms,
        last_error: input.last_error,
        reconnect_count: input.reconnect_count,
        intents: input.intents,
        shard: input.shard,
        credential_reference: input.credential_reference,
        credential_status: if input.credential_present {
            "configured"
        } else {
            "absent"
        }
        .into(),
        rate_limit_status: "ready".into(),
        capability: input.capability,
    }
}

#[must_use]
pub fn delivery_view(
    receipt: BotDeliveryReceipt,
    attempts: Vec<BotDeliveryAttempt>,
) -> QqDeliveryView {
    QqDeliveryView { receipt, attempts }
}

fn account_mut<'a>(
    state: &'a mut LocalState,
    account_id: &str,
) -> Result<&'a mut QqAccountView, QqManagementError> {
    state
        .accounts
        .iter_mut()
        .find(|item| item.account_id == account_id)
        .ok_or_else(|| QqManagementError {
            code: "not_found".into(),
            message: format!("account `{account_id}` was not found"),
        })
}

fn delivery_mut<'a>(
    state: &'a mut LocalState,
    delivery_id: &str,
) -> Result<&'a mut QqDeliveryView, QqManagementError> {
    state
        .deliveries
        .iter_mut()
        .find(|item| item.receipt.delivery_id == delivery_id)
        .ok_or_else(|| QqManagementError {
            code: "not_found".into(),
            message: format!("delivery `{delivery_id}` was not found"),
        })
}

fn stable_page<T>(
    mut items: Vec<T>,
    after: Option<&str>,
    limit: u32,
    id: impl Fn(&T) -> String,
) -> crate::QqManagementPage<T> {
    let limit = limit.clamp(1, 100);
    items.sort_by_key(|item| id(item));
    let after = after.unwrap_or_default();
    let mut items = items
        .into_iter()
        .filter(|item| id(item).as_str() > after)
        .take(limit.saturating_add(1) as usize)
        .collect::<Vec<_>>();
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next_cursor = has_more.then(|| items.last().map(&id)).flatten();
    crate::QqManagementPage { items, next_cursor }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn action_name(action: &QqManagementAction) -> &'static str {
    match action {
        QqManagementAction::AccountSetEnabled { .. } => "account_set_enabled",
        QqManagementAction::AccountHealthCheck { .. } => "account_health_check",
        QqManagementAction::AccountReconnect { .. } => "account_reconnect",
        QqManagementAction::AccountSendTest { .. } => "account_send_test",
        QqManagementAction::DeliveryRetry { .. } => "delivery_retry",
        QqManagementAction::DeliveryCancel { .. } => "delivery_cancel",
        QqManagementAction::DeliveryPreview { .. } => "delivery_preview",
        QqManagementAction::InteractionCancel { .. } => "interaction_cancel",
    }
}

fn revision_conflict(expected: u64, current: u64) -> QqManagementError {
    QqManagementError {
        code: "revision.conflict".into(),
        message: format!("expected revision {expected}, current {current}"),
    }
}

fn operation_conflict(operation_id: &str) -> QqManagementError {
    QqManagementError {
        code: "operation.conflict".into(),
        message: format!("management operation `{operation_id}` conflicts with its reservation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        BotConversationKind, InteractionScope, InteractionWaitSpec, QqMessageSegmentKind,
        QqPermissionRequirement, QqRateLimitPolicy, QqStreamingStrategy, QqUploadConstraints,
    };

    fn capability(account_id: &str) -> QqBotCapabilityMatrix {
        QqBotCapabilityMatrix {
            account_id: account_id.into(),
            conversation_kinds: vec![BotConversationKind::Private],
            outbound_conversation_kinds: vec![BotConversationKind::Private],
            active_message_kinds: vec![BotConversationKind::Private],
            inbound_segments: vec![QqMessageSegmentKind::Text],
            outbound_segments: vec![QqMessageSegmentKind::Text],
            inbound_media: Vec::new(),
            outbound_media: Vec::new(),
            active_message: true,
            message_edit: false,
            message_recall: true,
            reply: true,
            quote: true,
            mention: true,
            upload: QqUploadConstraints::default(),
            rate_limit: QqRateLimitPolicy::default(),
            streaming: vec![QqStreamingStrategy::FinalOnly],
            configured_intents: 1,
            shard: [0, 1],
            required_intents: vec!["group_and_c2c_event".into()],
            required_permissions: vec![QqPermissionRequirement::ReadC2cMessages],
        }
    }

    fn conversation() -> mutsuki_bot_protocol::QqConversationRef {
        mutsuki_bot_protocol::QqConversationRef {
            version: mutsuki_bot_protocol::QQ_CONVERSATION_REF_VERSION,
            kind: BotConversationKind::Private,
            account_id: "main".into(),
            user_id: Some("u1".into()),
            group_id: None,
            guild_id: None,
            channel_id: None,
            thread_id: None,
        }
    }

    fn service() -> (QqBotManagementService, Arc<LocalQqManagementProvider>) {
        let local = Arc::new(LocalQqManagementProvider::new());
        local.upsert_account(account_view_from_config(QqAccountViewInput {
            account_id: "main".into(),
            app_id: "app".into(),
            credential_reference: "QQBOT_CLIENT_SECRET".into(),
            credential_present: true,
            capability: capability("main"),
            intents: 1,
            shard: [0, 1],
            connected: true,
            identified: true,
            last_heartbeat_unix_ms: Some(10),
            last_error: None,
            reconnect_count: 0,
        }));
        local.upsert_delivery(delivery_view(
            BotDeliveryReceipt {
                delivery_id: "d1".into(),
                idempotency_key: "k1".into(),
                status: DeliveryStatus::RetryScheduled,
                attempt_count: 1,
                platform_message_ids: Vec::new(),
                part_receipts: Vec::new(),
                delivered_at_unix_ms: None,
                error_code: Some("transient".into()),
                generation: 1,
                lease_expires_at_unix_ms: None,
            },
            Vec::new(),
        ));
        local.upsert_interaction(BotInteractionSession {
            session_id: "i1".into(),
            conversation: conversation(),
            scope: InteractionScope::Conversation,
            actor_id: None,
            state_ref_id: "state".into(),
            wait: InteractionWaitSpec {
                event_kinds: Vec::new(),
                command: None,
                predicate_service_id: None,
                timeout_at_unix_ms: 1,
                retry_prompt: None,
            },
            status: InteractionStatus::Waiting,
            generation: 1,
            version: 1,
            exclusive: true,
            retries_remaining: 0,
        });
        (QqBotManagementService::new(local.clone()), local)
    }

    #[test]
    fn account_view_exposes_login_and_gateway_error() {
        let account = account_view_from_config(QqAccountViewInput {
            account_id: "main".into(),
            app_id: "app".into(),
            credential_reference: "QQBOT_CLIENT_SECRET".into(),
            credential_present: false,
            capability: capability("main"),
            intents: 1,
            shard: [0, 1],
            connected: false,
            identified: false,
            last_heartbeat_unix_ms: None,
            last_error: Some("identify rejected".into()),
            reconnect_count: 3,
        });
        assert_eq!(account.app_id, "app");
        assert_eq!(account.last_error.as_deref(), Some("identify rejected"));
        assert_eq!(account.reconnect_count, 3);
        assert_eq!(account.health, "unhealthy");
    }

    #[tokio::test]
    async fn revision_fence_audit_and_secret_redaction() {
        let (api, _) = service();
        let open = api.snapshot("", true).await.unwrap();
        assert_eq!(open.revision, 0);
        assert_eq!(open.accounts[0].credential_status, "configured");
        assert_eq!(open.accounts[0].app_id, "app");
        assert_eq!(open.accounts[0].reconnect_count, 0);
        assert_eq!(open.accounts[0].last_error, None);
        let redacted = api.snapshot("", false).await.unwrap();
        assert_eq!(redacted.accounts[0].credential_reference, "");
        assert_eq!(redacted.accounts[0].credential_status, "restricted");

        let stale = api.write(
            "op",
            QqManagementWriteRequest {
                operation_id: "stale".into(),
                expected_revision: 9,
                action: QqManagementAction::AccountReconnect {
                    account_id: "main".into(),
                },
            },
        );
        assert_eq!(stale.await.unwrap_err().code, "revision.conflict");

        let written = api
            .write(
                "op",
                QqManagementWriteRequest {
                    operation_id: "reconnect".into(),
                    expected_revision: 0,
                    action: QqManagementAction::AccountReconnect {
                        account_id: "main".into(),
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(written.revision, 1);
        assert_eq!(written.audit_id, "audit-1");
        let replayed = api
            .write(
                "op",
                QqManagementWriteRequest {
                    operation_id: "reconnect".into(),
                    expected_revision: 0,
                    action: QqManagementAction::AccountReconnect {
                        account_id: "main".into(),
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(replayed, written);
        let conflicting_replay = api
            .write(
                "op",
                QqManagementWriteRequest {
                    operation_id: "reconnect".into(),
                    expected_revision: 0,
                    action: QqManagementAction::AccountHealthCheck {
                        account_id: "main".into(),
                    },
                },
            )
            .await
            .unwrap_err();
        assert_eq!(conflicting_replay.code, "operation.conflict");
        assert_eq!(api.audits().len(), 1);
    }

    #[tokio::test]
    async fn delivery_and_interaction_writes_remain_operational() {
        let (api, _) = service();
        api.write(
            "op",
            QqManagementWriteRequest {
                operation_id: "retry".into(),
                expected_revision: 0,
                action: QqManagementAction::DeliveryRetry {
                    delivery_id: "d1".into(),
                },
            },
        )
        .await
        .unwrap();
        api.write(
            "op",
            QqManagementWriteRequest {
                operation_id: "cancel".into(),
                expected_revision: 1,
                action: QqManagementAction::InteractionCancel {
                    session_id: "i1".into(),
                },
            },
        )
        .await
        .unwrap();
        let snap = api.snapshot("", true).await.unwrap();
        assert_eq!(snap.revision, 2);
        assert_eq!(snap.deliveries[0].receipt.status, DeliveryStatus::Pending);
        assert_eq!(snap.interactions[0].status, InteractionStatus::Cancelled);
    }

    #[tokio::test]
    async fn delivery_preview_is_a_dry_run() {
        let (api, _) = service();
        let result = api
            .write(
                "op",
                QqManagementWriteRequest {
                    operation_id: "preview".into(),
                    expected_revision: 0,
                    action: QqManagementAction::DeliveryPreview {
                        delivery_id: "d1".into(),
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(result.result["status"], "previewed");
        assert_eq!(
            api.snapshot("", true).await.unwrap().deliveries[0]
                .receipt
                .status,
            DeliveryStatus::RetryScheduled
        );
    }
}
