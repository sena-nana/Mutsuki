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
pub const BOT_AGENT_BRIDGE_PROTOCOL_ID: &str = "mutsuki.bot.agent/handle@1";
pub const BOT_AGENT_SUBMIT_PROTOCOL_ID: &str = "mutsuki.bot.agent/submit@1";
pub const BOT_AGENT_CANCEL_PROTOCOL_ID: &str = "mutsuki.bot.agent/cancel@1";
pub const BOT_AGENT_RESET_PROTOCOL_ID: &str = "mutsuki.bot.agent/reset@1";
pub const BOT_AGENT_FORK_PROTOCOL_ID: &str = "mutsuki.bot.agent/fork@1";
pub const BOT_AGENT_STATUS_PROTOCOL_ID: &str = "mutsuki.bot.agent/status@1";
pub const BOT_AGENT_REGENERATE_PROTOCOL_ID: &str = "mutsuki.bot.agent/regenerate@1";
pub const BOT_FLOW_INGRESS_PROTOCOL_ID: &str = "mutsuki.bot.flow/ingress@1";
pub const BOT_FLOW_NODE_EXECUTE_PROTOCOL_ID: &str = "mutsuki.bot.flow.node/execute@1";
pub const BOT_FLOW_NODE_INVOKE_PROTOCOL_ID: &str = "mutsuki.bot.flow.node/invoke@1";

pub const QQBOT_RAW_CALL_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.raw/call@1";
pub const QQBOT_ACCOUNT_GET_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.account/get@1";
pub const QQBOT_GATEWAY_STATUS_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.gateway/status@1";
pub const QQBOT_CAPABILITY_GET_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.capability/get@1";

pub type BotExtMap = std::collections::BTreeMap<String, serde_json::Value>;
