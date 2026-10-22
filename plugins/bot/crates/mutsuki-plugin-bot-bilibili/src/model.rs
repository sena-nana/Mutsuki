//! Bilibili plugin data model: poll/link requests, configuration, shared state and store traits.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use mutsuki_bot_protocol::BotTarget;
use serde::{Deserialize, Serialize};

use crate::{POLL_DYNAMIC, POLL_LIVE, POLL_VIDEO};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    pub(crate) fn from_protocol_id(protocol_id: &str) -> Option<Self> {
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

/// Trigger event payload submitted into Bot Flow when polling finds a fresh
/// item. Card rendering and delivery are graph concerns; the plugin only
/// reports what changed and which chat the subscription watches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BilibiliNotification {
    pub kind: BilibiliPollKind,
    pub subscription_id: String,
    pub uid: u64,
    pub target: BotTarget,
    pub item_id: String,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BilibiliManagementConfig {
    pub enabled: bool,
    pub allow_self_binding: bool,
    pub admin_user_ids: Vec<String>,
    pub self_binding_notifications: Vec<BilibiliPollKind>,
    pub self_binding_outbound_binding: String,
}

impl Default for BilibiliManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_self_binding: false,
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
    WebCookie { cookie_secret_key: String },
}

impl BilibiliBackendConfig {
    pub fn cookie_secret_key(&self) -> Option<&str> {
        match self {
            Self::WebCookie { cookie_secret_key } => Some(cookie_secret_key),
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
    #[serde(default)]
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
            && (self
                .management
                .self_binding_outbound_binding
                .trim()
                .is_empty()
                || (self.management.allow_self_binding
                    && self.management.self_binding_notifications.is_empty()))
        {
            return Err("enabled management requires self-binding defaults".into());
        }
        if let Some(risk_control) = &self.risk_control {
            risk_control.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SharedBilibiliConfig(Arc<RwLock<Arc<BilibiliConfig>>>);

impl SharedBilibiliConfig {
    pub fn new(config: BilibiliConfig) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(config))))
    }

    /// Owned copy for callers that edit and `replace` the configuration.
    pub fn snapshot(&self) -> BilibiliConfig {
        self.current().as_ref().clone()
    }

    /// Cheap shared view of the current configuration for hot readers such as
    /// the polling tick; replacing the config never mutates a held view.
    pub fn current(&self) -> Arc<BilibiliConfig> {
        Arc::clone(&self.0.read().expect("Bilibili config read lock"))
    }

    pub fn replace(&self, config: BilibiliConfig) {
        *self.0.write().expect("Bilibili config write lock") = Arc::new(config);
    }
}

impl fmt::Debug for SharedBilibiliConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedBilibiliConfig(..)")
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

pub trait BilibiliCredentialStore: Send + Sync {
    fn rotate(&self, key: &str, credential: String) -> Result<(), String>;
}

pub trait BilibiliConfigStore: Send + Sync {
    fn replace(&self, config: &BilibiliConfig) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LINK_RESOLVE;

    #[test]
    fn poll_protocol_is_the_kind_discriminator() {
        assert_eq!(
            BilibiliPollKind::from_protocol_id(POLL_DYNAMIC),
            Some(BilibiliPollKind::Dynamic)
        );
        assert_eq!(BilibiliPollKind::from_protocol_id(LINK_RESOLVE), None);
    }
}
