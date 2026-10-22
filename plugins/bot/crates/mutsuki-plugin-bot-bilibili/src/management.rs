//! Structured Bilibili account/subscription management shared by chat commands and Web Console.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_management::{
    BilibiliBindChallengeResult, BilibiliBindVerifyResult, BilibiliCredentialSecretState,
    BilibiliLoginPollResult, BilibiliLoginSession, BilibiliLoginStartResult, BilibiliManagementApi,
    BilibiliManagementChangeArea, BilibiliManagementChangeEvent,
    BilibiliManagementChangeSubscription, BilibiliManagementError, BilibiliManagementStatus,
    BilibiliNotificationKind, BilibiliPreviewCardView, BilibiliQrLoginStatus,
    BilibiliSubscriptionView,
};
use mutsuki_bot_protocol::{BOT_EVENT_INGEST_PROTOCOL_ID, BotTarget};

use crate::commands::{binding_code, select_subscription, self_subscription_id_for};
use crate::{
    BILIBILI_EVENT_TYPE, BilibiliBackendConfig, BilibiliConfig, BilibiliConfigStore,
    BilibiliCredentialStore, BilibiliError, BilibiliHttp, BilibiliPollKind, BilibiliQrStatus,
    BilibiliSubscription, BilibiliTransport, SharedBilibiliConfig, SqliteBilibiliRepository,
};

#[async_trait]
pub trait BilibiliQrRenderer: Send + Sync {
    async fn render_qr(&self, content: &str) -> Result<Vec<u8>, BilibiliError>;
}

pub trait BilibiliSecretPresence: Send + Sync {
    fn inspect(&self, key: &str) -> BilibiliCredentialSecretState;
}

/// Account and subscription management. Upstream calls go through the HTTP effect: the
/// runner passes its own child-task channel, while the Web Console path uses the channel
/// bound with [`Self::bind_http`] once the runtime client exists.
pub struct BilibiliManagementService {
    config: SharedBilibiliConfig,
    transport: Arc<dyn BilibiliTransport>,
    http: RwLock<Option<Arc<dyn BilibiliHttp>>>,
    repository: Arc<SqliteBilibiliRepository>,
    credential_store: Arc<dyn BilibiliCredentialStore>,
    config_store: Arc<dyn BilibiliConfigStore>,
    secret_presence: Arc<dyn BilibiliSecretPresence>,
    qr_renderer: RwLock<Option<Arc<dyn BilibiliQrRenderer>>>,
    change_revision: AtomicU64,
    changes: tokio::sync::broadcast::Sender<BilibiliManagementChangeEvent>,
    flow_registry: Option<Arc<BotFlowRegistry>>,
}

impl BilibiliManagementService {
    pub fn new(
        config: SharedBilibiliConfig,
        transport: Arc<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        credential_store: Arc<dyn BilibiliCredentialStore>,
        config_store: Arc<dyn BilibiliConfigStore>,
        secret_presence: Arc<dyn BilibiliSecretPresence>,
    ) -> Self {
        let (changes, _) = tokio::sync::broadcast::channel(64);
        Self {
            config,
            transport,
            http: RwLock::new(None),
            repository,
            credential_store,
            config_store,
            secret_presence,
            qr_renderer: RwLock::new(None),
            change_revision: AtomicU64::new(0),
            changes,
            flow_registry: None,
        }
    }

