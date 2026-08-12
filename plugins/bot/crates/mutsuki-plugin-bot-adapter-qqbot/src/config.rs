use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use mutsuki_bot_protocol::{
    BotConversationKind, BotMediaKind, QqBotCapabilityMatrix, QqMessageSegmentKind,
    QqPermissionRequirement, QqRateLimitPolicy, QqStreamingStrategy, QqUploadConstraints,
};
use mutsuki_config_service::{
    ConfigConstraints, ConfigDescriptor, ConfigKey, ConfigMutability, ConfigNode,
    ConfigPresentation, ConfigProviderId, ConfigScope, ConfigValue, ConfigValueType, LocalizedText,
    RestartPolicy, SecretState,
};

pub const DEFAULT_QQBOT_INTENTS: u64 = 1_325_405_185;
pub const QQ_CLIENT_SECRET_FIELD: &str = "client_secret";
pub const QQ_CLIENT_SECRET_KEY: &str = "QQBOT_CLIENT_SECRET";

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
        let group_c2c_enabled = self.gateway_intents & GROUP_AND_C2C_EVENT != 0;
        let guild_enabled = self.gateway_intents & PUBLIC_GUILD_MESSAGES != 0;
        let mut conversation_kinds = Vec::new();
        let mut required_intents = Vec::new();
        let mut required_permissions = Vec::new();
        if group_c2c_enabled {
            conversation_kinds.extend([BotConversationKind::Private, BotConversationKind::Group]);
            required_intents.push("group_and_c2c_event".into());
            required_permissions.extend([
                QqPermissionRequirement::ReadGroupMessages,
                QqPermissionRequirement::ReadC2cMessages,
                QqPermissionRequirement::SendGroupMessages,
                QqPermissionRequirement::SendC2cMessages,
                QqPermissionRequirement::RecallGroupMessages,
                QqPermissionRequirement::RecallC2cMessages,
            ]);
        }
        if guild_enabled {
            conversation_kinds.push(BotConversationKind::Channel);
            required_intents.push("public_guild_messages".into());
            required_permissions.push(QqPermissionRequirement::ReadGuildAtMessages);
        }
        let media_enabled = self.media_provider_id.is_some();
        let inbound_media = if media_enabled && !conversation_kinds.is_empty() {
            vec![
                BotMediaKind::Image,
                BotMediaKind::Audio,
                BotMediaKind::Video,
                BotMediaKind::File,
            ]
        } else {
            Vec::new()
        };
        let outbound_conversation_kinds =
            [BotConversationKind::Private, BotConversationKind::Group]
                .into_iter()
                .filter(|_| group_c2c_enabled)
                .collect::<Vec<_>>();
        let active_message_kinds = outbound_conversation_kinds.clone();
        let outbound_media = if media_enabled && !outbound_conversation_kinds.is_empty() {
            required_permissions.extend([
                QqPermissionRequirement::UploadGroupMedia,
                QqPermissionRequirement::UploadC2cMedia,
            ]);
            inbound_media.clone()
        } else {
            Vec::new()
        };
        let mut inbound_segments = if conversation_kinds.is_empty() {
            Vec::new()
        } else {
            vec![
                QqMessageSegmentKind::Text,
                QqMessageSegmentKind::MentionUser,
                QqMessageSegmentKind::Reply,
                QqMessageSegmentKind::Quote,
            ]
        };
        inbound_segments.extend(inbound_media.iter().map(media_segment_kind));
        let mut outbound_segments = if outbound_conversation_kinds.is_empty() {
            Vec::new()
        } else {
            vec![
                QqMessageSegmentKind::Text,
                QqMessageSegmentKind::MentionUser,
                QqMessageSegmentKind::MentionAll,
                QqMessageSegmentKind::Reply,
                QqMessageSegmentKind::Quote,
            ]
        };
        outbound_segments.extend(outbound_media.iter().map(media_segment_kind));
        QqBotCapabilityMatrix {
            account_id: self.account_id.clone(),
            conversation_kinds,
            outbound_conversation_kinds,
            active_message: !active_message_kinds.is_empty(),
            active_message_kinds,
            inbound_segments,
            outbound_segments,
            inbound_media,
            outbound_media,
            message_edit: false,
            message_recall: group_c2c_enabled,
            reply: !required_intents.is_empty(),
            quote: !required_intents.is_empty(),
            mention: !required_intents.is_empty(),
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
            rate_limit: QqRateLimitPolicy {
                server_driven: true,
                honors_retry_after: true,
                max_retry_attempts: self.max_retry_attempts,
                retry_base_delay_ms: self.retry_base_delay_ms,
                retry_max_delay_ms: self.retry_max_delay_ms,
                gateway_rate_limit_delay_ms: self.gateway_rate_limit_delay_ms,
            },
            streaming: vec![
                QqStreamingStrategy::FinalOnly,
                QqStreamingStrategy::SegmentMessages,
            ],
            configured_intents: self.gateway_intents,
            shard: self.shard,
            required_intents,
            required_permissions,
        }
    }
}

