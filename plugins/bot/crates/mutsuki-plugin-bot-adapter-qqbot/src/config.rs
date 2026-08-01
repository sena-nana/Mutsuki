use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use mutsuki_bot_protocol::{
    BotConversationKind, BotMediaKind, QqBotCapabilityMatrix, QqStreamingStrategy,
    QqUploadConstraints,
};

pub const DEFAULT_QQBOT_INTENTS: u64 = 1_325_405_185;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QqBotConfig {
    pub account_id: String,
    pub app_id: String,
    /// Key resolved through `HostEventSourceConfig::secret`; never the secret value.
    pub client_secret_key: String,
    /// Host resource provider used for platform media upload. `None` keeps a text-only adapter.
    pub media_provider_id: Option<String>,
    /// DNS suffixes accepted for QQ-signed inbound attachment downloads.
    pub media_download_allowed_hosts: Vec<String>,
    pub token_url: String,
    pub openapi_base_url: String,
    pub gateway_intents: u64,
    pub shard: [u64; 2],
    pub request_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub response_body_limit_bytes: usize,
    pub token_refresh_margin_secs: u64,
    pub max_retry_attempts: u8,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub gateway_hello_timeout_ms: u64,
    pub gateway_ack_timeout_ms: u64,
    pub gateway_queue_capacity: usize,
    pub gateway_dedup_window: usize,
    pub reconnect_initial_delay_ms: u64,
    pub reconnect_max_delay_ms: u64,
    pub reconnect_jitter_ms: u64,
    pub gateway_rate_limit_delay_ms: u64,
    /// Test/local-only escape hatch. Production defaults require HTTPS and WSS.
    pub allow_insecure_transport: bool,
}

impl Default for QqBotConfig {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl QqBotConfig {
    pub fn new(account_id: impl Into<String>, app_id: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            app_id: app_id.into(),
            client_secret_key: "QQBOT_CLIENT_SECRET".into(),
            media_provider_id: None,
            media_download_allowed_hosts: vec![
                "qq.com".into(),
                "qpic.cn".into(),
                "gtimg.cn".into(),
            ],
            token_url: "https://bots.qq.com/app/getAppAccessToken".into(),
            openapi_base_url: "https://api.sgroup.qq.com".into(),
            gateway_intents: DEFAULT_QQBOT_INTENTS,
            shard: [0, 1],
            request_timeout_ms: 15_000,
            connect_timeout_ms: 10_000,
            response_body_limit_bytes: 2 * 1024 * 1024,
            token_refresh_margin_secs: 120,
            max_retry_attempts: 3,
            retry_base_delay_ms: 250,
            retry_max_delay_ms: 5_000,
            gateway_hello_timeout_ms: 15_000,
            gateway_ack_timeout_ms: 10_000,
            gateway_queue_capacity: 128,
            gateway_dedup_window: 2_048,
            reconnect_initial_delay_ms: 500,
            reconnect_max_delay_ms: 30_000,
            reconnect_jitter_ms: 250,
            gateway_rate_limit_delay_ms: 60_000,
            allow_insecure_transport: false,
        }
    }

    pub fn validate(&self) -> Result<(), QqConfigError> {
        required("account_id", &self.account_id)?;
        required("app_id", &self.app_id)?;
        required("client_secret_key", &self.client_secret_key)?;
        if self
            .media_provider_id
            .as_deref()
            .is_some_and(|provider_id| provider_id.trim().is_empty())
        {
            return Err(QqConfigError::Invalid(
                "media_provider_id must be non-empty when configured".into(),
            ));
        }
        if self.media_download_allowed_hosts.iter().any(|host| {
            host.trim().is_empty()
                || host.contains('/')
                || host.contains(':')
                || host.starts_with('.')
        }) {
            return Err(QqConfigError::Invalid(
                "media_download_allowed_hosts must contain DNS suffixes without schemes or ports"
                    .into(),
            ));
        }
        validate_http_url("token_url", &self.token_url, self.allow_insecure_transport)?;
        validate_http_url(
            "openapi_base_url",
            &self.openapi_base_url,
            self.allow_insecure_transport,
        )?;
        if self.gateway_intents == 0 {
            return Err(QqConfigError::Invalid(
                "gateway_intents must not be zero".into(),
            ));
        }
        if self.shard[1] == 0 || self.shard[0] >= self.shard[1] {
            return Err(QqConfigError::Invalid(
                "shard must be [index, count] with index < count".into(),
            ));
        }
        for (name, value) in [
            ("request_timeout_ms", self.request_timeout_ms),
            ("connect_timeout_ms", self.connect_timeout_ms),
            ("gateway_hello_timeout_ms", self.gateway_hello_timeout_ms),
            ("gateway_ack_timeout_ms", self.gateway_ack_timeout_ms),
            ("reconnect_max_delay_ms", self.reconnect_max_delay_ms),
            (
                "gateway_rate_limit_delay_ms",
                self.gateway_rate_limit_delay_ms,
            ),
        ] {
            if value == 0 {
                return Err(QqConfigError::Invalid(format!("{name} must be positive")));
            }
        }
        if self.response_body_limit_bytes == 0
            || self.gateway_queue_capacity == 0
            || self.gateway_dedup_window == 0
        {
            return Err(QqConfigError::Invalid(
                "body limit, queue capacity and dedup window must be positive".into(),
            ));
        }
        if self.retry_base_delay_ms > self.retry_max_delay_ms
            || self.reconnect_initial_delay_ms > self.reconnect_max_delay_ms
        {
            return Err(QqConfigError::Invalid(
                "initial retry delays must not exceed maximum delays".into(),
            ));
        }
        Ok(())
    }

