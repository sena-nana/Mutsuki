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
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, RunnerBatchCapability, RunnerContext, RunnerDescriptor,
    RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError, ScalarValue, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    HandlerBindingBuilder, PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway,
    RunnerDescriptorBuilder, map_work_batch_entries,
};
use qrcode::QrCode;
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.bot.bilibili";
pub const RUNNER_ID: &str = "mutsuki.bot.bilibili.runner";
pub const POLL_LIVE: &str = "mutsuki.bot.bilibili.poll/live@1";
pub const POLL_DYNAMIC: &str = "mutsuki.bot.bilibili.poll/dynamic@1";
pub const POLL_VIDEO: &str = "mutsuki.bot.bilibili.poll/video@1";
pub const LINK_RESOLVE: &str = "mutsuki.bot.bilibili.link/resolve@1";
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilibiliConfig {
    pub cookie_secret_key: String,
    pub live_interval_ms: u64,
    pub dynamic_interval_ms: u64,
    pub video_interval_ms: u64,
    pub retry: RetryConfig,
    pub subscriptions: Vec<BilibiliSubscription>,
    pub link_resolver: LinkResolverConfig,
    pub media_provider_id: String,
    #[serde(default)]
    pub management: BilibiliManagementConfig,
}

impl BilibiliConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.cookie_secret_key.trim().is_empty() {
            return Err("cookie_secret_key is required".into());
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

