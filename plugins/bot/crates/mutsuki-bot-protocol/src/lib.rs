// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::ref_option,
    clippy::struct_excessive_bools
)]

mod account;
mod capability;
mod command;
mod conversation;
mod delivery;
mod error;
mod event;
mod flow;
mod interaction;
mod media;
mod media_bridge;
mod message;
mod permission;
mod persona;
mod segment;
mod target;

pub use account::*;
pub use capability::*;
pub use command::*;
pub use conversation::*;
pub use delivery::*;
pub use error::*;
pub use event::*;
pub use flow::*;
pub use interaction::*;
pub use media::*;
pub use media_bridge::*;
pub use message::*;
pub use permission::*;
pub use persona::*;
pub use segment::*;
pub use target::*;

pub const BOT_EVENT_INGEST_PROTOCOL_ID: &str = "mutsuki.bot.event/ingest@1";
pub const BOT_MESSAGE_SEND_PROTOCOL_ID: &str = "mutsuki.bot.message/send@1";
pub const BOT_MESSAGE_EDIT_PROTOCOL_ID: &str = "mutsuki.bot.message/edit@1";
pub const BOT_MESSAGE_RECALL_PROTOCOL_ID: &str = "mutsuki.bot.message/recall@1";
pub const BOT_MEDIA_UPLOAD_PROTOCOL_ID: &str = "mutsuki.bot.media/upload@1";
pub const BOT_MEDIA_DOWNLOAD_PROTOCOL_ID: &str = "mutsuki.bot.media/download@1";
pub const BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID: &str = "mutsuki.bot.media/transcribe@1";
pub const BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID: &str = "mutsuki.bot.media/synthesize@1";
pub const BOT_COMMAND_PARSE_PROTOCOL_ID: &str = "mutsuki.bot.command/parse@1";
pub const BOT_COMMAND_REPLY_PROTOCOL_ID: &str = "mutsuki.bot.command/reply@1";
pub const BOT_COMMAND_HANDLE_PROTOCOL_ID: &str = "mutsuki.bot.command/handle@1";
pub const BOT_SESSION_GET_PROTOCOL_ID: &str = "mutsuki.bot.session/get@1";
pub const BOT_SESSION_SET_PROTOCOL_ID: &str = "mutsuki.bot.session/set@1";
pub const BOT_PERMISSION_CHECK_PROTOCOL_ID: &str = "mutsuki.bot.permission/check@1";
pub const BOT_RATE_LIMIT_CHECK_PROTOCOL_ID: &str = "mutsuki.bot.rate_limit/check@1";
pub const BOT_ACTIVE_DELIVERY_PROTOCOL_ID: &str = "mutsuki.bot.delivery/submit@1";
pub const BOT_REPLY_DELIVERY_PROTOCOL_ID: &str = "mutsuki.bot.delivery/reply@1";
pub const BOT_INTERACTION_SESSION_PROTOCOL_ID: &str = "mutsuki.bot.interaction/handle@1";
pub const BOT_INTERACTION_MATCH_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/interaction@1";
pub const BOT_INTERACTION_CREATE_PROTOCOL_ID: &str = "mutsuki.bot.flow.interaction/create@1";
pub const BOT_AGENT_BRIDGE_PROTOCOL_ID: &str = "mutsuki.bot.agent/handle@1";
pub const BOT_AGENT_SUBMIT_PROTOCOL_ID: &str = "mutsuki.bot.agent/submit@1";
pub const BOT_AGENT_CANCEL_PROTOCOL_ID: &str = "mutsuki.bot.agent/cancel@1";
pub const BOT_AGENT_RESET_PROTOCOL_ID: &str = "mutsuki.bot.agent/reset@1";
pub const BOT_AGENT_FORK_PROTOCOL_ID: &str = "mutsuki.bot.agent/fork@1";
pub const BOT_AGENT_STATUS_PROTOCOL_ID: &str = "mutsuki.bot.agent/status@1";
pub const BOT_AGENT_REGENERATE_PROTOCOL_ID: &str = "mutsuki.bot.agent/regenerate@1";
pub const BOT_AGENT_BIND_PROFILE_PROTOCOL_ID: &str = "mutsuki.bot.agent/bind-profile@1";
pub const BOT_AGENT_PERSONA_PROTOCOL_ID: &str = "mutsuki.bot.agent/persona@1";
pub const BOT_AGENT_ATTACH_BOUND_PERSONA_PROTOCOL_ID: &str =
    "mutsuki.bot.agent/attach-bound-persona@1";
pub const BOT_CONVERSATION_RECORD_ICL_PROTOCOL_ID: &str = "mutsuki.bot.conversation/record-icl@1";
pub const BOT_CONVERSATION_ATTACH_ICL_PROTOCOL_ID: &str = "mutsuki.bot.conversation/attach-icl@1";
pub const BOT_CONVERSATION_ATTACH_IDENTIFIERS_PROTOCOL_ID: &str =
    "mutsuki.bot.conversation/attach-identifiers@1";
pub const BOT_REPLY_QUOTE_PROTOCOL_ID: &str = "mutsuki.bot.reply/quote@1";
pub const BOT_REPLY_MENTION_PROTOCOL_ID: &str = "mutsuki.bot.reply/mention@1";
pub const BOT_REPLY_SEGMENT_PROTOCOL_ID: &str = "mutsuki.bot.reply/segment@1";
pub const BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID: &str = "mutsuki.bot.qq.reply/forward-fold@1";
pub const BOT_FLOW_INGRESS_PROTOCOL_ID: &str = "mutsuki.bot.flow/ingress@1";
pub const BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID: &str = "mutsuki.bot.flow.node/execute@1";
pub const BOT_FLOW_NODE_INVOKE_PROTOCOL_ID: &str = "mutsuki.bot.flow.node/invoke@1";
pub const BOT_FLOW_EMPTY_MENTION_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/empty-mention@1";
pub const BOT_FLOW_PROBABILITY_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/probability@1";
pub const BOT_FLOW_LINK_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/link@1";

pub const BOT_EXT_AGENT_PROFILE_ID: &str = "bot.agent.runtime_profile_id";
pub const BOT_EXT_PERSONA_PROMPT: &str = "bot.agent.persona_prompt";
pub const BOT_EXT_CONVERSATION_ICL: &str = "bot.conversation.icl";
pub const BOT_EXT_CONVERSATION_IDENTIFIERS: &str = "bot.conversation.identifiers";
pub const BOT_EXT_REPLY_SOURCE_MESSAGE_ID: &str = "bot.reply.source_message_id";
pub const BOT_EXT_REPLY_SOURCE_USER_ID: &str = "bot.reply.source_user_id";
pub const BOT_EXT_LINK_URL: &str = "bot.link.url";

pub const QQBOT_RAW_CALL_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.raw/call@1";
pub const QQBOT_ACCOUNT_GET_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.account/get@1";
pub const QQBOT_GATEWAY_STATUS_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.gateway/status@1";
pub const QQBOT_CAPABILITY_GET_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.capability/get@1";

pub type BotExtMap = std::collections::BTreeMap<String, serde_json::Value>;
