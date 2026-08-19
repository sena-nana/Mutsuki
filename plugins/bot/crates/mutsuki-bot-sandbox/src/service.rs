use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_conversation::qq_conversation_from_event;
use mutsuki_bot_protocol::{
    BotAccountRef, BotConversationKind, BotEvent, BotEventKind, BotExtMap, BotMessage, BotPlatform,
    BotUser, MessageSegment, QQ_CONVERSATION_REF_VERSION, QqConversationRef,
};
use mutsuki_runtime_contracts::{
    ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::content::{
    gc_assets, gc_stickers, hash_bytes, hydrate_segments, normalize_segments, preview_content,
    record_faces, upsert_asset,
};
use crate::types::{
    DEFAULT_SANDBOX_ACCOUNT_ID, LIVE_GROUP_TITLE_FALLBACK, SANDBOX_GROUP_ID, SANDBOX_ID_PREFIX,
    SANDBOX_MAX_MEDIA_BYTES, SANDBOX_MAX_MESSAGES, SANDBOX_MAX_STICKER_BYTES, SANDBOX_USER_LIMIT,
    SANDBOX_USER_NAMES, SandboxAction, SandboxAsset, SandboxChangeEvent, SandboxConversationView,
    SandboxError, SandboxFace, SandboxHistoryConversation, SandboxHistorySnapshot,
    SandboxMediaBlob, SandboxMediaRef, SandboxMessageView, SandboxMode, SandboxSnapshot,
    SandboxSpeakerRole, SandboxSticker, SandboxStickerKind, SandboxStickerView, SandboxUserView,
    SandboxWriteRequest, SandboxWriteResult, is_sandbox_conversation, parse_sandbox_mentions,
    sandbox_user_id,
};

const MAX_MESSAGES: usize = SANDBOX_MAX_MESSAGES;
const MAX_MEDIA_BYTES: usize = SANDBOX_MAX_MEDIA_BYTES;
const MAX_STICKER_BYTES: usize = SANDBOX_MAX_STICKER_BYTES;
const PASSIVE_REPLY_WINDOW_MS: u64 = 5 * 60 * 1_000;

pub struct SandboxChangeSubscription {
    receiver: broadcast::Receiver<SandboxChangeEvent>,
}

impl SandboxChangeSubscription {
    pub async fn changed(&mut self) -> Option<SandboxChangeEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// Optional live ingest/delivery hooks injected by the Service Host integration crate.
#[async_trait]
pub trait SandboxRuntime: Send + Sync {
    fn live_available(&self) -> bool;
    async fn ingest(&self, event: BotEvent) -> Result<(), SandboxError>;
    async fn deliver(
        &self,
        operation_id: &str,
        conversation: &QqConversationRef,
        segments: &[MessageSegment],
        reply_to: Option<&str>,
    ) -> Result<Value, SandboxError>;
}

#[async_trait]
pub trait SandboxApi: Send + Sync {
    fn subscribe_changes(&self) -> Option<SandboxChangeSubscription> {
        None
    }
    async fn snapshot(&self, query: &str) -> Result<SandboxSnapshot, SandboxError>;
    async fn write(
        &self,
        actor_id: &str,
        request: SandboxWriteRequest,
    ) -> Result<SandboxWriteResult, SandboxError>;
    async fn messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<SandboxMessageView>, SandboxError>;
    fn observe_event(&self, event: BotEvent);
    fn observe_outbound(
        &self,
        conversation: &QqConversationRef,
        segments: &[MessageSegment],
        reply_to: Option<&str>,
    ) -> Option<SandboxMessageView>;
    async fn upload_media(
        &self,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<SandboxMediaRef, SandboxError>;
    async fn media_blob(&self, media_id: &str) -> Result<SandboxMediaBlob, SandboxError>;
    async fn upload_sticker(
        &self,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<SandboxMediaRef, SandboxError>;
    async fn list_stickers(&self) -> Result<Vec<SandboxStickerView>, SandboxError>;
    async fn sticker_blob(&self, sticker_id: &str) -> Result<SandboxMediaBlob, SandboxError>;
}

/// Durable sandbox history. The sandbox writes snapshots; other plugins query
/// the same tables through `BotStateDbRepository`.
pub trait SandboxHistoryStore: Send + Sync {
    fn load(&self) -> Result<SandboxHistorySnapshot, SandboxError>;
    fn save(&self, snapshot: &SandboxHistorySnapshot) -> Result<(), SandboxError>;
}

#[derive(Clone)]
struct StoredConversation {
    view: SandboxConversationView,
    users: HashMap<String, SandboxUserView>,
    messages: Vec<SandboxMessageView>,
}

#[derive(Default)]
struct Store {
    conversations: HashMap<String, StoredConversation>,
}

struct Inner {
    revision: u64,
    mode: SandboxMode,
    account_id: String,
    simulate: Store,
    live: Store,
    media: HashMap<String, SandboxAsset>,
    stickers: HashMap<String, SandboxSticker>,
    faces: HashMap<String, SandboxFace>,
    bot: Option<BotUser>,
}

/// QQ conversation sandbox used by the Web Console, with optional durable history.
pub struct SandboxService {
    write_lock: tokio::sync::Mutex<()>,
    inner: Mutex<Inner>,
    runtime: Mutex<Option<Arc<dyn SandboxRuntime>>>,
    changes: broadcast::Sender<SandboxChangeEvent>,
    history: Option<Arc<dyn SandboxHistoryStore>>,
}

impl Default for SandboxService {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxService {
    #[must_use]
    pub fn new() -> Self {
        Self::with_account(DEFAULT_SANDBOX_ACCOUNT_ID)
    }

    #[must_use]
    pub fn with_account(account_id: impl Into<String>) -> Self {
        Self::from_snapshot(account_id.into(), SandboxHistorySnapshot::default(), None)
            .expect("in-memory sandbox cannot fail to seed")
    }

    /// Restores live and simulate history from `store`, seeding simulate only when empty.
    ///
    /// # Errors
    ///
    /// Returns an error when history cannot be loaded or the initial seed cannot be persisted.
    pub fn with_history(
        account_id: impl Into<String>,
        store: Arc<dyn SandboxHistoryStore>,
    ) -> Result<Self, SandboxError> {
        Self::from_snapshot(account_id.into(), store.load()?, Some(store))
    }

    fn from_snapshot(
        account_id: String,
        snapshot: SandboxHistorySnapshot,
        history: Option<Arc<dyn SandboxHistoryStore>>,
    ) -> Result<Self, SandboxError> {
        let mut simulate = store_from_history(snapshot.simulate);
        let live = store_from_history(snapshot.live);
        let (changes, _) = broadcast::channel(64);
        if simulate.conversations.is_empty() {
            seed_simulate(&mut simulate, &account_id);
        }
        let service = Self {
            write_lock: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner {
                revision: 0,
                mode: snapshot.mode,
                account_id,
                simulate,
                live,
                media: by_hash(snapshot.media, |item| item.content_hash.clone()),
                stickers: by_hash(snapshot.stickers, |item| item.content_hash.clone()),
                faces: by_hash(snapshot.faces, |item| item.face_key.clone()),
                bot: None,
            }),
            runtime: Mutex::new(None),
            changes,
            history,
        };
        service.persist()?;
        Ok(service)
    }

    /// Installs the live ingest/delivery runtime used by real-data mode.
    ///
    /// # Panics
    ///
    /// Panics if the runtime mutex is poisoned.
    pub fn set_runtime(&self, runtime: Arc<dyn SandboxRuntime>) {
        *self.runtime.lock().expect("sandbox runtime mutex") = Some(runtime);
    }

    pub fn set_bot_profile(&self, user: BotUser) {
        let mut inner = self.lock_inner();
        if inner.bot.as_ref() == Some(&user) {
            return;
        }
        inner.bot = Some(user);
        let (revision, mode) = finish(&mut inner);
        drop(inner);
        self.publish(revision, mode);
    }

    pub fn apply_live_title(&self, group_id: &str, title: &str) {
        let group_id = group_id.trim();
        let title = title.trim();
        if group_id.is_empty() || title.is_empty() {
            return;
        }
        let mut inner = self.lock_inner();
        let Some(stored) = inner.live.conversations.values_mut().find(|item| {
            item.view.kind == BotConversationKind::Group
                && item.view.conversation.group_id.as_deref() == Some(group_id)
        }) else {
            return;
        };
        if !assign_live_group_title(stored, title) {
            return;
        }
        let (revision, mode) = finish(&mut inner);
        drop(inner);
        self.persist_best_effort();
        self.publish(revision, mode);
    }

    fn runtime(&self) -> Option<Arc<dyn SandboxRuntime>> {
        self.runtime.lock().expect("sandbox runtime mutex").clone()
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("sandbox state mutex")
    }

    fn persist(&self) -> Result<(), SandboxError> {
        let Some(history) = &self.history else {
            return Ok(());
        };
        let snapshot = {
            let inner = self.lock_inner();
            snapshot_from_inner(&inner)
        };
        history.save(&snapshot)
    }

    fn persist_best_effort(&self) {
        let _ = self.persist();
    }

    fn publish(&self, revision: u64, mode: SandboxMode) {
        let _ = self.changes.send(SandboxChangeEvent { revision, mode });
    }

    fn require_flow(&self) -> Result<Arc<dyn SandboxRuntime>, SandboxError> {
        self.runtime()
            .ok_or_else(|| SandboxError::new("runtime.unavailable", "Bot 流程当前不可用"))
    }

    async fn ingest_as_user(
        &self,
        expected_revision: u64,
        conversation_id: String,
        user_id: String,
        text: String,
        segments: Vec<MessageSegment>,
        reply_to: Option<String>,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let runtime = self.require_flow()?;
        let (event, revision, mode, message) = {
            let mut inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            require_simulate(&inner, "真实模式不能伪造群成员发言")?;
            let stored = inner
                .simulate
                .conversations
                .get(&conversation_id)
                .ok_or_else(|| conversation_missing(&conversation_id))?;
            let user = stored.users.get(&user_id).cloned().ok_or_else(|| {
                SandboxError::new("not_found", format!("用户 `{user_id}` 不在当前会话"))
            })?;
            if let Some(reply_to) = reply_to.as_deref() {
                require_message(stored, reply_to)?;
            }
            let roster = stored.users.values().cloned().collect::<Vec<_>>();
            let mut segments = compose_segments(&text, segments, &roster)?;
            require_sandbox_refs(&inner, &segments)?;
            if let Some(reply_to) = reply_to.as_deref() {
                segments.insert(
                    0,
                    MessageSegment::Quote {
                        message_id: reply_to.to_owned(),
                    },
                );
            }
            let conversation = stored.view.conversation.clone();
            let account_id = inner.account_id.clone();
            let message = {
                let Inner {
                    simulate,
                    media,
                    faces,
                    ..
                } = &mut *inner;
                let stored = conversation_mut(simulate, &conversation_id)?;
                append_message(
                    stored,
                    media,
                    faces,
                    &user.user_id,
                    &user.display_name,
                    SandboxSpeakerRole::User,
                    segments,
                    reply_to,
                    None,
                    None,
                )
            };
            let event = sandbox_event(
                &account_id,
                &conversation,
                &user,
                BotEventKind::MessageCreated,
                Some(&message),
                message.time_ms,
            )?;
            let (revision, mode) = finish(&mut inner);
            (event, revision, mode, message)
        };
        self.persist()?;
        runtime.ingest(event).await?;
        self.publish(revision, mode);
        Ok(write_ok(
            revision,
            serde_json::to_value(message)
                .map_err(|error| SandboxError::new("encode_failed", error.to_string()))?,
        ))
    }

    async fn add_user(&self, expected_revision: u64) -> Result<SandboxWriteResult, SandboxError> {
        let runtime = self.require_flow()?;
        let (event, revision, mode, result) = {
            let mut inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            require_simulate(&inner, "真实模式不能修改虚拟用户")?;
            let taken = group_user_ids(&inner.simulate);
            let display_name = SANDBOX_USER_NAMES
                .iter()
                .copied()
                .find(|name| !taken.iter().any(|id| id == &sandbox_user_id(name)))
                .ok_or_else(|| SandboxError::new("invalid_state", "可创建的用户数量已达上限"))?;
            let now = unix_ms();
            let user_id = sandbox_user_id(display_name);
            let (group, user) = place_simulate_user(&mut inner, &user_id, display_name, None, now)?;
            let account_id = inner.account_id.clone();
            let event = sandbox_event(
                &account_id,
                &group,
                &user,
                BotEventKind::MemberJoined,
                None,
                i64::try_from(now).unwrap_or(i64::MAX),
            )?;
            let (revision, mode) = finish(&mut inner);
            (
                event,
                revision,
                mode,
                json!({
                    "user_id": user.user_id,
                    "display_name": user.display_name,
                }),
            )
        };
        self.persist()?;
        runtime.ingest(event).await?;
        self.publish(revision, mode);
        Ok(write_ok(revision, result))
    }

    async fn remove_user(
        &self,
        expected_revision: u64,
        user_id: String,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let runtime = self.require_flow()?;
        let (event, revision, mode, result) = {
            let mut inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            require_simulate(&inner, "真实模式不能修改虚拟用户")?;
            let account_id = inner.account_id.clone();
            let now = unix_ms();
            let group_id = group_conversation_id(&inner.simulate)?;
            let stored = conversation_mut(&mut inner.simulate, &group_id)?;
            let user = stored.users.remove(&user_id).ok_or_else(|| {
                SandboxError::new("not_found", format!("用户 `{user_id}` 不在当前会话"))
            })?;
            let group = stored.view.conversation.clone();
            let private_id = private_ref(&account_id, &user_id).origin_key();
            inner.simulate.conversations.remove(&private_id);
            let event = sandbox_event(
                &account_id,
                &group,
                &user,
                BotEventKind::MemberLeft,
                None,
                i64::try_from(now).unwrap_or(i64::MAX),
            )?;
            let (revision, mode) = finish(&mut inner);
            (event, revision, mode, json!({ "user_id": user.user_id }))
        };
        self.persist()?;
        runtime.ingest(event).await?;
        self.publish(revision, mode);
        Ok(write_ok(revision, result))
    }

    async fn update_user(
        &self,
        expected_revision: u64,
        user_id: String,
        new_user_id: String,
        display_name: String,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let user_id = require_identity(&user_id, "OpenID")?;
        let new_user_id = require_identity(&new_user_id, "OpenID")?;
        let display_name = display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 64 {
            return Err(SandboxError::new("invalid_argument", "昵称无效"));
        }
        let display_name = display_name.to_owned();
        let renamed = new_user_id != user_id;
        if renamed {
            self.require_flow()?;
        }
        let (events, revision, mode, result) = {
            let mut inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            require_simulate(&inner, "真实模式不能修改虚拟用户")?;
            let now = unix_ms();
            let account_id = inner.account_id.clone();
            let group_id = group_conversation_id(&inner.simulate)?;
            let (existing, group) = {
                let stored = conversation_mut(&mut inner.simulate, &group_id)?;
                let existing = stored.users.get(&user_id).cloned().ok_or_else(|| {
                    SandboxError::new("not_found", format!("用户 `{user_id}` 不在当前会话"))
                })?;
                if renamed && stored.users.contains_key(&new_user_id) {
                    return Err(SandboxError::new(
                        "already_exists",
                        format!("用户 `{new_user_id}` 已在沙盒中"),
                    ));
                }
                (existing, stored.view.conversation.clone())
            };
            let user =
                relocate_simulate_user(&mut inner, &user_id, &new_user_id, &display_name, now)?;
            let events = if renamed {
                vec![
                    sandbox_event(
                        &account_id,
                        &group,
                        &existing,
                        BotEventKind::MemberLeft,
                        None,
                        i64::try_from(now).unwrap_or(i64::MAX),
                    )?,
                    sandbox_event(
                        &account_id,
                        &group,
                        &user,
                        BotEventKind::MemberJoined,
                        None,
                        i64::try_from(now).unwrap_or(i64::MAX),
                    )?,
                ]
            } else {
                Vec::new()
            };
            let (revision, mode) = finish(&mut inner);
            (
                events,
                revision,
                mode,
                json!({
                    "user_id": user.user_id,
                    "display_name": user.display_name,
                }),
            )
        };
        self.persist()?;
        if !events.is_empty() {
            let runtime = self.require_flow()?;
            for event in events {
                runtime.ingest(event).await?;
            }
        }
        self.publish(revision, mode);
        Ok(write_ok(revision, result))
    }

    async fn import_live_users(
        &self,
        expected_revision: u64,
        user_ids: Vec<String>,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let runtime = self.require_flow()?;
        if user_ids.is_empty() {
            return Err(SandboxError::new("invalid_argument", "请选择要导入的成员"));
        }
        let (events, revision, mode, result) = {
            let mut inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            let live_users = collect_live_users(&inner.live);
            let mut imported = Vec::new();
            let mut skipped = Vec::new();
            let mut events = Vec::new();
            let now = unix_ms();
            let account_id = inner.account_id.clone();
            for raw_id in user_ids {
                let user_id = require_identity(&raw_id, "OpenID")?;
                let Some(source) = live_users.iter().find(|user| user.user_id == user_id) else {
                    return Err(SandboxError::new(
                        "not_found",
                        format!("真实数据中没有成员 `{user_id}`"),
                    ));
                };
                match place_simulate_user(
                    &mut inner,
                    &user_id,
                    &source.display_name,
                    source.avatar_url.as_deref(),
                    now,
                ) {
                    Ok((group, user)) => {
                        events.push(sandbox_event(
                            &account_id,
                            &group,
                            &user,
                            BotEventKind::MemberJoined,
                            None,
                            i64::try_from(now).unwrap_or(i64::MAX),
                        )?);
                        imported.push(json!({
                            "user_id": user.user_id,
                            "display_name": user.display_name,
                        }));
                    }
                    Err(error)
                        if error.code == "already_exists" || error.code == "invalid_state" =>
                    {
                        skipped.push(json!({ "user_id": user_id, "reason": error.code }));
                    }
                    Err(error) => return Err(error),
                }
            }
            let (revision, mode) = finish(&mut inner);
            (
                events,
                revision,
                mode,
                json!({ "imported": imported, "skipped": skipped }),
            )
        };
        self.persist()?;
        for event in events {
            runtime.ingest(event).await?;
        }
        self.publish(revision, mode);
        Ok(write_ok(revision, result))
    }

    fn clear_conversation(
        &self,
        expected_revision: u64,
        conversation_id: &str,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let mut inner = self.lock_inner();
        require_revision(&inner, expected_revision)?;
        require_simulate(&inner, "真实模式不能修改虚拟用户")?;
        let stored = conversation_mut(&mut inner.simulate, conversation_id)?;
        stored.messages.clear();
        stored.view.last_preview = None;
        stored.view.message_count = 0;
        for user in stored.users.values_mut() {
            user.message_count = 0;
        }
        let (revision, mode) = finish(&mut inner);
        drop(inner);
        self.persist()?;
        self.publish(revision, mode);
        Ok(write_ok(revision, json!({ "cleared": true })))
    }

    async fn send_as_bot(
        &self,
        operation_id: &str,
        expected_revision: u64,
        conversation_id: String,
        text: String,
        segments: Vec<MessageSegment>,
        reply_to: Option<String>,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let runtime = self.runtime().ok_or_else(|| {
            SandboxError::new("qq.owner_unavailable", "尚未连接 QQ，无法发送真实消息")
        })?;
        if !runtime.live_available() {
            return Err(SandboxError::new(
                "qq.owner_unavailable",
                "尚未连接 QQ，无法发送真实消息",
            ));
        }
        let (conversation, segments, reply_to) = {
            let inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            if inner.mode != SandboxMode::Live {
                return Err(SandboxError::new(
                    "invalid_state",
                    "模拟模式不能以后台机器人身份发送",
                ));
            }
            let stored = inner
                .live
                .conversations
                .get(&conversation_id)
                .ok_or_else(|| conversation_missing(&conversation_id))?;
            let quoted = reply_to
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let reply_to = match quoted {
                Some(id) => Some(require_live_reply_target(stored, id, unix_ms())?),
                None if stored.view.active_message => None,
                None => {
                    return Err(SandboxError::new(
                        "invalid_argument",
                        "当前会话没有主动消息权限，请先悬停用户消息并点击回复",
                    ));
                }
            };
            let conversation = stored.view.conversation.clone();
            let roster = stored.users.values().cloned().collect::<Vec<_>>();
            let segments = compose_segments(&text, segments, &roster)?;
            require_sandbox_refs(&inner, &segments)?;
            let segments = segments
                .into_iter()
                .map(|segment| expand_live_segment(&inner, segment))
                .collect::<Result<Vec<_>, _>>()?;
            require_live_outbound(&segments)?;
            (conversation, segments, reply_to)
        };
        let delivery = runtime
            .deliver(operation_id, &conversation, &segments, reply_to.as_deref())
            .await?;
        let recorded = {
            let mut inner = self.lock_inner();
            let (sender_id, sender_name) = bot_speaker(&inner);
            let message = {
                let Inner {
                    live, media, faces, ..
                } = &mut *inner;
                let stored = conversation_mut(live, &conversation_id)?;
                append_message(
                    stored,
                    media,
                    faces,
                    &sender_id,
                    &sender_name,
                    SandboxSpeakerRole::Bot,
                    segments,
                    reply_to,
                    None,
                    None,
                )
            };
            let (revision, mode) = finish(&mut inner);
            (revision, mode, message)
        };
        self.persist()?;
        self.publish(recorded.0, recorded.1);
        Ok(write_ok(
            recorded.0,
            json!({ "message": recorded.2, "delivery": delivery }),
        ))
    }
}

#[async_trait]
impl SandboxApi for SandboxService {
    fn subscribe_changes(&self) -> Option<SandboxChangeSubscription> {
        Some(SandboxChangeSubscription {
            receiver: self.changes.subscribe(),
        })
    }

    async fn snapshot(&self, query: &str) -> Result<SandboxSnapshot, SandboxError> {
        let inner = self.lock_inner();
        let query = query.trim().to_ascii_lowercase();
        let mut conversations = active_store(&inner)
            .conversations
            .values()
            .map(projected_conversation)
            .filter(|item| {
                query.is_empty()
                    || item.title.to_ascii_lowercase().contains(&query)
                    || item
                        .users
                        .iter()
                        .any(|user| user.display_name.to_ascii_lowercase().contains(&query))
            })
            .collect::<Vec<_>>();
        conversations.sort_by(|left, right| {
            right
                .last_activity_unix_ms
                .cmp(&left.last_activity_unix_ms)
                .then_with(|| left.conversation_id.cmp(&right.conversation_id))
        });
        Ok(SandboxSnapshot {
            revision: inner.revision,
            mode: inner.mode,
            live_available: self
                .runtime()
                .is_some_and(|runtime| runtime.live_available()),
            flow_available: self.runtime().is_some(),
            account_id: inner.account_id.clone(),
            conversations,
            live_users: collect_live_users(&inner.live),
            bot: inner.bot.clone(),
        })
    }

    async fn write(
        &self,
        _actor_id: &str,
        request: SandboxWriteRequest,
    ) -> Result<SandboxWriteResult, SandboxError> {
        if request.operation_id.is_empty()
            || request.operation_id.len() > 128
            || !request
                .operation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(SandboxError::new(
                "invalid_argument",
                "operation_id is invalid",
            ));
        }
        let _write = self.write_lock.lock().await;
        match request.action {
            SandboxAction::SetMode { mode } => {
                let mut inner = self.lock_inner();
                require_revision(&inner, request.expected_revision)?;
                inner.mode = mode;
                let (revision, _) = finish(&mut inner);
                drop(inner);
                self.persist()?;
                self.publish(revision, mode);
                Ok(write_ok(revision, json!({ "mode": mode })))
            }
            SandboxAction::AddUser => self.add_user(request.expected_revision).await,
            SandboxAction::UpdateUser {
                user_id,
                new_user_id,
                display_name,
            } => {
                self.update_user(
                    request.expected_revision,
                    user_id,
                    new_user_id,
                    display_name,
                )
                .await
            }
            SandboxAction::ImportLiveUsers { user_ids } => {
                self.import_live_users(request.expected_revision, user_ids)
                    .await
            }
            SandboxAction::RemoveUser { user_id } => {
                self.remove_user(request.expected_revision, user_id).await
            }
            SandboxAction::ClearConversation { conversation_id } => {
                self.clear_conversation(request.expected_revision, &conversation_id)
            }
            SandboxAction::IngestAsUser {
                conversation_id,
                user_id,
                text,
                segments,
                reply_to,
            } => {
                self.ingest_as_user(
                    request.expected_revision,
                    conversation_id,
                    user_id,
                    text,
                    segments,
                    reply_to,
                )
                .await
            }
            SandboxAction::SendAsBot {
                conversation_id,
                text,
                segments,
                reply_to,
            } => {
                self.send_as_bot(
                    &request.operation_id,
                    request.expected_revision,
                    conversation_id,
                    text,
                    segments,
                    reply_to,
                )
                .await
            }
        }
    }

    async fn messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<SandboxMessageView>, SandboxError> {
        let inner = self.lock_inner();
        active_store(&inner)
            .conversations
            .get(conversation_id)
            .map(|stored| stored.messages.clone())
            .ok_or_else(|| conversation_missing(conversation_id))
    }

    fn observe_event(&self, event: BotEvent) {
        if event.kind == BotEventKind::BotConnected {
            if let Some(actor) = event.actor {
                self.set_bot_profile(actor);
            }
            return;
        }
        let Ok(conversation) = qq_conversation_from_event(&event) else {
            return;
        };
        if is_sandbox_conversation(&conversation) || event.kind == BotEventKind::BotDisconnected {
            return;
        }
        let now = u64::try_from(event.time_ms.max(0)).unwrap_or(unix_ms());
        let active_message = active_message_permission(&event);
        let group_name = event
            .ext
            .get("qqbot.group_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let mut inner = self.lock_inner();
        let account_id = inner.account_id.clone();
        {
            let Inner {
                live, media, faces, ..
            } = &mut *inner;
            let stored = ensure_conversation(live, conversation, &live_title, now);
            if let Some(name) = group_name.as_deref() {
                assign_live_group_title(stored, name);
            }
            if let Some(allowed) = active_message {
                stored.view.active_message = allowed;
            }
            if let Some(actor) = &event.actor {
                upsert_user(
                    stored,
                    &actor.user_id,
                    actor.display_name.as_deref(),
                    actor.avatar_url.as_deref(),
                    now,
                );
            }
            if let Some(message) = event.message.as_ref() {
                let sender = event.actor.as_ref();
                let sender_id = sender.map_or("unknown", |user| user.user_id.as_str());
                let sender_name = sender
                    .and_then(|user| user.display_name.as_deref())
                    .unwrap_or(sender_id);
                let role = if sender_id == account_id {
                    SandboxSpeakerRole::Bot
                } else {
                    SandboxSpeakerRole::User
                };
                append_message(
                    stored,
                    media,
                    faces,
                    sender_id,
                    sender_name,
                    role,
                    message.segments.clone(),
                    message.reply_to.clone(),
                    message.message_id.clone(),
                    Some(event.time_ms),
                );
            }
        }
        let (revision, mode) = finish(&mut inner);
        drop(inner);
        self.persist_best_effort();
        self.publish(revision, mode);
    }

    fn observe_outbound(
        &self,
        conversation: &QqConversationRef,
        segments: &[MessageSegment],
        reply_to: Option<&str>,
    ) -> Option<SandboxMessageView> {
        let sandbox = is_sandbox_conversation(conversation);
        let now = unix_ms();
        let mut inner = self.lock_inner();
        let (sender_id, sender_name) = bot_speaker(&inner);
        let message = {
            let title: fn(&QqConversationRef) -> String =
                if sandbox { sandbox_title } else { live_title };
            let Inner {
                simulate,
                live,
                media,
                faces,
                ..
            } = &mut *inner;
            let stored = if sandbox {
                ensure_conversation(simulate, conversation.clone(), &title, now)
            } else {
                ensure_conversation(live, conversation.clone(), &title, now)
            };
            append_message(
                stored,
                media,
                faces,
                &sender_id,
                &sender_name,
                SandboxSpeakerRole::Bot,
                segments.to_vec(),
                reply_to.map(str::to_owned),
                None,
                None,
            )
        };
        let (revision, mode) = finish(&mut inner);
        drop(inner);
        self.persist_best_effort();
        self.publish(revision, mode);
        Some(message)
    }

    async fn upload_media(
        &self,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<SandboxMediaRef, SandboxError> {
        let name = name.trim();
        let mime = mime.trim();
        if name.is_empty() || name.len() > 128 {
            return Err(SandboxError::new("invalid_argument", "文件名无效"));
        }
        if mime.is_empty() || mime.len() > 128 {
            return Err(SandboxError::new("invalid_argument", "媒体类型无效"));
        }
        if bytes.is_empty() || bytes.len() > MAX_MEDIA_BYTES {
            return Err(SandboxError::new(
                "invalid_argument",
                format!("媒体大小必须在 1 到 {MAX_MEDIA_BYTES} 字节之间"),
            ));
        }
        let kind = if mime.starts_with("image/") {
            "image"
        } else if mime.starts_with("audio/") {
            "audio"
        } else if mime.starts_with("video/") {
            "video"
        } else {
            "file"
        };
        let hash = hash_bytes(&bytes);
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.media.get(&hash) {
            return Ok(media_ref(
                existing.content_hash.clone(),
                &existing.mime,
                &existing.name,
            ));
        }
        let created_at = next_created_at(&inner);
        upsert_asset(
            &mut inner.media,
            SandboxAsset {
                content_hash: hash.clone(),
                kind: kind.into(),
                mime: mime.to_owned(),
                name: name.to_owned(),
                bytes,
                url: None,
                created_at_unix_ms: created_at,
            },
        );
        gc_inner(&mut inner);
        drop(inner);
        self.persist()?;
        Ok(media_ref(hash, mime, name))
    }

    async fn media_blob(&self, media_id: &str) -> Result<SandboxMediaBlob, SandboxError> {
        let media_id = media_id.trim();
        if media_id.is_empty() {
            return Err(SandboxError::new("invalid_argument", "媒体 ID 无效"));
        }
        {
            let inner = self.lock_inner();
            if let Some(item) = inner
                .media
                .get(media_id)
                .filter(|item| !item.bytes.is_empty())
            {
                return Ok(SandboxMediaBlob {
                    media_id: item.content_hash.clone(),
                    mime: item.mime.clone(),
                    name: item.name.clone(),
                    bytes: item.bytes.clone(),
                });
            }
        }
        Err(SandboxError::new(
            "not_found",
            format!("媒体 `{media_id}` 不存在"),
        ))
    }

    async fn upload_sticker(
        &self,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<SandboxMediaRef, SandboxError> {
        let name = name.trim();
        let mime = mime.trim();
        if name.is_empty() || name.len() > 128 {
            return Err(SandboxError::new("invalid_argument", "文件名无效"));
        }
        if !mime.starts_with("image/") || mime.len() > 128 {
            return Err(SandboxError::new("invalid_argument", "表情包必须是图片"));
        }
        if bytes.is_empty() || bytes.len() > MAX_STICKER_BYTES {
            return Err(SandboxError::new(
                "invalid_argument",
                format!("表情包大小必须在 1 到 {MAX_STICKER_BYTES} 字节之间"),
            ));
        }
        let hash = hash_bytes(&bytes);
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.stickers.get(&hash) {
            return Ok(media_ref(
                existing.content_hash.clone(),
                &existing.mime,
                &existing.name,
            ));
        }
        let created_at = next_created_at(&inner);
        inner.stickers.insert(
            hash.clone(),
            SandboxSticker {
                content_hash: hash.clone(),
                mime: mime.to_owned(),
                name: name.to_owned(),
                bytes,
                created_at_unix_ms: created_at,
            },
        );
        gc_stickers(&mut inner.stickers);
        drop(inner);
        self.persist()?;
        Ok(media_ref(hash, mime, name))
    }

    async fn list_stickers(&self) -> Result<Vec<SandboxStickerView>, SandboxError> {
        let inner = self.lock_inner();
        let mut custom = inner.stickers.values().collect::<Vec<_>>();
        custom.sort_by(|left, right| {
            right
                .created_at_unix_ms
                .cmp(&left.created_at_unix_ms)
                .then_with(|| left.content_hash.cmp(&right.content_hash))
        });
        let mut listed = custom
            .into_iter()
            .map(|sticker| SandboxStickerView {
                id: sticker.content_hash.clone(),
                kind: SandboxStickerKind::Custom,
                mime: Some(sticker.mime.clone()),
                name: Some(sticker.name.clone()),
                face_type: None,
                face_id: None,
            })
            .collect::<Vec<_>>();
        let mut faces = inner.faces.values().collect::<Vec<_>>();
        faces.sort_by(|left, right| {
            right
                .last_seen_unix_ms
                .cmp(&left.last_seen_unix_ms)
                .then_with(|| left.face_key.cmp(&right.face_key))
        });
        listed.extend(faces.into_iter().map(|face| SandboxStickerView {
            id: face.face_key.clone(),
            kind: SandboxStickerKind::QqFace,
            mime: None,
            name: None,
            face_type: Some(face.face_type.clone()),
            face_id: Some(face.face_id.clone()),
        }));
        Ok(listed)
    }

    async fn sticker_blob(&self, sticker_id: &str) -> Result<SandboxMediaBlob, SandboxError> {
        let sticker_id = sticker_id.trim();
        if sticker_id.is_empty() {
            return Err(SandboxError::new("invalid_argument", "表情包 ID 无效"));
        }
        self.lock_inner()
            .stickers
            .get(sticker_id)
            .filter(|item| !item.bytes.is_empty())
            .map(|item| SandboxMediaBlob {
                media_id: item.content_hash.clone(),
                mime: item.mime.clone(),
                name: item.name.clone(),
                bytes: item.bytes.clone(),
            })
            .ok_or_else(|| SandboxError::new("not_found", format!("表情包 `{sticker_id}` 不存在")))
    }
}

fn snapshot_from_inner(inner: &Inner) -> SandboxHistorySnapshot {
    SandboxHistorySnapshot {
        mode: inner.mode,
        account_id: inner.account_id.clone(),
        simulate: inner
            .simulate
            .conversations
            .values()
            .map(history_conversation)
            .collect(),
        live: inner
            .live
            .conversations
            .values()
            .map(history_conversation)
            .collect(),
        media: inner.media.values().cloned().collect(),
        stickers: inner.stickers.values().cloned().collect(),
        faces: inner.faces.values().cloned().collect(),
    }
}

fn history_conversation(stored: &StoredConversation) -> SandboxHistoryConversation {
    SandboxHistoryConversation {
        view: projected_conversation(stored),
        users: stored.users.values().cloned().collect(),
        messages: stored.messages.clone(),
    }
}

fn store_from_history(items: Vec<SandboxHistoryConversation>) -> Store {
    let mut store = Store::default();
    for mut item in items {
        if item.messages.len() > MAX_MESSAGES {
            item.messages.drain(0..item.messages.len() - MAX_MESSAGES);
        }
        let mut users = HashMap::new();
        for user in item.users {
            users.insert(user.user_id.clone(), user);
        }
        store.conversations.insert(
            item.view.conversation_id.clone(),
            StoredConversation {
                view: item.view,
                users,
                messages: item.messages,
            },
        );
    }
    store
}

fn seed_simulate(store: &mut Store, account_id: &str) {
    let now = unix_ms();
    let group =
        insert_conversation(store, group_ref(account_id), "沙盒体验群", now).expect("seed group");
    if let Some(stored) = store.conversations.get_mut(&group.conversation_id) {
        for name in ["Alice", "Bob"] {
            upsert_user(stored, &sandbox_user_id(name), Some(name), None, now);
        }
        append_message(
            stored,
            &mut HashMap::new(),
            &mut HashMap::new(),
            "system",
            "系统",
            SandboxSpeakerRole::System,
            vec![MessageSegment::text(
                "这是虚拟 QQ 会话。以群成员身份发言会进入 Bot 流程，机器人回复会回到这里。",
            )],
            None,
            None,
            None,
        );
    }
    for name in ["Alice", "Bob"] {
        let user_id = sandbox_user_id(name);
        let private = insert_conversation(store, private_ref(account_id, &user_id), name, now)
            .expect("seed private");
        if let Some(stored) = store.conversations.get_mut(&private.conversation_id) {
            upsert_user(stored, &user_id, Some(name), None, now);
        }
    }
}

fn insert_conversation(
    store: &mut Store,
    conversation: QqConversationRef,
    title: &str,
    now: u64,
) -> Result<SandboxConversationView, SandboxError> {
    conversation
        .validate()
        .map_err(|error| SandboxError::new("invalid_argument", error.to_string()))?;
    let conversation_id = conversation.origin_key();
    if store.conversations.contains_key(&conversation_id) {
        return Err(SandboxError::new("already_exists", "会话已存在"));
    }
    let stored = StoredConversation {
        view: SandboxConversationView {
            conversation_id: conversation_id.clone(),
            account_id: conversation.account_id.clone(),
            kind: conversation.kind,
            title: if title.is_empty() {
                live_title(&conversation)
            } else {
                title.to_owned()
            },
            avatar_url: None,
            conversation,
            users: Vec::new(),
            last_preview: None,
            last_activity_unix_ms: now,
            message_count: 0,
            active_message: true,
        },
        users: HashMap::new(),
        messages: Vec::new(),
    };
    let view = projected_conversation(&stored);
    store.conversations.insert(conversation_id, stored);
    Ok(view)
}

fn ensure_conversation<'a>(
    store: &'a mut Store,
    conversation: QqConversationRef,
    title: &dyn Fn(&QqConversationRef) -> String,
    now: u64,
) -> &'a mut StoredConversation {
    let key = conversation.origin_key();
    if !store.conversations.contains_key(&key) {
        let label = title(&conversation);
        let _ = insert_conversation(store, conversation, &label, now);
    }
    store
        .conversations
        .get_mut(&key)
        .expect("conversation just inserted")
}

fn require_text(text: &str) -> Result<String, SandboxError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(SandboxError::new("invalid_argument", "消息不能为空"));
    }
    Ok(text.to_owned())
}

fn compose_segments(
    text: &str,
    segments: Vec<MessageSegment>,
    users: &[SandboxUserView],
) -> Result<Vec<MessageSegment>, SandboxError> {
    let mut body = if segments.is_empty() {
        parse_sandbox_mentions(&require_text(text)?, users)
    } else {
        let mut body = segments;
        if !text.trim().is_empty() {
            if body
                .iter()
                .any(|segment| is_markdown(segment) || is_keyboard(segment))
            {
                return Err(SandboxError::new(
                    "invalid_argument",
                    "Markdown 不能与普通文本混发",
                ));
            }
            body.extend(parse_sandbox_mentions(text.trim(), users));
        }
        body
    };
    body.retain(|segment| match segment {
        MessageSegment::Text { text } => !text.is_empty(),
        MessageSegment::Markdown { content } => !content.trim().is_empty(),
        _ => true,
    });
    if body.is_empty() {
        return Err(SandboxError::new("invalid_argument", "消息不能为空"));
    }
    require_markdown_combo(&body)?;
    Ok(body)
}

fn require_live_outbound(segments: &[MessageSegment]) -> Result<(), SandboxError> {
    for segment in segments {
        match segment {
            MessageSegment::Text { .. }
            | MessageSegment::MentionUser { .. }
            | MessageSegment::MentionAll
            | MessageSegment::Image { .. }
            | MessageSegment::File { .. }
            | MessageSegment::Audio { .. }
            | MessageSegment::Video { .. }
            | MessageSegment::Reply { .. }
            | MessageSegment::Quote { .. }
            | MessageSegment::Markdown { .. } => {}
            MessageSegment::PlatformSpecific { platform, kind, .. }
                if platform == "qqbot" && kind == "keyboard" => {}
            _ => {
                return Err(SandboxError::new(
                    "invalid_argument",
                    "真实模式只能发送文本、艾特、媒体、Markdown 和按钮",
                ));
            }
        }
    }
    Ok(())
}

fn expand_live_segment(
    inner: &Inner,
    segment: MessageSegment,
) -> Result<MessageSegment, SandboxError> {
    if let Some(media_id) = sandbox_payload_id(&segment, "media", "media_id") {
        let asset = inner
            .media
            .get(media_id)
            .ok_or_else(|| SandboxError::new("not_found", format!("媒体 `{media_id}` 不存在")))?;
        return Ok(media_segment_from_asset(asset));
    }
    if let Some(sticker_id) = sandbox_payload_id(&segment, "sticker", "sticker_id") {
        let sticker = inner.stickers.get(sticker_id).ok_or_else(|| {
            SandboxError::new("not_found", format!("表情包 `{sticker_id}` 不存在"))
        })?;
        return Ok(MessageSegment::Image {
            resource: sandbox_resource(&sticker.content_hash, &sticker.mime, sticker.bytes.len()),
        });
    }
    match &segment {
        MessageSegment::PlatformSpecific { kind, .. } if kind == "face" => Err(SandboxError::new(
            "invalid_argument",
            "真实模式不能发送 QQ 表情，请改用图片表情包",
        )),
        _ => Ok(segment),
    }
}

fn media_segment_from_asset(asset: &SandboxAsset) -> MessageSegment {
    let resource = sandbox_resource(&asset.content_hash, &asset.mime, asset.bytes.len());
    if asset.mime.starts_with("image/") {
        MessageSegment::Image { resource }
    } else if asset.mime.starts_with("audio/") {
        MessageSegment::Audio { resource }
    } else if asset.mime.starts_with("video/") {
        MessageSegment::Video { resource }
    } else {
        MessageSegment::File {
            resource,
            name: Some(asset.name.clone()).filter(|name| !name.is_empty()),
        }
    }
}

fn sandbox_resource(hash: &str, mime: &str, size: usize) -> ResourceRef {
    ResourceRef {
        ref_id: format!("sandbox:{hash}").into(),
        resource_id: ResourceId {
            kind_id: "blob".into(),
            slot_id: hash.into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: "sandbox".into(),
        resource_kind: "blob".into(),
        schema: mime.into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::Inline,
        size_hint: Some(size as u64),
        content_hash: Some(hash.into()),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn require_markdown_combo(segments: &[MessageSegment]) -> Result<(), SandboxError> {
    let markdown = segments
        .iter()
        .filter(|segment| is_markdown(segment))
        .count();
    let keyboard = segments
        .iter()
        .filter(|segment| is_keyboard(segment))
        .count();
    if keyboard > 0 && markdown == 0 {
        return Err(SandboxError::new(
            "invalid_argument",
            "按钮需要附在 Markdown 消息上",
        ));
    }
    if markdown > 1 || keyboard > 1 {
        return Err(SandboxError::new(
            "invalid_argument",
            "一次只能发送一条 Markdown 和一组按钮",
        ));
    }
    if markdown > 0
        && segments.iter().any(|segment| {
            !matches!(
                segment,
                MessageSegment::Markdown { .. }
                    | MessageSegment::Reply { .. }
                    | MessageSegment::Quote { .. }
            ) && !is_keyboard(segment)
                && !is_markdown(segment)
        })
    {
        return Err(SandboxError::new(
            "invalid_argument",
            "Markdown 不能与文本、艾特或媒体混发",
        ));
    }
    Ok(())
}

fn is_markdown(segment: &MessageSegment) -> bool {
    matches!(segment, MessageSegment::Markdown { .. }) || qq_kind(segment, "markdown")
}

fn is_keyboard(segment: &MessageSegment) -> bool {
    qq_kind(segment, "keyboard")
}

fn qq_kind(segment: &MessageSegment, kind: &str) -> bool {
    matches!(
        segment,
        MessageSegment::PlatformSpecific { platform, kind: k, .. }
            if platform == "qqbot" && k == kind
    )
}

fn require_sandbox_refs(inner: &Inner, segments: &[MessageSegment]) -> Result<(), SandboxError> {
    for segment in segments {
        if let Some(media_id) = sandbox_payload_id(segment, "media", "media_id")
            && !inner.media.contains_key(media_id)
        {
            return Err(SandboxError::new(
                "not_found",
                format!("媒体 `{media_id}` 不存在"),
            ));
        }
        if let Some(sticker_id) = sandbox_payload_id(segment, "sticker", "sticker_id")
            && !inner.stickers.contains_key(sticker_id)
        {
            return Err(SandboxError::new(
                "not_found",
                format!("表情包 `{sticker_id}` 不存在"),
            ));
        }
    }
    Ok(())
}

fn sandbox_payload_id<'a>(segment: &'a MessageSegment, kind: &str, key: &str) -> Option<&'a str> {
    match segment {
        MessageSegment::PlatformSpecific {
            platform,
            kind: segment_kind,
            payload,
        } if platform == "sandbox" && segment_kind == kind => {
            payload.get(key).and_then(Value::as_str)
        }
        _ => None,
    }
}

fn require_identity(value: &str, field: &str) -> Result<String, SandboxError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || ch == '|')
        || value == SANDBOX_GROUP_ID
    {
        return Err(SandboxError::new(
            "invalid_argument",
            format!("{field} 无效"),
        ));
    }
    Ok(value.to_owned())
}

fn require_simulate(inner: &Inner, message: &str) -> Result<(), SandboxError> {
    if inner.mode == SandboxMode::Simulate {
        Ok(())
    } else {
        Err(SandboxError::new("invalid_state", message))
    }
}

fn require_message(stored: &StoredConversation, message_id: &str) -> Result<(), SandboxError> {
    stored
        .messages
        .iter()
        .find(|item| item.message_id == message_id)
        .map(|_| ())
        .ok_or_else(|| SandboxError::new("not_found", format!("消息 `{message_id}` 不存在")))
}

fn require_live_reply_target(
    stored: &StoredConversation,
    reply_to: &str,
    now_unix_ms: u64,
) -> Result<String, SandboxError> {
    let message = stored
        .messages
        .iter()
        .find(|item| item.message_id == reply_to)
        .ok_or_else(|| {
            SandboxError::new(
                "invalid_argument",
                format!("引用的消息 `{reply_to}` 不存在"),
            )
        })?;
    if message.role != SandboxSpeakerRole::User {
        return Err(SandboxError::new("invalid_argument", "只能回复用户消息"));
    }
    let message_time = u64::try_from(message.time_ms.max(0)).unwrap_or(0);
    if now_unix_ms.saturating_sub(message_time) > PASSIVE_REPLY_WINDOW_MS {
        return Err(SandboxError::new(
            "invalid_argument",
            "引用的消息已超过 5 分钟，无法被动回复；请先在会话里 @ 机器人",
        ));
    }
    Ok(reply_to.to_owned())
}

fn group_user_ids(store: &Store) -> Vec<String> {
    store
        .conversations
        .values()
        .find(|item| item.view.kind == BotConversationKind::Group)
        .map(|item| item.users.keys().cloned().collect())
        .unwrap_or_default()
}

fn group_conversation_id(store: &Store) -> Result<String, SandboxError> {
    store
        .conversations
        .values()
        .find(|item| item.view.kind == BotConversationKind::Group)
        .map(|item| item.view.conversation_id.clone())
        .ok_or_else(|| SandboxError::new("not_found", "沙盒群不存在"))
}

fn collect_live_users(store: &Store) -> Vec<SandboxUserView> {
    let mut users = HashMap::<String, SandboxUserView>::new();
    for stored in store.conversations.values() {
        for user in stored.users.values() {
            users
                .entry(user.user_id.clone())
                .and_modify(|existing| {
                    if existing.last_seen_unix_ms <= user.last_seen_unix_ms {
                        *existing = user.clone();
                    }
                })
                .or_insert_with(|| user.clone());
        }
    }
    let mut users = users.into_values().collect::<Vec<_>>();
    users.sort_by(|left, right| {
        right
            .last_seen_unix_ms
            .cmp(&left.last_seen_unix_ms)
            .then_with(|| left.user_id.cmp(&right.user_id))
    });
    users
}

fn place_simulate_user(
    inner: &mut Inner,
    user_id: &str,
    display_name: &str,
    avatar_url: Option<&str>,
    now: u64,
) -> Result<(QqConversationRef, SandboxUserView), SandboxError> {
    let taken = group_user_ids(&inner.simulate);
    if taken.iter().any(|id| id == user_id) {
        return Err(SandboxError::new(
            "already_exists",
            format!("用户 `{user_id}` 已在沙盒中"),
        ));
    }
    if taken.len() >= SANDBOX_USER_LIMIT {
        return Err(SandboxError::new(
            "invalid_state",
            "可创建的用户数量已达上限",
        ));
    }
    let account_id = inner.account_id.clone();
    let group_id = group_conversation_id(&inner.simulate)?;
    let stored = conversation_mut(&mut inner.simulate, &group_id)?;
    set_user(stored, user_id, display_name, avatar_url, now);
    let group = stored.view.conversation.clone();
    let user = stored.users[user_id].clone();
    let private = private_ref(&account_id, user_id);
    let private_key = private.origin_key();
    if !inner.simulate.conversations.contains_key(&private_key) {
        insert_conversation(&mut inner.simulate, private, display_name, now)?;
    }
    if let Some(stored) = inner.simulate.conversations.get_mut(&private_key) {
        stored.view.title = display_name.to_owned();
        set_user(stored, user_id, display_name, avatar_url, now);
    }
    Ok((group, user))
}

fn relocate_simulate_user(
    inner: &mut Inner,
    old_id: &str,
    new_id: &str,
    display_name: &str,
    now: u64,
) -> Result<SandboxUserView, SandboxError> {
    let account_id = inner.account_id.clone();
    let group_id = group_conversation_id(&inner.simulate)?;
    let stored = conversation_mut(&mut inner.simulate, &group_id)?;
    let mut user = stored
        .users
        .remove(old_id)
        .ok_or_else(|| SandboxError::new("not_found", format!("用户 `{old_id}` 不在当前会话")))?;
    user.user_id = new_id.to_owned();
    display_name.clone_into(&mut user.display_name);
    user.last_seen_unix_ms = now;
    rewrite_sender(&mut stored.messages, old_id, new_id, display_name);
    stored.users.insert(new_id.to_owned(), user.clone());
    let old_private = private_ref(&account_id, old_id).origin_key();
    let conversation = private_ref(&account_id, new_id);
    let new_private = conversation.origin_key();
    if let Some(mut stored) = inner.simulate.conversations.remove(&old_private) {
        stored.view.conversation = conversation;
        stored.view.conversation_id = new_private.clone();
        stored.view.title = display_name.to_owned();
        stored.users.remove(old_id);
        set_user(
            &mut stored,
            new_id,
            display_name,
            user.avatar_url.as_deref(),
            now,
        );
        rewrite_sender(&mut stored.messages, old_id, new_id, display_name);
        for message in &mut stored.messages {
            message.conversation_id.clone_from(&new_private);
        }
        inner.simulate.conversations.insert(new_private, stored);
    }
    Ok(user)
}

fn rewrite_sender(
    messages: &mut [SandboxMessageView],
    old_id: &str,
    new_id: &str,
    display_name: &str,
) {
    for message in messages {
        if message.sender_id == old_id {
            message.sender_id = new_id.to_owned();
            message.sender_name = display_name.to_owned();
        }
    }
}

fn set_user(
    stored: &mut StoredConversation,
    user_id: &str,
    display_name: &str,
    avatar_url: Option<&str>,
    now: u64,
) {
    let user = stored
        .users
        .entry(user_id.to_owned())
        .or_insert_with(|| SandboxUserView {
            user_id: user_id.to_owned(),
            display_name: display_name.to_owned(),
            avatar_url: None,
            last_seen_unix_ms: now,
            message_count: 0,
        });
    user.user_id = user_id.to_owned();
    display_name.clone_into(&mut user.display_name);
    assign_avatar(&mut user.avatar_url, avatar_url);
    user.last_seen_unix_ms = now;
    stored.view.last_activity_unix_ms = stored.view.last_activity_unix_ms.max(now);
}

fn upsert_user(
    stored: &mut StoredConversation,
    user_id: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
    now: u64,
) {
    let display_name = display_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(user_id);
    let user = stored
        .users
        .entry(user_id.to_owned())
        .or_insert_with(|| SandboxUserView {
            user_id: user_id.to_owned(),
            display_name: display_name.to_owned(),
            avatar_url: None,
            last_seen_unix_ms: now,
            message_count: 0,
        });
    if user.display_name == user.user_id && display_name != user_id {
        display_name.clone_into(&mut user.display_name);
    }
    assign_avatar(&mut user.avatar_url, avatar_url);
    user.last_seen_unix_ms = now;
    stored.view.last_activity_unix_ms = stored.view.last_activity_unix_ms.max(now);
}

fn assign_avatar(target: &mut Option<String>, avatar_url: Option<&str>) {
    if let Some(avatar_url) = avatar_url.map(str::trim).filter(|value| !value.is_empty()) {
        *target = Some(avatar_url.to_owned());
    }
}

fn bot_speaker(inner: &Inner) -> (String, String) {
    match &inner.bot {
        Some(bot) => (
            bot.user_id.clone(),
            bot.display_name
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "机器人".into()),
        ),
        None => ("bot".into(), "机器人".into()),
    }
}

