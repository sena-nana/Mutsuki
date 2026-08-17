use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_conversation::qq_conversation_from_event;
use mutsuki_bot_protocol::{
    BotAccountRef, BotConversationKind, BotEvent, BotEventKind, BotExtMap, BotMessage, BotPlatform,
    BotUser, QQ_CONVERSATION_REF_VERSION, QqConversationRef,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::types::{
    DEFAULT_SANDBOX_ACCOUNT_ID, SandboxAction, SandboxChangeEvent, SandboxConversationView,
    SandboxError, SandboxMessageView, SandboxMode, SandboxSnapshot, SandboxSpeakerRole,
    SandboxUserView, SandboxWriteRequest, SandboxWriteResult,
};

const MAX_MESSAGES: usize = 200;

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

    /// Submits a simulated inbound `BotEvent` through `mutsuki.bot.flow/ingress@1`.
    ///
    /// # Errors
    ///
    /// Returns a typed sandbox error when the runtime rejects the task.
    async fn ingest(&self, event: BotEvent) -> Result<(), SandboxError>;

    /// Sends a bot message through the owner delivery path.
    ///
    /// # Errors
    ///
    /// Returns a typed sandbox error when live delivery is unavailable or rejected.
    async fn deliver(
        &self,
        operation_id: &str,
        conversation: &QqConversationRef,
        text: &str,
    ) -> Result<Value, SandboxError>;
}

#[async_trait]
pub trait SandboxApi: Send + Sync {
    fn subscribe_changes(&self) -> Option<SandboxChangeSubscription> {
        None
    }

    /// Loads the current sandbox projection for the active mode.
    ///
    /// # Errors
    ///
    /// Returns a typed sandbox error when the snapshot cannot be materialized.
    async fn snapshot(&self, query: &str) -> Result<SandboxSnapshot, SandboxError>;

    /// Applies one sandbox write against the current revision fence.
    ///
    /// # Errors
    ///
    /// Returns a typed sandbox error when the action is rejected.
    async fn write(
        &self,
        actor_id: &str,
        request: SandboxWriteRequest,
    ) -> Result<SandboxWriteResult, SandboxError>;

    /// Lists messages for one conversation.
    ///
    /// # Errors
    ///
    /// Returns a typed sandbox error when the conversation is missing.
    async fn messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<SandboxMessageView>, SandboxError>;

    fn observe_event(&self, event: BotEvent);
}

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
}

