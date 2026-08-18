use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_conversation::qq_conversation_from_event;
use mutsuki_bot_protocol::{
    BotAccountRef, BotConversationKind, BotEvent, BotEventKind, BotExtMap, BotMessage, BotPlatform,
    BotUser, MessageSegment, QQ_CONVERSATION_REF_VERSION, QqConversationRef,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::types::{
    DEFAULT_SANDBOX_ACCOUNT_ID, SANDBOX_GROUP_ID, SANDBOX_ID_PREFIX, SANDBOX_USER_LIMIT,
    SANDBOX_USER_NAMES, SandboxAction, SandboxChangeEvent, SandboxConversationView, SandboxError,
    SandboxMessageView, SandboxMode, SandboxSnapshot, SandboxSpeakerRole, SandboxUserView,
    SandboxWriteRequest, SandboxWriteResult, is_sandbox_conversation, preview_segments,
    sandbox_user_id,
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
    async fn ingest(&self, event: BotEvent) -> Result<(), SandboxError>;
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

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("sandbox state mutex")
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
        reply_to: Option<String>,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let runtime = self.require_flow()?;
        let (event, revision, mode, message) = {
            let mut inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            require_simulate(&inner, "真实模式不能伪造群成员发言")?;
            let text = require_text(&text)?;
            let account_id = inner.account_id.clone();
            let stored = conversation_mut(&mut inner.simulate, &conversation_id)?;
            let user = stored.users.get(&user_id).cloned().ok_or_else(|| {
                SandboxError::new("not_found", format!("用户 `{user_id}` 不在当前会话"))
            })?;
            if let Some(reply_to) = reply_to.as_deref() {
                require_message(stored, reply_to)?;
            }
            let conversation = stored.view.conversation.clone();
            let mut segments = Vec::new();
            if let Some(reply_to) = reply_to.as_deref() {
                segments.push(MessageSegment::Quote {
                    message_id: reply_to.to_owned(),
                });
            }
            segments.push(MessageSegment::text(&text));
            let message = append_message(
                stored,
                &user.user_id,
                &user.display_name,
                SandboxSpeakerRole::User,
                segments,
                reply_to,
                None,
            );
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
            let (group, user) = place_simulate_user(&mut inner, &user_id, display_name, now)?;
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
            inner
                .simulate
                .conversations
                .remove(&private_ref(&account_id, &user_id).origin_key());
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
            require_simulate(&inner, "真实模式不能修改虚拟用户")?;
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
                match place_simulate_user(&mut inner, &user_id, &source.display_name, now) {
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
        self.publish(revision, mode);
        Ok(write_ok(revision, json!({ "cleared": true })))
    }

    async fn send_as_bot(
        &self,
        operation_id: &str,
        expected_revision: u64,
        conversation_id: String,
        text: String,
    ) -> Result<SandboxWriteResult, SandboxError> {
        let (conversation, text) = {
            let inner = self.lock_inner();
            require_revision(&inner, expected_revision)?;
            if inner.mode != SandboxMode::Live {
                return Err(SandboxError::new(
                    "invalid_state",
                    "模拟模式不能以后台机器人身份发送",
                ));
            }
            let text = require_text(&text)?;
            let stored = inner
                .live
                .conversations
                .get(&conversation_id)
                .ok_or_else(|| conversation_missing(&conversation_id))?;
            (stored.view.conversation.clone(), text)
        };
        let runtime = self.runtime().ok_or_else(|| {
            SandboxError::new("qq.owner_unavailable", "尚未连接 QQ，无法发送真实消息")
        })?;
        if !runtime.live_available() {
            return Err(SandboxError::new(
                "qq.owner_unavailable",
                "尚未连接 QQ，无法发送真实消息",
            ));
        }
        let delivery = runtime.deliver(operation_id, &conversation, &text).await?;
        let recorded = {
            let mut inner = self.lock_inner();
            let stored = conversation_mut(&mut inner.live, &conversation_id)?;
            let message = append_message(
                stored,
                "bot",
                "机器人",
                SandboxSpeakerRole::Bot,
                vec![MessageSegment::text(&text)],
                None,
                None,
            );
            let (revision, mode) = finish(&mut inner);
            (revision, mode, message)
        };
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
                reply_to,
            } => {
                self.ingest_as_user(
                    request.expected_revision,
                    conversation_id,
                    user_id,
                    text,
                    reply_to,
                )
                .await
            }
            SandboxAction::SendAsBot {
                conversation_id,
                text,
            } => {
                self.send_as_bot(
                    &request.operation_id,
                    request.expected_revision,
                    conversation_id,
                    text,
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
        let Ok(conversation) = qq_conversation_from_event(&event) else {
            return;
        };
        if is_sandbox_conversation(&conversation)
            || matches!(
                event.kind,
                BotEventKind::BotConnected | BotEventKind::BotDisconnected
            )
        {
            return;
        }
        let now = u64::try_from(event.time_ms.max(0)).unwrap_or(unix_ms());
        let mut inner = self.lock_inner();
        let account_id = inner.account_id.clone();
        {
            let stored = ensure_conversation(&mut inner.live, conversation, &live_title, now);
            if let Some(actor) = &event.actor {
                upsert_user(stored, &actor.user_id, actor.display_name.as_deref(), now);
            }
            if let Some(message) = &event.message {
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
                    sender_id,
                    sender_name,
                    role,
                    message.segments.clone(),
                    message.reply_to.clone(),
                    message.message_id.clone(),
                );
            }
        }
        let (revision, mode) = finish(&mut inner);
        drop(inner);
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
        let message = {
            let store = if sandbox {
                &mut inner.simulate
            } else {
                &mut inner.live
            };
            let title: fn(&QqConversationRef) -> String =
                if sandbox { sandbox_title } else { live_title };
            let stored = ensure_conversation(store, conversation.clone(), &title, now);
            append_message(
                stored,
                "bot",
                "机器人",
                SandboxSpeakerRole::Bot,
                segments.to_vec(),
                reply_to.map(str::to_owned),
                None,
            )
        };
        let (revision, mode) = finish(&mut inner);
        drop(inner);
        self.publish(revision, mode);
        Some(message)
    }
}

fn seed_simulate(store: &mut Store, account_id: &str) {
    let now = unix_ms();
    let group =
        insert_conversation(store, group_ref(account_id), "沙盒体验群", now).expect("seed group");
    if let Some(stored) = store.conversations.get_mut(&group.conversation_id) {
        for name in ["Alice", "Bob"] {
            upsert_user(stored, &sandbox_user_id(name), Some(name), now);
        }
        append_message(
            stored,
            "system",
            "系统",
            SandboxSpeakerRole::System,
            vec![MessageSegment::text(
                "这是虚拟 QQ 会话。以群成员身份发言会进入 Bot 流程，机器人回复会回到这里。",
            )],
            None,
            None,
        );
    }
    for name in ["Alice", "Bob"] {
        let user_id = sandbox_user_id(name);
        let private = insert_conversation(store, private_ref(account_id, &user_id), name, now)
            .expect("seed private");
        if let Some(stored) = store.conversations.get_mut(&private.conversation_id) {
            upsert_user(stored, &user_id, Some(name), now);
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
    set_user(stored, user_id, display_name, now);
    let group = stored.view.conversation.clone();
    let user = stored.users[user_id].clone();
    let private = private_ref(&account_id, user_id);
    let private_key = private.origin_key();
    if !inner.simulate.conversations.contains_key(&private_key) {
        insert_conversation(&mut inner.simulate, private, display_name, now)?;
    }
    if let Some(stored) = inner.simulate.conversations.get_mut(&private_key) {
        stored.view.title = display_name.to_owned();
        set_user(stored, user_id, display_name, now);
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
        set_user(&mut stored, new_id, display_name, now);
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

fn set_user(stored: &mut StoredConversation, user_id: &str, display_name: &str, now: u64) {
    let user = stored
        .users
        .entry(user_id.to_owned())
        .or_insert_with(|| SandboxUserView {
            user_id: user_id.to_owned(),
            display_name: display_name.to_owned(),
            last_seen_unix_ms: now,
            message_count: 0,
        });
    user.user_id = user_id.to_owned();
    display_name.clone_into(&mut user.display_name);
    user.last_seen_unix_ms = now;
    stored.view.last_activity_unix_ms = stored.view.last_activity_unix_ms.max(now);
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
    segments: Vec<MessageSegment>,
    reply_to: Option<String>,
    message_id: Option<String>,
) -> SandboxMessageView {
    let now = unix_ms();
    if role == SandboxSpeakerRole::User {
        upsert_user(stored, sender_id, Some(sender_name), now);
        if let Some(user) = stored.users.get_mut(sender_id) {
            user.message_count = user.message_count.saturating_add(1);
        }
    }
    let text = preview_segments(&segments);
    let message = SandboxMessageView {
        message_id: message_id.unwrap_or_else(|| format!("msg-{}", Uuid::new_v4())),
        conversation_id: stored.view.conversation_id.clone(),
        sender_id: sender_id.into(),
        sender_name: sender_name.into(),
        role,
        text,
        segments,
        reply_to,
        time_ms: i64::try_from(now).unwrap_or(i64::MAX),
    };
    stored.messages.push(message.clone());
    if stored.messages.len() > MAX_MESSAGES {
        stored
            .messages
            .drain(0..stored.messages.len() - MAX_MESSAGES);
    }
    stored.view.last_preview = Some(message.text.clone());
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
        avatar_url: None,
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
            segments: item.segments.clone(),
            reply_to: item.reply_to.clone(),
            time_ms: Some(item.time_ms),
            ext: BotExtMap::new(),
        }),
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
    inner.revision = inner.revision.saturating_add(1);
    (inner.revision, inner.mode)
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