fn append_message(
    stored: &mut StoredConversation,
    assets: &mut HashMap<String, SandboxAsset>,
    faces: &mut HashMap<String, SandboxFace>,
    sender_id: &str,
    sender_name: &str,
    role: SandboxSpeakerRole,
    segments: Vec<MessageSegment>,
    reply_to: Option<String>,
    message_id: Option<String>,
    time_ms: Option<i64>,
) -> SandboxMessageView {
    if let Some(message_id) = message_id.as_deref()
        && let Some(existing) = stored
            .messages
            .iter()
            .find(|item| item.message_id == message_id)
    {
        return existing.clone();
    }
    let now = time_ms
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(unix_ms);
    if role == SandboxSpeakerRole::User {
        upsert_user(stored, sender_id, Some(sender_name), None, now);
        if let Some(user) = stored.users.get_mut(sender_id) {
            user.message_count = user.message_count.saturating_add(1);
        }
    }
    let users = stored.users.values().cloned().collect::<Vec<_>>();
    let (text, refs) = normalize_segments(&segments, &users, assets, now);
    record_faces(faces, &refs, now);
    let preview = preview_content(&text, &refs);
    let message = SandboxMessageView {
        message_id: message_id.unwrap_or_else(|| format!("msg-{}", Uuid::new_v4())),
        conversation_id: stored.view.conversation_id.clone(),
        sender_id: sender_id.into(),
        sender_name: sender_name.into(),
        role,
        text,
        refs,
        reply_to,
        time_ms: i64::try_from(now).unwrap_or(i64::MAX),
    };
    stored.messages.push(message.clone());
    if stored.messages.len() > MAX_MESSAGES {
        stored
            .messages
            .drain(0..stored.messages.len() - MAX_MESSAGES);
    }
    stored.view.last_preview = Some(preview);
    stored.view.last_activity_unix_ms = now;
    stored.view.message_count = stored.messages.len() as u64;
    message
}