/// In-memory QQ conversation sandbox used by the Web Console.
pub struct SandboxService {
    write_lock: tokio::sync::Mutex<()>,
    inner: Mutex<Inner>,
    runtime: Mutex<Option<Arc<dyn SandboxRuntime>>>,
    changes: broadcast::Sender<SandboxChangeEvent>,
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
        let account_id = account_id.into();
        let (changes, _) = broadcast::channel(64);
        let mut simulate = Store::default();
        seed_simulate(&mut simulate, &account_id);
        Self {
            write_lock: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner {
                revision: 0,
                mode: SandboxMode::Simulate,
                account_id,
                simulate,
                live: Store::default(),
            }),
            runtime: Mutex::new(None),
            changes,
        }
    }

    /// Installs the live ingest/delivery runtime used by real-data mode.
    ///
    /// # Panics
    ///
    /// Panics if the runtime mutex is poisoned.
    pub fn set_runtime(&self, runtime: Arc<dyn SandboxRuntime>) {
        *self.runtime.lock().expect("sandbox runtime mutex") = Some(runtime);
    }

    fn runtime(&self) -> Option<Arc<dyn SandboxRuntime>> {
        self.runtime.lock().expect("sandbox runtime mutex").clone()
    }

    fn live_available(&self) -> bool {
        self.runtime()
            .is_some_and(|runtime| runtime.live_available())
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("sandbox state mutex")
    }

    fn publish(&self, revision: u64, mode: SandboxMode) {
        let _ = self.changes.send(SandboxChangeEvent { revision, mode });
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
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
            live_available: self.live_available(),
            account_id: inner.account_id.clone(),
            conversations,
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
                bump(&mut inner);
                let result = write_ok(inner.revision, json!({ "mode": mode }));
                drop(inner);
                self.publish(result.revision, mode);
                Ok(result)
            }
            SandboxAction::IngestAsUser {
                conversation_id,
                user_id,
                text,
                inject_into_flow,
            } => {
                let (event, revision, mode, message) = {
                    let mut inner = self.lock_inner();
                    require_revision(&inner, request.expected_revision)?;
                    if inner.mode != SandboxMode::Simulate {
                        return Err(SandboxError::new(
                            "invalid_state",
                            "真实模式不能伪造群成员发言",
                        ));
                    }
                    let text = require_text(&text)?;
                    let account_id = inner.account_id.clone();
                    let stored = conversation_mut(&mut inner.simulate, &conversation_id)?;
                    let user = stored.users.get(&user_id).cloned().ok_or_else(|| {
                        SandboxError::new("not_found", format!("用户 `{user_id}` 不在当前会话"))
                    })?;
                    let conversation = stored.view.conversation.clone();
                    let message = append_message(
                        stored,
                        &user.user_id,
                        &user.display_name,
                        SandboxSpeakerRole::User,
                        &text,
                    );
                    let event = simulated_event(&account_id, &conversation, &user, &message)?;
                    bump(&mut inner);
                    (event, inner.revision, inner.mode, message)
                };
                if inject_into_flow {
                    let runtime = self.runtime().ok_or_else(|| {
                        SandboxError::new("runtime.unavailable", "Bot 流程当前不可用")
                    })?;
                    runtime.ingest(event).await?;
                }
                self.publish(revision, mode);
                Ok(write_ok(
                    revision,
                    serde_json::to_value(message)
                        .map_err(|error| SandboxError::new("encode_failed", error.to_string()))?,
                ))
            }
            SandboxAction::SendAsBot {
                conversation_id,
                text,
            } => {
                let (mode, conversation, text) = {
                    let inner = self.lock_inner();
                    require_revision(&inner, request.expected_revision)?;
                    let text = require_text(&text)?;
                    let stored = active_store(&inner)
                        .conversations
                        .get(&conversation_id)
                        .ok_or_else(|| conversation_missing(&conversation_id))?;
                    (inner.mode, stored.view.conversation.clone(), text)
                };
                let delivery = if mode == SandboxMode::Live {
                    let runtime = self.runtime().ok_or_else(|| {
                        SandboxError::new("qq.owner_unavailable", "尚未连接 QQ，无法发送真实消息")
                    })?;
                    if !runtime.live_available() {
                        return Err(SandboxError::new(
                            "qq.owner_unavailable",
                            "尚未连接 QQ，无法发送真实消息",
                        ));
                    }
                    Some(
                        runtime
                            .deliver(&request.operation_id, &conversation, &text)
                            .await?,
                    )
                } else {
                    None
                };
                let recorded = {
                    let mut inner = self.lock_inner();
                    let stored = conversation_mut(active_store_mut(&mut inner), &conversation_id)?;
                    let message =
                        append_message(stored, "bot", "机器人", SandboxSpeakerRole::Bot, &text);
                    bump(&mut inner);
                    (inner.revision, inner.mode, message)
                };
                self.publish(recorded.0, recorded.1);
                Ok(write_ok(
                    recorded.0,
                    json!({ "message": recorded.2, "delivery": delivery }),
                ))
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
        let Ok(conversation) = qq_conversation_from_event(&event) else {
            return;
        };
        if matches!(
            event.kind,
            BotEventKind::BotConnected | BotEventKind::BotDisconnected
        ) {
            return;
        }
        let now = u64::try_from(event.time_ms.max(0)).unwrap_or(unix_ms());
        let mut inner = self.lock_inner();
        let title = live_title(&conversation);
        let key = conversation.origin_key();
        if !inner.live.conversations.contains_key(&key) {
            let _ = insert_conversation(&mut inner.live, conversation, &title, now);
        }
        let Some(stored) = inner.live.conversations.get_mut(&key) else {
            return;
        };
        if let Some(actor) = &event.actor {
            upsert_user(stored, &actor.user_id, actor.display_name.as_deref(), now);
        }
        if let Some(text) = event
            .message
            .as_ref()
            .map(BotMessage::plain_text)
            .filter(|text| !text.trim().is_empty())
        {
            let sender = event.actor.as_ref();
            append_message(
                stored,
                sender.map_or("unknown", |user| user.user_id.as_str()),
                sender
                    .and_then(|user| user.display_name.as_deref())
                    .unwrap_or(sender.map_or("unknown", |user| user.user_id.as_str())),
                SandboxSpeakerRole::User,
                &text,
            );
        }
        bump(&mut inner);
        let revision = inner.revision;
        let mode = inner.mode;
        drop(inner);
        self.publish(revision, mode);
    }
}