    fn publish_change(&self, areas: Vec<BilibiliManagementChangeArea>) {
        let event = BilibiliManagementChangeEvent {
            revision: self
                .change_revision
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1),
            areas,
        };
        let _ = self.changes.send(event);
    }

    pub fn bind_qr_renderer(&self, renderer: Arc<dyn BilibiliQrRenderer>) {
        *self.qr_renderer.write().expect("QR renderer lock") = Some(renderer);
    }

    pub fn bind_http(&self, http: Arc<dyn BilibiliHttp>) {
        *self.http.write().expect("Bilibili HTTP lock") = Some(http);
    }

    fn bound_http(&self) -> Result<Arc<dyn BilibiliHttp>, BilibiliManagementError> {
        self.http
            .read()
            .expect("Bilibili HTTP lock")
            .clone()
            .ok_or_else(|| {
                map_error(BilibiliError::ManagementUnavailable(
                    "the runtime HTTP client is not bound yet".into(),
                ))
            })
    }

    /// Shares the Flow registry so `status()` can report whether the push
    /// Source chain is wired into the active graph.
    pub fn with_flow_registry(mut self, registry: Arc<BotFlowRegistry>) -> Self {
        self.flow_registry = Some(registry);
        self
    }

    fn status_impl(&self) -> BilibiliManagementStatus {
        let snapshot = self.config.snapshot();
        let backend = "web_cookie".into();
        let (cookie_secret_key, cookie_secret_state): (_, Option<BilibiliCredentialSecretState>) =
            match &snapshot.backend {
                BilibiliBackendConfig::WebCookie { cookie_secret_key } => (
                    Some(cookie_secret_key.clone()),
                    Some(self.secret_presence.inspect(cookie_secret_key)),
                ),
            };
        let management_enabled = snapshot.management.enabled;
        let reason = if !management_enabled {
            Some("subscription management is disabled".into())
        } else {
            None
        };
        BilibiliManagementStatus {
            backend,
            management_enabled,
            allow_self_binding: snapshot.management.allow_self_binding,
            cookie_secret_key,
            cookie_secret_state,
            credential_loaded: cookie_secret_state == Some(BilibiliCredentialSecretState::Present),
            subscription_count: snapshot.subscriptions.len(),
            reason,
            push_wired: self.flow_registry.as_ref().map(|registry| {
                registry.source_wired(BOT_EVENT_INGEST_PROTOCOL_ID, Some((BILIBILI_EVENT_TYPE, 1)))
            }),
        }
    }

    async fn login_start_impl(
        &self,
        http: &dyn BilibiliHttp,
        actor_id: &str,
    ) -> Result<BilibiliLoginStartResult, BilibiliError> {
        let renderer = self
            .qr_renderer
            .read()
            .expect("QR renderer lock")
            .clone()
            .ok_or_else(|| {
                BilibiliError::ManagementUnavailable("image QR renderer is unavailable".into())
            })?;
        let session = self.login_start_session_impl(http, actor_id).await?;
        let png = renderer.render_qr(&session.url).await?;
        Ok(BilibiliLoginStartResult {
            url: session.url,
            key: session.key,
            qr_png_base64: base64_encode(&png),
            qr_png: png,
        })
    }

    async fn login_start_session_impl(
        &self,
        http: &dyn BilibiliHttp,
        actor_id: &str,
    ) -> Result<BilibiliLoginSession, BilibiliError> {
        let qr = self.transport.qr_start(http).await?;
        self.repository
            .set_qr_session(actor_id, &qr.key)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        Ok(BilibiliLoginSession {
            url: qr.url,
            key: qr.key,
        })
    }

    async fn login_poll_impl(
        &self,
        http: &dyn BilibiliHttp,
        actor_id: &str,
    ) -> Result<BilibiliLoginPollResult, BilibiliError> {
        let key = self
            .repository
            .qr_session(actor_id)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .ok_or_else(|| {
                BilibiliError::ManagementUnavailable("no active QR login; run login first".into())
            })?;
        let status = self.transport.qr_poll(http, &key).await?;
        let message = match status {
            BilibiliQrStatus::Pending => "等待扫码。".into(),
            BilibiliQrStatus::Scanned => "已扫码，等待在 App 中确认。".into(),
            BilibiliQrStatus::Expired => {
                self.repository
                    .clear_qr_session(actor_id)
                    .map_err(|error| BilibiliError::Transport(error.to_string()))?;
                "二维码已过期，请重新执行 login。".into()
            }
            // The HTTP effect already stored the login cookies in the Host secret.
            BilibiliQrStatus::Confirmed => {
                self.repository
                    .clear_qr_session(actor_id)
                    .map_err(|error| BilibiliError::Transport(error.to_string()))?;
                "登录成功，凭据已通过 Host secret backend 原子轮换。".into()
            }
        };
        Ok(BilibiliLoginPollResult {
            status: status.into(),
            message,
        })
    }

    fn credential_clear_impl(&self) -> Result<(), BilibiliError> {
        let config = self.config.snapshot();
        let cookie_secret_key = config.backend.cookie_secret_key().ok_or_else(|| {
            BilibiliError::ManagementUnavailable("Cookie backend is not selected".into())
        })?;
        self.credential_store
            .rotate(cookie_secret_key, String::new())
            .map_err(BilibiliError::ManagementUnavailable)
    }

    fn list_impl(
        &self,
        actor_id: &str,
        is_admin: bool,
    ) -> Result<Vec<BilibiliSubscriptionView>, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled {
            return Err(BilibiliError::ManagementUnavailable(
                "management is disabled".into(),
            ));
        }
        Ok(config
            .subscriptions
            .into_iter()
            .filter(|subscription| {
                is_admin || subscription.owner_user_id.as_deref() == Some(actor_id)
            })
            .map(BilibiliSubscriptionView::from)
            .collect())
    }

    fn subscribe_impl(
        &self,
        subscription_id: String,
        uid: u64,
        notifications: Vec<BilibiliNotificationKind>,
        target: BotTarget,
        outbound_binding: String,
    ) -> Result<BilibiliSubscriptionView, BilibiliError> {
        self.require_web_management()?;
        if subscription_id.trim().is_empty() || uid == 0 || notifications.is_empty() {
            return Err(BilibiliError::InvalidResponse(
                "subscription requires id, uid and notification types".into(),
            ));
        }
        if outbound_binding.trim().is_empty() {
            return Err(BilibiliError::InvalidResponse(
                "outbound_binding is required".into(),
            ));
        }
        require_deliverable_target(&target)?;
        let mut next = self.config.snapshot();
        if next
            .subscriptions
            .iter()
            .any(|item| item.subscription_id == subscription_id)
        {
            return Err(BilibiliError::InvalidResponse(format!(
                "subscription_id {subscription_id} already exists"
            )));
        }
        let subscription = BilibiliSubscription {
            subscription_id,
            uid,
            notifications: notifications.into_iter().map(Into::into).collect(),
            target,
            outbound_binding,
            paused: false,
            owner_user_id: None,
        };
        next.subscriptions.push(subscription.clone());
        self.persist(next)?;
        Ok(BilibiliSubscriptionView::from(subscription))
    }

    fn unsubscribe_impl(&self, subscription_id: &str) -> Result<(), BilibiliError> {
        self.require_web_management()?;
        let mut next = self.config.snapshot();
        let before = next.subscriptions.len();
        next.subscriptions
            .retain(|subscription| subscription.subscription_id != subscription_id);
        if next.subscriptions.len() == before {
            return Err(BilibiliError::ManagementUnavailable(format!(
                "subscription {subscription_id} was not found"
            )));
        }
        self.persist(next)
    }

    fn set_paused_impl(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
        paused: bool,
    ) -> Result<BilibiliSubscriptionView, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled {
            return Err(BilibiliError::ManagementUnavailable(
                "management is disabled".into(),
            ));
        }
        let mut next = config;
        let index = select_subscription(&next, actor_id, is_admin, selector)?;
        next.subscriptions[index].paused = paused;
        let view = BilibiliSubscriptionView::from(next.subscriptions[index].clone());
        self.persist(next)?;
        Ok(view)
    }

    async fn preview_impl(
        &self,
        http: &dyn BilibiliHttp,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
    ) -> Result<BilibiliPreviewCardView, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled {
            return Err(BilibiliError::ManagementUnavailable(
                "management is disabled".into(),
            ));
        }
        let index = select_subscription(&config, actor_id, is_admin, selector)?;
        let subscription = &config.subscriptions[index];
        let item = self
            .transport
            .poll(http, &BilibiliPollKind::Dynamic, subscription.uid)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| BilibiliError::ManagementUnavailable("该账号暂无可预览动态。".into()))?;
        Ok(BilibiliPreviewCardView {
            title: item.title,
            url: item.url,
            description: "通知预览（不会推进轮询 cursor）".into(),
            image_url: item.image_url,
        })
    }

    async fn bind_start_impl(
        &self,
        http: &dyn BilibiliHttp,
        operator_user_id: &str,
        uid: u64,
        challenge_seed: &str,
    ) -> Result<BilibiliBindChallengeResult, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled || !config.management.allow_self_binding {
            return Err(BilibiliError::Forbidden);
        }
        if uid == 0 {
            return Err(BilibiliError::InvalidResponse(
                "invalid Bilibili UID".into(),
            ));
        }
        let profile = self.transport.profile(http, uid).await?;
        let code = binding_code(operator_user_id, uid, challenge_seed);
        self.repository
            .set_binding_challenge(operator_user_id, uid, &code)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        Ok(BilibiliBindChallengeResult {
            uid,
            name: profile.name,
            code,
        })
    }

    async fn bind_verify_impl(
        &self,
        http: &dyn BilibiliHttp,
        operator_user_id: &str,
        platform: &str,
        target: BotTarget,
    ) -> Result<BilibiliBindVerifyResult, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled || !config.management.allow_self_binding {
            return Err(BilibiliError::Forbidden);
        }
        let (uid, code) = self
            .repository
            .binding_challenge(operator_user_id)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .ok_or_else(|| {
                BilibiliError::ManagementUnavailable("no binding challenge; run bind first".into())
            })?;
        let profile = self.transport.profile(http, uid).await?;
        if !profile.signature.contains(&code) {
            return Ok(BilibiliBindVerifyResult::SignatureMismatch { code });
        }
        require_deliverable_target(&target)?;
        let mut next = config.clone();
        let subscription_id = self_subscription_id_for(platform, operator_user_id);
        next.subscriptions
            .retain(|subscription| subscription.owner_user_id.as_deref() != Some(operator_user_id));
        let subscription = BilibiliSubscription {
            subscription_id,
            uid,
            notifications: next.management.self_binding_notifications.clone(),
            target,
            outbound_binding: next.management.self_binding_outbound_binding.clone(),
            paused: false,
            owner_user_id: Some(operator_user_id.into()),
        };
        next.subscriptions.push(subscription.clone());
        self.persist(next)?;
        self.repository
            .clear_binding_challenge(operator_user_id)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        Ok(BilibiliBindVerifyResult::Verified(
            BilibiliSubscriptionView::from(subscription),
        ))
    }

    fn unbind_impl(&self, operator_user_id: &str) -> Result<bool, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled {
            return Err(BilibiliError::ManagementUnavailable(
                "management is disabled".into(),
            ));
        }
        let mut next = config;
        let before = next.subscriptions.len();
        next.subscriptions
            .retain(|subscription| subscription.owner_user_id.as_deref() != Some(operator_user_id));
        if next.subscriptions.len() == before {
            return Ok(false);
        }
        self.persist(next)?;
        Ok(true)
    }

    fn persist(&self, next: BilibiliConfig) -> Result<(), BilibiliError> {
        next.validate()
            .map_err(BilibiliError::ManagementUnavailable)?;
        self.config_store
            .replace(&next)
            .map_err(BilibiliError::ManagementUnavailable)?;
        self.config.replace(next);
        Ok(())
    }

    fn require_web_management(&self) -> Result<(), BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled {
            return Err(BilibiliError::ManagementUnavailable(
                "management is disabled".into(),
            ));
        }
        Ok(())
    }
}

