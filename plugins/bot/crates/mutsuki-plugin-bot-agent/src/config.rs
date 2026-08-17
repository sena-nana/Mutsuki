use std::fmt;
use std::sync::Arc;

use mutsuki_agent_client::{AgentConnectionId, AgentConnectionIdError};
use mutsuki_bot_protocol::{AgentSessionScope, BotSpeechReplyPolicy, QqStreamingStrategy};
use mutsuki_config_service::{
    ConfigDescriptor, ConfigValueType, EnumOption, LocalizedText, MutsukiConfigSchema,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::BOT_AGENT_BRIDGE_PLUGIN_ID;

pub const BOT_AGENT_CONFIG_PROVIDER_ID: &str = BOT_AGENT_BRIDGE_PLUGIN_ID;
pub const BOT_AGENT_CONFIG_SERVICE_ID: &str = "mutsuki.bot.agent.config";
pub const BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES: usize = 1_800;
pub const BOT_AGENT_MIN_MESSAGE_BYTES: usize = 4;
pub const BOT_AGENT_MAX_CONCURRENCY: usize = 64;
pub const BOT_AGENT_MAX_TIMEOUT_MS: u64 = 600_000;

/// Runtime settings owned by the bot-agent bridge.
///
/// Conversation-level policy remains the authority for permissions and explicit profile
/// bindings. These settings only provide product defaults and delivery/runtime controls.
#[derive(
    Clone, Debug, Deserialize, Eq, PartialEq, Serialize, mutsuki_config_service::MutsukiConfig,
)]
#[config(
    provider_id = "mutsuki.plugin.bot.agent",
    title = "回复",
    schema_version = 1,
    value_version = 1
)]
#[serde(default, deny_unknown_fields)]
pub struct BotAgentConfig {
    #[config(
        title = "启用",
        default = false,
        restart = "plugin_reload",
        group = "对话"
    )]
    pub enabled: bool,
    #[config(
        title = "会话范围",
        default = "shared_conversation",
        restart = "plugin_reload",
        group = "对话",
        enabled_if = "enabled"
    )]
    pub session_scope: String,
    #[config(
        title = "模型连接",
        description = "本机模型一般填 local。",
        default = "",
        max_length = 128,
        restart = "plugin_reload",
        group = "对话",
        enabled_if = "enabled"
    )]
    pub connection_id: String,
    #[config(
        title = "默认配置",
        description = "会话未指定时使用。留空则必须在会话里指定。",
        default = "",
        max_length = 256,
        restart = "plugin_reload",
        group = "对话",
        enabled_if = "enabled"
    )]
    pub default_profile_id: String,
    #[config(
        title = "发送方式",
        default = "final_only",
        restart = "plugin_reload",
        group = "发送",
        enabled_if = "enabled"
    )]
    pub streaming: String,
    #[config(
        title = "单条上限",
        description = "超出会拆成多条发送。",
        default = 1800,
        min = 4,
        max = 1800,
        unit = "字节",
        restart = "plugin_reload",
        group = "发送",
        enabled_if = "enabled"
    )]
    pub max_message_bytes: usize,
    #[config(
        title = "并行对话",
        description = "同时处理的对话数。",
        default = 1,
        min = 1,
        max = 64,
        restart = "plugin_reload",
        group = "发送",
        enabled_if = "enabled"
    )]
    pub max_concurrency: usize,
    #[config(
        title = "等待超时",
        description = "单轮回复最长等待时间。",
        default = 120000,
        min = 1,
        max = 600000,
        unit = "毫秒",
        restart = "plugin_reload",
        group = "发送",
        enabled_if = "enabled"
    )]
    pub timeout_ms: u64,
    #[config(
        title = "识别语音",
        description = "收到语音时先转成文字。",
        default = false,
        restart = "plugin_reload",
        group = "语音",
        enabled_if = "enabled"
    )]
    pub stt_enabled: bool,
    #[config(
        title = "语音播报",
        description = "把文字回复合成语音。",
        default = false,
        restart = "plugin_reload",
        group = "语音",
        enabled_if = "enabled"
    )]
    pub tts_enabled: bool,
    #[config(
        title = "回复内容",
        default = "text_only",
        restart = "plugin_reload",
        group = "语音",
        enabled_if = "enabled"
    )]
    pub speech_reply_policy: String,
    #[config(
        title = "识别服务",
        description = "留空使用默认服务。",
        default = "",
        restart = "plugin_reload",
        group = "语音",
        enabled_if = "enabled",
        visible_if = "stt_enabled"
    )]
    pub stt_selector_id: String,
    #[config(
        title = "合成服务",
        description = "留空使用默认服务。",
        default = "",
        restart = "plugin_reload",
        group = "语音",
        enabled_if = "enabled",
        visible_if = "tts_enabled"
    )]
    pub tts_selector_id: String,
}