fn seed_simulate(store: &mut Store, account_id: &str) {
    let now = unix_ms();
    let group = insert_conversation(
        store,
        QqConversationRef {
            version: QQ_CONVERSATION_REF_VERSION,
            account_id: account_id.into(),
            kind: BotConversationKind::Group,
            user_id: None,
            group_id: Some("sandbox-group".into()),
            guild_id: None,
            channel_id: None,
            thread_id: None,
        },
        "沙盒体验群",
        now,
    )
    .expect("seed group");
    if let Some(stored) = store.conversations.get_mut(&group.conversation_id) {
        upsert_user(stored, "alice", Some("Alice"), now);
        upsert_user(stored, "bob", Some("Bob"), now);
        append_message(
            stored,
            "system",
            "系统",
            SandboxSpeakerRole::System,
            "这是模拟 QQ 会话。可切换到真实模式查看已接入群聊里实际发言的用户。",
        );
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
            conversation,
            users: Vec::new(),
            last_preview: None,
            last_activity_unix_ms: now,
            message_count: 0,
        },
        users: HashMap::new(),
        messages: Vec::new(),
    };
    let view = projected_conversation(&stored);
    store.conversations.insert(conversation_id, stored);
    Ok(view)
}

fn require_text(text: &str) -> Result<String, SandboxError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(SandboxError::new("invalid_argument", "消息不能为空"));
    }
    Ok(text.to_owned())
}

fn upsert_user(
    stored: &mut StoredConversation,
    user_id: &str,
    display_name: Option<&str>,
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
            last_seen_unix_ms: now,
            message_count: 0,
        });
    if user.display_name == user.user_id && display_name != user_id {
        display_name.clone_into(&mut user.display_name);
    }
    user.last_seen_unix_ms = now;
    stored.view.last_activity_unix_ms = stored.view.last_activity_unix_ms.max(now);
}

fn append_message(
    stored: &mut StoredConversation,
    sender_id: &str,
    sender_name: &str,
    role: SandboxSpeakerRole,
    text: &str,
) -> SandboxMessageView {
    let now = unix_ms();
    if role == SandboxSpeakerRole::User {
        upsert_user(stored, sender_id, Some(sender_name), now);
        if let Some(user) = stored.users.get_mut(sender_id) {
            user.message_count = user.message_count.saturating_add(1);
        }
    }
    let message = SandboxMessageView {
        message_id: format!("msg-{}", Uuid::new_v4()),
        conversation_id: stored.view.conversation_id.clone(),
        sender_id: sender_id.into(),
        sender_name: sender_name.into(),
        role,
        text: text.to_owned(),
        time_ms: i64::try_from(now).unwrap_or(i64::MAX),
    };
    stored.messages.push(message.clone());
    if stored.messages.len() > MAX_MESSAGES {
        stored
            .messages
            .drain(0..stored.messages.len() - MAX_MESSAGES);
    }
    stored.view.last_preview = Some(text.to_owned());
    stored.view.last_activity_unix_ms = now;
    stored.view.message_count = stored.messages.len() as u64;
    message
}

fn simulated_event(
    account_id: &str,
    conversation: &QqConversationRef,
    user: &SandboxUserView,
    message: &SandboxMessageView,
) -> Result<BotEvent, SandboxError> {
    let target = conversation
        .target()
        .ok_or_else(|| SandboxError::new("invalid_argument", "会话目标无效"))?;
    let actor = BotUser {
        user_id: user.user_id.clone(),
        display_name: Some(user.display_name.clone()),
        avatar_url: None,
    };
    let mut ext = BotExtMap::new();
    ext.insert("sandbox".into(), Value::Bool(true));
    let mut body = BotMessage::text(target.clone(), &message.text);
    body.message_id = Some(message.message_id.clone());
    body.sender = Some(actor.clone());
    body.time_ms = Some(message.time_ms);
    Ok(BotEvent {
        event_id: message.message_id.clone(),
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            account_id: account_id.into(),
            platform: BotPlatform::QqBot,
        },
        kind: BotEventKind::MessageCreated,
        time_ms: message.time_ms,
        target,
        actor: Some(actor),
        message: Some(body),
        raw: None,
        ext,
    })
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

fn active_store_mut(inner: &mut Inner) -> &mut Store {
    match inner.mode {
        SandboxMode::Simulate => &mut inner.simulate,
        SandboxMode::Live => &mut inner.live,
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

fn live_title(conversation: &QqConversationRef) -> String {
    match conversation.kind {
        BotConversationKind::Private => conversation
            .user_id
            .clone()
            .unwrap_or_else(|| "私聊".into()),
        BotConversationKind::Group => conversation
            .group_id
            .clone()
            .unwrap_or_else(|| "群聊".into()),
        BotConversationKind::Channel => conversation
            .channel_id
            .clone()
            .unwrap_or_else(|| "频道".into()),
    }
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

fn bump(inner: &mut Inner) {
    inner.revision = inner.revision.saturating_add(1);
}

fn write_ok(revision: u64, result: Value) -> SandboxWriteResult {
    SandboxWriteResult { revision, result }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