fn sandbox_event(
    account_id: &str,
    conversation: &QqConversationRef,
    user: &SandboxUserView,
    kind: BotEventKind,
    message: Option<&SandboxMessageView>,
    time_ms: i64,
) -> Result<BotEvent, SandboxError> {
    let target = conversation
        .target()
        .ok_or_else(|| SandboxError::new("invalid_argument", "会话目标无效"))?;
    let actor = BotUser {
        user_id: user.user_id.clone(),
        display_name: Some(user.display_name.clone()),
        avatar_url: user.avatar_url.clone(),
    };
    let mut ext = BotExtMap::new();
    ext.insert("sandbox".into(), Value::Bool(true));
    Ok(BotEvent {
        event_id: message.map_or_else(
            || format!("evt-{}", Uuid::new_v4()),
            |item| item.message_id.clone(),
        ),
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            account_id: account_id.into(),
            platform: BotPlatform::QqBot,
        },
        kind,
        time_ms,
        target: target.clone(),
        actor: Some(actor.clone()),
        message: message.map(|item| BotMessage {
            message_id: Some(item.message_id.clone()),
            target,
            sender: Some(actor),
            segments: hydrate_segments(&item.text, &item.refs, item.reply_to.as_deref()),
            reply_to: item.reply_to.clone(),
            time_ms: Some(item.time_ms),
            ext: BotExtMap::new(),
        }),
        raw: None,
        ext,
    })
}

