use std::collections::BTreeMap;
use std::fmt;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::ImageFormat;
use mutsuki_bot_link_parser::{MAX_LINK_CARD_MEDIA_BYTES, ResolvedLinkCard};
use mutsuki_bot_protocol::{
    BOT_COMMAND_HANDLE_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID, BotCommandEvent, BotExtMap,
    BotMessage, BotTarget, MessageSegment, bot_command_binding_id,
};
use mutsuki_protocol_browser::{
    BrowserSnapshot, BrowserSnapshotRequest, BrowserWaitMode, SNAPSHOT, SNAPSHOT_SCHEMA,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, DomainEvent, ExecutionClass, ProtocolClass, ReadPlan, RunnerBatchCapability,
    RunnerContext, RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect,
    RuntimeError, ScalarValue, Task, TaskOutcome, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, HandlerBindingBuilder, PluginBuilder, ProtocolDescriptorBuilder,
    ResourceRegistryGateway, RunnerDescriptorBuilder, RuntimeClientRef, TaskAwaitRunnerAdapter,
    map_work_batch_entries,
};
use qrcode::QrCode;
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

mod management;
mod open_platform;

pub use management::{
    BilibiliManagementService, BilibiliSecretPresence, BindChallengeResult, BindVerifyResult,
    CredentialSecretState, LoginPollResult, LoginStartResult, ManagementStatus, PreviewCardView,
    SubscriptionView,
};
pub use open_platform::{
    BilibiliOpenPlatformCredential, BilibiliOpenPlatformHttpClient,
    BilibiliOpenPlatformHttpRequest, BilibiliOpenPlatformHttpResponse,
    BilibiliOpenPlatformRequestContext, OpenPlatformHttpMethod,
    ReqwestBilibiliOpenPlatformTransport, open_platform_signed_headers,
};

pub const PLUGIN_ID: &str = "mutsuki.bot.bilibili";
pub const RUNNER_ID: &str = "mutsuki.bot.bilibili.runner";
pub const POLL_LIVE: &str = "mutsuki.bot.bilibili.poll/live@1";
pub const POLL_DYNAMIC: &str = "mutsuki.bot.bilibili.poll/dynamic@1";
pub const POLL_VIDEO: &str = "mutsuki.bot.bilibili.poll/video@1";
pub const LINK_RESOLVE: &str = "mutsuki.bot.bilibili.link/resolve@1";
pub const MANAGEMENT_COMMAND: &str = "mutsuki.bot.bilibili.management/command@1";
pub const RISK_CONTROL_STATUS_EVENT: &str = "mutsuki.bot.bilibili.risk_control/status@1";
pub const MAX_MEDIA_BYTES: usize = MAX_LINK_CARD_MEDIA_BYTES;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliPollKind {
    Live,
    Dynamic,
    Video,
}

