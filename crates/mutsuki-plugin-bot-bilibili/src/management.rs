//! Structured Bilibili account/subscription management shared by chat commands and Web Console.

use std::sync::{Arc, Mutex};

use mutsuki_bot_link_parser::ResolvedLinkCard;
use mutsuki_bot_protocol::BotTarget;
use serde::{Deserialize, Serialize};

use crate::{
    BilibiliBackendConfig, BilibiliBackendKind, BilibiliConfig, BilibiliConfigStore,
    BilibiliCredentialStore, BilibiliError, BilibiliPollKind, BilibiliQrStatus,
    BilibiliSubscription, BilibiliTransport, SharedBilibiliConfig, SharedBilibiliCredential,
    SqliteBilibiliRepository, binding_code, parse_notifications, parse_uid, render_qr_png,
    required_arg, select_subscription, self_subscription_id_for,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSecretState {
    Absent,
    Present,
    Invalid,
}

pub trait BilibiliSecretPresence: Send + Sync {
    fn inspect(&self, key: &str) -> CredentialSecretState;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManagementStatus {
    pub available: bool,
    pub backend: String,
    pub management_enabled: bool,
    pub allow_self_binding: bool,
    pub cookie_secret_key: Option<String>,
    pub oauth_credential_key: Option<String>,
    pub cookie_secret_state: Option<CredentialSecretState>,
    pub oauth_secret_state: Option<CredentialSecretState>,
    pub credential_loaded: bool,
    pub oauth_expires_at: Option<i64>,
    pub oauth_scopes: Vec<String>,
    pub subscription_count: usize,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LoginStartResult {
    pub url: String,
    pub key: String,
    pub qr_png_base64: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LoginPollResult {
    pub status: BilibiliQrStatus,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SubscriptionView {
    pub subscription_id: String,
    pub uid: u64,
    pub notifications: Vec<BilibiliPollKind>,
    pub target: BotTarget,
    pub outbound_binding: String,
    pub paused: bool,
    pub owner_user_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PreviewCardView {
    pub title: String,
    pub url: String,
    pub description: String,
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BindChallengeResult {
    pub uid: u64,
    pub name: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum BindVerifyResult {
    Verified(SubscriptionView),
    SignatureMismatch { code: String },
}

pub struct BilibiliManagementService {
    config: SharedBilibiliConfig,
    credential: SharedBilibiliCredential,
    transport: Mutex<Box<dyn BilibiliTransport>>,
    repository: Arc<SqliteBilibiliRepository>,
    credential_store: Arc<dyn BilibiliCredentialStore>,
    config_store: Arc<dyn BilibiliConfigStore>,
    secret_presence: Arc<dyn BilibiliSecretPresence>,
}

impl BilibiliManagementService {
    pub fn new(
        config: SharedBilibiliConfig,
        credential: SharedBilibiliCredential,
        transport: Box<dyn BilibiliTransport>,
        repository: Arc<SqliteBilibiliRepository>,
        credential_store: Arc<dyn BilibiliCredentialStore>,
        config_store: Arc<dyn BilibiliConfigStore>,
        secret_presence: Arc<dyn BilibiliSecretPresence>,
    ) -> Self {
        Self {
            config,
            credential,
            transport: Mutex::new(transport),
            repository,
            credential_store,
            config_store,
            secret_presence,
        }
    }

    pub fn config(&self) -> &SharedBilibiliConfig {
        &self.config
    }

    pub fn status(&self) -> ManagementStatus {
        let snapshot = self.config.snapshot();
        let backend = match snapshot.backend.kind() {
            BilibiliBackendKind::WebCookie => "web_cookie",
            BilibiliBackendKind::OpenPlatform => "open_platform",
        }
        .into();
        let (cookie_secret_key, oauth_credential_key, cookie_secret_state, oauth_secret_state) =
            match &snapshot.backend {
                BilibiliBackendConfig::WebCookie { cookie_secret_key } => (
                    Some(cookie_secret_key.clone()),
                    None,
                    Some(self.secret_presence.inspect(cookie_secret_key)),
                    None,
                ),
                BilibiliBackendConfig::OpenPlatform {
                    oauth_credential_key,
                    ..
                } => (
                    None,
                    Some(oauth_credential_key.clone()),
                    None,
                    Some(self.secret_presence.inspect(oauth_credential_key)),
                ),
            };
        let (oauth_expires_at, oauth_scopes) =
            if matches!(snapshot.backend.kind(), BilibiliBackendKind::OpenPlatform) {
                match self.credential.raw() {
                    Some(raw) => match crate::BilibiliOpenPlatformCredential::parse(&raw) {
                        Ok(credential) => (Some(credential.expires_at), credential.scopes),
                        Err(_) => (None, Vec::new()),
                    },
                    None => (None, Vec::new()),
                }
            } else {
                (None, Vec::new())
            };
        let management_enabled = snapshot.management.enabled;
        let available =
            management_enabled && matches!(snapshot.backend.kind(), BilibiliBackendKind::WebCookie);
        let reason = if !management_enabled {
            Some("management is disabled".into())
        } else if !matches!(snapshot.backend.kind(), BilibiliBackendKind::WebCookie) {
            Some("full console management requires web_cookie backend".into())
        } else {
            None
        };
        ManagementStatus {
            available,
            backend,
            management_enabled,
            allow_self_binding: snapshot.management.allow_self_binding,
            cookie_secret_key,
            oauth_credential_key,
            cookie_secret_state,
            oauth_secret_state,
            credential_loaded: self.credential.is_loaded(),
            oauth_expires_at,
            oauth_scopes,
            subscription_count: snapshot.subscriptions.len(),
            reason,
        }
    }

    pub fn login_start(&self, actor_id: &str) -> Result<LoginStartResult, BilibiliError> {
        self.require_web_management()?;
        let qr = self
            .transport
            .lock()
            .expect("bilibili transport mutex")
            .qr_start()?;
        self.repository
            .set_qr_session(actor_id, &qr.key)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        let png = render_qr_png(&qr.url)?;
        Ok(LoginStartResult {
            url: qr.url,
            key: qr.key,
            qr_png_base64: base64_encode(&png),
        })
    }

    pub fn login_poll(&self, actor_id: &str) -> Result<LoginPollResult, BilibiliError> {
        self.require_web_management()?;
        let config = self.config.snapshot();
        let key = self
            .repository
            .qr_session(actor_id)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?
            .ok_or_else(|| {
                BilibiliError::ManagementUnavailable("no active QR login; run login first".into())
            })?;
        let polled = self
            .transport
            .lock()
            .expect("bilibili transport mutex")
            .qr_poll(&key)?;
        let message = match polled.status {
            BilibiliQrStatus::Pending => "等待扫码。".into(),
            BilibiliQrStatus::Scanned => "已扫码，等待在 App 中确认。".into(),
            BilibiliQrStatus::Expired => {
                self.repository
                    .clear_qr_session(actor_id)
                    .map_err(|error| BilibiliError::Transport(error.to_string()))?;
                "二维码已过期，请重新执行 login。".into()
            }
            BilibiliQrStatus::Confirmed => {
                let credential = polled.credential.ok_or_else(|| {
                    BilibiliError::InvalidResponse("confirmed QR login omitted credential".into())
                })?;
                let cookie_secret_key = config.backend.cookie_secret_key().ok_or_else(|| {
                    BilibiliError::ManagementUnavailable("Cookie backend is not selected".into())
                })?;
                self.credential_store
                    .rotate(cookie_secret_key, credential.clone())
                    .map_err(BilibiliError::ManagementUnavailable)?;
                self.credential.set(credential);
                self.repository
                    .clear_qr_session(actor_id)
                    .map_err(|error| BilibiliError::Transport(error.to_string()))?;
                "登录成功，凭据已通过 Host secret backend 原子轮换。".into()
            }
        };
        Ok(LoginPollResult {
            status: polled.status,
            message,
        })
    }

    pub fn credential_clear(&self) -> Result<(), BilibiliError> {
        self.require_web_management()?;
        let config = self.config.snapshot();
        let cookie_secret_key = config.backend.cookie_secret_key().ok_or_else(|| {
            BilibiliError::ManagementUnavailable("Cookie backend is not selected".into())
        })?;
        self.credential_store
            .rotate(cookie_secret_key, String::new())
            .map_err(BilibiliError::ManagementUnavailable)?;
        self.credential.clear();
        Ok(())
    }

    pub fn list(
        &self,
        actor_id: &str,
        is_admin: bool,
    ) -> Result<Vec<SubscriptionView>, BilibiliError> {
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
            .map(SubscriptionView::from)
            .collect())
    }

    pub fn subscribe(
        &self,
        subscription_id: String,
        uid: u64,
        notifications: Vec<BilibiliPollKind>,
        target: BotTarget,
        outbound_binding: String,
    ) -> Result<SubscriptionView, BilibiliError> {
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
        let mut next = self.config.snapshot();
        next.subscriptions
            .retain(|subscription| subscription.subscription_id != subscription_id);
        let subscription = BilibiliSubscription {
            subscription_id,
            uid,
            notifications,
            target,
            outbound_binding,
            paused: false,
            owner_user_id: None,
        };
        next.subscriptions.push(subscription.clone());
        self.persist(next)?;
        Ok(SubscriptionView::from(subscription))
    }

    pub fn unsubscribe(&self, subscription_id: &str) -> Result<(), BilibiliError> {
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

    pub fn set_paused(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
        paused: bool,
    ) -> Result<SubscriptionView, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled {
            return Err(BilibiliError::ManagementUnavailable(
                "management is disabled".into(),
            ));
        }
        let mut next = config;
        let index = select_subscription(&next, actor_id, is_admin, selector)?;
        next.subscriptions[index].paused = paused;
        let view = SubscriptionView::from(next.subscriptions[index].clone());
        self.persist(next)?;
        Ok(view)
    }

    pub fn preview(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
    ) -> Result<PreviewCardView, BilibiliError> {
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
            .lock()
            .expect("bilibili transport mutex")
            .poll(&BilibiliPollKind::Dynamic, subscription.uid)?
            .into_iter()
            .next()
            .ok_or_else(|| BilibiliError::ManagementUnavailable("该账号暂无可预览动态。".into()))?;
        Ok(PreviewCardView {
            title: item.title,
            url: item.url,
            description: "通知预览（不会推进轮询 cursor）".into(),
            image_url: item.image_url,
        })
    }

    pub fn bind_start(
        &self,
        operator_user_id: &str,
        uid: u64,
        challenge_seed: &str,
    ) -> Result<BindChallengeResult, BilibiliError> {
        let config = self.config.snapshot();
        if !config.management.enabled || !config.management.allow_self_binding {
            return Err(BilibiliError::Forbidden);
        }
        if uid == 0 {
            return Err(BilibiliError::InvalidResponse(
                "invalid Bilibili UID".into(),
            ));
        }
        let profile = self
            .transport
            .lock()
            .expect("bilibili transport mutex")
            .profile(uid)?;
        let code = binding_code(operator_user_id, uid, challenge_seed);
        self.repository
            .set_binding_challenge(operator_user_id, uid, &code)
            .map_err(|error| BilibiliError::Transport(error.to_string()))?;
        Ok(BindChallengeResult {
            uid,
            name: profile.name,
            code,
        })
    }

    pub fn bind_verify(
        &self,
        operator_user_id: &str,
        platform: &str,
        target: BotTarget,
    ) -> Result<BindVerifyResult, BilibiliError> {
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
        let profile = self
            .transport
            .lock()
            .expect("bilibili transport mutex")
            .profile(uid)?;
        if !profile.signature.contains(&code) {
            return Ok(BindVerifyResult::SignatureMismatch { code });
        }
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
        Ok(BindVerifyResult::Verified(SubscriptionView::from(
            subscription,
        )))
    }

    pub fn unbind(&self, operator_user_id: &str) -> Result<bool, BilibiliError> {
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

    /// Chat-oriented helpers that mirror `/bili` argument parsing.
    pub fn chat_subscribe(
        &self,
        args: &[String],
        target: BotTarget,
    ) -> Result<String, BilibiliError> {
        self.require_web_management()?;
        let subscription_id = required_arg(args, 1, "subscription id")?;
        let uid = parse_uid(args.get(2))?;
        let notifications = parse_notifications(args.get(3))?;
        let outbound_binding = self
            .config
            .snapshot()
            .management
            .self_binding_outbound_binding;
        self.subscribe(
            subscription_id.clone(),
            uid,
            notifications,
            target,
            outbound_binding,
        )?;
        Ok(format!("订阅 {subscription_id} 已写入产品配置。"))
    }

    pub fn chat_login_start_png(&self, actor_id: &str) -> Result<(String, Vec<u8>), BilibiliError> {
        let started = self.login_start(actor_id)?;
        let png = base64_decode(&started.qr_png_base64).ok_or_else(|| {
            BilibiliError::InvalidResponse("failed to decode QR png payload".into())
        })?;
        Ok((
            "请使用 Bilibili App 扫码确认，然后发送 /bili login-status；二维码不会把 Cookie 写入聊天或 Task payload。"
                .into(),
            png,
        ))
    }

    pub fn preview_as_card(
        &self,
        actor_id: &str,
        is_admin: bool,
        selector: Option<&str>,
    ) -> Result<ResolvedLinkCard, BilibiliError> {
        let preview = self.preview(actor_id, is_admin, selector)?;
        Ok(ResolvedLinkCard {
            url: preview.url,
            title: preview.title,
            description: preview.description,
            image_url: preview.image_url,
        })
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
        if !matches!(config.backend.kind(), BilibiliBackendKind::WebCookie) {
            return Err(BilibiliError::ManagementUnavailable(
                "Cookie backend is not selected".into(),
            ));
        }
        Ok(())
    }
}

impl From<BilibiliSubscription> for SubscriptionView {
    fn from(subscription: BilibiliSubscription) -> Self {
        Self {
            subscription_id: subscription.subscription_id,
            uid: subscription.uid,
            notifications: subscription.notifications,
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

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let (a, b, c, d) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        let av = value(a)?;
        let bv = value(b)?;
        let cv = if c == b'=' { 0 } else { value(c)? };
        let dv = if d == b'=' { 0 } else { value(d)? };
        let triple = ((av as u32) << 18) | ((bv as u32) << 12) | ((cv as u32) << 6) | (dv as u32);
        out.push(((triple >> 16) & 0xff) as u8);
        if c != b'=' {
            out.push(((triple >> 8) & 0xff) as u8);
        }
        if d != b'=' {
            out.push((triple & 0xff) as u8);
        }
    }
    Some(out)
}
