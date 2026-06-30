mod account;
mod error;
mod event;
mod message;
mod permission;
mod segment;
mod target;

pub use account::*;
pub use error::*;
pub use event::*;
pub use message::*;
pub use permission::*;
pub use segment::*;
pub use target::*;

pub const BOT_EVENT_INGEST_PROTOCOL_ID: &str = "mutsuki.bot.event/ingest@1";
pub const BOT_EVENT_HANDLE_PROTOCOL_ID: &str = "mutsuki.bot.event/handle@1";
pub const BOT_MESSAGE_SEND_PROTOCOL_ID: &str = "mutsuki.bot.message/send@1";
pub const BOT_MESSAGE_EDIT_PROTOCOL_ID: &str = "mutsuki.bot.message/edit@1";
pub const BOT_MESSAGE_RECALL_PROTOCOL_ID: &str = "mutsuki.bot.message/recall@1";
pub const BOT_MEDIA_UPLOAD_PROTOCOL_ID: &str = "mutsuki.bot.media/upload@1";
pub const BOT_MEDIA_DOWNLOAD_PROTOCOL_ID: &str = "mutsuki.bot.media/download@1";
pub const BOT_COMMAND_PARSE_PROTOCOL_ID: &str = "mutsuki.bot.command/parse@1";
pub const BOT_COMMAND_HANDLE_PROTOCOL_ID: &str = "mutsuki.bot.command/handle@1";
pub const BOT_SESSION_GET_PROTOCOL_ID: &str = "mutsuki.bot.session/get@1";
pub const BOT_SESSION_SET_PROTOCOL_ID: &str = "mutsuki.bot.session/set@1";
pub const BOT_PERMISSION_CHECK_PROTOCOL_ID: &str = "mutsuki.bot.permission/check@1";

pub const QQBOT_RAW_CALL_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.raw/call@1";
pub const QQBOT_ACCOUNT_GET_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.account/get@1";
pub const QQBOT_GATEWAY_STATUS_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.gateway/status@1";

pub type BotExtMap = std::collections::BTreeMap<String, serde_json::Value>;