impl BilibiliPollKind {
    pub fn protocol_id(&self) -> &'static str {
        match self {
            Self::Live => POLL_LIVE,
            Self::Dynamic => POLL_DYNAMIC,
            Self::Video => POLL_VIDEO,
        }
    }

    fn from_protocol_id(protocol_id: &str) -> Option<Self> {
        match protocol_id {
            POLL_LIVE => Some(Self::Live),
            POLL_DYNAMIC => Some(Self::Dynamic),
            POLL_VIDEO => Some(Self::Video),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PollRequest {
    pub subscription_id: String,
    pub uid: u64,
    pub target: BotTarget,
    pub outbound_binding: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkResolveRequest {
    pub url: String,
    pub target: BotTarget,
    pub outbound_binding: String,
    pub account_id: String,
    pub now_ms: u64,
    pub cooldown_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BilibiliSubscription {
    pub subscription_id: String,
    pub uid: u64,
    pub notifications: Vec<BilibiliPollKind>,
    pub target: BotTarget,
    pub outbound_binding: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
}

fn default_management_command() -> String {
    "bili".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BilibiliManagementConfig {
    pub enabled: bool,
    pub allow_self_binding: bool,
    pub command: String,
    pub admin_user_ids: Vec<String>,
    pub self_binding_notifications: Vec<BilibiliPollKind>,
    pub self_binding_outbound_binding: String,
}

impl Default for BilibiliManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_self_binding: false,
            command: default_management_command(),
            admin_user_ids: Vec::new(),
            self_binding_notifications: vec![
                BilibiliPollKind::Live,
                BilibiliPollKind::Dynamic,
                BilibiliPollKind::Video,
            ],
            self_binding_outbound_binding: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkResolverConfig {
    pub enabled: bool,
    pub cooldown_ms: u64,
    pub account_to_binding: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliRiskControlBackend {
    Chromium,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilibiliRiskControlConfig {
    pub backend: BilibiliRiskControlBackend,
    pub timeout_ms: u64,
    pub max_response_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BilibiliBackendConfig {
    WebCookie {
        cookie_secret_key: String,
    },
    OpenPlatform {
        client_id: String,
        app_secret_key: String,
        oauth_credential_key: String,
        authorized_uid: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BilibiliBackendKind {
    WebCookie,
    OpenPlatform,
}

impl BilibiliBackendConfig {
    pub fn kind(&self) -> BilibiliBackendKind {
        match self {
            Self::WebCookie { .. } => BilibiliBackendKind::WebCookie,
            Self::OpenPlatform { .. } => BilibiliBackendKind::OpenPlatform,
        }
    }

    pub fn cookie_secret_key(&self) -> Option<&str> {
        match self {
            Self::WebCookie { cookie_secret_key } => Some(cookie_secret_key),
            Self::OpenPlatform { .. } => None,
        }
    }
}

impl BilibiliRiskControlConfig {
    fn validate(&self) -> Result<(), String> {
        if self.timeout_ms == 0 {
            return Err("risk_control.timeout_ms must be greater than zero".into());
        }
        if self.max_response_bytes == 0 {
            return Err("risk_control.max_response_bytes must be greater than zero".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilibiliConfig {
    pub backend: BilibiliBackendConfig,
    pub live_interval_ms: u64,
    pub dynamic_interval_ms: u64,
    pub video_interval_ms: u64,
    pub retry: RetryConfig,
    pub subscriptions: Vec<BilibiliSubscription>,
    pub link_resolver: LinkResolverConfig,
    pub media_provider_id: String,
    #[serde(default)]
    pub risk_control: Option<BilibiliRiskControlConfig>,
    #[serde(default)]
    pub management: BilibiliManagementConfig,
}

impl BilibiliConfig {
    pub fn validate(&self) -> Result<(), String> {
        match &self.backend {
            BilibiliBackendConfig::WebCookie { cookie_secret_key } => {
                if cookie_secret_key.trim().is_empty() {
                    return Err("backend.cookie_secret_key is required".into());
                }
            }
            BilibiliBackendConfig::OpenPlatform {
                client_id,
                app_secret_key,
                oauth_credential_key,
                authorized_uid,
            } => {
                if client_id.trim().is_empty()
                    || app_secret_key.trim().is_empty()
                    || oauth_credential_key.trim().is_empty()
                    || authorized_uid == &0
                {
                    return Err("open_platform backend requires client_id, app_secret_key, oauth_credential_key and authorized_uid".into());
                }
                if app_secret_key == oauth_credential_key {
                    return Err("open_platform secret keys must be distinct".into());
                }
                if self.management.enabled || self.risk_control.is_some() {
                    return Err("open_platform backend does not support Cookie management or Chromium risk control".into());
                }
                if self.link_resolver.enabled {
                    return Err("open_platform backend does not support Web link resolution".into());
                }
                for subscription in &self.subscriptions {
                    if subscription.uid != *authorized_uid {
                        return Err("open_platform subscriptions must target authorized_uid".into());
                    }
                    if subscription
                        .notifications
                        .iter()
                        .any(|kind| matches!(kind, BilibiliPollKind::Dynamic))
                    {
                        return Err("open_platform backend does not provide poll/dynamic".into());
                    }
                }
            }
        }
        if self.media_provider_id.trim().is_empty() {
            return Err("media_provider_id is required".into());
        }
        if [
            self.live_interval_ms,
            self.dynamic_interval_ms,
            self.video_interval_ms,
            self.retry.initial_backoff_ms,
            self.retry.max_backoff_ms,
        ]
        .contains(&0)
            || self.retry.max_attempts == 0
            || self.retry.initial_backoff_ms > self.retry.max_backoff_ms
        {
            return Err("poll intervals and retry/backoff must be positive and ordered".into());
        }
        for subscription in &self.subscriptions {
            if subscription.subscription_id.trim().is_empty()
                || subscription.uid == 0
                || subscription.notifications.is_empty()
                || subscription.outbound_binding.trim().is_empty()
            {
                return Err("subscriptions require id, uid, notification types and binding".into());
            }
        }
        let mut ids = self
            .subscriptions
            .iter()
            .map(|subscription| subscription.subscription_id.trim())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("subscription_id values must be unique".into());
        }
        if self.management.enabled
            && (self.management.command.trim().is_empty()
                || self
                    .management
                    .self_binding_outbound_binding
                    .trim()
                    .is_empty()
                || (self.management.allow_self_binding
                    && self.management.self_binding_notifications.is_empty()))
        {
            return Err("enabled management requires a command and self-binding defaults".into());
        }
        if let Some(risk_control) = &self.risk_control {
            risk_control.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SharedBilibiliConfig(Arc<RwLock<BilibiliConfig>>);

impl SharedBilibiliConfig {
    pub fn new(config: BilibiliConfig) -> Self {
        Self(Arc::new(RwLock::new(config)))
    }

    pub fn snapshot(&self) -> BilibiliConfig {
        self.0.read().expect("Bilibili config read lock").clone()
    }

    pub fn replace(&self, config: BilibiliConfig) {
        *self.0.write().expect("Bilibili config write lock") = config;
    }
}

impl fmt::Debug for SharedBilibiliConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedBilibiliConfig(..)")
    }
}

#[derive(Clone, Default)]
pub struct SharedBilibiliCredential(Arc<Mutex<Option<String>>>);

impl fmt::Debug for SharedBilibiliCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedBilibiliCredential([REDACTED])")
    }
}

impl SharedBilibiliCredential {
    pub fn set(&self, cookie: String) {
        *self.0.lock().expect("credential mutex") = Some(cookie);
    }

    pub fn clear(&self) {
        *self.0.lock().expect("credential mutex") = None;
    }

    pub fn is_loaded(&self) -> bool {
        self.0
            .lock()
            .expect("credential mutex")
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
    }

    pub fn raw(&self) -> Option<String> {
        self.0
            .lock()
            .expect("credential mutex")
            .clone()
            .filter(|value| !value.trim().is_empty())
    }

    fn get(&self) -> Result<String, BilibiliError> {
        self.raw().ok_or(BilibiliError::CookieExpired)
    }

    fn get_named(&self, name: &str) -> Result<String, BilibiliError> {
        self.raw()
            .ok_or_else(|| BilibiliError::OpenPlatformCredentialUnavailable(name.into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilibiliItem {
    pub id: String,
    pub title: String,
    pub url: String,
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BilibiliProfile {
    pub name: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilibiliQrCode {
    pub url: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilibiliQrStatus {
    Pending,
    Scanned,
    Expired,
    Confirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BilibiliQrPoll {
    pub status: BilibiliQrStatus,
    pub credential: Option<String>,
}

pub trait BilibiliCredentialStore: Send + Sync {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String>;
}

pub trait BilibiliConfigStore: Send + Sync {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
pub enum BilibiliError {
    #[error("Bilibili cookie is missing or expired")]
    CookieExpired,
    #[error("Bilibili request was rate limited")]
    RateLimited,
    #[error("Bilibili risk control rejected the request (code 352)")]
    RiskControl352,
    #[error("Bilibili domain is not allowed: {0}")]
    DomainDenied(String),
    #[error("Bilibili response is invalid: {0}")]
    InvalidResponse(String),
    #[error("Bilibili transport failed: {0}")]
    Transport(String),
    #[error("Bilibili management is unavailable: {0}")]
    ManagementUnavailable(String),
    #[error("Bilibili management request is forbidden")]
    Forbidden,
    #[error("Bilibili Open Platform credential is unavailable: {0}")]
    OpenPlatformCredentialUnavailable(String),
    #[error("Bilibili Open Platform credential is invalid: {0}")]
    OpenPlatformCredentialInvalid(String),
    #[error("Bilibili Open Platform permission is unavailable: {scope} (code {code})")]
    OpenPlatformPermissionDenied {
        code: i64,
        scope: String,
        request_id: Option<String>,
    },
    #[error("Bilibili Open Platform OAuth credential is expired")]
    OpenPlatformOAuthExpired { request_id: Option<String> },
    #[error("Bilibili Open Platform signature was rejected (code {code})")]
    OpenPlatformSignatureRejected {
        code: i64,
        request_id: Option<String>,
    },
    #[error("Bilibili Open Platform request failed with code {code}: {message}")]
    OpenPlatformApi {
        code: i64,
        message: String,
        request_id: Option<String>,
    },
    #[error("Bilibili Open Platform does not support capability: {0}")]
    OpenPlatformUnsupported(String),
}

pub trait BilibiliTransport: Send {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError>;
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError>;
    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError>;
    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError>;
    fn qr_poll(&mut self, key: &str) -> Result<BilibiliQrPoll, BilibiliError>;
    fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError>;
}

pub struct ReqwestBilibiliTransport {
    client: Option<Client>,
    credential: SharedBilibiliCredential,
    timeout: Duration,
}

impl ReqwestBilibiliTransport {
    pub fn new(credential: SharedBilibiliCredential, timeout: Duration) -> Self {
        Self {
            client: None,
            credential,
            timeout,
        }
    }

    fn client(&mut self) -> Result<&Client, BilibiliError> {
        if self.client.is_none() {
            self.client = Some(
                Client::builder()
                    .timeout(self.timeout)
                    .user_agent("Mozilla/5.0 MutsukiBot/0.1")
                    .build()
                    .map_err(|error| BilibiliError::Transport(error.to_string()))?,
            );
        }
        Ok(self.client.as_ref().expect("client initialized"))
    }

    fn json(&mut self, url: &str) -> Result<Value, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let cookie = self.credential.get()?;
        let response = self
            .client()?
            .get(url)
            .header("Cookie", cookie)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        if response.status().as_u16() == 429 {
            return Err(BilibiliError::RateLimited);
        }
        let value: Value = response
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        match value.get("code").and_then(Value::as_i64) {
            Some(-101) => Err(BilibiliError::CookieExpired),
            Some(-352 | 352) => Err(BilibiliError::RiskControl352),
            Some(code) if code != 0 => Err(BilibiliError::InvalidResponse(format!("code {code}"))),
            _ => Ok(value),
        }
    }

    fn wbi_url(
        &mut self,
        path: &str,
        params: Vec<(String, String)>,
    ) -> Result<String, BilibiliError> {
        let nav = self.json("https://api.bilibili.com/x/web-interface/nav")?;
        let img = string_field(&nav["data"]["wbi_img"], "img_url")?;
        let sub = string_field(&nav["data"]["wbi_img"], "sub_url")?;
        let key = wbi_mixin_key(&img, &sub)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .as_secs() as i64;
        Ok(format!(
            "https://api.bilibili.com{path}?{}",
            sign_wbi_query(&params, &key, now)
        ))
    }
}

impl BilibiliTransport for ReqwestBilibiliTransport {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError> {
        let url = match kind {
            BilibiliPollKind::Live => self.wbi_url(
                "/x/space/wbi/acc/info",
                vec![("mid".into(), uid.to_string())],
            )?,
            BilibiliPollKind::Dynamic => format!(
                "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space?host_mid={uid}"
            ),
            BilibiliPollKind::Video => self.wbi_url(
                "/x/space/wbi/arc/search",
                vec![
                    ("mid".into(), uid.to_string()),
                    ("pn".into(), "1".into()),
                    ("ps".into(), "10".into()),
                ],
            )?,
        };
        parse_poll_items(kind, uid, self.json(&url)?)
    }

    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let mut parsed =
            Url::parse(url).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        if parsed.host_str() == Some("b23.tv") {
            let response = self
                .client()?
                .get(parsed.clone())
                .send()
                .map_err(|error| BilibiliError::Transport(error.to_string()))?;
            parsed = response.url().clone();
            ensure_bilibili_domain(parsed.as_str())?;
        }
        let path = parsed.path();
        let bvid = path.split('/').find(|part| part.starts_with("BV"));
        let api = bvid
            .map(|bvid| format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}"))
            .ok_or_else(|| BilibiliError::InvalidResponse("unsupported Bilibili link".into()))?;
        let value = self.json(&api)?;
        let data = &value["data"];
        Ok(ResolvedLinkCard {
            url: parsed.to_string(),
            title: string_field(data, "title")?,
            description: data
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            image_url: data
                .get("pic")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        })
    }

    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let response = self
            .client()?
            .get(url)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        let bytes = response
            .bytes()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        if bytes.len() > max_bytes {
            return Err(BilibiliError::InvalidResponse(
                "media exceeds configured limit".into(),
            ));
        }
        Ok(bytes.to_vec())
    }

    fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
        let value: Value = self
            .client()?
            .get("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        if value.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(BilibiliError::InvalidResponse(
                "QR generation failed".into(),
            ));
        }
        Ok(BilibiliQrCode {
            url: string_field(&value["data"], "url")?,
            key: string_field(&value["data"], "qrcode_key")?,
        })
    }

    fn qr_poll(&mut self, key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
        let mut url = Url::parse("https://passport.bilibili.com/x/passport-login/web/qrcode/poll")
            .expect("static Bilibili QR URL");
        url.query_pairs_mut()
            .append_pair("qrcode_key", key)
            .append_pair("source", "main-fe-header");
        let value: Value = self
            .client()?
            .get(url)
            .send()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .json()
            .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
        let code = value["data"]["code"]
            .as_i64()
            .ok_or_else(|| BilibiliError::InvalidResponse("QR status code".into()))?;
        let status = match code {
            86101 => BilibiliQrStatus::Pending,
            86090 => BilibiliQrStatus::Scanned,
            86038 => BilibiliQrStatus::Expired,
            0 => BilibiliQrStatus::Confirmed,
            _ => {
                return Err(BilibiliError::InvalidResponse(format!(
                    "QR status code {code}"
                )));
            }
        };
        let credential = if status == BilibiliQrStatus::Confirmed {
            Some(credential_from_redirect(
                string_field(&value["data"], "url")?.as_str(),
            )?)
        } else {
            None
        };
        Ok(BilibiliQrPoll { status, credential })
    }

    fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError> {
        let url = self.wbi_url(
            "/x/space/wbi/acc/info",
            vec![("mid".into(), uid.to_string())],
        )?;
        let value = self.json(&url)?;
        Ok(BilibiliProfile {
            name: string_field(&value["data"], "name")?,
            signature: value["data"]["sign"].as_str().unwrap_or_default().into(),
        })
    }
}

pub struct SqliteBilibiliRepository {
    connection: Mutex<Connection>,
}

impl SqliteBilibiliRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| rusqlite::Error::InvalidPath(parent.into()))?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS cursor (key TEXT PRIMARY KEY, value TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS cooldown (key TEXT PRIMARY KEY, seen_ms INTEGER NOT NULL);\
             CREATE TABLE IF NOT EXISTS qr_session (actor_id TEXT PRIMARY KEY, qr_key TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS binding_challenge (actor_id TEXT PRIMARY KEY, uid INTEGER NOT NULL, code TEXT NOT NULL);",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn cursor(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row("SELECT value FROM cursor WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
    }

    fn set_cursor(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO cursor(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn admit_cooldown(
        &self,
        key: &str,
        now_ms: u64,
        cooldown_ms: u64,
    ) -> Result<bool, rusqlite::Error> {
        let connection = self.connection.lock().expect("sqlite mutex");
        let previous: Option<u64> = connection
            .query_row(
                "SELECT seen_ms FROM cooldown WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        if previous.is_some_and(|previous| now_ms.saturating_sub(previous) < cooldown_ms) {
            return Ok(false);
        }
        connection.execute(
            "INSERT INTO cooldown(key,seen_ms) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET seen_ms=excluded.seen_ms",
            params![key, now_ms],
        )?;
        Ok(true)
    }

    pub fn set_qr_session(&self, actor_id: &str, key: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO qr_session(actor_id,qr_key) VALUES(?1,?2) ON CONFLICT(actor_id) DO UPDATE SET qr_key=excluded.qr_key",
            params![actor_id, key],
        )?;
        Ok(())
    }

    pub fn qr_session(&self, actor_id: &str) -> Result<Option<String>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row(
                "SELECT qr_key FROM qr_session WHERE actor_id = ?1",
                [actor_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn clear_qr_session(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .execute("DELETE FROM qr_session WHERE actor_id = ?1", [actor_id])?;
        Ok(())
    }

    pub fn set_binding_challenge(
        &self,
        actor_id: &str,
        uid: u64,
        code: &str,
    ) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO binding_challenge(actor_id,uid,code) VALUES(?1,?2,?3) ON CONFLICT(actor_id) DO UPDATE SET uid=excluded.uid,code=excluded.code",
            params![actor_id, uid, code],
        )?;
        Ok(())
    }

    pub fn binding_challenge(
        &self,
        actor_id: &str,
    ) -> Result<Option<(u64, String)>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row(
                "SELECT uid,code FROM binding_challenge WHERE actor_id = ?1",
                [actor_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    pub fn clear_binding_challenge(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "DELETE FROM binding_challenge WHERE actor_id = ?1",
            [actor_id],
        )?;
        Ok(())
    }
}

pub struct BilibiliRunner {
    descriptor: RunnerDescriptor,
    backend_kind: BilibiliBackendKind,
    transport: Box<dyn BilibiliTransport>,
    repository: Arc<SqliteBilibiliRepository>,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
    managed_config: Option<SharedBilibiliConfig>,
    management: Option<Arc<BilibiliManagementService>>,
}

impl BilibiliRunner {
    pub fn new(
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
    ) -> Self {
        Self::new_for_backend(
            transport,
            repository,
            resources,
            media_provider_id,
            BilibiliBackendKind::WebCookie,
        )
    }

    pub fn new_for_backend(
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
        backend_kind: BilibiliBackendKind,
    ) -> Self {
        Self {
            descriptor: runner_descriptor(false, false, backend_kind),
            backend_kind,
            transport,
            repository,
            resources,
            media_provider_id: media_provider_id.into(),
            managed_config: None,
            management: None,
        }
    }

    pub fn with_management(mut self, management: Arc<BilibiliManagementService>) -> Self {
        self.descriptor = runner_descriptor(true, false, self.backend_kind);
        self.managed_config = Some(management.config().clone());
        self.management = Some(management);
        self
    }

    pub fn into_runtime_runner(
        mut self,
        client: RuntimeClientRef,
        risk_control: Option<BilibiliRiskControlConfig>,
    ) -> Box<dyn Runner> {
        let Some(risk_control) = risk_control else {
            return Box::new(self);
        };
        let descriptor = runner_descriptor(self.managed_config.is_some(), true, self.backend_kind);
        self.descriptor = descriptor.clone();
        let state = Arc::new(Mutex::new(self));
        let factory = Box::new(move |ctx: AsyncRunnerContext, task: Task| {
            let state = state.clone();
            let risk_control = risk_control.clone();
            Box::pin(
                async move { run_task_with_risk_control(ctx, task, state, risk_control).await },
            )
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
                >
        });
        Box::new(
            TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false),
        )
    }

    fn run_task(&mut self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        if task.protocol_id == MANAGEMENT_COMMAND {
            return self.run_command(task);
        }
        if task.protocol_id == LINK_RESOLVE {
            let request: LinkResolveRequest = decode(task)?;
            let cooldown_key = format!("{}:{}", request.account_id, request.url);
            if !self
                .repository
                .admit_cooldown(&cooldown_key, request.now_ms, request.cooldown_ms)
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?
            {
                return Ok(RunnerResult::completed(task.task_id.clone()));
            }
            let card = self
                .transport
                .resolve(&request.url)
                .map_err(|error| bili_error(task, error))?;
            let message = self.card_message(request.target, card, task)?;
            return Ok(outbound_result(task, message, request.outbound_binding));
        }
        let request: PollRequest = decode(task)?;
        let kind = BilibiliPollKind::from_protocol_id(&task.protocol_id).ok_or_else(|| {
            bili_error(
                task,
                BilibiliError::InvalidResponse("unsupported poll protocol".into()),
            )
        })?;
        let items = self
            .transport
            .poll(&kind, request.uid)
            .map_err(|error| bili_error(task, error))?;
        self.finish_poll(task, request, kind, items, None)
    }

    fn finish_poll(
        &mut self,
        task: &Task,
        request: PollRequest,
        kind: BilibiliPollKind,
        items: Vec<BilibiliItem>,
        status: Option<DomainEvent>,
    ) -> Result<RunnerResult, RuntimeError> {
        let key = format!("{kind:?}:{}:{}", request.uid, request.subscription_id);
        let previous = self
            .repository
            .cursor(&key)
            .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
        let Some(head) = items.first().map(|item| item.id.clone()) else {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.events.extend(status);
            return Ok(result);
        };
        self.repository
            .set_cursor(&key, &head)
            .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
        let Some(previous) = previous else {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.events.extend(status);
            return Ok(result);
        };
        let fresh = fresh_since(items, &previous);
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.events.extend(status);
        for item in fresh {
            let card = ResolvedLinkCard {
                url: item.url,
                title: item.title,
                description: match &kind {
                    BilibiliPollKind::Live => "直播状态更新",
                    BilibiliPollKind::Dynamic => "发布了新动态",
                    BilibiliPollKind::Video => "发布了新投稿",
                }
                .into(),
                image_url: item.image_url,
            };
            let message = self.card_message(request.target.clone(), card, task)?;
            result.tasks.push(outbound_task(
                task,
                message,
                &request.outbound_binding,
                result.tasks.len(),
            ));
        }
        Ok(result)
    }

    fn run_command(&mut self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        let command: BotCommandEvent = decode(task)?;
        let Some(management) = self.management.clone() else {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        };
        let config = management.config().snapshot();
        if !config.management.enabled
            || !command
                .name
                .eq_ignore_ascii_case(&config.management.command)
        {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        }
        let actor_id = command
            .source
            .actor
            .as_ref()
            .map(|actor| actor.user_id.as_str())
            .ok_or_else(|| bili_error(task, BilibiliError::Forbidden))?;
        let action = command.args.first().map(String::as_str).unwrap_or("help");
        let is_admin = config
            .management
            .admin_user_ids
            .iter()
            .any(|candidate| candidate == actor_id);
        match action {
            "help" => Ok(self.command_reply(
                task,
                &command,
                "可用命令：login、login-status、bind <uid>、verify、unbind、pause [订阅/UID]、resume [订阅/UID]、preview [订阅/UID]、list；管理员另可使用 subscribe <id> <uid> [live,dynamic,video] 与 unsubscribe <id>。",
                None,
            )),
            "login" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let started = management
                    .login_start(actor_id)
                    .map_err(|error| bili_error(task, error))?;
                let image = self.qr_resource_from_png(started.qr_png, task)?;
                Ok(self.command_reply(
                    task,
                    &command,
                    "请使用 Bilibili App 扫码确认，然后发送 /bili login-status；二维码不会把 Cookie 写入聊天或 Task payload。",
                    Some(image),
                ))
            }
            "login-status" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let polled = management
                    .login_poll(actor_id)
                    .map_err(|error| bili_error(task, error))?;
                Ok(self.command_reply(task, &command, polled.message, None))
            }
            "bind" => {
                let uid = parse_uid(command.args.get(1)).map_err(|error| bili_error(task, error))?;
                let challenge = management
                    .bind_start(actor_id, uid, &task.task_id)
                    .map_err(|error| bili_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    &format!(
                        "已为 {} ({}) 创建验证。请临时把 {} 加入 Bilibili 个性签名，然后发送 /{} verify。",
                        challenge.name, challenge.uid, challenge.code, config.management.command
                    ),
                    None,
                ))
            }
            "verify" => {
                match management
                    .bind_verify(
                        actor_id,
                        command.source.platform.as_str(),
                        command.source.target.clone(),
                    )
                    .map_err(|error| bili_error(task, error))?
                {
                    BindVerifyResult::Verified(subscription) => Ok(self.command_reply(
                        task,
                        &command,
                        &format!(
                            "验证成功，已绑定 UID {} 并写入产品配置。",
                            subscription.uid
                        ),
                        None,
                    )),
                    BindVerifyResult::SignatureMismatch { code } => Ok(self.command_reply(
                        task,
                        &command,
                        &format!("验证未通过：个性签名中尚未找到 {code}。"),
                        None,
                    )),
                }
            }
            "unbind" => {
                let removed = management
                    .unbind(actor_id)
                    .map_err(|error| bili_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    if removed {
                        "已解除绑定并更新产品配置。"
                    } else {
                        "当前没有自助绑定。"
                    },
                    None,
                ))
            }
            "pause" | "resume" => {
                let paused = action == "pause";
                let view = management
                    .set_paused(
                        actor_id,
                        is_admin,
                        command.args.get(1).map(String::as_str),
                        paused,
                    )
                    .map_err(|error| bili_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    &format!(
                        "订阅 {} 已{}。",
                        view.subscription_id,
                        if paused { "暂停" } else { "恢复" }
                    ),
                    None,
                ))
            }
            "preview" => {
                match management.preview(
                    actor_id,
                    is_admin,
                    command.args.get(1).map(String::as_str),
                ) {
                    Ok(card) => {
                        let message = self.card_message(
                            command.source.target.clone(),
                            ResolvedLinkCard {
                                url: card.url,
                                title: card.title,
                                description: card.description,
                                image_url: card.image_url,
                            },
                            task,
                        )?;
                        Ok(command_outbound_result(
                            task,
                            message,
                            Some(&config.management.self_binding_outbound_binding),
                        ))
                    }
                    Err(BilibiliError::ManagementUnavailable(message))
                        if message.contains("暂无可预览") =>
                    {
                        Ok(self.command_reply(task, &command, message, None))
                    }
                    Err(error) => Err(bili_error(task, error)),
                }
            }
            "list" => {
                let lines = management
                    .list(actor_id, is_admin)
                    .map_err(|error| bili_error(task, error))?
                    .into_iter()
                    .map(|subscription| {
                        format!(
                            "{} -> UID {} [{}]{}",
                            subscription.subscription_id,
                            subscription.uid,
                            subscription
                                .notifications
                                .iter()
                                .map(|kind| format!("{kind:?}").to_ascii_lowercase())
                                .collect::<Vec<_>>()
                                .join(","),
                            if subscription.paused { " (paused)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(self.command_reply(
                    task,
                    &command,
                    if lines.is_empty() {
                        "没有可管理的订阅。".into()
                    } else {
                        lines.join("\n")
                    },
                    None,
                ))
            }
            "subscribe" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let subscription_id = required_arg(&command.args, 1, "subscription id")
                    .map_err(|error| bili_error(task, error))?;
                let uid = parse_uid(command.args.get(2)).map_err(|error| bili_error(task, error))?;
                let notifications = parse_notifications(command.args.get(3))
                    .map_err(|error| bili_error(task, error))?;
                management
                    .subscribe(
                        subscription_id.clone(),
                        uid,
                        notifications,
                        command.source.target.clone(),
                        config.management.self_binding_outbound_binding.clone(),
                    )
                    .map_err(|error| bili_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    &format!("订阅 {subscription_id} 已写入产品配置。"),
                    None,
                ))
            }
            "unsubscribe" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let subscription_id = required_arg(&command.args, 1, "subscription id")
                    .map_err(|error| bili_error(task, error))?;
                management
                    .unsubscribe(&subscription_id)
                    .map_err(|error| bili_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    &format!("订阅 {subscription_id} 已从产品配置删除。"),
                    None,
                ))
            }
            _ => Ok(self.command_reply(task, &command, "未知 Bilibili 管理命令。", None)),
        }
    }

    fn qr_resource_from_png(
        &self,
        bytes: Vec<u8>,
        task: &Task,
    ) -> Result<mutsuki_runtime_contracts::ResourceRef, RuntimeError> {
        self.resources
            .create_blob_resource(
                &self.media_provider_id,
                "mutsuki.bot.image.qrcode.png.v1",
                bytes,
            )
            .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))
    }

    fn command_reply(
        &self,
        task: &Task,
        command: &BotCommandEvent,
        text: impl Into<String>,
        image: Option<mutsuki_runtime_contracts::ResourceRef>,
    ) -> RunnerResult {
        let mut segments = Vec::new();
        if let Some(resource) = image {
            segments.push(MessageSegment::Image { resource });
        }
        segments.push(MessageSegment::Text { text: text.into() });
        let binding = self
            .managed_config
            .as_ref()
            .map(SharedBilibiliConfig::snapshot)
            .map(|config| config.management.self_binding_outbound_binding);
        command_outbound_result(
            task,
            BotMessage {
                message_id: None,
                target: command.source.target.clone(),
                sender: None,
                segments,
                reply_to: command
                    .source
                    .message
                    .as_ref()
                    .and_then(|message| message.message_id.clone()),
                time_ms: None,
                ext: BotExtMap::new(),
            },
            binding.as_deref(),
        )
    }

    fn card_message(
        &mut self,
        target: BotTarget,
        card: ResolvedLinkCard,
        task: &Task,
    ) -> Result<BotMessage, RuntimeError> {
        let mut segments = Vec::new();
        if let Some(image_url) = card.image_url {
            let bytes = self
                .transport
                .download(&image_url, MAX_MEDIA_BYTES)
                .map_err(|error| bili_error(task, error))?;
            let resource = self
                .resources
                .create_blob_resource(
                    &self.media_provider_id,
                    "mutsuki.bot.image.original.v1",
                    bytes,
                )
                .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
            segments.push(MessageSegment::Image { resource });
        }
        segments.push(MessageSegment::Text {
            text: format!("{}\n{}\n{}", card.title, card.description, card.url),
        });
        Ok(BotMessage {
            message_id: None,
            target,
            sender: None,
            segments,
            reply_to: None,
            time_ms: None,
            ext: BotExtMap::new(),
        })
    }
}

async fn run_task_with_risk_control(
    ctx: AsyncRunnerContext,
    task: Task,
    state: Arc<Mutex<BilibiliRunner>>,
    risk_control: BilibiliRiskControlConfig,
) -> RuntimeResult<RunnerResult> {
    if task.protocol_id != POLL_DYNAMIC {
        return state
            .lock()
            .expect("Bilibili runner mutex")
            .run_task(&task)
            .map_err(RuntimeFailure::new);
    }

    let request: PollRequest = decode(&task).map_err(RuntimeFailure::new)?;
    let attempt = state
        .lock()
        .expect("Bilibili runner mutex")
        .transport
        .poll(&BilibiliPollKind::Dynamic, request.uid);
    match attempt {
        Ok(items) => state
            .lock()
            .expect("Bilibili runner mutex")
            .finish_poll(&task, request, BilibiliPollKind::Dynamic, items, None)
            .map_err(RuntimeFailure::new),
        Err(BilibiliError::RiskControl352) => {
            run_chromium_risk_control_fallback(ctx, task, request, state, risk_control).await
        }
        Err(error) => Err(RuntimeFailure::new(bili_error(&task, error))),
    }
}

async fn run_chromium_risk_control_fallback(
    ctx: AsyncRunnerContext,
    task: Task,
    request: PollRequest,
    state: Arc<Mutex<BilibiliRunner>>,
    risk_control: BilibiliRiskControlConfig,
) -> RuntimeResult<RunnerResult> {
    let (resources, media_provider_id) = {
        let runner = state.lock().expect("Bilibili runner mutex");
        (runner.resources.clone(), runner.media_provider_id.clone())
    };
    let output = resources
        .create_cow_state_resource(
            &media_provider_id,
            "mutsuki.browser.snapshot.output",
            SNAPSHOT_SCHEMA,
            Vec::new(),
        )
        .map_err(|error| risk_control_failure(&task, "resource.create", error.to_string()))?;
    let snapshot_request = BrowserSnapshotRequest {
        url: format!("https://space.bilibili.com/{}/dynamic", request.uid),
        output_resource: output.clone(),
        wait_mode: BrowserWaitMode::Selector,
        selector: Some("body".into()),
        timeout_ms: risk_control.timeout_ms,
    };
    let outcome = ctx
        .call_raw(
            SNAPSHOT,
            serde_json::to_value(snapshot_request)
                .map_err(|error| risk_control_failure(&task, "request.encode", error))?,
        )
        .await
        .map_err(|error| risk_control_failure(&task, "snapshot.task", error.to_string()))?;
    if !matches!(outcome, TaskOutcome::Completed { .. }) {
        return Err(risk_control_failure(
            &task,
            "snapshot.outcome",
            "Chromium snapshot task did not complete",
        ));
    }
    let latest = resources
        .open_resource_descriptor(&output.ref_id)
        .map_err(|error| risk_control_failure(&task, "resource.open", error.to_string()))?;
    let bytes = resources
        .collect_read_plan(&ReadPlan {
            plan_id: format!("bilibili.risk-control.read.{}", task.task_id),
            resource: latest,
            operation: "collect".into(),
            args: Value::Null,
        })
        .map_err(|error| risk_control_failure(&task, "resource.read", error.to_string()))?;
    if bytes.len() > risk_control.max_response_bytes {
        return Err(risk_control_failure(
            &task,
            "response.oversized",
            format!(
                "Chromium response is {} bytes; maximum is {}",
                bytes.len(),
                risk_control.max_response_bytes
            ),
        ));
    }
    let snapshot: BrowserSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| risk_control_failure(&task, "response.decode", error))?;
    ensure_bilibili_domain(&snapshot.final_url)
        .map_err(|error| risk_control_failure(&task, "redirect.denied", error))?;
    let items = parse_dynamic_snapshot(&snapshot.html)
        .map_err(|error| risk_control_failure(&task, "dom.parse", error))?;
    let status = DomainEvent {
        event_id: format!("{}:risk-control", task.task_id),
        kind: RISK_CONTROL_STATUS_EVENT.into(),
        payload: json!({
            "task_id": task.task_id,
            "uid": request.uid,
            "risk_control_code": 352,
            "backend": "chromium",
            "status": "degraded",
            "fallback": "succeeded"
        }),
    };
    state
        .lock()
        .expect("Bilibili runner mutex")
        .finish_poll(
            &task,
            request,
            BilibiliPollKind::Dynamic,
            items,
            Some(status),
        )
        .map_err(RuntimeFailure::new)
}

impl Runner for BilibiliRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }
    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| self.run_task(task))
    }
}

pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    manifest_with_management(None)
}

