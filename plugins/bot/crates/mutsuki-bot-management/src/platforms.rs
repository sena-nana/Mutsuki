//! Generic platform console providers: each adapter's management API projected onto the
//! descriptor-driven `platforms` console namespace.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use mutsuki_bot_console_protocol::platforms::{
    AccountActionParams, AccountActionResult, PLATFORMS_WRITE_CAPABILITY, PlatformAccountAction,
    PlatformAccountError, PlatformAccountField, PlatformAccountView, PlatformAdapterKind,
    PlatformAdapterView, PlatformFieldFormat, PlatformHealth,
};
use serde_json::{Value, json};

use crate::{
    BilibiliCredentialSecretState, BilibiliManagementApi, QqBotManagementApi,
    QqGatewayConnectionState, QqManagementAction, QqManagementWriteRequest,
    qq_gateway_error_message,
};

/// Structured failure of a platform console call (`code` is stable, `message` is for people).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformConsoleError {
    pub code: String,
    pub message: String,
}

impl PlatformConsoleError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// One adapter's surface in the generic platforms console.
#[async_trait]
pub trait PlatformConsoleProvider: Send + Sync {
    fn adapter(&self) -> PlatformAdapterView;

    async fn accounts(&self) -> Result<Vec<PlatformAccountView>, PlatformConsoleError>;

    async fn action(
        &self,
        actor_id: &str,
        request: AccountActionParams,
    ) -> Result<AccountActionResult, PlatformConsoleError>;
}

fn field(key: &str, label: &str, format: PlatformFieldFormat) -> PlatformAccountField {
    PlatformAccountField {
        key: key.into(),
        label: label.into(),
        format,
    }
}

fn action(id: &str, label: &str, danger: bool) -> PlatformAccountAction {
    PlatformAccountAction {
        id: id.into(),
        label: label.into(),
        danger,
        capability: PLATFORMS_WRITE_CAPABILITY.into(),
    }
}

/// The QQ adapter's accounts, backed by its management service.
pub struct QqPlatformConsoleProvider {
    adapter_id: String,
    api: Arc<dyn QqBotManagementApi>,
}

impl QqPlatformConsoleProvider {
    pub fn new(adapter_id: impl Into<String>, api: Arc<dyn QqBotManagementApi>) -> Self {
        Self {
            adapter_id: adapter_id.into(),
            api,
        }
    }
}

#[async_trait]
impl PlatformConsoleProvider for QqPlatformConsoleProvider {
    fn adapter(&self) -> PlatformAdapterView {
        PlatformAdapterView {
            adapter_id: self.adapter_id.clone(),
            platform: "qqbot".into(),
            label: "QQ".into(),
            kind: PlatformAdapterKind::Messaging,
            config_provider_id: Some(self.adapter_id.clone()),
            account_fields: vec![
                field("app_id", "AppID", PlatformFieldFormat::Mono),
                field("connected_since", "在线时长", PlatformFieldFormat::Since),
                field("last_heartbeat", "最近心跳", PlatformFieldFormat::Timestamp),
                field("reconnect_count", "重连次数", PlatformFieldFormat::Count),
                field("credential", "凭据", PlatformFieldFormat::SecretStatus),
                field("rate_limit", "限流", PlatformFieldFormat::Status),
            ],
            account_actions: vec![
                action("reconnect", "重新连接", true),
                action("health_check", "健康检查", false),
                action("enable", "启用", false),
                action("disable", "停用", true),
            ],
            detail_panels: Vec::new(),
        }
    }

    async fn accounts(&self) -> Result<Vec<PlatformAccountView>, PlatformConsoleError> {
        let snapshot = self
            .api
            .snapshot("", true)
            .await
            .map_err(|error| PlatformConsoleError::new(error.code, error.message))?;
        Ok(snapshot
            .accounts
            .into_iter()
            .map(|account| {
                let health = match (account.enabled, account.connection_state) {
                    (false, _) => PlatformHealth::Stopped,
                    (true, QqGatewayConnectionState::Identified) => PlatformHealth::Ok,
                    (
                        true,
                        QqGatewayConnectionState::Connected | QqGatewayConnectionState::Resumable,
                    ) => PlatformHealth::Degraded,
                    (true, QqGatewayConnectionState::Disconnected) => PlatformHealth::Unhealthy,
                };
                let last_error = account
                    .last_error_code
                    .clone()
                    .map(|code| PlatformAccountError {
                        message: qq_gateway_error_message(&code)
                            .map(str::to_owned)
                            .or_else(|| account.last_error.clone())
                            .unwrap_or_else(|| code.clone()),
                        code,
                    });
                let mut fields = BTreeMap::new();
                fields.insert("app_id".into(), json!(account.app_id));
                fields.insert(
                    "connected_since".into(),
                    json!(account.connected_since_unix_ms),
                );
                fields.insert(
                    "last_heartbeat".into(),
                    json!(account.last_heartbeat_unix_ms),
                );
                fields.insert("reconnect_count".into(), json!(account.reconnect_count));
                fields.insert("credential".into(), json!(account.credential_status));
                fields.insert("rate_limit".into(), json!(account.rate_limit_status));
                PlatformAccountView {
                    adapter_id: self.adapter_id.clone(),
                    display_name: account
                        .self_user
                        .as_ref()
                        .and_then(|user| user.display_name.clone())
                        .unwrap_or_else(|| account.account_id.clone()),
                    avatar_url: account
                        .self_user
                        .as_ref()
                        .and_then(|user| user.avatar_url.clone()),
                    account_id: account.account_id,
                    enabled: account.enabled,
                    health,
                    connection_state: serde_json::to_value(account.connection_state)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                    fields,
                    last_error,
                }
            })
            .collect())
    }