fn active_message_permission(event: &BotEvent) -> Option<bool> {
    let event_type = event
        .ext
        .get("qqbot.event_type")
        .and_then(Value::as_str)
        .or_else(|| match &event.kind {
            BotEventKind::PlatformSpecific(kind) => Some(kind.as_str()),
            _ => None,
        })?;
    match event_type {
        "GROUP_MSG_RECEIVE" | "C2C_MSG_RECEIVE" => Some(true),
        "GROUP_MSG_REJECT" | "C2C_MSG_REJECT" | "GROUP_DEL_ROBOT" | "FRIEND_DEL" => Some(false),
        _ => None,
    }
}

fn projected_conversation(stored: &StoredConversation) -> SandboxConversationView {
    let mut view = stored.view.clone();
    let mut users = stored.users.values().cloned().collect::<Vec<_>>();
    users.sort_by(|left, right| {
        right
            .last_seen_unix_ms
            .cmp(&left.last_seen_unix_ms)
            .then_with(|| left.user_id.cmp(&right.user_id))
    });
    if view.avatar_url.is_none() && view.kind == BotConversationKind::Private {
        view.avatar_url = users
            .iter()
            .find(|user| view.conversation.user_id.as_deref() == Some(user.user_id.as_str()))
            .or(users.first())
            .and_then(|user| user.avatar_url.clone());
    }
    if view.kind == BotConversationKind::Group
        && view
            .conversation
            .group_id
            .as_deref()
            .is_some_and(|group_id| view.title == group_id)
    {
        view.title = LIVE_GROUP_TITLE_FALLBACK.into();
    }
    view.users = users;
    view.message_count = stored.messages.len() as u64;
    view
}