    /// Returns only capabilities that are enabled by this account's intents and Host resources.
    pub fn capability_matrix(&self) -> QqBotCapabilityMatrix {
        const PUBLIC_GUILD_MESSAGES: u64 = 1 << 30;
        const GROUP_AND_C2C_EVENT: u64 = 1 << 25;
        let mut conversation_kinds = Vec::new();
        let mut required_intents = Vec::new();
        if self.gateway_intents & GROUP_AND_C2C_EVENT != 0 {
            conversation_kinds.extend([BotConversationKind::Private, BotConversationKind::Group]);
            required_intents.push("group_and_c2c_event".into());
        }
        if self.gateway_intents & PUBLIC_GUILD_MESSAGES != 0 {
            conversation_kinds.push(BotConversationKind::Channel);
            required_intents.push("public_guild_messages".into());
        }
        let media_enabled = self.media_provider_id.is_some();
        let media = if media_enabled {
            vec![
                BotMediaKind::Image,
                BotMediaKind::Audio,
                BotMediaKind::Video,
                BotMediaKind::File,
            ]
        } else {
            Vec::new()
        };
        let active_message_kinds = [BotConversationKind::Private, BotConversationKind::Group]
            .into_iter()
            .filter(|_| self.gateway_intents & GROUP_AND_C2C_EVENT != 0)
            .collect::<Vec<_>>();
        QqBotCapabilityMatrix {
            account_id: self.account_id.clone(),
            conversation_kinds,
            outbound_conversation_kinds: [BotConversationKind::Private, BotConversationKind::Group]
                .into_iter()
                .filter(|_| self.gateway_intents & GROUP_AND_C2C_EVENT != 0)
                .collect(),
            active_message: !active_message_kinds.is_empty(),
            active_message_kinds,
            inbound_media: media.clone(),
            outbound_media: media,
            message_edit: false,
            message_recall: true,
            reply: true,
            mention: true,
            upload: QqUploadConstraints {
                max_bytes: Some(100 * 1024 * 1024),
                max_bytes_by_kind: [
                    (BotMediaKind::Image, 20 * 1024 * 1024),
                    (BotMediaKind::Video, 30 * 1024 * 1024),
                    (BotMediaKind::Audio, 20 * 1024 * 1024),
                    (BotMediaKind::File, 100 * 1024 * 1024),
                ]
                .into(),
                allowed_mime_types: vec![
                    "image/jpeg".into(),
                    "image/png".into(),
                    "image/gif".into(),
                    "image/webp".into(),
                    "audio/mpeg".into(),
                    "audio/wav".into(),
                    "audio/ogg".into(),
                    "video/mp4".into(),
                    "application/octet-stream".into(),
                ],
            },
            streaming: vec![
                QqStreamingStrategy::FinalOnly,
                QqStreamingStrategy::SegmentMessages,
            ],
            required_intents,
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum QqConfigError {
    #[error("missing QQBot config field: {0}")]
    Missing(&'static str),
    #[error("invalid QQBot config: {0}")]
    Invalid(String),
}

pub fn validate_gateway_url(url: &str, allow_insecure: bool) -> Result<Url, QqConfigError> {
    let parsed = Url::parse(url).map_err(|error| QqConfigError::Invalid(error.to_string()))?;
    let allowed = parsed.scheme() == "wss" || (allow_insecure && parsed.scheme() == "ws");
    if !allowed
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return Err(QqConfigError::Invalid(
            "Gateway URL must use wss:// with a host and without credentials".into(),
        ));
    }
    Ok(parsed)
}

fn required(name: &'static str, value: &str) -> Result<(), QqConfigError> {
    if value.trim().is_empty() {
        Err(QqConfigError::Missing(name))
    } else {
        Ok(())
    }
}

fn validate_http_url(name: &str, value: &str, allow_insecure: bool) -> Result<(), QqConfigError> {
    let parsed =
        Url::parse(value).map_err(|error| QqConfigError::Invalid(format!("{name}: {error}")))?;
    let allowed = parsed.scheme() == "https" || (allow_insecure && parsed.scheme() == "http");
    if !allowed
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return Err(QqConfigError::Invalid(format!(
            "{name} must be an absolute HTTPS URL without credentials"
        )));
    }
    Ok(())
}