/// The upstream-facing operations over an explicit HTTP channel; the runner passes its
/// child-task channel, the [`BilibiliManagementApi`] impl the bound Host client.
impl BilibiliManagementService {
    pub(crate) async fn login_start_session_via(
        &self,
        http: &dyn BilibiliHttp,
        actor_id: &str,
    ) -> Result<BilibiliLoginSession, BilibiliManagementError> {
        let result = self
            .login_start_session_impl(http, actor_id)
            .await
            .map_err(map_error);
        if result.is_ok() {
            self.publish_change(vec![BilibiliManagementChangeArea::Login]);
        }
        result
    }

    pub(crate) async fn login_poll_via(
        &self,
        http: &dyn BilibiliHttp,
        actor_id: &str,
    ) -> Result<BilibiliLoginPollResult, BilibiliManagementError> {
        let result = self
            .login_poll_impl(http, actor_id)
            .await
            .map_err(map_error);
        if result.is_ok() {
            self.publish_change(vec![
                BilibiliManagementChangeArea::Login,
                BilibiliManagementChangeArea::Status,
            ]);
        }
        result
    }

    pub(crate) async fn preview_via(
        &self,
        http: &dyn BilibiliHttp,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
    ) -> Result<BilibiliPreviewCardView, BilibiliManagementError> {
        self.preview_impl(http, actor_id, is_admin, selector)
            .await
            .map_err(map_error)
    }