fn active_store(inner: &Inner) -> &Store {
    match inner.mode {
        SandboxMode::Simulate => &inner.simulate,
        SandboxMode::Live => &inner.live,
    }
}

fn conversation_mut<'a>(
    store: &'a mut Store,
    conversation_id: &str,
) -> Result<&'a mut StoredConversation, SandboxError> {
    store
        .conversations
        .get_mut(conversation_id)
        .ok_or_else(|| conversation_missing(conversation_id))
}

fn conversation_missing(conversation_id: &str) -> SandboxError {
    SandboxError::new("not_found", format!("会话 `{conversation_id}` 不存在"))
}

fn group_ref(account_id: &str) -> QqConversationRef {
    QqConversationRef {
        version: QQ_CONVERSATION_REF_VERSION,
        account_id: account_id.into(),
        kind: BotConversationKind::Group,
        user_id: None,
        group_id: Some(SANDBOX_GROUP_ID.into()),
        guild_id: None,
        channel_id: None,
        thread_id: None,
    }
}

fn private_ref(account_id: &str, user_id: &str) -> QqConversationRef {
    QqConversationRef {
        version: QQ_CONVERSATION_REF_VERSION,
        account_id: account_id.into(),
        kind: BotConversationKind::Private,
        user_id: Some(user_id.into()),
        group_id: None,
        guild_id: None,
        channel_id: None,
        thread_id: None,
    }
}

