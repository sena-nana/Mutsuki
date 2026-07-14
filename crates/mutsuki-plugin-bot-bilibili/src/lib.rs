use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mutsuki_bot_link_parser::ResolvedLinkCard;
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotExtMap, BotMessage, BotTarget, MessageSegment,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, RunnerBatchCapability, RunnerContext, RunnerDescriptor,
    RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError, ScalarValue, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
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
pub const MAX_MEDIA_BYTES: usize = 8 * 1024 * 1024;

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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PollRequest {
    pub uid: u64,
    pub kind: BilibiliPollKind,
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
    pub uid: u64,
    pub notifications: Vec<BilibiliPollKind>,
    pub target: BotTarget,
    pub outbound_binding: String,
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
    #[serde(default = "default_media_limit")]
    pub max_media_bytes: usize,
}

fn default_media_limit() -> usize {
    MAX_MEDIA_BYTES
}

impl BilibiliConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.cookie_secret_key.trim().is_empty() {
            return Err("cookie_secret_key is required".into());
        }
        if self.media_provider_id.trim().is_empty() {
            return Err("media_provider_id is required".into());
        }
        if self.max_media_bytes == 0 || self.max_media_bytes > MAX_MEDIA_BYTES {
            return Err(format!(
                "max_media_bytes must be between 1 and {MAX_MEDIA_BYTES}"
            ));
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
            if subscription.uid == 0
                || subscription.notifications.is_empty()
                || subscription.outbound_binding.trim().is_empty()
            {
                return Err("subscriptions require uid, notification types and binding".into());
            }
        }
        Ok(())
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
    pub live: Option<bool>,
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
}

pub trait BilibiliTransport: Send {
    fn poll(
        &mut self,
        kind: &BilibiliPollKind,
        uid: u64,
    ) -> Result<Vec<BilibiliItem>, BilibiliError>;
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, BilibiliError>;
    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BilibiliError>;
}

pub struct ReqwestBilibiliTransport {
    client: Client,
    credential: SharedBilibiliCredential,
}

impl ReqwestBilibiliTransport {
    pub fn new(
        credential: SharedBilibiliCredential,
        timeout: Duration,
    ) -> Result<Self, BilibiliError> {
        let client = Client::builder()
            .timeout(timeout)
            .user_agent("Mozilla/5.0 MutsukiBot/0.1")
            .build()
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        Ok(Self { client, credential })
    }

    fn json(&self, url: &str) -> Result<Value, BilibiliError> {
        ensure_bilibili_domain(url)?;
        let response = self
            .client
            .get(url)
            .header("Cookie", self.credential.get()?)
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

    fn wbi_url(&self, path: &str, params: Vec<(String, String)>) -> Result<String, BilibiliError> {
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
                .client
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
            .client
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
             CREATE TABLE IF NOT EXISTS cooldown (key TEXT PRIMARY KEY, seen_ms INTEGER NOT NULL);",
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
}

pub struct BilibiliRunner {
    descriptor: RunnerDescriptor,
    transport: Box<dyn BilibiliTransport>,
    repository: Arc<SqliteBilibiliRepository>,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
    max_media_bytes: usize,
}

impl BilibiliRunner {
    pub fn new(
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
        max_media_bytes: usize,
    ) -> Self {
        Self {
            descriptor: runner_descriptor(),
            transport,
            repository,
            resources,
            media_provider_id: media_provider_id.into(),
            max_media_bytes,
        }
    }

    fn run_task(&mut self, task: &Task) -> Result<RunnerResult, RuntimeError> {
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
        if request.kind.protocol_id() != task.protocol_id {
            return Err(bili_error(
                task,
                BilibiliError::InvalidResponse("poll kind/protocol mismatch".into()),
            ));
        }
        let items = self
            .transport
            .poll(&request.kind, request.uid)
            .map_err(|error| bili_error(task, error))?;
        let key = format!("{:?}:{}", request.kind, request.uid);
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
                description: match request.kind {
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
                .download(&image_url, self.max_media_bytes)
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
    PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestRunner {
            descriptor: runner_descriptor(),
        }))
        .protocol_handler(protocol(POLL_LIVE), RUNNER_ID, "io")
        .protocol_handler(protocol(POLL_DYNAMIC), RUNNER_ID, "io")
        .protocol_handler(protocol(POLL_VIDEO), RUNNER_ID, "io")
        .protocol_handler(protocol(LINK_RESOLVE), RUNNER_ID, "io")
        .build()
        .manifest
}

fn runner_descriptor() -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID);
    for protocol in [POLL_LIVE, POLL_DYNAMIC, POLL_VIDEO, LINK_RESOLVE] {
        builder = builder.accepted_protocol(protocol);
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
                live: Some(status),
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
                        live: None,
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
                        live: None,
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
            live: None,
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
}
