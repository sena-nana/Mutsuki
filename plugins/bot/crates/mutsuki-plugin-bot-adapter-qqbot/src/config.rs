use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use mutsuki_bot_protocol::{
    BotConversationKind, BotMediaKind, QqBotCapabilityMatrix, QqMessageSegmentKind,
    QqPermissionRequirement, QqRateLimitPolicy, QqStreamingStrategy, QqUploadConstraints,
};
use mutsuki_config_service::{
    ConfigConstraints, ConfigDescriptor, ConfigExpr, ConfigKey, ConfigMutability, ConfigNode,
    ConfigPresentation, ConfigProviderId, ConfigScope, ConfigValue, ConfigValueType, LocalizedText,
    RestartPolicy, SecretState,
};

pub const QQ_INTENT_GUILDS: u64 = 1 << 0;
pub const QQ_INTENT_GROUP_AND_C2C: u64 = 1 << 25;
pub const QQ_INTENT_INTERACTION: u64 = 1 << 26;
pub const QQ_INTENT_MESSAGE_AUDIT: u64 = 1 << 27;
pub const QQ_INTENT_PUBLIC_GUILD: u64 = 1 << 30;
/// Documented default Identify mask. Omits undocumented bit 24 from the previous numeric default.
pub const DEFAULT_QQBOT_INTENTS: u64 = QQ_INTENT_GUILDS
    | QQ_INTENT_GROUP_AND_C2C
    | QQ_INTENT_INTERACTION
    | QQ_INTENT_MESSAGE_AUDIT
    | QQ_INTENT_PUBLIC_GUILD;
pub const QQ_CLIENT_SECRET_FIELD: &str = "client_secret";
pub const QQ_CLIENT_SECRET_KEY: &str = "QQBOT_CLIENT_SECRET";
pub const QQ_RECEIVE_PRIVATE_AND_GROUP_FIELD: &str = "receive_private_and_group";
pub const QQ_RECEIVE_GUILD_FIELD: &str = "receive_guild";

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
        let group_c2c_enabled = receive_private_and_group(self.gateway_intents);
        let guild_enabled = receive_guild(self.gateway_intents);
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
                QqMessageSegmentKind::Markdown,
                QqMessageSegmentKind::Keyboard,
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
                QqMessageSegmentKind::Markdown,
                QqMessageSegmentKind::Keyboard,
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

/// Product-facing QQ configuration. Network and retry tuning remains hidden but is persisted by
/// the owner document so deployment-specific values have one authority.
#[must_use]
pub fn qq_config_descriptor(provider_id: &str) -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(provider_id),
        schema_version: 3,
        value_version: 3,
        title: LocalizedText::new("QQ 登录"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("qq"),
            value_type: ConfigValueType::Object,
            title: LocalizedText::new("QQ 登录"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::PluginReload,
            children: vec![
                bool_node("enabled", "启用", Some("关闭后不会连接 QQ。")),
                when_enabled(app_id_node()),
                when_enabled(secret_node(QQ_CLIENT_SECRET_FIELD, "AppSecret")),
                when_enabled(bool_node(
                    QQ_RECEIVE_PRIVATE_AND_GROUP_FIELD,
                    "私聊和群聊",
                    None,
                )),
                when_enabled(bool_node(
                    QQ_RECEIVE_GUILD_FIELD,
                    "频道消息",
                    Some("只接收 @ 到机器人的消息。"),
                )),
                hidden_object_node("runtime_config", "QQ Runtime Config"),
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
                QQ_RECEIVE_PRIVATE_AND_GROUP_FIELD.into(),
                ConfigValue::Bool(receive_private_and_group(config.gateway_intents)),
            ),
            (
                QQ_RECEIVE_GUILD_FIELD.into(),
                ConfigValue::Bool(receive_guild(config.gateway_intents)),
            ),
            (
                "runtime_config".into(),
                ConfigValue::from_json(
                    &serde_json::to_value(config).expect("QQ runtime config serializes"),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

#[must_use]
pub fn receive_private_and_group(intents: u64) -> bool {
    intents & QQ_INTENT_GROUP_AND_C2C != 0
}

#[must_use]
pub fn receive_guild(intents: u64) -> bool {
    intents & QQ_INTENT_PUBLIC_GUILD != 0
}

/// Updates only the product-facing receive bits and preserves any other configured intents.
#[must_use]
pub fn apply_receive_intents(intents: u64, private_and_group: bool, guild: bool) -> u64 {
    let mut next = intents & !(QQ_INTENT_GROUP_AND_C2C | QQ_INTENT_PUBLIC_GUILD);
    if private_and_group {
        next |= QQ_INTENT_GROUP_AND_C2C;
    }
    if guild {
        next |= QQ_INTENT_PUBLIC_GUILD;
    }
    next
}

fn hidden_object_node(key: &str, title: &str) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Object,
        ConfigConstraints::default(),
    );
    node.visibility = Some(ConfigExpr::Literal {
        value: ConfigValue::Bool(false),
    });
    node
}

fn bool_node(key: &str, title: &str, description: Option<&str>) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Bool,
        ConfigConstraints::default(),
    );
    node.description = description.map(LocalizedText::new);
    node
}