fn live_title(conversation: &QqConversationRef) -> String {
    match conversation.kind {
        BotConversationKind::Private => conversation
            .user_id
            .clone()
            .unwrap_or_else(|| "私聊".into()),
        BotConversationKind::Group => LIVE_GROUP_TITLE_FALLBACK.into(),
        BotConversationKind::Channel => conversation
            .channel_id
            .clone()
            .unwrap_or_else(|| "频道".into()),
    }
}

fn sandbox_title(conversation: &QqConversationRef) -> String {
    match conversation.kind {
        BotConversationKind::Group => "沙盒体验群".into(),
        BotConversationKind::Private => {
            let Some(name) = conversation
                .user_id
                .as_deref()
                .and_then(|user_id| user_id.strip_prefix(SANDBOX_ID_PREFIX))
                .filter(|name| !name.is_empty())
            else {
                return "私聊".into();
            };
            let mut chars = name.chars();
            chars.next().map_or_else(
                || "私聊".into(),
                |first| format!("{}{}", first.to_uppercase(), chars.as_str()),
            )
        }
        BotConversationKind::Channel => "沙盒频道".into(),
    }
}

fn assign_live_group_title(stored: &mut StoredConversation, title: &str) -> bool {
    if !is_replaceable_live_group_title(
        &stored.view.title,
        stored.view.conversation.group_id.as_deref(),
    ) || stored.view.title == title
    {
        return false;
    }
    stored.view.title = title.to_owned();
    true
}

