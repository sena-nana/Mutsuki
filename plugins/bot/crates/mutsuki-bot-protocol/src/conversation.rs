use serde::{Deserialize, Serialize};

use crate::{BotConversationKind, BotSpeechReplyPolicy, BotTarget};

pub const QQ_CONVERSATION_REF_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqConversationRef {
    pub version: u16,
    pub account_id: String,
    pub kind: BotConversationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

impl QqConversationRef {
    pub fn origin_key(&self) -> String {
        let parts = [
            self.version.to_string(),
            self.account_id.clone(),
            match self.kind {
                BotConversationKind::Private => "private",
                BotConversationKind::Group => "group",
                BotConversationKind::Channel => "channel",
            }
            .into(),
            self.user_id.clone().unwrap_or_default(),
            self.group_id.clone().unwrap_or_default(),
            self.guild_id.clone().unwrap_or_default(),
            self.channel_id.clone().unwrap_or_default(),
            self.thread_id.clone().unwrap_or_default(),
        ];
        let encoded = parts
            .iter()
            .map(|part| format!("{}:{part}", part.len()))
            .collect::<Vec<_>>()
            .join("|");
        format!("qq:v{}:{encoded}", self.version)
    }

    pub fn target(&self) -> Option<BotTarget> {
        match self.kind {
            BotConversationKind::Private => self.user_id.as_ref().map(|user_id| BotTarget::User {
                user_id: user_id.clone(),
            }),
            BotConversationKind::Group => self.group_id.as_ref().map(|group_id| BotTarget::Group {
                group_id: group_id.clone(),
            }),
            BotConversationKind::Channel => Some(BotTarget::GuildChannel {
                guild_id: self.guild_id.clone()?,
                channel_id: self.channel_id.clone()?,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionScope {
    #[default]
    SharedConversation,
    ActorInConversation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectMessagePolicy {
    #[default]
    Allow,
    Deny,
    Allowlisted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicy {
    pub revision: u64,
    pub enabled: bool,
    pub agent_enabled: bool,
    #[serde(default)]
    pub direct_message_policy: DirectMessagePolicy,
    pub must_mention: bool,
    #[serde(default)]
    pub wake_words: Vec<String>,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub denylist: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_profile_id: Option<String>,
    pub session_scope: AgentSessionScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub business_profile_binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_runtime_profile_id: Option<String>,
    pub stt_enabled: bool,
    pub tts_enabled: bool,
    #[serde(default)]
    pub speech_reply_policy: BotSpeechReplyPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_selector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_selector_id: Option<String>,
    pub active_delivery_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedConversationPolicy {
    pub conversation: QqConversationRef,
    pub policy: ConversationPolicy,
    pub matched_rule_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicyPatch {
    pub enabled: Option<bool>,
    pub agent_enabled: Option<bool>,
    pub direct_message_policy: Option<DirectMessagePolicy>,
    pub must_mention: Option<bool>,
    pub wake_words: Option<Vec<String>>,
    pub allowlist: Option<Vec<String>>,
    pub denylist: Option<Vec<String>>,
    pub rate_limit_profile_id: Option<Option<String>>,
    pub session_scope: Option<AgentSessionScope>,
    pub business_profile_binding_id: Option<Option<String>>,
    pub agent_runtime_profile_id: Option<Option<String>>,
    pub stt_enabled: Option<bool>,
    pub tts_enabled: Option<bool>,
    pub speech_reply_policy: Option<BotSpeechReplyPolicy>,
    pub stt_selector_id: Option<Option<String>>,
    pub tts_selector_id: Option<Option<String>>,
    pub active_delivery_enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicyMatch {
    pub account_id: Option<String>,
    pub group_id: Option<String>,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub origin_key: Option<String>,
    pub actor_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPolicyRule {
    pub rule_id: String,
    pub revision: u64,
    pub matcher: ConversationPolicyMatch,
    pub patch: ConversationPolicyPatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionBinding {
    pub origin_key: String,
    pub session_id: String,
    pub session_version: u64,
    pub last_event_sequence: u64,
    pub policy_revision: u64,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BotAgentBridgeRequest {
    Submit {
        event: crate::BotEvent,
    },
    Cancel {
        event: crate::BotEvent,
        turn_id: String,
    },
    Reset {
        event: crate::BotEvent,
    },
    Fork {
        event: crate::BotEvent,
    },
    Status {
        event: crate::BotEvent,
    },
    Regenerate {
        event: crate::BotEvent,
    },
}
