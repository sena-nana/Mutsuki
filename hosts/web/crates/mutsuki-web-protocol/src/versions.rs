//! Protocol and extension manifest version pins.

/// Current Web RPC/Event protocol version string.
///
/// 1.1: error frames carry `request_id`/`subscription_id`, hello rejection is `auth_failed`,
/// and browsers may authenticate with the `/auth/session` cookie instead of a hello token.
pub const WEB_PROTOCOL_VERSION: &str = "1.1.0";
pub const WEB_PROTOCOL_VERSION_MAJOR: u32 = 1;
pub const WEB_PROTOCOL_VERSION_MINOR: u32 = 1;

/// Current extension manifest schema version.
pub const EXTENSION_MANIFEST_VERSION: u32 = 1;