pub fn manifest_with_management(
    command: Option<&str>,
) -> mutsuki_runtime_contracts::PluginManifest {
    manifest_with_management_and_risk_control(command, false)
}

pub fn manifest_with_management_and_risk_control(
    command: Option<&str>,
    risk_control_enabled: bool,
) -> mutsuki_runtime_contracts::PluginManifest {
    manifest_for_backend(
        BilibiliBackendKind::WebCookie,
        command,
        risk_control_enabled,
    )
}

pub fn manifest_for_config(config: &BilibiliConfig) -> mutsuki_runtime_contracts::PluginManifest {
    manifest_for_backend(
        config.backend.kind(),
        config
            .management
            .enabled
            .then_some(config.management.command.as_str()),
        config.risk_control.is_some(),
    )
}

fn manifest_for_backend(
    backend_kind: BilibiliBackendKind,
    command: Option<&str>,
    risk_control_enabled: bool,
) -> mutsuki_runtime_contracts::PluginManifest {
    let mut builder = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestRunner {
            descriptor: runner_descriptor(command.is_some(), risk_control_enabled, backend_kind),
        }))
        .protocol_handler(protocol(POLL_LIVE), RUNNER_ID, "io")
        .protocol_handler(protocol(POLL_VIDEO), RUNNER_ID, "io");
    if backend_kind == BilibiliBackendKind::WebCookie {
        builder = builder
            .protocol_handler(protocol(POLL_DYNAMIC), RUNNER_ID, "io")
            .protocol_handler(protocol(LINK_RESOLVE), RUNNER_ID, "io");
    }
    if let Some(command) = command {
        builder = builder
            .protocol_handler(protocol(MANAGEMENT_COMMAND), RUNNER_ID, "io")
            .handler_binding(
                HandlerBindingBuilder::new(
                    bot_command_binding_id(command),
                    PLUGIN_ID,
                    BOT_COMMAND_HANDLE_PROTOCOL_ID,
                    MANAGEMENT_COMMAND,
                )
                .target_runner_hint(RUNNER_ID)
                .pool_id("io")
                .build(),
            );
    }
    let mut manifest = builder.build().manifest;
    for protocol_id in manifest.provides.runners[0]
        .accepted_protocol_ids
        .iter()
        .cloned()
    {
        manifest
            .provides
            .protocol_classes
            .insert(protocol_id, ProtocolClass::Effect);
    }
    if risk_control_enabled {
        manifest.requires.push(format!("task_protocol:{SNAPSHOT}"));
    }
    manifest
}