fn is_replaceable_live_group_title(title: &str, group_id: Option<&str>) -> bool {
    title.is_empty()
        || title == LIVE_GROUP_TITLE_FALLBACK
        || group_id.is_some_and(|group_id| title == group_id)
}

fn require_revision(inner: &Inner, expected: u64) -> Result<(), SandboxError> {
    if inner.revision == expected {
        Ok(())
    } else {
        Err(SandboxError::new(
            "revision.conflict",
            format!("expected revision {expected}, actual {}", inner.revision),
        ))
    }
}

fn finish(inner: &mut Inner) -> (u64, SandboxMode) {
    gc_inner(inner);
    inner.revision = inner.revision.saturating_add(1);
    (inner.revision, inner.mode)
}

fn gc_inner(inner: &mut Inner) {
    let mut referenced = std::collections::HashSet::new();
    for store in [&inner.simulate, &inner.live] {
        for stored in store.conversations.values() {
            for message in &stored.messages {
                for item in &message.refs {
                    if item.kind.is_media()
                        && let Some(hash) = &item.h
                    {
                        referenced.insert(hash.clone());
                    }
                }
            }
        }
    }
    gc_assets(&mut inner.media, &referenced);
    gc_stickers(&mut inner.stickers);
}

fn write_ok(revision: u64, result: Value) -> SandboxWriteResult {
    SandboxWriteResult { revision, result }
}

fn by_hash<T>(items: Vec<T>, key: impl Fn(&T) -> String) -> HashMap<String, T> {
    items.into_iter().map(|item| (key(&item), item)).collect()
}

fn media_ref(media_id: String, mime: &str, name: &str) -> SandboxMediaRef {
    SandboxMediaRef {
        media_id,
        mime: mime.to_owned(),
        name: name.to_owned(),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn next_created_at(inner: &Inner) -> u64 {
    let latest = inner
        .media
        .values()
        .map(|item| item.created_at_unix_ms)
        .chain(inner.stickers.values().map(|item| item.created_at_unix_ms))
        .max()
        .unwrap_or(0);
    unix_ms().max(latest.saturating_add(1))
}
