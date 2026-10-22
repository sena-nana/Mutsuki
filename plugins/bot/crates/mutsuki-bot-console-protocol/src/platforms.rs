//! `platforms` namespace: one generic console surface for every platform adapter.
//!
//! An adapter integration describes itself with a [`PlatformAdapterView`] (which account fields
//! to show and which actions to offer) and reports [`PlatformAccountView`]s; the console renders
//! accounts and actions from these descriptors, so a new platform needs no new page.

use std::collections::BTreeMap;

use mutsuki_web_protocol::{RpcOperation, TypedRpc, TypedTopic};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PLATFORMS_NAMESPACE: &str = "platforms";
pub const PLATFORMS_READ_CAPABILITY: &str = "bot.platforms.read";
pub const PLATFORMS_WRITE_CAPABILITY: &str = "bot.platforms.write";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformAdapterKind {
    /// A chat platform the bot sends and receives messages on.
    Messaging,
    /// A content service account (push subscriptions, link resolution).
    ServiceAccount,
}

/// How the console renders one account field value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformFieldFormat {
    Text,
    Mono,
    /// Unix milliseconds rendered as a date-time.
    Timestamp,
    /// Unix milliseconds rendered as a duration since then.
    Since,
    Count,
    Status,
    /// `configured` / `missing` / `invalid`; never the secret itself.
    SecretStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAccountField {
    pub key: String,
    pub label: String,
    pub format: PlatformFieldFormat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAccountAction {
    pub id: String,
    pub label: String,
    /// Destructive or disruptive; the console asks for confirmation.
    pub danger: bool,
    pub capability: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAdapterView {
    pub adapter_id: String,
    /// `BotPlatform::as_str()` of the adapter.
    pub platform: String,
    pub label: String,
    pub kind: PlatformAdapterKind,
    /// Config provider rendered in the account's settings tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_provider_id: Option<String>,
    pub account_fields: Vec<PlatformAccountField>,
    pub account_actions: Vec<PlatformAccountAction>,
    /// Console slot ids (`platform.detail:<adapter>`) with adapter-specific panels.
    #[serde(default)]
    pub detail_panels: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformHealth {
    Ok,
    Degraded,
    Unhealthy,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAccountError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlatformAccountView {
    pub adapter_id: String,
    pub account_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub enabled: bool,
    pub health: PlatformHealth,
    /// Machine state (`identified`, `logged_in`, ...).
    pub connection_state: String,
    /// Values for [`PlatformAdapterView::account_fields`], keyed by field key.
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<PlatformAccountError>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountActionParams {
    pub adapter_id: String,
    pub account_id: String,
    pub action: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountActionResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub data: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformsChanged {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoParams {}

pub struct AdaptersList;

impl TypedRpc for AdaptersList {
    const NAMESPACE: &'static str = PLATFORMS_NAMESPACE;
    const METHOD: &'static str = "adapters.list";
    const OPERATION: RpcOperation = RpcOperation::Read;
    const CAPABILITY: &'static str = PLATFORMS_READ_CAPABILITY;
    const TS_PARAMS: &'static str = "NoParams";
    const TS_RESULT: &'static str = "PlatformAdapterView[]";
    type Params = NoParams;
    type Result = Vec<PlatformAdapterView>;
}

pub struct AccountsList;

impl TypedRpc for AccountsList {
    const NAMESPACE: &'static str = PLATFORMS_NAMESPACE;
    const METHOD: &'static str = "accounts.list";
    const OPERATION: RpcOperation = RpcOperation::Read;
    const CAPABILITY: &'static str = PLATFORMS_READ_CAPABILITY;
    const TS_PARAMS: &'static str = "AccountsListParams";
    const TS_RESULT: &'static str = "PlatformAccountView[]";
    type Params = AccountsListParams;
    type Result = Vec<PlatformAccountView>;
}

pub struct AccountsAction;

impl TypedRpc for AccountsAction {
    const NAMESPACE: &'static str = PLATFORMS_NAMESPACE;
    const METHOD: &'static str = "accounts.action";
    const OPERATION: RpcOperation = RpcOperation::Write;
    const CAPABILITY: &'static str = PLATFORMS_WRITE_CAPABILITY;
    const TS_PARAMS: &'static str = "AccountActionParams";
    const TS_RESULT: &'static str = "AccountActionResult";
    type Params = AccountActionParams;
    type Result = AccountActionResult;
}

pub struct Changed;

impl TypedTopic for Changed {
    const NAMESPACE: &'static str = PLATFORMS_NAMESPACE;
    const TOPIC: &'static str = "changed";
    const CAPABILITY: &'static str = PLATFORMS_READ_CAPABILITY;
    const TS_PAYLOAD: &'static str = "PlatformsChanged";
    type Payload = PlatformsChanged;
}