    fn replace(&self, config: BilibiliConfig) {
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

    fn get(&self) -> Result<String, BilibiliError> {
        self.0
            .lock()
            .expect("credential mutex")
            .clone()
            .filter(|cookie| !cookie.trim().is_empty())
            .ok_or(BilibiliError::CookieExpired)
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

    fn set_qr_session(&self, actor_id: &str, key: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO qr_session(actor_id,qr_key) VALUES(?1,?2) ON CONFLICT(actor_id) DO UPDATE SET qr_key=excluded.qr_key",
            params![actor_id, key],
        )?;
        Ok(())
    }

    fn qr_session(&self, actor_id: &str) -> Result<Option<String>, rusqlite::Error> {
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

    fn clear_qr_session(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .execute("DELETE FROM qr_session WHERE actor_id = ?1", [actor_id])?;
        Ok(())
    }

    fn set_binding_challenge(
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

    fn binding_challenge(&self, actor_id: &str) -> Result<Option<(u64, String)>, rusqlite::Error> {
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

    fn clear_binding_challenge(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "DELETE FROM binding_challenge WHERE actor_id = ?1",
            [actor_id],
        )?;
        Ok(())
    }
}

pub struct BilibiliRunner {
    descriptor: RunnerDescriptor,
    transport: Box<dyn BilibiliTransport>,
    repository: Arc<SqliteBilibiliRepository>,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
    managed_config: Option<SharedBilibiliConfig>,
    credential_store: Option<Arc<dyn BilibiliCredentialStore>>,
    config_store: Option<Arc<dyn BilibiliConfigStore>>,
}

impl BilibiliRunner {
    pub fn new(
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
    ) -> Self {
        Self {
            descriptor: runner_descriptor(false),
            transport,
            repository,
            resources,
            media_provider_id: media_provider_id.into(),
            managed_config: None,
            credential_store: None,
            config_store: None,
        }
    }

    pub fn with_management(
        mut self,
        config: SharedBilibiliConfig,
        credential_store: Arc<dyn BilibiliCredentialStore>,
        config_store: Arc<dyn BilibiliConfigStore>,
    ) -> Self {
        self.descriptor = runner_descriptor(true);
        self.managed_config = Some(config);
        self.credential_store = Some(credential_store);
        self.config_store = Some(config_store);
        self
    }

    fn run_task(&mut self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        if task.protocol_id == BOT_COMMAND_HANDLE_PROTOCOL_ID {
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
        let key = format!("{kind:?}:{}:{}", request.uid, request.subscription_id);
        let previous = self
            .repository
            .cursor(&key)
            .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
        let Some(head) = items.first().map(|item| item.id.clone()) else {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        };
        self.repository
            .set_cursor(&key, &head)
            .map_err(|error| bili_error(task, BilibiliError::Transport(error.to_string())))?;
        let Some(previous) = previous else {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        };
        let fresh = fresh_since(items, &previous);
        let mut result = RunnerResult::completed(task.task_id.clone());
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
        let Some(shared) = self.managed_config.clone() else {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        };
        let config = shared.snapshot();
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
                let qr = self
                    .transport
                    .qr_start()
                    .map_err(|error| bili_error(task, error))?;
                self.repository
                    .set_qr_session(actor_id, &qr.key)
                    .map_err(|error| {
                        bili_error(task, BilibiliError::Transport(error.to_string()))
                    })?;
                let image = self.qr_resource(&qr.url, task)?;
                Ok(self.command_reply(
                    task,
                    &command,
                    "请使用 Bilibili App 扫码确认，然后发送 /bili login-status；二维码不会把 Cookie 写入聊天或 Task payload。",
                    Some(image),
                ))
            }
            "login-status" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let key = self
                    .repository
                    .qr_session(actor_id)
                    .map_err(|error| {
                        bili_error(task, BilibiliError::Transport(error.to_string()))
                    })?
                    .ok_or_else(|| {
                        bili_error(
                            task,
                            BilibiliError::ManagementUnavailable(
                                "no active QR login; run login first".into(),
                            ),
                        )
                    })?;
                let polled = self
                    .transport
                    .qr_poll(&key)
                    .map_err(|error| bili_error(task, error))?;
                let text = match polled.status {
                    BilibiliQrStatus::Pending => "等待扫码。",
                    BilibiliQrStatus::Scanned => "已扫码，等待在 App 中确认。",
                    BilibiliQrStatus::Expired => {
                        self.repository.clear_qr_session(actor_id).map_err(|error| {
                            bili_error(task, BilibiliError::Transport(error.to_string()))
                        })?;
                        "二维码已过期，请重新执行 login。"
                    }
                    BilibiliQrStatus::Confirmed => {
                        let credential = polled.credential.ok_or_else(|| {
                            bili_error(
                                task,
                                BilibiliError::InvalidResponse(
                                    "confirmed QR login omitted credential".into(),
                                ),
                            )
                        })?;
                        let store = self.credential_store.as_ref().ok_or_else(|| {
                            bili_error(
                                task,
                                BilibiliError::ManagementUnavailable(
                                    "Host secret rotation boundary is unavailable".into(),
                                ),
                            )
                        })?;
                        store
                            .rotate(&config.cookie_secret_key, credential.clone())
                            .map_err(|detail| {
                                bili_error(task, BilibiliError::ManagementUnavailable(detail))
                            })?;
                        self.repository.clear_qr_session(actor_id).map_err(|error| {
                            bili_error(task, BilibiliError::Transport(error.to_string()))
                        })?;
                        "登录成功，凭据已通过 Host secret backend 原子轮换。"
                    }
                };
                Ok(self.command_reply(task, &command, text, None))
            }
            "bind" => {
                if !config.management.allow_self_binding {
                    return Err(bili_error(task, BilibiliError::Forbidden));
                }
                let uid = parse_uid(command.args.get(1)).map_err(|error| bili_error(task, error))?;
                let profile = self
                    .transport
                    .profile(uid)
                    .map_err(|error| bili_error(task, error))?;
                let code = binding_code(actor_id, uid, &task.task_id);
                self.repository
                    .set_binding_challenge(actor_id, uid, &code)
                    .map_err(|error| {
                        bili_error(task, BilibiliError::Transport(error.to_string()))
                    })?;
                Ok(self.command_reply(
                    task,
                    &command,
                    &format!(
                        "已为 {} ({uid}) 创建验证。请临时把 {code} 加入 Bilibili 个性签名，然后发送 /{} verify。",
                        profile.name, config.management.command
                    ),
                    None,
                ))
            }
            "verify" => {
                if !config.management.allow_self_binding {
                    return Err(bili_error(task, BilibiliError::Forbidden));
                }
                let (uid, code) = self
                    .repository
                    .binding_challenge(actor_id)
                    .map_err(|error| {
                        bili_error(task, BilibiliError::Transport(error.to_string()))
                    })?
                    .ok_or_else(|| {
                        bili_error(
                            task,
                            BilibiliError::ManagementUnavailable(
                                "no binding challenge; run bind first".into(),
                            ),
                        )
                    })?;
                let profile = self
                    .transport
                    .profile(uid)
                    .map_err(|error| bili_error(task, error))?;
                if !profile.signature.contains(&code) {
                    return Ok(self.command_reply(
                        task,
                        &command,
                        &format!("验证未通过：个性签名中尚未找到 {code}。"),
                        None,
                    ));
                }
                let mut next = config.clone();
                let subscription_id = self_subscription_id(&command, actor_id);
                next.subscriptions.retain(|subscription| {
                    subscription.owner_user_id.as_deref() != Some(actor_id)
                });
                next.subscriptions.push(BilibiliSubscription {
                    subscription_id,
                    uid,
                    notifications: next.management.self_binding_notifications.clone(),
                    target: command.source.target.clone(),
                    outbound_binding: next.management.self_binding_outbound_binding.clone(),
                    paused: false,
                    owner_user_id: Some(actor_id.into()),
                });
                self.persist_config(task, &shared, next)?;
                self.repository
                    .clear_binding_challenge(actor_id)
                    .map_err(|error| {
                        bili_error(task, BilibiliError::Transport(error.to_string()))
                    })?;
                Ok(self.command_reply(
                    task,
                    &command,
                    &format!("验证成功，已绑定 {} ({uid}) 并写入产品配置。", profile.name),
                    None,
                ))
            }
            "unbind" => {
                let mut next = config.clone();
                let before = next.subscriptions.len();
                next.subscriptions.retain(|subscription| {
                    subscription.owner_user_id.as_deref() != Some(actor_id)
                });
                if next.subscriptions.len() == before {
                    return Ok(self.command_reply(task, &command, "当前没有自助绑定。", None));
                }
                self.persist_config(task, &shared, next)?;
                Ok(self.command_reply(task, &command, "已解除绑定并更新产品配置。", None))
            }
            "pause" | "resume" => {
                let paused = action == "pause";
                let mut next = config.clone();
                let index = select_subscription(
                    &next,
                    actor_id,
                    is_admin,
                    command.args.get(1).map(String::as_str),
                )
                .map_err(|error| bili_error(task, error))?;
                let id = next.subscriptions[index].subscription_id.clone();
                next.subscriptions[index].paused = paused;
                self.persist_config(task, &shared, next)?;
                Ok(self.command_reply(
                    task,
                    &command,
                    &format!("订阅 {id} 已{}。", if paused { "暂停" } else { "恢复" }),
                    None,
                ))
            }
            "preview" => {
                let index = select_subscription(
                    &config,
                    actor_id,
                    is_admin,
                    command.args.get(1).map(String::as_str),
                )
                .map_err(|error| bili_error(task, error))?;
                let subscription = &config.subscriptions[index];
                let item = self
                    .transport
                    .poll(&BilibiliPollKind::Dynamic, subscription.uid)
                    .map_err(|error| bili_error(task, error))?
                    .into_iter()
                    .next();
                let Some(item) = item else {
                    return Ok(self.command_reply(task, &command, "该账号暂无可预览动态。", None));
                };
                let card = ResolvedLinkCard {
                    url: item.url,
                    title: item.title,
                    description: "通知预览（不会推进轮询 cursor）".into(),
                    image_url: item.image_url,
                };
                let message = self.card_message(command.source.target.clone(), card, task)?;
                Ok(command_outbound_result(
                    task,
                    message,
                    Some(&config.management.self_binding_outbound_binding),
                ))
            }
            "list" => {
                let lines = config
                    .subscriptions
                    .iter()
                    .filter(|subscription| {
                        is_admin || subscription.owner_user_id.as_deref() == Some(actor_id)
                    })
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
                let mut next = config.clone();
                next.subscriptions
                    .retain(|subscription| subscription.subscription_id != subscription_id);
                next.subscriptions.push(BilibiliSubscription {
                    subscription_id: subscription_id.clone(),
                    uid,
                    notifications,
                    target: command.source.target.clone(),
                    outbound_binding: next.management.self_binding_outbound_binding.clone(),
                    paused: false,
                    owner_user_id: None,
                });
                self.persist_config(task, &shared, next)?;
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
                let mut next = config.clone();
                let before = next.subscriptions.len();
                next.subscriptions
                    .retain(|subscription| subscription.subscription_id != subscription_id);
                if next.subscriptions.len() == before {
                    return Err(bili_error(
                        task,
                        BilibiliError::ManagementUnavailable(format!(
                            "subscription {subscription_id} was not found"
                        )),
                    ));
                }
                self.persist_config(task, &shared, next)?;
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

    fn persist_config(
        &self,
        task: &Task,
        shared: &SharedBilibiliConfig,
        next: BilibiliConfig,
    ) -> Result<(), RuntimeError> {
        next.validate()
            .map_err(|detail| bili_error(task, BilibiliError::ManagementUnavailable(detail)))?;
        let store = self.config_store.as_ref().ok_or_else(|| {
            bili_error(
                task,
                BilibiliError::ManagementUnavailable(
                    "Host configured-plugin persistence is unavailable".into(),
                ),
            )
        })?;
        store
            .replace(&next)
            .map_err(|detail| bili_error(task, BilibiliError::ManagementUnavailable(detail)))?;
        shared.replace(next);
        Ok(())
    }

    fn qr_resource(
        &self,
        value: &str,
        task: &Task,
    ) -> Result<mutsuki_runtime_contracts::ResourceRef, RuntimeError> {
        let image = QrCode::new(value.as_bytes())
            .map_err(|error| bili_error(task, BilibiliError::InvalidResponse(error.to_string())))?
            .render::<image::Luma<u8>>()
            .min_dimensions(256, 256)
            .build();
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .map_err(|error| bili_error(task, BilibiliError::InvalidResponse(error.to_string())))?;
        self.resources
            .create_blob_resource(
                &self.media_provider_id,
                "mutsuki.bot.image.qrcode.png.v1",
                bytes.into_inner(),
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
    let mut builder = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestRunner {
            descriptor: runner_descriptor(command.is_some()),
        }))
        .protocol_handler(protocol(POLL_LIVE), RUNNER_ID, "io")
        .protocol_handler(protocol(POLL_DYNAMIC), RUNNER_ID, "io")
        .protocol_handler(protocol(POLL_VIDEO), RUNNER_ID, "io")
        .protocol_handler(protocol(LINK_RESOLVE), RUNNER_ID, "io");
    if let Some(command) = command {
        builder = builder.handler_binding(
            HandlerBindingBuilder::new(
                bot_command_binding_id(command),
                PLUGIN_ID,
                BOT_COMMAND_HANDLE_PROTOCOL_ID,
                BOT_COMMAND_HANDLE_PROTOCOL_ID,
            )
            .target_runner_hint(RUNNER_ID)
            .pool_id("io")
            .build(),
        );
    }
    builder.build().manifest
}

fn runner_descriptor(management: bool) -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID);
    for protocol in [POLL_LIVE, POLL_DYNAMIC, POLL_VIDEO, LINK_RESOLVE] {
        builder = builder.accepted_protocol(protocol);
    }
    if management {
        builder = builder.accepted_protocol(BOT_COMMAND_HANDLE_PROTOCOL_ID);
    }
    builder
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Io)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            max_inflight_batches: 1,
            ..Default::default()
        })
        .metadata("domain", ScalarValue::String("bilibili".into()))
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
    serde_json::from_value(task.payload.clone())
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

