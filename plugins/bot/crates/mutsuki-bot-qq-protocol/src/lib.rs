//! QQ Bot platform contract: protocol ids, error codes and capability facts that only the QQ
//! adapter, its integration and QQ-specific console pages use. Platform-neutral contracts stay in
//! `mutsuki-bot-protocol`; business plugins never depend on this crate.

use serde::{Deserialize, Serialize};

use mutsuki_bot_protocol::BotAdapterCapabilities;

pub const QQBOT_RAW_CALL_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.raw/call@1";
pub const QQBOT_ACCOUNT_GET_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.account/get@1";
pub const QQBOT_GATEWAY_STATUS_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.gateway/status@1";
pub const QQBOT_CAPABILITY_GET_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.capability/get@1";
/// Folds a long reply into a QQ forward card; owned by the QQ adapter's OpenAPI runner.
pub const QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID: &str = "mutsuki.bot.qq.reply/forward-fold@1";

pub const QQBOT_OPENAPI_RATE_LIMITED_ERROR: &str = "qqbot.openapi.rate_limited";
pub const QQBOT_OPENAPI_AUTHENTICATION_ERROR: &str = "qqbot.openapi.authentication";
pub const QQBOT_OPENAPI_TRANSIENT_ERROR: &str = "qqbot.openapi.transient";
pub const QQBOT_OPENAPI_INVALID_REQUEST_ERROR: &str = "qqbot.openapi.invalid_request";
pub const QQBOT_OPENAPI_PERMANENT_ERROR: &str = "qqbot.openapi.permanent";
pub const QQBOT_OPENAPI_MEDIA_PROVIDER_ERROR: &str = "qqbot.openapi.media_provider";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqPermissionRequirement {
    ReadGroupMessages,
    ReadC2cMessages,
    ReadGuildAtMessages,
    SendGroupMessages,
    SendC2cMessages,
    UploadGroupMedia,
    UploadC2cMedia,
    RecallGroupMessages,
    RecallC2cMessages,
}

/// QQ account capability snapshot: the neutral adapter capabilities plus Gateway facts. The
/// neutral part is flattened so the wire shape of `capability/get@1` is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotCapabilityMatrix {
    pub account_id: String,
    #[serde(flatten)]
    pub capabilities: BotAdapterCapabilities,
    #[serde(default)]
    pub configured_intents: u64,
    #[serde(default)]
    pub shard: [u64; 2],
    pub required_intents: Vec<String>,
    #[serde(default)]
    pub required_permissions: Vec<QqPermissionRequirement>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotCapabilityGetRequest {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotAccountGetRequest {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqBotGatewayStatusRequest {}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{BotAdapterCapabilities, BotStreamingStrategy};
    use serde_json::json;

    use super::*;

    #[test]
    fn capability_matrix_keeps_its_flat_wire_shape() {
        let matrix = QqBotCapabilityMatrix {
            account_id: "qq-main".into(),
            capabilities: BotAdapterCapabilities {
                conversation_kinds: Vec::new(),
                outbound_conversation_kinds: Vec::new(),
                active_message_kinds: Vec::new(),
                inbound_segments: Vec::new(),
                outbound_segments: Vec::new(),
                inbound_media: Vec::new(),
                outbound_media: Vec::new(),
                active_message: false,
                message_edit: false,
                message_recall: true,
                reply: true,
                quote: false,
                mention: true,
                upload: Default::default(),
                rate_limit: Default::default(),
                streaming: vec![BotStreamingStrategy::FinalOnly],
            },
            configured_intents: 1,
            shard: [0, 1],
            required_intents: Vec::new(),
            required_permissions: vec![QqPermissionRequirement::SendGroupMessages],
        };
        let wire = serde_json::to_value(&matrix).unwrap();
        assert_eq!(wire["message_recall"], json!(true));
        assert_eq!(wire["streaming"], json!(["final_only"]));
        assert!(wire.get("capabilities").is_none());
        assert_eq!(
            serde_json::from_value::<QqBotCapabilityMatrix>(wire).unwrap(),
            matrix
        );
    }
}