/// Product-facing QQ configuration. Network and retry tuning stays on owner defaults.
#[must_use]
pub fn qq_config_descriptor(provider_id: &str) -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(provider_id),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("QQ Bot"),
        description: Some(LocalizedText::new("连接一个 QQ Bot 账号并管理消息能力")),
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("qq"),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("QQ Bot"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::PluginReload,
            children: vec![
                bool_node("enabled", "启用 QQ Bot"),
                string_node("app_id", "App ID", false),
                secret_node(QQ_CLIENT_SECRET_FIELD, "Client Secret"),
                integer_node("gateway_intents", "接收事件范围", 1.0, u64::MAX as f64),
                integer_node("shard_index", "分片序号", 0.0, u32::MAX as f64),
                integer_node("shard_count", "分片总数", 1.0, u32::MAX as f64),
            ],
        },
        groups: Vec::new(),
    }
}

#[must_use]
pub fn qq_config_value(enabled: bool, config: &QqBotConfig) -> ConfigValue {
    ConfigValue::Object(
        [
            ("enabled".into(), ConfigValue::Bool(enabled)),
            ("app_id".into(), ConfigValue::String(config.app_id.clone())),
            (
                QQ_CLIENT_SECRET_FIELD.into(),
                ConfigValue::Secret(SecretState::Keep),
            ),
            (
                "gateway_intents".into(),
                ConfigValue::Integer(i64::try_from(config.gateway_intents).unwrap_or(i64::MAX)),
            ),
            (
                "shard_index".into(),
                ConfigValue::Integer(i64::try_from(config.shard[0]).unwrap_or(i64::MAX)),
            ),
            (
                "shard_count".into(),
                ConfigValue::Integer(i64::try_from(config.shard[1]).unwrap_or(i64::MAX)),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

fn bool_node(key: &str, title: &str) -> ConfigNode {
    field_node(
        key,
        title,
        ConfigValueType::Bool,
        ConfigConstraints::default(),
    )
}

fn string_node(key: &str, title: &str, multiline: bool) -> ConfigNode {
    field_node(
        key,
        title,
        ConfigValueType::String { multiline },
        ConfigConstraints {
            required: true,
            min_length: Some(1),
            max_length: Some(2_048),
            ..ConfigConstraints::default()
        },
    )
}

fn integer_node(key: &str, title: &str, min: f64, max: f64) -> ConfigNode {
    field_node(
        key,
        title,
        ConfigValueType::Integer,
        ConfigConstraints {
            required: true,
            min: Some(min),
            max: Some(max),
            ..ConfigConstraints::default()
        },
    )
}

fn secret_node(key: &str, title: &str) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Secret,
        ConfigConstraints {
            required: true,
            ..ConfigConstraints::default()
        },
    );
    node.description = Some(LocalizedText::new("保存后不会再次显示"));
    node.presentation.secret = true;
    node
}

fn field_node(
    key: &str,
    title: &str,
    value_type: ConfigValueType,
    constraints: ConfigConstraints,
) -> ConfigNode {
    ConfigNode {
        key: ConfigKey::new(key),
        value_type,
        title: LocalizedText::new(title),
        description: None,
        default_value: None,
        constraints,
        presentation: ConfigPresentation::default(),
        visibility: None,
        enabled_if: None,
        mutability: ConfigMutability::ReadWrite,
        restart_policy: RestartPolicy::PluginReload,
        children: Vec::new(),
    }
}

fn media_segment_kind(kind: &BotMediaKind) -> QqMessageSegmentKind {
    match kind {
        BotMediaKind::Image => QqMessageSegmentKind::Image,
        BotMediaKind::Audio => QqMessageSegmentKind::Audio,
        BotMediaKind::Video => QqMessageSegmentKind::Video,
        BotMediaKind::File => QqMessageSegmentKind::File,
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