    pub(crate) async fn bind_start_via(
        &self,
        http: &dyn BilibiliHttp,
        operator_user_id: &str,
        uid: u64,
        challenge_seed: &str,
    ) -> Result<BilibiliBindChallengeResult, BilibiliManagementError> {
        self.bind_start_impl(http, operator_user_id, uid, challenge_seed)
            .await
            .map_err(map_error)
    }

    pub(crate) async fn bind_verify_via(
        &self,
        http: &dyn BilibiliHttp,
        operator_user_id: &str,
        platform: &str,
        target: BotTarget,
    ) -> Result<BilibiliBindVerifyResult, BilibiliManagementError> {
        let result = self
            .bind_verify_impl(http, operator_user_id, platform, target)
            .await
            .map_err(map_error);
        if matches!(result, Ok(BilibiliBindVerifyResult::Verified(_))) {
            self.publish_change(vec![
                BilibiliManagementChangeArea::Subscriptions,
                BilibiliManagementChangeArea::Status,
            ]);
        }
        result
    }
}

#[async_trait]
impl BilibiliManagementApi for BilibiliManagementService {
    fn subscribe_changes(&self) -> Option<BilibiliManagementChangeSubscription> {
        Some(BilibiliManagementChangeSubscription::new(
            self.changes.subscribe(),
        ))
    }