fn runner_descriptor(
    management: bool,
    risk_control_enabled: bool,
    backend_kind: BilibiliBackendKind,
) -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID);
    let protocols: &[&str] = match backend_kind {
        BilibiliBackendKind::WebCookie => &[POLL_LIVE, POLL_DYNAMIC, POLL_VIDEO, LINK_RESOLVE],
        BilibiliBackendKind::OpenPlatform => &[POLL_LIVE, POLL_VIDEO],
    };
    for protocol in protocols {
        builder = builder.accepted_protocol(*protocol);
    }
    if management {
        builder = builder.accepted_protocol(MANAGEMENT_COMMAND);
    }
    builder
        .purity(RunnerPurity::Effectful)
        .execution_class(if risk_control_enabled {
            ExecutionClass::Orchestration
        } else {
            ExecutionClass::Io
        })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            max_inflight_batches: 1,
            ..Default::default()
        })
        .metadata("domain", ScalarValue::String("bilibili".into()))
        .metadata(
            "backend",
            ScalarValue::String(
                match backend_kind {
                    BilibiliBackendKind::WebCookie => "web_cookie",
                    BilibiliBackendKind::OpenPlatform => "open_platform",
                }
                .into(),
            ),
        )
        .build()
}

fn protocol(id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(id)
        .input_schema(json!({"type":"object"}))
        .output_schema(json!({"type":"object"}))
        .error_schema(json!({"type":"object"}))
        .build()
}