fn app_id_node() -> ConfigNode {
    let mut node = field_node(
        "app_id",
        "App ID",
        ConfigValueType::String { multiline: false },
        ConfigConstraints {
            required: false,
            max_length: Some(2_048),
            ..ConfigConstraints::default()
        },
    );
    node.description = Some(LocalizedText::new("在开放平台机器人详情中查看。"));
    node
}

fn secret_node(key: &str, title: &str) -> ConfigNode {
    let mut node = field_node(
        key,
        title,
        ConfigValueType::Secret,
        ConfigConstraints {
            required: false,
            ..ConfigConstraints::default()
        },
    );
    node.presentation.secret = true;
    node
}

fn when_enabled(mut node: ConfigNode) -> ConfigNode {
    node.enabled_if = Some(ConfigExpr::Field {
        key: ConfigKey::new("enabled"),
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_descriptor_exposes_login_switches_instead_of_intent_bits() {
        let descriptor = qq_config_descriptor("mutsuki.bot.adapter.qqbot");
        assert_eq!(descriptor.schema_version, 3);
        assert_eq!(descriptor.value_version, 3);
        let keys = descriptor
            .root
            .children
            .iter()
            .map(|node| node.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "enabled",
                "app_id",
                QQ_CLIENT_SECRET_FIELD,
                QQ_RECEIVE_PRIVATE_AND_GROUP_FIELD,
                QQ_RECEIVE_GUILD_FIELD,
                "runtime_config",
            ]
        );
        assert_eq!(descriptor.title.default, "QQ 登录");
        assert!(descriptor.description.is_none());
    }

    #[test]
    fn config_value_derives_receive_switches_from_intents() {
        let mut config = QqBotConfig::new("local", "app");
        config.gateway_intents = QQ_INTENT_GROUP_AND_C2C;
        let value = qq_config_value(true, &config).to_json();
        assert_eq!(value["receive_private_and_group"], true);
        assert_eq!(value["receive_guild"], false);
        assert!(value.get("gateway_intents").is_none());
    }

    #[test]
    fn apply_receive_intents_preserves_unrelated_bits() {
        let extra = 1 << 1;
        let next = apply_receive_intents(extra | QQ_INTENT_PUBLIC_GUILD, true, false);
        assert_eq!(next, extra | QQ_INTENT_GROUP_AND_C2C);
        assert!(receive_private_and_group(next));
        assert!(!receive_guild(next));
    }

    #[test]
    fn default_intents_keep_documented_bits_and_omit_undocumented_bit_24() {
        assert_eq!(
            DEFAULT_QQBOT_INTENTS,
            QQ_INTENT_GUILDS
                | QQ_INTENT_GROUP_AND_C2C
                | QQ_INTENT_INTERACTION
                | QQ_INTENT_MESSAGE_AUDIT
                | QQ_INTENT_PUBLIC_GUILD
        );
        assert_eq!(DEFAULT_QQBOT_INTENTS & (1 << 24), 0);
    }
}
