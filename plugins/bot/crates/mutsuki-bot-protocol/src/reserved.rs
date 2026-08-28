//! Reserved protocol IDs. No runner `accepted_protocol_ids` until a concrete behavior path exists.

/// Reserved. QQ adapter has no edit endpoint yet.
pub const BOT_MESSAGE_EDIT_PROTOCOL_ID: &str = "mutsuki.bot.message/edit@1";
/// Reserved. No resource-writer download runner yet.
pub const BOT_MEDIA_DOWNLOAD_PROTOCOL_ID: &str = "mutsuki.bot.media/download@1";
/// Reserved. No session plugin.
pub const BOT_SESSION_GET_PROTOCOL_ID: &str = "mutsuki.bot.session/get@1";
/// Reserved. No session plugin.
pub const BOT_SESSION_SET_PROTOCOL_ID: &str = "mutsuki.bot.session/set@1";
/// Reserved. No permission match node or plugin.
pub const BOT_PERMISSION_CHECK_PROTOCOL_ID: &str = "mutsuki.bot.permission/check@1";
/// Reserved. Superseded by `agent/submit@1` and the other Agent admin protocols.
pub const BOT_AGENT_HANDLE_PROTOCOL_ID: &str = "mutsuki.bot.agent/handle@1";
/// Reserved. Superseded by `flow.node/execute@1`.
pub const BOT_FLOW_NODE_INVOKE_PROTOCOL_ID: &str = "mutsuki.bot.flow.node/invoke@1";