fn required_arg(args: &[String], index: usize, name: &str) -> Result<String, BilibiliError> {
    args.get(index)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(format!("missing {name}")))
}

fn parse_uid(value: Option<&String>) -> Result<u64, BilibiliError> {
    value
        .ok_or_else(|| BilibiliError::InvalidResponse("missing Bilibili UID".into()))?
        .parse::<u64>()
        .ok()
        .filter(|uid| *uid > 0)
        .ok_or_else(|| BilibiliError::InvalidResponse("invalid Bilibili UID".into()))
}

fn parse_notifications(value: Option<&String>) -> Result<Vec<BilibiliPollKind>, BilibiliError> {
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

fn binding_code(actor_id: &str, uid: u64, task_id: &str) -> String {
    let digest = format!("{:x}", md5::compute(format!("{actor_id}:{uid}:{task_id}")));
    format!("mutsuki-{}", &digest[..8])
}

fn self_subscription_id(command: &BotCommandEvent, actor_id: &str) -> String {
    let digest = format!(
        "{:x}",
        md5::compute(format!("{}:{actor_id}", command.source.platform.as_str()))
    );
    format!("self-{}", &digest[..12])
}

fn select_subscription(
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
    let code = match error {
        BilibiliError::CookieExpired => "bilibili.cookie_expired",
        BilibiliError::RateLimited => "bilibili.rate_limited",
        BilibiliError::RiskControl352 => "bilibili.risk_control_352",
        BilibiliError::Forbidden => "bilibili.management_forbidden",
        BilibiliError::ManagementUnavailable(_) => "bilibili.management_unavailable",
        _ => "bilibili.request_failed",
    };
    let mut runtime = RuntimeError::new(code, PLUGIN_ID, format!("bilibili.{}", task.task_id));
    runtime
        .evidence
        .insert("detail".into(), ScalarValue::String(error.to_string()));
    runtime
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
        PlanReceipt, ReadPlan, ResourceRef, SnapshotDescriptor, StreamPlan, WorkResourcePlan,
        WritePlan,
    };
    use mutsuki_runtime_sdk::ResourcePlanGateway;

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
                .contains(&BOT_COMMAND_HANDLE_PROTOCOL_ID.to_string())
        );
        assert!(managed.provides.handler_bindings.iter().any(|binding| {
            binding.binding_id == bot_command_binding_id("bili")
                && binding.target_runner_hint.as_deref() == Some(RUNNER_ID)
        }));
    }

    #[test]
    fn management_flow_rotates_secret_persists_verified_binding_and_previews_without_cursor() {
        let state = Arc::new(Mutex::new(FakeTransportState::default()));
        let config = SharedBilibiliConfig::new(managed_config());
        let credential_store = Arc::new(RecordingCredentialStore::default());
        let config_store = Arc::new(RecordingConfigStore::default());
        let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
        let mut runner = BilibiliRunner::new(
            Box::new(FakeTransport(state.clone())),
            repository.clone(),
            Arc::new(UnusedResources),
            "memory",
        )
        .with_management(
            config.clone(),
            credential_store.clone(),
            config_store.clone(),
        );

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

    fn managed_config() -> BilibiliConfig {
        BilibiliConfig {
            cookie_secret_key: "BILIBILI_COOKIE".into(),
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
            BOT_COMMAND_HANDLE_PROTOCOL_ID,
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
        assert_eq!(
            bili_error(&task, BilibiliError::RiskControl352).code,
            "bilibili.risk_control_352"
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