impl Default for BotAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            connection_id: String::new(),
            default_profile_id: String::new(),
            session_scope: "shared_conversation".into(),
            stt_enabled: false,
            tts_enabled: false,
            speech_reply_policy: "text_only".into(),
            stt_selector_id: String::new(),
            tts_selector_id: String::new(),
            streaming: "final_only".into(),
            max_concurrency: 1,
            timeout_ms: 120_000,
            max_message_bytes: BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

impl BotAgentConfig {
    /// Validates every live Bot Agent runtime policy field as one snapshot.
    ///
    /// # Errors
    ///
    /// Returns the typed invalid field or unsupported streaming strategy.
    pub fn validate(&self) -> Result<(), BotAgentConfigError> {
        if self.enabled {
            AgentConnectionId::new(self.connection_id.clone())?;
        } else if !self.connection_id.is_empty() {
            AgentConnectionId::new(self.connection_id.clone())?;
        }
        if self.default_profile_id.len() > 256
            || self.default_profile_id.chars().any(char::is_control)
        {
            return Err(BotAgentConfigError::InvalidProfileId);
        }
        self.session_scope()?;
        self.speech_reply_policy()?;
        self.streaming_strategy()?;
        if !(1..=BOT_AGENT_MAX_CONCURRENCY).contains(&self.max_concurrency) {
            return Err(BotAgentConfigError::InvalidConcurrency(
                self.max_concurrency,
            ));
        }
        if !(1..=BOT_AGENT_MAX_TIMEOUT_MS).contains(&self.timeout_ms) {
            return Err(BotAgentConfigError::InvalidTimeout(self.timeout_ms));
        }
        if !(BOT_AGENT_MIN_MESSAGE_BYTES..=BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES)
            .contains(&self.max_message_bytes)
        {
            return Err(BotAgentConfigError::InvalidMessageBytes(
                self.max_message_bytes,
            ));
        }
        Ok(())
    }

    /// Returns the selected typed connection id when the plugin is enabled.
    pub fn selected_connection_id(&self) -> Result<Option<AgentConnectionId>, BotAgentConfigError> {
        if !self.enabled {
            return Ok(None);
        }
        AgentConnectionId::new(self.connection_id.clone())
            .map(Some)
            .map_err(BotAgentConfigError::from)
    }

    /// Decodes the configured streaming policy into the QQ delivery strategy.
    ///
    /// # Errors
    ///
    /// Returns an error when the persisted streaming name is unsupported.
    pub fn streaming_strategy(&self) -> Result<QqStreamingStrategy, BotAgentConfigError> {
        match self.streaming.trim() {
            "final_only" => Ok(QqStreamingStrategy::FinalOnly),
            "segment_messages" => Ok(QqStreamingStrategy::SegmentMessages),
            other => Err(BotAgentConfigError::InvalidStreaming(other.into())),
        }
    }