struct ManifestRunner {
    descriptor: RunnerDescriptor,
}
impl Runner for ManifestRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }
    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_error(
            &batch,
            RuntimeError::new("runner.unavailable", PLUGIN_ID, "manifest_only"),
        ))
    }
}

fn decode<T: for<'de> Deserialize<'de>>(task: &Task) -> Result<T, RuntimeError> {
    serde_json::from_value(task.payload.clone().into())
        .map_err(|error| bili_error(task, BilibiliError::InvalidResponse(error.to_string())))
}

fn outbound_result(task: &Task, message: BotMessage, binding: String) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.tasks.push(outbound_task(task, message, &binding, 0));
    result
}

fn command_outbound_result(
    task: &Task,
    message: BotMessage,
    binding: Option<&str>,
) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    let mut child = Task::new(
        format!("{}:reply", task.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).expect("BotMessage serializes"),
    );
    child.trace_id = task.trace_id.clone();
    child.correlation_id = task
        .correlation_id
        .clone()
        .or_else(|| Some(task.task_id.clone()));
    child.registry_generation = task.registry_generation;
    child.target_binding_id = binding.filter(|value| !value.is_empty()).map(Into::into);
    result.tasks.push(child);
    result
}

fn require_admin(is_admin: bool) -> Result<(), BilibiliError> {
    is_admin.then_some(()).ok_or(BilibiliError::Forbidden)
}

pub(crate) fn required_arg(
    args: &[String],
    index: usize,
    name: &str,
) -> Result<String, BilibiliError> {
    args.get(index)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(format!("missing {name}")))
}

pub(crate) fn parse_uid(value: Option<&String>) -> Result<u64, BilibiliError> {
    value
        .ok_or_else(|| BilibiliError::InvalidResponse("missing Bilibili UID".into()))?
        .parse::<u64>()
        .ok()
        .filter(|uid| *uid > 0)
        .ok_or_else(|| BilibiliError::InvalidResponse("invalid Bilibili UID".into()))
}

pub(crate) fn parse_notifications(
    value: Option<&String>,
) -> Result<Vec<BilibiliPollKind>, BilibiliError> {
    let values = value
        .map(|value| value.split(',').collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["live", "dynamic", "video"]);
    let mut notifications = Vec::new();
    for value in values {
        let kind = match value.trim() {
            "live" => BilibiliPollKind::Live,
            "dynamic" => BilibiliPollKind::Dynamic,
            "video" => BilibiliPollKind::Video,
            unknown => {
                return Err(BilibiliError::InvalidResponse(format!(
                    "unknown notification type {unknown}"
                )));
            }
        };
        if !notifications.contains(&kind) {
            notifications.push(kind);
        }
    }
    if notifications.is_empty() {
        return Err(BilibiliError::InvalidResponse(
            "notification types must not be empty".into(),
        ));
    }
    Ok(notifications)
}

pub(crate) fn binding_code(actor_id: &str, uid: u64, task_id: &str) -> String {
    let digest = format!("{:x}", md5::compute(format!("{actor_id}:{uid}:{task_id}")));
    format!("mutsuki-{}", &digest[..8])
}

pub(crate) fn self_subscription_id_for(platform: &str, actor_id: &str) -> String {
    let digest = format!("{:x}", md5::compute(format!("{platform}:{actor_id}")));
    format!("self-{}", &digest[..12])
}

pub(crate) fn select_subscription(
    config: &BilibiliConfig,
    actor_id: &str,
    is_admin: bool,
    selector: Option<&str>,
) -> Result<usize, BilibiliError> {
    let matches = config
        .subscriptions
        .iter()
        .enumerate()
        .filter(|(_, subscription)| {
            is_admin || subscription.owner_user_id.as_deref() == Some(actor_id)
        })
        .filter(|(_, subscription)| {
            selector.is_none_or(|selector| {
                subscription.subscription_id == selector || subscription.uid.to_string() == selector
            })
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(BilibiliError::ManagementUnavailable(
            "matching subscription was not found".into(),
        )),
        _ => Err(BilibiliError::ManagementUnavailable(
            "subscription selector is ambiguous".into(),
        )),
    }
}

pub(crate) fn render_qr_png(value: &str) -> Result<Vec<u8>, BilibiliError> {
    let image = QrCode::new(value.as_bytes())
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?
        .render::<image::Luma<u8>>()
        .min_dimensions(256, 256)
        .build();
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    Ok(bytes.into_inner())
}

fn outbound_task(parent: &Task, message: BotMessage, binding: &str, index: usize) -> Task {
    let mut task = Task::new(
        format!("{}:notify:{index}", parent.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).expect("BotMessage serializes"),
    );
    task.target_binding_id = Some(binding.into());
    task.trace_id = parent.trace_id.clone();
    task.correlation_id = parent
        .correlation_id
        .clone()
        .or_else(|| Some(parent.task_id.clone()));
    task
}

fn bili_error(task: &Task, error: BilibiliError) -> RuntimeError {
    let code = match &error {
        BilibiliError::CookieExpired => "bilibili.cookie_expired",
        BilibiliError::RateLimited => "bilibili.rate_limited",
        BilibiliError::RiskControl352 => "bilibili.risk_control_352",
        BilibiliError::Forbidden => "bilibili.management_forbidden",
        BilibiliError::ManagementUnavailable(_) => "bilibili.management_unavailable",
        BilibiliError::OpenPlatformCredentialUnavailable(_)
        | BilibiliError::OpenPlatformCredentialInvalid(_) => "bilibili.open_platform.credentials",
        BilibiliError::OpenPlatformPermissionDenied { .. } => {
            "bilibili.open_platform.permission_denied"
        }
        BilibiliError::OpenPlatformOAuthExpired { .. } => "bilibili.open_platform.oauth_expired",
        BilibiliError::OpenPlatformSignatureRejected { .. } => {
            "bilibili.open_platform.signature_rejected"
        }
        BilibiliError::OpenPlatformApi { .. } => "bilibili.open_platform.api_failed",
        BilibiliError::OpenPlatformUnsupported(_) => {
            "bilibili.open_platform.unsupported_capability"
        }
        _ => "bilibili.request_failed",
    };
    let mut runtime = RuntimeError::new(code, PLUGIN_ID, format!("bilibili.{}", task.task_id));
    runtime
        .evidence
        .insert("detail".into(), ScalarValue::String(error.to_string()));
    match &error {
        BilibiliError::OpenPlatformPermissionDenied {
            code,
            scope,
            request_id,
        } => {
            runtime.evidence.insert(
                "open_platform_code".into(),
                ScalarValue::String(code.to_string()),
            );
            runtime
                .evidence
                .insert("required_scope".into(), ScalarValue::String(scope.clone()));
            if let Some(request_id) = request_id {
                runtime
                    .evidence
                    .insert("request_id".into(), ScalarValue::String(request_id.clone()));
            }
        }
        BilibiliError::OpenPlatformOAuthExpired {
            request_id: Some(request_id),
        } => {
            runtime
                .evidence
                .insert("request_id".into(), ScalarValue::String(request_id.clone()));
        }
        BilibiliError::OpenPlatformSignatureRejected { code, request_id }
        | BilibiliError::OpenPlatformApi {
            code, request_id, ..
        } => {
            runtime.evidence.insert(
                "open_platform_code".into(),
                ScalarValue::String(code.to_string()),
            );
            if let Some(request_id) = request_id {
                runtime
                    .evidence
                    .insert("request_id".into(), ScalarValue::String(request_id.clone()));
            }
        }
        _ => {}
    }
    if code == "bilibili.risk_control_352" {
        for (key, value) in [
            ("risk_control_code", "352"),
            ("fallback_status", "not_configured"),
            ("degraded", "true"),
        ] {
            runtime
                .evidence
                .insert(key.into(), ScalarValue::String(value.into()));
        }
    }
    runtime
}

fn risk_control_failure(task: &Task, route: &str, detail: impl fmt::Display) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        "bilibili.risk_control_fallback_failed",
        PLUGIN_ID,
        format!("bilibili.risk_control.{route}.{}", task.task_id),
    );
    for (key, value) in [
        ("risk_control_code", "352"),
        ("backend", "chromium"),
        ("fallback_status", "failed"),
        ("degraded", "true"),
    ] {
        error
            .evidence
            .insert(key.into(), ScalarValue::String(value.into()));
    }
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.to_string()));
    RuntimeFailure::new(error)
}

fn ensure_bilibili_domain(value: &str) -> Result<(), BilibiliError> {
    let url = Url::parse(value).map_err(|error| BilibiliError::DomainDenied(error.to_string()))?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = host == "b23.tv"
        || host == "bilibili.com"
        || host.ends_with(".bilibili.com")
        || host == "hdslb.com"
        || host.ends_with(".hdslb.com");
    if url.scheme() == "https" && allowed {
        Ok(())
    } else {
        Err(BilibiliError::DomainDenied(host))
    }
}