    async fn action(
        &self,
        actor_id: &str,
        request: AccountActionParams,
    ) -> Result<AccountActionResult, PlatformConsoleError> {
        let account_id = request.account_id;
        // Each console click is its own idempotent operation.
        let operation_id = format!(
            "platforms.{}.{}",
            request.action,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let action = match request.action.as_str() {
            "reconnect" => QqManagementAction::AccountReconnect { account_id },
            "health_check" => QqManagementAction::AccountHealthCheck { account_id },
            "enable" | "disable" => QqManagementAction::AccountSetEnabled {
                account_id,
                enabled: request.action == "enable",
            },
            other => {
                return Err(PlatformConsoleError::new(
                    "platform.action_unknown",
                    format!("QQ has no account action `{other}`"),
                ));
            }
        };
        let snapshot = self
            .api
            .snapshot("", false)
            .await
            .map_err(|error| PlatformConsoleError::new(error.code, error.message))?;
        let result = self
            .api
            .write(
                actor_id,
                QqManagementWriteRequest {
                    operation_id,
                    expected_revision: snapshot.revision,
                    action,
                },
            )
            .await
            .map_err(|error| PlatformConsoleError::new(error.code, error.message))?;
        Ok(AccountActionResult {
            message: None,
            data: result.result,
        })
    }
}

/// The Bilibili owner's login account; QR login and binding stay in its detail panel.
pub struct BilibiliPlatformConsoleProvider {
    adapter_id: String,
    api: Arc<dyn BilibiliManagementApi>,
}

impl BilibiliPlatformConsoleProvider {
    pub fn new(adapter_id: impl Into<String>, api: Arc<dyn BilibiliManagementApi>) -> Self {
        Self {
            adapter_id: adapter_id.into(),
            api,
        }
    }
}

#[async_trait]
impl PlatformConsoleProvider for BilibiliPlatformConsoleProvider {
    fn adapter(&self) -> PlatformAdapterView {
        PlatformAdapterView {
            adapter_id: self.adapter_id.clone(),
            platform: "bilibili".into(),
            label: "B 站".into(),
            kind: PlatformAdapterKind::ServiceAccount,
            config_provider_id: Some(self.adapter_id.clone()),
            account_fields: vec![
                field("credential", "Cookie", PlatformFieldFormat::SecretStatus),
                field("subscriptions", "订阅数", PlatformFieldFormat::Count),
                field("push_wired", "推送已接入流程", PlatformFieldFormat::Status),
            ],
            account_actions: vec![action("credential_clear", "清除登录凭据", true)],
            detail_panels: vec!["platform.detail:bilibili".into()],
        }
    }

    async fn accounts(&self) -> Result<Vec<PlatformAccountView>, PlatformConsoleError> {
        let status = self.api.status();
        let credential = match status.cookie_secret_state {
            Some(BilibiliCredentialSecretState::Present) => "configured",
            Some(BilibiliCredentialSecretState::Invalid) => "invalid",
            Some(BilibiliCredentialSecretState::Absent) | None => "missing",
        };
        let health = if !status.management_enabled {
            PlatformHealth::Stopped
        } else if status.credential_loaded {
            PlatformHealth::Ok
        } else {
            PlatformHealth::Degraded
        };
        let mut fields = BTreeMap::new();
        fields.insert("credential".into(), json!(credential));
        fields.insert("subscriptions".into(), json!(status.subscription_count));
        fields.insert(
            "push_wired".into(),
            status.push_wired.map_or(Value::Null, Value::Bool),
        );
        Ok(vec![PlatformAccountView {
            adapter_id: self.adapter_id.clone(),
            account_id: "default".into(),
            display_name: "B 站账号".into(),
            avatar_url: None,
            enabled: status.management_enabled,
            health,
            connection_state: if status.credential_loaded {
                "logged_in".into()
            } else {
                "logged_out".into()
            },
            fields,
            last_error: status.reason.map(|reason| PlatformAccountError {
                code: "bilibili.status".into(),
                message: reason,
            }),
        }])
    }

    async fn action(
        &self,
        _actor_id: &str,
        request: AccountActionParams,
    ) -> Result<AccountActionResult, PlatformConsoleError> {
        match request.action.as_str() {
            "credential_clear" => {
                self.api
                    .credential_clear()
                    .map_err(|error| PlatformConsoleError::new(error.code, error.message))?;
                Ok(AccountActionResult::default())
            }
            other => Err(PlatformConsoleError::new(
                "platform.action_unknown",
                format!("Bilibili has no account action `{other}`"),
            )),
        }
    }
}