    pub fn session_scope(&self) -> Result<AgentSessionScope, BotAgentConfigError> {
        match self.session_scope.trim() {
            "shared_conversation" => Ok(AgentSessionScope::SharedConversation),
            "actor_in_conversation" => Ok(AgentSessionScope::ActorInConversation),
            other => Err(BotAgentConfigError::InvalidSessionScope(other.into())),
        }
    }

    pub fn speech_reply_policy(&self) -> Result<BotSpeechReplyPolicy, BotAgentConfigError> {
        match self.speech_reply_policy.trim() {
            "text_only" => Ok(BotSpeechReplyPolicy::TextOnly),
            "text_and_voice" => Ok(BotSpeechReplyPolicy::TextAndVoice),
            "voice_only" => Ok(BotSpeechReplyPolicy::VoiceOnly),
            other => Err(BotAgentConfigError::InvalidSpeechReplyPolicy(other.into())),
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum BotAgentConfigError {
    #[error(transparent)]
    InvalidConnectionId(#[from] AgentConnectionIdError),
    #[error("default_profile_id must be at most 256 characters and contain no control characters")]
    InvalidProfileId,
    #[error("unsupported session scope `{0}`")]
    InvalidSessionScope(String),
    #[error("unsupported speech reply policy `{0}`")]
    InvalidSpeechReplyPolicy(String),
    #[error("unsupported streaming mode `{0}`")]
    InvalidStreaming(String),
    #[error("max_concurrency must be between 1 and {0}")]
    InvalidConcurrency(usize),
    #[error("timeout_ms must be between 1 and {0}")]
    InvalidTimeout(u64),
    #[error("max_message_bytes must be between 4 and 1800 (got {0})")]
    InvalidMessageBytes(usize),
}

/// Shared live settings used by the bridge and the authenticated Config Web backend.
#[derive(Clone)]
pub struct BotAgentConfigHandle(Arc<RwLock<VersionedBotAgentConfig>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotAgentConfigSnapshot {
    pub generation: u64,
    pub config: BotAgentConfig,
}

struct VersionedBotAgentConfig {
    generation: u64,
    config: BotAgentConfig,
}

impl Default for BotAgentConfigHandle {
    fn default() -> Self {
        Self::new(BotAgentConfig::default()).expect("default Bot Agent config is valid")
    }
}

impl fmt::Debug for BotAgentConfigHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("BotAgentConfigHandle")
            .field(&self.snapshot())
            .finish()
    }
}

impl BotAgentConfigHandle {
    /// Creates a live generation handle from a fully validated policy snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when any runtime policy field is invalid.
    pub fn new(config: BotAgentConfig) -> Result<Self, BotAgentConfigError> {
        config.validate()?;
        Ok(Self(Arc::new(RwLock::new(VersionedBotAgentConfig {
            generation: 1,
            config,
        }))))
    }

    #[must_use]
    pub fn snapshot(&self) -> BotAgentConfig {
        self.versioned_snapshot().config
    }

    #[must_use]
    pub fn versioned_snapshot(&self) -> BotAgentConfigSnapshot {
        let live = self.0.read();
        BotAgentConfigSnapshot {
            generation: live.generation,
            config: live.config.clone(),
        }
    }

    /// Atomically publishes all runtime policy fields as the next generation.
    ///
    /// # Errors
    ///
    /// Returns an error without advancing the generation when validation fails.
    pub fn replace(&self, config: BotAgentConfig) -> Result<(), BotAgentConfigError> {
        config.validate()?;
        let mut live = self.0.write();
        live.generation = live.generation.saturating_add(1);
        live.config = config;
        Ok(())
    }
}

/// The generic config renderer exposes this field as a select instead of an unbounded string.
#[must_use]
pub fn bot_agent_config_schema() -> ConfigDescriptor {
    let mut descriptor = BotAgentConfig::schema();
    patch_enum_field(
        &mut descriptor,
        "streaming",
        &[("final_only", "完整回复"), ("segment_messages", "分段回复")],
    );
    patch_enum_field(
        &mut descriptor,
        "session_scope",
        &[
            ("shared_conversation", "同一对话共享"),
            ("actor_in_conversation", "按用户隔离"),
        ],
    );
    patch_enum_field(
        &mut descriptor,
        "speech_reply_policy",
        &[
            ("text_only", "仅文本"),
            ("text_and_voice", "文本与语音"),
            ("voice_only", "仅语音"),
        ],
    );
    descriptor
}

fn patch_enum_field(descriptor: &mut ConfigDescriptor, key: &str, options: &[(&str, &str)]) {
    if let Some(node) = descriptor
        .root
        .children
        .iter_mut()
        .find(|node| node.key.as_str() == key)
    {
        node.value_type = ConfigValueType::Enum {
            options: options
                .iter()
                .map(|(value, label)| EnumOption {
                    value: (*value).into(),
                    label: LocalizedText::new(*label),
                })
                .collect(),
            multi: false,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_schema_has_streaming_select() {
        let config = BotAgentConfig::default();
        config.validate().unwrap();
        assert!(!config.enabled);
        assert!(config.connection_id.is_empty());
        let schema = bot_agent_config_schema();
        assert_eq!(
            schema
                .groups
                .iter()
                .map(|group| group.id.as_str())
                .collect::<Vec<_>>(),
            ["对话", "发送", "语音"]
        );
        let field = |key: &str| {
            schema
                .root
                .children
                .iter()
                .find(|node| node.key.as_str() == key)
                .unwrap()
        };
        let enum_values = |key: &str| match &field(key).value_type {
            ConfigValueType::Enum {
                options,
                multi: false,
            } => options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            other => panic!("{key} should be a single-select enum, got {other:?}"),
        };
        assert_eq!(enum_values("streaming"), ["final_only", "segment_messages"]);
        assert_eq!(
            enum_values("session_scope"),
            ["shared_conversation", "actor_in_conversation"]
        );
        assert_eq!(
            enum_values("speech_reply_policy"),
            ["text_only", "text_and_voice", "voice_only"]
        );
        assert_eq!(
            field("connection_id").presentation.group.as_deref(),
            Some("对话")
        );
        assert_eq!(
            field("stt_selector_id").presentation.group.as_deref(),
            Some("语音")
        );
        assert_eq!(
            field("streaming").presentation.group.as_deref(),
            Some("发送")
        );
        assert!(field("stt_selector_id").visibility.is_some());
        assert!(field("connection_id").enabled_if.is_some());
        assert!(field("enabled").enabled_if.is_none());
    }

    #[test]
    fn enabled_config_requires_a_valid_connection_id() {
        let mut config = BotAgentConfig::default();
        config.enabled = true;
        assert!(matches!(
            config.validate(),
            Err(BotAgentConfigError::InvalidConnectionId(_))
        ));
        config.connection_id = "primary".into();
        assert_eq!(
            config.selected_connection_id().unwrap().unwrap().as_str(),
            "primary"
        );
    }

    #[test]
    fn invalid_runtime_settings_are_rejected_before_live_replace() {
        let handle = BotAgentConfigHandle::default();
        let initial = handle.versioned_snapshot();
        let mut invalid = handle.snapshot();
        invalid.streaming = "unknown".into();
        assert_eq!(
            handle.replace(invalid),
            Err(BotAgentConfigError::InvalidStreaming("unknown".into()))
        );
        assert_eq!(handle.versioned_snapshot(), initial);
    }

    #[test]
    fn runtime_policy_is_published_as_one_monotonic_generation() {
        let handle = BotAgentConfigHandle::default();
        let before = handle.versioned_snapshot();
        let mut candidate = before.config.clone();
        candidate.max_concurrency = 8;
        candidate.timeout_ms = 30_000;
        candidate.streaming = "segment_messages".into();
        candidate.default_profile_id = "support".into();

        handle.replace(candidate.clone()).unwrap();

        let after = handle.versioned_snapshot();
        assert_eq!(after.generation, before.generation + 1);
        assert_eq!(after.config, candidate);
    }
}