fn credential_from_redirect(value: &str) -> Result<String, BilibiliError> {
    let url =
        Url::parse(value).map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let allowed = [
        "SESSDATA",
        "bili_jct",
        "DedeUserID",
        "DedeUserID__ckMd5",
        "buvid3",
    ];
    let values = url
        .query_pairs()
        .filter(|(key, value)| allowed.contains(&key.as_ref()) && !value.trim().is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    if !values.iter().any(|value| value.starts_with("SESSDATA=")) {
        return Err(BilibiliError::InvalidResponse(
            "QR login response did not contain SESSDATA".into(),
        ));
    }
    Ok(values.join("; "))
}

fn string_field(value: &Value, field: &str) -> Result<String, BilibiliError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(field.into()))
}

fn parse_dynamic_snapshot(html: &str) -> Result<Vec<BilibiliItem>, BilibiliError> {
    let document = Html::parse_document(html);
    let cards = Selector::parse(".bili-dyn-list__item")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let links = Selector::parse("a[href]")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let images = Selector::parse("img[src]")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let mut items = Vec::new();
    for card in document.select(&cards) {
        let Some((id, url)) = card.select(&links).find_map(|link| {
            let href = link.value().attr("href")?;
            dynamic_id_and_url(href)
        }) else {
            continue;
        };
        let title = first_card_text(
            card,
            &[
                ".bili-rich-text",
                ".bili-dyn-card-video__title",
                ".bili-dyn-card-opus__summary",
            ],
        )
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "新动态".into())
        .chars()
        .take(80)
        .collect();
        let image_url = card
            .select(&images)
            .filter_map(|image| image.value().attr("src"))
            .filter_map(normalize_browser_url)
            .find(|url| ensure_bilibili_domain(url).is_ok());
        items.push(BilibiliItem {
            id,
            title,
            url,
            image_url,
        });
    }
    Ok(items)
}

fn first_card_text(card: ElementRef<'_>, selectors: &[&str]) -> Option<String> {
    selectors.iter().find_map(|selector| {
        let selector = Selector::parse(selector).ok()?;
        let text = card
            .select(&selector)
            .next()?
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty()).then_some(text)
    })
}

fn dynamic_id_and_url(value: &str) -> Option<(String, String)> {
    let url = normalize_browser_url(value)?;
    ensure_bilibili_domain(&url).ok()?;
    let parsed = Url::parse(&url).ok()?;
    let host = parsed.host_str()?;
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let id = match (host, segments.as_slice()) {
        ("t.bilibili.com", [id]) => *id,
        ("bilibili.com" | "www.bilibili.com", ["opus", id]) => *id,
        _ => return None,
    };
    id.chars()
        .all(|character| character.is_ascii_digit())
        .then(|| (id.to_owned(), format!("https://t.bilibili.com/{id}")))
}

fn normalize_browser_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("//") {
        Some(format!("https:{value}"))
    } else if value.starts_with('/') {
        Some(format!("https://www.bilibili.com{value}"))
    } else if value.starts_with("https://") {
        Some(value.to_owned())
    } else {
        None
    }
}