    fn status(&self) -> BilibiliManagementStatus {
        self.status_impl()
    }

    async fn login_start_session(
        &self,
        actor_id: &str,
    ) -> Result<BilibiliLoginSession, BilibiliManagementError> {
        self.login_start_session_via(&*self.bound_http()?, actor_id)
            .await
    }

    async fn login_start(
        &self,
        actor_id: &str,
    ) -> Result<BilibiliLoginStartResult, BilibiliManagementError> {
        let http = self.bound_http()?;
        let result = self
            .login_start_impl(&*http, actor_id)
            .await
            .map_err(map_error);
        if result.is_ok() {
            self.publish_change(vec![BilibiliManagementChangeArea::Login]);
        }
        result
    }

    async fn login_poll(
        &self,
        actor_id: &str,
    ) -> Result<BilibiliLoginPollResult, BilibiliManagementError> {
        self.login_poll_via(&*self.bound_http()?, actor_id).await
    }

    fn credential_clear(&self) -> Result<(), BilibiliManagementError> {
        let result = self.credential_clear_impl().map_err(map_error);
        if result.is_ok() {
            self.publish_change(vec![BilibiliManagementChangeArea::Status]);
        }
        result
    }

    fn list(
        &self,
        actor_id: &str,
        is_admin: bool,
    ) -> Result<Vec<BilibiliSubscriptionView>, BilibiliManagementError> {
        self.list_impl(actor_id, is_admin).map_err(map_error)
    }

    fn subscribe(
        &self,
        subscription_id: String,
        uid: u64,
        notifications: Vec<BilibiliNotificationKind>,
        target: BotTarget,
        outbound_binding: String,
    ) -> Result<BilibiliSubscriptionView, BilibiliManagementError> {
        let result = self
            .subscribe_impl(
                subscription_id,
                uid,
                notifications,
                target,
                outbound_binding,
            )
            .map_err(map_error);
        if result.is_ok() {
            self.publish_change(vec![
                BilibiliManagementChangeArea::Subscriptions,
                BilibiliManagementChangeArea::Status,
            ]);
        }
        result
    }

    fn unsubscribe(&self, subscription_id: &str) -> Result<(), BilibiliManagementError> {
        let result = self.unsubscribe_impl(subscription_id).map_err(map_error);
        if result.is_ok() {
            self.publish_change(vec![
                BilibiliManagementChangeArea::Subscriptions,
                BilibiliManagementChangeArea::Status,
            ]);
        }
        result
    }

    fn set_paused(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
        paused: bool,
    ) -> Result<BilibiliSubscriptionView, BilibiliManagementError> {
        let result = self
            .set_paused_impl(actor_id, is_admin, selector, paused)
            .map_err(map_error);
        if result.is_ok() {
            self.publish_change(vec![BilibiliManagementChangeArea::Subscriptions]);
        }
        result
    }

    async fn preview(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
    ) -> Result<BilibiliPreviewCardView, BilibiliManagementError> {
        self.preview_via(&*self.bound_http()?, actor_id, is_admin, selector)
            .await
    }

    async fn bind_start(
        &self,
        operator_user_id: &str,
        uid: u64,
        challenge_seed: &str,
    ) -> Result<BilibiliBindChallengeResult, BilibiliManagementError> {
        self.bind_start_via(&*self.bound_http()?, operator_user_id, uid, challenge_seed)
            .await
    }

    async fn bind_verify(
        &self,
        operator_user_id: &str,
        platform: &str,
        target: BotTarget,
    ) -> Result<BilibiliBindVerifyResult, BilibiliManagementError> {
        self.bind_verify_via(&*self.bound_http()?, operator_user_id, platform, target)
            .await
    }

    fn unbind(&self, operator_user_id: &str) -> Result<bool, BilibiliManagementError> {
        let result = self.unbind_impl(operator_user_id).map_err(map_error);
        if matches!(result, Ok(true)) {
            self.publish_change(vec![
                BilibiliManagementChangeArea::Subscriptions,
                BilibiliManagementChangeArea::Status,
            ]);
        }
        result
    }
}

fn require_deliverable_target(target: &BotTarget) -> Result<(), BilibiliError> {
    let empty = match target {
        BotTarget::User { user_id } => user_id.trim().is_empty(),
        BotTarget::Group { group_id } => group_id.trim().is_empty(),
        BotTarget::GuildChannel {
            guild_id,
            channel_id,
        } => guild_id.trim().is_empty() || channel_id.trim().is_empty(),
        BotTarget::Conversation { conversation_id } => conversation_id.trim().is_empty(),
        BotTarget::PlatformSpecific { platform, kind, id } => {
            platform.trim().is_empty() || kind.trim().is_empty() || id.trim().is_empty()
        }
    };
    if empty {
        return Err(BilibiliError::InvalidResponse(
            "target identifiers must not be empty".into(),
        ));
    }
    Ok(())
}

fn map_error(error: BilibiliError) -> BilibiliManagementError {
    BilibiliManagementError {
        code: match &error {
            BilibiliError::Forbidden => "bilibili.management_forbidden",
            BilibiliError::ManagementUnavailable(_) => "bilibili.management_unavailable",
            _ => "bilibili.request_failed",
        }
        .into(),
        message: error.to_string(),
    }
}

impl From<BilibiliQrStatus> for BilibiliQrLoginStatus {
    fn from(value: BilibiliQrStatus) -> Self {
        match value {
            BilibiliQrStatus::Pending => Self::Pending,
            BilibiliQrStatus::Scanned => Self::Scanned,
            BilibiliQrStatus::Expired => Self::Expired,
            BilibiliQrStatus::Confirmed => Self::Confirmed,
        }
    }
}

impl From<BilibiliNotificationKind> for BilibiliPollKind {
    fn from(value: BilibiliNotificationKind) -> Self {
        match value {
            BilibiliNotificationKind::Live => Self::Live,
            BilibiliNotificationKind::Dynamic => Self::Dynamic,
            BilibiliNotificationKind::Video => Self::Video,
        }
    }
}

impl From<BilibiliPollKind> for BilibiliNotificationKind {
    fn from(value: BilibiliPollKind) -> Self {
        match value {
            BilibiliPollKind::Live => Self::Live,
            BilibiliPollKind::Dynamic => Self::Dynamic,
            BilibiliPollKind::Video => Self::Video,
        }
    }
}

impl From<BilibiliSubscription> for BilibiliSubscriptionView {
    fn from(subscription: BilibiliSubscription) -> Self {
        Self {
            subscription_id: subscription.subscription_id,
            uid: subscription.uid,
            notifications: subscription
                .notifications
                .into_iter()
                .map(Into::into)
                .collect(),
            target: subscription.target,
            outbound_binding: subscription.outbound_binding,
            paused: subscription.paused,
            owner_user_id: subscription.owner_user_id,
        }
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