fn parse_poll_items(
    kind: &BilibiliPollKind,
    uid: u64,
    value: Value,
) -> Result<Vec<BilibiliItem>, BilibiliError> {
    match kind {
        BilibiliPollKind::Live => {
            let live = &value["data"]["live_room"];
            let status = live
                .get("liveStatus")
                .or_else(|| live.get("live_status"))
                .and_then(Value::as_i64)
                .unwrap_or(0)
                == 1;
            Ok(vec![BilibiliItem {
                id: status.to_string(),
                title: live
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("直播")
                    .into(),
                url: live
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or(&format!("https://space.bilibili.com/{uid}"))
                    .into(),
                image_url: live
                    .get("cover")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            }])
        }
        BilibiliPollKind::Dynamic => {
            let items = value["data"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Ok(items
                .into_iter()
                .filter_map(|item| {
                    let id = item.get("id_str")?.as_str()?.to_owned();
                    let modules = &item["modules"]["module_dynamic"];
                    Some(BilibiliItem {
                        id: id.clone(),
                        title: modules["desc"]["text"]
                            .as_str()
                            .unwrap_or("新动态")
                            .chars()
                            .take(80)
                            .collect(),
                        url: format!("https://t.bilibili.com/{id}"),
                        image_url: modules["major"]["archive"]["cover"]
                            .as_str()
                            .map(ToOwned::to_owned),
                    })
                })
                .collect())
        }
        BilibiliPollKind::Video => {
            let items = value["data"]["list"]["vlist"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Ok(items
                .into_iter()
                .filter_map(|item| {
                    let bvid = item.get("bvid")?.as_str()?.to_owned();
                    Some(BilibiliItem {
                        id: bvid.clone(),
                        title: item["title"].as_str().unwrap_or("新投稿").into(),
                        url: format!("https://www.bilibili.com/video/{bvid}"),
                        image_url: item["pic"].as_str().map(|url| format!("https:{url}")),
                    })
                })
                .collect())
        }
    }
}

fn fresh_since(items: Vec<BilibiliItem>, previous: &str) -> Vec<BilibiliItem> {
    let mut fresh = items
        .into_iter()
        .take_while(|item| item.id != previous)
        .collect::<Vec<_>>();
    fresh.reverse();
    fresh
}

pub fn sign_wbi_query(params: &[(String, String)], mixin_key: &str, unix_seconds: i64) -> String {
    let mut params = params.to_vec();
    params.push(("wts".into(), unix_seconds.to_string()));
    params.sort_by(|left, right| left.0.cmp(&right.0));
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            params
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .finish();
    let signature = format!("{:x}", md5::compute(format!("{encoded}{mixin_key}")));
    format!("{encoded}&w_rid={signature}")
}

fn wbi_mixin_key(img_url: &str, sub_url: &str) -> Result<String, BilibiliError> {
    const TABLE: [usize; 64] = [
        46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19,
        29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4,
        22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
    ];
    let filename = |value: &str| {
        Url::parse(value)
            .ok()?
            .path_segments()?
            .next_back()?
            .split('.')
            .next()
            .map(ToOwned::to_owned)
    };
    let source = format!(
        "{}{}",
        filename(img_url).ok_or_else(|| BilibiliError::InvalidResponse("wbi img key".into()))?,
        filename(sub_url).ok_or_else(|| BilibiliError::InvalidResponse("wbi sub key".into()))?
    );
    let chars = source.chars().collect::<Vec<_>>();
    if chars.len() < 64 {
        return Err(BilibiliError::InvalidResponse("wbi key length".into()));
    }
    Ok(TABLE.iter().take(32).map(|index| chars[*index]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{BotAccountRef, BotEvent, BotEventKind, BotPlatform, BotUser};
    use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, CommandPlan, DispatchLane, ExportPlan, OrderingRequirement,
        PlanReceipt, ReadPlan, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
        ResourceSealState, ResourceSemantic, SnapshotDescriptor, StreamPlan, TaskBatch, TaskHandle,
        WorkResourcePlan, WritePlan,
    };
    use mutsuki_runtime_sdk::{ResourcePlanGateway, RuntimeClient};

    #[derive(Default)]
    struct FakeTransportState {
        signature: String,
    }

    struct FakeTransport(Arc<Mutex<FakeTransportState>>);

    impl BilibiliTransport for FakeTransport {
        fn poll(
            &mut self,
            _kind: &BilibiliPollKind,
            uid: u64,
        ) -> Result<Vec<BilibiliItem>, BilibiliError> {
            Ok(vec![BilibiliItem {
                id: "dynamic-1".into(),
                title: "latest".into(),
                url: format!("https://t.bilibili.com/{uid}"),
                image_url: None,
            }])
        }

        fn resolve(&mut self, _url: &str) -> Result<ResolvedLinkCard, BilibiliError> {
            unreachable!()
        }

        fn download(&mut self, _url: &str, _max_bytes: usize) -> Result<Vec<u8>, BilibiliError> {
            unreachable!()
        }

        fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
            Ok(BilibiliQrCode {
                url: "https://passport.bilibili.com/qr".into(),
                key: "qr-key".into(),
            })
        }

        fn qr_poll(&mut self, _key: &str) -> Result<BilibiliQrPoll, BilibiliError> {
            Ok(BilibiliQrPoll {
                status: BilibiliQrStatus::Confirmed,
                credential: Some("SESSDATA=ROTATED".into()),
            })
        }

        fn profile(&mut self, uid: u64) -> Result<BilibiliProfile, BilibiliError> {
            Ok(BilibiliProfile {
                name: format!("user-{uid}"),
                signature: self.0.lock().unwrap().signature.clone(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingCredentialStore(Mutex<Vec<(String, String)>>);

    impl BilibiliCredentialStore for RecordingCredentialStore {
        fn rotate(&self, key: &str, credential: String) -> Result<(), String> {
            self.0.lock().unwrap().push((key.into(), credential));
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingConfigStore(Mutex<Vec<BilibiliConfig>>);

    impl BilibiliConfigStore for RecordingConfigStore {
        fn replace(&self, config: &BilibiliConfig) -> Result<(), String> {
            self.0.lock().unwrap().push(config.clone());
            Ok(())
        }
    }

    struct UnusedResources;

    impl ResourcePlanGateway for UnusedResources {
        fn collect_read_plan(&self, _: &ReadPlan) -> RuntimeResult<Vec<u8>> {
            unreachable!()
        }
        fn snapshot_read_plan(
            &self,
            _: &ReadPlan,
            _: &str,
            _: &str,
        ) -> RuntimeResult<SnapshotDescriptor> {
            unreachable!()
        }
        fn open_stream_plan(&self, _: &ReadPlan) -> RuntimeResult<StreamPlan> {
            unreachable!()
        }
        fn execute_export_plan(&self, _: &ExportPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn commit_write_plan(&self, _: &WritePlan, _: Vec<u8>) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_plan(&self, _: &CommandPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_batch(&self, _: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
        fn execute_saga_plan(&self, _: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
    }

    impl ResourceRegistryGateway for UnusedResources {
        fn open_resource_descriptor(&self, _: &str) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_blob_resource(&self, _: &str, _: &str, _: Vec<u8>) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_cow_state_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Vec<u8>,
        ) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_capability_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
    }

    struct SnapshotResources {
        bytes: Vec<u8>,
    }

    impl ResourcePlanGateway for SnapshotResources {
        fn collect_read_plan(&self, _: &ReadPlan) -> RuntimeResult<Vec<u8>> {
            Ok(self.bytes.clone())
        }
        fn snapshot_read_plan(
            &self,
            _: &ReadPlan,
            _: &str,
            _: &str,
        ) -> RuntimeResult<SnapshotDescriptor> {
            unreachable!()
        }
        fn open_stream_plan(&self, _: &ReadPlan) -> RuntimeResult<StreamPlan> {
            unreachable!()
        }
        fn execute_export_plan(&self, _: &ExportPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn commit_write_plan(&self, _: &WritePlan, _: Vec<u8>) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_plan(&self, _: &CommandPlan) -> RuntimeResult<PlanReceipt> {
            unreachable!()
        }
        fn execute_command_batch(&self, _: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
        fn execute_saga_plan(&self, _: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
            unreachable!()
        }
    }

    impl ResourceRegistryGateway for SnapshotResources {
        fn open_resource_descriptor(&self, _: &str) -> RuntimeResult<ResourceRef> {
            Ok(snapshot_resource())
        }
        fn create_blob_resource(&self, _: &str, _: &str, _: Vec<u8>) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
        fn create_cow_state_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Vec<u8>,
        ) -> RuntimeResult<ResourceRef> {
            Ok(snapshot_resource())
        }
        fn create_capability_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> RuntimeResult<ResourceRef> {
            unreachable!()
        }
    }

    fn snapshot_resource() -> ResourceRef {
        ResourceRef {
            ref_id: "bilibili-risk-snapshot".into(),
            resource_id: ResourceId {
                kind_id: "browser.snapshot".into(),
                slot_id: "bilibili-risk-snapshot".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::CowVersionedState,
            provider_id: "memory".into(),
            resource_kind: "browser.snapshot".into(),
            schema: SNAPSHOT_SCHEMA.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: "memory".into(),
                method: "memory".into(),
            },
            size_hint: Some(0),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    struct CompletedChildClient;

    impl RuntimeClient for CompletedChildClient {
        fn submit_batch(&self, _: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            unreachable!()
        }

        fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(Some(TaskOutcome::Completed {
                task_id: handle.task_id.clone(),
                output: None,
                output_ref: None,
            }))
        }
    }

    struct RiskControlledTransport;

    impl BilibiliTransport for RiskControlledTransport {
        fn poll(
            &mut self,
            _: &BilibiliPollKind,
            _: u64,
        ) -> Result<Vec<BilibiliItem>, BilibiliError> {
            Err(BilibiliError::RiskControl352)
        }
        fn resolve(&mut self, _: &str) -> Result<ResolvedLinkCard, BilibiliError> {
            unreachable!()
        }
        fn download(&mut self, _: &str, _: usize) -> Result<Vec<u8>, BilibiliError> {
            unreachable!()
        }
        fn qr_start(&mut self) -> Result<BilibiliQrCode, BilibiliError> {
            unreachable!()
        }
        fn qr_poll(&mut self, _: &str) -> Result<BilibiliQrPoll, BilibiliError> {
            unreachable!()
        }
        fn profile(&mut self, _: u64) -> Result<BilibiliProfile, BilibiliError> {
            unreachable!()
        }
    }

    #[test]
    fn wbi_signature_is_deterministic_with_fixed_clock() {
        let signed = sign_wbi_query(&[("mid".into(), "1".into())], "secret", 1_700_000_000);
        assert_eq!(
            signed,
            sign_wbi_query(&[("mid".into(), "1".into())], "secret", 1_700_000_000)
        );
        assert!(signed.contains("wts=1700000000&w_rid="));
    }

    #[test]
    fn credential_debug_and_errors_never_expose_cookie() {
        let credential = SharedBilibiliCredential::default();
        credential.set("SESSDATA=top-secret".into());
        assert!(!format!("{credential:?}").contains("top-secret"));
        assert!(
            !BilibiliError::CookieExpired
                .to_string()
                .contains("SESSDATA")
        );
    }

    #[test]
    fn qr_redirect_extracts_only_cookie_fields_and_requires_sessdata() {
        let credential = credential_from_redirect(
            "https://www.bilibili.com/?SESSDATA=abc%3D%3D&bili_jct=csrf&ignored=value",
        )
        .unwrap();
        assert_eq!(credential, "SESSDATA=abc==; bili_jct=csrf");
        assert!(credential_from_redirect("https://www.bilibili.com/?bili_jct=x").is_err());
    }

    #[test]
    fn management_manifest_exposes_only_the_configured_command_binding() {
        let base = manifest();
        assert!(
            !base.provides.runners[0]
                .accepted_protocol_ids
                .contains(&BOT_COMMAND_HANDLE_PROTOCOL_ID.to_string())
        );
        assert!(
            base.provides
                .handler_bindings
                .iter()
                .all(|binding| { binding.protocol_id != BOT_COMMAND_HANDLE_PROTOCOL_ID })
        );

        let managed = manifest_with_management(Some("bili"));
        assert!(
            managed.provides.runners[0]
                .accepted_protocol_ids
                .contains(&MANAGEMENT_COMMAND.to_string())
        );
        assert!(managed.provides.handler_bindings.iter().any(|binding| {
            binding.binding_id == bot_command_binding_id("bili")
                && binding.protocol_id == BOT_COMMAND_HANDLE_PROTOCOL_ID
                && binding.target_protocol_id == MANAGEMENT_COMMAND
                && binding.target_runner_hint.as_deref() == Some(RUNNER_ID)
        }));
        assert!(
            managed.provides.runners[0]
                .accepted_protocol_ids
                .iter()
                .all(
                    |protocol_id| managed.provides.protocol_classes.get(protocol_id)
                        == Some(&ProtocolClass::Effect)
                )
        );

        let risk_control = manifest_with_management_and_risk_control(None, true);
        assert!(
            risk_control
                .requires
                .contains(&format!("task_protocol:{SNAPSHOT}"))
        );
        assert_eq!(
            risk_control.provides.runners[0].execution_class,
            ExecutionClass::Orchestration
        );
    }

    #[test]
    fn dynamic_snapshot_parser_keeps_bilibili_urls_and_normalizes_cards() {
        let items = parse_dynamic_snapshot(
            r#"<article class="bili-dyn-list__item">
                <a href="https://www.bilibili.com/opus/123456">detail</a>
                <div class="bili-rich-text">  hello   browser fallback </div>
                <img src="//i0.hdslb.com/bfs/archive/cover.jpg">
            </article>
            <article class="bili-dyn-list__item">
                <a href="https://evil.example/opus/999">denied</a>
            </article>"#,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "123456");
        assert_eq!(items[0].title, "hello browser fallback");
        assert_eq!(items[0].url, "https://t.bilibili.com/123456");
        assert_eq!(
            items[0].image_url.as_deref(),
            Some("https://i0.hdslb.com/bfs/archive/cover.jpg")
        );
    }

    #[test]
    fn explicit_chromium_backend_awaits_snapshot_and_reports_degraded_success() {
        let html = r#"<article class="bili-dyn-list__item">
            <a href="https://t.bilibili.com/42">detail</a>
            <div class="bili-rich-text">fallback item</div>
        </article>"#;
        let bytes = serde_json::to_vec(&BrowserSnapshot {
            final_url: "https://space.bilibili.com/7/dynamic".into(),
            title: "space".into(),
            html: html.into(),
        })
        .unwrap();
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let runner = BilibiliRunner::new(
            Box::new(RiskControlledTransport),
            repository.clone(),
            Arc::new(SnapshotResources { bytes }),
            "memory",
        );
        let mut runner = runner.into_runtime_runner(
            Arc::new(CompletedChildClient),
            Some(BilibiliRiskControlConfig {
                backend: BilibiliRiskControlBackend::Chromium,
                timeout_ms: 5_000,
                max_response_bytes: 64 * 1024,
            }),
        );
        let task = Task::new(
            "risk-control",
            POLL_DYNAMIC,
            serde_json::to_value(PollRequest {
                subscription_id: "sub".into(),
                uid: 7,
                target: BotTarget::Group {
                    group_id: "group".into(),
                },
                outbound_binding: "qq-main".into(),
            })
            .unwrap(),
        );
        let batch = command_batch(vec![task]);
        let context = RunnerContext::new(1, 1, "executor", Vec::<String>::new(), "invocation")
            .with_batch("batch", 1);
        let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
        let waiting = waiting.results[0].result.as_ref().unwrap();
        assert_eq!(waiting.tasks[0].protocol_id, SNAPSHOT);
        assert!(waiting.task_await.is_some());

        let completed = runner.run_batch(context, batch).unwrap();
        let completed = completed.results[0].result.as_ref().unwrap();
        assert!(
            completed
                .events
                .iter()
                .any(|event| event.kind == RISK_CONTROL_STATUS_EVENT
                    && event.payload["fallback"] == "succeeded")
        );
        assert_eq!(
            repository.cursor("Dynamic:7:sub").unwrap().as_deref(),
            Some("42")
        );
    }

    #[test]
    fn management_flow_rotates_secret_persists_verified_binding_and_previews_without_cursor() {
        let state = Arc::new(Mutex::new(FakeTransportState::default()));
        let config = SharedBilibiliConfig::new(managed_config());
        let credential = SharedBilibiliCredential::default();
        let credential_store = Arc::new(RecordingCredentialStore::default());
        let config_store = Arc::new(RecordingConfigStore::default());
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let management = Arc::new(BilibiliManagementService::new(
            config.clone(),
            credential,
            Box::new(FakeTransport(state.clone())),
            repository.clone(),
            credential_store.clone(),
            config_store.clone(),
            Arc::new(AlwaysPresentSecrets),
        ));
        let mut runner = BilibiliRunner::new(
            Box::new(FakeTransport(state.clone())),
            repository.clone(),
            Arc::new(UnusedResources),
            "memory",
        )
        .with_management(management);

        repository.set_qr_session("admin", "qr-key").unwrap();
        let login = runner
            .run_batch(
                RunnerContext::new(1, 1, "executor", Vec::<String>::new(), "batch")
                    .with_batch("batch", 2),
                command_batch(vec![
                    command_task("login", "admin", &["login-status"]),
                    command_task("forbidden-login", "alice", &["login-status"]),
                ]),
            )
            .unwrap();
        assert_eq!(login.results.len(), 2);
        assert!(login.results[1].error.is_some());
        let login = login.results[0].result.as_ref().unwrap();
        assert_eq!(credential_store.0.lock().unwrap().len(), 1);
        assert!(
            !serde_json::to_string(&login.tasks)
                .unwrap()
                .contains("ROTATED")
        );

        runner
            .run_command(&command_task("bind", "alice", &["bind", "42"]))
            .unwrap();
        let (_, code) = repository.binding_challenge("alice").unwrap().unwrap();
        state.lock().unwrap().signature = format!("hello {code}");
        runner
            .run_command(&command_task("verify", "alice", &["verify"]))
            .unwrap();
        let snapshot = config.snapshot();
        assert_eq!(snapshot.subscriptions.len(), 1);
        assert_eq!(snapshot.subscriptions[0].uid, 42);
        assert_eq!(
            snapshot.subscriptions[0].owner_user_id.as_deref(),
            Some("alice")
        );

        runner
            .run_command(&command_task("pause", "alice", &["pause"]))
            .unwrap();
        assert!(config.snapshot().subscriptions[0].paused);
        assert!(config_store.0.lock().unwrap().len() >= 2);

        assert!(repository.cursor("Dynamic:42").unwrap().is_none());
        let preview = runner
            .run_command(&command_task("preview", "alice", &["preview"]))
            .unwrap();
        assert_eq!(preview.tasks.len(), 1);
        assert!(repository.cursor("Dynamic:42").unwrap().is_none());
    }

    #[test]
    fn management_service_web_subscribe_and_clear_never_echo_cookie() {
        let config = SharedBilibiliConfig::new(managed_config());
        let credential = SharedBilibiliCredential::default();
        credential.set("SESSDATA=secret-cookie".into());
        let credential_store = Arc::new(RecordingCredentialStore::default());
        let config_store = Arc::new(RecordingConfigStore::default());
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let management = BilibiliManagementService::new(
            config.clone(),
            credential.clone(),
            Box::new(FakeTransport(Arc::new(Mutex::new(
                FakeTransportState::default(),
            )))),
            repository,
            credential_store.clone(),
            config_store.clone(),
            Arc::new(AlwaysPresentSecrets),
        );
        let status = management.status();
        assert!(status.available);
        assert!(status.credential_loaded);
        assert!(
            !serde_json::to_string(&status)
                .unwrap()
                .contains("secret-cookie")
        );

        let view = management
            .subscribe(
                "sub-1".into(),
                7,
                vec![BilibiliPollKind::Live],
                BotTarget::Group {
                    group_id: "g1".into(),
                },
                "qq-main".into(),
            )
            .unwrap();
        assert_eq!(view.subscription_id, "sub-1");
        assert_eq!(config.snapshot().subscriptions.len(), 1);
        assert_eq!(config_store.0.lock().unwrap().len(), 1);

        management.credential_clear().unwrap();
        assert!(!credential.is_loaded());
        assert_eq!(credential_store.0.lock().unwrap().last().unwrap().1, "");
    }

    struct AlwaysPresentSecrets;

    impl BilibiliSecretPresence for AlwaysPresentSecrets {
        fn inspect(&self, _key: &str) -> CredentialSecretState {
            CredentialSecretState::Present
        }
    }

    fn managed_config() -> BilibiliConfig {
        BilibiliConfig {
            backend: BilibiliBackendConfig::WebCookie {
                cookie_secret_key: "BILIBILI_COOKIE".into(),
            },
            live_interval_ms: 1_000,
            dynamic_interval_ms: 1_000,
            video_interval_ms: 1_000,
            retry: RetryConfig {
                max_attempts: 3,
                initial_backoff_ms: 10,
                max_backoff_ms: 100,
            },
            subscriptions: Vec::new(),
            link_resolver: LinkResolverConfig {
                enabled: false,
                cooldown_ms: 1_000,
                account_to_binding: BTreeMap::new(),
            },
            media_provider_id: "memory".into(),
            risk_control: None,
            management: BilibiliManagementConfig {
                enabled: true,
                allow_self_binding: true,
                command: "bili".into(),
                admin_user_ids: vec!["admin".into()],
                self_binding_notifications: vec![BilibiliPollKind::Dynamic],
                self_binding_outbound_binding: "qq-main".into(),
            },
        }
    }

    fn command_task(task_id: &str, actor_id: &str, args: &[&str]) -> Task {
        let target = BotTarget::Group {
            group_id: "group".into(),
        };
        Task::new(
            task_id,
            MANAGEMENT_COMMAND,
            serde_json::to_value(BotCommandEvent {
                source: BotEvent {
                    event_id: format!("event-{task_id}"),
                    platform: BotPlatform::QqBot,
                    bot: BotAccountRef {
                        account_id: "bot".into(),
                        platform: BotPlatform::QqBot,
                    },
                    kind: BotEventKind::MessageCreated,
                    time_ms: 1,
                    target: target.clone(),
                    actor: Some(BotUser {
                        user_id: actor_id.into(),
                        display_name: None,
                        avatar_url: None,
                    }),
                    message: Some(BotMessage::text(target, "/bili")),
                    raw: None,
                    ext: BotExtMap::new(),
                },
                name: "bili".into(),
                args: args.iter().map(|value| (*value).into()).collect(),
                raw_text: format!("/bili {}", args.join(" ")),
            })
            .unwrap(),
        )
    }

    fn command_batch(tasks: Vec<Task>) -> WorkBatch {
        WorkBatch {
            batch_id: "batch".into(),
            tick_id: "tick".into(),
            batch_key: RUNNER_ID.into(),
            entries: tasks
                .iter()
                .enumerate()
                .map(|(index, task)| BatchEntry {
                    entry_id: format!("entry-{index}"),
                    task_id: task.task_id.clone(),
                    trace_id: None,
                    parent_id: None,
                    payload_index: index,
                    resource_requirement_indices: Vec::new(),
                    cancel_index: None,
                    deadline_tick: None,
                    priority: 0,
                    lane: DispatchLane::Normal,
                    ordering: OrderingRequirement::PreserveSubmitOrder,
                })
                .collect(),
            payload: BatchPayload::from_tasks(&tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    #[test]
    fn first_cursor_is_persisted_without_history() {
        let repo = SqliteBilibiliRepository::open(":memory:").unwrap();
        assert!(repo.cursor("dynamic:1").unwrap().is_none());
        repo.set_cursor("dynamic:1", "newest").unwrap();
        assert_eq!(repo.cursor("dynamic:1").unwrap().as_deref(), Some("newest"));
    }

    #[test]
    fn dynamic_and_video_items_are_emitted_oldest_first_after_cursor() {
        let item = |id: &str| BilibiliItem {
            id: id.into(),
            title: id.into(),
            url: format!("https://www.bilibili.com/{id}"),
            image_url: None,
        };
        let fresh = fresh_since(vec![item("3"), item("2"), item("1")], "1");
        assert_eq!(
            fresh
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2", "3"]
        );
    }

    #[test]
    fn cooldown_is_persisted_in_sqlite() {
        let repo = SqliteBilibiliRepository::open(":memory:").unwrap();
        assert!(repo.admit_cooldown("account:url", 100, 50).unwrap());
        assert!(!repo.admit_cooldown("account:url", 120, 50).unwrap());
        assert!(repo.admit_cooldown("account:url", 151, 50).unwrap());
    }

    #[test]
    fn rate_limit_cookie_and_352_have_distinct_runtime_codes() {
        let task = Task::new("error", POLL_LIVE, Value::Null);
        assert_eq!(
            bili_error(&task, BilibiliError::RateLimited).code,
            "bilibili.rate_limited"
        );
        assert_eq!(
            bili_error(&task, BilibiliError::CookieExpired).code,
            "bilibili.cookie_expired"
        );
        let risk_control = bili_error(&task, BilibiliError::RiskControl352);
        assert_eq!(risk_control.code, "bilibili.risk_control_352");
        assert_eq!(
            risk_control.evidence.get("fallback_status"),
            Some(&ScalarValue::String("not_configured".into()))
        );
    }

    #[test]
    fn risk_control_config_rejects_unbounded_limits() {
        let mut config = managed_config();
        config.risk_control = Some(BilibiliRiskControlConfig {
            backend: BilibiliRiskControlBackend::Chromium,
            timeout_ms: 0,
            max_response_bytes: 1024,
        });
        assert!(config.validate().unwrap_err().contains("timeout_ms"));
        config.risk_control.as_mut().unwrap().timeout_ms = 1000;
        config.risk_control.as_mut().unwrap().max_response_bytes = 0;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("max_response_bytes")
        );
    }

    #[test]
    fn open_platform_config_rejects_web_only_capabilities_and_wrong_uid() {
        let mut config = managed_config();
        config.backend = BilibiliBackendConfig::OpenPlatform {
            client_id: "client".into(),
            app_secret_key: "BILIBILI_OPEN_APP_SECRET".into(),
            oauth_credential_key: "BILIBILI_OPEN_OAUTH".into(),
            authorized_uid: 42,
        };
        assert!(config.validate().unwrap_err().contains("Cookie management"));
        config.management = BilibiliManagementConfig::default();
        config.subscriptions.push(BilibiliSubscription {
            subscription_id: "dynamic".into(),
            uid: 42,
            notifications: vec![BilibiliPollKind::Dynamic],
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            outbound_binding: "qq-main".into(),
            paused: false,
            owner_user_id: None,
        });
        assert!(config.validate().unwrap_err().contains("poll/dynamic"));
        config.subscriptions[0].notifications = vec![BilibiliPollKind::Video];
        config.subscriptions[0].uid = 7;
        assert!(config.validate().unwrap_err().contains("authorized_uid"));
        config.subscriptions[0].uid = 42;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn open_platform_manifest_advertises_only_live_and_video() {
        let mut config = managed_config();
        config.backend = BilibiliBackendConfig::OpenPlatform {
            client_id: "client".into(),
            app_secret_key: "BILIBILI_OPEN_APP_SECRET".into(),
            oauth_credential_key: "BILIBILI_OPEN_OAUTH".into(),
            authorized_uid: 42,
        };
        config.management = BilibiliManagementConfig::default();
        let manifest = manifest_for_config(&config);
        let protocols = manifest
            .provides
            .protocols
            .iter()
            .map(|protocol| protocol.protocol_id.as_str())
            .collect::<Vec<_>>();
        assert!(protocols.contains(&POLL_LIVE));
        assert!(protocols.contains(&POLL_VIDEO));
        assert!(!protocols.contains(&POLL_DYNAMIC));
        assert!(!protocols.contains(&LINK_RESOLVE));
        assert_eq!(
            manifest.provides.runners[0].accepted_protocol_ids,
            vec![POLL_LIVE, POLL_VIDEO]
        );
    }

    #[test]
    fn poll_protocol_is_the_kind_discriminator() {
        assert_eq!(
            BilibiliPollKind::from_protocol_id(POLL_DYNAMIC),
            Some(BilibiliPollKind::Dynamic)
        );
        assert_eq!(BilibiliPollKind::from_protocol_id(LINK_RESOLVE), None);
    }
}
